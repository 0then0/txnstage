# Development

TxnStage uses Rust for the coordinator and workers, React and CodeMirror for the workbench, and SQLite compiled from the source pinned by Cargo.lock.

## Prerequisites

- Rust stable with Cargo, rustfmt and Clippy.
- A native C toolchain for bundled SQLite: Command Line Tools on macOS, a C compiler on Linux, or Visual Studio Build Tools with the C++ workload on Windows.
- Node.js 24 and pnpm 12.14.0. The frontend package declares its package-manager version.
- Python 3 for standalone verification and archive packaging.

A system SQLite installation is not required. Keep both lockfiles when updating dependencies.

## Build and run

Run from the repository root:

```sh
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend build
cargo build --release --locked
./target/release/txnstage serve
```

On Windows, the executable is `target/release/txnstage.exe`. Open the complete URL printed by `serve`, including its token fragment. Ctrl+C stops the server, reaps workers and removes the experiment database. Closing the browser tab leaves the server running.

The release build embeds `frontend/dist`, so rebuild Rust after changing frontend assets. For frontend development, use two terminals:

```sh
# Terminal 1: rebuild assets when source files change.
pnpm --dir frontend build --watch
```

```sh
# Terminal 2: run the Rust server after the first frontend build completes.
cargo run -- serve
```

Debug builds read the generated frontend assets from disk. Reload the browser after a rebuild. Using the Rust server keeps the same authentication and origin checks as the release application.

## Run checks

```sh
pnpm --dir frontend check
pnpm --dir frontend test
pnpm --dir frontend build
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
pnpm --dir frontend exec playwright install chromium
pnpm --dir frontend e2e
python3 scripts/smoke-release.py
```

On Linux, Playwright may also need system libraries; use `playwright install --with-deps chromium` when setting up the machine. Run commands in an environment that permits local browser processes and loopback HTTP. A failed browser launch is reported as a test failure, not a successful UI check.

Native tests use disposable databases and child processes. Browser tests start the production executable, exercise the HTTP API and real workers, then stop the server. Set `TXNSTAGE_BINARY` to an absolute executable path to test another build. Generated screenshots and JSON reports go to `.cache/reports/`; Playwright's HTML report and failure traces go to `frontend/playwright-report/` and `frontend/test-results/`. These directories are ignored by Git. See [verification](verification.md) for coverage and report usage.

CI runs frontend checks, native checks, release builds and Chromium tests on Linux, macOS and Windows. Reports are uploaded as workflow artifacts, including on failure. Platform-specific process behavior must be checked on the target platform.

## Refresh the README screenshot

The first browser test captures the current workbench with an expected `SQLITE_BUSY_SNAPSHOT` event selected. That test uses a 2× device scale factor, so the desktop image is 2880 pixels wide. Other browser tests use the default scale.

After building the release executable:

```sh
pnpm --dir frontend e2e --grep 'snapshot through real UI'
```

Inspect `.cache/reports/workbench-desktop.png`, then copy it to `docs/images/workbench.png`. The README image is a curated documentation asset; ordinary test runs do not overwrite it. Keep browser chrome, launch tokens and unrelated windows out of the capture.

## Package binaries

The **Build release binaries** workflow builds Linux x86_64, macOS arm64, macOS x86_64 and Windows x86_64 archives. Start it from GitHub Actions. It uploads archives, SHA-256 files and SQLite environment metadata as Actions artifacts; publishing a GitHub Release is a separate step.

To package an already built executable locally, choose the label matching its platform:

```sh
python3 scripts/package-release.py target/release/txnstage macos-arm64
```

Accepted labels are `linux-x86_64`, `macos-arm64`, `macos-x86_64` and `windows-x86_64`. On Windows, pass the `.exe` path. The script checks the executable's reported platform and writes to `dist/`. Each archive includes the executable, README, license, documentation, scenarios, sample recordings and icon. It does not compile or publish anything.

Verify the checksum from the archive's directory:

```sh
cd dist
shasum -a 256 -c txnstage-0.1.1-macos-arm64.tar.gz.sha256
```

Use `sha256sum -c` on Linux or `Get-FileHash -Algorithm SHA256` on Windows. Test the archive on its target platform before distribution. macOS executables are unsigned; Linux compatibility depends on the build runner's libc. Code signing, notarization and automatic updates are not part of the current packaging workflow.

Dependabot checks Cargo, frontend and Actions dependencies monthly. Dependency changes should pass the native scenario/replay tests and production browser checks before merging.
