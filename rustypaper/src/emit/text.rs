//! Plain text.
//!
//! For indexing and search, where markup is noise. Structure survives as blank lines and
//! indentation rather than as syntax.

use crate::doc::{Block, BlockKind, Document};

/// Renders a document as plain text.
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
    match &block.kind {
        BlockKind::Table => {
            if let Some(data) = &block.table {
                for row in &data.rows {
                    out.push_str(&row.join("\t"));
                    out.push('\n');
                }
                out.pop();
            }
        }
        BlockKind::Equation => match &block.math {
            Some(math) => out.push_str(&math.latex),
            None => out.push_str(&block.text),
        },
        BlockKind::ListItem { .. } => {
            out.push_str("  ");
            out.push_str(&block.text);
        }
        _ => out.push_str(&block.text),
    }
    out.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::TableData;
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
    fn markup_is_absent() {
        let doc = Document {
            title: None,
            blocks: vec![
                block(BlockKind::Title, "A Paper"),
                block(BlockKind::Heading { level: 1 }, "1 Introduction"),
                block(BlockKind::Paragraph, "Body text."),
            ],
        };
        assert_eq!(render(&doc), "A Paper\n\n1 Introduction\n\nBody text.\n");
    }

    #[test]
    fn tables_become_tab_separated_rows() {
        let mut b = block(BlockKind::Table, "");
        b.table = Some(TableData {
            rows: vec![vec!["a".into(), "b".into()], vec!["c".into(), "d".into()]],
            header_rows: 1,
        });
        let doc = Document {
            title: None,
            blocks: vec![b],
        };
        assert_eq!(render(&doc), "a\tb\nc\td\n");
    }

    #[test]
    fn an_empty_document_renders_to_a_newline() {
        assert_eq!(render(&Document::default()), "\n");
    }
}
