"""Command line entry point backed by the shared Rust implementation."""

import sys

from ._rustypaper import _run_cli


def main():
    """Return the native CLI's exit status without exiting the Python host."""
    return _run_cli(["rustypaper", *sys.argv[1:]])
