"""Package and smoke test a native binary before uploading a release artifact."""

import argparse
import hashlib
from pathlib import Path
import shutil
import tarfile
import tempfile
import zipfile

from smoke_cli import ROOT, verify


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target")
    parser.add_argument("version")
    parser.add_argument("binary", type=Path)
    parser.add_argument("--out", type=Path, default=Path("dist"))
    args = parser.parse_args()
    windows = "windows" in args.target
    name = f"rustypaper-{args.version}-{args.target}"
    executable = "rustypaper.exe" if windows else "rustypaper"
    args.out.mkdir(parents=True, exist_ok=True)
    archive = args.out.resolve() / (name + (".zip" if windows else ".tar.gz"))
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        staging = work / name
        staging.mkdir()
        shutil.copy2(args.binary, staging / executable)
        for filename in ("README.md", "LICENSE-MIT", "LICENSE-APACHE"):
            shutil.copy2(ROOT / filename, staging / filename)
        if windows:
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
                for path in sorted(staging.iterdir()):
                    bundle.write(path, f"{name}/{path.name}")
        else:
            with tarfile.open(archive, "w:gz") as bundle:
                bundle.add(staging, arcname=name)

        extracted = work / "extracted"
        if windows:
            with zipfile.ZipFile(archive) as bundle:
                bundle.extractall(extracted)
        else:
            with tarfile.open(archive) as bundle:
                bundle.extractall(extracted, filter="data")
        verify([str(extracted / name / executable)])
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(
        f"{checksum}  {archive.name}\n", encoding="utf-8"
    )
    print(archive)


if __name__ == "__main__":
    main()
