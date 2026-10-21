# Testing and verification

TxnStage tests use real SQLite connections in separate worker processes. The CLI and browser share the same coordinator and scheduler, so both entry points exercise the same transaction behavior.

For setup and exact commands, see [development](development.md). Current CI results are available in [GitHub Actions](https://github.com/0then0/txnstage/actions/workflows/ci.yml).

## Native integration tests

`tests/native.rs` covers:

- All six built-in scenarios, their typed results and expected SQLite errors, followed by exported-recording replay.
- Writer contention, confirmed busy barriers and progress on another connection while a worker waits.
- Cancellation during SQL execution and busy retry; deadlines, worker crashes and directory cleanup.
- Full worker-pipe backpressure during execute, cancel and stop. Unix tests pause a worker with SIGSTOP to exercise forced termination.
- Signed 64-bit integers, NULL, BLOB, REAL, RETURNING, trigger bodies and statement boundaries.
- Rejection of filesystem access and unsupported PRAGMAs, including rejection before prepare-time settings can change.
- Checkpoint result validation, preview limits, bounded large-recording export/replay and observation without hidden checkpoint SQL.

Tests synchronize on worker events rather than assuming a particular callback count or duration. Timeouts bound hangs; deadlines and retry policies are themselves time-based behavior.

## Frontend and browser tests

Vitest checks typed value presentation, observed dependency barriers, compact JSON serialization and the actual UTF-8 artifact byte limit.

Playwright launches the release executable and drives Chromium through the full stack. It covers schedule execution, error 517, contention, cancellation, reset, artifact export/import/rerun, draft/history separation and stale asynchronous replies. UI checks also cover keyboard execution and inspection, event filtering, settings, read-only artifacts, long Cyrillic content and widths from 320 to 1440 CSS pixels.

The large-recording test runs with Chromium CPU throttled by a factor of four and checks result pagination as well as export/import/rerun, without extending completion timeouts.

Run all browser checks with:

```sh
pnpm --dir frontend e2e
```

To inspect a failure:

```sh
pnpm --dir frontend exec playwright show-report
pnpm --dir frontend exec playwright show-trace test-results/path/to/trace.zip
```

HTML reports and traces remain under `frontend/`; JSON reports and screenshots are written to `.cache/reports/`. CI uploads these generated files as artifacts. They are not committed or included in release archives.

Browser tests and screenshots do not establish screen-reader compatibility or comprehensive WCAG conformance. Test additional browsers, assistive technologies and target devices for your distribution requirements.

## Standalone executable check

After building the release executable:

```sh
python3 scripts/smoke-release.py
```

The smoke check copies the executable to a temporary directory, hides source frontend assets and empties PATH. It verifies embedded asset serving, three independent workers, graceful SIGINT shutdown and removal of the owned experiment directory. Source assets are restored even if the check fails. Run it from the repository root without a concurrent build.

The result is printed and saved to `.cache/reports/release-smoke.json`. This check targets the local platform; it does not replace testing a distributed archive on its target machine.

## Reproduce a scenario

```sh
./target/release/txnstage run examples/01.scenario.json --output snapshot.recording.json
./target/release/txnstage run snapshot.recording.json --output replay.recording.json
```

A successful exit means the declared expectations matched. An expected SQL error remains an error event in the recording. It is the scenario's expected outcome, not a failed test.

[Sample recordings](../examples/recordings/) demonstrate the format and observable outcomes. Their PIDs, timing, file sizes and retry counts describe those particular runs. Fresh executions can differ while satisfying the same expectations. Use the executable scenario and typed result/error assertions for comparisons, rather than byte-for-byte recording equality.
