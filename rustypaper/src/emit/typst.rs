//! Typst.
//!
//! Typst models what a paper actually is — figures with captions, tables with real cells,
//! references — where Markdown models a text document and has to fake all three. That is the
//! reason this emitter exists, and the reason the document model was never allowed to become
//! "whatever Markdown can express".
//!
//! Mathematics is the one place the mapping is not direct. Typst has its own maths syntax, not
//! LaTeX's, so formulae go through the `mitex` package, which renders LaTeX inside Typst. The
//! import is emitted automatically when a document contains any.

use crate::doc::{Block, BlockKind, Document, TableData};

/// Renders a document as Typst.
pub fn render(doc: &Document) -> String {
    let mut out = String::new();

    if uses_math(doc) {
        out.push_str("#import \"@preview/mitex:0.2.4\": mi, mitex\n\n");
    }

    for block in &doc.blocks {
        render_block(&mut out, block);
    }

    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

fn uses_math(doc: &Document) -> bool {
    doc.blocks
        .iter()
        .any(|b| b.math.is_some() || b.text.contains('$'))
}

fn render_block(out: &mut String, block: &Block) {
    match &block.kind {
        BlockKind::Title => {
            out.push_str("= ");
            out.push_str(&inline(&block.text));
        }
        BlockKind::Heading { level } => {
            // Typst's `=` is the document title, so a level-1 section is `==`.
            for _ in 0..=(*level).clamp(1, 5) {
                out.push('=');
            }
            out.push(' ');
            out.push_str(&inline(&block.text));
        }
        BlockKind::Figure => match &block.asset {
            Some(path) => {
                out.push_str("#figure(\n  image(\"");
                out.push_str(path);
                out.push_str("\"),\n  caption: [");
                out.push_str(&inline(&block.text));
                out.push_str("],\n)");
            }
            None => {
                out.push_str("#emph[");
                out.push_str(&inline(&block.text));
                out.push(']');
            }
        },
        BlockKind::Caption => {
            out.push_str("#emph[");
            out.push_str(&inline(&block.text));
            out.push(']');
        }
        BlockKind::ListItem { ordered } => {
            out.push_str(if *ordered { "+ " } else { "- " });
            out.push_str(&inline(strip_marker(&block.text)));
        }
        BlockKind::Footnote => {
            out.push_str("#footnote[");
            out.push_str(&inline(&block.text));
            out.push(']');
        }
        BlockKind::Equation => match (&block.math, &block.asset) {
            (_, Some(path)) => {
                out.push_str("#figure(image(\"");
                out.push_str(path);
                out.push_str("\"))");
            }
            (Some(math), None) => {
                out.push_str("#mitex(`");
                out.push_str(&math.latex);
                out.push_str("`)");
                if let Some(number) = &math.number {
                    out.push_str(" <eq-");
                    out.push_str(number);
                    out.push('>');
                }
            }
            (None, None) => out.push_str(&inline(&block.text)),
        },
        BlockKind::Table => match &block.table {
            Some(data) => render_table(out, data),
            None => out.push_str("#emph[table]"),
        },
        BlockKind::Reference => {
            if let Some(label) = block.reference.as_ref().and_then(|r| r.label.as_deref()) {
                out.push_str(&format!("#label(\"ref-{label}\")"));
            }
            out.push_str(&inline(&block.text));
        }
        BlockKind::Paragraph => out.push_str(&inline(&block.text)),
    }
    out.push_str("\n\n");
}

/// Renders a table with real cells, which is the thing Markdown cannot do.
fn render_table(out: &mut String, data: &TableData) {
    let columns = data.rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return;
    }

    out.push_str(&format!("#table(\n  columns: {columns},\n"));
    for (i, row) in data.rows.iter().enumerate() {
        // Header cells are marked as such, so Typst can style and repeat them across pages.
        let header = i < data.header_rows;
        out.push_str("  ");
        for column in 0..columns {
            let text = row.get(column).map(String::as_str).unwrap_or("");
            if header {
                out.push_str(&format!("table.cell(fill: luma(240))[{}], ", inline(text)));
            } else {
                out.push_str(&format!("[{}], ", inline(text)));
            }
        }
        out.push('\n');
    }
    out.push(')');
}

/// Escapes the characters that carry meaning in Typst markup, leaving mathematics alone.
///
/// The exemption matters. Inline formulae are already delimited with `$`, and their contents are
/// LaTeX — full of backslashes and braces that escaping would turn into literal text, so that
/// `$x^{\alpha}$` reached the page as `x^{\\alpha}`.
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_math = false;

    for c in text.chars() {
        if c == '$' {
            in_math = !in_math;
            out.push(c);
            continue;
        }
        if in_math {
            out.push(c);
            continue;
        }
        if matches!(c, '#' | '[' | ']' | '*' | '_' | '@' | '\\' | '<' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn strip_marker(text: &str) -> &str {
    super::markdown::strip_marker(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Rect;

    fn block(kind: BlockKind, text: &str) -> Block {
        Block {
            kind,
            text: text.to_owned(),
            page: 0,
            bbox: Rect::from_corners(0.0, 0.0, 1.0, 1.0),
            size: 10.0,
            asset: None,
            table: None,
            math: None,
            reference: None,
        }
    }

    #[test]
    fn renders_headings_and_prose() {
        let doc = Document {
            title: Some("A Paper".into()),
            blocks: vec![
                block(BlockKind::Title, "A Paper"),
                block(BlockKind::Heading { level: 1 }, "1 Introduction"),
                block(BlockKind::Paragraph, "Some body text."),
            ],
        };
        assert_eq!(
            render(&doc),
            "= A Paper\n\n== 1 Introduction\n\nSome body text.\n"
        );
    }

    #[test]
    fn figures_become_figure_elements_with_captions() {
        let mut figure = block(BlockKind::Figure, "Figure 1. A plot.");
        figure.asset = Some("assets/figure-001.png".into());
        let doc = Document {
            title: None,
            blocks: vec![figure],
        };
        let out = render(&doc);
        assert!(out.contains("#figure("), "{out}");
        assert!(out.contains("image(\"assets/figure-001.png\")"));
        assert!(out.contains("caption: [Figure 1. A plot.]"));
    }

    /// The reason Typst is worth emitting: a table with real cells and a marked header.
    #[test]
    fn tables_become_real_tables() {
        let mut b = block(BlockKind::Table, "");
        b.table = Some(TableData {
            rows: vec![
                vec!["Model".into(), "BLEU".into()],
                vec!["GNMT".into(), "24.61".into()],
            ],
            header_rows: 1,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        let out = render(&doc);
        assert!(out.contains("#table(\n  columns: 2,"), "{out}");
        assert!(out.contains("table.cell(fill: luma(240))[Model]"));
        assert!(out.contains("[GNMT]"));
    }

    #[test]
    fn maths_goes_through_mitex_and_imports_it() {
        let mut b = block(BlockKind::Equation, "");
        b.math = Some(crate::doc::MathData {
            latex: r"E=mc^2".into(),
            number: Some("1".into()),
            confidence: 1.0,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        let out = render(&doc);
        assert!(out.starts_with("#import \"@preview/mitex"), "{out}");
        assert!(out.contains("#mitex(`E=mc^2`)"));
        assert!(out.contains("<eq-1>"), "the equation number became a label");
    }

    #[test]
    fn a_document_without_maths_does_not_import_mitex() {
        let doc = Document {
            title: None,
            blocks: vec![block(BlockKind::Paragraph, "no formulae here")],
        };
        assert!(!render(&doc).contains("mitex"));
    }

    #[test]
    fn markup_characters_are_escaped() {
        let doc = Document {
            title: None,
            blocks: vec![block(
                BlockKind::Paragraph,
                "a #hash and [brackets] and *stars*",
            )],
        };
        let out = render(&doc);
        assert!(out.contains("\\#hash"), "{out}");
        assert!(out.contains("\\[brackets\\]"));
        assert!(out.contains("\\*stars\\*"));
    }

    /// LaTeX inside an inline formula must survive verbatim.
    #[test]
    fn inline_maths_is_not_escaped() {
        let doc = Document {
            title: None,
            blocks: vec![block(
                BlockKind::Paragraph,
                r"we set $x^{\alpha}$ and [see] this",
            )],
        };
        let out = render(&doc);
        assert!(out.contains(r"$x^{\alpha}$"), "maths was mangled: {out}");
        assert!(out.contains(r"\[see\]"), "prose was not escaped: {out}");
    }

    #[test]
    fn ordered_and_unordered_lists_differ() {
        let doc = Document {
            title: None,
            blocks: vec![
                block(BlockKind::ListItem { ordered: true }, "1. first"),
                block(BlockKind::ListItem { ordered: false }, "• bullet"),
            ],
        };
        assert_eq!(render(&doc), "+ first\n\n- bullet\n");
    }

    #[test]
    fn an_empty_document_renders_to_a_newline() {
        assert_eq!(render(&Document::default()), "\n");
    }
}
