"""The Python entry point must preserve the native CLI's behavior."""

import os
from pathlib import Path
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from smoke_cli import verify


def test_python_module_cli(monkeypatch):
    monkeypatch.setenv("PYTHONPATH", str(ROOT / "python"))
    verify([sys.executable, "-m", "rustypaper"])


def test_cli_returns_to_python_instead_of_exiting():
    script = "\n".join([
        "from rustypaper._rustypaper import _run_cli",
        "assert _run_cli(['rustypaper', '--help']) == 0",
        "assert _run_cli(['rustypaper', 'unknown-command']) == 2",
        "print('host survived')",
    ])
    result = subprocess.run(
        [sys.executable, "-c", script], capture_output=True, text=True,
        env={**os.environ, "PYTHONPATH": str(ROOT / "python")},
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "host survived" in result.stdout


@pytest.mark.parametrize("fmt", ["md", "typst", "text", "json"])
def test_python_matches_native_cli(fmt, paper):
    native = ROOT / "target" / "release" / ("rustypaper.exe" if os.name == "nt" else "rustypaper")
    if not native.is_file():
        pytest.skip("native CLI has not been built")
    args = ["convert", paper, "--format", fmt, "--caveman", "hard"]
    expected = subprocess.check_output([str(native), *args], timeout=30)
    actual = subprocess.check_output(
        [sys.executable, "-m", "rustypaper", *args],
        env={**os.environ, "PYTHONPATH": str(ROOT / "python")}, timeout=30,
    )
    assert actual == expected
