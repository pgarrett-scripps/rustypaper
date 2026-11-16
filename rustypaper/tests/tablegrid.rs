//! A two-column journal page whose table is drawn as per-cell border fragments, with
//! negative numbers in its cells.
//!
//! Word exports and journal production systems draw a table grid cell by cell: every border
//! is its own short rule fragment, abutting its neighbours, and no single rule spans the
//! table. Each fragment is far narrower than `MIN_TABLE_RULE_WIDTH`, so without coalescing
//! the whole table vanishes — the shape that lost every table of two papers in the
//! peer-review pilot. The corpus cannot catch it: `booktabs` draws each rule in one piece.
//!
//! The cells also carry the three ways a minus arrives broken: an en-dash standing for the
//! sign, a sign set as its own text run a word-space away from its number, and a column of
//! signs far enough from the digits to become a column of its own.

mod common;

/// Left-column and right-column prose, and a 4×3 grid table in the right column. The two
/// columns share no baselines, so the fixture does not depend on gutter detection.
fn journal_page() -> Vec<u8> {
    let mut content = String::new();

    let left = [
        "The assignment of each sample to a group was concealed from the",
        "operators until every reading had been recorded and signed. The",
        "groups were then compared on the change from baseline, with the",
        "interval reported beside each estimate. Negative changes mean the",
        "quantity fell over the study period, and the sign of each entry",
        "therefore carries the finding itself rather than a detail of it.",
    ];
    for (i, line) in left.iter().enumerate() {
        content.push_str(&common::text_at(
            1,
            10.0,
            50.0,
            700.0 - 26.0 * i as f32,
            line,
        ));
    }
    content.push_str(&common::text_at(
        1,
        10.0,
        50.0,
        443.0,
        "The table lists the change measured in each group together with",
    ));
    content.push_str(&common::text_at(
        1,
        10.0,
        50.0,
        417.0,
        "the half-width of its interval, signed as measured.",
    ));

    let right = [
        "Readings from the three groups are summarised below. Entries",
        "are the mean change from baseline in each group, and the",
        "interval column gives the signed half-width about that mean.",
        "No reading was excluded from any group after unblinding, and",
        "the table carries every group that completed the protocol.",
    ];
    for (i, line) in right.iter().enumerate() {
        content.push_str(&common::text_at(
            1,
            10.0,
            317.0,
            687.0 - 26.0 * i as f32,
            line,
        ));
    }

    // The grid: five border lines, each drawn as three abutting per-cell fragments, the way
    // Word and journal production systems print a table. No fragment spans the table.
    for y in [540.0, 522.0, 504.0, 486.0, 468.0] {
        content.push_str(&common::filled_rect(317.0, y, 83.0, 0.5));
        content.push_str(&common::filled_rect(399.7, y, 80.3, 0.5));
        content.push_str(&common::filled_rect(479.7, y, 82.3, 0.5));
    }

    // Header row.
    content.push_str(&common::text_at(1, 9.0, 322.0, 527.5, "Param"));
    content.push_str(&common::text_at(1, 9.0, 405.0, 527.5, "Change"));
    content.push_str(&common::text_at(1, 9.0, 508.0, 527.5, "CI"));

    // Row 1: the sign set as an en-dash (\226 in WinAnsi), one run with its number.
    content.push_str(&common::text_at(1, 9.0, 322.0, 509.5, "Alpha"));
    content.push_str(&common::text_at(1, 9.0, 405.0, 509.5, "\\2263.2"));
    // Row 2: the sign as its own text run, a word-space short of its number.
    content.push_str(&common::text_at(1, 9.0, 322.0, 491.5, "Beta"));
    content.push_str(&common::text_at(1, 9.0, 405.0, 491.5, "-"));
    content.push_str(&common::text_at(1, 9.0, 412.0, 491.5, "0.8"));
    // Row 3: a plain positive value, which must stay untouched.
    content.push_str(&common::text_at(1, 9.0, 322.0, 473.5, "Gamma"));
    content.push_str(&common::text_at(1, 9.0, 405.0, 473.5, "4.1"));

    // The interval column: signs far enough from their digits to read as a column of
    // their own. Row 2's interval is positive and has no sign.
    content.push_str(&common::text_at(1, 9.0, 496.0, 509.5, "-"));
    content.push_str(&common::text_at(1, 9.0, 508.0, 509.5, "1.1"));
    content.push_str(&common::text_at(1, 9.0, 508.0, 491.5, "2.5"));
    content.push_str(&common::text_at(1, 9.0, 496.0, 473.5, "-"));
    content.push_str(&common::text_at(1, 9.0, 508.0, 473.5, "7.4"));

    content.push_str(&common::text_at(
        1,
        9.0,
        322.0,
        450.0,
        "Table 1. Change from baseline in the three synthetic groups.",
    ));

    common::one_page(&content, 612.0, 792.0)
}

#[test]
fn a_grid_of_border_fragments_is_a_table_with_its_signs() {
    let doc = common::convert_bytes("rustypaper-tablegrid-journal.pdf", &journal_page());

    let tables: Vec<&rustypaper::doc::Block> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypaper::doc::BlockKind::Table)
        .collect();
    assert_eq!(
        tables.len(),
        1,
        "expected the grid table, got {:?}",
        doc.blocks
            .iter()
            .map(|b| (&b.kind, b.text.as_str()))
            .collect::<Vec<_>>()
    );

    let data = tables[0].table.as_ref().expect("table block without cells");
    assert_eq!(data.header_rows, 1, "the border under the header marks it");
    assert_eq!(
        data.rows,
        vec![
            vec!["Param", "Change", "CI"],
            vec!["Alpha", "-3.2", "-1.1"],
            vec!["Beta", "-0.8", "2.5"],
            vec!["Gamma", "4.1", "-7.4"],
        ],
        "cells or signs came out wrong"
    );
}
