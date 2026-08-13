"""A crude LaTeX-to-text reducer, for building reference text from arXiv sources.

This is deliberately not a LaTeX implementation. It does not expand macros, follow ``\\input``
semantics beyond textual inclusion, or understand packages. It exists to turn a paper's source
into a bag of prose words that can be compared against a converter's output.

That is enough because **the metric is relative**. A score here is meaningful compared against
the same score on another commit, not as a claim about absolute accuracy. Anything this reducer
gets wrong, it gets wrong identically for every version of the converter, so regressions still
show up. Treat an absolute score as a number with no units.
"""

from __future__ import annotations

import re

# Environments whose contents are not prose and would only add noise on both sides.
DROPPED_ENVIRONMENTS = (
    "equation", "equation*", "align", "align*", "eqnarray", "eqnarray*",
    "gather", "gather*", "multline", "multline*", "array", "matrix",
    "tabular", "tabular*", "tabularx", "table", "table*", "figure", "figure*",
    "algorithm", "algorithmic", "lstlisting", "verbatim", "tikzpicture",
    "thebibliography", "filecontents",
)

# Commands whose braced argument is *not* prose and should vanish with it.
DROPPED_WITH_ARGUMENT = (
    "cite", "citep", "citet", "citeauthor", "citeyear", "ref", "eqref", "pageref",
    "label", "bibliography", "bibliographystyle", "usepackage", "documentclass",
    "includegraphics", "input", "include", "hspace", "vspace", "setlength",
    "newcommand", "renewcommand", "def", "graphicspath", "url", "href",
)

_COMMENT = re.compile(r"(?<!\\)%.*?$", re.MULTILINE)
# A macro defined with an empty body discards its argument, so the reference must discard it
# too: pinsage wraps a third of its prose in `\cut{...}`, none of which the PDF prints, and
# scoring against it made the best-converted paper in the corpus read as the worst.
_DISCARD_DEF = re.compile(
    r"\\(?:re)?newcommand\*?\s*\{?\\([a-zA-Z@]+)\}?\s*\[[0-9]\]\s*\{\s*\}"
)
_DROPPED_ENV = re.compile(
    r"\\begin\{(" + "|".join(re.escape(e) for e in DROPPED_ENVIRONMENTS) + r")\}"
    r".*?\\end\{\1\}",
    re.DOTALL,
)
_DROPPED_CMD = re.compile(
    r"\\(?:" + "|".join(re.escape(c) for c in DROPPED_WITH_ARGUMENT) + r")\s*"
    r"(?:\[[^\]]*\])?(?:\{[^{}]*\})*",
)
_INLINE_MATH = re.compile(r"(?<!\\)\$\$?.*?(?<!\\)\$\$?", re.DOTALL)
_DISPLAY_MATH = re.compile(r"\\\[.*?\\\]", re.DOTALL)
_ANY_COMMAND = re.compile(r"\\[a-zA-Z@]+\*?\s*(?:\[[^\]]*\])?")
_ESCAPED = re.compile(r"\\([%&_#${}])")
_ACCENT = re.compile(r'\\[\'"`^~=.]\{?([a-zA-Z])\}?')


def _drop_balanced(text: str, name: str) -> str:
    """Remove every ``\\name{...}`` with its full brace-balanced argument."""
    needle = "\\" + name
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        j = text.find(needle, i)
        if j < 0:
            out.append(text[i:])
            break
        k = j + len(needle)
        if k < n and (text[k].isalpha() or text[k] == "@"):
            # A longer macro name that merely starts with this one.
            out.append(text[i:k])
            i = k
            continue
        out.append(text[i:j])
        while k < n and text[k].isspace():
            k += 1
        if k < n and text[k] == "{":
            depth = 0
            while k < n:
                if text[k] == "{":
                    depth += 1
                elif text[k] == "}":
                    depth -= 1
                    if depth == 0:
                        k += 1
                        break
                k += 1
        out.append(" ")
        i = k
    return "".join(out)


def strip(source: str) -> str:
    """Reduce LaTeX source to prose words."""
    text = source

    # Discard-macro definitions live in the preamble, so read them before it goes.
    discards = set(_DISCARD_DEF.findall(_COMMENT.sub("", text)))

    # Preamble carries no prose; keep only the body when there is one.
    body = re.search(r"\\begin\{document\}(.*?)\\end\{document\}", text, re.DOTALL)
    if body:
        text = body.group(1)

    text = _COMMENT.sub("", text)
    for name in discards:
        text = _drop_balanced(text, name)
    # Repeat: environments nest, and one pass only removes the outermost match.
    for _ in range(4):
        reduced = _DROPPED_ENV.sub(" ", text)
        if reduced == text:
            break
        text = reduced

    text = _DISPLAY_MATH.sub(" ", text)
    text = _INLINE_MATH.sub(" ", text)
    text = _DROPPED_CMD.sub(" ", text)
    text = _ACCENT.sub(r"\1", text)
    text = _ANY_COMMAND.sub(" ", text)
    text = _ESCAPED.sub(r"\1", text)
    text = text.replace("{", " ").replace("}", " ").replace("~", " ")
    text = text.replace("``", '"').replace("''", '"')

    return re.sub(r"\s+", " ", text).strip()


def find_main_source(files: dict[str, str]) -> str | None:
    """Pick the top-level `.tex` file from an unpacked arXiv source archive.

    The main file is the one containing ``\\begin{document}``; where several do, the longest
    wins, which handles a source tree that also ships a rebuttal or a supplement.
    """
    candidates = [
        (len(content), name)
        for name, content in files.items()
        if name.endswith(".tex") and r"\begin{document}" in content
    ]
    if not candidates:
        return None
    return max(candidates)[1]


def inline_inputs(files: dict[str, str], main: str, depth: int = 0) -> str:
    """Textually splice ``\\input`` and ``\\include`` files into the main source."""
    if depth > 4:
        return files.get(main, "")

    def replace(match: re.Match[str]) -> str:
        target = match.group(1).strip()
        for name in (target, target + ".tex"):
            if name in files:
                return inline_inputs(files, name, depth + 1)
        return " "

    return re.sub(
        r"\\(?:input|include)\s*\{([^{}]*)\}",
        replace,
        files.get(main, ""),
    )
