"""Score the converter against the corpus."""

from __future__ import annotations

import argparse
import json
import sys
import time
from collections import Counter
from pathlib import Path

from . import convert, corpus, formula, score

REPO = Path(__file__).resolve().parents[2]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="rustypaper_eval", description=__doc__)
    parser.add_argument("--corpus", type=Path, default=REPO / "corpus")
    parser.add_argument("--cache", type=Path, default=REPO / "corpus" / ".sources")
    parser.add_argument("--only", help="substring of a paper filename")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    parser.add_argument(
        "--baseline",
        type=Path,
        help="a previous --json report; fail if any paper regresses against it",
    )
    args = parser.parse_args(argv)

    papers = corpus.load(args.corpus, args.cache, args.only)
    if not papers:
        print(
            f"no corpus papers under {args.corpus} (run scripts/fetch-corpus.sh)",
            file=sys.stderr,
        )
        return 2

    report = {
        "backend": convert.BACKEND,
        "scorer": score.BACKEND,
        "papers": {},
        "unscorable": {},
    }

    for paper in papers:
        started = time.perf_counter()
        markdown = convert.to_markdown(paper.pdf)
        document = convert.to_document(paper.pdf)
        elapsed = time.perf_counter() - started

        kinds = Counter(block["kind"]["type"] for block in document["blocks"])
        row = {
            "arxiv_id": paper.arxiv_id,
            "output_words": len(score.normalise(markdown)),
            "blocks": dict(kinds),
            "seconds": round(elapsed, 3),
        }

        if not paper.scorable:
            # Converted fine, but there is nothing to score it against. Reporting it as a bad
            # score would be worse than reporting it as unknown.
            row["reason"] = "arXiv source carries no LaTeX prose"
            report["unscorable"][paper.pdf.name] = row
            continue

        emitted = [b["math"]["latex"] for b in document["blocks"] if b.get("math")]
        maths = formula.compare(paper.source, emitted)
        tables_found = sum(1 for b in document["blocks"] if b.get("table"))
        tables_wanted = formula.reference_tables(paper.source)

        # A paper whose source declares no entries anywhere gets no number rather than a zero:
        # nothing was measured, and `0` would read as "this paper cites nothing".
        refs_wanted = formula.reference_bibitems(paper.bibliography) or None
        refs_found = kinds.get("reference", 0)

        result = score.compare(paper.reference, markdown)
        row.update(
            bigram_recall=round(score.bigram_recall(paper.reference, markdown), 4),
            coverage=round(score.coverage(paper.reference, markdown), 4),
            similarity=round(result.similarity, 4),
            reference_words=result.reference_words,
            equations=maths.reference,
            equation_recall=round(maths.recall, 4),
            equation_fidelity=round(maths.fidelity, 4),
            tables=tables_wanted,
            tables_found=tables_found,
            references=refs_wanted,
            references_found=refs_found,
        )
        report["papers"][paper.pdf.name] = row

    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        _print_table(report)

    if args.baseline:
        return _check_regressions(report, json.loads(args.baseline.read_text()))
    return 0


def _print_table(report: dict) -> None:
    print(f"converter={report['backend']}  scorer={report['scorer']}")
    print(
        f"{'paper':<16} {'bigram':>7} {'cover':>6} "
        f"{'eq':>4} {'eq rec':>7} {'eq fid':>7} {'tables':>8} {'refs':>8} {'sec':>6}"
    )
    print("-" * 81)
    for name, row in sorted(report["papers"].items()):
        tables = f"{row['tables_found']}/{row['tables']}"
        # A paper whose source declares no bibliography is reported as unknown, not as zero.
        wanted = row.get("references")
        refs = f"{row.get('references_found', 0)}/{wanted if wanted is not None else '?'}"
        print(
            f"{name:<16} {row['bigram_recall']:>7.3f} {row['coverage']:>6.3f} "
            f"{row['equations']:>4} {row['equation_recall']:>7.3f} "
            f"{row['equation_fidelity']:>7.3f} {tables:>8} {refs:>8} {row['seconds']:>6.2f}"
        )

    rows = list(report["papers"].values())
    if rows:
        with_maths = [r for r in rows if r["equations"]]
        print("-" * 81)
        print(f"{'mean':<16} {sum(r['bigram_recall'] for r in rows) / len(rows):>7.3f}", end="")
        if with_maths:
            recall = sum(r["equation_recall"] for r in with_maths) / len(with_maths)
            fidelity = sum(r["equation_fidelity"] for r in with_maths) / len(with_maths)
            print(f" {'':>6} {'':>4} {recall:>7.3f} {fidelity:>7.3f}")
        else:
            print()

    for name, row in sorted(report.get("unscorable", {}).items()):
        print(f"\n  skipped {name}: {row['reason']} "
              f"(converted to {row['output_words']} words, "
              f"{sum(row['blocks'].values())} blocks)")


def _check_regressions(report: dict, baseline: dict, tolerance: float = 0.005) -> int:
    """Compare against a stored report. Small movements are noise, not regressions."""
    failed = False
    for name, row in sorted(report["papers"].items()):
        before = baseline.get("papers", {}).get(name)
        if before is None:
            print(f"  new     {name}")
            continue
        delta = row["bigram_recall"] - before["bigram_recall"]
        if delta < -tolerance:
            print(
                f"  REGRESS {name}: "
                f"{before['bigram_recall']:.3f} -> {row['bigram_recall']:.3f}"
            )
            failed = True
        elif delta > tolerance:
            print(
                f"  improve {name}: "
                f"{before['bigram_recall']:.3f} -> {row['bigram_recall']:.3f}"
            )

        # References are a count, so any decrease is a real one; no tolerance applies. A
        # baseline recorded before this column existed simply says nothing about them, and
        # must not be read as zero — older baselines have to keep passing.
        found_before = before.get("references_found")
        found_now = row.get("references_found", 0)
        if found_before is not None and found_now < found_before:
            print(f"  REGRESS {name}: references found {found_before} -> {found_now}")
            failed = True
        elif found_before is not None and found_now > found_before:
            print(f"  improve {name}: references found {found_before} -> {found_now}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
