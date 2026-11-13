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

/// The heading that starts a bibliography.
const HEADINGS: [&str; 4] = [
    "references",
    "bibliography",
    "works cited",
    "literature cited",
];

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
        // A digit counts: journals that number entries `1.` open the list with one, and
        // requiring a capital found no bibliography at all in papers set that way.
        let plausible = rest
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit() || c == '[');
        if plausible {
            return Some(text.len() - rest.len());
        }
    }
    None
}

/// Splits a run of concatenated entries into one string per entry.
///
/// Paragraph assembly gives a bibliography back as one block per column, because its lines are
/// evenly spaced and nothing about them says "new entry" geometrically. Two things in the text do
/// say it, and this tries them in order of how certain they are.
///
/// The labels are certain: `[` digits `]` occurs nowhere else in a reference list, and a counting
/// `n.` sequence occurs nowhere else either. Where a bibliography carries neither — every
/// author-year house style, which is most of the natural sciences — the author list at the head
/// of each entry says it instead; see [`author_starts`].
///
/// A sequence of labels always wins, even where it is shorter than what the author lists would
/// give. Inside a numbered list the author signal fires one entry late, just past each label,
/// which both strands the label as an entry of its own and merges the last two.
pub fn split_entries(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();

    let bracketed = label_positions(&chars, true);
    let numbered = label_positions(&chars, false);
    let mut starts = if bracketed.len() >= numbered.len() {
        bracketed
    } else {
        numbered
    };

    // One label is no sequence: there is nothing to have counted up, so believe the author lists.
    if starts.len() < 2 {
        starts = author_starts(&chars);
    }

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

/// The longest a *typical* entry may run for an unlabelled split to be believed, in characters.
///
/// Not a limit on any one entry — a book chapter with thirty authors runs to nine hundred
/// characters and is perfectly real. It is a limit on the median, which is what separates a
/// signal that fired on every entry from one that fired on three lucky places and left the rest
/// as blobs. Across the corpus the two cases do not overlap: bibliographies the signal segments
/// properly have a median entry of 75 to 215 characters, and the one it barely touches has a
/// median of 863.
const MEDIAN_ENTRY_LIMIT: usize = 400;

/// Characters a surname is spelt with, beyond the letters.
///
/// The reader reports a combining accent as a character of its own — `Ho¨chst`, `Arbel´aez`,
/// `Abra`moff` are all in the corpus — so a surname is not simply a run of alphabetic characters,
/// and treating it as one truncates the name and loses the entry.
const NAME_MARKS: [char; 8] = ['\'', '\u{2019}', '`', '\u{00b4}', '\u{00a8}', '-', '~', '^'];

/// Nobiliary particles: a surname may begin with one, and so therefore may an entry.
///
/// Lower case, and matched case-insensitively — the same name is printed `van Ginneken` in one
/// bibliography and `Von Klitzing` in another.
const PARTICLES: [&str; 12] = [
    "van", "von", "de", "den", "der", "di", "da", "del", "du", "la", "le", "ten",
];

/// Finds the offsets where entries begin, by the author list each one opens with.
///
/// The shape looked for is narrow on purpose: a sentence has to have ended, and a surname,
/// a comma and an initial have to follow it. `... Phys. Rev. Lett. 102, 216404. Alpichshev, Z.,`
/// is an entry boundary; nothing else in a reference list looks like that. In particular the
/// initials *inside* an author list do not, because the character before their full stop is a
/// lone capital — which is exactly what [`ends_a_sentence`] refuses.
///
/// Two guards keep a handful of chance matches from being reported as a segmentation. There must
/// be at least three pieces, and their median length must be entry-sized; a style this does not
/// fit — a bibliography giving first names in full, say — then leaves the text whole for the
/// caller to keep in one piece, which is the honest answer and not a cut through the middle of
/// somebody's title.
fn author_starts(chars: &[char]) -> Vec<usize> {
    let mut starts = vec![0usize];
    for i in 1..chars.len() {
        if chars[i - 1].is_whitespace()
            && ends_a_sentence(chars, i)
            && opens_an_author_list(chars, i)
        {
            starts.push(i);
        }
    }
    if starts.len() < 3 {
        return Vec::new();
    }

    let mut lengths: Vec<usize> = starts.windows(2).map(|w| w[1] - w[0]).collect();
    lengths.push(chars.len() - starts[starts.len() - 1]);
    lengths.sort_unstable();
    if lengths[lengths.len() / 2] > MEDIAN_ENTRY_LIMIT {
        return Vec::new();
    }
    starts
}

/// Whether what sits before `at` ended a sentence, rather than an author's initial.
///
/// The distinction carries the whole split. An author list is a field of full stops —
/// `Agergaard, S., C. Sondergaard, H. Li,` — and every one of them is followed by a capital
/// letter that starts a name. Only the stop that is *not* an initial's ends an entry.
fn ends_a_sentence(chars: &[char], at: usize) -> bool {
    let mut i = at;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i == 0 || chars[i - 1] != '.' {
        return false;
    }
    let dot = i - 1;
    // `A.` is an initial; `Phys.`, `2016.` and `York).` are ends.
    let initial = dot > 0
        && chars[dot - 1].is_alphabetic()
        && chars[dot - 1].is_uppercase()
        && (dot < 2 || !chars[dot - 2].is_alphanumeric());
    !initial
}

/// Whether an author list plausibly begins at `at`: an optional particle, a surname, an initial.
fn opens_an_author_list(chars: &[char], at: usize) -> bool {
    let mut i = at + particle_len(chars, at);
    if i > at {
        if chars.get(i) != Some(&' ') {
            return false;
        }
        i += 1;
    }

    let len = surname_len(chars, i);
    if len == 0 {
        return false;
    }
    i += len;

    if chars.get(i) != Some(&',') || chars.get(i + 1) != Some(&' ') {
        return false;
    }
    // A given name reduced to an initial. Requiring it is what keeps `In Thomson, D. L., Cooch,
    // E. G., editors,` — the editor clause of a book chapter, mid-entry — from opening one: the
    // full stop before `In` is a real sentence end, and only the shape of `In` itself says no.
    chars
        .get(i + 2)
        .is_some_and(|c| c.is_alphabetic() && c.is_uppercase())
        && chars.get(i + 3) == Some(&'.')
}

/// The length of the surname starting at `at`, or zero if there is none.
fn surname_len(chars: &[char], at: usize) -> usize {
    if !chars
        .get(at)
        .is_some_and(|c| c.is_alphabetic() && c.is_uppercase())
    {
        return 0;
    }
    let mut i = at;
    while chars
        .get(i)
        .is_some_and(|c| c.is_alphabetic() || NAME_MARKS.contains(c))
    {
        i += 1;
    }
    let len = i - at;
    // One letter is an initial, not a surname; thirty is past any of them.
    if (2..=30).contains(&len) {
        len
    } else {
        0
    }
}

/// The length of the nobiliary particle starting at `at`, or zero if the word is not one.
fn particle_len(chars: &[char], at: usize) -> usize {
    let mut i = at;
    while chars.get(i).is_some_and(|c| c.is_alphabetic()) {
        i += 1;
    }
    let word: String = chars[at..i].iter().collect::<String>().to_lowercase();
    if PARTICLES.contains(&word.as_str()) {
        i - at
    } else {
        0
    }
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
/// publication year sits at the end in most styles. Two runs of four digits are ruled out first,
/// because in the styles that put the year early the *last* number in the entry is one of them
/// and the year is not — see [`in_a_page_range`] and [`arxiv_span`].
fn find_year(text: &str) -> Option<u16> {
    let chars: Vec<char> = text.chars().collect();
    let identifier = arxiv_span(&chars);
    let mut found = None;
    for i in 0..chars.len().saturating_sub(3) {
        if !chars[i].is_ascii_digit() {
            continue;
        }
        let window: String = chars[i..i + 4].iter().collect();
        if !window.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // Must not be part of a longer number.
        let before_ok = i == 0 || !chars[i - 1].is_ascii_digit();
        let after_ok = i + 4 >= chars.len() || !chars[i + 4].is_ascii_digit();
        if !before_ok || !after_ok {
            continue;
        }
        if identifier.as_ref().is_some_and(|span| span.contains(&i)) {
            continue;
        }
        if in_a_page_range(&chars, i) {
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

/// Whether the four digits at `at` are one end of a page range.
///
/// `Documenta Math. 26 (2021), 1851–1869.` ends in two numbers that both fall inside the band a
/// year occupies, and the last of them beat the real year. Either end is ruled out, not just the
/// far one: `IEEE Trans Pattern Anal Mach Intell 35, 1798–1828` opens with a page that reads as a
/// perfectly ordinary year.
fn in_a_page_range(chars: &[char], at: usize) -> bool {
    const DASHES: [char; 6] = [
        '-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}',
    ];
    let after_dash = at >= 2 && DASHES.contains(&chars[at - 1]) && chars[at - 2].is_ascii_digit();
    let before_dash = chars.get(at + 4).is_some_and(|c| DASHES.contains(c))
        && chars.get(at + 5).is_some_and(char::is_ascii_digit);
    after_dash || before_dash
}

/// A DOI: `10.` followed by a registrant code, a slash and a suffix.
///
/// Every `10.` is tried, not just the first: an entry that carries a volume number reads
/// `vol. 10. doi:10.1145/3292500`, and giving up on the first candidate loses the DOI.
fn find_doi(text: &str) -> Option<String> {
    text.match_indices("10.")
        .find_map(|(start, _)| doi_at(&text[start..]))
}

/// The DOI beginning at the start of `rest`, if there is one.
fn doi_at(rest: &str) -> Option<String> {
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
    let chars: Vec<char> = text.chars().collect();
    let id: String = chars[arxiv_span(&chars)?].iter().collect();
    let id = id.trim_end_matches('.');
    (!id.is_empty()).then(|| id.to_owned())
}

/// Where the arXiv identifier sits, the `arXiv:` marker excluded.
///
/// Wanted as a span and not just a string because a year search has to step over it. A modern
/// identifier is four digits, a dot and four or five more — `arXiv:0812.2078` offers `0812` and
/// `2078`, and the second of them was being reported as the publication year of a 2008 paper.
fn arxiv_span(chars: &[char]) -> Option<std::ops::Range<usize>> {
    const MARK: [char; 6] = ['a', 'r', 'x', 'i', 'v', ':'];
    let start = (0..chars.len().saturating_sub(MARK.len() - 1)).find(|&i| {
        chars[i..i + MARK.len()]
            .iter()
            .zip(MARK)
            .all(|(c, m)| c.to_ascii_lowercase() == m)
    })? + MARK.len();

    let mut end = start;
    while chars
        .get(end)
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
    {
        end += 1;
    }
    (end > start).then_some(start..end)
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

    // The date the list is bounded by is not one of the names in it.
    let head = head.trim_end();
    let head = head
        .strip_suffix(')')
        .and_then(|rest| rest.rfind('(').map(|open| &rest[..open]))
        .unwrap_or(head)
        .trim_end();

    let names: Vec<String> = head
        .split([',', ';'])
        .flat_map(|part| part.split(" and "))
        .flat_map(|part| part.split(" & "))
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
///
/// A parenthesised date ends it too, and has to be asked about first. `Bengio, Y., Mesnil, G.,
/// and Rifai, S. (2013a). Better mixing via deep representations. In ICML'13.` offers no
/// qualifying stop until the one *after* the title, because the stop closing `(2013a)` follows a
/// bracket rather than a word — so the author list was taken to run through the title, and the
/// venue was reported as the title of every entry in the style.
///
/// Nature's style defeats both rules: `Conti, E. & Izaurralde, E. Title. Nature 123, 45–67
/// (2016).` dates the entry at the very end and ends every author in an initial, so the first
/// qualifying stop is the one after the title and the title was swallowed into the author list.
/// The `&` before the final author is the shape to key on — see [`nature_boundary`]. Whichever
/// rule fires earlier wins, because every rule here marks a point the author list cannot
/// continue past.
fn author_boundary(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let at = match (generic_boundary(&chars), nature_boundary(&chars)) {
        (Some(g), Some(n)) => g.min(n),
        (g, n) => g.or(n)?,
    };
    Some(chars[..at].iter().map(|c| c.len_utf8()).sum())
}

/// The first stop after a word of more than one letter, or after a parenthesised date.
/// Returns a character index into `chars`.
fn generic_boundary(chars: &[char]) -> Option<usize> {
    for (i, c) in chars.iter().enumerate() {
        if *c == '.' && chars.get(i + 1).is_some_and(|n| n.is_whitespace()) {
            if i > 0 && chars[i - 1] == ')' && ends_in_a_date(&chars[..i - 1]) {
                return Some(i);
            }
            // The token immediately before the stop.
            let mut start = i;
            while start > 0 && chars[start - 1].is_alphanumeric() {
                start -= 1;
            }
            if i - start > 1 {
                return Some(i);
            }
        }
    }
    None
}

/// Where a Nature-style author list ends: after the last initial of the name that follows the
/// final `&`. Returns a character index into `chars`.
///
/// The shape asked for is `& Surname, I.` — a comma must come between the ampersand and the
/// initial, which is what separates Nature's surname-first names from an initials-first list
/// like `Y. Bengio & P. Simard`, where the stop after `P` is the middle of a name and the
/// generic rule already ends the list correctly. Consecutive initials (`D. I.`) extend the run.
/// Where the shape is not met, no boundary is reported rather than a guess — an `&` in a venue
/// name finds no initial within reach and settles nothing.
fn nature_boundary(chars: &[char]) -> Option<usize> {
    // The final ampersand of the author region; one later in the entry is a venue's.
    let amp = (0..chars.len().min(200)).rev().find(|&i| {
        chars[i] == '&'
            && i > 0
            && chars[i - 1].is_whitespace()
            && chars.get(i + 1).is_some_and(|c| c.is_whitespace())
    })?;

    let mut comma = false;
    let mut boundary = None;
    let mut i = amp + 1;
    while i < chars.len() && i < amp + 60 {
        if chars[i] == ',' {
            comma = true;
        }
        if chars[i] == '.' && chars.get(i + 1).is_some_and(|n| n.is_whitespace()) {
            let mut start = i;
            while start > 0 && chars[start - 1].is_alphanumeric() {
                start -= 1;
            }
            if i - start == 1 && chars[start].is_uppercase() && comma {
                boundary = Some(i);
                // Another initial follows — the `I.` of `D. I.` — so the list runs on.
                let mut j = i + 1;
                while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                    j += 1;
                }
                if chars.get(j).is_some_and(|c| c.is_uppercase()) && chars.get(j + 1) == Some(&'.')
                {
                    i = j + 1;
                    continue;
                }
                break;
            }
            if i - start > 1 {
                // A full word ended before any initial did: not the Nature shape.
                break;
            }
        }
        i += 1;
    }
    boundary
}

/// Whether `head` ends in the body of a parenthesised date, its closing bracket already removed.
///
/// `(1975)` and `(2013a)` both count — a style that dates its entries this way disambiguates two
/// papers of one year with a letter.
fn ends_in_a_date(head: &[char]) -> bool {
    let Some(open) = head.iter().rposition(|c| *c == '(') else {
        return false;
    };
    let inner = &head[open + 1..];
    let digits = inner.iter().take_while(|c| c.is_ascii_digit()).count();
    digits == 4 && inner.len() <= 5 && inner[digits..].iter().all(char::is_ascii_alphabetic)
}

/// The title: the first substantial sentence after the author list.
///
/// A sentence that is only a year is skipped. Author-year styles put the date between the
/// authors and the title — `Clark, Luong, Manning. 2019. What does BERT look at?` — and taking
/// the first sentence blindly yields `2019` as the title of every entry in such a paper.
///
/// A sentence that does not read as prose is skipped too; see [`reads_as_prose`].
fn find_title(text: &str) -> Option<String> {
    let mut cursor = author_boundary(text)? + 2;

    for _ in 0..3 {
        let rest = text.get(cursor..)?;
        let end = rest.find(". ").unwrap_or(rest.len());
        let candidate = rest[..end].trim().trim_end_matches('.');

        if reads_as_prose(candidate) && (8..300).contains(&candidate.len()) {
            return Some(candidate.to_owned());
        }
        if end >= rest.len() {
            return None;
        }
        cursor += end + 2;
    }
    None
}

/// Whether a candidate title is a sentence of prose rather than the citation tail after one.
///
/// Some styles print no title at all — `Gmitra, M., and J. Fabian, 2009, Phys. Rev. B 80,
/// 235431.` is a whole entry — and others print it before the date rather than after. In both,
/// the first sentence the search reaches is a volume and a page: `B 80, 235431`, `IJCV,
/// 88(2):303–338`, `Neural Computation, 23(8), 2053–2073`. What separates them from a title is
/// not their length but what they are made of. A title is mostly letters; a citation tail is
/// mostly digits and punctuation, and reporting one as a title is exactly the confident wrong
/// answer this module would rather not give.
fn reads_as_prose(text: &str) -> bool {
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    letters * 4 > text.chars().count() * 3
}

/// Rewrites inline citations as Markdown links to the reference list.
///
/// Only numeric citations are linked. Author-year forms (`Devlin et al., 2019`) cannot be
/// resolved to an entry without matching names, which is the part heuristics get wrong.
///
/// Two printed forms are recognised. Bracketed runs — `[3]`, `[1, 2]` — are linked when every
/// part is a known label. Superscript runs — the `3` of `fish³`, including lists and ranges
/// (`2,3`, `4-6`) — are linked on the same condition, taking the block's `sup` spans as the
/// evidence that the digits are a citation marker rather than body text. `spans` are character
/// offsets into `text` (see [`crate::doc::Span`]); the returned spans are the same annotations
/// remapped onto the rewritten text, with a linked superscript run widened to cover its own link
/// markup, and any span the rewrite cut through dropped rather than guessed at.
///
/// `link_sups` is the caller's judgment on whether a numeric superscript here can be a citation
/// at all. In the front matter it cannot — the same digits are affiliation markers on author
/// names — so the pipeline passes `false` there and the spans pass through unlinked. Bracketed
/// citations are unaffected: `[3]` does not share a shape with anything in a byline.
pub fn link_citations(
    text: &str,
    spans: &[crate::doc::Span],
    known: &[String],
    link_sups: bool,
) -> (String, Vec<crate::doc::Span>) {
    use crate::doc::{Span, SpanKind};

    if known.is_empty() {
        return (text.to_owned(), spans.to_vec());
    }
    let chars: Vec<char> = text.chars().collect();

    // Every rewrite is collected as an edit over a character range first and applied in one
    // pass, so span offsets can be remapped exactly instead of re-derived from the new text.
    struct Edit {
        start: usize,
        end: usize,
        replacement: String,
        /// Index into `spans` when the edit rewrites that span's own text.
        owner: Option<usize>,
    }
    let mut edits: Vec<Edit> = Vec::new();

    // Bracketed citations.
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '[' {
            i += 1;
            continue;
        }
        let Some(close) = chars[i + 1..]
            .iter()
            .position(|&c| c == ']')
            .map(|n| i + 1 + n)
        else {
            break;
        };
        let inner: String = chars[i + 1..close].iter().collect();
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        let all_known = !parts.is_empty()
            && parts
                .iter()
                .all(|p| !p.is_empty() && known.iter().any(|k| k == p));
        if all_known {
            let mut replacement = String::from("[");
            for (n, part) in parts.iter().enumerate() {
                if n > 0 {
                    replacement.push_str(", ");
                }
                replacement.push_str(&format!("[{part}](#ref-{part})"));
            }
            replacement.push(']');
            edits.push(Edit {
                start: i,
                end: close + 1,
                replacement,
                owner: None,
            });
        }
        i = close + 1;
    }

    // Superscript citations.
    for (index, span) in spans.iter().enumerate() {
        if !link_sups || span.kind != SpanKind::Sup {
            continue;
        }
        if let Some(replacement) = sup_citation(&chars, span.start, span.end, known) {
            edits.push(Edit {
                start: span.start,
                end: span.end,
                replacement,
                owner: Some(index),
            });
        }
    }

    // Left to right; where a superscript run sits inside a bracketed one, the bracket wins.
    edits.sort_by_key(|e| (e.start, e.owner.is_some()));
    let mut kept: Vec<Edit> = Vec::new();
    for edit in edits {
        if kept.last().is_none_or(|last| last.end <= edit.start) {
            kept.push(edit);
        }
    }

    // Apply, recording where each edit's replacement landed.
    let mut out = String::with_capacity(text.len());
    let mut out_chars = 0usize;
    let mut cursor = 0usize;
    // (old start, old end, new start, replacement length in chars, owner)
    let mut placed: Vec<(usize, usize, usize, usize, Option<usize>)> = Vec::new();
    for edit in &kept {
        out.extend(&chars[cursor..edit.start]);
        out_chars += edit.start - cursor;
        let length = edit.replacement.chars().count();
        placed.push((edit.start, edit.end, out_chars, length, edit.owner));
        out.push_str(&edit.replacement);
        out_chars += length;
        cursor = edit.end;
    }
    out.extend(&chars[cursor..]);

    let mut remapped: Vec<Span> = Vec::new();
    'spans: for (index, span) in spans.iter().enumerate() {
        if let Some(&(_, _, at, length, _)) = placed.iter().find(|p| p.4 == Some(index)) {
            remapped.push(Span {
                start: at,
                end: at + length,
                kind: span.kind,
            });
            continue;
        }
        let mut shift = 0isize;
        for &(start, end, _, length, _) in &placed {
            if end <= span.start {
                shift += length as isize - (end - start) as isize;
            } else if start < span.end {
                // The rewrite cut through this span; its offsets describe nothing now.
                continue 'spans;
            }
        }
        remapped.push(Span {
            start: (span.start as isize + shift) as usize,
            end: (span.end as isize + shift) as usize,
            kind: span.kind,
        });
    }
    remapped.sort_by_key(|s| s.start);

    (out, remapped)
}

/// The link markup for a superscript run that reads as a numeric citation, or `None`.
///
/// A run qualifies when it is digits joined by commas or range dashes (`3`, `2,3`, `4-6`),
/// every label it names — a range's interior included — is a known reference label, and the
/// character before it reads like a citation site. The last condition is what keeps exponents
/// and units unlinked: `x²` and `cm²` are superscript digits too, and `2` is usually a known
/// label, but both follow a short token where a citation marker follows a word or closing
/// punctuation.
fn sup_citation(chars: &[char], start: usize, end: usize, known: &[String]) -> Option<String> {
    if start >= end || end > chars.len() || !sup_follows_citation_site(chars, start) {
        return None;
    }

    // Tokenise into numbers and the separators between them.
    #[derive(PartialEq, Clone, Copy)]
    enum State {
        Start,
        Number,
        AfterNumber,
        Separator,
    }
    let mut state = State::Start;
    let mut numbers: Vec<String> = Vec::new();
    let mut separators: Vec<char> = Vec::new();
    let mut current = String::new();
    for &c in &chars[start..end] {
        match c {
            '0'..='9' => match state {
                State::Start | State::Separator => {
                    current.push(c);
                    state = State::Number;
                }
                State::Number => current.push(c),
                // Two numbers with nothing between them is not a citation list.
                State::AfterNumber => return None,
            },
            ',' | '-' | '\u{2013}' | '\u{2014}' => match state {
                State::Number | State::AfterNumber => {
                    numbers.push(std::mem::take(&mut current));
                    separators.push(c);
                    state = State::Separator;
                }
                _ => return None,
            },
            ' ' => {
                if state == State::Number {
                    state = State::AfterNumber;
                }
            }
            _ => return None,
        }
    }
    if !matches!(state, State::Number | State::AfterNumber) {
        return None;
    }
    numbers.push(current);

    if numbers.len() > MAX_SUP_CITATION_LIST || numbers.iter().any(|n| n.len() > 3) {
        return None;
    }
    for number in &numbers {
        if !known.iter().any(|k| k == number) {
            return None;
        }
    }
    // A range cites its interior too, so the interior labels must exist for the run to be one.
    for (i, separator) in separators.iter().enumerate() {
        if *separator == ',' {
            continue;
        }
        let a: u32 = numbers[i].parse().ok()?;
        let b: u32 = numbers[i + 1].parse().ok()?;
        if a >= b || b - a > MAX_SUP_CITATION_RANGE {
            return None;
        }
        for v in a + 1..b {
            if !known.iter().any(|k| k == &v.to_string()) {
                return None;
            }
        }
    }

    let mut out = String::new();
    for (i, number) in numbers.iter().enumerate() {
        if i > 0 {
            match separators[i - 1] {
                ',' => out.push_str(", "),
                dash => out.push(dash),
            }
        }
        out.push_str(&format!("[{number}](#ref-{number})"));
    }
    Some(out)
}

/// Longest comma list a superscript citation run may carry.
const MAX_SUP_CITATION_LIST: usize = 20;

/// Widest range (`4-6`) a superscript citation run may span.
const MAX_SUP_CITATION_RANGE: u32 = 50;

/// Whether the character before a superscript run reads like a citation site.
///
/// Citation markers follow a word of prose or its closing punctuation. Exponents follow a digit
/// (`10⁶`) or a one- or two-letter token (`x²`, `cm²`), and a run opening a block follows
/// nothing at all.
fn sup_follows_citation_site(chars: &[char], start: usize) -> bool {
    let Some(i) = start.checked_sub(1) else {
        return false;
    };
    let previous = chars[i];
    if matches!(
        previous,
        '.' | ',' | ';' | ':' | ')' | ']' | '"' | '\'' | '\u{201D}' | '\u{2019}'
    ) {
        return true;
    }
    if !previous.is_alphabetic() {
        return false;
    }
    let mut length = 0usize;
    let mut j = start;
    while j > 0 && chars[j - 1].is_alphabetic() {
        length += 1;
        if length >= 3 {
            return true;
        }
        j -= 1;
    }
    false
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

    /// Half of an arXiv identifier is four digits in the same band as a year.
    ///
    /// Invisible while a whole bibliography was one entry — some later entry always carried a
    /// real year — and unmissable once each entry is parsed on its own.
    #[test]
    fn an_arxiv_identifier_is_not_a_year() {
        let parsed = parse("Wray, L. and M. Z. Hasan, 2008, Nat. Phys. 5, 398, arXiv:0812.2078.");
        assert_eq!(parsed.year, Some(2008));
        assert_eq!(parsed.arxiv.as_deref(), Some("0812.2078"));
        // The leading half is no better: `arXiv:1808.04444` would give 1808.
        assert_eq!(
            parse("Rami Al-Rfou. 2018. A title. arXiv preprint arXiv:1808.04444.").year,
            Some(2018)
        );
    }

    /// Page numbers run into the thousands, and a page range puts two of them side by side.
    #[test]
    fn a_page_range_is_not_a_year() {
        assert_eq!(
            parse("N. Daans, A title, Documenta Math. 26 (2021), 1851–1869.").year,
            Some(2021)
        );
        // The near end of the range is as wrong as the far one.
        assert_eq!(
            parse("Bengio, Y., 2013. Representation learning. IEEE Trans 35, 1798–1828.").year,
            Some(2013)
        );
        // A hyphen serves as the dash in plenty of templates.
        assert_eq!(
            parse("A. Author. A title. Journal 6, 1817-1853, 2005.").year,
            Some(2005)
        );
    }

    #[test]
    fn finds_dois() {
        let parsed = parse("[1] A. B. A title. Journal, 2016. doi:10.1145/3292500.3330701");
        assert_eq!(parsed.doi.as_deref(), Some("10.1145/3292500.3330701"));
        assert_eq!(parse("[1] A. B. Version 10.2 of the tool.").doi, None);
    }

    /// An earlier `10.` that is not a DOI must not hide the one that is.
    #[test]
    fn a_doi_after_a_volume_number_is_still_found() {
        let parsed = parse("[1] A. B. A title. Journal, vol. 10. doi:10.1145/3292500.3330701");
        assert_eq!(parsed.doi.as_deref(), Some("10.1145/3292500.3330701"));
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

    /// A parenthesised date ends the author list, so the title is the title and not the venue.
    #[test]
    fn a_parenthesised_date_ends_the_author_list() {
        let parsed = parse(
            "Bengio, Y., Mesnil, G., Dauphin, Y., and Rifai, S. (2013a). Better mixing via deep \
             representations. In ICML.",
        );
        assert_eq!(
            parsed.title.as_deref(),
            Some("Better mixing via deep representations")
        );
        assert_eq!(parsed.year, Some(2013));
        // The date is not one of the names it bounds.
        assert!(
            parsed.authors.iter().all(|a| !a.contains("2013")),
            "{:?}",
            parsed.authors
        );
        // A single author is bounded the same way.
        assert_eq!(
            parse("Bengio, Y. (2009). Learning deep architectures for AI. Now Publishers.")
                .title
                .as_deref(),
            Some("Learning deep architectures for AI")
        );
    }

    /// Where a style prints no title, the volume and page that follow are not one.
    ///
    /// A whole Rev. Mod. Phys. entry is `Gmitra, M., and J. Fabian, 2009, Phys. Rev. B 80,
    /// 235431.`, and the first sentence after the author list is `B 80, 235431`.
    #[test]
    fn a_volume_and_page_are_not_a_title() {
        assert_eq!(
            parse("Gmitra, M., C. Ertler, and J. Fabian, 2009, Phys. Rev. B 80, 235431.").title,
            None
        );
        assert_eq!(
            parse("Everingham, M., and Zisserman, A. Some venue. IJCV, 88(2):303-338.").title,
            None
        );
        // A short title is still a title: what disqualifies one is what it is made of.
        assert_eq!(
            parse("Goodfellow, I. J., and Bengio, Y. (2013a). Maxout networks. In ICML.")
                .title
                .as_deref(),
            Some("Maxout networks")
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

    /// Nature's style: surname-first names that all end in an initial, the date at the end.
    /// The `&` before the final author marks where the list can end — see [`nature_boundary`].
    #[test]
    fn a_nature_style_entry_yields_its_title() {
        let parsed = parse(
            "1. Conti, E. & Izaurralde, E. Nonsense-mediated mRNA decay: molecular insights \
             and mechanistic variations. Curr. Opin. Cell Biol. 17, 316-325 (2005).",
        );
        assert_eq!(
            parsed.title.as_deref(),
            Some("Nonsense-mediated mRNA decay: molecular insights and mechanistic variations")
        );
        assert!(
            parsed.authors.contains(&"Conti".to_owned()),
            "{:?}",
            parsed.authors
        );
        assert!(parsed.authors.contains(&"Izaurralde".to_owned()));
        assert_eq!(parsed.year, Some(2005));
    }

    /// The final author's run of initials — `D. I.` — is part of the list, not the title.
    #[test]
    fn a_nature_style_author_with_two_initials_is_bounded_after_both() {
        let parsed = parse(
            "3. Kourou, K., Exarchos, T. P. & Fotiadis, D. I. Machine learning applications \
             in cancer prognosis and prediction. Comput. Struct. Biotechnol. J. 13, 8-17 (2015).",
        );
        assert_eq!(
            parsed.title.as_deref(),
            Some("Machine learning applications in cancer prognosis and prediction")
        );
        assert!(
            parsed.authors.contains(&"Fotiadis".to_owned()),
            "{:?}",
            parsed.authors
        );
    }

    /// An `&` with no surname-first name after it settles nothing: an initials-first list is
    /// still ended by the generic rule, and an `&` in a venue name is no boundary at all.
    #[test]
    fn an_ampersand_outside_the_nature_shape_moves_no_boundary() {
        let parsed = parse(
            "[2] Y. Bengio & P. Simard. Learning long-term dependencies with gradient \
             descent is difficult. IEEE Transactions, 1994.",
        );
        assert_eq!(
            parsed.title.as_deref(),
            Some("Learning long-term dependencies with gradient descent is difficult")
        );
        assert_eq!(parsed.authors, ["Y. Bengio", "P. Simard"]);

        let venue = parse("Smith, John. A study of things. Johnson & Johnson Reports 5, 3 (2020).");
        assert_eq!(venue.title.as_deref(), Some("A study of things"));
    }

    #[test]
    fn a_merged_heading_is_recognised() {
        // What paragraph assembly produces when `References` is set at body size.
        assert_eq!(
            opens_bibliography("ReferencesKevin Clark, Minh-Thang"),
            Some(10)
        );
        assert_eq!(opens_bibliography("REFERENCES [1] A. Author"), Some(11));
        // A numbered list opens with a digit, not a capital.
        assert_eq!(
            opens_bibliography("References 1. Lahiry P, et al."),
            Some(11)
        );
        assert_eq!(opens_bibliography("Referenced work is discussed"), None);
        assert_eq!(opens_bibliography("Related Work"), None);
    }

    #[test]
    fn an_unusual_style_reports_no_authors() {
        let parsed = parse("[1] Anonymous submission under review");
        assert!(parsed.authors.is_empty());
        assert!(parsed.title.is_none());
    }

    fn labels(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|s| (*s).to_string()).collect()
    }

    fn sup(start: usize, end: usize) -> crate::doc::Span {
        crate::doc::Span {
            start,
            end,
            kind: crate::doc::SpanKind::Sup,
        }
    }

    #[test]
    fn numeric_citations_are_linked() {
        let known = labels(&["1", "12"]);
        assert_eq!(
            link_citations("as shown in [12] and [1]", &[], &known, true).0,
            "as shown in [[12](#ref-12)] and [[1](#ref-1)]"
        );
    }

    #[test]
    fn multiple_citations_in_one_bracket_are_linked_separately() {
        let known = labels(&["1", "2"]);
        assert_eq!(
            link_citations("see [1, 2]", &[], &known, true).0,
            "see [[1](#ref-1), [2](#ref-2)]"
        );
    }

    #[test]
    fn unknown_or_non_numeric_brackets_are_left_alone() {
        let known = labels(&["1"]);
        assert_eq!(link_citations("see [99]", &[], &known, true).0, "see [99]");
        assert_eq!(
            link_citations("an array a[i]", &[], &known, true).0,
            "an array a[i]"
        );
        assert_eq!(
            link_citations("no brackets here", &[], &known, true).0,
            "no brackets here"
        );
    }

    #[test]
    fn a_superscript_citation_is_linked_and_its_span_widened() {
        let known = labels(&["3"]);
        // "fish3." with the 3 superscript.
        let (text, spans) = link_citations("fish3.", &[sup(4, 5)], &known, true);
        assert_eq!(text, "fish[3](#ref-3).");
        assert_eq!(spans, vec![sup(4, 15)]);
    }

    #[test]
    fn superscript_lists_and_ranges_are_linked() {
        let known = labels(&["2", "3", "4", "5", "6"]);
        let (text, spans) = link_citations(
            "shown2,3 and done4-6 here",
            &[sup(5, 8), sup(17, 20)],
            &known,
            true,
        );
        assert_eq!(
            text,
            "shown[2](#ref-2), [3](#ref-3) and done[4](#ref-4)-[6](#ref-6) here"
        );
        assert_eq!(spans, vec![sup(5, 29), sup(38, 61)]);
    }

    #[test]
    fn a_range_with_a_missing_interior_label_is_not_linked() {
        // A bibliography with [4] and [6] but no [5] cannot be cited as "4-6".
        let known = labels(&["4", "6"]);
        let (text, spans) = link_citations("done4-6 here", &[sup(4, 7)], &known, true);
        assert_eq!(text, "done4-6 here");
        assert_eq!(spans, vec![sup(4, 7)]);
    }

    #[test]
    fn exponents_and_units_are_not_linked() {
        let known = labels(&["2", "3", "6"]);
        // x² is a one-letter token, cm³ a two-letter one, 10⁶ follows a digit: all exponents.
        for (text, span) in [
            ("area x2 here", sup(6, 7)),
            ("a cm3 flask", sup(4, 5)),
            ("some 106 cells", sup(7, 8)),
        ] {
            let (out, spans) = link_citations(text, &[span], &known, true);
            assert_eq!(out, text);
            assert_eq!(spans, vec![span]);
        }
    }

    #[test]
    fn superscript_offsets_count_characters_not_bytes() {
        let known = labels(&["7"]);
        // "Müller" is six characters and seven bytes; the span is at character offsets.
        let (text, spans) = link_citations("Müller7 agrees", &[sup(6, 7)], &known, true);
        assert_eq!(text, "Müller[7](#ref-7) agrees");
        assert_eq!(spans, vec![sup(6, 17)]);
    }

    #[test]
    fn spans_after_a_bracket_rewrite_are_shifted() {
        let known = labels(&["1"]);
        // A subscript after a linked bracket must move with the text.
        let sub = crate::doc::Span {
            start: 9,
            end: 10,
            kind: crate::doc::SpanKind::Sub,
        };
        let (text, spans) = link_citations("see [1] H2O", &[sub], &known, true);
        assert_eq!(text, "see [[1](#ref-1)] H2O");
        assert_eq!(
            spans,
            vec![crate::doc::Span {
                start: 19,
                end: 20,
                kind: crate::doc::SpanKind::Sub,
            }]
        );
    }

    #[test]
    fn a_byline_with_affiliation_superscripts_is_left_unlinked() {
        // Front matter: the pipeline passes link_sups = false there. The affiliation markers
        // have exactly the shape of citations, so only the flag keeps them unlinked — and the
        // spans must survive, because the superscripts are still real.
        let known = labels(&["1", "2"]);
        let byline = "Abdul-rizaq Hamoud1, Khaled Alganem2, Ali Imami1";
        let spans = vec![sup(18, 19), sup(35, 36), sup(47, 48)];
        let (text, out) = link_citations(byline, &spans, &known, false);
        assert_eq!(text, byline);
        assert_eq!(out, spans);
    }

    #[test]
    fn brackets_are_still_linked_when_superscripts_are_not() {
        let known = labels(&["1"]);
        let (text, _) = link_citations("see [1] here3", &[sup(12, 13)], &known, false);
        assert_eq!(text, "see [[1](#ref-1)] here3");
    }

    #[test]
    fn a_corresponding_author_marker_with_an_asterisk_never_links() {
        // "Richa Pathak1*" — the starred affiliation marker is not a citation list even in
        // body text, because '*' is not a digit or separator.
        let known = labels(&["1"]);
        let byline = "Richa Pathak1*";
        let spans = vec![sup(12, 14)];
        let (text, out) = link_citations(byline, &spans, &known, true);
        assert_eq!(text, byline);
        assert_eq!(out, spans);
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

    /// REVTeX's author-year bibliography carries no labels at all: `topological.pdf` has 368
    /// entries and not one `[n]`, and arrived as six column-sized blobs because of it.
    #[test]
    fn revtex_author_year_entries_split_on_their_author_lists() {
        let text = "Agergaard, S., C. Sondergaard, H. Li, M. B. Nielsen, and Ph. Hofmann, 2001, \
                    New J. Phys. 3, 15. Akhmerov, A. R., J. Nilsson, and C. W. J. Beenakker, \
                    2009, Phys. Rev. Lett. 102, 216404. Anderson, P. W., 1958, Phys. Rev. 109, \
                    1492. Ast, C. R., 2001, Phys. Rev. Lett. 87, 177602.";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 4, "got {entries:?}");
        assert!(entries[0].starts_with("Agergaard, S."));
        assert!(entries[1].starts_with("Akhmerov, A. R."));
        assert!(entries[3].ends_with("177602."));
        // The initials inside the first entry's author list are not entry boundaries.
        assert!(entries[0].contains("H. Li"), "{:?}", entries[0]);
        // Nor is an abbreviated journal name that happens to precede a capital.
        assert!(entries[1].contains("Phys. Rev. Lett."));
    }

    /// Elsevier's style runs the author list on commas and dates the entry with a bare year.
    #[test]
    fn elsevier_author_lists_split_without_labels() {
        let text = "Abadi, M., Agarwal, A., Corrado, G. S., Zheng, X., 2016. Tensorflow: \
                    large-scale machine learning. arXiv:1603.04467. Akram, S. U., Kannala, J., \
                    Eklund, L., 2016. Cell segmentation proposal network. In: DLMIA. Vol. 10008 \
                    of Lect Notes Comput Sci. pp. 21-29. Bauer, S., Carion, N., Wild, P., 2016. \
                    Multi-organ cancer classification. arXiv:1606.00897.";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 3, "got {entries:?}");
        // `Corrado, G. S., Davis` is a comma inside an author list, not the end of an entry.
        assert!(
            entries[0].contains("Corrado, G. S., Zheng"),
            "{:?}",
            entries[0]
        );
        assert!(entries[1].starts_with("Akram, S. U."));
        // A volume number sitting between two capitals must not cut the entry.
        assert!(entries[1].contains("Vol. 10008"));
        assert!(entries[2].starts_with("Bauer, S."));
    }

    /// The other author-year form brackets the date; `imagenet.pdf` is set this way.
    #[test]
    fn parenthesised_year_entries_split_on_their_author_lists() {
        let text = "Ahonen, T., Hadid, A., and Pietikinen, M. (2006). Face description with \
                    local binary patterns. PAMI, 28. Alexe, B., Deselares, T., and Ferrari, V. \
                    (2012). Measuring the objectness of image windows. In PAMI. Lowe, D. G. \
                    (2004). Distinctive image features. IJCV, 60(2):91-110.";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 3, "got {entries:?}");
        assert!(entries[1].ends_with("In PAMI."));
        assert!(entries[2].starts_with("Lowe, D. G."));
    }

    /// A surname may begin with a particle, and then so does the entry.
    #[test]
    fn a_particle_can_begin_an_entry() {
        let text = "Abadi, M., Zheng, X., 2016. Tensorflow: large-scale learning. \
                    arXiv:1603.04467. van Ginneken, B., Setio, A. A., 2015. Off-the-shelf \
                    convolutional features. In: ISBI. pp. 286-289. De Gennes, P. G., 1966, \
                    Superconductivity of Metals and Alloys (W. A. Benjamin, New York). \
                    Von Klitzing, K., 2005, Phil. Trans. R. Soc. A 363, 2203.";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 4, "got {entries:?}");
        assert!(entries[1].starts_with("van Ginneken, B."));
        assert!(entries[2].starts_with("De Gennes, P. G."));
        assert!(entries[3].starts_with("Von Klitzing, K."));
    }

    /// The editor clause of a book chapter looks like an author list and is not one.
    ///
    /// `In Thomson, D. L., Cooch, E. G., editors, ...` follows a finished sentence and reads
    /// surname-comma-initial from the second word on. Requiring the *first* word to be a name is
    /// what keeps it inside the entry it belongs to; `statistics.pdf` scores 27 of 27 only
    /// because of it.
    #[test]
    fn an_editor_clause_does_not_open_an_entry() {
        let text = "Efford, M. G., Borchers, D. L., and Byrom, A. E. (2009). Density estimation \
                    by spatially explicit capture-recapture. In Thomson, D. L., Cooch, E. G., \
                    and Conroy, M. J., editors, Modeling Demographic Processes, pages 255-269. \
                    Springer, Boston, MA. Haines, L. M., and Altwegg, R. (2023). Exact \
                    likelihoods for N-mixture models. Aust NZ J Stat, 65(4):327-343. Hougaard, \
                    P. (2000). Analysis of Multivariate Survival Data. Springer, New York, NY.";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 3, "got {entries:?}");
        assert!(entries[0].contains("In Thomson, D. L."), "{:?}", entries[0]);
        assert!(entries[1].starts_with("Haines, L. M."));
    }

    /// A style the signal only half-fits leaves the text whole rather than cutting it badly.
    ///
    /// `adam.pdf` prints given names in full — `Amari, Shun-Ichi.` — so the initial the split
    /// looks for is absent from nearly every entry, and the two places it does appear would
    /// carve the bibliography into three blobs. Three blobs are worse than one, and worse than
    /// the caller's own fallback; the median guard rejects them.
    #[test]
    fn a_few_chance_matches_leave_the_text_whole() {
        let filler = "Some author wrote a title of quite unremarkable length about learning. ";
        let mut text = String::new();
        text.push_str(
            "Amari, Shun-Ichi. Natural gradient works efficiently. Neural \
                       computation, 10(2):251-276, 1998. ",
        );
        for _ in 0..12 {
            text.push_str(filler);
        }
        text.push_str(
            "Hinton, G.E. and Salakhutdinov, R.R. Reducing the dimensionality of \
                       data. Science, 313(5786):504-507, 2006. ",
        );
        for _ in 0..12 {
            text.push_str(filler);
        }
        text.push_str(
            "Tieleman, T. and Hinton, G. Lecture 6.5 - RMSProp. Technical report, \
                       2012.",
        );
        assert_eq!(split_entries(&text).len(), 1, "{:?}", split_entries(&text));
    }

    /// Where a bibliography is labelled, the labels settle it.
    ///
    /// The author signal fires inside a `1.`-numbered list too, one entry late and one entry
    /// short, and letting it win would cost an entry and strand every label on the wrong side of
    /// its own entry. `unet.pdf` is numbered this way.
    #[test]
    fn labels_outrank_author_lists() {
        let text = "1. Ciresan, D. C., Giusti, A., Schmidhuber, J.: Deep neural networks \
                    segment neuronal membranes. In: NIPS. pp. 2852-2860 (2012) 2. \
                    Dosovitskiy, A., Riedmiller, M., Brox, T.: Discriminative unsupervised \
                    feature learning. In: NIPS (2014) 3. Girshick, R., Donahue, J., Malik, J.: \
                    Rich feature hierarchies. In: CVPR (2014)";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 3, "got {entries:?}");
        assert!(entries[0].starts_with("1. Ciresan"));
        assert!(entries[2].starts_with("3. Girshick"));
    }

    /// An accent the reader prints as its own character is part of the surname.
    #[test]
    fn an_accent_does_not_truncate_a_surname() {
        let text = "Abadi, M., Zheng, X., 2016. Tensorflow: large-scale learning. \
                    arXiv:1603.04467. Abra`moff, M. D., Folk, J. C., 2016. Improved automated \
                    detection of retinopathy. Invest Ophthalmol Vis Sci 57, 5200-5206. \
                    Heikkila\u{a8}, J., Kannala, J., 2016. Cell segmentation proposals. In: \
                    DLMIA. pp. 21-29.";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 3, "got {entries:?}");
        assert!(entries[1].starts_with("Abra`moff, M. D."));
        assert!(entries[2].starts_with("Heikkila\u{a8}, J."));
    }
}
