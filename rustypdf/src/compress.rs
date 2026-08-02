//! Telegraphic compression — "caveman mode".
//!
//! Strips the grammatical scaffolding of academic prose while leaving every content word
//! standing, for feeding papers to a model that charges by the token. `The results that we
//! obtained in the case of the larger model were shown to be significantly better` becomes
//! `results we obtained for larger model significantly better`.
//!
//! The rule throughout is that **only closed-class words go**. Articles, copulas, expletives and
//! the stock phrases academic writing pads itself with are predictable and carry no information a
//! reader — or a retrieval index — needs. Nouns, verbs, adjectives, numbers and symbols are never
//! touched, which is what makes the transformation safe to apply blind and what
//! `content_words_survive_compression` in the test suite checks.
//!
//! Four things are exempt outright: mathematics, tables, equations and bibliography entries.
//! Formulae are already minimal and mangling one changes its meaning; a citation with words
//! removed no longer resolves.

use crate::doc::{BlockKind, Document};

/// How hard to compress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    /// Articles, copulas and stock phrases. Reads as clipped English and loses nothing.
    #[default]
    Light,
    /// Also prepositions, pronouns, filler adverbs and connectives. Reads as telegraphese and
    /// loses the direction of some relations: `effect of X on Y` becomes `effect X Y`.
    Hard,
}

/// Dropped at [`Level::Hard`]: function words that carry structure rather than content.
///
/// Prepositions are the prize — they are frequent and short, which is exactly the profile of a
/// word worth removing when the cost is counted in tokens. They are also the reason `Hard` is
/// opt-in: `effect of X on Y` and `effect of Y on X` compress to the same thing.
const HARD_DROP: &[&str] = &[
    // Prepositions.
    "of", "in", "on", "at", "to", "for", "with", "by", "from", "into", "onto", "upon", "about",
    "over", "under", "between", "among", "through", "during", "within", "across",
    // Pronouns and determiners that repeat the subject.
    "it", "its", "this", "these", "those", "their", "our", "his", "her", "them", "they",
    // Auxiliary chains. Bare modals are handled separately and survive.
    "has", "have", "had", "do", "does", "did", "been", // Conjunctions that only join.
    "and", "or", "but", "than", "as", "so",
];

/// Filler and hedging adverbs, dropped at [`Level::Hard`].
///
/// Deliberately excludes `significantly`, `approximately`, `statistically` and their kin. In a
/// chat log those are padding; in a paper they are claims about effect size and precision, and
/// removing them rewrites the result. This is where a compressor built for prompts and one built
/// for scientific prose must part company.
const FILLER: &[&str] = &[
    "very",
    "quite",
    "essentially",
    "basically",
    "actually",
    "simply",
    "just",
    "really",
    "rather",
    "somewhat",
    "fairly",
    "clearly",
    "obviously",
    "certainly",
    "arguably",
    "generally",
    "typically",
    "usually",
    "often",
    "indeed",
    "moreover",
    "furthermore",
    "however",
    "therefore",
    "thus",
    "hence",
    "additionally",
    "consequently",
    "nevertheless",
    "nonetheless",
    "also",
    "then",
    "here",
    "well",
];

/// Words deleted wherever they appear.
///
/// Articles and copulas only. Modals (`can`, `may`, `should`) are deliberately absent: dropping
/// them changes a claim's strength, which is the one thing a scientific paper cannot afford to
/// lose.
const DELETABLE: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "am",
];

/// Deleted only when they are grammatical glue rather than content.
///
/// `that` as a complementiser (`we show that X`) is noise; as a demonstrative (`that model`) it
/// is not, so it goes only when a following word makes it a complementiser is too subtle to
/// judge here — it is dropped, and the loss is a determiner.
const DELETABLE_GLUE: &[&str] = &["that", "which", "there"];

/// Stock phrases, replaced by their meaning. Longest first, so that a longer phrase is not
/// broken up by a shorter one nested inside it.
const PHRASES: &[(&str, &str)] = &[
    ("it is important to note that", ""),
    ("it should be noted that", ""),
    ("it is worth noting that", ""),
    ("due to the fact that", "because"),
    ("in spite of the fact that", "although"),
    ("for the purpose of", "for"),
    ("in the case of", "for"),
    ("a large number of", "many"),
    ("a small number of", "few"),
    ("the majority of", "most"),
    ("with respect to", "for"),
    ("in terms of", "in"),
    ("as a result of", "from"),
    ("in this paper we", "we"),
    ("in this work we", "we"),
    ("has been shown to be", "is"),
    ("have been shown to be", "are"),
    ("in order to", "to"),
    ("as well as", "and"),
    ("such that", "so"),
    ("we note that", ""),
    ("we observe that", ""),
    ("note that", ""),
];

/// Discourse connectives that open a sentence and only signal the direction of travel.
const OPENERS: &[&str] = &[
    "furthermore",
    "moreover",
    "in addition",
    "additionally",
    "importantly",
    "notably",
    "specifically",
    "in particular",
    "that is",
    "for example",
    "for instance",
    "in fact",
    "indeed",
    "overall",
    "finally",
];

/// Compresses every prose block of a document in place.
pub fn compress(document: &mut Document, level: Level) {
    for block in &mut document.blocks {
        let compressible = matches!(
            block.kind,
            BlockKind::Paragraph
                | BlockKind::Caption
                | BlockKind::Figure
                | BlockKind::ListItem { .. }
                | BlockKind::Footnote
        );
        if compressible {
            block.text = compress_text_at(&block.text, level);
        }
    }
}

/// Compresses one run of prose at [`Level::Light`].
pub fn compress_text(text: &str) -> String {
    compress_text_at(text, Level::Light)
}

/// Compresses one run of prose.
pub fn compress_text_at(text: &str, level: Level) -> String {
    // Mathematics is set aside first and put back afterwards, so that no rule can reach inside a
    // formula. `$\alpha$` contains no English and must survive byte for byte.
    let (masked, formulae) = mask_maths(text);
    let stripped = strip_openers(&masked);
    let replaced = replace_phrases(&stripped);
    let dropped = drop_words(&replaced, level);
    restore_maths(&dropped, &formulae)
}

/// Replaces each `$...$` span with a placeholder, returning the spans.
fn mask_maths(text: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut formulae = Vec::new();
    let mut rest = text;

    while let Some(open) = rest.find('$') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('$') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&format!(" \u{1}{}\u{1} ", formulae.len()));
        formulae.push(rest[open..open + close + 2].to_owned());
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    (out, formulae)
}

fn restore_maths(text: &str, formulae: &[String]) -> String {
    let mut out = text.to_owned();
    for (i, formula) in formulae.iter().enumerate() {
        out = out.replace(&format!("\u{1}{i}\u{1}"), formula);
    }
    out
}

/// Removes a leading discourse connective, with its comma.
fn strip_openers(text: &str) -> String {
    let lower = text.to_lowercase();
    for opener in OPENERS {
        for suffix in [", ", " "] {
            let prefix = format!("{opener}{suffix}");
            if lower.starts_with(&prefix) {
                return capitalise(text[prefix.len()..].trim_start());
            }
        }
    }
    text.to_owned()
}

fn replace_phrases(text: &str) -> String {
    let mut out = text.to_owned();
    for (phrase, replacement) in PHRASES {
        out = replace_ignoring_case(&out, phrase, replacement);
    }
    out
}

/// Case-insensitive whole-phrase replacement.
fn replace_ignoring_case(text: &str, phrase: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let lower = text.to_lowercase();
    let mut cursor = 0;

    while let Some(found) = lower[cursor..].find(phrase) {
        let at = cursor + found;
        // Must be a whole phrase, not part of a longer word.
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let end = at + phrase.len();
        let after_ok = end >= lower.len() || !lower.as_bytes()[end].is_ascii_alphanumeric();

        out.push_str(&text[cursor..at]);
        if before_ok && after_ok {
            out.push_str(replacement);
        } else {
            out.push_str(&text[at..end]);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Drops closed-class words, preserving punctuation attached to what remains.
fn drop_words(text: &str, level: Level) -> String {
    let mut kept: Vec<String> = Vec::new();

    for token in text.split_whitespace() {
        let bare: String = token
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '\u{1}')
            .to_lowercase();

        let mut droppable =
            DELETABLE.contains(&bare.as_str()) || DELETABLE_GLUE.contains(&bare.as_str());
        if level == Level::Hard {
            droppable |= HARD_DROP.contains(&bare.as_str()) || FILLER.contains(&bare.as_str());
        }
        if droppable && !token.contains('\u{1}') {
            // Punctuation attached to a dropped word belongs to the sentence, not the word.
            let punctuation: String = token
                .chars()
                .filter(|c| matches!(c, '.' | ',' | ';' | ':' | '?' | '!' | ')'))
                .collect();
            if !punctuation.is_empty() {
                if let Some(last) = kept.last_mut() {
                    last.push_str(&punctuation);
                    continue;
                }
            }
            continue;
        }
        kept.push(token.to_owned());
    }

    capitalise(&kept.join(" "))
}

/// Restores a leading capital, which dropping the first word can lose.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_articles_and_copulas() {
        assert_eq!(
            compress_text("The results are better than the baseline"),
            "Results better than baseline"
        );
    }

    #[test]
    fn collapses_stock_phrases() {
        assert_eq!(
            compress_text("We trained in order to improve accuracy"),
            "We trained to improve accuracy"
        );
        assert_eq!(
            compress_text("Due to the fact that data is scarce"),
            "Because data scarce"
        );
    }

    #[test]
    fn strips_discourse_openers() {
        assert_eq!(
            compress_text("Furthermore, we evaluate on ImageNet"),
            "We evaluate on ImageNet"
        );
    }

    /// Modality is meaning. A claim that "may hold" must not become one that holds.
    #[test]
    fn modal_verbs_are_never_dropped() {
        for modal in ["can", "may", "might", "should", "must", "could", "would"] {
            let sentence = format!("Results {modal} improve");
            assert!(
                compress_text(&sentence).contains(modal),
                "{modal} was dropped"
            );
        }
    }

    /// Negation is meaning too, and is the most dangerous thing to lose.
    #[test]
    fn negation_survives() {
        assert_eq!(compress_text("The model is not better"), "Model not better");
    }

    #[test]
    fn mathematics_is_untouched() {
        let input = r"The value of $x^{2} + \alpha$ is the bound";
        let out = compress_text(input);
        assert!(
            out.contains(r"$x^{2} + \alpha$"),
            "maths was mangled: {out}"
        );
        assert!(!out.contains(" the "), "prose was not compressed: {out}");
    }

    #[test]
    fn punctuation_moves_to_the_surviving_word() {
        assert_eq!(
            compress_text("We evaluate the model, and report the results."),
            "We evaluate model, and report results."
        );
    }

    #[test]
    fn numbers_and_symbols_survive() {
        assert_eq!(
            compress_text("The error was 3.57% on the test set"),
            "Error 3.57% on test set"
        );
    }

    #[test]
    fn a_phrase_inside_a_word_is_not_replaced() {
        // `as well as` must not fire inside `assessment`.
        assert!(compress_text("Assessment of the data").starts_with("Assessment"));
    }

    #[test]
    fn hard_level_drops_prepositions_and_filler() {
        let light = compress_text("The results of the model are very clearly better");
        let hard = compress_text_at(
            "The results of the model are very clearly better",
            Level::Hard,
        );
        assert_eq!(light, "Results of model very clearly better");
        assert_eq!(hard, "Results model better");
        assert!(hard.len() < light.len());
    }

    /// The line this project will not cross. `significantly` is an effect-size claim in a paper,
    /// whatever it is in a chat log.
    #[test]
    fn statistical_qualifiers_survive_even_at_hard_level() {
        for word in [
            "significantly",
            "approximately",
            "statistically",
            "marginally",
        ] {
            let sentence = format!("Results were {word} better");
            assert!(
                compress_text_at(&sentence, Level::Hard).contains(word),
                "{word} was dropped at hard level"
            );
        }
    }

    #[test]
    fn hard_level_still_keeps_modals_and_negation() {
        assert!(compress_text_at("The model may not converge", Level::Hard).contains("may"));
        assert!(compress_text_at("The model may not converge", Level::Hard).contains("not"));
    }

    #[test]
    fn hard_level_leaves_maths_alone() {
        let out = compress_text_at(r"The bound of $x^{2}$ is tight", Level::Hard);
        assert!(out.contains(r"$x^{2}$"), "maths was mangled: {out}");
    }

    #[test]
    fn empty_and_whitespace_input_is_handled() {
        assert_eq!(compress_text(""), "");
        assert_eq!(compress_text("   "), "");
    }

    #[test]
    fn references_and_tables_are_left_alone() {
        use crate::ir::Rect;

        let mut doc = Document {
            title: None,
            blocks: vec![
                crate::doc::Block::new(
                    BlockKind::Reference,
                    0,
                    Rect::from_corners(0.0, 0.0, 1.0, 1.0),
                )
                .with_text("A. Author. The theory of the thing. Venue, 2016."),
                crate::doc::Block::new(
                    BlockKind::Paragraph,
                    0,
                    Rect::from_corners(0.0, 0.0, 1.0, 1.0),
                )
                .with_text("The theory is sound"),
            ],
        };
        compress(&mut doc, Level::Light);
        assert!(
            doc.blocks[0].text.contains("The theory of the thing"),
            "a citation was compressed and no longer resolves"
        );
        assert_eq!(doc.blocks[1].text, "Theory sound");
    }
}
