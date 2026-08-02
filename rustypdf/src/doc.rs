//! The document model, and the assembly of ordered lines into it.
//!
//! This is the contract the emitters render. Markdown, Typst and plain text are all projections
//! of a [`Document`]; nothing downstream of here looks at geometry again.

use serde::{Deserialize, Serialize};

use crate::ir::Rect;
use crate::layout::stats::Stats;
use crate::text::lines::Line;
use crate::text::vocab::Vocabulary;

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

/// A title is between this many words and [`TITLE_MAX_WORDS`].
const TITLE_MIN_WORDS: usize = 2;
const TITLE_MAX_WORDS: usize = 25;

/// Only this many opening blocks are considered as title candidates.
const TITLE_SEARCH_BLOCKS: usize = 4;

/// A footnote is set smaller than body text by at least this factor.
const FOOTNOTE_MAX_SIZE_RATIO: f32 = 0.92;

/// A footnote starts below this fraction of the page height.
const FOOTNOTE_MIN_Y_FRACTION: f32 = 0.68;

/// How many blocks either side of a list item to search for a sibling item.
const LIST_SIBLING_RANGE: usize = 2;

/// A block of at most this many words is a fragment rather than a paragraph.
const FRAGMENT_MAX_WORDS: usize = 3;

/// A fragment rejoins the block above it only within this multiple of the leading.
const FRAGMENT_MAX_GAP: f32 = 1.6;

/// Two blocks are on the same visual row when they overlap vertically by this fraction of the
/// shorter one's height.
const FRAGMENT_ROW_OVERLAP: f32 = 0.5;

/// Serialised with an internal tag so that the JSON reads `{"type": "heading", "level": 2}`
/// rather than `{"Heading": 2}`. The document model is the contract other tools consume, so it
/// is worth being pleasant in languages that are not Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockKind {
    Title,
    /// Section heading; level 1 is a top-level section.
    Heading {
        level: u8,
    },
    Paragraph,
    /// A figure or table caption.
    Caption,
    /// A caption with the graphic it describes attached.
    Figure,
    /// An item of a bulleted or numbered list.
    ListItem {
        ordered: bool,
    },
    /// Set small at the foot of a page, below the body.
    Footnote,
    /// A reconstructed table; the cells live in [`Block::table`].
    Table,
    /// A display equation; the LaTeX lives in [`Block::math`].
    Equation,
    /// One bibliography entry; the parsed fields live in [`Block::reference`].
    Reference,
}

/// A reconstructed display equation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MathData {
    pub latex: String,
    /// The equation number as printed, if there is one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub number: Option<String>,
    /// How much to trust the reconstruction; 1.0 when nothing had to be guessed.
    pub confidence: f32,
}

/// A table's contents, flattened for emission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableData {
    /// Cell text, row-major.
    pub rows: Vec<Vec<String>>,
    /// How many leading rows are header rows.
    pub header_rows: usize,
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
    /// Relative path to an extracted graphic, for [`BlockKind::Figure`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub asset: Option<String>,
    /// Cells, for [`BlockKind::Table`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub table: Option<TableData>,
    /// LaTeX, for [`BlockKind::Equation`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub math: Option<MathData>,
    /// Parsed fields, for [`BlockKind::Reference`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reference: Option<crate::refs::Reference>,
}

impl Block {
    /// A block with no attachments. Figures, tables, equations and references fill in the
    /// relevant field afterwards.
    pub fn new(kind: BlockKind, page: usize, bbox: Rect) -> Self {
        Self {
            kind,
            text: String::new(),
            page,
            bbox,
            size: 0.0,
            asset: None,
            table: None,
            math: None,
            reference: None,
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    pub fn with_size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Document {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

/// Assembles ordered per-page lines into a document.
///
/// `page_heights` is needed because "near the foot of the page" is what distinguishes a footnote
/// from any other small text.
pub fn assemble(
    pages: &[Vec<Line>],
    page_heights: &[f32],
    stats: Stats,
    vocab: &Vocabulary,
) -> Document {
    let mut blocks = Vec::new();

    for (index, lines) in pages.iter().enumerate() {
        let column_width = lines
            .iter()
            .map(|l| l.bbox.width())
            .fold(0.0f32, f32::max)
            .max(1.0);

        let page_height = page_heights.get(index).copied().unwrap_or(792.0);
        for group in group_lines(lines, stats) {
            if let Some(block) = build_block(&group, index, stats, column_width, vocab, page_height)
            {
                blocks.push(block);
            }
        }
    }

    coalesce_fragments(&mut blocks, stats);
    demote_lonely_list_items(&mut blocks);
    promote_title(&mut blocks, stats);
    assign_heading_levels(&mut blocks);

    let title = blocks
        .iter()
        .find(|b| b.kind == BlockKind::Title)
        .map(|b| b.text.clone());

    Document { title, blocks }
}

/// Splits a page's ordered lines into block-sized groups.
fn group_lines(lines: &[Line], stats: Stats) -> Vec<Vec<&Line>> {
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

#[allow(clippy::too_many_arguments)]
fn build_block(
    group: &[&Line],
    page: usize,
    stats: Stats,
    column_width: f32,
    vocab: &Vocabulary,
    page_height: f32,
) -> Option<Block> {
    let text = join_lines(group, vocab);
    if text.trim().is_empty() {
        return None;
    }

    let bbox = group
        .iter()
        .map(|l| l.bbox)
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Rect::from_corners(0.0, 0.0, 0.0, 0.0));
    let size = group[0].size;

    // Order matters here. `1. Introduction` is indistinguishable from a numbered list item by
    // its marker alone, so the heading test — which also demands brevity, a narrow measure and
    // no terminal full stop — has to run first.
    let kind = if is_caption(&text) {
        BlockKind::Caption
    } else if is_heading(group, &text, stats, column_width) {
        // Level is assigned once the whole document's headings are known.
        BlockKind::Heading { level: 1 }
    } else if let Some(ordered) = list_marker(&text) {
        BlockKind::ListItem { ordered }
    } else if is_footnote(group, size, stats, page_height) {
        BlockKind::Footnote
    } else {
        BlockKind::Paragraph
    };

    Some(Block::new(kind, page, bbox).with_text(text).with_size(size))
}

/// Whether a block opens with a list marker, and whether that marker is ordered.
///
/// Requires a following space so that `(1+x)` and `1.5` are not mistaken for markers.
fn list_marker(text: &str) -> Option<bool> {
    let mut chars = text.chars();
    let first = chars.next()?;

    if matches!(
        first,
        '•' | '◦' | '‣' | '▪' | '·' | '∗' | '*' | '–' | '—' | '-'
    ) {
        return chars
            .next()
            .is_some_and(char::is_whitespace)
            .then_some(false);
    }

    // `1.` `2)` `(3)` `a.` `iv.`
    let head = text.split_whitespace().next()?;
    let rest = text[head.len()..].trim_start();
    if rest.is_empty() || head.len() > 6 {
        return None;
    }
    let core = head
        .trim_start_matches('(')
        .trim_end_matches([')', '.', ':']);
    if core.is_empty() || core.len() == head.len() {
        return None;
    }
    let ordered = core.chars().all(|c| c.is_ascii_digit())
        || core.chars().all(|c| matches!(c, 'i' | 'v' | 'x' | 'l'))
        || (core.len() == 1 && core.chars().all(|c| c.is_ascii_alphabetic()));
    ordered.then_some(true)
}

/// Small text sitting at the foot of the page.
///
/// Both conditions matter. Size alone catches captions and affiliations; position alone catches
/// the last paragraph of every page.
fn is_footnote(group: &[&Line], size: f32, stats: Stats, page_height: f32) -> bool {
    if page_height <= 0.0 || size >= stats.body_size * FOOTNOTE_MAX_SIZE_RATIO {
        return false;
    }
    let top = group.iter().map(|l| l.bbox.y0).fold(f32::MAX, f32::min);
    top / page_height >= FOOTNOTE_MIN_Y_FRACTION
}

/// Joins a block's lines into running text, rejoining words split across line breaks.
///
/// How visible the break is depends on the backend. pdfium deletes the hyphen from the text page
/// — Chrome does this so copy-paste rejoins hyphenated words — leaving nothing to key off but
/// the words themselves, which is what [`Vocabulary::rejoin`] is for. rustium reports the page
/// as written, so the hyphen is still there and says outright that the word continues.
///
/// Both are handled here rather than normalised away in a backend, because the hyphen is the
/// better evidence and throwing it away to imitate pdfium would be losing information on
/// purpose. A geometric guard comes first either way: hyphenation only happens where a line was
/// broken to fit, so a line stopping short of the block's right edge ended for some other reason
/// and its last word is whole.
fn join_lines(group: &[&Line], vocab: &Vocabulary) -> String {
    let right_edge = group.iter().map(|l| l.bbox.x1).fold(0.0f32, f32::max);

    let mut out = String::new();
    for (i, line) in group.iter().enumerate() {
        let text = line.text();
        if i == 0 {
            out.push_str(&text);
            continue;
        }

        let justified = line_reaches(group[i - 1], right_edge, group[i - 1].size);
        match justified
            .then(|| split_boundary(&out, &text))
            .flatten()
            .and_then(|(head, tail)| rejoin_across_break(vocab, head, tail))
        {
            Some(merged) => {
                out.truncate(out.len() - head_len(&out));
                out.push_str(&merged);
                out.push_str(&text[first_word_len(&text)..]);
            }
            None => {
                out.push(' ');
                out.push_str(&text);
            }
        }
    }
    out.trim().to_owned()
}

/// Decides whether the word ending one line and the word starting the next are one word.
///
/// With an explicit hyphen the question is only *which* word: `learn-` + `ing` is `learning`,
/// but `state-` + `of-the-art` is a compound that keeps its hyphen. The vocabulary settles it —
/// if the document uses the joined form elsewhere the hyphen was inserted by the typesetter, and
/// if it does not, the hyphen was the author's and stays.
fn rejoin_across_break(vocab: &Vocabulary, head: &str, tail: &str) -> Option<String> {
    let Some(stem) = head.strip_suffix(['-', '\u{2010}']) else {
        // No hyphen survived the backend: the words themselves are the only evidence.
        return vocab.rejoin(head, tail);
    };
    if stem.is_empty() {
        return None;
    }
    match vocab.rejoin(stem, tail) {
        Some(merged) => Some(merged),
        // A compound broken at its own hyphen still joins, keeping the hyphen; it just must not
        // acquire a space that was never in the word.
        None => tail
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-')
            .then(|| format!("{head}{tail}")),
    }
}

/// Whether a line runs to the block's right edge, within one character's slack.
fn line_reaches(line: &Line, right_edge: f32, size: f32) -> bool {
    right_edge - line.bbox.x1 <= size
}

/// The last word of `left` and the first word of `right`, if both exist.
fn split_boundary<'a>(left: &'a str, right: &'a str) -> Option<(&'a str, &'a str)> {
    let head = left.rsplit(' ').next()?;
    let tail = right.split(' ').next()?;
    (!head.is_empty() && !tail.is_empty()).then_some((head, tail))
}

fn head_len(text: &str) -> usize {
    text.rsplit(' ').next().map_or(0, str::len)
}

fn first_word_len(text: &str) -> usize {
    text.split(' ').next().map_or(0, str::len)
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

    // A lettered component is a single capital — appendices run `A`, `B`, `C`. Allowing longer
    // runs of capitals made every word of a title set in caps look like a label, so
    // `ON DIOPHANTINE SETS...` parsed as section `ON`.
    let plausible = components.iter().all(|part| {
        !part.is_empty()
            && ((part.len() <= MAX_NUMBERING_COMPONENT && part.chars().all(|c| c.is_ascii_digit()))
                || (part.len() == 1 && part.chars().all(|c| c.is_ascii_uppercase())))
    });
    if !plausible {
        return false;
    }

    // A lettered label must carry its full stop. Appendices are written `A.` or `A.1`, never a
    // bare `A` — and a bare one would make every title beginning `A ...` a numbered heading.
    let lettered = components
        .iter()
        .any(|p| p.chars().all(|c| c.is_ascii_uppercase()));
    if lettered && !head.contains('.') {
        return false;
    }

    // A bare label with nothing after it is a list marker or a stray number, not a heading.
    text[head.len()..]
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic())
}

/// Merges stranded fragments back into the block above them.
///
/// Blocks split whenever the font size changes by more than a fraction of a point, which is
/// right for prose but wrong inside a long derivation: a line carrying a fraction, a summation
/// sign and a subscript spans three sizes and shatters into three blocks. On a physics paper
/// with page-long derivations that left a quarter of all blocks holding three words or fewer —
/// `we have`, `∑`, `$ab$`.
///
/// A fragment rejoins the block above when it is short, sits directly beneath it, and shares
/// its horizontal extent. Prose is unaffected because prose blocks are not short.
fn coalesce_fragments(blocks: &mut Vec<Block>, stats: Stats) {
    let mut merged: Vec<Block> = Vec::with_capacity(blocks.len());

    for block in blocks.drain(..) {
        let joins = merged.last().is_some_and(|previous| {
            if !is_fragment(&block)
                || previous.page != block.page
                || !matches!(previous.kind, BlockKind::Paragraph)
                || !matches!(block.kind, BlockKind::Paragraph)
            {
                return false;
            }

            // A fragment that opens a sentence is a new element, not a continuation. Templates
            // that set `References` at body size make it a one-word paragraph, and absorbing it
            // into the paragraph above cost a whole paper its bibliography — the heading is what
            // the bibliography pass looks for.
            let opens_element = block.text.chars().next().is_some_and(char::is_uppercase)
                && previous.text.ends_with(['.', ':', '?', '!']);
            if opens_element || crate::refs::is_bibliography_heading(&block.text) {
                return false;
            }

            // Stacked: a continuation directly beneath, sharing the column.
            let below = block.bbox.y0 - previous.bbox.y1 <= stats.leading * FRAGMENT_MAX_GAP
                && previous.bbox.x_overlap(&block.bbox) > 0.0;

            // Side by side: the same visual row. A line of a derivation carrying a fraction and
            // a summation sits on three baselines at three sizes, so line building sees three
            // lines and block assembly splits them on size — leaving `we have`, `∑` and `$ab$`
            // as separate blocks strung along one row.
            let shortest = previous.bbox.height().min(block.bbox.height()).max(1.0);
            let beside = previous.bbox.y_overlap(&block.bbox) >= shortest * FRAGMENT_ROW_OVERLAP
                && block.bbox.x0 >= previous.bbox.x0;

            below || beside
        });

        match joins {
            true => {
                let previous = merged.last_mut().expect("checked above");
                previous.text.push(' ');
                previous.text.push_str(&block.text);
                previous.bbox = previous.bbox.union(&block.bbox);
            }
            false => merged.push(block),
        }
    }

    *blocks = merged;
}

fn is_fragment(block: &Block) -> bool {
    block.text.split_whitespace().count() <= FRAGMENT_MAX_WORDS
}

/// Turns list items with no siblings back into paragraphs.
///
/// A marker on its own is weak evidence: a paragraph opening `4. First, the situation is
/// reversed...` and a stray table cell reading `- 43.9` both look like list items. A real list
/// has more than one item, and its items sit together, so requiring a neighbour costs nothing
/// on genuine lists and removes both false positives.
fn demote_lonely_list_items(blocks: &mut [Block]) {
    let is_item: Vec<bool> = blocks
        .iter()
        .map(|b| matches!(b.kind, BlockKind::ListItem { .. }))
        .collect();

    for i in 0..blocks.len() {
        if !is_item[i] {
            continue;
        }
        let lo = i.saturating_sub(LIST_SIBLING_RANGE);
        let hi = (i + LIST_SIBLING_RANGE + 1).min(blocks.len());
        let has_sibling =
            (lo..hi).any(|j| j != i && is_item[j] && blocks[j].page == blocks[i].page);
        if !has_sibling {
            blocks[i].kind = BlockKind::Paragraph;
        }
    }
}

/// Promotes the document's title.
///
/// Size alone is not enough. A `amsart` paper sets its title in capitals at *body size* and
/// distinguishes it by position and case, so requiring the title to be larger than the body
/// found no title at all on a pure-maths paper. What holds across templates is that the title is
/// among the opening blocks of page one, is a few words long, is set no smaller than the body,
/// and does not end in a full stop.
///
/// Length breaks ties. A running head such as `Preprint` sits above the title at the same size,
/// and picking on size alone chose whichever came last.
fn promote_title(blocks: &mut [Block], stats: Stats) {
    let candidate = blocks
        .iter()
        .enumerate()
        .take(TITLE_SEARCH_BLOCKS)
        .filter(|(_, b)| b.page == 0 && b.size >= stats.body_size)
        .filter(|(_, b)| {
            let words = b.text.split_whitespace().count();
            (TITLE_MIN_WORDS..=TITLE_MAX_WORDS).contains(&words)
                && !b.text.ends_with('.')
                // A numbered block is a section heading. Papers do not number their titles, and
                // without this a document opening with `1 Introduction` promotes it.
                && !numbered_heading(&b.text)
        })
        .max_by(|(_, a), (_, b)| {
            a.size.total_cmp(&b.size).then_with(|| {
                a.text
                    .split_whitespace()
                    .count()
                    .cmp(&b.text.split_whitespace().count())
            })
        })
        .map(|(i, _)| i);

    if let Some(index) = candidate {
        blocks[index].kind = BlockKind::Title;
    }
}

/// Assigns heading levels.
///
/// Section numbering is authoritative where it exists — `3.2` is unambiguously one level below
/// `3` — and it also calibrates the headings that have no number. `Abstract` is set at exactly
/// the size of `1 Introduction` and belongs at the same level, but ranking sizes in isolation
/// gave it level 3; matching it against the size of a heading whose level *is* known fixes that.
/// Size rank remains the fallback for templates that number nothing.
fn assign_heading_levels(blocks: &mut [Block]) {
    // Sizes whose level is known from numbering.
    let mut calibrated: Vec<(f32, u8)> = Vec::new();
    for block in blocks.iter() {
        if !matches!(block.kind, BlockKind::Heading { .. }) {
            continue;
        }
        if let Some(depth) = numbering_depth(&block.text) {
            if !calibrated
                .iter()
                .any(|(size, _)| (*size - block.size).abs() < 0.25)
            {
                calibrated.push((block.size, depth));
            }
        }
    }

    let mut sizes: Vec<f32> = blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Heading { .. }))
        .map(|b| b.size)
        .collect();
    sizes.sort_by(|a, b| b.total_cmp(a));
    sizes.dedup_by(|a, b| (*a - *b).abs() < 0.25);

    for block in blocks.iter_mut() {
        if !matches!(block.kind, BlockKind::Heading { .. }) {
            continue;
        }
        let level = numbering_depth(&block.text)
            .or_else(|| {
                calibrated
                    .iter()
                    .find(|(size, _)| (*size - block.size).abs() < 0.25)
                    .map(|(_, depth)| *depth)
            })
            .unwrap_or_else(|| {
                let rank = sizes
                    .iter()
                    .position(|s| (s - block.size).abs() < 0.25)
                    .unwrap_or(0);
                (rank + 1).min(6) as u8
            });
        block.kind = BlockKind::Heading {
            level: level.max(1),
        };
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

    /// A vocabulary in which `learning` is an established word of the document.
    fn learned_vocab() -> Vocabulary {
        // Only a line's *interior* words are counted — a fragment is always at one end — so
        // every occurrence here needs a word on each side of it.
        let one_line = |words: &[&str]| Line {
            words: words
                .iter()
                .map(|t| Word {
                    bbox: Rect::from_corners(0.0, 0.0, 1.0, 1.0),
                    text: (*t).to_owned(),
                    start: 0,
                    end: 1,
                })
                .collect(),
            ..line("", 100.0, 10.0, 0.0, 50.0)
        };
        Vocabulary::build(&[vec![
            one_line(&["the", "learning", "rate", "and", "learning", "curve", "of"]),
            one_line(&["a", "learning", "b", "learning", "c", "learning", "d"]),
        ]])
    }

    #[test]
    fn an_explicit_break_hyphen_is_removed_when_the_word_is_known() {
        let vocab = learned_vocab();
        // What rustium reports: the typesetter's hyphen is still on the page.
        assert_eq!(
            rejoin_across_break(&vocab, "learn-", "ing"),
            Some("learning".into())
        );
        // What pdfium reports: no hyphen, so the words alone have to carry it.
        assert_eq!(
            rejoin_across_break(&vocab, "learn", "ing"),
            Some("learning".into())
        );
    }

    #[test]
    fn a_compound_word_keeps_the_authors_hyphen() {
        let vocab = learned_vocab();
        // `stateof-the-art` is not a word; the hyphen belongs to the compound and stays.
        assert_eq!(
            rejoin_across_break(&vocab, "state-", "of-the-art"),
            Some("state-of-the-art".into())
        );
    }

    #[test]
    fn a_break_before_punctuation_is_not_a_hyphenation() {
        let vocab = learned_vocab();
        // A trailing hyphen with nothing to attach to must not produce a bare join.
        assert_eq!(rejoin_across_break(&vocab, "-", "ing"), None);
        // Punctuation starting the next line means the line ended for another reason.
        assert_eq!(rejoin_across_break(&vocab, "learn-", "(ing)"), None);
    }

    #[test]
    fn consecutive_body_lines_form_one_paragraph() {
        let page = vec![
            line("The first line of a paragraph", 100.0, 10.0, 72.0, 540.0),
            line("and its continuation.", 112.0, 10.0, 72.0, 540.0),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
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
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
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
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        let kinds: Vec<&BlockKind> = doc.blocks.iter().map(|b| &b.kind).collect();
        assert_eq!(kinds[0], &BlockKind::Heading { level: 1 });
        assert_eq!(kinds[1], &BlockKind::Paragraph);
        assert_eq!(kinds[2], &BlockKind::Heading { level: 2 });
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
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        assert_eq!(doc.blocks[0].kind, BlockKind::Title);
        assert!(
            matches!(doc.blocks[2].kind, BlockKind::Heading { .. }),
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
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
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
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
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
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
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
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        assert_eq!(doc.title.as_deref(), Some("Deep Residual Learning"));
        assert_eq!(doc.blocks[0].kind, BlockKind::Title);
    }

    #[test]
    fn a_section_number_alone_is_not_a_heading_pattern() {
        assert!(!numbered_heading("3.14159 is pi"));
        assert!(numbered_heading("3.2 Ablation studies"));
        assert!(numbered_heading("A.1 Proofs"));
        assert!(numbered_heading("B. Further results"));
        assert!(!numbered_heading("However the result holds"));
    }

    /// Capitalised words are not section labels, however short.
    #[test]
    fn words_set_in_capitals_are_not_numbering() {
        assert!(!numbered_heading("ON DIOPHANTINE SETS OVER THE RATIONALS"));
        assert!(!numbered_heading("IN THIS PAPER we show"));
        // A bare capital with no full stop opens a title, it does not label a section.
        assert!(!numbered_heading("A Neural Algorithm of Artistic Style"));
        assert!(!numbered_heading("I Introduction is not a label"));
    }

    #[test]
    fn a_fragment_directly_below_rejoins_the_block_above() {
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line(
                "The paragraph text runs on here for a while.",
                100.0,
                10.0,
                72.0,
                400.0,
            ),
            // A stray fragment one line below, sharing the column.
            line("and so", 112.0, 7.0, 72.0, 110.0),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        assert!(
            doc.blocks.iter().any(|b| b.text.ends_with("and so")),
            "the fragment did not rejoin: {:?}",
            doc.blocks.iter().map(|b| &b.text).collect::<Vec<_>>()
        );
    }

    /// The case that motivates it: a derivation whose row sits on three baselines at three
    /// sizes, which line building sees as three lines and assembly splits on size.
    #[test]
    fn fragments_on_the_same_row_are_gathered() {
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line("we have", 300.0, 10.0, 100.0, 150.0),
            line("N", 297.0, 7.0, 160.0, 168.0),
            line("= 1", 303.0, 9.4, 175.0, 200.0),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        let gathered = doc
            .blocks
            .iter()
            .any(|b| b.text.contains("we have") && b.text.contains("= 1"));
        assert!(
            gathered,
            "the row was not gathered: {:?}",
            doc.blocks.iter().map(|b| &b.text).collect::<Vec<_>>()
        );
    }

    /// Prose must be left alone: only *short* blocks are fragments.
    #[test]
    fn full_paragraphs_are_never_merged() {
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line(
                "The first paragraph says something at length here.",
                100.0,
                10.0,
                72.0,
                400.0,
            ),
            line(
                "The second paragraph says something else entirely.",
                140.0,
                10.0,
                72.0,
                400.0,
            ),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        let paragraphs = doc
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Paragraph)
            .count();
        assert_eq!(paragraphs, 2, "two paragraphs were merged into one");
    }

    #[test]
    fn a_fragment_far_below_stays_separate() {
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line(
                "The paragraph text runs on here for a while.",
                100.0,
                10.0,
                72.0,
                400.0,
            ),
            // Half a page down, and not on the same row.
            line("stray", 500.0, 10.0, 72.0, 110.0),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        assert!(
            doc.blocks.iter().any(|b| b.text == "stray"),
            "a distant fragment was absorbed"
        );
    }

    /// A body-size `References` heading is a one-word paragraph. Absorbing it into the block
    /// above costs the document its whole bibliography, because that heading is what the
    /// bibliography pass looks for.
    #[test]
    fn a_heading_sized_like_body_text_is_not_absorbed() {
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line(
                "The last paragraph of the paper ends here.",
                300.0,
                10.0,
                72.0,
                400.0,
            ),
            // Set apart by more than the block gap, so these really are separate blocks and the
            // coalescing pass is what decides their fate.
            line("References", 345.0, 10.0, 72.0, 130.0),
            line(
                "[1] A. Author. A title. Venue, 2016.",
                390.0,
                10.0,
                72.0,
                400.0,
            ),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        assert!(
            doc.blocks.iter().any(|b| b.text == "References"),
            "the heading was absorbed: {:?}",
            doc.blocks.iter().map(|b| &b.text).collect::<Vec<_>>()
        );
    }

    /// A title set at body size, distinguished only by position and case, as `amsart` does.
    #[test]
    fn a_title_at_body_size_is_still_found() {
        let page = vec![
            line("Preprint", 40.0, 10.0, 250.0, 300.0),
            line(
                "ON DIOPHANTINE SETS OVER THE RATIONALS",
                70.0,
                10.0,
                150.0,
                440.0,
            ),
            line("A. Author", 95.0, 9.0, 250.0, 330.0),
            line(
                "Body text that runs on for a while here.",
                130.0,
                10.0,
                72.0,
                540.0,
            ),
            line("1 Introduction", 170.0, 10.0, 72.0, 200.0),
            line("More body text follows on here.", 200.0, 10.0, 72.0, 540.0),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());
        assert_eq!(
            doc.title.as_deref(),
            Some("ON DIOPHANTINE SETS OVER THE RATIONALS"),
            "a one-word running head above it should not win on length"
        );
        // And the numbered section must remain a heading.
        assert!(doc
            .blocks
            .iter()
            .any(|b| b.text == "1 Introduction" && matches!(b.kind, BlockKind::Heading { .. })));
    }

    /// An unnumbered heading takes its level from numbered headings of the same size.
    #[test]
    fn unnumbered_headings_are_calibrated_against_numbered_ones() {
        let page = vec![
            line("A Paper About Things", 40.0, 20.0, 72.0, 400.0),
            line("Abstract", 80.0, 12.0, 72.0, 160.0),
            line("Body text that continues on.", 110.0, 10.0, 72.0, 540.0),
            line("1 Introduction", 140.0, 12.0, 72.0, 200.0),
            line("More body text follows here.", 170.0, 10.0, 72.0, 540.0),
            line("1.1 Background", 200.0, 10.5, 72.0, 200.0),
            line("Yet more body text here.", 230.0, 10.0, 72.0, 540.0),
        ];
        let doc = assemble(&[page], &[792.0], stats(), &Vocabulary::default());

        let level = |text: &str| {
            doc.blocks
                .iter()
                .find(|b| b.text == text)
                .map(|b| b.kind.clone())
        };
        assert_eq!(
            level("1 Introduction"),
            Some(BlockKind::Heading { level: 1 })
        );
        assert_eq!(
            level("Abstract"),
            Some(BlockKind::Heading { level: 1 }),
            "Abstract is set at the same size as a level-1 section"
        );
        assert_eq!(
            level("1.1 Background"),
            Some(BlockKind::Heading { level: 2 })
        );
    }

    #[test]
    fn numbering_depth_counts_components() {
        assert_eq!(numbering_depth("1 Introduction"), Some(1));
        assert_eq!(numbering_depth("3.2 Ablation"), Some(2));
        assert_eq!(numbering_depth("3.2.1 Detail"), Some(3));
        assert_eq!(numbering_depth("Introduction"), None);
    }
}
