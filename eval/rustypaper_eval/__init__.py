"""Evaluation harness for rustypaper.

Run it as a module:

    python3 -m rustypaper_eval            # score every corpus paper
    python3 -m rustypaper_eval --json     # machine-readable, for CI

Scores are **relative**. The reference text comes from a crude LaTeX reducer, so an absolute
number means little; a number that moved between commits means a lot.
"""

from . import corpus, detex, score

__all__ = ["corpus", "detex", "score"]
