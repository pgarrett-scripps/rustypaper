//! Bibliographies: splitting them into entries and pulling fields out of each.
//!
//! Field extraction here is heuristic, and deliberately so. Reference strings follow hundreds of
//! house styles, and the accurate way to parse them is a sequence model trained on labelled
//! examples — which is what GROBID does, and what this project has ruled out. What heuristics do
//! recover reliably is the *structured* part: the year, the DOI, the arXiv identifier, the entry
//! label. Those are the fields anything downstream actually resolves against, and each is
//! pattern-bound rather than style-bound.
//!
//! Authors and title are reported when the shape is clear and left out when it is not. An absent
//! field is more useful than a wrong one.

use serde::{Deserialize, Serialize};

use crate::text::lines::Line;

/// The heading that starts a bibliography.
const HEADINGS: [&str; 4] = [
    "references",
    "bibliography",
    "works cited",
    "literature cited",
];

/// A first line indented left of the following ones by at least this many points marks a new
/// entry, in styles that use a hanging indent instead of a label.
const HANGING_INDENT: f32 = 4.0;

/// One bibliography entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// The printed label, such as `12` from `[12]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The entry as printed, with the label removed.
    pub raw: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arxiv: Option<String>,
}

/// Whether a heading starts the bibliography.
pub fn is_bibliography_heading(text: &str) -> bool {
    let cleaned: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic() || c.is_whitespace())
        .collect();
    let cleaned = cleaned.trim();
    HEADINGS.contains(&cleaned)
}

/// How many characters to strip if this block *opens* with a bibliography heading.
///
/// The heading is not always its own block. Where a template sets `References` at body size with
/// ordinary leading, paragraph assembly merges it into the first entry and the document reads
/// `ReferencesKevin Clark, ...` — which is how two of the four corpus papers behave, and why
/// looking only for a heading block found no bibliography in either.
pub fn opens_bibliography(text: &str) -> Option<usize> {
    let lower = text.to_lowercase();
    for heading in HEADINGS {
        if !lower.starts_with(heading) {
            continue;
        }
        let rest = text[heading.len()..].trim_start();
        // The next thing must look like the start of an entry, not a continuation of a word.
        let plausible = rest
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase() || c == '[');
        if plausible {
            return Some(text.len() - rest.len());
        }
    }
    None
}

/// Splits the lines of a bibliography into entries.
///
/// A bracketed label is the strongest signal and is tried first. Author-year styles have no
/// label, so the fallback is the hanging indent every such style uses to make entries scannable.
pub fn split(lines: &[&Line]) -> Vec<Vec<usize>> {
    if lines.is_empty() {
        return Vec::new();
    }

    let labelled: Vec<bool> = lines
        .iter()
        .map(|l| leading_label(&l.text()).is_some())
        .collect();
    if labelled.iter().filter(|l| **l).count() >= 2 {
        return group_by(&labelled);
    }

    // Hanging indent: an entry's first line starts left of its continuations.
    let base = lines.iter().map(|l| l.bbox.x0).fold(f32::MAX, f32::min);
    let starts: Vec<bool> = lines
        .iter()
        .map(|l| l.bbox.x0 <= base + HANGING_INDENT)
        .collect();
    if starts.iter().filter(|s| **s).count() >= 2 {
        return group_by(&starts);
    }

    vec![(0..lines.len()).collect()]
}

fn group_by(starts: &[bool]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, is_start) in starts.iter().enumerate() {
        if *is_start || groups.is_empty() {
            groups.push(vec![i]);
        } else {
            groups.last_mut().unwrap().push(i);
        }
    }
    groups
}

/// Splits a run of concatenated entries on their `[n]` labels.
///
/// Paragraph assembly gives a bibliography back as one block per column, because its lines are
/// evenly spaced and nothing about them says "new entry" geometrically. The labels do say it,
/// and they are unambiguous: `[` digits `]` occurs nowhere else in a reference list.
pub fn split_entries(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();

    let bracketed = label_positions(&chars, true);
    let numbered = label_positions(&chars, false);
    let starts = if bracketed.len() >= numbered.len() {
        bracketed
    } else {
        numbered
    };

    if starts.len() < 2 {
        let trimmed = text.trim();
        return if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![trimmed.to_owned()]
        };
    }

    let mut entries = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(chars.len());
        let entry: String = chars[start..end].iter().collect();
        let entry = entry.trim();
        if !entry.is_empty() {
            entries.push(entry.to_owned());
        }
    }
    entries
}

/// Finds the offsets where entry labels begin, in `[n]` or `n.` form.
///
/// Only *sequential* labels count: the first must be 1, and each one after it must be the
/// previous plus one. That is what makes bare `n.` numbering safe to split on at all — a
/// bibliography is full of numbers followed by full stops (`pp. 2852-2860 (2012)`), and the only
/// thing distinguishing an entry label from any of them is that the labels count up.
fn label_positions(chars: &[char], bracketed: bool) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut expected = 1u32;

    let mut i = 0;
    while i < chars.len() {
        let open = if bracketed { chars[i] == '[' } else { true };
        let at_boundary = i == 0 || chars[i - 1].is_whitespace();
        if !open || !at_boundary {
            i += 1;
            continue;
        }

        let digits_at = if bracketed { i + 1 } else { i };
        let mut j = digits_at;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if j == digits_at || j - digits_at > 3 {
            i += 1;
            continue;
        }

        let closes = if bracketed {
            chars.get(j) == Some(&']')
        } else {
            // `12.` followed by a space, so a decimal such as `1.5` cannot match.
            chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(|c| c.is_whitespace())
        };
        if !closes {
            i += 1;
            continue;
        }

        let value: u32 = chars[digits_at..j]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        if value == expected {
            starts.push(i);
            expected += 1;
            i = j + 1;
        } else {
            i += 1;
        }
    }

    starts
}

/// Parses one entry.
pub fn parse(text: &str) -> Reference {
    let (label, rest) = match leading_label(text) {
        Some((label, rest)) => (Some(label), rest.to_owned()),
        None => (None, text.to_owned()),
    };
    let raw = rest.trim().to_owned();

    Reference {
        label,
        year: find_year(&raw),
        doi: find_doi(&raw),
        arxiv: find_arxiv(&raw),
        authors: find_authors(&raw),
        title: find_title(&raw),
        raw,
    }
}

/// Strips a leading `[12]`, `12.` or `(12)` label.
fn leading_label(text: &str) -> Option<(String, &str)> {
    let trimmed = text.trim_start();

    if let Some(rest) = trimmed.strip_prefix('[') {
        let end = rest.find(']')?;
        let label = &rest[..end];
        if !label.is_empty() && label.len() <= 12 {
            return Some((label.to_owned(), &rest[end + 1..]));
        }
    }

    // `12.` but not `1992.`, which is a year.
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if (1..=3).contains(&digits.len()) {
        let rest = &trimmed[digits.len()..];
        if let Some(rest) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            if rest.starts_with(char::is_whitespace) {
                return Some((digits, rest));
            }
        }
    }

    None
}

/// The publication year: a plausible four-digit year, preferring the last one.
///
/// The last is preferred because a title can contain a year (`... since 1998 ...`) while the
/// publication year sits at the end in most styles.
fn find_year(text: &str) -> Option<u16> {
    let bytes: Vec<char> = text.chars().collect();
    let mut found = None;
    for i in 0..bytes.len().saturating_sub(3) {
        if !bytes[i].is_ascii_digit() {
            continue;
        }
        let window: String = bytes[i..i + 4].iter().collect();
        if !window.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // Must not be part of a longer number.
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
        let after_ok = i + 4 >= bytes.len() || !bytes[i + 4].is_ascii_digit();
        if !before_ok || !after_ok {
            continue;
        }
        if let Ok(year) = window.parse::<u16>() {
            if (1800..=2100).contains(&year) {
                found = Some(year);
            }
        }
    }
    found
}

/// A DOI: `10.` followed by a registrant code, a slash and a suffix.
fn find_doi(text: &str) -> Option<String> {
    let start = text.find("10.")?;
    let rest = &text[start..];
    let slash = rest.find('/')?;
    if slash < 5 || !rest[3..slash].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let end = rest[slash..]
        .find(|c: char| c.is_whitespace())
        .map(|i| slash + i)
        .unwrap_or(rest.len());
    let doi = rest[..end].trim_end_matches(['.', ',', ';']);
    (doi.len() > slash + 1).then(|| doi.to_owned())
}

/// An arXiv identifier in either the modern or the legacy form.
fn find_arxiv(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let start = lower.find("arxiv:")?;
    let rest = &text[start + 6..];
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
        .collect();
    let id = id.trim_end_matches('.');
    (!id.is_empty()).then(|| id.to_owned())
}

/// Author surnames, when the entry opens in a recognisable author list.
///
/// Only the clear shape is accepted: a run of comma-separated names before the first sentence
/// break. Where the style is not recognised, no authors are reported rather than a guess.
fn find_authors(text: &str) -> Vec<String> {
    let head = match author_boundary(text) {
        Some(i) if i < 200 => &text[..i],
        _ => return Vec::new(),
    };

    let names: Vec<String> = head
        .split([',', ';'])
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|part| {
            !part.is_empty()
                && part.len() <= 60
                && part.chars().next().is_some_and(char::is_uppercase)
                && part.chars().any(char::is_alphabetic)
        })
        .map(str::to_owned)
        .collect();

    // A single fragment is more likely a title than an author list.
    if names.len() < 2 {
        Vec::new()
    } else {
        names
    }
}

/// Where the author list ends.
///
/// Not simply the first `". "`. Author lists are full of initials — `Y. Bengio, P. Simard` — and
/// splitting at the first one leaves `Y` as the entire author list, which is why author
/// extraction found nothing on a paper with fifty well-formed references. A full stop ends the
/// list only when the word before it is more than one letter.
fn author_boundary(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let mut byte = 0;

    for (i, c) in chars.iter().enumerate() {
        if *c == '.' && chars.get(i + 1).is_some_and(|n| n.is_whitespace()) {
            // The token immediately before the stop.
            let mut start = i;
            while start > 0 && chars[start - 1].is_alphanumeric() {
                start -= 1;
            }
            if i - start > 1 {
                return Some(byte);
            }
        }
        byte += c.len_utf8();
    }
    None
}

/// The title: the first substantial sentence after the author list.
///
/// A sentence that is only a year is skipped. Author-year styles put the date between the
/// authors and the title — `Clark, Luong, Manning. 2019. What does BERT look at?` — and taking
/// the first sentence blindly yields `2019` as the title of every entry in such a paper.
fn find_title(text: &str) -> Option<String> {
    let mut cursor = author_boundary(text)? + 2;

    for _ in 0..3 {
        let rest = text.get(cursor..)?;
        let end = rest.find(". ").unwrap_or(rest.len());
        let candidate = rest[..end].trim().trim_end_matches('.');

        let is_date = candidate
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_whitespace() || matches!(c, '(' | ')' | ','));
        if !is_date && (8..300).contains(&candidate.len()) {
            return Some(candidate.to_owned());
        }
        if end >= rest.len() {
            return None;
        }
        cursor += end + 2;
    }
    None
}

/// Rewrites inline citations as Markdown links to the reference list.
///
/// Only numeric citations are linked. Author-year forms (`Devlin et al., 2019`) cannot be
/// resolved to an entry without matching names, which is the part heuristics get wrong.
pub fn link_citations(text: &str, known: &[String]) -> String {
    if known.is_empty() || !text.contains('[') {
        return text.to_owned();
    }

    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']').map(|i| open + i) else {
            break;
        };
        let inner = &rest[open + 1..close];
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();

        let all_known = !parts.is_empty()
            && parts
                .iter()
                .all(|p| !p.is_empty() && known.iter().any(|k| k == p));

        out.push_str(&rest[..open]);
        if all_known {
            out.push('[');
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("[{part}](#ref-{part})"));
            }
            out.push(']');
        } else {
            out.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_bibliography_headings() {
        for heading in ["References", "REFERENCES", "5 References", "Bibliography"] {
            assert!(is_bibliography_heading(heading), "{heading:?}");
        }
        for other in ["Related Work", "Reference implementation", "Conclusion"] {
            assert!(!is_bibliography_heading(other), "{other:?}");
        }
    }

    #[test]
    fn strips_bracketed_labels() {
        let parsed = parse("[12] K. He and X. Zhang. Deep residual learning. CVPR, 2016.");
        assert_eq!(parsed.label.as_deref(), Some("12"));
        assert!(parsed.raw.starts_with("K. He"));
    }

    #[test]
    fn strips_numbered_labels_without_confusing_years() {
        assert_eq!(
            parse("3. Some Author. A title. 2019.").label.as_deref(),
            Some("3")
        );
        // A leading year is not a label.
        assert_eq!(parse("1992. Not a label.").label, None);
    }

    #[test]
    fn finds_the_publication_year() {
        assert_eq!(parse("[1] A. B. A title. Venue, 2016.").year, Some(2016));
        // The later year wins: a title may mention one.
        assert_eq!(
            parse("[1] A. B. Trends since 1998. Venue, 2016.").year,
            Some(2016)
        );
        assert_eq!(parse("[1] A. B. No year here.").year, None);
        // Not a year: part of a longer number.
        assert_eq!(parse("[1] A. B. Model 20164 results.").year, None);
    }

    #[test]
    fn finds_dois() {
        let parsed = parse("[1] A. B. A title. Journal, 2016. doi:10.1145/3292500.3330701");
        assert_eq!(parsed.doi.as_deref(), Some("10.1145/3292500.3330701"));
        assert_eq!(parse("[1] A. B. Version 10.2 of the tool.").doi, None);
    }

    #[test]
    fn finds_arxiv_identifiers() {
        assert_eq!(
            parse("[1] A. B. A title. arXiv:1706.03762, 2017.")
                .arxiv
                .as_deref(),
            Some("1706.03762")
        );
        assert_eq!(
            parse("[1] A. B. A title. arXiv:cs/0501001.")
                .arxiv
                .as_deref(),
            Some("cs/0501001")
        );
    }

    #[test]
    fn finds_authors_and_title_when_the_shape_is_clear() {
        let parsed = parse(
            "[7] Kaiming He, Xiangyu Zhang, Shaoqing Ren. Deep residual learning for image \
             recognition. In CVPR, 2016.",
        );
        assert_eq!(parsed.authors.len(), 3);
        assert_eq!(parsed.authors[0], "Kaiming He");
        assert_eq!(
            parsed.title.as_deref(),
            Some("Deep residual learning for image recognition")
        );
    }

    /// An unrecognised shape yields no authors rather than a wrong guess.
    /// Initials must not be mistaken for the end of the author list.
    #[test]
    fn initials_do_not_end_the_author_list() {
        let parsed = parse(
            "[1] Y. Bengio, P. Simard, and P. Frasconi. Learning long-term dependencies with \
             gradient descent is difficult. IEEE Transactions, 1994.",
        );
        assert_eq!(parsed.authors, ["Y. Bengio", "P. Simard", "P. Frasconi"]);
        assert_eq!(
            parsed.title.as_deref(),
            Some("Learning long-term dependencies with gradient descent is difficult")
        );
        assert_eq!(parsed.year, Some(1994));
    }

    /// Author-year styles date the entry between the authors and the title.
    #[test]
    fn a_year_between_authors_and_title_is_skipped() {
        let parsed = parse(
            "Kevin Clark, Urvashi Khandelwal, Christopher D. Manning. 2019. What does BERT look \
             at? An analysis of attention. In BlackboxNLP.",
        );
        assert_eq!(parsed.year, Some(2019));
        assert_eq!(
            parsed.title.as_deref(),
            Some("What does BERT look at? An analysis of attention")
        );
    }

    #[test]
    fn a_merged_heading_is_recognised() {
        // What paragraph assembly produces when `References` is set at body size.
        assert_eq!(
            opens_bibliography("ReferencesKevin Clark, Minh-Thang"),
            Some(10)
        );
        assert_eq!(opens_bibliography("REFERENCES [1] A. Author"), Some(11));
        assert_eq!(opens_bibliography("Referenced work is discussed"), None);
        assert_eq!(opens_bibliography("Related Work"), None);
    }

    #[test]
    fn an_unusual_style_reports_no_authors() {
        let parsed = parse("[1] Anonymous submission under review");
        assert!(parsed.authors.is_empty());
        assert!(parsed.title.is_none());
    }

    #[test]
    fn numeric_citations_are_linked() {
        let known = vec!["1".to_string(), "12".to_string()];
        assert_eq!(
            link_citations("as shown in [12] and [1]", &known),
            "as shown in [[12](#ref-12)] and [[1](#ref-1)]"
        );
    }

    #[test]
    fn multiple_citations_in_one_bracket_are_linked_separately() {
        let known = vec!["1".to_string(), "2".to_string()];
        assert_eq!(
            link_citations("see [1, 2]", &known),
            "see [[1](#ref-1), [2](#ref-2)]"
        );
    }

    #[test]
    fn unknown_or_non_numeric_brackets_are_left_alone() {
        let known = vec!["1".to_string()];
        assert_eq!(link_citations("see [99]", &known), "see [99]");
        assert_eq!(link_citations("an array a[i]", &known), "an array a[i]");
        assert_eq!(
            link_citations("no brackets here", &known),
            "no brackets here"
        );
    }

    #[test]
    fn concatenated_entries_split_on_their_labels() {
        let text = "[1] A. Author. First title. Venue, 2016. [2] B. Author. Second title. 2017. \
                    [3] C. Author. Third title. 2018.";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].starts_with("[1]"));
        assert!(entries[1].starts_with("[2]"));
        assert!(entries[2].ends_with("2018."));
    }

    /// Springer and many journals number entries `1.` rather than `[1]`.
    #[test]
    fn numbered_entries_split_on_sequential_labels() {
        let text = "1. Ciresan, D.C., Schmidhuber, J.: Deep neural networks. In: NIPS. \
                    pp. 2852-2860 (2012) 2. Dosovitskiy, A.: Another title. (2015) \
                    3. Long, J.: A third. (2015)";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 3, "got {entries:?}");
        assert!(entries[0].starts_with("1. Ciresan"));
        assert!(entries[1].starts_with("2. Dosovitskiy"));
        // The page range and the year inside entry 1 must not have split it.
        assert!(entries[0].contains("2852-2860"));
    }

    /// Numbers that do not count up are page ranges and years, not labels.
    #[test]
    fn out_of_sequence_numbers_are_not_labels() {
        let text = "1. A. Author. A title. pp. 10. 1998. Later work. 7. Not an entry.";
        assert_eq!(split_entries(text).len(), 1);
    }

    #[test]
    fn a_bracket_mid_word_does_not_start_an_entry() {
        // A citation inside an entry's own text must not split it.
        let text = "[1] A. Author. Building on[2] earlier work. 2016.";
        assert_eq!(split_entries(text).len(), 1);
    }

    #[test]
    fn unlabelled_text_stays_whole() {
        assert_eq!(split_entries("no labels at all here").len(), 1);
        assert!(split_entries("   ").is_empty());
    }

    #[test]
    fn entries_split_on_labels() {
        // Three labelled first lines and two continuations.
        let flags = [true, false, true, true, false];
        assert_eq!(group_by(&flags), vec![vec![0, 1], vec![2], vec![3, 4]]);
    }
}
