# Development and distribution

TxnStage uses Rust for the execution engine, React and CodeMirror for the web interface, and SQLite built from the source pinned by `Cargo.lock`. The release executable embeds the interface and SQLite.

## Prerequisites

- Rust stable with Cargo, rustfmt and Clippy.
- A native C toolchain: Command Line Tools on macOS, a C compiler on Linux, or Visual Studio Build Tools with the C++ workload on Windows.
- Node.js 24 and pnpm 12.14.0, as declared in `frontend/package.json`.
- Python 3 for the standalone-executable check and archive packaging.

A system SQLite installation is not required. Preserve `Cargo.lock` and `frontend/pnpm-lock.yaml` when building an existing revision. Run pnpm commands from `frontend` so Corepack can resolve the package-manager version declared there.

## Build and run

From the repository root:

```sh
cd frontend
pnpm install --frozen-lockfile
pnpm build
cd ..
cargo build --release --locked
./target/release/txnstage serve
```

On Windows, use `target/release/txnstage.exe`. Open the complete URL printed by `serve`, including its token fragment. Ctrl+C stops workers and removes the experiment database. Closing the browser tab leaves the server running.

Build frontend assets before the release executable. After changing frontend code, rebuild both the assets and the release binary to include the changes.

## Work on the interface

Run the asset watcher from `frontend`:

```sh
pnpm build --watch
```

After the first frontend build completes, start the Rust server in a second terminal from the repository root:

```sh
cargo run -- serve
```

Debug builds read generated assets from `frontend/dist`. Reload the browser after a rebuild. Using the Rust server exercises the same HTTP authentication and worker processes as a release build.

## Run checks

From the repository root:

```sh
cd frontend
pnpm check
pnpm test
pnpm build
cd ..
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
cd frontend
pnpm exec playwright install chromium
pnpm e2e
cd ..
python3 scripts/smoke-release.py
```

Playwright needs permission to start Chromium and access loopback HTTP. On Linux, its system libraries may also be required; run `pnpm exec playwright install --with-deps chromium` from `frontend` when preparing a machine. Set `TXNSTAGE_BINARY` to an absolute executable path to test another build.

The [verification guide](verification.md) describes test coverage, reports and failure investigation. CI is configured to run frontend checks, native tests, release builds and Chromium tests on Linux, macOS and Windows. Consult the [workflow results](https://github.com/0then0/txnstage/actions/workflows/ci.yml) for a particular revision.

## Update examples

The built-in scenarios are defined in `src/scenarios.rs` and `src/scenarios/fk_rebuild.rs`. After changing a scenario, export the catalog and record the affected examples with the built executable:

```sh
./target/release/txnstage scenarios examples
./target/release/txnstage run examples/07.scenario.json --output examples/recordings/07.recording.json
./target/release/txnstage run examples/recordings/07.recording.json --output replay.recording.json
```

The `scenarios` command overwrites numbered scenario files in its destination. The preservation-assertion example is a separate file, `examples/fk-preservation-fails.scenario.json`; it intentionally exits 1 and is not part of the successful built-in catalog. Keep its final complete-record expectation consistent with the preservation control. Native tests verify catalog files and replay the FK recordings.

## Update documentation images

The Playwright test named `snapshot through real UI` captures the workbench with an expected `SQLITE_BUSY_SNAPSHOT` event selected. Its 2× device scale produces a 2880-pixel-wide image.

After building the release executable, run from `frontend`:

```sh
pnpm e2e --grep 'snapshot through real UI'
```

Inspect `.cache/reports/workbench-desktop.png` from the repository root, then copy it to `docs/images/workbench.png`. Test runs write only generated screenshots; they do not replace documentation images. Exclude launch tokens and unrelated windows from images added to the repository.

## Package an executable

From the repository root, package a previously built executable for its matching platform:

```sh
python3 scripts/package-release.py target/release/txnstage macos-arm64
```

Accepted platform labels are `linux-x86_64`, `macos-arm64`, `macos-x86_64` and `windows-x86_64`. On Windows, pass the `.exe` path. The script checks the binary's reported platform and writes an archive, a SHA-256 file and environment metadata to `dist/`. Each archive contains the executable, README, license, documentation, scenarios, recordings and icon.

Verify the checksum from the archive's directory:

```sh
cd dist
shasum -a 256 -c txnstage-0.2.0-macos-arm64.tar.gz.sha256
```

Use `sha256sum -c` on Linux or `Get-FileHash -Algorithm SHA256` on Windows. Test archives on their target machines. macOS executables are unsigned; Linux compatibility depends on the build runner's libc.

The manually started [Build release binaries](https://github.com/0then0/txnstage/actions/workflows/binaries.yml) workflow packages the four platform targets and uploads them as Actions artifacts. Publishing a GitHub Release is a separate action. The workflow and local packaging script do not publish releases, sign binaries or perform automatic updates.

Dependabot checks Cargo, frontend and GitHub Actions dependencies monthly. Dependency changes should pass the scenario, recording-replay and browser checks before merging.
