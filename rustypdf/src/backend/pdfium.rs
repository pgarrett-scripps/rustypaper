//! The pdfium-backed [`PageSource`].
//!
//! This is the only module that knows pdfium exists. It converts pdfium's bottom-left/y-up
//! coordinate space into the top-left/y-down space the rest of the crate assumes, applies page
//! rotation, and classifies vector paths by shape so that later passes never have to look at
//! path data.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use pdfium_render::prelude::*;

use super::PageSource;
use crate::error::{Error, Result};
use crate::ir::{
    FontFlags, FontId, FontTable, Glyph, GlyphText, ImageItem, PageRaw, PathItem, PathKind, Point,
    Rect, Rgba,
};

/// A vector path at most this thick (in points) is a candidate rule rather than a filled shape.
/// `\toprule` is ~0.8pt and a fraction bar ~0.4pt; anything above 3pt is a drawn box edge.
const MAX_RULE_THICKNESS: f32 = 3.0;

/// A rule must be at least this many times longer than it is thick, which keeps small filled
/// squares (list bullets, plot markers) out of the rule sets.
const MIN_RULE_ASPECT: f32 = 3.0;

/// How deep to follow nested Form XObjects. Real documents nest one or two levels; the limit is
/// only there so a malformed or adversarial file cannot drive unbounded recursion.
const MAX_FORM_DEPTH: usize = 8;

/// Process-wide pdfium instance.
///
/// pdfium's library initialisation is global, so there is exactly one of these per process. It
/// also sidesteps a self-referential struct: with a `&'static Pdfium` the documents it hands
/// out are `PdfDocument<'static>` and can be stored alongside it.
static PDFIUM: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();

fn pdfium() -> Result<&'static Pdfium> {
    PDFIUM
        .get_or_init(|| {
            // Prefer a vendored or explicitly configured library over whatever the system has,
            // so the pdfium build we test against is the one we run against.
            let mut attempts = Vec::new();

            if let Ok(dir) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
                attempts.push(Pdfium::pdfium_platform_library_name_at_path(&dir));
            }
            for dir in ["./vendor/pdfium/lib", "../vendor/pdfium/lib", "./lib", "."] {
                attempts.push(Pdfium::pdfium_platform_library_name_at_path(dir));
            }

            let mut errors = Vec::new();
            for path in &attempts {
                match Pdfium::bind_to_library(path) {
                    Ok(bindings) => return Ok(Pdfium::new(bindings)),
                    Err(e) => errors.push(format!("{}: {e}", path.display())),
                }
            }

            match Pdfium::bind_to_system_library() {
                Ok(bindings) => Ok(Pdfium::new(bindings)),
                Err(e) => {
                    errors.push(format!("system library: {e}"));
                    Err(errors.join("; "))
                }
            }
        })
        .as_ref()
        .map_err(|e| Error::PdfiumUnavailable(e.clone()))
}

/// Serialises every call into pdfium.
///
/// pdfium is single-threaded: concurrent calls corrupt its heap, and it does not have to be the
/// *same* document for that to happen. pdfium-render's `thread_safe` feature does not help here
/// — despite the name, all it does is add `unsafe impl Send`/`Sync` to its types so they can
/// cross thread boundaries. It adds no locking whatsoever; serialising is the caller's job.
///
/// Consequence for the pipeline: ingest is serialised and the pure-Rust stages (layout, math,
/// tables) are what get parallelised. Ingest is ~5 ms/page, so this is not the bottleneck. To
/// convert many documents concurrently, shard across processes rather than threads.
static PDFIUM_LOCK: Mutex<()> = Mutex::new(());

/// Takes the pdfium lock, recovering from poisoning.
///
/// A panic while converting one document must not permanently break every later conversion in
/// the process; the guarded resource is pdfium's internal state, which we re-enter cleanly.
fn lock() -> MutexGuard<'static, ()> {
    PDFIUM_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct PdfiumBackend {
    /// `Option` only so that [`Drop`] can close the document while holding the pdfium lock;
    /// it is `Some` for the entire useful life of the backend.
    document: Option<PdfDocument<'static>>,
    path: PathBuf,
}

impl PdfiumBackend {
    /// Opens a PDF from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_password(path, None)
    }

    pub fn open_with_password(path: impl AsRef<Path>, password: Option<&str>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let _guard = lock();
        let document = pdfium()?
            .load_pdf_from_file(&path, password)
            .map_err(|e| Error::PdfOpen(format!("{}: {e}", path.display())))?;
        Ok(Self {
            document: Some(document),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The open document. Callers must already hold the pdfium lock.
    fn document(&self) -> &PdfDocument<'static> {
        self.document
            .as_ref()
            .expect("document is only taken during drop")
    }

    /// Caller must already hold the pdfium lock; the lock is not reentrant.
    fn page_count_locked(&self) -> usize {
        self.document().pages().len() as usize
    }

    /// Caller must already hold the pdfium lock.
    fn open_page(&self, index: usize) -> Result<PdfPage<'_>> {
        if index >= self.page_count_locked() {
            return Err(Error::PageOutOfRange(index));
        }
        self.document()
            .pages()
            .get(index as PdfPageIndex)
            .map_err(|e| Error::PdfPage {
                page: index,
                source: Box::new(e),
            })
    }
}

impl Drop for PdfiumBackend {
    fn drop(&mut self) {
        // Closing a document calls into pdfium, so it has to happen under the lock like every
        // other call. Dropping the field here rather than letting it fall out of scope is what
        // keeps it inside the guard.
        let _guard = lock();
        drop(self.document.take());
    }
}

/// Maps pdfium's bottom-left/y-up page space into our top-left/y-down space, applying the
/// page's `/Rotate` so that downstream code sees the page as a reader sees it.
#[derive(Debug, Clone, Copy)]
struct Transform {
    /// Unrotated page size.
    src_w: f32,
    src_h: f32,
    rotation: i32,
}

impl Transform {
    fn new(src_w: f32, src_h: f32, rotation: i32) -> Self {
        Self {
            src_w,
            src_h,
            rotation,
        }
    }

    /// Size of the page as displayed.
    fn out_size(&self) -> (f32, f32) {
        match self.rotation {
            90 | 270 => (self.src_h, self.src_w),
            _ => (self.src_w, self.src_h),
        }
    }

    /// Converts a point. `y_up` is measured from the bottom of the unrotated page.
    fn point(&self, x: f32, y_up: f32) -> Point {
        // Flip to a top-left origin first, then rotate clockwise by /Rotate.
        let (x, y) = (x, self.src_h - y_up);
        let (x, y) = match self.rotation {
            90 => (self.src_h - y, x),
            180 => (self.src_w - x, self.src_h - y),
            270 => (y, self.src_w - x),
            _ => (x, y),
        };
        Point::new(x, y)
    }

    fn rect(&self, left: f32, bottom: f32, right: f32, top: f32) -> Rect {
        let a = self.point(left, bottom);
        let b = self.point(right, top);
        Rect::from_corners(a.x, a.y, b.x, b.y)
    }
}

fn color_to_rgba(color: PdfColor) -> Rgba {
    Rgba::new(color.red(), color.green(), color.blue(), color.alpha())
}

/// Tolerance, in points, for calling two coordinates equal when testing for a rectangle.
const RECT_EPSILON: f32 = 0.01;

/// Distinguishes a real axis-aligned rectangle from arbitrary artwork.
///
/// The distinction earns its keep downstream: rectangles are cell shading and frames, which
/// table detection cares about, while everything else is figure content, and a dense cluster of
/// it is how a vector figure is recognised. Classifying every painted path as a box would
/// collapse both signals into one.
///
/// Points are tested in the path's own space. Form matrices in practice only translate and
/// scale, both of which preserve axis alignment.
fn is_axis_aligned_rect(path: &PdfPagePathObject) -> bool {
    let segments = path.segments();
    let count = segments.len();
    if !(4..=6).contains(&count) {
        return false;
    }

    let mut points: Vec<(f32, f32)> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let Ok(segment) = segments.get(i) else {
            return false;
        };
        if segment.segment_type() == PdfPathSegmentType::BezierTo {
            return false;
        }
        let point = (segment.x().value, segment.y().value);
        let duplicate = points.last().is_some_and(|p| same_point(*p, point));
        if !duplicate {
            points.push(point);
        }
    }

    // A closed path repeats its starting point.
    if points.len() > 1 && same_point(points[0], *points.last().unwrap()) {
        points.pop();
    }
    if points.len() != 4 {
        return false;
    }

    (0..4).all(|i| {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % 4];
        (x1 - x0).abs() <= RECT_EPSILON || (y1 - y0).abs() <= RECT_EPSILON
    })
}

fn same_point(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() <= RECT_EPSILON && (a.1 - b.1).abs() <= RECT_EPSILON
}

fn classify_path(bbox: &Rect) -> (PathKind, f32) {
    let (w, h) = (bbox.width(), bbox.height());

    if h <= MAX_RULE_THICKNESS && w >= h * MIN_RULE_ASPECT {
        (PathKind::HorizontalRule, h)
    } else if w <= MAX_RULE_THICKNESS && h >= w * MIN_RULE_ASPECT {
        (PathKind::VerticalRule, w)
    } else {
        (PathKind::Other, w.min(h))
    }
}

impl PageSource for PdfiumBackend {
    fn page_count(&self) -> usize {
        let _guard = lock();
        self.page_count_locked()
    }

    fn page(&self, index: usize, fonts: &mut FontTable) -> Result<PageRaw> {
        let _guard = lock();
        let page = self.open_page(index)?;

        let size = page.page_size();
        let rotation = page
            .rotation()
            .map(|r| r.as_degrees() as i32)
            .unwrap_or(0)
            .rem_euclid(360);
        let xform = Transform::new(size.width().value, size.height().value, rotation);
        let (width, height) = xform.out_size();

        let mut raw = PageRaw {
            index,
            width,
            height,
            rotation,
            ..Default::default()
        };

        collect_glyphs(&page, &xform, fonts, &mut raw);
        collect_objects(&page, &xform, &mut raw);

        Ok(raw)
    }

    fn render_region(&self, index: usize, region: Rect, dpi: f32) -> Result<Vec<u8>> {
        use image::ImageEncoder;

        let _guard = lock();
        let page = self.open_page(index)?;
        let scale = dpi / 72.0;

        let fail = |e: PdfiumError| Error::PdfPage {
            page: index,
            source: Box::new(e),
        };

        let bitmap = page
            .render_with_config(
                &PdfRenderConfig::new()
                    .scale_page_by_factor(scale)
                    .render_annotations(false)
                    .render_form_data(false),
            )
            .map_err(fail)?;

        let full = bitmap.as_image().map_err(fail)?.to_rgba8();

        // `region` is already in our top-left/y-down space, which is also the bitmap's, so the
        // crop is a straight scale. Clamp so a slightly oversized bbox cannot panic.
        let (iw, ih) = (full.width(), full.height());
        let x = ((region.x0 * scale).floor().max(0.0) as u32).min(iw.saturating_sub(1));
        let y = ((region.y0 * scale).floor().max(0.0) as u32).min(ih.saturating_sub(1));
        let w = ((region.width() * scale).ceil().max(1.0) as u32).min(iw - x);
        let h = ((region.height() * scale).ceil().max(1.0) as u32).min(ih - y);

        let cropped = image::imageops::crop_imm(&full, x, y, w, h).to_image();

        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                cropped.as_raw(),
                cropped.width(),
                cropped.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| Error::PdfPage {
                page: index,
                source: Box::new(e),
            })?;

        Ok(png)
    }
}

fn collect_glyphs(page: &PdfPage, xform: &Transform, fonts: &mut FontTable, raw: &mut PageRaw) {
    let Ok(text) = page.text() else {
        return;
    };

    let chars = text.chars();
    raw.glyphs.reserve(chars.len());

    // Consecutive characters nearly always share a font, so remembering the last name turns the
    // per-glyph font lookup into a string compare instead of a hash.
    //
    // TODO(perf): `font_name()` still allocates a `String` per character inside pdfium-render.
    // If ingest shows up in the profile, drop to the raw `FPDFText_GetFontInfo` binding and read
    // into a reusable buffer.
    let mut last: Option<(String, FontId)> = None;

    for ch in chars.iter() {
        let Some(c) = ch.unicode_char() else {
            continue;
        };
        // pdfium reports layout breaks as control characters. Line building here is geometric,
        // so they carry no information and would only pollute the glyph stream.
        if c.is_control() {
            continue;
        }

        let name = ch.font_name();
        let font = match &last {
            Some((prev, id)) if *prev == name => *id,
            _ => {
                let id = fonts.intern(&name);
                last = Some((name, id));
                id
            }
        };

        let bounds = ch
            .tight_bounds()
            .or_else(|_| ch.loose_bounds())
            .unwrap_or(PdfRect::ZERO);
        let bbox = xform.rect(
            bounds.left().value,
            bounds.bottom().value,
            bounds.right().value,
            bounds.top().value,
        );

        let origin = ch
            .origin()
            .map(|(x, y)| xform.point(x.value, y.value))
            .unwrap_or(Point::new(bbox.x0, bbox.y1));

        let mut flags = FontFlags::default();
        flags.set(FontFlags::ITALIC, ch.font_is_italic());
        flags.set(FontFlags::SERIF, ch.font_is_serif());
        flags.set(FontFlags::SANS_SERIF, ch.font_is_sans_serif());
        flags.set(FontFlags::SYMBOLIC, ch.font_is_symbolic());
        flags.set(FontFlags::FIXED_PITCH, ch.font_is_fixed_pitch());
        flags.set(FontFlags::CURSIVE, ch.font_is_cursive());
        flags.set(FontFlags::SMALL_CAPS, ch.font_is_small_caps());
        flags.set(FontFlags::ALL_CAPS, ch.font_is_all_caps());
        // The descriptor's "bold" bit alone misses fonts that are bold by weight but not by
        // flag, which is common in publisher-typeset PDFs.
        let bold = ch.font_weight().is_some_and(is_bold_weight) || ch.font_is_bold_reenforced();
        flags.set(FontFlags::BOLD, bold);

        let angle = ch.angle_radians().unwrap_or(0.0);
        raw.glyphs.push(Glyph {
            text: GlyphText::Char(c),
            bbox,
            origin,
            font,
            size: glyph_size(&ch, &bbox, angle),
            angle,
            flags,
            color: ch.fill_color().map(color_to_rgba).unwrap_or(Rgba::BLACK),
            generated: ch.is_generated().unwrap_or(false),
        });
    }
}

/// Intersects an object's bounds with the page, returning `None` when nothing is left visible.
fn clip_to_page(bbox: Rect, width: f32, height: f32) -> Option<Rect> {
    if !bbox.x0.is_finite() || !bbox.y0.is_finite() || !bbox.x1.is_finite() || !bbox.y1.is_finite()
    {
        return None;
    }
    let clipped = Rect {
        x0: bbox.x0.max(0.0),
        y0: bbox.y0.max(0.0),
        x1: bbox.x1.min(width),
        y1: bbox.y1.min(height),
    };
    (clipped.width() > 0.0 && clipped.height() > 0.0).then_some(clipped)
}

/// Effective font size in points, with fallbacks.
///
/// pdfium reports a scaled size of 0 for text whose matrix is a pure rotation — the arXiv stamp
/// down the margin of every preprint is the case that surfaced this. Size is load-bearing
/// everywhere downstream (baseline tolerance, word gaps, heading detection), so the invariant
/// that it is positive and finite is established here rather than defended in every consumer.
fn glyph_size(ch: &PdfPageTextChar, bbox: &Rect, angle: f32) -> f32 {
    let scaled = ch.scaled_font_size().value;
    if scaled.is_finite() && scaled > 0.0 {
        return scaled;
    }
    let unscaled = ch.unscaled_font_size().value;
    if unscaled.is_finite() && unscaled > 0.0 {
        return unscaled;
    }

    // Last resort: infer from the ink. Font size runs *across* the baseline, so for sideways
    // text that is the box's width, not its height — taking the larger of the two would report
    // the advance and make a margin stamp look like display type.
    let upright = (angle.to_degrees().rem_euclid(180.0) - 90.0).abs() >= 45.0;
    let extent = if upright { bbox.height() } else { bbox.width() };
    if extent.is_finite() && extent > 0.0 {
        // A cap-height glyph is roughly 0.7em.
        extent / 0.7
    } else {
        1.0
    }
}

fn is_bold_weight(weight: PdfFontWeight) -> bool {
    match weight {
        PdfFontWeight::Weight100 | PdfFontWeight::Weight200 | PdfFontWeight::Weight300 => false,
        PdfFontWeight::Weight400Normal | PdfFontWeight::Weight500 => false,
        PdfFontWeight::Weight600
        | PdfFontWeight::Weight700Bold
        | PdfFontWeight::Weight800
        | PdfFontWeight::Weight900 => true,
        PdfFontWeight::Custom(w) => w >= 600,
    }
}

fn collect_objects(page: &PdfPage, xform: &Transform, raw: &mut PageRaw) {
    let objects = page.objects();
    let children: Vec<_> = (0..objects.len())
        .filter_map(|i| objects.get(i).ok())
        .collect();
    let mut next_index = 0;
    collect_from(
        children,
        PdfMatrix::identity(),
        xform,
        raw,
        0,
        &mut next_index,
    );
}

/// Walks a page's object tree, descending into Form XObjects.
///
/// This descent is not optional: LaTeX includes figures via `\includegraphics`, which lands in
/// the content stream as a Form XObject. Treating forms as opaque loses every rule and image in
/// every figure — on a typical paper that is most of the vector content on the page.
///
/// Child objects report bounds in their form's coordinate space, so the form matrix accumulates
/// down the tree. `apply_to_points` uses the row-vector convention, meaning a point maps as
/// `p · form · parent`, hence `form.multiply(parent)`.
fn collect_from(
    objects: Vec<PdfPageObject<'_>>,
    ctm: PdfMatrix,
    xform: &Transform,
    raw: &mut PageRaw,
    depth: usize,
    next_index: &mut usize,
) {
    let (page_width, page_height) = (raw.width, raw.height);
    for object in objects {
        let i = *next_index;
        *next_index += 1;

        let Ok(bounds) = object.bounds() else {
            continue;
        };
        let bounds = bounds.transform(ctm).to_rect();
        let bbox = xform.rect(
            bounds.left().value,
            bounds.bottom().value,
            bounds.right().value,
            bounds.top().value,
        );
        // A clipped path reports the bounds of its *unclipped* geometry, which can run tens of
        // thousands of points off-page. Taking that at face value let a single object drag a
        // figure region to 6000% of the page and swallow every block on it. What is visible is
        // what falls on the page, so that is what gets recorded.
        let Some(bbox) = clip_to_page(bbox, page_width, page_height) else {
            continue;
        };

        match object.object_type() {
            PdfPageObjectType::XObjectForm => {
                if depth >= MAX_FORM_DEPTH {
                    continue;
                }
                let Some(form) = object.as_x_object_form_object() else {
                    continue;
                };
                let inner = form.matrix().unwrap_or(PdfMatrix::identity()).multiply(ctm);
                let children: Vec<_> = (0..form.len()).filter_map(|i| form.get(i).ok()).collect();
                collect_from(children, inner, xform, raw, depth + 1, next_index);
            }
            PdfPageObjectType::Path => {
                let (mut kind, mut thickness) = classify_path(&bbox);
                let path = object.as_path_object();
                let filled = path
                    .and_then(|p| p.fill_mode().ok())
                    .is_some_and(|m| m != PdfPathFillMode::None);
                let stroked = path.and_then(|p| p.is_stroked().ok()).unwrap_or(false);

                if kind == PathKind::Other {
                    if let Some(path) = path {
                        if (filled || stroked) && !bbox.is_empty() && is_axis_aligned_rect(path) {
                            kind = PathKind::Box;
                        }
                    }
                } else {
                    // A rule drawn as a stroked line has a bbox that may collapse to zero
                    // thickness, with the real width in the stroke.
                    thickness =
                        thickness.max(object.stroke_width().map(|w| w.value).unwrap_or(0.0));
                }

                raw.paths.push(PathItem {
                    bbox,
                    kind,
                    thickness,
                    color: object
                        .fill_color()
                        .or_else(|_| object.stroke_color())
                        .map(color_to_rgba)
                        .unwrap_or(Rgba::BLACK),
                    filled,
                    stroked,
                });
            }
            PdfPageObjectType::Image => {
                let image = object.as_image_object();
                raw.images.push(ImageItem {
                    bbox,
                    object_index: i,
                    pixel_width: image.and_then(|im| im.width().ok()).unwrap_or(0) as u32,
                    pixel_height: image.and_then(|im| im.height().ok()).unwrap_or(0) as u32,
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_is_identity_for_unrotated_pages() {
        let t = Transform::new(612.0, 792.0, 0);
        assert_eq!(t.out_size(), (612.0, 792.0));
        // The bottom-left of the PDF page is the bottom-left on screen: only y flips.
        let p = t.point(0.0, 0.0);
        assert_eq!((p.x, p.y), (0.0, 792.0));
        let p = t.point(0.0, 792.0);
        assert_eq!((p.x, p.y), (0.0, 0.0));
    }

    #[test]
    fn transform_rotates_page_size_and_corners() {
        let t = Transform::new(612.0, 792.0, 90);
        assert_eq!(t.out_size(), (792.0, 612.0));
        // The top-left of the unrotated page ends up top-right after a 90 degree clockwise turn.
        let p = t.point(0.0, 792.0);
        assert_eq!((p.x, p.y), (792.0, 0.0));

        let t = Transform::new(612.0, 792.0, 270);
        assert_eq!(t.out_size(), (792.0, 612.0));
        let p = t.point(0.0, 792.0);
        assert_eq!((p.x, p.y), (0.0, 612.0));
    }

    #[test]
    fn transform_180_maps_opposite_corners() {
        let t = Transform::new(600.0, 800.0, 180);
        let p = t.point(0.0, 0.0);
        assert_eq!((p.x, p.y), (600.0, 0.0));
    }

    #[test]
    fn rect_conversion_normalises_after_flip() {
        let t = Transform::new(612.0, 792.0, 0);
        // A box near the bottom of the page in PDF space.
        let r = t.rect(100.0, 50.0, 200.0, 80.0);
        assert_eq!((r.x0, r.x1), (100.0, 200.0));
        // y flips, so the PDF top edge (80) becomes the smaller y.
        assert_eq!((r.y0, r.y1), (712.0, 742.0));
        assert!(r.y0 < r.y1);
    }

    #[test]
    fn path_classification() {
        // A booktabs rule: long and 0.8pt thick.
        let rule = Rect::from_corners(72.0, 300.0, 540.0, 300.8);
        assert_eq!(classify_path(&rule).0, PathKind::HorizontalRule);

        // A tabular column separator.
        let vline = Rect::from_corners(300.0, 100.0, 300.5, 400.0);
        assert_eq!(classify_path(&vline).0, PathKind::VerticalRule);

        // A list bullet: thin, but not long enough to be a rule.
        let bullet = Rect::from_corners(72.0, 300.0, 74.0, 302.0);
        assert_eq!(classify_path(&bullet).0, PathKind::Other);

        // A framed figure.
        let boxy = Rect::from_corners(72.0, 100.0, 540.0, 400.0);
        assert_eq!(classify_path(&boxy).0, PathKind::Other);
    }

    #[test]
    fn off_page_geometry_is_clipped_to_the_page() {
        // What a clipped path reports: bounds far outside the page in both directions.
        let huge = Rect::from_corners(106.0, -38870.0, 492.0, 39239.0);
        let clipped = clip_to_page(huge, 595.0, 842.0).expect("some of it is on the page");
        assert_eq!((clipped.x0, clipped.y0), (106.0, 0.0));
        assert_eq!((clipped.x1, clipped.y1), (492.0, 842.0));

        // Entirely off-page geometry contributes nothing.
        assert_eq!(
            clip_to_page(Rect::from_corners(-500.0, -500.0, -100.0, -100.0), 595.0, 842.0),
            None
        );
        // Degenerate coordinates must not produce a rect at all.
        assert_eq!(
            clip_to_page(Rect::from_corners(f32::NAN, 0.0, 10.0, 10.0), 595.0, 842.0),
            None
        );
    }

    #[test]
    fn fraction_bar_is_a_horizontal_rule() {
        // 12pt wide, 0.4pt thick: what `\frac` draws.
        let bar = Rect::from_corners(100.0, 200.0, 112.0, 200.4);
        let (kind, thickness) = classify_path(&bar);
        assert_eq!(kind, PathKind::HorizontalRule);
        assert!((thickness - 0.4).abs() < 1e-5);
    }
}
