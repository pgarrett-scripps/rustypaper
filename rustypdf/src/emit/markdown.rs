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
        BlockKind::Paragraph => out.push_str(&escape(&block.text)),
    }
    out.push_str("\n\n");
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
