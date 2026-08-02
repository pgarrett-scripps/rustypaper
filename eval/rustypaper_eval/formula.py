"""Scoring mathematics and tables against the LaTeX source.

Prose is scored by comparing word streams, which works because both sides are prose. Formulae
need their own treatment: the reference is LaTeX the author wrote, and the output is LaTeX
reconstructed from glyph positions, so the two agree on content and disagree on everything
cosmetic — spacing, `\\left(` versus `(`, whether a single-character group is braced.

Normalising both sides hard, then matching each reference equation to its best candidate in the
output, gives two numbers worth having:

* **recall** — how many of the paper's display equations were found at all. This is the number
  that says whether detection works.
* **fidelity** — how close the matched ones are. This is the number that says whether
  reconstruction works.

Both are approximate and both are *relative*, for the same reason the prose metric is: the
normaliser is crude, but it is crude identically for every version of the converter.
"""

from __future__ import annotations

import difflib
import re
from dataclasses import dataclass

#: Environments whose bodies are display equations.
_ENVIRONMENTS = (
    "equation", "equation*", "align", "align*", "aligned", "gather", "gather*",
    "multline", "multline*", "eqnarray", "eqnarray*", "displaymath",
)

_ENV_BODY = re.compile(
    r"\\begin\{(" + "|".join(re.escape(e) for e in _ENVIRONMENTS) + r")\}(.*?)\\end\{\1\}",
    re.DOTALL,
)
_BRACKET_MATH = re.compile(r"\\\[(.*?)\\\]", re.DOTALL)
_TABULAR = re.compile(r"\\begin\{tabular\*?\}", re.DOTALL)

#: Commands that affect only appearance, and their arguments where they carry none.
_COSMETIC = re.compile(
    r"\\(?:left|right|displaystyle|textstyle|scriptstyle|nonumber|notag|limits|nolimits"
    r"|quad|qquad|thinspace|medspace|negthinspace|ensuremath|mathrm|mathit|mathbf"
    r"|text|mbox|operatorname|big|Big|bigg|Bigg|bigl|bigr|Bigl|Bigr)\b"
)

#: Spacing commands are punctuation, so a word boundary never follows them and they need their
#: own pattern: `\,` and `\;` are as cosmetic as `\quad`.
_SPACING = re.compile(r"\\[,;:!>]")
_LABELS = re.compile(r"\\(?:label|tag|intertext)\s*\{[^{}]*\}")


def normalise(latex: str) -> str:
    """Reduce a formula to its content, discarding everything cosmetic."""
    text = _LABELS.sub("", latex)
    text = _SPACING.sub("", text)
    text = _COSMETIC.sub("", text)
    # Alignment and line breaks inside an environment are layout, not content.
    text = text.replace("\\\\", " ").replace("&", "")
    text = re.sub(r"[{}\s]", "", text)
    # `\left(` and `(` must compare equal, and so must `\cdot` and the character it prints.
    return text


def reference_equations(source: str) -> list[str]:
    """Every display equation in a paper's LaTeX source."""
    found = [body for _, body in _ENV_BODY.findall(source)]
    found += _BRACKET_MATH.findall(source)

    equations = []
    for body in found:
        # An `align` block holds several equations, one per row.
        for row in body.split("\\\\"):
            cleaned = normalise(row)
            # Anything this short is a fragment, not an equation worth scoring. `a=b+c` is a
            # real equation at five characters, so the floor has to sit below that.
            if len(cleaned) >= MIN_EQUATION_LENGTH:
                equations.append(cleaned)
    return equations


def reference_tables(source: str) -> int:
    """How many tabular environments the source contains."""
    return len(_TABULAR.findall(source))


@dataclass(frozen=True)
class FormulaScore:
    reference: int
    """Display equations in the source."""

    found: int
    """Of those, how many have a plausible counterpart in the output."""

    fidelity: float
    """Mean similarity of the matched ones; 1.0 is character-identical after normalising."""

    @property
    def recall(self) -> float:
        return self.found / self.reference if self.reference else 0.0


#: Below this similarity, two formulae are different formulae rather than a poor reconstruction.
MATCH_THRESHOLD = 0.5

#: Shortest normalised string still worth calling an equation.
MIN_EQUATION_LENGTH = 4


def compare(source: str, emitted: list[str]) -> FormulaScore:
    """Score emitted LaTeX against the equations in a paper's source."""
    reference = reference_equations(source)
    if not reference:
        return FormulaScore(0, 0, 0.0)

    candidates = [normalise(e) for e in emitted]
    candidates = [c for c in candidates if len(c) >= 3]

    matched, scores = 0, []
    for wanted in reference:
        best = 0.0
        for candidate in candidates:
            # Length is a cheap prefilter: sequence matching is quadratic and most pairs are
            # nowhere near each other.
            if not 0.4 <= len(candidate) / max(len(wanted), 1) <= 2.5:
                continue
            ratio = difflib.SequenceMatcher(None, wanted, candidate, autojunk=False).ratio()
            best = max(best, ratio)
            if best > 0.98:
                break
        if best >= MATCH_THRESHOLD:
            matched += 1
            scores.append(best)

    fidelity = sum(scores) / len(scores) if scores else 0.0
    return FormulaScore(len(reference), matched, fidelity)
