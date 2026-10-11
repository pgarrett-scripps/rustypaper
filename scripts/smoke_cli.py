"""Exercise a packaged CLI using only the Python standard library."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "python" / "tests" / "fixtures"


def verify(command):
    """Check commands, output files, diagnostics, and exit codes outside the source tree."""
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        first = work / "paper with spaces.pdf"
        second = work / "second.pdf"
        first.write_bytes((FIXTURES / "single-column.pdf").read_bytes())
        second.write_bytes((FIXTURES / "two-column.pdf").read_bytes())

        def run(*args, code=0):
            result = subprocess.run(
                [*command, *map(str, args)], cwd=work,
                capture_output=True, text=True, encoding="utf-8", timeout=60,
            )
            assert result.returncode == code, (args, result.returncode, result.stderr)
            return result.stdout

        assert "convert" in run("--help")
        version = run("--version").strip()
        assert version.startswith("rustypaper "), version
        run(code=2)
        run("unknown-command", code=2)
        run("convert", first, "--format", "unknown", code=2)
        run("convert", "missing.pdf", code=1)

        for fmt in ("md", "typst", "text", "json"):
            output = run("convert", first, "--format", fmt)
            assert "Reliable Widget Measurements" in output, fmt
            if fmt == "json":
                assert json.loads(output)["blocks"]
            destination = work / ("output." + fmt)
            assert not run("convert", first, "-f", fmt, "-o", destination)
            assert destination.read_text(encoding="utf-8") == output

        for level in ("light", "hard"):
            assert json.loads(run("convert", first, "-f", "json", "--caveman", level))["blocks"]

        run("convert", first, second, code=1)
        batch = work / "batch"
        run("convert", first, second, "-o", batch, "--assets", work / "assets", "--figure-dpi", "72")
        assert (batch / "paper with spaces.md").is_file()
        assert (batch / "second.md").is_file()
        partial = work / "partial"
        run("convert", "missing.pdf", second, "-o", partial, code=1)
        assert (partial / "second.md").is_file()

        assert json.loads(run("dump", first))["pages"]
        assert json.loads(run("dump", first, "--page", "0", "--pretty"))
        assert "pages," in run("probe", first, "--pages")
        assert "[y=" in run("text", first, "--page", "0", "--geometry")

        # A closed consumer must not turn successful conversion into a failed command.
        for args in (("convert", first), ("dump", first, "--pretty")):
            with subprocess.Popen(
                [*command, *map(str, args)], cwd=work,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            ) as process:
                process.stdout.close()
                process.stdout = None
                _, stderr = process.communicate(timeout=60)
                assert process.returncode == 0, stderr
    print("CLI smoke checks passed: " + " ".join(command))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit("usage: smoke_cli.py <executable> [prefix arguments]")
    verify(sys.argv[1:])
