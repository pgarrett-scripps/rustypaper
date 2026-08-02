//! Finding figures on a page.
//!
//! A figure is a region of the page given over to graphics rather than text. There is no marker
//! for one in a PDF — `\includegraphics` leaves an image, and a plot drawn by pgfplots or
//! matplotlib leaves several hundred vector paths — so the region has to be recovered from where
//! the ink is.
//!
//! Rules are excluded deliberately. A `booktabs` table is nothing but horizontal rules, and a
//! framed text box is a single rectangle; neither is a figure, and both would otherwise be
//! swept up by a naive "graphics on the page" test.

use crate::ir::{PageRaw, PathKind, Rect};

/// Two pieces of artwork closer than this belong to the same figure.
///
/// Generous on purpose. A plot's axes are long thin lines, so they are classified as rules and
/// excluded here — which means the remaining marks, curves and legend have to bridge the gaps
/// the axes would otherwise have spanned. Only graphical boxes are merged, never text, so a
/// wide radius costs little: unrelated artwork elsewhere on a page is normally 100pt away.
const MERGE_GAP: f32 = 24.0;

/// A figure occupies at least this fraction of the page.
const MIN_AREA_FRACTION: f32 = 0.015;

/// And at most this fraction. A "figure" covering the whole page is a merge that has run away,
/// and acting on it removes every block on the page. Belt and braces alongside clipping object
/// bounds to the page in the backend.
const MAX_AREA_FRACTION: f32 = 0.85;

/// A figure is at least this many points on its shorter side.
const MIN_EXTENT: f32 = 36.0;

/// A region built only from vector paths needs at least this many of them. One or two shapes
/// are a frame, a bullet or a logo; a plot has dozens.
const MIN_PATHS_WITHOUT_IMAGE: usize = 8;

/// A graphical region of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub bbox: Rect,
    /// How many raster images fall inside it.
    pub images: usize,
    /// How many non-rule vector paths fall inside it.
    pub paths: usize,
}

/// Finds the figure regions of a page.
pub fn regions(page: &PageRaw) -> Vec<Region> {
    let mut boxes: Vec<(Rect, bool)> = Vec::new();

    for image in &page.images {
        if !image.bbox.is_empty() {
            boxes.push((image.bbox, true));
        }
    }
    for path in &page.paths {
        let graphical = matches!(path.kind, PathKind::Box | PathKind::Other);
        if graphical && !path.bbox.is_empty() {
            boxes.push((path.bbox, false));
        }
    }

    if boxes.is_empty() {
        return Vec::new();
    }

    let clusters = merge(&boxes);
    let page_area = (page.width * page.height).max(1.0);

    clusters
        .into_iter()
        .filter(|region| {
            let area = region.bbox.width() * region.bbox.height();
            area / page_area >= MIN_AREA_FRACTION
                && area / page_area <= MAX_AREA_FRACTION
                && region.bbox.width().min(region.bbox.height()) >= MIN_EXTENT
                && (region.images > 0 || region.paths >= MIN_PATHS_WITHOUT_IMAGE)
        })
        .collect()
}

/// Repeatedly unions boxes that are within [`MERGE_GAP`] of each other.
fn merge(boxes: &[(Rect, bool)]) -> Vec<Region> {
    let mut regions: Vec<Region> = boxes
        .iter()
        .map(|&(bbox, is_image)| Region {
            bbox,
            images: usize::from(is_image),
            paths: usize::from(!is_image),
        })
        .collect();

    // Quadratic in the number of regions, but each pass shrinks the set fast and a page rarely
    // has more than a few thousand paths before the first pass collapses them.
    loop {
        let mut merged = false;
        let mut out: Vec<Region> = Vec::with_capacity(regions.len());

        'outer: for region in regions.drain(..) {
            for existing in &mut out {
                if near(&existing.bbox, &region.bbox) {
                    existing.bbox = existing.bbox.union(&region.bbox);
                    existing.images += region.images;
                    existing.paths += region.paths;
                    merged = true;
                    continue 'outer;
                }
            }
            out.push(region);
        }

        regions = out;
        if !merged {
            return regions;
        }
    }
}

fn near(a: &Rect, b: &Rect) -> bool {
    a.x_overlap(b) >= -MERGE_GAP && a.y_overlap(b) >= -MERGE_GAP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ImageItem, PathItem, Rgba};

    fn page(paths: Vec<PathItem>, images: Vec<ImageItem>) -> PageRaw {
        PageRaw {
            index: 0,
            width: 612.0,
            height: 792.0,
            rotation: 0,
            glyphs: Vec::new(),
            paths,
            images,
            expansions: Vec::new(),
        }
    }

    fn path(kind: PathKind, x0: f32, y0: f32, x1: f32, y1: f32) -> PathItem {
        PathItem {
            bbox: Rect::from_corners(x0, y0, x1, y1),
            kind,
            thickness: 0.5,
            color: Rgba::BLACK,
            filled: true,
            stroked: false,
        }
    }

    fn image(x0: f32, y0: f32, x1: f32, y1: f32) -> ImageItem {
        ImageItem {
            bbox: Rect::from_corners(x0, y0, x1, y1),
            object_index: 0,
            pixel_width: 800,
            pixel_height: 600,
        }
    }

    #[test]
    fn a_single_image_is_a_figure() {
        let found = regions(&page(Vec::new(), vec![image(72.0, 100.0, 400.0, 380.0)]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].images, 1);
    }

    #[test]
    fn scattered_plot_paths_merge_into_one_figure() {
        // A plot: many small marks spread across a region, none big enough alone.
        let mut paths = Vec::new();
        for i in 0..40 {
            let x = 80.0 + (i % 8) as f32 * 24.0;
            let y = 120.0 + (i / 8) as f32 * 24.0;
            paths.push(path(PathKind::Other, x, y, x + 6.0, y + 6.0));
        }
        let found = regions(&page(paths, Vec::new()));
        assert_eq!(found.len(), 1, "plot marks should form one region: {found:?}");
        assert!(found[0].paths >= 40);
        assert!(found[0].bbox.width() > 150.0);
    }

    /// A booktabs table is horizontal rules and nothing else. It is not a figure.
    #[test]
    fn table_rules_are_not_a_figure() {
        let paths = vec![
            path(PathKind::HorizontalRule, 72.0, 100.0, 540.0, 100.6),
            path(PathKind::HorizontalRule, 72.0, 140.0, 540.0, 140.6),
            path(PathKind::HorizontalRule, 72.0, 300.0, 540.0, 300.6),
        ];
        assert!(regions(&page(paths, Vec::new())).is_empty());
    }

    /// A single rectangle is a frame around text, not a plot.
    #[test]
    fn one_lone_rectangle_is_not_a_figure() {
        let paths = vec![path(PathKind::Box, 72.0, 100.0, 540.0, 400.0)];
        assert!(regions(&page(paths, Vec::new())).is_empty());
    }

    #[test]
    fn tiny_graphics_are_ignored() {
        // A bullet glyph drawn as a path, and a small logo.
        let paths = vec![path(PathKind::Other, 72.0, 100.0, 76.0, 104.0)];
        let images = vec![image(500.0, 40.0, 520.0, 60.0)];
        assert!(regions(&page(paths, images)).is_empty());
    }

    #[test]
    fn two_separated_figures_stay_separate() {
        let images = vec![
            image(72.0, 100.0, 280.0, 300.0),
            image(330.0, 100.0, 540.0, 300.0),
        ];
        let found = regions(&page(Vec::new(), images));
        assert_eq!(found.len(), 2, "figures 200pt apart must not merge");
    }

    #[test]
    fn a_region_covering_the_whole_page_is_rejected() {
        let images = vec![image(0.0, 0.0, 612.0, 792.0)];
        assert!(
            regions(&page(Vec::new(), images)).is_empty(),
            "a page-sized region is a runaway merge, not a figure"
        );
    }

    #[test]
    fn an_empty_page_has_no_figures() {
        assert!(regions(&page(Vec::new(), Vec::new())).is_empty());
    }
}
