"""The evaluation corpus: PDFs and their arXiv LaTeX sources.

Ground truth for a PDF converter is normally hand-annotated and therefore scarce. arXiv gives it
away: every paper ships its LaTeX source, so the prose the PDF was rendered *from* is available
for free, at any scale, for exactly the document class this project targets.
"""

from __future__ import annotations

import io
import tarfile
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path

from . import detex

USER_AGENT = "rustypdf-eval/0.1 (https://github.com/pgarrett-scripps/rustypdf2markdown)"

#: arXiv id -> local pdf name. Pinned to a version so scores stay comparable across commits.
PAPERS: dict[str, str] = {
    # Machine learning preprints.
    "1706.03762v7": "transformer.pdf",
    "1512.03385v1": "resnet.pdf",
    "1412.6980v9": "adam.pdf",
    "1810.04805v2": "bert.pdf",
    # Other fields and other LaTeX classes, so that the converter is not tuned to the handful of
    # conference templates machine learning happens to use.
    "2607.28606v1": "numbertheory.pdf",
    "2607.28558v1": "optics.pdf",
    "2607.28053v1": "biology.pdf",
    "2607.28455v1": "statistics.pdf",
    # Older submissions, built by older toolchains.
    "1505.04597v1": "unet.pdf",
    "1406.2661v1": "gan.pdf",
}


#: Below this many words, a "source" carries no prose worth scoring against. Some arXiv
#: submissions are a PDF with a one-line LaTeX wrapper around `\includepdf`, which yields a
#: few hundred characters of preamble and nothing else. Scoring against that produces a
#: meaningless near-zero rather than an honest "unknown".
MIN_REFERENCE_WORDS = 500


@dataclass(frozen=True)
class Paper:
    arxiv_id: str
    pdf: Path
    reference: str
    """Prose extracted from the LaTeX source."""

    source: str = ""
    """The raw LaTeX, for scoring formulae and tables against."""

    @property
    def scorable(self) -> bool:
        """Whether this paper has enough reference prose to score against."""
        return len(self.reference.split()) >= MIN_REFERENCE_WORDS


def _fetch(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def fetch_source(arxiv_id: str, cache: Path) -> Path:
    """Download a paper's e-print archive, returning the cached path."""
    cache.mkdir(parents=True, exist_ok=True)
    target = cache / f"{arxiv_id}.tar.gz"
    if target.exists():
        return target

    data = _fetch(f"https://arxiv.org/e-print/{arxiv_id}")
    target.write_bytes(data)
    time.sleep(3)  # arXiv rate-limits; be a good citizen.
    return target


def read_raw_source(archive: Path) -> str:
    """The concatenated LaTeX of an e-print archive, unreduced."""
    raw = archive.read_bytes()
    try:
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:*") as tar:
            parts = []
            for member in tar.getmembers():
                if member.isfile() and member.name.endswith(".tex"):
                    handle = tar.extractfile(member)
                    if handle is not None:
                        parts.append(handle.read().decode("utf-8", errors="replace"))
            return "\n".join(parts)
    except tarfile.TarError:
        import gzip

        try:
            return gzip.decompress(raw).decode("utf-8", errors="replace")
        except OSError:
            return raw.decode("utf-8", errors="replace")


def read_source(archive: Path) -> str:
    """Extract prose from an e-print archive.

    arXiv serves either a gzipped tar of the source tree or a single gzipped `.tex` file.
    """
    raw = archive.read_bytes()

    try:
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:*") as tar:
            files = {}
            for member in tar.getmembers():
                if not member.isfile() or not member.name.endswith(".tex"):
                    continue
                handle = tar.extractfile(member)
                if handle is None:
                    continue
                files[member.name] = handle.read().decode("utf-8", errors="replace")
    except tarfile.TarError:
        import gzip

        try:
            text = gzip.decompress(raw).decode("utf-8", errors="replace")
        except OSError:
            text = raw.decode("utf-8", errors="replace")
        return detex.strip(text)

    main = detex.find_main_source(files)
    if main is None:
        return ""
    return detex.strip(detex.inline_inputs(files, main))


def load(corpus_dir: Path, cache_dir: Path, only: str | None = None) -> list[Paper]:
    """Load every corpus paper that is present locally, fetching sources as needed."""
    papers = []
    for arxiv_id, name in PAPERS.items():
        if only and only not in name:
            continue
        pdf = corpus_dir / name
        if not pdf.exists():
            continue
        archive = fetch_source(arxiv_id, cache_dir)
        papers.append(
            Paper(
                arxiv_id=arxiv_id,
                pdf=pdf,
                reference=read_source(archive),
                source=read_raw_source(archive),
            )
        )
    return papers
