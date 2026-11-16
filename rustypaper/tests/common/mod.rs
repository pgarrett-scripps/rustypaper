//! Shared helpers for building minimal synthetic PDFs in integration tests.
//!
//! The corpus is all LaTeX output, so page shapes that only Word exports and journal
//! production systems produce — margin line numbers, tables drawn as per-cell border
//! fragments — never appear in it. These fixtures are generated at test time, so they
//! live in the repo as code, always run, and carry no licensing question.
//!
//! Each integration-test crate compiles this module afresh and uses its own subset of the
//! helpers, so an unused helper is expected, not a defect.
#![allow(dead_code)]

use std::fmt::Write as _;

/// Assembles a one-page PDF from `objects`, numbering them from 1 and writing the xref table.
pub fn assemble_pdf(objects: &[String]) -> Vec<u8> {
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

/// Wraps a content stream and two Helvetica fonts (`/F1` regular, `/F2` bold) into a
/// single-page document. `width` and `height` are the page's MediaBox extent.
pub fn one_page(content: &str, width: f32, height: f32) -> Vec<u8> {
    let objects = vec![
        "<</Type /Catalog /Pages 2 0 R>>".to_owned(),
        "<</Type /Pages /Kids [3 0 R] /Count 1>>".to_owned(),
        format!(
            "<</Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] \
             /Resources <</Font <</F1 5 0 R /F2 6 0 R>>>> /Contents 4 0 R>>"
        ),
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

/// Like [`one_page`], with a catalog `/Rotate` on the page: how a scanned-and-reflowed or
/// landscape-inserted page declares which way a viewer should turn it.
pub fn one_page_rotated(content: &str, width: f32, height: f32, rotate: i32) -> Vec<u8> {
    let objects = vec![
        "<</Type /Catalog /Pages 2 0 R>>".to_owned(),
        "<</Type /Pages /Kids [3 0 R] /Count 1>>".to_owned(),
        format!(
            "<</Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Rotate {rotate} \
             /Resources <</Font <</F1 5 0 R /F2 6 0 R>>>> /Contents 4 0 R>>"
        ),
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

/// One text run rotated 90° counter-clockwise in user space, reading bottom-to-top: how a
/// landscape table's rows arrive on a portrait page.
pub fn text_sideways(font: u8, size: f32, x: f32, y: f32, text: &str) -> String {
    format!("BT\n/F{font} {size} Tf\n0 1 -1 0 {x} {y} Tm\n({text})Tj\nET\n")
}

/// A filled rectangle in the content stream: how both `\toprule` and a Word table's cell
/// borders arrive on a page.
pub fn filled_rect(x: f32, y: f32, w: f32, h: f32) -> String {
    format!("{x} {y} {w} {h} re f\n")
}

/// One text run at a position: `BT /F<font> <size> Tf <x> <y> Td (<text>) Tj ET`.
pub fn text_at(font: u8, size: f32, x: f32, y: f32, text: &str) -> String {
    format!("BT\n/F{font} {size} Tf\n{x} {y} Td\n({text})Tj\nET\n")
}

/// Converts fixture bytes by writing them to a scratch file and running the pipeline.
pub fn convert_bytes(name: &str, bytes: &[u8]) -> rustypaper::doc::Document {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, bytes).expect("write fixture");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let _ = std::fs::remove_file(&path);
    doc
}
