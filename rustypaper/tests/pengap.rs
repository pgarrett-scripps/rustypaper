//! A DOI whose glyphs leave white inside their cells without the pen ever leaving the pitch.
//!
//! In print this happens with tabular digits: the digits of `brainsci11020174` sit in
//! fixed-pitch cells, set solid — each advance exactly one cell — but a narrow `1` inks only
//! a strip of its cell, so the ink-to-ink gap around every `1` reads as up to 0.2 em of
//! white. A word segmenter that measures ink gaps prints `brainsci 1 1 020 1 74`; the pen,
//! which never jumped, says the line is one token.
//!
//! A hand-written fixture cannot embed a font with wide side bearings, so this one makes the
//! same geometry with character spacing instead: `Tc` widens every cell beyond its ink — the
//! spacing is part of each glyph's advance, exactly as a side bearing is part of its cell —
//! while the un-embedded font's boxes span only the glyph widths. Ink gaps of 0.25 em, pen
//! gaps of zero: red under an ink-gap segmenter, green under the pen.

use std::fmt::Write as _;

/// Assembles a one-page PDF from `objects`, numbering them from 1 and writing the xref table.
fn assemble_pdf(objects: &[String]) -> Vec<u8> {
    let mut pdf = String::from("%PDF-1.5\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (n, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        write!(pdf, "{} 0 obj\n{body}\nendobj\n", n + 1).unwrap();
    }
    let xref_at = pdf.len();
    write!(pdf, "xref\n0 {}\n", objects.len() + 1).unwrap();
    pdf.push_str("0000000000 65535 f \n");
    for offset in offsets {
        writeln!(pdf, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        pdf,
        "trailer\n<</Size {} /Root 1 0 R>>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    pdf.into_bytes()
}

/// One page: a title, a body paragraph, a References heading, and three labelled entries.
/// The first entry's DOI wraps, and its continuation line is a single letterspaced token:
/// `2.5 Tc` at 10 pt leaves 0.25 em of white between every pair of ink boxes while the pen
/// stays on its pitch.
fn letterspaced_doi_fixture() -> Vec<u8> {
    let mut content = String::new();

    content.push_str("BT\n/F2 16 Tf\n72 720 Td\n(Measurements of Synthetic Widgets)Tj\nET\n");

    content.push_str("BT\n/F1 10 Tf\n72 690 Td\n12.5 TL\n");
    content.push_str("(This synthetic page exists to test reference extraction. The three)Tj\n");
    content.push_str("T*\n(entries below are invented and cite no actual publications.)Tj\nET\n");

    content.push_str("BT\n/F2 12 Tf\n72 650 Td\n(References)Tj\nET\n");

    content.push_str("BT\n/F1 10 Tf\n72 630 Td\n12.5 TL\n");
    // The entry whose DOI continuation is letterspaced: the whole second line is one token
    // with no space glyph on it, so the segmenter has only the gaps to go by.
    content.push_str(
        "(1. Ada Example. Resting imaging of invented subjects. Brain Sci. 2021,)Tj\nT*\n",
    );
    content.push_str("(11, 174. https://doi.org/10.3390/)Tj\nT*\n");
    content.push_str("2.5 Tc\n(brainsci11020174)Tj\n0 Tc\nT*\n");
    // Two plain entries so the list still reads as a reference list.
    content
        .push_str("(2. Grace Sample. Repeated readings of unchanged objects. Journal of)Tj\nT*\n");
    content.push_str("(Invented Results, 3:101-110, 2021.)Tj\nT*\n");
    content
        .push_str("(3. Mary Fixture. A comparison of instruments under fixed placement.)Tj\nT*\n");
    content.push_str("(Annals of Test Data, 7:1-9, 2022.)Tj\nT*\n");
    content.push_str("ET\n");

    let objects = vec![
        "<</Type /Catalog /Pages 2 0 R>>".to_owned(),
        "<</Type /Pages /Kids [3 0 R] /Count 1>>".to_owned(),
        "<</Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources <</Font <</F1 5 0 R /F2 6 0 R>>>> /Contents 4 0 R>>"
            .to_owned(),
        format!(
            "<</Length {}>>\nstream\n{content}\nendstream",
            content.len() + 1
        ),
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding>>"
            .to_owned(),
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding>>"
            .to_owned(),
    ];
    assemble_pdf(&objects)
}

#[test]
fn white_inside_the_cells_is_not_a_word_break() {
    let path = std::env::temp_dir().join("rustypaper-pengap-references.pdf");
    std::fs::write(&path, letterspaced_doi_fixture()).expect("write fixture");

    let doc = rustypaper::convert(&path).expect("conversion failed");
    let _ = std::fs::remove_file(&path);

    let all_text: Vec<String> = doc.blocks.iter().map(|b| b.text.clone()).collect();

    // The letterspaced token comes out solid, and the wrap before it mends.
    assert!(
        all_text
            .iter()
            .any(|t| t.contains("https://doi.org/10.3390/brainsci11020174")),
        "letterspaced DOI continuation broken: {all_text:?}"
    );
    assert!(
        !all_text.iter().any(|t| t.contains("brainsci 1")),
        "white inside the cells was read as a space: {all_text:?}"
    );
}
