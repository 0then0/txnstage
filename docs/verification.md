# Testing and verification

TxnStage tests execute real SQL through independent SQLite worker processes. The command line and web interface share the same engine, scheduler and SQL restrictions. All experiment databases used by these checks are disposable.

See [development](development.md#run-checks) for the complete command sequence. CI is configured for Linux, macOS and Windows; [GitHub Actions](https://github.com/0then0/txnstage/actions/workflows/ci.yml) records the results for each revision.

## Native tests

Run from the repository root:

```sh
cargo test --locked
```

[`tests/native.rs`](../tests/native.rs) covers:

- Built-in scenarios, typed result expectations, expected SQLite errors and exported-recording replay.
- Catalog files matching their built-in definitions and FK recordings rerunning in fresh databases.
- The executable JSON example in the artifact reference, through the CLI and recording validator.
- The [FK rebuild experiments](fk-rebuild-lab.md): committed cascade loss, PRAGMA scope and deferral, WAL snapshots, complete-record preservation and a failing preservation assertion with successful SQL events.
- Writer contention, observed busy callbacks and progress on another connection while a worker waits.
- Cancellation, deadlines, worker crashes, pipe backpressure and experiment-directory cleanup. Unix-specific checks use SIGSTOP to pause a worker.
- INTEGER precision, NULL, BLOB, REAL, RETURNING, trigger bodies and SQL statement boundaries.
- Read-only FK diagnostics with a real orphan, rejected PRAGMA arguments and blocked filesystem, extension and instrumentation changes.
- Checkpoint result validation, result-preview limits, large-recording export/replay and state polling without hidden checkpoint SQL.

Tests synchronize on reported events. Timeouts bound hangs, while callback counts and elapsed durations can vary between executions.

## Repository and packaging checks

Run from the repository root:

```sh
python3 -m unittest discover -s tests -p 'test_*.py'
```

These standard-library tests check local documentation links, Git ignore and line-ending rules, pinned workflow actions and credentials settings. Packaging tests exercise both tar.gz and ZIP creation, policy-file inclusion, exclusion of a root `.env`, checksums and environment metadata. They mock only the executable's version response and do not need an installed SQLite or a platform-specific executable.

## Frontend checks

Run from `frontend`:

```sh
pnpm check
pnpm test
pnpm build
```

Biome checks formatting and lint rules. Vitest checks typed values, event dependencies, compact JSON serialization, UTF-8 file-size limits and incremental state updates. The build checks TypeScript and bundles the interface.

## Browser tests

Build the release executable first, then run from `frontend`:

```sh
pnpm exec playwright install chromium
pnpm e2e
```

Playwright starts the release executable and drives Chromium through the interface, HTTP API, workers and bundled SQLite. Set `TXNSTAGE_BINARY` to an absolute path to select another executable.

The tests cover schedule execution, expected error 517, contention, cancellation, reset, recording export/import/rerun, editing without rewriting history and stale asynchronous replies. FK tests inspect data before DROP, after DROP and after COMMIT; compare old and fresh WAL reads; check large integer display; and distinguish a failing preservation assertion from a passing control. After successful replay, they also execute the default custom SQL against the FK schema.

Interface checks cover keyboard actions, event filtering, settings, read-only imports, long Cyrillic text and viewport widths from 320 to 1440 CSS pixels. A large-recording test runs Chromium with fourfold CPU throttling and checks pagination, incremental polling and recording round trips. Automated browser checks cover these interactions; screen-reader and broader accessibility testing need separate assessment.

### Reports and failures

Generated reports are ignored by Git:

- `.cache/reports/`: screenshots and JSON results.
- `frontend/playwright-report/`: browser-test HTML report.
- `frontend/test-results/`: failure context and traces.

From `frontend`, open a report or trace with:

```sh
pnpm exec playwright show-report
pnpm exec playwright show-trace test-results/path/to/trace.zip
```

CI uploads reports on failure as well as success. Browser launch or loopback-permission failures mean the browser check did not execute the application; inspect the launch error before investigating scenario assertions.

## Standalone executable

After a release build, run from the repository root without a concurrent build:

```sh
python3 scripts/smoke-release.py
```

The check copies the executable to a temporary directory, temporarily hides source frontend assets and empties PATH. It verifies embedded asset serving, three independent workers, graceful SIGINT shutdown and removal of the experiment directory. Source assets are restored during cleanup. Results are printed and saved to `.cache/reports/release-smoke.json`.

This check verifies the local executable. A distributed archive also needs testing on its target platform.

## Replay and expected failures

From the repository root:

```sh
./target/release/txnstage run examples/01.scenario.json --output snapshot.recording.json
./target/release/txnstage run snapshot.recording.json --output replay.recording.json
```

Exit code 0 means the declared expectations matched. An expected SQLite error remains an error event. A SQL statement can also complete successfully while its row expectation fails; the [preservation assertion example](fk-rebuild-lab.md#observation-and-preservation) intentionally demonstrates this with exit code 1.

Compare typed results and expected error codes when checking a replay. PIDs, timing, file sizes and retry counts describe individual executions and are not byte-for-byte equality targets.
