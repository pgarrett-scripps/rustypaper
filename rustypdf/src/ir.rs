//! Page primitives: what we get out of a PDF before any interpretation happens.
//!
//! Coordinates are PDF points (1/72 inch) in a **top-left origin, y-down** space, already
//! corrected for page rotation. PDF's native space is bottom-left/y-up; backends convert.
//! Everything downstream — line building, XY-cut, math baselines — assumes y-down, so the
//! conversion happens exactly once, at the backend boundary.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A point in page space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle. Invariant: `x0 <= x1` and `y0 <= y1`, with `y0` the *top* edge.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// Builds a rect from two arbitrary corners, normalising the ordering.
    pub fn from_corners(ax: f32, ay: f32, bx: f32, by: f32) -> Self {
        Self {
            x0: ax.min(bx),
            y0: ay.min(by),
            x1: ax.max(bx),
            y1: ay.max(by),
        }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn center_x(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }

    pub fn center_y(&self) -> f32 {
        (self.y0 + self.y1) * 0.5
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }

    /// Smallest rect containing both.
    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x0 < other.x1 && other.x0 < self.x1 && self.y0 < other.y1 && other.y0 < self.y1
    }

    pub fn contains(&self, other: &Rect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }

    /// Length of the overlap of the two rects' x-extents; negative when they are disjoint.
    pub fn x_overlap(&self, other: &Rect) -> f32 {
        self.x1.min(other.x1) - self.x0.max(other.x0)
    }

    /// Length of the overlap of the two rects' y-extents; negative when they are disjoint.
    pub fn y_overlap(&self, other: &Rect) -> f32 {
        self.y1.min(other.y1) - self.y0.max(other.y0)
    }
}

/// Index into [`FontTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FontId(pub u32);

/// Document-wide interning of font names.
///
/// Font names repeat on every glyph, so they are interned once per document and glyphs carry a
/// 4-byte id. This is load-bearing for the memory budget: a dense page has several thousand
/// glyphs and a paper has hundreds of pages.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FontTable {
    names: Vec<String>,
    #[serde(skip)]
    lookup: HashMap<String, FontId>,
}

impl FontTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, name: &str) -> FontId {
        if let Some(id) = self.lookup.get(name) {
            return *id;
        }
        let id = FontId(self.names.len() as u32);
        self.names.push(name.to_owned());
        self.lookup.insert(name.to_owned(), id);
        id
    }

    pub fn name(&self, id: FontId) -> &str {
        self.names.get(id.0 as usize).map_or("", String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (FontId, &str)> {
        self.names
            .iter()
            .enumerate()
            .map(|(i, n)| (FontId(i as u32), n.as_str()))
    }

    /// Rebuilds the lookup index after deserialisation.
    pub fn reindex(&mut self) {
        self.lookup = self
            .names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), FontId(i as u32)))
            .collect();
    }
}

/// Font descriptor flags, as reported per-character by the backend.
///
/// Packed rather than a struct of `bool`s because it rides along on every glyph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontFlags(pub u16);

impl FontFlags {
    pub const ITALIC: u16 = 1 << 0;
    pub const SERIF: u16 = 1 << 1;
    pub const SANS_SERIF: u16 = 1 << 2;
    /// Non-standard encoding — the usual marker for TeX math fonts and dingbats.
    pub const SYMBOLIC: u16 = 1 << 3;
    pub const FIXED_PITCH: u16 = 1 << 4;
    pub const CURSIVE: u16 = 1 << 5;
    pub const SMALL_CAPS: u16 = 1 << 6;
    pub const ALL_CAPS: u16 = 1 << 7;
    pub const BOLD: u16 = 1 << 8;

    pub fn set(&mut self, flag: u16, on: bool) {
        if on {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }

    pub fn has(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    pub fn is_italic(self) -> bool {
        self.has(Self::ITALIC)
    }

    pub fn is_bold(self) -> bool {
        self.has(Self::BOLD)
    }

    pub fn is_symbolic(self) -> bool {
        self.has(Self::SYMBOLIC)
    }
}

/// Packed sRGB colour with alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rgba(pub u32);

impl Rgba {
    pub const BLACK: Rgba = Rgba(0xFF00_0000);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    pub const fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }
}

/// The text a glyph stands for.
///
/// Almost always a single `char`. The exception is the case that motivates the whole Unicode
/// repair pass: a TeX ligature glyph that must expand to several characters (`ﬃ` -> `ffi`).
/// Those are rare, so the expansion lives in a per-page side table and the common case stays
/// allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlyphText {
    Char(char),
    /// Index into [`PageRaw::expansions`].
    Expanded(u32),
}

/// One rendered character.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Glyph {
    pub text: GlyphText,
    /// Tight glyph bounds — the inked extent, used for baseline and script detection.
    pub bbox: Rect,
    /// Baseline origin of the glyph.
    pub origin: Point,
    pub font: FontId,
    /// Effective font size in points, including any scaling from the text matrix.
    pub size: f32,
    /// Rotation in radians; 0 for the overwhelming majority of scientific text.
    pub angle: f32,
    pub flags: FontFlags,
    pub color: Rgba,
    /// The backend synthesised this character (typically a space) rather than reading it from a
    /// content stream. We do our own gap-based word segmentation, so this is a cross-check, not
    /// a source of truth.
    pub generated: bool,
}

/// What a vector path looks like geometrically. Classified at extraction time because the
/// downstream consumers (table rules, fraction bars, footnote separators) only care about the
/// shape, never the path data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathKind {
    /// Wider than tall and thin in absolute terms: a rule. Fraction bars, `\hline`, `\toprule`.
    HorizontalRule,
    /// Taller than wide and thin: a column separator or `|` in a tabular.
    VerticalRule,
    /// A filled or stroked box with real area — cell shading, framed figures.
    Box,
    /// Anything else: curves, diagrams, plot content.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PathItem {
    pub bbox: Rect,
    pub kind: PathKind,
    /// Thickness of the rule in points, for `HorizontalRule` / `VerticalRule`.
    pub thickness: f32,
    pub color: Rgba,
    pub filled: bool,
    pub stroked: bool,
}

/// A raster image placed on the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageItem {
    pub bbox: Rect,
    /// Index of the object within the page, so the backend can be asked for pixels later
    /// without keeping them in memory during layout.
    pub object_index: usize,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

/// Everything extracted from one page, before interpretation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageRaw {
    /// Zero-based page index in the document.
    pub index: usize,
    /// Page box in points, after rotation is applied.
    pub width: f32,
    pub height: f32,
    /// Page rotation that was applied, in degrees (0, 90, 180 or 270).
    pub rotation: i32,
    pub glyphs: Vec<Glyph>,
    pub paths: Vec<PathItem>,
    pub images: Vec<ImageItem>,
    /// Multi-character glyph expansions referenced by [`GlyphText::Expanded`].
    pub expansions: Vec<String>,
}

impl PageRaw {
    /// The page rectangle.
    pub fn bounds(&self) -> Rect {
        Rect {
            x0: 0.0,
            y0: 0.0,
            x1: self.width,
            y1: self.height,
        }
    }

    /// Resolves a glyph to its text.
    pub fn glyph_str<'a>(&'a self, glyph: &Glyph, buf: &'a mut [u8; 4]) -> &'a str {
        match glyph.text {
            GlyphText::Char(c) => c.encode_utf8(buf),
            GlyphText::Expanded(i) => self.expansions.get(i as usize).map_or("", String::as_str),
        }
    }

    /// Records a multi-character expansion and returns the [`GlyphText`] referring to it.
    pub fn add_expansion(&mut self, text: &str) -> GlyphText {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => GlyphText::Char(c),
            _ => {
                self.expansions.push(text.to_owned());
                GlyphText::Expanded(self.expansions.len() as u32 - 1)
            }
        }
    }

    /// True when the page carries no extractable text but does carry imagery — the signature of
    /// a scanned page, which this pipeline deliberately does not handle.
    pub fn looks_scanned(&self) -> bool {
        if !self.glyphs.is_empty() {
            return false;
        }
        let page_area = self.width * self.height;
        if page_area <= 0.0 {
            return false;
        }
        self.images.iter().any(|img| {
            let area = img.bbox.width() * img.bbox.height();
            area / page_area > 0.5
        })
    }
}

/// A whole document's primitives.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocRaw {
    pub fonts: FontTable,
    pub pages: Vec<PageRaw>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_normalises_corners() {
        let r = Rect::from_corners(10.0, 20.0, 4.0, 2.0);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (4.0, 2.0, 10.0, 20.0));
        assert_eq!(r.width(), 6.0);
        assert_eq!(r.height(), 18.0);
    }

    #[test]
    fn rect_overlap_is_negative_when_disjoint() {
        let a = Rect::from_corners(0.0, 0.0, 10.0, 10.0);
        let b = Rect::from_corners(20.0, 0.0, 30.0, 10.0);
        assert!(a.x_overlap(&b) < 0.0);
        assert!(a.y_overlap(&b) > 0.0);
        assert!(!a.intersects(&b));
    }

    #[test]
    fn font_table_interns_once() {
        let mut fonts = FontTable::new();
        let a = fonts.intern("CMR10");
        let b = fonts.intern("CMMI10");
        let c = fonts.intern("CMR10");
        assert_eq!(a, c);
        assert_ne!(a, b);
        assert_eq!(fonts.len(), 2);
        assert_eq!(fonts.name(a), "CMR10");
    }

    #[test]
    fn font_table_reindexes_after_roundtrip() {
        let mut fonts = FontTable::new();
        fonts.intern("CMR10");
        let json = serde_json::to_string(&fonts).unwrap();
        let mut back: FontTable = serde_json::from_str(&json).unwrap();
        back.reindex();
        assert_eq!(back.intern("CMR10"), FontId(0));
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn single_char_expansion_stays_inline() {
        let mut page = PageRaw::default();
        assert_eq!(page.add_expansion("f"), GlyphText::Char('f'));
        assert!(page.expansions.is_empty());

        let g = page.add_expansion("ffi");
        assert_eq!(g, GlyphText::Expanded(0));
        assert_eq!(page.expansions.len(), 1);
    }

    #[test]
    fn glyph_str_resolves_both_forms() {
        let mut page = PageRaw::default();
        let plain = page.add_expansion("x");
        let lig = page.add_expansion("ffi");
        let glyph = |text| Glyph {
            text,
            bbox: Rect::from_corners(0.0, 0.0, 1.0, 1.0),
            origin: Point::new(0.0, 0.0),
            font: FontId(0),
            size: 10.0,
            angle: 0.0,
            flags: FontFlags::default(),
            color: Rgba::BLACK,
            generated: false,
        };
        let mut buf = [0u8; 4];
        assert_eq!(page.glyph_str(&glyph(plain), &mut buf), "x");
        let mut buf = [0u8; 4];
        assert_eq!(page.glyph_str(&glyph(lig), &mut buf), "ffi");
    }

    #[test]
    fn scanned_detection_needs_no_text_and_a_big_image() {
        let mut page = PageRaw {
            width: 612.0,
            height: 792.0,
            ..Default::default()
        };
        assert!(!page.looks_scanned(), "empty page is not a scan");

        page.images.push(ImageItem {
            bbox: Rect::from_corners(0.0, 0.0, 612.0, 792.0),
            object_index: 0,
            pixel_width: 2550,
            pixel_height: 3300,
        });
        assert!(page.looks_scanned());

        page.glyphs.push(Glyph {
            text: GlyphText::Char('a'),
            bbox: Rect::from_corners(0.0, 0.0, 1.0, 1.0),
            origin: Point::new(0.0, 0.0),
            font: FontId(0),
            size: 10.0,
            angle: 0.0,
            flags: FontFlags::default(),
            color: Rgba::BLACK,
            generated: false,
        });
        assert!(!page.looks_scanned(), "text present means born-digital");
    }
}
