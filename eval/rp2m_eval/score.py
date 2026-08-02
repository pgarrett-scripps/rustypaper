"""Similarity scoring between converter output and reference text."""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass

try:  # pragma: no cover - depends on the environment
    from rapidfuzz.distance import Levenshtein as _rapidfuzz

    BACKEND = "rapidfuzz"
except ImportError:  # pragma: no cover
    _rapidfuzz = None
    BACKEND = "difflib"

if _rapidfuzz is None:
    import difflib


_PUNCT = re.compile(r"[^\w\s]", re.UNICODE)


def normalise(text: str) -> list[str]:
    """Reduce text to a comparable word sequence.

    Case, punctuation and Unicode form are discarded because none of them is what the converter
    is being judged on, and all of them differ for uninteresting reasons between LaTeX source
    and rendered output — a source ``--`` is an en dash on the page, ``\\%`` is ``%``, and so on.
    """
    text = unicodedata.normalize("NFKC", text).lower()
    text = _PUNCT.sub(" ", text)
    return text.split()


@dataclass(frozen=True)
class Score:
    """How close two word sequences are."""

    similarity: float
    """1.0 is identical; 0.0 is nothing in common."""

    reference_words: int
    output_words: int
    backend: str

    @property
    def edit_distance(self) -> float:
        """Normalised edit distance, the complement of similarity."""
        return 1.0 - self.similarity

    def __str__(self) -> str:
        return (
            f"similarity={self.similarity:.3f} "
            f"({self.output_words} vs {self.reference_words} words, {self.backend})"
        )


def compare(reference: str, output: str) -> Score:
    """Score converter `output` against `reference` text."""
    ref = normalise(reference)
    got = normalise(output)

    if not ref:
        return Score(0.0, 0, len(got), BACKEND)

    if _rapidfuzz is not None:
        similarity = _rapidfuzz.normalized_similarity(ref, got)
    else:
        # SequenceMatcher is C-accelerated and close enough for a relative metric. autojunk
        # would discard words appearing in more than 1% of a long document — which is most
        # function words — so it has to be off.
        matcher = difflib.SequenceMatcher(None, ref, got, autojunk=False)
        similarity = matcher.ratio()

    return Score(similarity, len(ref), len(got), BACKEND)


def bigram_recall(reference: str, output: str) -> float:
    """Fraction of the reference's word bigrams that survive in the output.

    This is the primary metric, because plain similarity is unfair here in a specific way: the
    reference reducer deliberately drops tables, figures and the bibliography, so a *correct*
    conversion legitimately contains far more words than the reference and is penalised for it.
    Recall over the reference's own bigrams asks only "did the prose that is here come out
    right, in the right local order", which is the actual question.

    It is sensitive to what matters. A scrambled reading order breaks the bigrams at every
    column boundary; a dropped word breaks the two bigrams around it; extra content costs
    nothing.
    """
    ref = normalise(reference)
    if len(ref) < 2:
        return 0.0
    got = normalise(output)
    got_bigrams = set(zip(got, got[1:]))
    ref_bigrams = set(zip(ref, ref[1:]))
    return len(ref_bigrams & got_bigrams) / len(ref_bigrams)


def coverage(reference: str, output: str) -> float:
    """Fraction of the reference's distinct words that appear in the output.

    Insensitive to ordering, so read alongside :func:`compare`: high coverage with low
    similarity means the words were all found but put in the wrong order — a reading-order
    failure rather than an extraction failure.
    """
    ref = set(normalise(reference))
    if not ref:
        return 0.0
    return len(ref & set(normalise(output))) / len(ref)
