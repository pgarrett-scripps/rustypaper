"""Self-tests for the harness. The harness measures the converter, so it has to be trustworthy.

    python3 -m unittest discover -s eval/tests
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from rustypaper_eval import corpus, detex, formula, score  # noqa: E402
from rustypaper_eval.__main__ import _check_regressions, _section_titles  # noqa: E402


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

    def test_a_discard_macro_discards_its_argument(self):
        source = r"""
        \documentclass{article}
        \newcommand{\cut}[1]{}
        \begin{document}
        Kept prose. \cut{Dropped words with {nested \emph{braces}} inside.} More kept.
        A \cutting edge remains, since that is a different macro.
        \end{document}
        """
        text = detex.strip(source)
        self.assertIn("Kept prose", text)
        self.assertIn("More kept", text)
        self.assertIn("edge remains", text)
        for absent in ["Dropped", "nested", "braces", "inside"]:
            self.assertNotIn(absent, text, f"{absent!r} survived")

    def test_a_macro_with_a_body_is_not_a_discard(self):
        source = (
            r"\newcommand{\keepit}[1]{#1}"
            r"\begin{document}\keepit{Visible words.}\end{document}"
        )
        self.assertIn("Visible words", detex.strip(source))

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

    def test_maths_is_stripped_from_both_sides(self):
        # detex removes formulae from the reference, so recovering them must not be punished.
        self.assertEqual(score.normalise("we set $x^2$ here"), ["we", "set", "here"])
        self.assertEqual(score.normalise("a $$E=mc^2$$ b"), ["a", "b"])
        self.assertEqual(
            score.bigram_recall("the value is bounded", "the value $\\alpha$ is bounded"),
            1.0,
        )

    def test_empty_reference_is_zero_not_an_error(self):
        self.assertEqual(score.bigram_recall("", "anything"), 0.0)
        self.assertEqual(score.coverage("", "anything"), 0.0)


if __name__ == "__main__":
    unittest.main()


class TestFormula(unittest.TestCase):
    def test_extracts_display_equations_from_source(self):
        source = r"""
        \begin{equation}\label{eq:1} E = mc^2 \end{equation}
        Some prose with $inline$ maths that must not count.
        \begin{align} a &= b + c \\ d &= e - f \end{align}
        """
        found = formula.reference_equations(source)
        self.assertEqual(len(found), 3, found)
        self.assertIn("E=mc^2", found)
        # An align block is several equations, one per row, with alignment stripped.
        self.assertIn("a=b+c", found)
        self.assertIn("d=e-f", found)

    def test_cosmetic_differences_do_not_count(self):
        # What the author wrote versus what reconstruction produces.
        self.assertEqual(
            formula.normalise(r"\left( \frac{x}{y} \right) \, \cdot z"),
            formula.normalise(r"(\frac{x}{y})\cdot z"),
        )

    def test_scores_recall_and_fidelity_separately(self):
        source = r"\begin{equation} x^2 + y^2 = z^2 \end{equation}" \
                 r"\begin{equation} a = b + c \end{equation}"
        # One recovered exactly, one missed entirely.
        result = formula.compare(source, [r"x^{2}+y^{2}=z^{2}"])
        self.assertEqual(result.reference, 2)
        self.assertEqual(result.found, 1)
        self.assertEqual(result.recall, 0.5)
        self.assertGreater(result.fidelity, 0.95)

    def test_a_paper_without_maths_scores_zero_not_an_error(self):
        result = formula.compare("no maths at all here", ["x^2"])
        self.assertEqual(result.reference, 0)
        self.assertEqual(result.recall, 0.0)

    def test_counts_tabular_environments(self):
        source = r"\begin{tabular}{cc} a & b \end{tabular} \begin{tabular*}{c} x \end{tabular*}"
        self.assertEqual(formula.reference_tables(source), 2)


class TestBibliography(unittest.TestCase):
    """The source side of the references metric: how many entries the paper actually has."""

    def test_counts_plain_bibitems(self):
        source = r"""
        \begin{thebibliography}{9}
        \bibitem{he2016} K. He et al. Deep residual learning. CVPR, 2016.
        \bibitem{lecun1998} Y. LeCun et al. Gradient-based learning. IEEE, 1998.
        \end{thebibliography}
        """
        self.assertEqual(formula.reference_bibitems(source), 2)

    def test_counts_natbib_labelled_bibitems(self):
        # What a BibTeX `.bbl` looks like: the printed label comes first, and may itself be
        # braced or carry brackets of its own.
        source = r"""
        \begin{thebibliography}{27}
        \bibitem[He et~al.(2016)He, Zhang]{he2016} Deep residual learning.
        \bibitem[{Kingma and Ba(2015)}]{kingma2015} Adam.
        \bibitem [Smith(1999)] {smith99} A spaced one.
        \end{thebibliography}
        """
        self.assertEqual(formula.reference_bibitems(source), 3)

    def test_commented_out_entries_do_not_count(self):
        source = "\\bibitem{kept} Real.\n% \\bibitem{dropped} Cut before submission.\n"
        self.assertEqual(formula.reference_bibitems(source), 1)

    def test_a_source_without_a_bibliography_counts_none(self):
        # Not an error: a paper that cited with BibTeX and shipped only its `.bib` leaves no
        # entry list, and the harness reports that as absent rather than as zero references.
        self.assertEqual(formula.reference_bibitems(r"\bibliography{refs}"), 0)

    def test_bibliography_file_is_the_largest_list_not_the_sum(self):
        # The main `.tex` keeps a two-entry stub beside the generated `.bbl`; summing would
        # claim 5 entries for a paper that prints 3.
        files = {
            "main.tex": r"\bibitem{a} A. \bibitem{b} B.",
            "main.bbl": r"\bibitem{x} X. \bibitem{y} Y. \bibitem{z} Z.",
        }
        self.assertEqual(formula.reference_bibitems(corpus.select_bibliography(files)), 3)

    def test_a_duplicated_bbl_is_counted_once(self):
        # arXiv archives routinely carry the same bibliography twice under two names.
        entries = r"\bibitem{a} A. \bibitem{b} B."
        files = {"main.bbl": entries, "camera-ready.bbl": entries}
        self.assertEqual(formula.reference_bibitems(corpus.select_bibliography(files)), 2)

    def test_no_bibliography_anywhere_selects_nothing(self):
        files = {"main.tex": r"\bibliography{refs}\bibliographystyle{plain}"}
        self.assertEqual(corpus.select_bibliography(files), "")


class TestSections(unittest.TestCase):
    """The source side of the sections metric, and the rule that matches the two sides up."""

    def test_extracts_sections_and_subsections(self):
        source = r"""
        \section{Introduction}
        \subsection*{A starred one}
        \section[Short running head]{The Full Title}
        \subsubsection{Too deep to count}
        """
        self.assertEqual(
            formula.reference_sections(source),
            ["introduction", "a starred one", "the full title"],
        )

    def test_a_duplicated_source_file_does_not_double_the_outline(self):
        # arXiv archives ship a submission and a camera-ready copy of the same paper.
        source = r"\section{Introduction}\section{Method}" * 2
        self.assertEqual(formula.reference_sections(source), ["introduction", "method"])

    def test_commented_out_sections_do_not_count(self):
        source = "\\section{Kept}\n% \\section{Cut before submission}\n"
        self.assertEqual(formula.reference_sections(source), ["kept"])

    def test_markup_inside_a_title_is_reduced(self):
        source = r"\section{Learning $\alpha$ with \emph{care}}"
        self.assertEqual(formula.reference_sections(source), ["learning with care"])

    def test_numbering_and_case_do_not_have_to_match(self):
        # What the source declares, against what the PDF prints in four different templates.
        source = (
            r"\section{Model Architecture}\section{Theory}"
            r"\section{Proofs}\subsection{Ablation Studies}"
        )
        result = formula.compare_sections(
            source, ["3 Model Architecture", "II. THEORY", "A.1 Proofs", "6.2. Ablation Studies"]
        )
        self.assertEqual((result.reference, result.found), (4, 4))
        self.assertEqual(result.recall, 1.0)

    def test_a_title_beginning_with_an_article_keeps_it(self):
        # `A Survey of Methods` is not section A: a lettered label carries its full stop.
        self.assertEqual(
            formula.normalise_heading("A Survey of Methods"), "a survey of methods"
        )
        self.assertEqual(formula.normalise_heading("A. Further Results"), "further results")

    def test_a_heading_may_carry_more_than_the_source_declares(self):
        # A long title broken across lines, and a heading with a subtitle fused onto it.
        source = r"\section{Results and Discussion}\section{Conclusion}"
        result = formula.compare_sections(
            source, ["6 Results and Discussion of the Ablations", "Conclusion"]
        )
        self.assertEqual(result.found, 2)

    def test_a_short_heading_does_not_match_everything(self):
        # The floor on containment: `of` is inside half an outline and names none of it.
        source = r"\section{Proof of the Main Theorem}"
        self.assertEqual(formula.compare_sections(source, ["of"]).found, 0)

    def test_a_missed_section_is_not_found(self):
        source = r"\section{Introduction}\subsection{GLUE}"
        result = formula.compare_sections(source, ["1 Introduction"])
        self.assertEqual((result.reference, result.found), (2, 1))
        self.assertEqual(result.recall, 0.5)

    def test_a_paper_declaring_no_sections_scores_zero_not_an_error(self):
        result = formula.compare_sections("no sectioning at all", ["Introduction"])
        self.assertEqual((result.reference, result.found), (0, 0))
        self.assertEqual(result.recall, 0.0)


class TestSectionTitles(unittest.TestCase):
    """Reading the titles out of a converted document."""

    def test_the_section_map_is_read_at_every_depth(self):
        document = {
            "blocks": [],
            "sections": [
                {"title": None, "children": []},
                {
                    "title": "1 Introduction",
                    "children": [{"title": "1.1 Background", "children": []}],
                },
            ],
        }
        self.assertEqual(
            sorted(_section_titles(document)), ["1 Introduction", "1.1 Background"]
        )

    def test_a_document_without_a_section_map_falls_back_to_headings(self):
        # An older extension has heading blocks and no `sections` key; scoring it zero would
        # blame the converter for the harness being newer than the binary.
        document = {
            "blocks": [
                {"kind": {"type": "heading", "level": 1}, "text": "1 Introduction"},
                {"kind": {"type": "paragraph"}, "text": "Body."},
            ]
        }
        self.assertEqual(_section_titles(document), ["1 Introduction"])


class TestRegressionGate(unittest.TestCase):
    """The gate has to fail on real losses and stay quiet about what a baseline never recorded."""

    def _report(self, **row) -> dict:
        return {"papers": {"paper.pdf": {"bigram_recall": 0.9, **row}}}

    def test_fewer_references_found_is_a_regression(self):
        report = self._report(references_found=10, references=41)
        baseline = self._report(references_found=15, references=41)
        self.assertEqual(_check_regressions(report, baseline), 1)

    def test_more_references_found_passes(self):
        report = self._report(references_found=41, references=41)
        baseline = self._report(references_found=0, references=41)
        self.assertEqual(_check_regressions(report, baseline), 0)

    def test_a_baseline_without_the_field_is_not_read_as_zero(self):
        # Baselines predate this column. An old one says nothing about references, which must
        # not be mistaken for "it used to find none".
        report = self._report(references_found=0, references=41)
        self.assertEqual(_check_regressions(report, self._report()), 0)

    def test_fewer_sections_found_is_a_regression(self):
        report = self._report(sections_found=9, sections=22)
        baseline = self._report(sections_found=22, sections=22)
        self.assertEqual(_check_regressions(report, baseline), 1)

    def test_more_sections_found_passes(self):
        report = self._report(sections_found=22, sections=22)
        baseline = self._report(sections_found=9, sections=22)
        self.assertEqual(_check_regressions(report, baseline), 0)

    def test_a_baseline_without_sections_is_not_read_as_zero(self):
        # The baseline this column was added against records no sections at all.
        report = self._report(sections_found=0, sections=22)
        self.assertEqual(_check_regressions(report, self._report()), 0)
