//! Unicode to LaTeX for mathematical symbols.
//!
//! Born-digital PDFs hand us the actual character a glyph stands for, so this is a lookup rather
//! than recognition. Both backends resolve TeX's Computer Modern encodings to sensible Unicode
//! through a glyph-name fallback — the corpus yields `α β θ × · √ ≤ ∞ ∈ ∇ ∆ −` and so on — which
//! means the job here is to name them again in LaTeX.

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

        // Greek, upper case. Half of them have no command, because they are set in the Latin
        // letter they look like: TeX has no `\Alpha`, and an author writes `A`.
        'Α' => "A",
        'Β' => "B",
        'Ε' => "E",
        'Ζ' => "Z",
        'Η' => "H",
        'Ι' => "I",
        'Κ' => "K",
        'Μ' => "M",
        'Ν' => "N",
        'Ο' => "O",
        'Ρ' => "P",
        'Τ' => "T",
        'Χ' => "X",
        'ο' => "o",
        'ϰ' => r"\varkappa",
        'ϴ' => r"\Theta",
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

        _ => return unstyled_latex(c),
    })
}

/// The LaTeX for a styled character from the Mathematical Alphanumeric Symbols block.
fn unstyled_latex(c: char) -> Option<&'static str> {
    let base = unstyle(c)?;
    if base.is_ascii_alphanumeric() {
        return Some(ascii_str(base));
    }
    // Greek, `∇` and `∂` arrive styled too, and are named above under their plain codepoint.
    latex(base)
}

/// A one-character `&'static str` for an ASCII alphanumeric, by slicing a static alphabet.
fn ascii_str(c: char) -> &'static str {
    const ALPHABET: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let at = ALPHABET.find(c).expect("ASCII alphanumeric");
    &ALPHABET[at..=at]
}

/// The unstyled character behind a Mathematical Alphanumeric Symbol.
///
/// A TeX-set paper hands us `α` and `x`, because Computer Modern's maths italic encodes them at
/// their ordinary codepoints and the style lives in the font. A paper set in an OpenType maths
/// font hands us `𝛼` and `𝑥` — U+1D6FC and U+1D465 — where the style lives in the *character*.
/// The two say the same thing, and only the first was previously writable as LaTeX: the biology
/// paper's fifty-two detected equations came out as `𝑏𝐴-(𝛼_1+𝜇_1)𝐸` and matched nothing.
///
/// Style is discarded rather than translated. `\mathit{x}` and `x` are the same formula, and the
/// styles that do carry meaning — blackboard bold and script — are named above for the letters
/// papers actually use them for, which is checked before this is reached.
pub fn unstyle(c: char) -> Option<char> {
    let code = c as u32;
    match code {
        // Latin: fourteen alphabets of A-Z then a-z, each 52 long and aligned to the first.
        // Reserved slots inside them (italic `h`, script `B`, …) are encoded in the Letterlike
        // Symbols block instead and never arrive here.
        0x1D400..=0x1D6A3 => {
            let at = (code - 0x1D400) % 52;
            char::from_u32(if at < 26 {
                'A' as u32 + at
            } else {
                'a' as u32 + at - 26
            })
        }
        0x1D6A4 => Some('i'), // dotless i
        0x1D6A5 => Some('j'),
        // Greek: five alphabets of 58, each capitals, `∇`, smalls, `∂`, then six variants.
        0x1D6A8..=0x1D7C9 => GREEK.chars().nth(((code - 0x1D6A8) % 58) as usize),
        // Digits: five sets of ten.
        0x1D7CE..=0x1D7FF => char::from_u32('0' as u32 + (code - 0x1D7CE) % 10),
        _ => None,
    }
}

/// One styled Greek alphabet, in the order the Unicode block repeats it.
const GREEK: &str = "ΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡϴΣΤΥΦΧΨΩ∇αβγδεζηθικλμνξοπρςστυφχψω∂ϵϑϰϕϱϖ";

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
    const MARKERS: [&str; 20] = [
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
        // The TX/PX families and their newtx/newpx successors, which Times- and Palatino-set
        // papers use for their formulae. Whole papers were invisible without these: medimaging
        // sets every equation in `rtxmi` and `txsy`, and one display equation in eleven was
        // found. Only the maths members are named — `rtxr` is that family's *text* roman, and
        // admitting it would seed on ordinary prose.
        "TXMI", // tx/newtx math italic, as `rtxmi`
        "TXSY", // tx/newtx symbols, as `txsy`, `txsyb`, `ntxsyralt`
        "TXEX", // tx/newtx extensions
        "PXMI", // px/newpx math italic
        "PXSY",
        "PXEX",
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

    /// The TX and newtx families, which every Times-set paper puts its formulae in. Their text
    /// members are named alike and must stay out: `rtxr` is a roman, not a maths font.
    #[test]
    fn recognises_the_tx_maths_fonts() {
        for name in ["rtxmi", "txsy", "txsyb", "ntxsyralt", "txex", "npxmi"] {
            assert!(is_math_font(name), "{name} should be a maths font");
        }
        for name in ["rtxr", "rtxb", "LinLibertineT", "ptmr8t"] {
            assert!(!is_math_font(name), "{name} is not a maths font");
        }
    }

    /// A paper set in an OpenType maths font hands over `𝛼` and `𝑥`, where the style lives in
    /// the character rather than the font. They are the same formula and must be written alike.
    #[test]
    fn styled_alphabets_are_named_unstyled() {
        assert_eq!(latex('\u{1D6FC}'), Some(r"\alpha")); // italic alpha
        assert_eq!(latex('\u{1D465}'), Some("x")); // italic x
        assert_eq!(latex('\u{1D400}'), Some("A")); // bold A
        assert_eq!(latex('\u{1D7D9}'), Some("1")); // double-struck one
        assert_eq!(latex('\u{1D6C1}'), Some(r"\nabla"));
        assert_eq!(latex('\u{1D6DB}'), Some(r"\partial"));
    }

    /// A style that carries meaning is named before it is discarded.
    #[test]
    fn blackboard_bold_keeps_its_command() {
        assert_eq!(latex('𝔼'), Some(r"\mathbb{E}"));
    }

    #[test]
    fn the_styled_greek_alphabet_is_the_length_the_block_repeats_at() {
        assert_eq!(GREEK.chars().count(), 58);
        assert_eq!(unstyle('\u{1D6A8}'), Some('Α')); // bold capital alpha
        assert_eq!(unstyle('\u{1D7C9}'), Some('ϖ')); // last of the last Greek block
        assert_eq!(unstyle('x'), None);
    }

    /// TeX has no `\Alpha`: a capital that looks Latin is written as the Latin letter.
    #[test]
    fn look_alike_greek_capitals_become_latin() {
        assert_eq!(latex('Α'), Some("A"));
        assert_eq!(latex('Ρ'), Some("P"));
        assert_eq!(latex('Ω'), Some(r"\Omega"));
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
