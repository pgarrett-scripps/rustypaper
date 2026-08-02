"""Score the converter against the corpus."""

from __future__ import annotations

import argparse
import json
import sys
import time
from collections import Counter
from pathlib import Path

from . import convert, corpus, score

REPO = Path(__file__).resolve().parents[2]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="rp2m_eval", description=__doc__)
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

        result = score.compare(paper.reference, markdown)
        row.update(
            bigram_recall=round(score.bigram_recall(paper.reference, markdown), 4),
            coverage=round(score.coverage(paper.reference, markdown), 4),
            similarity=round(result.similarity, 4),
            reference_words=result.reference_words,
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
        f"{'paper':<18} {'bigram':>7} {'cover':>6} {'sim':>6} "
        f"{'words':>13} {'blocks':>7} {'sec':>6}"
    )
    print("-" * 70)
    for name, row in sorted(report["papers"].items()):
        blocks = sum(row["blocks"].values())
        words = f"{row['output_words']}/{row['reference_words']}"
        print(
            f"{name:<18} {row['bigram_recall']:>7.3f} {row['coverage']:>6.3f} "
            f"{row['similarity']:>6.3f} {words:>13} {blocks:>7} {row['seconds']:>6.2f}"
        )

    rows = list(report["papers"].values())
    if rows:
        mean = sum(r["bigram_recall"] for r in rows) / len(rows)
        print("-" * 70)
        print(f"{'mean bigram recall':<18} {mean:>7.3f}")

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
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
