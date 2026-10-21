"""Verify that a release executable serves embedded assets and cleans up its workers."""

import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import tempfile
import urllib.request


def main():
    project = pathlib.Path.cwd()
    binary = project / "target/release" / ("txnstage.exe" if os.name == "nt" else "txnstage")
    with tempfile.TemporaryDirectory(prefix="txnstage-release-smoke-") as tmp:
        root = pathlib.Path(tmp)
        exe = root / binary.name
        shutil.copy2(binary, exe)
        own_tmp = root / "experiments"
        own_tmp.mkdir()
        env = os.environ.copy()
        env.update(TMPDIR=str(own_tmp), TMP=str(own_tmp), TEMP=str(own_tmp), PATH="")

        # Hide source assets so a compiled-in source path cannot satisfy the check.
        assets = project / "frontend/dist"
        hidden = project / ".cache/smoke-hidden-assets"
        hidden.parent.mkdir(parents=True, exist_ok=True)
        assets.rename(hidden)
        proc = None
        try:
            proc = subprocess.Popen(
                [str(exe), "serve"], cwd=root, env=env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            )
            line = proc.stdout.readline()
            launch = proc.stdout.readline().strip()
            match = re.fullmatch(r"(http://127\.0\.0\.1:\d+)/#token=([a-f0-9-]+)", launch)
            assert match, (line, launch)
            origin, token = match.groups()

            def get(path, auth=False):
                request = urllib.request.Request(
                    origin + path,
                    headers={"x-txnstage-token": token} if auth else {},
                )
                return urllib.request.urlopen(request, timeout=5)

            html = get("/").read().decode()
            assert "TxnStage" in html
            paths = re.findall(r'(?:src|href)="(/assets/[^\"]+)"', html)
            assert len(paths) >= 2
            for path in paths:
                assert len(get(path).read()) > 100
            state = json.load(get("/api/state", True))
            pids = [e["data"]["pid"] for e in state["events"] if e["kind"] == "ready"]
            assert len(set(pids)) == 3
            assert all(
                e["operation"].startswith("seed_") or not e["operation"]
                for e in state["events"]
            )
            proc.send_signal(signal.SIGINT)
            assert proc.wait(timeout=8) == 0
            assert not list(own_tmp.iterdir()), "Owned experiment directory not removed"
            if os.name != "nt":
                for pid in pids:
                    try:
                        os.kill(pid, 0)
                    except ProcessLookupError:
                        pass
                    else:
                        raise AssertionError(f"Worker {pid} remains alive")

            result = {
                "copied_binary": True,
                "source_assets_hidden": True,
                "empty_PATH": True,
                "served_asset_count": len(paths),
                "independent_workers": len(pids),
                "sigint_exit": 0,
                "cleanup": True,
                "platform": state["environment"]["os"],
                "arch": state["environment"]["arch"],
            }
            report = project / ".cache/reports/release-smoke.json"
            report.parent.mkdir(parents=True, exist_ok=True)
            report.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
            print(json.dumps(result, indent=2))
        finally:
            if proc is not None and proc.poll() is None:
                proc.kill()
                proc.wait()
            hidden.rename(assets)


if __name__ == "__main__":
    main()
