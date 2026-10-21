"""Package a built TxnStage executable, documentation and examples for distribution."""

import argparse
import hashlib
import json
import pathlib
import subprocess
import tarfile
import zipfile

PLATFORMS = {
    "linux-x86_64": ("linux", "x86_64"),
    "macos-arm64": ("macos", "aarch64"),
    "macos-x86_64": ("macos", "x86_64"),
    "windows-x86_64": ("windows", "x86_64"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=pathlib.Path, help="Path to the built executable")
    parser.add_argument("platform", choices=PLATFORMS, help="Archive target platform")
    args = parser.parse_args()
    binary = args.binary.resolve()
    metadata = json.loads(subprocess.check_output([str(binary), "version"]))
    if PLATFORMS[args.platform] != (metadata["os"], metadata["arch"]):
        parser.error("Archive platform must match the compiled executable")

    out = pathlib.Path("dist")
    out.mkdir(exist_ok=True)
    name = f"txnstage-{metadata['txnstage']}-{args.platform}"
    files = [
        (binary, binary.name),
        (pathlib.Path("README.md"), "README.md"),
        (pathlib.Path("LICENSE"), "LICENSE"),
        (pathlib.Path("frontend/public/icon.svg"), "frontend/public/icon.svg"),
    ]
    for directory in ("docs", "examples"):
        files.extend(
            (source, source.as_posix())
            for source in sorted(pathlib.Path(directory).rglob("*"))
            if source.is_file()
        )

    if args.platform.startswith("windows"):
        archive = out / f"{name}.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
            for source, dest in files:
                bundle.write(source, f"{name}/{dest}")
    else:
        archive = out / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            for source, dest in files:
                bundle.add(source, f"{name}/{dest}")

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix + ".sha256").write_text(
        f"{digest}  {archive.name}\n", encoding="utf-8"
    )
    (out / f"{name}.environment.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
    )
    print(archive)


if __name__ == "__main__":
    main()
