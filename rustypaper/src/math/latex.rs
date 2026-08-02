//! Rendering a reconstructed formula as LaTeX.

use super::build::Node;

/// Renders a formula.
pub fn render(node: &Node) -> String {
    let mut out = String::new();
    write(&mut out, node);
    out.trim().to_owned()
}

fn write(out: &mut String, node: &Node) {
    match node {
        Node::Symbol(text) => out.push_str(text),
        Node::Row(children) => {
            for child in children {
                write(out, child);
            }
        }
        Node::Scripted { base, sup, sub } => {
            write(out, base);
            if let Some(sub) = sub {
                out.push('_');
                script(out, sub);
            }
            if let Some(sup) = sup {
                out.push('^');
                script(out, sup);
            }
        }
        Node::Fraction { num, den } => {
            out.push_str(r"\frac");
            braced(out, num);
            braced(out, den);
        }
        Node::Radical { arg } => {
            out.push_str(r"\sqrt");
            braced(out, arg);
        }
        Node::Operator {
            symbol,
            below,
            above,
        } => {
            out.push_str(symbol.trim_end());
            if let Some(below) = below {
                out.push('_');
                script(out, below);
            }
            if let Some(above) = above {
                out.push('^');
                script(out, above);
            }
            out.push(' ');
        }
        Node::Fenced { open, close, body } => {
            out.push_str(r"\left");
            out.push_str(open);
            write(out, body);
            out.push_str(r"\right");
            out.push_str(close);
        }
    }
}

/// Writes a script, omitting braces where they add nothing.
///
/// `x^2` is what a person would write, not `x^{2}`. Anything longer must be braced, because
/// `x^{10}` and `x^10` mean different things.
fn script(out: &mut String, node: &Node) {
    let inner = render(node);
    if inner.chars().count() == 1 && !inner.starts_with('\\') {
        out.push_str(&inner);
    } else {
        braced_str(out, &inner);
    }
}

/// Writes a command argument, always braced.
///
/// Unlike a script, the brace is not optional here: `\sqrt` followed by a bare `n` reads as the
/// command `\sqrtn`, which does not exist.
fn braced(out: &mut String, node: &Node) {
    braced_str(out, &render(node));
}

fn braced_str(out: &mut String, inner: &str) {
    out.push('{');
    out.push_str(inner);
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(s: &str) -> Node {
        Node::Symbol(s.into())
    }

    #[test]
    fn renders_a_plain_run() {
        assert_eq!(render(&sym("a+b")), "a+b");
    }

    #[test]
    fn single_character_scripts_are_unbraced() {
        let node = Node::Scripted {
            base: Box::new(sym("x")),
            sup: Some(Box::new(sym("2"))),
            sub: None,
        };
        assert_eq!(render(&node), "x^2");
    }

    #[test]
    fn multi_character_scripts_are_braced() {
        let node = Node::Scripted {
            base: Box::new(sym("x")),
            sup: Some(Box::new(sym("10"))),
            sub: None,
        };
        assert_eq!(render(&node), "x^{10}");
    }

    /// Subscript before superscript, which is how it is conventionally written.
    #[test]
    fn both_scripts_are_ordered_conventionally() {
        let node = Node::Scripted {
            base: Box::new(sym("x")),
            sup: Some(Box::new(sym("2"))),
            sub: Some(Box::new(sym("i"))),
        };
        assert_eq!(render(&node), "x_i^2");
    }

    #[test]
    fn fractions_brace_both_parts() {
        let node = Node::Fraction {
            num: Box::new(sym("a+b")),
            den: Box::new(sym("2")),
        };
        assert_eq!(render(&node), r"\frac{a+b}{2}");
    }

    #[test]
    fn operators_carry_their_limits() {
        let node = Node::Operator {
            symbol: r"\sum ".into(),
            below: Some(Box::new(sym("i=1"))),
            above: Some(Box::new(sym("n"))),
        };
        assert_eq!(render(&node), r"\sum_{i=1}^n");
    }

    #[test]
    fn radicals_are_braced() {
        let node = Node::Radical {
            arg: Box::new(sym("x+1")),
        };
        assert_eq!(render(&node), r"\sqrt{x+1}");
    }

    #[test]
    fn fences_grow_with_their_contents() {
        let node = Node::Fenced {
            open: "(".into(),
            close: ")".into(),
            body: Box::new(sym("x")),
        };
        assert_eq!(render(&node), r"\left(x\right)");
    }

    #[test]
    fn nested_structure_composes() {
        // \frac{x^2}{\sqrt{n}}
        let node = Node::Fraction {
            num: Box::new(Node::Scripted {
                base: Box::new(sym("x")),
                sup: Some(Box::new(sym("2"))),
                sub: None,
            }),
            den: Box::new(Node::Radical {
                arg: Box::new(sym("n")),
            }),
        };
        assert_eq!(render(&node), r"\frac{x^2}{\sqrt{n}}");
    }

    #[test]
    fn an_empty_row_renders_to_nothing() {
        assert_eq!(render(&Node::Row(Vec::new())), "");
    }
}
