//! GitHub-flavoured Markdown.

use crate::doc::{Block, BlockKind, Document};

/// Renders a document as Markdown.
pub fn render(doc: &Document) -> String {
    let mut out = String::new();
    for block in &doc.blocks {
        render_block(&mut out, block);
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

fn render_block(out: &mut String, block: &Block) {
    match block.kind {
        BlockKind::Title => {
            out.push_str("# ");
            out.push_str(&escape(&block.text));
        }
        BlockKind::Heading { level } => {
            // Level 1 headings sit under the title, so they render as `##`.
            for _ in 0..=level.clamp(1, 5) {
                out.push('#');
            }
            out.push(' ');
            out.push_str(&escape(&block.text));
        }
        BlockKind::Caption => {
            out.push('*');
            out.push_str(&escape(&block.text));
            out.push('*');
        }
        BlockKind::Figure => {
            // The caption doubles as alt text, which is what a screen reader wants and what a
            // retrieval index will embed.
            let alt = block.text.replace(['[', ']'], "");
            match &block.asset {
                Some(path) => {
                    out.push_str("![");
                    out.push_str(alt.trim());
                    out.push_str("](");
                    out.push_str(path);
                    out.push(')');
                }
                // Detected but not extracted: say so rather than dropping the figure silently.
                None if block.text.is_empty() => out.push_str("*[figure]*"),
                None => {
                    out.push('*');
                    out.push_str(&escape(&block.text));
                    out.push('*');
                }
            }
        }
        BlockKind::ListItem { ordered } => {
            // The document's own marker is kept in the text, so emit a plain bullet and let
            // Markdown renumber rather than fighting over `1.` versus `(a)`.
            out.push_str(if ordered { "1. " } else { "- " });
            out.push_str(strip_marker(&block.text));
        }
        BlockKind::Footnote => {
            out.push_str("> ");
            out.push_str(&escape(&block.text));
        }
        BlockKind::Equation => match (&block.math, &block.asset) {
            // A reconstruction the pass was not sure of is shown as a picture rather than as
            // LaTeX that looks authoritative and is wrong.
            (_, Some(path)) => {
                out.push_str("![equation](");
                out.push_str(path);
                out.push(')');
            }
            (Some(math), None) => {
                out.push_str("$$\n");
                out.push_str(&math.latex);
                if let Some(number) = &math.number {
                    out.push_str(" \\tag{");
                    out.push_str(number);
                    out.push('}');
                }
                out.push_str("\n$$");
            }
            (None, None) => out.push_str(&escape(&block.text)),
        },
        BlockKind::Reference => {
            // An anchor so that inline citations can point at the entry.
            if let Some(label) = block.reference.as_ref().and_then(|r| r.label.as_deref()) {
                out.push_str(&format!("<a id=\"ref-{label}\"></a>"));
            }
            out.push_str(&escape(&block.text));
        }
        BlockKind::Table => match &block.table {
            Some(data) => render_table(out, data),
            None => out.push_str("*[table]*"),
        },
        BlockKind::Paragraph => out.push_str(&escape(&block.text)),
    }
    out.push_str("\n\n");
}

/// Renders a table as GitHub-flavoured Markdown.
///
/// GFM requires a header row, so a table without one gets an empty header. Multi-row headers are
/// flattened into a single row: GFM has no way to express them, and losing the distinction reads
/// better than emitting the second header row as data.
fn render_table(out: &mut String, data: &crate::doc::TableData) {
    let columns = data.rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return;
    }

    let header = flatten_header(data, columns);
    write_row(out, &header);
    out.push('|');
    for _ in 0..columns {
        out.push_str(" --- |");
    }
    out.push('\n');

    for row in data
        .rows
        .iter()
        .skip(data.header_rows.max(1).min(data.rows.len()))
    {
        let mut cells: Vec<String> = row.iter().map(|c| cell(c)).collect();
        cells.resize(columns, String::new());
        write_row(out, &cells);
    }
    // The trailing blank line is added by the caller.
    out.pop();
}

fn flatten_header(data: &crate::doc::TableData, columns: usize) -> Vec<String> {
    let mut header = vec![String::new(); columns];
    for row in data.rows.iter().take(data.header_rows) {
        for (i, text) in row.iter().enumerate().take(columns) {
            if text.is_empty() {
                continue;
            }
            if !header[i].is_empty() {
                header[i].push(' ');
            }
            header[i].push_str(&cell(text));
        }
    }
    header
}

fn write_row(out: &mut String, cells: &[String]) {
    out.push('|');
    for text in cells {
        out.push(' ');
        out.push_str(text);
        out.push_str(" |");
    }
    out.push('\n');
}

/// A pipe inside a cell would end the cell, so it has to be escaped.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").trim().to_owned()
}

/// Removes a leading list marker, which the emitter re-adds in Markdown's own form.
fn strip_marker(text: &str) -> &str {
    let rest = text
        .strip_prefix(|c: char| {
            matches!(c, '•' | '◦' | '‣' | '▪' | '·' | '∗' | '*' | '–' | '—' | '-')
        })
        .map(str::trim_start);
    if let Some(rest) = rest {
        return rest;
    }
    match text.split_once(char::is_whitespace) {
        Some((head, rest)) if head.len() <= 6 => rest.trim_start(),
        _ => text,
    }
}

/// Escapes the few characters that would otherwise change the structure of the output.
///
/// Deliberately minimal: over-escaping scientific prose turns `x*y` into `x\*y` and makes the
/// result worse to read than the risk it avoids. Only line-leading markers, which would silently
/// become headings, lists or rules, are escaped.
fn escape(text: &str) -> String {
    let trimmed = text.trim_start();
    let leader = trimmed
        .chars()
        .next()
        .filter(|c| matches!(c, '#' | '>' | '-' | '+' | '=' | '|'));

    match leader {
        Some(c) => {
            let mut out = String::with_capacity(text.len() + 1);
            out.push('\\');
            out.push(c);
            out.push_str(&trimmed[c.len_utf8()..]);
            out
        }
        None => text.to_owned(),
    }
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
    fn renders_a_small_document() {
        let doc = Document {
            title: Some("A Paper".into()),
            blocks: vec![
                block(BlockKind::Title, "A Paper"),
                block(BlockKind::Heading { level: 1 }, "1 Introduction"),
                block(BlockKind::Paragraph, "Some body text."),
                block(BlockKind::Heading { level: 2 }, "1.1 Background"),
                block(BlockKind::Caption, "Figure 1. A plot."),
            ],
        };
        assert_eq!(
            render(&doc),
            "# A Paper\n\n\
             ## 1 Introduction\n\n\
             Some body text.\n\n\
             ### 1.1 Background\n\n\
             *Figure 1. A plot.*\n"
        );
    }

    #[test]
    fn figures_render_as_images_with_the_caption_as_alt_text() {
        let mut figure = block(BlockKind::Figure, "Figure 1. A plot.");
        figure.asset = Some("assets/figure-001.png".into());
        let doc = Document {
            title: None,
            blocks: vec![figure],
        };
        assert_eq!(
            render(&doc),
            "![Figure 1. A plot.](assets/figure-001.png)\n"
        );
    }

    #[test]
    fn a_figure_without_an_extracted_image_still_appears() {
        let doc = Document {
            title: None,
            blocks: vec![block(BlockKind::Figure, "Figure 2. Architecture.")],
        };
        assert_eq!(render(&doc), "*Figure 2. Architecture.*\n");
    }

    #[test]
    fn list_items_are_renumbered_by_markdown() {
        let doc = Document {
            title: None,
            blocks: vec![
                block(BlockKind::ListItem { ordered: true }, "1. first point"),
                block(BlockKind::ListItem { ordered: true }, "2. second point"),
                block(BlockKind::ListItem { ordered: false }, "• a bullet"),
            ],
        };
        assert_eq!(
            render(&doc),
            "1. first point\n\n1. second point\n\n- a bullet\n"
        );
    }

    #[test]
    fn footnotes_render_as_quotes() {
        let doc = Document {
            title: None,
            blocks: vec![block(BlockKind::Footnote, "1 Work done while at X.")],
        };
        assert_eq!(render(&doc), "> 1 Work done while at X.\n");
    }

    #[test]
    fn display_equations_render_as_latex() {
        let mut b = block(BlockKind::Equation, "");
        b.math = Some(crate::doc::MathData {
            latex: r"E=mc^2".into(),
            number: None,
            confidence: 1.0,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        assert_eq!(render(&doc), "$$\nE=mc^2\n$$\n");
    }

    #[test]
    fn a_numbered_equation_carries_its_tag() {
        let mut b = block(BlockKind::Equation, "");
        b.math = Some(crate::doc::MathData {
            latex: "a=b".into(),
            number: Some("3".into()),
            confidence: 1.0,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        assert!(render(&doc).contains(r"a=b \tag{3}"));
    }

    /// An uncertain reconstruction is shown, not asserted.
    #[test]
    fn a_low_confidence_equation_falls_back_to_a_picture() {
        let mut b = block(BlockKind::Equation, "");
        b.math = Some(crate::doc::MathData {
            latex: "garbled".into(),
            number: None,
            confidence: 0.2,
        });
        b.asset = Some("assets/equation-004.png".into());
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        assert_eq!(render(&doc), "![equation](assets/equation-004.png)\n");
    }

    #[test]
    fn tables_render_as_gfm() {
        let mut b = block(BlockKind::Table, "");
        b.table = Some(crate::doc::TableData {
            rows: vec![
                vec!["Model".into(), "BLEU".into()],
                vec!["ByteNet".into(), "23.75".into()],
                vec!["GNMT".into(), "24.61".into()],
            ],
            header_rows: 1,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        assert_eq!(
            render(&doc),
            "| Model | BLEU |\n| --- | --- |\n| ByteNet | 23.75 |\n| GNMT | 24.61 |\n"
        );
    }

    #[test]
    fn a_table_without_a_header_gets_an_empty_one() {
        let mut b = block(BlockKind::Table, "");
        b.table = Some(crate::doc::TableData {
            rows: vec![vec!["a".into(), "b".into()], vec!["c".into(), "d".into()]],
            header_rows: 0,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        // GFM needs a header row even when the table has none.
        assert!(render(&doc).starts_with("|  |  |\n| --- | --- |\n"));
    }

    #[test]
    fn pipes_inside_cells_are_escaped() {
        let mut b = block(BlockKind::Table, "");
        b.table = Some(crate::doc::TableData {
            rows: vec![vec!["a|b".into(), "c".into()]],
            header_rows: 1,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        assert!(render(&doc).contains("a\\|b"));
    }

    #[test]
    fn line_leading_markers_are_escaped() {
        let doc = Document {
            title: None,
            blocks: vec![block(BlockKind::Paragraph, "- not a list item")],
        };
        assert_eq!(render(&doc), "\\- not a list item\n");
    }

    #[test]
    fn inline_punctuation_is_left_alone() {
        let doc = Document {
            title: None,
            blocks: vec![block(BlockKind::Paragraph, "we set x*y and a_b in place")],
        };
        assert_eq!(render(&doc), "we set x*y and a_b in place\n");
    }

    #[test]
    fn an_empty_document_renders_to_a_newline() {
        assert_eq!(render(&Document::default()), "\n");
    }
}
