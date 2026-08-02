//! The rustium-backed [`PageSource`]: a pure-Rust PDF interpreter, no FFI and no global state.
//!
//! rustium reports primitives in PDF user space — bottom-left origin, y-up, before `/Rotate`.
//! Everything above this module assumes top-left/y-down with rotation already applied, and
//! `Page::page_matrix` produces exactly that transform, so the conversion is one matrix applied
//! to every point rather than the hand-rolled case analysis a raw coordinate flip would need.

use std::path::{Path, PathBuf};

use rustium::font::glyph::PathCmd;
use rustium::geom::Matrix;
use rustium::page::Page;

use super::{classify_path, clip_to_page, expand_ligature, resolve_size, PageSource, RECT_EPSILON};
use crate::error::{Error, Result};
use crate::ir::{
    FontFlags, FontId, FontTable, Glyph, ImageItem, PageRaw, PathItem, PathKind, Point, Rect, Rgba,
};

pub struct RustiumBackend {
    document: rustium::Document,
    path: PathBuf,
}

impl RustiumBackend {
    /// Opens a PDF from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_password(path, None)
    }

    pub fn open_with_password(path: impl AsRef<Path>, password: Option<&str>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let document = rustium::Document::open_with_password(&path, password)
            .map_err(|e| Error::PdfOpen(format!("{}: {e}", path.display())))?;
        Ok(Self { document, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open_page(&self, index: usize) -> Result<Page> {
        if index >= self.document.page_count() {
            return Err(Error::PageOutOfRange(index));
        }
        self.document.page(index).map_err(|e| Error::PdfPage {
            page: index,
            source: Box::new(e),
        })
    }
}

/// Maps rustium's user space onto our page space, and carries the page size so that geometry can
/// be clipped as it is converted.
struct Space {
    to_page: Matrix,
    width: f32,
    height: f32,
}

impl Space {
    fn new(page: &Page) -> Self {
        Self {
            // Unit scale: our page space is in points, exactly what a 1:1 page matrix gives.
            to_page: page.page_matrix(1.0),
            width: page.width(),
            height: page.height(),
        }
    }

    fn point(&self, p: rustium::geom::Point) -> Point {
        let p = self.to_page.apply(p);
        Point::new(p.x, p.y)
    }

    fn rect(&self, r: &rustium::geom::Rect) -> Rect {
        let a = self.point(rustium::geom::Point::new(r.x0, r.y0));
        let b = self.point(rustium::geom::Point::new(r.x1, r.y1));
        Rect::from_corners(a.x, a.y, b.x, b.y)
    }

    /// Converts a baseline rotation from user space into page space.
    ///
    /// The page matrix flips y and may rotate, so a glyph's angle cannot be carried across
    /// unchanged: transforming the baseline direction vector applies both in one step.
    fn angle(&self, rotation: f32) -> f32 {
        let (dx, dy) = self.to_page.apply_vector(rotation.cos(), rotation.sin());
        if dx == 0.0 && dy == 0.0 {
            0.0
        } else {
            dy.atan2(dx)
        }
    }
}

/// Converts a colour component in 0..=1 to 8-bit, clamping out-of-gamut values.
fn channel(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn to_rgba(rgb: [f32; 3], alpha: f32) -> Rgba {
    Rgba::new(
        channel(rgb[0]),
        channel(rgb[1]),
        channel(rgb[2]),
        channel(alpha),
    )
}

fn to_flags(flags: rustium::FontFlags) -> FontFlags {
    let mut out = FontFlags::default();
    out.set(FontFlags::ITALIC, flags.is_italic());
    out.set(FontFlags::SERIF, flags.is_serif());
    out.set(FontFlags::SYMBOLIC, flags.is_symbolic());
    out.set(FontFlags::FIXED_PITCH, flags.is_fixed_pitch());
    out.set(FontFlags::BOLD, flags.is_bold());
    out
}

/// Whether a path's commands trace an axis-aligned rectangle.
///
/// The distinction earns its keep downstream: rectangles are cell shading and frames, which
/// table detection cares about, while everything else is figure content, and a dense cluster of
/// it is how a vector figure is recognised. Classifying every painted path as a box would
/// collapse both signals into one.
fn is_axis_aligned_rect(cmds: &[PathCmd]) -> bool {
    let mut points: Vec<(f32, f32)> = Vec::with_capacity(5);
    for cmd in cmds {
        match cmd {
            PathCmd::CurveTo(..) => return false,
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => {
                // A subpath that restarts means more than one shape, which is not a plain rect.
                if matches!(cmd, PathCmd::MoveTo(_)) && !points.is_empty() {
                    return false;
                }
                let point = (p.x, p.y);
                if !points.last().is_some_and(|last| same_point(*last, point)) {
                    points.push(point);
                }
            }
            PathCmd::Close => {}
        }
        if points.len() > 5 {
            return false;
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

impl PageSource for RustiumBackend {
    fn page_count(&self) -> usize {
        self.document.page_count()
    }

    fn page(&self, index: usize, fonts: &mut FontTable) -> Result<PageRaw> {
        let page = self.open_page(index)?;
        let space = Space::new(&page);

        let mut raw = PageRaw {
            index,
            width: space.width,
            height: space.height,
            rotation: page.rotation.rem_euclid(360),
            ..Default::default()
        };

        collect_glyphs(&page, &space, fonts, &mut raw);
        collect_paths(&page, &space, &mut raw);
        collect_images(&page, &space, &mut raw);

        Ok(raw)
    }

    fn render_region(&self, index: usize, region: Rect, dpi: f32) -> Result<Vec<u8>> {
        let page = self.open_page(index)?;
        let fail = |e: rustium::Error| Error::PdfPage {
            page: index,
            source: Box::new(e),
        };

        // `region` is already in the y-down page space rustium renders into, so it crosses over
        // unchanged; rustium clamps it to the page itself.
        let region = rustium::geom::Rect {
            x0: region.x0,
            y0: region.y0,
            x1: region.x1,
            y1: region.y1,
        };
        page.render_region(
            &self.document,
            Some(region),
            rustium::RenderOptions::at_dpi(dpi),
        )
        .map_err(fail)?
        .to_png()
        .map_err(fail)
    }
}

/// Rewrites a glyph's text with its ligatures expanded, or `None` when it contains none.
fn expanded(text: &str) -> Option<String> {
    if !text.chars().any(|c| expand_ligature(c).is_some()) {
        return None;
    }
    let mut out = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match expand_ligature(c) {
            Some(letters) => out.push_str(letters),
            None => out.push(c),
        }
    }
    Some(out)
}

fn collect_glyphs(page: &Page, space: &Space, fonts: &mut FontTable, raw: &mut PageRaw) {
    raw.glyphs.reserve(page.glyphs.len());

    // Consecutive glyphs nearly always share a font, so the interned id and converted flags are
    // remembered across the run rather than recomputed per glyph.
    let mut last: Option<(&str, FontId, FontFlags)> = None;

    for g in &page.glyphs {
        // Downstream line building is geometric, so layout control characters carry no
        // information and would only pollute the glyph stream.
        if g.text.is_empty() || g.text.chars().all(|c| c.is_control()) {
            continue;
        }

        // `is_math_font` and the heading heuristics match on the embedded font's real name
        // (`XFBAPD+CMMI10`), which is `/BaseFont`. The resource name (`/F13`) says nothing.
        let name: &str = if g.base_font.is_empty() {
            &g.font_name
        } else {
            &g.base_font
        };
        let (font, flags) = match last {
            Some((prev, id, flags)) if prev == name => (id, flags),
            _ => {
                let id = fonts.intern(name);
                let flags = to_flags(g.flags);
                // Borrowing from `page`, which outlives the loop, keeps this allocation-free.
                let name: &str = if g.base_font.is_empty() {
                    &g.font_name
                } else {
                    &g.base_font
                };
                last = Some((name, id, flags));
                (id, flags)
            }
        };

        let bbox = space.rect(&g.bbox);
        let angle = space.angle(g.rotation);
        // A ligature is one glyph on the page but several letters in the text; `add_expansion`
        // keeps the common single-character case allocation-free.
        let text = match expanded(&g.text) {
            Some(expanded) => raw.add_expansion(&expanded),
            None => raw.add_expansion(&g.text),
        };
        raw.glyphs.push(Glyph {
            text,
            bbox,
            origin: space.point(g.origin),
            font,
            size: resolve_size(g.font_size, &bbox, angle),
            angle,
            flags,
            color: to_rgba(g.color, g.alpha),
            generated: g.is_generated_space,
        });
    }
}

/// Splits a path's commands into subpaths, each starting at a `MoveTo`.
///
/// rustium emits one item per painting operator, so a whole `tabular`'s rules — or an entire
/// plot's axes — can arrive as a single path with dozens of subpaths. Classifying that by its
/// combined bounding box calls a table grid one large `Other` and loses every rule in it, which
/// is what table detection runs on. Splitting here keeps the promise this module makes to
/// everything above it: one item per subpath, whatever the reader underneath emits.
fn subpaths(cmds: &[PathCmd]) -> Vec<&[PathCmd]> {
    let mut starts: Vec<usize> = cmds
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c, PathCmd::MoveTo(_)))
        .map(|(i, _)| i)
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(cmds.len());
            &cmds[start..end]
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// The thinnest stroke to assume when a path sets a line width of zero.
///
/// PDF defines width 0 as the thinnest line the device can render — one pixel — which is real
/// ink, not nothing. Rules in TeX output are routinely drawn this way.
const HAIRLINE: f32 = 0.1;

/// Grows a stroked path's bounds by the ink the stroke lays outside its geometry.
///
/// This is load-bearing, not cosmetic. A horizontal rule is a single `m`/`l` segment whose
/// geometric bounding box has **exactly zero height**, so without this it is discarded as
/// degenerate before it can ever be classified — which silently cost every `\hline`, every
/// `\toprule` and every fraction bar on the page.
fn inflate_stroke(r: rustium::geom::Rect, line_width: f32) -> rustium::geom::Rect {
    let half = line_width.max(HAIRLINE) / 2.0;
    rustium::geom::Rect {
        x0: r.x0 - half,
        y0: r.y0 - half,
        x1: r.x1 + half,
        y1: r.y1 + half,
    }
}

fn bounds_of(cmds: &[PathCmd]) -> rustium::geom::Rect {
    let mut points = cmds.iter().flat_map(|c| match c {
        PathCmd::MoveTo(p) | PathCmd::LineTo(p) => vec![*p],
        PathCmd::CurveTo(a, b, c) => vec![*a, *b, *c],
        PathCmd::Close => vec![],
    });
    let Some(first) = points.next() else {
        return rustium::geom::Rect::default();
    };
    let mut r = rustium::geom::Rect::from_corners(first.x, first.y, first.x, first.y);
    for p in points {
        r.x0 = r.x0.min(p.x);
        r.y0 = r.y0.min(p.y);
        r.x1 = r.x1.max(p.x);
        r.y1 = r.y1.max(p.y);
    }
    r
}

fn collect_paths(page: &Page, space: &Space, raw: &mut PageRaw) {
    for p in &page.paths {
        let filled = p.fill.is_some();
        let stroked = p.stroke.is_some();

        // A stroke paints each subpath independently, so splitting is exactly what the page
        // does. A fill treats all of them as one region under a winding rule — an `O` is two
        // subpaths and one shape — so a filled path is classified whole.
        let pieces: Vec<&[PathCmd]> = if stroked && !filled {
            subpaths(&p.cmds)
        } else {
            vec![&p.cmds[..]]
        };

        for piece in pieces {
            let mut geometry = bounds_of(piece);
            if stroked {
                geometry = inflate_stroke(geometry, p.line_width);
            }
            let Some(bbox) = clip_to_page(space.rect(&geometry), space.width, space.height) else {
                continue;
            };

            let (mut kind, _) = classify_path(&bbox);
            let mut thickness = if kind == PathKind::HorizontalRule {
                bbox.height()
            } else {
                bbox.width()
            };
            if kind == PathKind::Other {
                if (filled || stroked) && !bbox.is_empty() && is_axis_aligned_rect(piece) {
                    kind = PathKind::Box;
                }
                thickness = bbox.width().min(bbox.height());
            }

            raw.paths.push(PathItem {
                bbox,
                kind,
                thickness,
                color: to_rgba(
                    p.fill.or(p.stroke).unwrap_or([0.0, 0.0, 0.0]),
                    if filled { p.fill_alpha } else { p.stroke_alpha },
                ),
                filled,
                stroked,
            });
        }
    }
}

fn collect_images(page: &Page, space: &Space, raw: &mut PageRaw) {
    for (i, im) in page.images.iter().enumerate() {
        let Some(bbox) = clip_to_page(space.rect(&im.bbox), space.width, space.height) else {
            continue;
        };
        raw.images.push(ImageItem {
            bbox,
            object_index: i,
            pixel_width: im.width,
            pixel_height: im.height,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> rustium::geom::Point {
        rustium::geom::Point::new(x, y)
    }

    /// Builds a `Space` for a page of the given size and rotation without needing a real PDF.
    fn space(w: f32, h: f32, rotation: i32) -> Space {
        // Mirrors `Page::page_matrix`: shift to the origin, flip y, then apply `/Rotate`.
        let flip = Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, h);
        let rotate = match rotation.rem_euclid(360) {
            90 => Matrix::new(0.0, 1.0, -1.0, 0.0, h, 0.0),
            180 => Matrix::new(-1.0, 0.0, 0.0, -1.0, w, h),
            270 => Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, w),
            _ => Matrix::IDENTITY,
        };
        let (out_w, out_h) = if rotation.rem_euclid(180) == 0 {
            (w, h)
        } else {
            (h, w)
        };
        Space {
            to_page: flip.concat(&rotate),
            width: out_w,
            height: out_h,
        }
    }

    #[test]
    fn user_space_origin_lands_at_the_page_top_left() {
        let s = space(612.0, 792.0, 0);
        // PDF's origin is the bottom-left, which is the bottom-left on screen: only y flips.
        let p = s.point(pt(0.0, 0.0));
        assert_eq!((p.x, p.y), (0.0, 792.0));
        let p = s.point(pt(0.0, 792.0));
        assert_eq!((p.x, p.y), (0.0, 0.0));
    }

    #[test]
    fn rotated_pages_swap_extent_and_move_corners() {
        let s = space(612.0, 792.0, 90);
        assert_eq!((s.width, s.height), (792.0, 612.0));
        // The top-left of the unrotated page ends up top-right after a 90 degree clockwise turn.
        let p = s.point(pt(0.0, 792.0));
        assert_eq!((p.x, p.y), (792.0, 0.0));
    }

    #[test]
    fn rect_conversion_normalises_after_the_flip() {
        let s = space(612.0, 792.0, 0);
        // A box near the bottom of the page in PDF space.
        let r = s.rect(&rustium::geom::Rect::from_corners(100.0, 50.0, 200.0, 80.0));
        assert_eq!((r.x0, r.x1), (100.0, 200.0));
        // y flips, so the PDF top edge (80) becomes the smaller y.
        assert_eq!((r.y0, r.y1), (712.0, 742.0));
        assert!(r.y0 < r.y1);
    }

    #[test]
    fn horizontal_text_stays_horizontal_through_the_y_flip() {
        let s = space(612.0, 792.0, 0);
        // The flip negates the angle; `is_horizontal` cares about the magnitude, but a sign
        // error here would put upright text at 180 degrees on a rotated page.
        assert!(s.angle(0.0).abs() < 1e-6);
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!((s.angle(quarter).abs() - quarter).abs() < 1e-5);
    }

    #[test]
    fn a_closed_rectangle_is_recognised() {
        let cmds = vec![
            PathCmd::MoveTo(pt(10.0, 10.0)),
            PathCmd::LineTo(pt(50.0, 10.0)),
            PathCmd::LineTo(pt(50.0, 30.0)),
            PathCmd::LineTo(pt(10.0, 30.0)),
            PathCmd::LineTo(pt(10.0, 10.0)),
            PathCmd::Close,
        ];
        assert!(is_axis_aligned_rect(&cmds));
    }

    #[test]
    fn curves_and_diagonals_are_not_rectangles() {
        let curved = vec![
            PathCmd::MoveTo(pt(0.0, 0.0)),
            PathCmd::CurveTo(pt(1.0, 1.0), pt(2.0, 2.0), pt(3.0, 3.0)),
            PathCmd::Close,
        ];
        assert!(!is_axis_aligned_rect(&curved));

        // Four points, but one edge runs diagonally.
        let diamond = vec![
            PathCmd::MoveTo(pt(10.0, 0.0)),
            PathCmd::LineTo(pt(20.0, 10.0)),
            PathCmd::LineTo(pt(10.0, 20.0)),
            PathCmd::LineTo(pt(0.0, 10.0)),
            PathCmd::Close,
        ];
        assert!(!is_axis_aligned_rect(&diamond));
    }

    #[test]
    fn two_subpaths_are_not_a_single_rectangle() {
        let cmds = vec![
            PathCmd::MoveTo(pt(0.0, 0.0)),
            PathCmd::LineTo(pt(10.0, 0.0)),
            PathCmd::MoveTo(pt(20.0, 0.0)),
            PathCmd::LineTo(pt(30.0, 0.0)),
        ];
        assert!(!is_axis_aligned_rect(&cmds));
    }

    #[test]
    fn a_grid_drawn_in_one_operator_splits_into_its_rules() {
        // What `\hline` twice in one `S` looks like: two open segments, one path object.
        let cmds = vec![
            PathCmd::MoveTo(pt(72.0, 300.0)),
            PathCmd::LineTo(pt(540.0, 300.0)),
            PathCmd::MoveTo(pt(72.0, 260.0)),
            PathCmd::LineTo(pt(540.0, 260.0)),
        ];
        let pieces = subpaths(&cmds);
        assert_eq!(pieces.len(), 2, "one subpath per rule");
        for piece in pieces {
            let b = bounds_of(piece);
            assert_eq!((b.x0, b.x1), (72.0, 540.0));
            // Each piece is a flat segment, which is what makes it classify as a rule rather
            // than as the enclosing box the combined path would have produced.
            assert_eq!(b.y0, b.y1);
        }

        // Combined, the same commands span the whole block and would classify as `Other`.
        let whole = bounds_of(&cmds);
        assert_eq!((whole.y0, whole.y1), (260.0, 300.0));
    }

    #[test]
    fn a_path_with_no_leading_moveto_is_still_one_subpath() {
        let cmds = vec![PathCmd::LineTo(pt(1.0, 1.0)), PathCmd::LineTo(pt(2.0, 2.0))];
        assert_eq!(subpaths(&cmds).len(), 1);
        assert!(subpaths(&[]).is_empty());
    }

    #[test]
    fn curve_control_points_are_inside_the_bounds() {
        let cmds = vec![
            PathCmd::MoveTo(pt(0.0, 0.0)),
            PathCmd::CurveTo(pt(5.0, 20.0), pt(15.0, -4.0), pt(20.0, 0.0)),
        ];
        let b = bounds_of(&cmds);
        assert_eq!((b.x0, b.x1), (0.0, 20.0));
        assert_eq!((b.y0, b.y1), (-4.0, 20.0));
    }

    #[test]
    fn ligatures_expand_to_their_letters() {
        assert_eq!(expanded("\u{FB01}"), Some("fi".into()));
        assert_eq!(expanded("\u{FB03}"), Some("ffi".into()));
        // Mixed content keeps everything around the ligature.
        assert_eq!(expanded("e\u{FB03}cient"), Some("efficient".into()));
        // Ordinary text is left alone, and says so, so the caller can skip the allocation.
        assert_eq!(expanded("efficient"), None);
        assert_eq!(expanded(""), None);
    }

    #[test]
    fn colours_clamp_out_of_gamut_components() {
        let c = to_rgba([1.5, -0.2, 0.5], 1.0);
        assert_eq!((c.r(), c.g(), c.b(), c.a()), (255, 0, 128, 255));
    }
}
