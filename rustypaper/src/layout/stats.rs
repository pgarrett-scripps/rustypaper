//! Per-document typographic statistics.
//!
//! Almost every later decision is relative rather than absolute: a heading is text *larger than
//! body*, a paragraph break is a gap *wider than leading*. Measuring those two quantities once
//! per document, rather than guessing constants, is what lets the same code handle a 9pt
//! two-column IEEE template and a 12pt single-column preprint.

use crate::text::lines::Line;

/// Font sizes within this many points of each other are the same size.
const SIZE_TOLERANCE: f32 = 0.25;

/// Typographic baseline for a document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Font size of running text, by total glyph count.
    pub body_size: f32,
    /// Typical baseline-to-baseline distance within a paragraph.
    pub leading: f32,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            body_size: 10.0,
            leading: 12.0,
        }
    }
}

impl Stats {
    /// Measures a document from its lines.
    ///
    /// Body size is the most *common* size weighted by how much text is set in it, not the mean:
    /// a paper's headings, captions and footnotes would drag a mean well off the body value,
    /// whereas body text always dominates by volume.
    pub fn measure(pages: &[Vec<Line>]) -> Stats {
        let body_size = crate::util::dominant(
            pages.iter().flatten(),
            SIZE_TOLERANCE,
            |line| line.size,
            |line| line.glyphs.len() as f32,
        )
        .filter(|s| *s > 0.0)
        .unwrap_or(Stats::default().body_size);

        Stats {
            body_size,
            leading: measure_leading(pages, body_size),
        }
    }
}

/// Median baseline step between vertically adjacent body lines.
///
/// Only steps between two body-size lines count, and only those within a plausible range: a step
/// across a column break or a section boundary says nothing about leading, and including them
/// would inflate it enough to stop paragraph detection working.
fn measure_leading(pages: &[Vec<Line>], body_size: f32) -> f32 {
    let mut steps: Vec<f32> = Vec::new();

    for lines in pages {
        let mut body: Vec<&Line> = lines
            .iter()
            .filter(|l| (l.size - body_size).abs() < 0.25)
            .collect();
        body.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));

        for pair in body.windows(2) {
            let step = pair[1].baseline - pair[0].baseline;
            // Lines side by side in different columns have a step near zero; a step of several
            // lines' height spans a break rather than measuring leading.
            if step > body_size * 0.5 && step < body_size * 2.5 {
                steps.push(step);
            }
        }
    }

    crate::util::median(&mut steps).unwrap_or(body_size * 1.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Rect;
    use crate::text::lines::{Line, Placed, Script};

    fn line(baseline: f32, size: f32, glyphs: usize, x0: f32, x1: f32) -> Line {
        Line {
            bbox: Rect::from_corners(x0, baseline - size, x1, baseline),
            baseline,
            size,
            bold: false,
            italic: false,
            glyphs: (0..glyphs)
                .map(|index| Placed {
                    index,
                    script: Script::Normal,
                    break_before: false,
                })
                .collect(),
            words: Vec::new(),
        }
    }

    #[test]
    fn body_size_is_the_most_set_size_not_the_largest() {
        let page = vec![
            line(100.0, 24.0, 30, 72.0, 400.0), // title
            line(140.0, 10.0, 90, 72.0, 400.0),
            line(152.0, 10.0, 88, 72.0, 400.0),
            line(164.0, 10.0, 91, 72.0, 400.0),
            line(200.0, 8.0, 40, 72.0, 400.0), // caption
        ];
        let stats = Stats::measure(&[page]);
        assert_eq!(stats.body_size, 10.0);
        assert_eq!(stats.leading, 12.0);
    }

    #[test]
    fn leading_ignores_column_and_section_breaks() {
        let page = vec![
            line(100.0, 10.0, 50, 72.0, 280.0),
            line(112.0, 10.0, 50, 72.0, 280.0),
            // Top of the second column: same page, huge apparent backward step, and a forward
            // step far larger than leading. Neither should count.
            line(400.0, 10.0, 50, 320.0, 530.0),
            line(412.0, 10.0, 50, 320.0, 530.0),
        ];
        let stats = Stats::measure(&[page]);
        assert_eq!(stats.leading, 12.0);
    }

    #[test]
    fn empty_input_falls_back_to_defaults() {
        let stats = Stats::measure(&[]);
        assert_eq!(stats, Stats::default());
    }
}
