//! A reference list whose DOIs wrap across line breaks.
//!
//! A typesetter breaks a long URL after a separator — `/`, `.`, `-` — and adds no hyphen, so
//! the page shows `https://doi.org/10.1016/` at one line's end and the rest at the start of
//! the next. Joining those lines with a space truncates the DOI at the first break, and a
//! consumer that resolves the DOI then finds a registrant with no suffix: a reference that
//! "does not exist". Seven of the pilot papers' references were marked exactly that way. The
//! corpus never shows this: its LaTeX bibliographies keep DOIs on one line or set them in
//! breakable fonts that keep the slash and suffix together.

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

/// One page: a title, a body paragraph, a References heading, and three labelled entries
/// whose DOIs wrap onto a continuation line — after the scheme, after a path slash, and at
/// a hyphen that belongs to the identifier.
fn wrapped_doi_fixture() -> Vec<u8> {
    let mut content = String::new();

    content.push_str("BT\n/F2 16 Tf\n72 720 Td\n(Measurements of Synthetic Widgets)Tj\nET\n");

    content.push_str("BT\n/F1 10 Tf\n72 690 Td\n12.5 TL\n");
    content.push_str("(This synthetic page exists to test reference extraction. The three)Tj\n");
    content.push_str("T*\n(entries below are invented and cite no actual publications.)Tj\nET\n");

    content.push_str("BT\n/F2 12 Tf\n72 650 Td\n(References)Tj\nET\n");

    // Each entry is two lines; the second carries the rest of a DOI the first line started.
    let entries = [
        (
            "1. Ada Example. Reliable widget placement on a level bench. Synthetic",
            "Journal of Widgets, 12:45-67, 2020. https://doi.org/10.1016/",
            "j.synwid.2020.123456",
        ),
        (
            "2. Grace Sample. Repeated readings of unchanged objects. Journal of",
            "Invented Results, 3:101-110, 2021. https://",
            "doi.org/10.1038/s00000-021-00000-1",
        ),
        (
            "3. Mary Fixture. A comparison of instruments under fixed placement.",
            "Annals of Test Data, 7:1-9, 2022. https://doi.org/10.1128/jb.181.21.6650-",
            "6655.1999",
        ),
    ];
    content.push_str("BT\n/F1 10 Tf\n72 630 Td\n12.5 TL\n");
    for (first, second, continuation) in entries {
        writeln!(content, "({first})Tj\nT*").unwrap();
        writeln!(content, "({second})Tj\nT*").unwrap();
        writeln!(content, "({continuation})Tj\nT*").unwrap();
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

/// A reference list in which every entry ends with a complete DOI and a period at a line's
/// end, and the next entry's label — in the given style — starts the next line. That boundary
/// has the exact shape of a URL wrap, and gluing one label collapses every entry after it.
fn label_collision_fixture(labels: [&str; 3]) -> Vec<u8> {
    let mut content = String::new();

    content.push_str("BT\n/F2 16 Tf\n72 720 Td\n(Measurements of Synthetic Widgets)Tj\nET\n");
    content.push_str("BT\n/F1 10 Tf\n72 690 Td\n12.5 TL\n");
    content.push_str("(This synthetic page exists to test reference extraction. The three)Tj\n");
    content.push_str("T*\n(entries below are invented and cite no actual publications.)Tj\nET\n");
    content.push_str("BT\n/F2 12 Tf\n72 650 Td\n(References)Tj\nET\n");

    let bodies = [
        "Ada Example. Reliable widget placement on a level bench.",
        "Grace Sample. Repeated readings of unchanged objects.",
        "Mary Fixture. A comparison of instruments under fixed placement.",
    ];
    content.push_str("BT\n/F1 10 Tf\n72 630 Td\n12.5 TL\n");
    for (i, (label, body)) in labels.iter().zip(bodies).enumerate() {
        writeln!(content, "({label} {body})Tj\nT*").unwrap();
        // The entry's last line ends in its DOI and a final period; the next entry's
        // label starts the next line.
        writeln!(
            content,
            "(Synthetic Journal, 2020. https://doi.org/10.1101/2020.04.02.02227{i}.)Tj\nT*"
        )
        .unwrap();
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
fn an_entry_label_after_a_doi_is_never_glued_to_it() {
    for (style, labels) in [
        ("numbered", ["1.", "2.", "3."]),
        ("bare", ["1", "2", "3"]),
        ("parenthesised", ["(1)", "(2)", "(3)"]),
    ] {
        let path = std::env::temp_dir().join(format!("rustypaper-urlwrap-labels-{style}.pdf"));
        std::fs::write(&path, label_collision_fixture(labels)).expect("write fixture");
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let _ = std::fs::remove_file(&path);

        let all_text: Vec<&str> = doc.blocks.iter().map(|b| b.text.as_str()).collect();
        for i in 0..labels.len() {
            let doi = format!("https://doi.org/10.1101/2020.04.02.02227{i}.");
            assert!(
                all_text.iter().any(|t| t.contains(&doi)),
                "{style}: DOI {doi:?} lost from {all_text:?}"
            );
            // The next entry's label must not have been read as the DOI's continuation.
            if let Some(next) = labels.get(i + 1) {
                let fused = format!("{doi}{next}");
                assert!(
                    !all_text.iter().any(|t| t.contains(&fused)),
                    "{style}: label glued onto the DOI before it: {fused:?}"
                );
            }
        }
    }
}

#[test]
fn a_doi_wrapped_across_lines_comes_out_whole() {
    let path = std::env::temp_dir().join("rustypaper-urlwrap-references.pdf");
    std::fs::write(&path, wrapped_doi_fixture()).expect("write fixture");

    let doc = rustypaper::convert(&path).expect("conversion failed");
    let _ = std::fs::remove_file(&path);

    let entries: Vec<&rustypaper::doc::Block> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypaper::doc::BlockKind::Reference)
        .collect();
    let all_text: Vec<String> = entries.iter().map(|b| b.text.clone()).collect();

    assert_eq!(entries.len(), 3, "expected three entries, got {all_text:?}");

    // The whole DOI, exactly as printed, with no space at the break.
    for doi in [
        "https://doi.org/10.1016/j.synwid.2020.123456",
        "https://doi.org/10.1038/s00000-021-00000-1",
        "https://doi.org/10.1128/jb.181.21.6650-6655.1999",
    ] {
        assert!(
            all_text.iter().any(|t| t.contains(doi)),
            "DOI truncated at its line break: {doi:?} not whole in {all_text:?}"
        );
    }

    // The join must not have glued the words around the URL: the prose before each DOI
    // still ends in a spaced year, not a fused token.
    for intact in ["2020. https://", "2021. https://", "2022. https://"] {
        assert!(
            all_text.iter().any(|t| t.contains(intact)),
            "prose around the DOI was damaged: {intact:?} missing from {all_text:?}"
        );
    }
}
