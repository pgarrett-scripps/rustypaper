//! A Word-style preprint page with margin line numbers.
//!
//! bioRxiv and medRxiv manuscripts come out of Word with every line of the body numbered in
//! the left margin. The numbers share the body's baselines, so line building fuses them into
//! the text lines — or column splitting strands them as one-word lines — and either way they
//! leak into every output format as stray integers. The corpus cannot catch this: LaTeX
//! papers are not line-numbered. The body prose here contains no digits at all, so any digit
//! in the converted document is a leaked line number.

mod common;

/// A single-column manuscript page: a bold title, then fourteen body lines, each with its
/// line number set in the left margin on the same baseline — the shape Word prints.
fn numbered_preprint() -> Vec<u8> {
    let body = [
        "The samples were washed twice and dried on a level bench before",
        "any measurement began, and the bench itself was checked against a",
        "reference plate between runs so that drift could be excluded from",
        "the comparison. Each reading was repeated by a second operator who",
        "had no knowledge of the first result, and the pair of readings was",
        "accepted only where the two agreed within the tolerance stated by",
        "the instrument maker. Disagreements were resolved by a third",
        "reading taken on the following day under the same conditions. The",
        "procedure is deliberately conservative, since the aim of the study",
        "was not speed but a defensible record of the measured values. All",
        "rejected readings were retained in the notebook with the reason",
        "for rejection written beside them, so that a later reader could",
        "audit every exclusion against the stated rule rather than taking",
        "the summary on trust.",
    ];

    let mut content = String::new();
    content.push_str(&common::text_at(
        2,
        16.0,
        108.0,
        720.0,
        "A Study of Careful Measurement",
    ));
    for (i, line) in body.iter().enumerate() {
        let y = 690.0 - 14.0 * i as f32;
        let number = (i + 1).to_string();
        content.push_str(&common::text_at(1, 9.0, 80.0, y, &number));
        content.push_str(&common::text_at(1, 11.0, 108.0, y, line));
    }
    common::one_page(&content, 612.0, 792.0)
}

#[test]
fn margin_line_numbers_do_not_leak_into_the_text() {
    let doc = common::convert_bytes("rustypaper-linenumbers-preprint.pdf", &numbered_preprint());

    let all_text: Vec<String> = doc.blocks.iter().map(|b| b.text.clone()).collect();

    // The prose must survive the stripping.
    assert!(
        all_text
            .iter()
            .any(|t| t.contains("checked against a reference plate")),
        "the body prose was lost: {all_text:?}"
    );

    // The body contains no digits, so any digit in any block is a leaked line number.
    for text in &all_text {
        assert!(
            !text.chars().any(|c| c.is_ascii_digit()),
            "a margin line number leaked into the output: {text:?}"
        );
    }
}
