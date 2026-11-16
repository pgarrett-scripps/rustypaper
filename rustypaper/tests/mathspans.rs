//! Inline mathematics welded to the word after it by a missing word break.
//!
//! IEEE's typesetting compresses the gap between a formula and the next word below the
//! space-synthesis threshold, so the reader reports `$T_b^{BF}$` and `has` as one word. Span
//! classification is word-level, and the span then swallows the prose: the pilot paper's
//! Markdown read `$T_b^{BF} has$ rank one`, `$1 \times Q-vector$`, `$\phi \in C^{N\times 1}
//! containing$`. The corpus cannot catch this — its LaTeX output keeps honest spaces — so the
//! weld is built synthetically: a maths-font run placed flush against the following word.

mod common;

/// One page of prose whose third font is a Computer Modern maths face. The maths run `Tb`
/// starts at x=120; `has` starts at x=131, inside or flush against the run's extent, so no
/// space is synthesised between them and the two arrive as one word.
fn welded_fixture() -> Vec<u8> {
    let mut content = String::new();

    content.push_str("BT\n/F2 16 Tf\n72 720 Td\n(Measurements of Synthetic Widgets)Tj\nET\n");
    content.push_str("BT\n/F1 10 Tf\n72 690 Td\n12.5 TL\n");
    content.push_str("(This synthetic page exists to test inline mathematics. The line)Tj\n");
    content.push_str("T*\n(below carries a formula welded onto the word after it.)Tj\nET\n");

    content.push_str("BT\n/F1 10 Tf\n72 600 Td\n(the matrix )Tj\nET\n");
    content.push_str("BT\n/F3 10 Tf\n120 600 Td\n(Tb)Tj\nET\n");
    content.push_str("BT\n/F1 10 Tf\n131 600 Td\n(has)Tj\nET\n");
    content.push_str("BT\n/F1 10 Tf\n148 600 Td\n( rank one and the rest is prose)Tj\nET\n");

    let objects = vec![
        "<</Type /Catalog /Pages 2 0 R>>".to_owned(),
        "<</Type /Pages /Kids [3 0 R] /Count 1>>".to_owned(),
        "<</Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources <</Font <</F1 5 0 R /F2 6 0 R /F3 7 0 R>>>> /Contents 4 0 R>>"
            .to_owned(),
        format!(
            "<</Length {}>>\nstream\n{content}\nendstream",
            content.len() + 1
        ),
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding>>"
            .to_owned(),
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding>>"
            .to_owned(),
        "<</Type /Font /Subtype /Type1 /BaseFont /CMMI10>>".to_owned(),
    ];
    common::assemble_pdf(&objects)
}

#[test]
fn a_span_ends_where_the_maths_ends() {
    let doc = common::convert_bytes("rustypaper-mathspans-welded.pdf", &welded_fixture());

    let text = doc
        .blocks
        .iter()
        .map(|b| b.text.as_str())
        .find(|t| t.contains("rank one"))
        .expect("the test line should survive conversion")
        .to_owned();

    assert!(text.contains('$'), "no inline span was found: {text:?}");
    assert!(
        !text.contains("has$") && !text.contains("has $"),
        "the welded word rode into the span: {text:?}"
    );
    assert!(
        text.contains("$ has"),
        "the welded word should follow the span as prose: {text:?}"
    );
}
