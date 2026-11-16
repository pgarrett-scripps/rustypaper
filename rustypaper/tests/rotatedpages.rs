//! Pages whose text is sideways as a whole must still convert.
//!
//! Two shapes arrive this way and both used to convert to zero blocks without a word of
//! warning: a landscape table drawn sideways on a portrait page (two medical-journal papers in
//! the peer-review pilot lost three full-page tables, ~300 words each, exactly so), and a page
//! with upright content under a non-zero catalog `/Rotate`, which the backend applies to the
//! geometry, leaving every glyph angled in display space. The line builder drops any glyph more
//! than 5° off horizontal — the right call for an arXiv margin stamp, and the wrong one when
//! sideways text is all the page has.

mod common;

use common::{one_page, one_page_rotated, text_at, text_sideways};

/// Three lines of prose, enough glyphs to clear every evidence floor on the way in.
const LINES: [&str; 3] = [
    "This page carries a paragraph of perfectly ordinary prose that should",
    "survive conversion whatever the page rotation attribute says about how",
    "a viewer ought to turn it for display on a screen.",
];

fn joined_text(document: &rustypaper::doc::Document) -> String {
    document
        .blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn a_page_under_a_catalog_rotate_still_converts() {
    for rotate in [90, 180, 270] {
        let mut content = String::new();
        for (i, line) in LINES.iter().enumerate() {
            content.push_str(&text_at(1, 12.0, 72.0, 700.0 - 14.0 * i as f32, line));
        }
        let bytes = one_page_rotated(&content, 612.0, 792.0, rotate);
        let doc = common::convert_bytes(&format!("rp-rotate-{rotate}.pdf"), &bytes);
        assert!(
            !doc.blocks.is_empty(),
            "/Rotate {rotate} page converted to nothing"
        );
        assert!(
            joined_text(&doc).contains("perfectly ordinary prose"),
            "/Rotate {rotate} page lost its text: {:?}",
            joined_text(&doc)
        );
    }
}

#[test]
fn a_page_of_sideways_text_still_converts() {
    // A landscape body on a portrait /Rotate 0 page: every run reads bottom-to-top.
    let mut content = String::new();
    for (i, line) in LINES.iter().enumerate() {
        content.push_str(&text_sideways(1, 12.0, 92.0 + 14.0 * i as f32, 720.0, line));
    }
    let bytes = one_page(&content, 612.0, 792.0);
    let doc = common::convert_bytes("rp-sideways.pdf", &bytes);
    assert!(!doc.blocks.is_empty(), "sideways page converted to nothing");
    assert!(
        joined_text(&doc).contains("perfectly ordinary prose"),
        "sideways page lost its text: {:?}",
        joined_text(&doc)
    );
}

#[test]
fn a_margin_stamp_beside_a_body_changes_nothing() {
    // The usual arXiv shape: upright prose with one sideways stamp down the margin. The page
    // must not be mistaken for a sideways one, and the stamp must stay out of the prose.
    let mut content = String::new();
    for (i, line) in LINES.iter().enumerate() {
        content.push_str(&text_at(1, 12.0, 72.0, 700.0 - 14.0 * i as f32, line));
    }
    content.push_str(&text_sideways(
        1,
        9.0,
        20.0,
        200.0,
        "arXiv:2611.01234v1 [cs.CL] 13 Nov 2026",
    ));
    let bytes = one_page(&content, 612.0, 792.0);
    let doc = common::convert_bytes("rp-stamp.pdf", &bytes);
    let text = joined_text(&doc);
    assert!(
        text.contains("perfectly ordinary prose"),
        "body lost: {text:?}"
    );
    assert!(
        !text.contains("arXiv:2611"),
        "stamp leaked into prose: {text:?}"
    );
}
