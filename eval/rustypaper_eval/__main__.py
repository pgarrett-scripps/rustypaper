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
        help="a previous --json report; fail if any paper regresses against it in bigram "
        "recall, equation recall or equation fidelity (by more than 0.005), loses tables, "
        "or stops being scored at all",
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
        )
        report["papers"][paper.pdf.name] = row

    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        _print_table(report)

    if args.baseline:
        return _check_regressions(
            report,
            json.loads(args.baseline.read_text()),
            check_missing=args.only is None,
        )
    return 0


def _print_table(report: dict) -> None:
    print(f"converter={report['backend']}  scorer={report['scorer']}")
    print(
        f"{'paper':<16} {'bigram':>7} {'cover':>6} "
        f"{'eq':>4} {'eq rec':>7} {'eq fid':>7} {'tables':>8} {'sec':>6}"
    )
    print("-" * 72)
    for name, row in sorted(report["papers"].items()):
        tables = f"{row['tables_found']}/{row['tables']}"
        print(
            f"{name:<16} {row['bigram_recall']:>7.3f} {row['coverage']:>6.3f} "
            f"{row['equations']:>4} {row['equation_recall']:>7.3f} "
            f"{row['equation_fidelity']:>7.3f} {tables:>8} {row['seconds']:>6.2f}"
        )

    rows = list(report["papers"].values())
    if rows:
        with_maths = [r for r in rows if r["equations"]]
        print("-" * 72)
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


# The per-paper scores the baseline gate holds, and how they print. Movements smaller than the
# tolerance are noise. A metric the baseline does not carry is skipped rather than treated as
# zero, so an older baseline still checks whatever it does record.
GATED_SCORES = (
    ("bigram_recall", "bigram"),
    ("equation_recall", "eq recall"),
    ("equation_fidelity", "eq fid"),
)


def _check_regressions(
    report: dict, baseline: dict, tolerance: float = 0.005, check_missing: bool = True
) -> int:
    """Compare against a stored report. Small movements are noise, not regressions.

    Fails on three kinds of loss: a scored paper's bigram recall, equation recall or equation
    fidelity dropping by more than `tolerance`; the number of tables it found dropping at all;
    and a paper the baseline scored no longer being scored — it stopped converting, or lost its
    ground truth. That last one was the hole: iterating the new report alone means a paper that
    vanished is a paper nobody checks, and a crash reads as a clean run. `check_missing` is off
    for a `--only` run, which never claims to cover the rest of the corpus.
    """
    failed = False
    scored = report.get("papers", {})
    unscorable = report.get("unscorable", {})
    was_scored = baseline.get("papers", {})

    if check_missing:
        for name in sorted(was_scored):
            if name in scored:
                continue
            if name in unscorable:
                reason = unscorable[name].get("reason", "no reason given")
                print(f"  REGRESS {name}: was scored, now unscorable ({reason})")
            else:
                print(f"  REGRESS {name}: in the baseline, absent from this run")
            failed = True

    for name, row in sorted(scored.items()):
        before = was_scored.get(name)
        if before is None:
            print(f"  new     {name}")
            continue
        for field, label in GATED_SCORES:
            if field not in before or field not in row:
                continue
            delta = row[field] - before[field]
            if delta < -tolerance:
                print(f"  REGRESS {name} {label}: {before[field]:.3f} -> {row[field]:.3f}")
                failed = True
            elif delta > tolerance:
                print(f"  improve {name} {label}: {before[field]:.3f} -> {row[field]:.3f}")
        if "tables_found" in before and "tables_found" in row:
            # A count, not a match, so only a fall is evidence of anything.
            if row["tables_found"] < before["tables_found"]:
                print(
                    f"  REGRESS {name} tables: "
                    f"{before['tables_found']} -> {row['tables_found']}"
                )
                failed = True
            elif row["tables_found"] > before["tables_found"]:
                print(
                    f"  improve {name} tables: "
                    f"{before['tables_found']} -> {row['tables_found']}"
                )
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
