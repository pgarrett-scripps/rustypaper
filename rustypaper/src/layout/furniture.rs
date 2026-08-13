//! Removing running heads, folios and watermarks.
//!
//! Page furniture is the text that belongs to the *publication* rather than the document:
//! journal names along the top of every page, page numbers, conference footers, preprint
//! watermarks. It is cheap to find — it repeats in the same place on most pages — and removing
//! it strips a large fraction of the noise that generic extractors ship.
//!
//! Two conditions must both hold, which keeps genuinely repeated body text safe: the line has to
//! sit in a margin band, and its signature has to recur on most pages.
//!
//! The second condition has a blind spot, because a journal running head is usually *two* heads.
//! A recto/verso pair — the title on the right-hand page, the authors on the left — puts each of
//! them on every other page, so neither can ever appear on much more than half the pages, and
//! anything that thins them further (a title page carrying the journal line instead, a plate page
//! carrying no head at all) drops both below a half-the-document quorum. So the count is also
//! kept per page parity, and a signature that carries its own class is furniture as surely as one
//! that carries the whole document. See [`alternates`] for what stops that from being a licence
//! to delete anything that repeats twice.

use std::collections::HashMap;

use crate::text::lines::Line;

/// Fraction of the page height at the top and bottom within which furniture can live.
const MARGIN_BAND: f32 = 0.12;

/// A signature must appear on at least this fraction of pages to count as furniture.
const MIN_PAGE_FRACTION: f32 = 0.5;

/// Below this many pages there is no evidence of repetition worth acting on.
const MIN_PAGES: usize = 4;

/// Below this many pages *of one parity* there is no evidence of an alternation worth acting on.
const MIN_CLASS_PAGES: usize = 3;

/// However small a parity class is, this many occurrences are needed to call it a pattern.
const MIN_CLASS_OCCURRENCES: usize = 2;

/// Which margin a line sits in.
///
/// The two edges are counted apart because they alternate independently: a document can set a
/// recto/verso head at the top and a folio in the same place on every page at the bottom, and
/// evidence of an alternation at one edge is no evidence of one at the other.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum Edge {
    Top,
    Bottom,
}

/// Drops page furniture from each page's lines, returning how many lines were removed.
pub fn strip(pages: &mut [Vec<Line>], page_heights: &[f32]) -> usize {
    if pages.len() < MIN_PAGES {
        return 0;
    }

    let total = pages.len();

    // Count the *pages* a signature appears on, not its total occurrences, so a line repeated
    // several times on one page cannot vote itself out. `seen` counts across the document;
    // `by_class` keeps the same count split by margin and page parity, for heads that alternate.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut by_class: HashMap<(Edge, usize), HashMap<String, usize>> = HashMap::new();

    for (index, (lines, &height)) in pages.iter().zip(page_heights).enumerate() {
        let mut on_this_page: Vec<(Edge, String)> = lines
            .iter()
            .filter_map(|l| Some((edge(l, height)?, signature(l))))
            .collect();
        on_this_page.sort();
        on_this_page.dedup();

        for (at, key) in &on_this_page {
            *by_class
                .entry((*at, index % 2))
                .or_default()
                .entry(key.clone())
                .or_default() += 1;
        }

        // A signature sitting at both margins of one page is still one page for the whole-document
        // count, which is what it has always been.
        let mut keys: Vec<&String> = on_this_page.iter().map(|(_, key)| key).collect();
        keys.sort();
        keys.dedup();
        for key in keys {
            *seen.entry(key.clone()).or_default() += 1;
        }
    }

    let quorum = (total as f32 * MIN_PAGE_FRACTION).ceil() as usize;
    let alternating: Vec<Edge> = [Edge::Top, Edge::Bottom]
        .into_iter()
        .filter(|&at| alternates(at, total, &by_class))
        .collect();

    let mut removed = 0;
    for (index, (lines, &height)) in pages.iter_mut().zip(page_heights).enumerate() {
        let parity = index % 2;
        lines.retain(|line| {
            let Some(at) = edge(line, height) else {
                return true;
            };
            let key = signature(line);
            let repeats = seen.get(&key).is_some_and(|&n| n >= quorum);
            let carries_its_class = alternating.contains(&at)
                && by_class
                    .get(&(at, parity))
                    .and_then(|counts| counts.get(&key))
                    .is_some_and(|&n| n >= class_quorum(class_size(total, parity)));
            let furniture = repeats || carries_its_class;
            removed += usize::from(furniture);
            !furniture
        });
    }

    removed
}

/// Whether the head at this margin alternates between recto and verso pages.
///
/// This is the guard on the parity count, and it is what keeps a section heading that happens to
/// repeat from being deleted. Half of a parity class is a low bar on a short paper — three pages
/// a side means two occurrences — so the relaxation is only granted where the document is visibly
/// setting an alternating head at this margin *at all*: **both** parity classes must have some
/// signature *of their own* that carries the class. An `Appendix` heading landing at the top of
/// two same-parity pages is left where it is in a paper that sets no top head, in one whose only
/// alternation is the folio at the foot, and in one whose top line is the same on both sides.
fn alternates(
    at: Edge,
    total: usize,
    by_class: &HashMap<(Edge, usize), HashMap<String, usize>>,
) -> bool {
    let carriers = |parity: usize| -> Option<Vec<&String>> {
        let size = class_size(total, parity);
        if size < MIN_CLASS_PAGES {
            return None;
        }
        let needed = class_quorum(size);
        let counts = by_class.get(&(at, parity))?;
        Some(
            counts
                .iter()
                .filter(|(_, &n)| n >= needed)
                .map(|(key, _)| key)
                .collect(),
        )
    };

    let (Some(recto), Some(verso)) = (carriers(0), carriers(1)) else {
        return false;
    };
    // Each side must carry something the other side does not. A line the two classes share is
    // evidence of a head on *every* page, which the whole-document quorum already answers, and
    // letting it stand as one half of a pair would hand the other half to whatever else happened
    // to repeat at that margin.
    recto.iter().any(|key| !verso.contains(key)) && verso.iter().any(|key| !recto.contains(key))
}

/// How many of `total` pages have the given index parity.
fn class_size(total: usize, parity: usize) -> usize {
    total / 2 + usize::from(parity == 0 && total % 2 == 1)
}

/// How many pages of a parity class a signature must appear on to carry it.
fn class_quorum(size: usize) -> usize {
    (((size as f32) * MIN_PAGE_FRACTION).ceil() as usize).max(MIN_CLASS_OCCURRENCES)
}

fn edge(line: &Line, page_height: f32) -> Option<Edge> {
    if page_height <= 0.0 {
        return None;
    }
    let y = line.baseline / page_height;
    if y <= MARGIN_BAND {
        Some(Edge::Top)
    } else if y >= 1.0 - MARGIN_BAND {
        Some(Edge::Bottom)
    } else {
        None
    }
}

/// A line's identity for repetition purposes.
///
/// Digits collapse to `#` so that folios (`3`, `4`, `5`) and running heads carrying a page or
/// section number all share one signature. Case and spacing are normalised for the same reason.
fn signature(line: &Line) -> String {
    let mut out = String::new();
    let mut last_was_digit = false;
    for c in line.text().chars() {
        if c.is_ascii_digit() {
            if !last_was_digit {
                out.push('#');
            }
            last_was_digit = true;
        } else if c.is_whitespace() {
            last_was_digit = false;
        } else {
            out.extend(c.to_lowercase());
            last_was_digit = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Rect;
    use crate::text::lines::{Line, Word};

    const HEIGHT: f32 = 792.0;

    fn line(text: &str, baseline: f32) -> Line {
        let bbox = Rect::from_corners(72.0, baseline - 10.0, 540.0, baseline);
        Line {
            bbox,
            baseline,
            size: 10.0,
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

    fn document(n: usize) -> (Vec<Vec<Line>>, Vec<f32>) {
        let pages = (0..n)
            .map(|i| {
                vec![
                    line("Preprint. Under review.", 40.0),
                    line("body text that differs per page", 300.0),
                    line(&format!("{}", i + 1), 760.0),
                ]
            })
            .collect();
        (pages, vec![HEIGHT; n])
    }

    #[test]
    fn running_heads_and_folios_are_removed() {
        let (mut pages, heights) = document(8);
        let removed = strip(&mut pages, &heights);
        assert_eq!(removed, 16, "expected one head and one folio per page");
        for page in &pages {
            assert_eq!(page.len(), 1);
            assert_eq!(page[0].text(), "body text that differs per page");
        }
    }

    #[test]
    fn page_numbers_collapse_to_one_signature_despite_differing() {
        let (mut pages, heights) = document(8);
        strip(&mut pages, &heights);
        assert!(
            pages.iter().all(|p| p.iter().all(|l| l.text() != "1")),
            "folios differ per page and must still be recognised as one signature"
        );
    }

    #[test]
    fn body_text_in_the_middle_of_the_page_is_never_furniture() {
        // The same line in the body band on every page: repeated, but not in a margin.
        let mut pages: Vec<Vec<Line>> = (0..8)
            .map(|_| vec![line("identical body", 400.0)])
            .collect();
        let heights = vec![HEIGHT; 8];
        assert_eq!(strip(&mut pages, &heights), 0);
        assert!(pages.iter().all(|p| p.len() == 1));
    }

    #[test]
    fn a_margin_line_appearing_once_is_kept() {
        let (mut pages, heights) = document(8);
        pages[0].push(line("Accepted at NeurIPS 2024", 55.0));
        strip(&mut pages, &heights);
        assert!(
            pages[0].iter().any(|l| l.text().starts_with("Accepted")),
            "a one-off margin note is not furniture"
        );
    }

    #[test]
    fn short_documents_are_left_alone() {
        let (mut pages, heights) = document(3);
        assert_eq!(strip(&mut pages, &heights), 0);
        assert!(pages.iter().all(|p| p.len() == 3));
    }

    /// A journal set the way `sklearn.pdf` is: the title on recto pages, the authors on verso,
    /// and a first page that carries the journal's own line instead of either.
    ///
    /// The title head can then reach only two of the six pages, a third of the document, so a
    /// half-the-document quorum leaves it standing — and a line in the title's own face at the
    /// top of a page is then read as a section heading.
    fn alternating(n: usize) -> (Vec<Vec<Line>>, Vec<f32>) {
        let pages = (0..n)
            .map(|i| {
                let head = match i {
                    0 => "Journal of Machine Learning Research 12 (2011) 2825-2830",
                    _ if i % 2 == 0 => "Scikit-learn: Machine Learning in Python",
                    _ => "Pedregosa, Varoquaux, Gramfort et al.",
                };
                vec![
                    line(head, 48.0),
                    line(&format!("body of page {}", i + 1), 300.0),
                    line(&format!("{}", 2825 + i), 733.0),
                ]
            })
            .collect();
        (pages, vec![HEIGHT; n])
    }

    #[test]
    fn a_head_that_alternates_recto_and_verso_is_removed() {
        let (mut pages, heights) = alternating(6);
        strip(&mut pages, &heights);

        let left: Vec<String> = pages.iter().flatten().map(Line::text).collect();
        for head in ["Scikit-learn", "Pedregosa"] {
            assert!(
                !left.iter().any(|t| t.starts_with(head)),
                "the {head:?} head survived, in {left:?}"
            );
        }
        // The body of every page, and the journal line the first page carries once, remain.
        for (i, page) in pages.iter().enumerate() {
            assert!(
                page.iter()
                    .any(|l| l.text() == format!("body of page {}", i + 1)),
                "page {} lost its body",
                i + 1
            );
        }
        assert_eq!(pages[0].len(), 2, "the journal line appears once and stays");
    }

    #[test]
    fn the_odd_head_out_on_the_first_page_goes_with_the_rest() {
        // One occurrence is one occurrence however alternating the pages around it are.
        let (mut pages, heights) = alternating(6);
        pages[0].push(line("Submitted 3/11; Published 10/11", 60.0));
        strip(&mut pages, &heights);
        assert!(
            pages[0].iter().any(|l| l.text().starts_with("Submitted")),
            "a line appearing once is not furniture, alternation or no alternation"
        );
    }

    /// Distinct words, so that the digit-collapsing signature cannot make two unrelated lines
    /// one. `page 3` and `page 5` share a signature; `gamma` and `epsilon` do not.
    const DISTINCT: [&str; 8] = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ];

    fn appendix_repeated_on_two_recto_pages() -> Vec<Vec<Line>> {
        let mut pages: Vec<Vec<Line>> = (0..8)
            .map(|i| {
                vec![
                    line(DISTINCT[i], 48.0),
                    line(&format!("body of page {}", i + 1), 300.0),
                ]
            })
            .collect();
        pages[2][0] = line("Appendix A. Proofs", 48.0);
        pages[4][0] = line("Appendix A. Proofs", 48.0);
        pages
    }

    fn pages_holding_the_appendix(pages: &[Vec<Line>]) -> usize {
        pages
            .iter()
            .filter(|p| p.iter().any(|l| l.text().starts_with("Appendix")))
            .count()
    }

    /// The guard. Two same-parity pages carrying the same heading, in a document that sets no
    /// head at that margin, are two pages carrying the same heading.
    #[test]
    fn a_heading_repeating_on_same_parity_pages_survives() {
        let mut pages = appendix_repeated_on_two_recto_pages();
        let heights = vec![HEIGHT; 8];
        assert_eq!(strip(&mut pages, &heights), 0);
        assert_eq!(pages_holding_the_appendix(&pages), 2);
    }

    /// The guard again, this time in a document that *does* alternate — at the other margin.
    /// A folio pattern at the foot says nothing about what the top of the page holds.
    #[test]
    fn an_alternation_at_one_margin_does_not_unlock_the_other() {
        let mut pages = appendix_repeated_on_two_recto_pages();
        for (i, page) in pages.iter_mut().enumerate() {
            let folio = if i % 2 == 0 {
                format!("{}   Draft", i + 1)
            } else {
                format!("Draft   {}", i + 1)
            };
            page.push(line(&folio, 760.0));
        }
        let heights = vec![HEIGHT; 8];

        strip(&mut pages, &heights);
        assert_eq!(
            pages_holding_the_appendix(&pages),
            2,
            "the heading is at the top; only the foot of the page alternates"
        );
        assert!(
            pages.iter().all(|p| p.len() <= 2),
            "the alternating folio itself is furniture and should be gone"
        );
    }

    /// The guard once more, on the case the signature's own normalisation creates: every page
    /// opens with `Draft of page n`, which collapses to one signature on both sides. That is a
    /// head on every page, not an alternation, and it cannot lend its evidence to the `Appendix`
    /// heading standing beside it.
    #[test]
    fn each_side_of_an_alternation_needs_a_head_of_its_own() {
        let mut pages = appendix_repeated_on_two_recto_pages();
        for (i, page) in pages.iter_mut().enumerate() {
            page[0] = if i == 2 || i == 4 {
                line("Appendix A. Proofs", 48.0)
            } else {
                line(&format!("Draft of page {}", i + 1), 48.0)
            };
        }
        let heights = vec![HEIGHT; 8];

        strip(&mut pages, &heights);
        assert_eq!(
            pages_holding_the_appendix(&pages),
            2,
            "one head shared by both sides is not a recto/verso pair"
        );
        assert!(
            pages
                .iter()
                .all(|p| !p.iter().any(|l| l.text().starts_with("Draft"))),
            "the head on every page is furniture by the whole-document quorum"
        );
    }

    #[test]
    fn an_alternation_needs_three_pages_a_side() {
        // Four pages is two a side, and two pages agreeing is not a pattern.
        let (mut pages, heights) = alternating(4);
        strip(&mut pages, &heights);
        assert!(
            pages[2]
                .iter()
                .any(|l| l.text().starts_with("Scikit-learn")),
            "two same-parity pages are too thin to establish an alternation"
        );
    }

    #[test]
    fn class_sizes_partition_the_document() {
        for total in 0..12 {
            assert_eq!(class_size(total, 0) + class_size(total, 1), total);
            assert!(class_size(total, 0) >= class_size(total, 1));
        }
    }
}
