//! A numbered reference list whose spaces carry `/ActualText`.
//!
//! Publisher PDFs rendered through Chromium wrap individual space glyphs in marked-content
//! spans: the space after each entry label declares `ActualText` U+0009 (a tab), and ordinary
//! word spaces declare U+00A0. The glyph on the page is a real space either way. A reader that
//! honours `ActualText` must still report a word boundary there — when the replacement text
//! swallowed the space, every label fused to its entry (`1.Lahiry`), sequential-label detection
//! found nothing, and whole reference lists collapsed to a block per column. The corpus never
//! sees this: its papers come from LaTeX, which writes no `ActualText` at all. This fixture is
//! the smallest synthetic equivalent of the four pilot papers that did.

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

/// A space glyph wrapped in a span declaring `text` (UTF-16BE hex) as its actual text.
fn actual_text_space(text: &str) -> String {
    format!("/Span<</ActualText<{text}>>>BDC\n( )Tj\nEMC\n")
}

/// One page: a title, a body paragraph, a References heading, and three labelled entries
/// in which the space after each label is an ActualText tab and the spaces between the
/// first words of each entry are ActualText no-break spaces — the shape Chromium prints.
fn reference_list_fixture() -> Vec<u8> {
    let mut content = String::new();

    content.push_str("BT\n/F2 16 Tf\n72 720 Td\n(Measurements of Synthetic Widgets)Tj\nET\n");

    content.push_str("BT\n/F1 10 Tf\n72 690 Td\n12.5 TL\n");
    content.push_str("(This synthetic page exists to test reference extraction. The three)Tj\n");
    content.push_str("T*\n(entries below are invented and cite no actual publications.)Tj\nET\n");

    content.push_str("BT\n/F2 12 Tf\n72 650 Td\n(References)Tj\nET\n");

    let entries = [
        (
            "1.",
            "Ada",
            "Example.",
            "Reliable widget placement on a level bench. \
                Synthetic Journal of Widgets, 12:45-67, 2020.",
        ),
        (
            "2.",
            "Grace",
            "Sample.",
            "Repeated readings of unchanged objects. \
                Journal of Invented Results, 3:101-110, 2021.",
        ),
        (
            "3.",
            "Mary",
            "Fixture.",
            "A comparison of instruments under fixed placement. \
                Annals of Test Data, 7:1-9, 2022.",
        ),
    ];
    content.push_str("BT\n/F1 10 Tf\n72 630 Td\n12.5 TL\n");
    for (label, first, second, rest) in entries {
        writeln!(content, "({label})Tj").unwrap();
        // The label's space declares a tab, exactly as the pilot papers do.
        content.push_str(&actual_text_space("FEFF0009"));
        writeln!(content, "({first})Tj").unwrap();
        // A word space declaring a no-break space, the other replacement seen in print.
        content.push_str(&actual_text_space("FEFF00A0"));
        write!(content, "({second} {rest})Tj\nT*\n").unwrap();
    }
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
fn actual_text_spaces_keep_labels_apart_from_their_entries() {
    let path = std::env::temp_dir().join("rustypaper-actualtext-references.pdf");
    std::fs::write(&path, reference_list_fixture()).expect("write fixture");

    let doc = rustypaper::convert(&path).expect("conversion failed");
    let _ = std::fs::remove_file(&path);

    let entries: Vec<&rustypaper::doc::Block> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypaper::doc::BlockKind::Reference)
        .collect();
    let all_text: Vec<String> = doc.blocks.iter().map(|b| b.text.clone()).collect();

    // The word boundary the page prints must survive the ActualText replacement: a tab or a
    // no-break space is still a space. Fused words mean the reader swallowed it.
    for fused in ["1.Ada", "2.Grace", "3.Mary", "AdaExample", "GraceSample"] {
        assert!(
            !all_text.iter().any(|t| t.contains(fused)),
            "ActualText replacement swallowed a printed space: found {fused:?} in {all_text:?}"
        );
    }

    // With the boundaries intact, the sequential labels segment the list.
    assert_eq!(
        entries.len(),
        3,
        "expected three reference entries, got {:?}",
        all_text
    );
    for (entry, surname) in entries.iter().zip(["Example", "Sample", "Fixture"]) {
        assert!(
            entry.text.contains(surname),
            "entry lost its author: {:?}",
            entry.text
        );
    }
}
