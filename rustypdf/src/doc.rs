//! The document model, and the assembly of ordered lines into it.
//!
//! This is the contract the emitters render. Markdown, Typst and plain text are all projections
//! of a [`Document`]; nothing downstream of here looks at geometry again.

use serde::{Deserialize, Serialize};

use crate::ir::Rect;
use crate::layout::stats::Stats;
use crate::text::lines::Line;

/// A baseline step larger than leading by this factor starts a new block.
const BLOCK_GAP_RATIO: f32 = 1.45;

/// A font size change of more than this many points starts a new block.
const BLOCK_SIZE_DELTA: f32 = 0.4;

/// A heading is set larger than body text by at least this factor, unless it is bold or numbered.
const HEADING_SIZE_RATIO: f32 = 1.06;

/// A heading occupies at most this fraction of its column's width.
const HEADING_MAX_WIDTH_RATIO: f32 = 0.9;

/// A heading is at most this many words. Guards against a large-set opening paragraph.
const HEADING_MAX_WORDS: usize = 14;

/// Longest plausible component of a section number, in characters.
const MAX_NUMBERING_COMPONENT: usize = 3;

/// Deepest plausible section numbering, as in `A.3.2.1`.
const MAX_NUMBERING_DEPTH: usize = 4;

/// The title must be at least this much larger than body text.
const TITLE_SIZE_RATIO: f32 = 1.15;

/// Only this many opening blocks are considered as title candidates.
const TITLE_SEARCH_BLOCKS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    Title,
    /// Section heading; level 1 is a top-level section.
    Heading(u8),
    Paragraph,
    /// A figure or table caption.
    Caption,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    /// Zero-based page the block starts on.
    pub page: usize,
    pub bbox: Rect,
    /// Dominant font size, kept so emitters and later passes can reason without re-measuring.
    pub size: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Document {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

/// Assembles ordered per-page lines into a document.
pub fn assemble(pages: &[Vec<Line>], stats: Stats) -> Document {
    let mut blocks = Vec::new();

    for (index, lines) in pages.iter().enumerate() {
        let column_width = lines
            .iter()
            .map(|l| l.bbox.width())
            .fold(0.0f32, f32::max)
            .max(1.0);

        for group in group_lines(lines, stats) {
            if let Some(block) = build_block(&group, index, stats, column_width) {
                blocks.push(block);
            }
        }
    }

    promote_title(&mut blocks, stats);
    assign_heading_levels(&mut blocks, stats);

    let title = blocks
        .iter()
        .find(|b| b.kind == BlockKind::Title)
        .map(|b| b.text.clone());

    Document { title, blocks }
}

/// Splits a page's ordered lines into block-sized groups.
fn group_lines<'a>(lines: &'a [Line], stats: Stats) -> Vec<Vec<&'a Line>> {
    let mut groups: Vec<Vec<&Line>> = Vec::new();
    let mut current: Vec<&Line> = Vec::new();

    for line in lines {
        if let Some(previous) = current.last() {
            if starts_new_block(previous, line, stats) {
                groups.push(std::mem::take(&mut current));
            }
        }
        current.push(line);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn starts_new_block(previous: &Line, line: &Line, stats: Stats) -> bool {
    if (previous.size - line.size).abs() > BLOCK_SIZE_DELTA {
        return true;
    }

    let step = line.baseline - previous.baseline;
    // A backwards or sideways step means reading order moved to a new column or region.
    if step <= 0.0 {
        return true;
    }
    step > stats.leading * BLOCK_GAP_RATIO
}

fn build_block(group: &[&Line], page: usize, stats: Stats, column_width: f32) -> Option<Block> {
    let text = join_lines(group);
    if text.trim().is_empty() {
        return None;
    }

    let bbox = group
        .iter()
        .map(|l| l.bbox)
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Rect::from_corners(0.0, 0.0, 0.0, 0.0));
    let size = group[0].size;

    let kind = if is_caption(&text) {
        BlockKind::Caption
    } else if is_heading(group, &text, stats, column_width) {
        // Level is assigned once the whole document's headings are known.
        BlockKind::Heading(1)
    } else {
        BlockKind::Paragraph
    };

    Some(Block {
        kind,
        text,
        page,
        bbox,
        size,
    })
}

/// Joins a block's lines into running text.
///
/// Line-ending hyphens are left alone. Deciding whether `state-` + `of-the-art` should rejoin
/// needs the document's own vocabulary, which is M2's de-hyphenation pass; guessing here would
/// silently corrupt compound words.
fn join_lines(group: &[&Line]) -> String {
    let mut out = String::new();
    for (i, line) in group.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&line.text());
    }
    out.trim().to_owned()
}

fn is_caption(text: &str) -> bool {
    let head: String = text.chars().take(24).collect::<String>().to_lowercase();
    ["figure", "fig.", "table", "algorithm", "listing"]
        .iter()
        .any(|prefix| head.starts_with(prefix))
        && head.chars().any(|c| c.is_ascii_digit())
}

fn is_heading(group: &[&Line], text: &str, stats: Stats, column_width: f32) -> bool {
    if group.len() > 2 {
        return false;
    }
    let words = text.split_whitespace().count();
    if words == 0 || words > HEADING_MAX_WORDS {
        return false;
    }
    // Running text ends in a full stop; headings almost never do.
    if text.ends_with('.') && !numbered_heading(text) {
        return false;
    }

    let width_ratio = group.iter().map(|l| l.bbox.width()).fold(0.0f32, f32::max) / column_width;
    if width_ratio > HEADING_MAX_WIDTH_RATIO {
        return false;
    }

    // Numbering is decisive; otherwise the line has to stand out as larger or bold, because at
    // body size and body weight a short line is far more likely to be the last line of a
    // paragraph than a heading.
    numbered_heading(text)
        || group[0].size >= stats.body_size * HEADING_SIZE_RATIO
        || group.iter().all(|l| l.bold)
}

/// `1 Introduction`, `3.2 Ablation studies`, `A.1 Proofs`.
///
/// Each dot-separated component is short by construction — section numbers do not run to five
/// digits — which is what keeps `3.14159 is pi` from parsing as a numbered heading.
fn numbered_heading(text: &str) -> bool {
    let Some(head) = text.split_whitespace().next() else {
        return false;
    };
    // A trailing dot is part of the label (`3.` Introduction), not another component.
    let label = head.trim_end_matches('.');
    if label.is_empty() {
        return false;
    }

    let components: Vec<&str> = label.split('.').collect();
    if components.is_empty() || components.len() > MAX_NUMBERING_DEPTH {
        return false;
    }
    let plausible = components.iter().all(|part| {
        !part.is_empty()
            && part.len() <= MAX_NUMBERING_COMPONENT
            && (part.chars().all(|c| c.is_ascii_digit())
                || part.chars().all(|c| c.is_ascii_uppercase()))
    });
    if !plausible {
        return false;
    }

    // A bare label with nothing after it is a list marker or a stray number, not a heading.
    text[head.len()..]
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic())
}

/// The largest text near the top of the first page is the title.
///
/// Restricted to the opening blocks of page 1 so that a paper whose first page begins with a
/// section heading, or one with no title at all, cannot have a mid-page heading promoted.
fn promote_title(blocks: &mut [Block], stats: Stats) {
    let Some((index, _)) = blocks
        .iter()
        .enumerate()
        .take(TITLE_SEARCH_BLOCKS)
        .filter(|(_, b)| b.page == 0 && b.size >= stats.body_size * TITLE_SIZE_RATIO)
        .max_by(|(_, a), (_, b)| a.size.total_cmp(&b.size))
    else {
        return;
    };
    blocks[index].kind = BlockKind::Title;
}

/// Ranks heading sizes so that larger headings nest outside smaller ones.
///
/// Section numbering is preferred where present — `3.2` is unambiguously a level below `3` —
/// and size rank is the fallback for unnumbered templates.
fn assign_heading_levels(blocks: &mut [Block], stats: Stats) {
    let mut sizes: Vec<f32> = blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Heading(_)))
        .map(|b| b.size)
        .collect();
    sizes.sort_by(|a, b| b.total_cmp(a));
    sizes.dedup_by(|a, b| (*a - *b).abs() < 0.25);

    for block in blocks.iter_mut() {
        if !matches!(block.kind, BlockKind::Heading(_)) {
            continue;
        }
        let level = match numbering_depth(&block.text) {
            Some(depth) => depth,
            None => {
                let rank = sizes
                    .iter()
                    .position(|s| (s - block.size).abs() < 0.25)
                    .unwrap_or(0);
                (rank + 1).min(6) as u8
            }
        };
        block.kind = BlockKind::Heading(level.max(1));
        let _ = stats;
    }
}

/// Depth implied by a leading section number: `3` is 1, `3.2` is 2, `3.2.1` is 3.
fn numbering_depth(text: &str) -> Option<u8> {
    if !numbered_heading(text) {
        return None;
    }
    let head = text.split_whitespace().next()?;
    let depth = head
        .trim_end_matches('.')
        .split('.')
        .filter(|part| !part.is_empty())
        .count();
    (depth > 0).then(|| depth.min(6) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::lines::Word;

    fn line(text: &str, baseline: f32, size: f32, x0: f32, x1: f32) -> Line {
        let bbox = Rect::from_corners(x0, baseline - size, x1, baseline);
        Line {
            bbox,
            baseline,
            size,
            bold: false,
            italic: false,
            glyphs: Vec::new(),
            words: vec![Word {
                bbox,
                text: text.to_owned(),
                start: 0,
                end: 1,
            }],
        }
    }

    fn stats() -> Stats {
        Stats {
            body_size: 10.0,
            leading: 12.0,
        }
    }

    #[test]
    fn consecutive_body_lines_form_one_paragraph() {
        let page = vec![
            line("The first line of a paragraph", 100.0, 10.0, 72.0, 540.0),
            line("and its continuation.", 112.0, 10.0, 72.0, 540.0),
        ];
        let doc = assemble(&[page], stats());
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(
            doc.blocks[0].text,
            "The first line of a paragraph and its continuation."
        );
    }

    #[test]
    fn a_wide_vertical_gap_starts_a_new_block() {
        let page = vec![
            line("First paragraph.", 100.0, 10.0, 72.0, 540.0),
            line("Second paragraph.", 140.0, 10.0, 72.0, 540.0),
        ];
        let doc = assemble(&[page], stats());
        assert_eq!(doc.blocks.len(), 2);
    }

    #[test]
    fn numbered_headings_get_levels_from_their_numbering() {
        let page = vec![
            line("1 Introduction", 100.0, 10.0, 72.0, 200.0),
            line("Body text here that runs on.", 130.0, 10.0, 72.0, 540.0),
            line("3.2 Ablation studies", 160.0, 10.0, 72.0, 220.0),
            line("More body text.", 190.0, 10.0, 72.0, 540.0),
        ];
        let doc = assemble(&[page], stats());
        let kinds: Vec<&BlockKind> = doc.blocks.iter().map(|b| &b.kind).collect();
        assert_eq!(kinds[0], &BlockKind::Heading(1));
        assert_eq!(kinds[1], &BlockKind::Paragraph);
        assert_eq!(kinds[2], &BlockKind::Heading(2));
    }

    #[test]
    fn a_larger_short_line_is_a_heading_even_without_numbering() {
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line(
                "Body text that continues for a while.",
                80.0,
                10.0,
                72.0,
                540.0,
            ),
            line("Related Work", 130.0, 12.0, 72.0, 200.0),
            line("More body text follows on.", 160.0, 10.0, 72.0, 540.0),
        ];
        let doc = assemble(&[page], stats());
        assert_eq!(doc.blocks[0].kind, BlockKind::Title);
        assert!(
            matches!(doc.blocks[2].kind, BlockKind::Heading(_)),
            "expected a heading, got {:?}",
            doc.blocks[2].kind
        );
    }

    #[test]
    fn a_full_width_sentence_is_never_a_heading() {
        // Set slightly large like a heading, but it fills the column and ends in a stop.
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line(
                "This sentence is set slightly large but runs the full width of the column.",
                100.0,
                12.0,
                72.0,
                540.0,
            ),
        ];
        let doc = assemble(&[page], stats());
        assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn a_document_with_no_title_does_not_promote_a_mid_page_heading() {
        let mut page = vec![
            line("Body text opening the page.", 100.0, 10.0, 72.0, 540.0),
            line("More body text here.", 130.0, 10.0, 72.0, 540.0),
        ];
        for i in 0..6 {
            page.push(line(
                "Filler body text on the page.",
                170.0 + i as f32 * 30.0,
                10.0,
                72.0,
                540.0,
            ));
        }
        page.push(line("5 Conclusion", 400.0, 14.0, 72.0, 200.0));
        let doc = assemble(&[page], stats());
        assert!(doc.title.is_none(), "promoted {:?} to a title", doc.title);
    }

    #[test]
    fn captions_are_recognised() {
        let page = vec![line(
            "Figure 3. Training error on CIFAR-10.",
            100.0,
            9.0,
            72.0,
            400.0,
        )];
        let doc = assemble(&[page], stats());
        assert_eq!(doc.blocks[0].kind, BlockKind::Caption);
    }

    #[test]
    fn the_largest_block_on_the_first_page_is_the_title() {
        let page = vec![
            line("Deep Residual Learning", 60.0, 20.0, 72.0, 400.0),
            line("1 Introduction", 120.0, 12.0, 72.0, 200.0),
            line(
                "Body text goes here and continues.",
                150.0,
                10.0,
                72.0,
                540.0,
            ),
        ];
        let doc = assemble(&[page], stats());
        assert_eq!(doc.title.as_deref(), Some("Deep Residual Learning"));
        assert_eq!(doc.blocks[0].kind, BlockKind::Title);
    }

    #[test]
    fn a_section_number_alone_is_not_a_heading_pattern() {
        assert!(!numbered_heading("3.14159 is pi"));
        assert!(numbered_heading("3.2 Ablation studies"));
        assert!(numbered_heading("A.1 Proofs"));
        assert!(!numbered_heading("However the result holds"));
    }

    #[test]
    fn numbering_depth_counts_components() {
        assert_eq!(numbering_depth("1 Introduction"), Some(1));
        assert_eq!(numbering_depth("3.2 Ablation"), Some(2));
        assert_eq!(numbering_depth("3.2.1 Detail"), Some(3));
        assert_eq!(numbering_depth("Introduction"), None);
    }
}
