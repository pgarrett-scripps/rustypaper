//! Unicode to LaTeX for mathematical symbols.
//!
//! Born-digital PDFs hand us the actual character a glyph stands for, so this is a lookup rather
//! than recognition. pdfium's glyph-name fallback resolves TeX's Computer Modern encodings to
//! sensible Unicode — the corpus yields `α β θ × · √ ≤ ∞ ∈ ∇ ∆ −` and so on — which means the
//! job here is to name them again in LaTeX.

/// The LaTeX for a mathematical character, or `None` when the character can be written as-is.
pub fn latex(c: char) -> Option<&'static str> {
    Some(match c {
        // Greek, lower case.
        'α' => r"\alpha",
        'β' => r"\beta",
        'γ' => r"\gamma",
        'δ' => r"\delta",
        'ε' => r"\epsilon",
        'ϵ' => r"\epsilon",
        'ζ' => r"\zeta",
        'η' => r"\eta",
        'θ' => r"\theta",
        'ϑ' => r"\vartheta",
        'ι' => r"\iota",
        'κ' => r"\kappa",
        'λ' => r"\lambda",
        'μ' | 'µ' => r"\mu",
        'ν' => r"\nu",
        'ξ' => r"\xi",
        'π' => r"\pi",
        'ϖ' => r"\varpi",
        'ρ' => r"\rho",
        'ϱ' => r"\varrho",
        'σ' => r"\sigma",
        'ς' => r"\varsigma",
        'τ' => r"\tau",
        'υ' => r"\upsilon",
        'φ' => r"\phi",
        'ϕ' => r"\varphi",
        'χ' => r"\chi",
        'ψ' => r"\psi",
        'ω' => r"\omega",

        // Greek, upper case.
        'Γ' => r"\Gamma",
        'Δ' | '∆' => r"\Delta",
        'Θ' => r"\Theta",
        'Λ' => r"\Lambda",
        'Ξ' => r"\Xi",
        'Π' => r"\Pi",
        'Σ' => r"\Sigma",
        'Υ' => r"\Upsilon",
        'Φ' => r"\Phi",
        'Ψ' => r"\Psi",
        'Ω' => r"\Omega",

        // Binary operators and relations.
        '−' | '–' => "-",
        '×' => r"\times",
        '÷' => r"\div",
        '±' => r"\pm",
        '∓' => r"\mp",
        '·' | '⋅' => r"\cdot",
        '∗' => r"\ast",
        '∘' => r"\circ",
        '⊕' => r"\oplus",
        '⊗' => r"\otimes",
        '⊙' => r"\odot",
        '≤' | '⩽' => r"\leq",
        '≥' | '⩾' => r"\geq",
        '≠' => r"\neq",
        '≈' => r"\approx",
        '≡' => r"\equiv",
        '∼' | '∽' => r"\sim",
        '≃' => r"\simeq",
        '≅' => r"\cong",
        '∝' => r"\propto",
        '≪' => r"\ll",
        '≫' => r"\gg",
        '≜' => r"\triangleq",
        '≐' => r"\doteq",

        // Set theory and logic.
        '∈' => r"\in",
        '∉' => r"\notin",
        '∋' => r"\ni",
        '⊂' => r"\subset",
        '⊆' => r"\subseteq",
        '⊃' => r"\supset",
        '⊇' => r"\supseteq",
        '∪' => r"\cup",
        '∩' => r"\cap",
        '∅' | '⌀' => r"\emptyset",
        '∀' => r"\forall",
        '∃' => r"\exists",
        '¬' => r"\neg",
        '∧' => r"\wedge",
        '∨' => r"\vee",

        // Arrows.
        '→' => r"\to",
        '←' => r"\leftarrow",
        '↔' => r"\leftrightarrow",
        '⇒' => r"\Rightarrow",
        '⇐' => r"\Leftarrow",
        '⇔' => r"\Leftrightarrow",
        '↦' => r"\mapsto",
        '↑' => r"\uparrow",
        '↓' => r"\downarrow",

        // Large operators. Limits are attached by the reconstruction pass.
        '∑' => r"\sum",
        '∏' => r"\prod",
        '∐' => r"\coprod",
        '∫' => r"\int",
        '∬' => r"\iint",
        '∮' => r"\oint",
        '⋃' => r"\bigcup",
        '⋂' => r"\bigcap",

        // Analysis.
        '∞' => r"\infty",
        '∂' => r"\partial",
        '∇' => r"\nabla",
        '√' => r"\sqrt",
        '∴' => r"\therefore",
        '∵' => r"\because",
        '…' => r"\dots",
        '⋯' => r"\cdots",
        '⋮' => r"\vdots",
        '⋱' => r"\ddots",
        '′' => "'",
        '″' => "''",
        '∥' => r"\parallel",
        '⊥' => r"\perp",
        '∠' => r"\angle",
        '†' => r"\dagger",
        '‡' => r"\ddagger",
        'ℓ' => r"\ell",
        'ℏ' => r"\hbar",
        'ℜ' => r"\Re",
        'ℑ' => r"\Im",
        'ℵ' => r"\aleph",
        '°' => r"^\circ",

        // Blackboard bold, the common ones in papers.
        'ℝ' => r"\mathbb{R}",
        'ℕ' => r"\mathbb{N}",
        'ℤ' => r"\mathbb{Z}",
        'ℚ' => r"\mathbb{Q}",
        'ℂ' => r"\mathbb{C}",
        '𝔼' => r"\mathbb{E}",
        'ℙ' => r"\mathbb{P}",

        // Characters that mean something else in LaTeX source.
        '%' => r"\%",
        '&' => r"\&",
        '#' => r"\#",
        '$' => r"\$",
        '_' => r"\_",
        '{' => r"\{",
        '}' => r"\}",

        _ => return None,
    })
}

/// Whether a character belongs to mathematics wherever it appears.
///
/// Used to seed detection. Deliberately excludes digits, Latin letters and ordinary punctuation,
/// which occur constantly in prose.
pub fn is_math_symbol(c: char) -> bool {
    matches!(c,
        '\u{2190}'..='\u{21FF}'   // arrows
        | '\u{2200}'..='\u{22FF}' // mathematical operators
        | '\u{2300}'..='\u{23FF}' // technical
        | '\u{27C0}'..='\u{27EF}' // supplemental operators A
        | '\u{2900}'..='\u{297F}' // supplemental arrows B
        | '\u{2A00}'..='\u{2AFF}' // supplemental operators B
        | '\u{1D400}'..='\u{1D7FF}' // mathematical alphanumeric symbols
        | '\u{0370}'..='\u{03FF}' // Greek
        | '\u{2100}'..='\u{214F}' // letterlike symbols
    )
}

/// Whether a font name is one that is only ever used for mathematics.
///
/// TeX's maths fonts are the strongest possible signal: `CMMI` is Computer Modern Math Italic
/// and appears nowhere but in formulae.
pub fn is_math_font(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const MARKERS: [&str; 14] = [
        "CMMI",  // Computer Modern math italic
        "CMSY",  // Computer Modern math symbols
        "CMEX",  // Computer Modern math extensions (big operators, grown delimiters)
        "CMBSY", // bold math symbols
        "MSAM",  // AMS symbols A
        "MSBM",  // AMS symbols B (includes blackboard bold)
        "EUFM",  // Euler Fraktur
        "EUSM",  // Euler script
        "RSFS",  // Ralph Smith's formal script
        "STIXMATH",
        "XITSMATH",
        "LMMATH", // Latin Modern Math
        "MATHJAX",
        "MATHITALIC",
    ];
    MARKERS.iter().any(|m| upper.contains(m))
        // TeX Gyre's maths fonts, and anything self-describing.
        || (upper.contains("MATH") && !upper.contains("MATHEMATICA"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_greek_and_operators() {
        assert_eq!(latex('α'), Some(r"\alpha"));
        assert_eq!(latex('Σ'), Some(r"\Sigma"));
        assert_eq!(latex('≤'), Some(r"\leq"));
        assert_eq!(latex('∈'), Some(r"\in"));
    }

    #[test]
    fn ordinary_characters_pass_through() {
        assert_eq!(latex('x'), None);
        assert_eq!(latex('2'), None);
        assert_eq!(latex('+'), None);
        assert_eq!(latex('('), None);
    }

    /// The minus sign in a PDF is U+2212, not the hyphen a reader would type.
    #[test]
    fn the_unicode_minus_becomes_a_hyphen() {
        assert_eq!(latex('−'), Some("-"));
    }

    #[test]
    fn latex_special_characters_are_escaped() {
        assert_eq!(latex('%'), Some(r"\%"));
        assert_eq!(latex('_'), Some(r"\_"));
    }

    #[test]
    fn recognises_tex_maths_fonts() {
        for name in [
            "CMMI10",
            "CMSY7",
            "CMEX10",
            "MSBM10",
            "LMMath10-Regular",
            "XITSMath",
        ] {
            assert!(is_math_font(name), "{name} should be a maths font");
        }
        for name in ["NimbusRomNo9L-Regu", "CMR10", "Times-Roman", "Helvetica"] {
            assert!(!is_math_font(name), "{name} is not a maths font");
        }
    }

    #[test]
    fn symbol_ranges_exclude_prose() {
        assert!(is_math_symbol('∑'));
        assert!(is_math_symbol('α'));
        assert!(is_math_symbol('→'));
        for c in ['a', 'Z', '7', '.', ',', '(', '-'] {
            assert!(!is_math_symbol(c), "{c:?} occurs constantly in prose");
        }
    }
}
