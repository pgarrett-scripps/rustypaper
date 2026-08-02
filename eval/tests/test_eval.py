"""Self-tests for the harness. The harness measures the converter, so it has to be trustworthy.

    python3 -m unittest discover -s eval/tests
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from rp2m_eval import detex, score  # noqa: E402


class TestDetex(unittest.TestCase):
    def test_keeps_prose_and_drops_markup(self):
        source = r"""
        \documentclass{article}
        \usepackage{graphicx}
        \begin{document}
        \section{Introduction}
        Deep networks are hard to train~\cite{he2016}. % a comment
        \begin{equation}\label{eq:1} x^2 + y^2 = z^2 \end{equation}
        We show that $\alpha$ matters.
        \end{document}
        """
        text = detex.strip(source)
        self.assertIn("Deep networks are hard to train", text)
        self.assertIn("Introduction", text)
        self.assertIn("We show that", text)
        for absent in ["cite", "he2016", "equation", "alpha", "comment", "usepackage"]:
            self.assertNotIn(absent, text, f"{absent!r} survived")

    def test_preamble_is_excluded(self):
        source = r"\documentclass{article}\title{Secret}\begin{document}Body.\end{document}"
        self.assertNotIn("Secret", detex.strip(source))
        self.assertIn("Body", detex.strip(source))

    def test_accents_reduce_to_their_letter(self):
        self.assertIn("Muller", detex.strip(r"\begin{document}M\"{u}ller\end{document}"))

    def test_main_source_is_the_one_with_a_document(self):
        files = {
            "macros.tex": r"\newcommand{\x}{y}",
            "paper.tex": r"\begin{document}hello\end{document}",
        }
        self.assertEqual(detex.find_main_source(files), "paper.tex")

    def test_inputs_are_spliced(self):
        files = {
            "main.tex": r"\begin{document}A \input{sec1} C\end{document}",
            "sec1.tex": "B",
        }
        self.assertIn("A B C", detex.strip(detex.inline_inputs(files, "main.tex")))


class TestScore(unittest.TestCase):
    def test_identical_text_scores_one(self):
        text = "the quick brown fox jumps over the lazy dog"
        self.assertEqual(score.bigram_recall(text, text), 1.0)
        self.assertEqual(score.coverage(text, text), 1.0)

    def test_normalisation_ignores_case_and_punctuation(self):
        self.assertEqual(
            score.bigram_recall("The quick, brown fox!", "the quick brown fox"), 1.0
        )

    def test_extra_output_does_not_reduce_recall(self):
        # The whole point: a correct conversion has tables and references the reference lacks.
        reference = "alpha beta gamma"
        output = "preamble text alpha beta gamma and a bibliography follows"
        self.assertEqual(score.bigram_recall(reference, output), 1.0)
        self.assertLess(score.compare(reference, output).similarity, 0.7)

    def test_scrambled_order_is_penalised(self):
        reference = "alpha beta gamma delta epsilon"
        scrambled = "delta epsilon alpha beta gamma"
        self.assertEqual(score.coverage(reference, scrambled), 1.0)
        self.assertLess(score.bigram_recall(reference, scrambled), 1.0)

    def test_interleaved_columns_are_penalised(self):
        # What a reading-order failure looks like: two columns zipped line by line.
        reference = "one two three four five six"
        interleaved = "one four two five three six"
        self.assertEqual(score.coverage(reference, interleaved), 1.0)
        self.assertLess(score.bigram_recall(reference, interleaved), 0.25)

    def test_empty_reference_is_zero_not_an_error(self):
        self.assertEqual(score.bigram_recall("", "anything"), 0.0)
        self.assertEqual(score.coverage("", "anything"), 0.0)


if __name__ == "__main__":
    unittest.main()
