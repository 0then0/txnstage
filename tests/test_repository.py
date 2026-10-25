"""Repository policies and release packaging checks using the standard library."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import unquote, urlsplit
import zipfile


ROOT = Path(__file__).resolve().parents[1]
POLICIES = ("README.md", "LICENSE", "CONTRIBUTING.md", "CODE_OF_CONDUCT.md", "SECURITY.md", "CHANGELOG.md")


class RepositoryTests(unittest.TestCase):
    def test_local_documentation_links_resolve(self):
        files = [ROOT / name for name in POLICIES if name.endswith(".md")]
        files += list((ROOT / "docs").glob("*.md"))
        files += [ROOT / "examples/README.md", ROOT / ".github/PULL_REQUEST_TEMPLATE.md"]
        for source in files:
            for target in re.findall(r"\[[^\]]*\]\(([^\s)]+)\)", source.read_text(encoding="utf-8")):
                parsed = urlsplit(target)
                if parsed.scheme or target.startswith("#"):
                    continue
                path = source.parent / unquote(parsed.path)
                with self.subTest(source=source.relative_to(ROOT), target=target):
                    self.assertTrue(path.exists(), f"Missing target: {path}")
                    if parsed.fragment and path.suffix == ".md":
                        headings = re.findall(r"^#+\s+(.+)$", path.read_text(encoding="utf-8"), re.MULTILINE)
                        anchors = {re.sub(r"[^\w -]", "", heading.lower()).replace(" ", "-") for heading in headings}
                        self.assertIn(parsed.fragment, anchors)

    def test_local_secrets_and_databases_are_ignored_but_examples_are_not(self):
        ignored = [
            ".env", ".env.local", "frontend/.env.production",
            "test.db", "test.db-wal", "test.db-shm", "test.db-journal",
            "test.sqlite", "test.sqlite-wal", "test.sqlite3-shm",
            "frontend/node_modules/test.js", "frontend/dist/index.html",
            "target/debug/txnstage", ".cache/reports/result.json",
            "tests/__pycache__/test_repository.cpython-314.pyc",
        ]
        tracked = [".env.example", "frontend/.env.local.example", "Cargo.lock", "frontend/pnpm-lock.yaml", "examples/07.scenario.json", "examples/recordings/07.recording.json"]
        for path, expected in [(p, True) for p in ignored] + [(p, False) for p in tracked]:
            with self.subTest(path=path):
                result = subprocess.run(["git", "check-ignore", "--no-index", "-q", path], cwd=ROOT, check=False)
                self.assertIn(result.returncode, (0, 1))
                self.assertEqual(result.returncode == 0, expected)

    def test_line_endings_and_binary_attributes(self):
        paths = ["src/main.rs", "frontend/src/main.tsx", "README.md", ".github/workflows/ci.yml", "frontend/public/icon.svg", "docs/images/workbench.png"]
        result = subprocess.check_output(["git", "check-attr", "text", "eol", "--", *paths], cwd=ROOT, text=True)
        self.assertIn("docs/images/workbench.png: text: unset", result)
        for path in paths[:-1]:
            self.assertIn(f"{path}: eol: lf", result)

    def test_workflow_actions_are_pinned_and_do_not_persist_credentials(self):
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            text = path.read_text(encoding="utf-8")
            with self.subTest(workflow=path.name):
                self.assertIn("contents: read", text)
                self.assertIn("persist-credentials: false", text)
                for action in re.findall(r"^\s*- uses: (\S+)", text, re.MULTILINE):
                    self.assertRegex(action, r"^[\w.-]+/[\w./-]+@[a-f0-9]{40}$")


class PackageTests(unittest.TestCase):
    def test_tar_and_zip_include_policies_and_have_correct_checksums(self):
        spec = importlib.util.spec_from_file_location("package_release", ROOT / "scripts/package-release.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for label, os_name, arch, filename in [
            ("macos-arm64", "macos", "aarch64", "txnstage"),
            ("windows-x86_64", "windows", "x86_64", "txnstage.exe"),
        ]:
            with self.subTest(platform=label), tempfile.TemporaryDirectory() as directory:
                project = Path(directory)
                for name in POLICIES:
                    shutil.copy2(ROOT / name, project / name)
                for name in ("docs", "examples", "frontend/public"):
                    shutil.copytree(ROOT / name, project / name)
                binary = project / filename
                binary.write_bytes(b"packaging fixture")
                (project / ".env").write_text("PRIVATE=not-for-distribution\n", encoding="utf-8")
                metadata = {"txnstage": "9.8.7", "os": os_name, "arch": arch}
                previous = Path.cwd()
                try:
                    os.chdir(project)
                    with patch.object(module.subprocess, "check_output", return_value=json.dumps(metadata).encode()), patch("sys.argv", ["package-release.py", str(binary), label]):
                        module.main()
                finally:
                    os.chdir(previous)
                base = f"txnstage-9.8.7-{label}"
                archive = project / "dist" / (base + (".zip" if os_name == "windows" else ".tar.gz"))
                if os_name == "windows":
                    with zipfile.ZipFile(archive) as bundle:
                        files = {name: bundle.read(name) for name in bundle.namelist()}
                else:
                    with tarfile.open(archive) as bundle:
                        files = {member.name: bundle.extractfile(member).read() for member in bundle.getmembers()}
                for name in POLICIES:
                    self.assertEqual(files[f"{base}/{name}"], (ROOT / name).read_bytes())
                self.assertIn(f"{base}/docs/fk-rebuild-lab.md", files)
                self.assertIn(f"{base}/examples/recordings/12.recording.json", files)
                self.assertIn(f"{base}/{filename}", files)
                self.assertFalse(any(name.endswith("/.env") for name in files))
                checksum = archive.with_suffix(archive.suffix + ".sha256").read_text(encoding="utf-8").split()
                self.assertEqual(checksum, [hashlib.sha256(archive.read_bytes()).hexdigest(), archive.name])
                self.assertEqual(json.loads((project / "dist" / f"{base}.environment.json").read_text(encoding="utf-8")), metadata)


if __name__ == "__main__":
    unittest.main()
