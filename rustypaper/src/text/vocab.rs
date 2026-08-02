//! Rejoining words split across a line break.
//!
//! A backend may remove soft line-break hyphens from the text page. pdfium does — Chrome does it
//! so that copy-paste rejoins hyphenated words — so by the time the glyphs reach us, `learn-` at
//! the end of one line and `ing` at the start of the next have become `learn` and `ing` with no
//! marker at all, and there is nothing to key off but the words themselves. rustium leaves the
//! hyphen where the document put it, and [`crate::doc`] prefers it when it is there; this module
//! is what happens when it is not.
//!
//! The evidence used is the document's own vocabulary. A paper that hyphenates `learn-ing` at
//! one line break almost always writes `learning` somewhere else, so the merged form can be
//! checked against text the document has already produced. That is far safer than a dictionary:
//! it needs no word list, works in any language, and knows the paper's own jargon.

use std::collections::HashMap;

use crate::text::lines::Line;

/// Shortest merged word worth forming. Below this, coincidental merges (`the` + `n`) outnumber
/// real ones.
const MIN_MERGED_LENGTH: usize = 6;

/// Shortest tail fragment. A single letter carried to the next line is rare, and allowing it
/// admits far too many false merges.
const MIN_TAIL_LENGTH: usize = 2;

/// The merged form must be at least this many times as common as the tail standing alone.
/// `in` + `stead` is a real break even though `in` is a word; `the` + `n` is not, because `n`
/// never stands alone but `then` is not more common than... — the ratio is what separates them.
const MIN_FREQUENCY_RATIO: u32 = 2;

/// Word frequencies across a document.
#[derive(Debug, Default, Clone)]
pub struct Vocabulary {
    counts: HashMap<String, u32>,
}

impl Vocabulary {
    /// Counts words that are interior to a line — neither first nor last.
    ///
    /// The restriction is essential, not an optimisation. A hyphenation fragment is *always*
    /// line-final (`learn`) or line-initial (`ing`), so counting every word lets the fragments
    /// pollute the vocabulary that is supposed to judge them. Measured on ResNet, `ing` was
    /// recorded as a word four times — once per break — which dragged the frequency test below
    /// its threshold and rejected every genuine rejoin. Interior words cannot be fragments, and
    /// a paper has tens of thousands of them, so the evidence stays ample.
    pub fn build(pages: &[Vec<Line>]) -> Self {
        let mut counts: HashMap<String, u32> = HashMap::new();
        for line in pages.iter().flatten() {
            if line.words.len() < 3 {
                continue;
            }
            for word in &line.words[1..line.words.len() - 1] {
                if let Some(key) = normalise(&word.text) {
                    *counts.entry(key).or_default() += 1;
                }
            }
        }
        Self { counts }
    }

    pub fn count(&self, word: &str) -> u32 {
        normalise(word)
            .and_then(|key| self.counts.get(&key).copied())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Decides whether `head` at a line end and `tail` at the next line start are one word.
    ///
    /// Returns the merged form when they are.
    pub fn rejoin(&self, head: &str, tail: &str) -> Option<String> {
        // Punctuation at the break means the line ended for a reason other than hyphenation.
        if !head.chars().all(is_word_char) || !tail.chars().all(is_word_char) {
            return None;
        }
        if tail.chars().count() < MIN_TAIL_LENGTH {
            return None;
        }
        // A capitalised tail is a new sentence or a proper noun, not a word continuing.
        if tail.chars().next().is_some_and(char::is_uppercase) {
            return None;
        }

        let merged = format!("{head}{tail}");
        if merged.chars().count() < MIN_MERGED_LENGTH {
            return None;
        }

        let merged_count = self.count(&merged);
        if merged_count == 0 {
            return None;
        }
        // The tail standing alone is the competing explanation. `ing` and `stead` essentially
        // never appear as words, so the merged form wins easily; for a tail that is a common
        // word in its own right, it should not.
        if merged_count < self.count(tail).saturating_mul(MIN_FREQUENCY_RATIO).max(1) {
            return None;
        }

        Some(merged)
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || c == '-'
}

/// Lowercased form used as the lookup key, or `None` for anything that is not a word.
fn normalise(word: &str) -> Option<String> {
    if word.is_empty() || !word.chars().all(is_word_char) {
        return None;
    }
    Some(word.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(words: &[(&str, u32)]) -> Vocabulary {
        let mut counts = HashMap::new();
        for (word, n) in words {
            counts.insert(word.to_string(), *n);
        }
        Vocabulary { counts }
    }

    #[test]
    fn rejoins_a_word_the_document_uses_elsewhere() {
        let v = vocab(&[("learning", 12), ("learn", 3)]);
        assert_eq!(v.rejoin("learn", "ing").as_deref(), Some("learning"));
    }

    #[test]
    fn refuses_when_the_merged_form_is_unknown() {
        let v = vocab(&[("learn", 3), ("ing", 1)]);
        assert_eq!(v.rejoin("learn", "ing"), None);
    }

    /// The failure mode this guards against: two ordinary words that happen to concatenate.
    #[test]
    fn refuses_when_the_tail_is_a_common_word_itself() {
        let v = vocab(&[("theory", 4), ("the", 400), ("ory", 0)]);
        assert_eq!(v.rejoin("the", "ory"), Some("theory".into()));

        // `over` + `all` -> `overall`, but `all` is far too common for that to be evidence.
        let v = vocab(&[("overall", 3), ("all", 90)]);
        assert_eq!(v.rejoin("over", "all"), None);
    }

    #[test]
    fn refuses_a_single_letter_tail() {
        let v = vocab(&[("often", 20)]);
        assert_eq!(v.rejoin("ofte", "n"), None);
    }

    #[test]
    fn refuses_across_punctuation() {
        let v = vocab(&[("network", 20)]);
        assert_eq!(v.rejoin("net.", "work"), None);
        assert_eq!(v.rejoin("net", "work,"), None);
    }

    #[test]
    fn refuses_a_capitalised_tail() {
        let v = vocab(&[("imagenet", 9)]);
        assert_eq!(v.rejoin("image", "Net"), None);
    }

    #[test]
    fn refuses_short_merges() {
        let v = vocab(&[("into", 50)]);
        assert_eq!(v.rejoin("in", "to"), None, "too short to be evidence");
    }

    #[test]
    fn builds_counts_from_lines() {
        use crate::ir::Rect;
        use crate::text::lines::Word;

        let bbox = Rect::from_corners(0.0, 0.0, 1.0, 1.0);
        let word = |t: &str| Word {
            bbox,
            text: t.to_owned(),
            start: 0,
            end: 1,
        };
        let line = Line {
            bbox,
            baseline: 0.0,
            size: 10.0,
            bold: false,
            italic: false,
            glyphs: Vec::new(),
            words: vec![
                word("edge"),
                word("Learning"),
                word("learning"),
                word("42"),
                word("rate"),
                word("edge"),
            ],
        };

        let v = Vocabulary::build(&[vec![line]]);
        assert_eq!(v.count("learning"), 2, "case is folded");
        assert_eq!(v.count("rate"), 1);
        assert_eq!(v.count("42"), 0, "numbers are not words");
        assert_eq!(
            v.count("edge"),
            0,
            "line-edge words could be hyphenation fragments"
        );
    }
}
