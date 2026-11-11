# Architecture

This guide explains how TxnStage runs an experiment, what its safety limits are and what its observations do and do not tell you. It is written for people who want to interpret results precisely or change the code.

## Overview

TxnStage is one Rust executable. Its public modes are `serve`, `run`, `scenarios` and `version`; `--worker` is internal. The React and CodeMirror interface is embedded in the executable at release build time. SQLite is compiled from the source pinned by rusqlite's `bundled` feature, independently of any SQLite installed on the system.

```text
browser ── HTTP on 127.0.0.1 ──┐
                               ├── coordinator ──┬── worker A ──┐
txnstage run <file> ───────────┘   (engine.rs)   ├── worker B ──┼── experiment.db
                                                 └── worker C ──┘   (temporary directory)
```

The coordinator is the single process you start. For each experiment it creates a temporary directory containing `experiment.db` and starts one worker process per connection. Each worker opens its own SQLite connection to that file. The coordinator sends commands to workers, records what they report as events, and never runs experiment SQL itself. The web interface and the command line use the same coordinator, scheduler and SQL restrictions.

## Source layout

- [`model.rs`](../src/model.rs) defines the versioned scenario and recording formats and validates imported data.
- [`scenarios.rs`](../src/scenarios.rs), [`scenarios/fk_rebuild.rs`](../src/scenarios/fk_rebuild.rs) and [`scenarios/crash_reopen.rs`](../src/scenarios/crash_reopen.rs) define the [built-in experiments](../examples/README.md).
- [`engine.rs`](../src/engine.rs) is the coordinator: worker processes, operation scheduling, event order, cancellation and cleanup. Each connection accepts one operation at a time.
- [`worker.rs`](../src/worker.rs) executes SQL through its own SQLite connection and reports results.
- [`server.rs`](../src/server.rs) exposes the loopback HTTP API. [`main.rs`](../src/main.rs) provides the CLI commands.
- [`frontend/src`](../frontend/src/) contains the React workbench and recording viewer.

## Starting an experiment

Each experiment has one owned temporary directory and a file `experiment.db`. A short-lived bootstrap connection sets the requested journal mode before workers exist. Initial schema and data then execute as visible `seed_*` operations on A, using exactly the same worker restrictions. Initialization must use individual statements; a failed seed aborts startup and reaps all workers.

Every worker disables autocheckpoint, uses in-memory temporary storage, disables trusted schema and extension loading, and installs the authorizer before accepting user SQL.

## Communication between coordinator and workers

Commands and events are JSON Lines on the worker's standard input and output. There are two independent logical channels: command input and observation output.

- A dedicated reader thread in the worker stays available while the SQL thread executes.
- Execute commands go to a bounded single-slot queue.
- Cancel commands bypass that queue. They set an atomic reason and call rusqlite's thread-safe interrupt handle.
- End of input terminates the worker. This also happens when the coordinator dies abruptly.

Cancellation shares the pipe with other commands, and a stalled reader can back the pipe up. The coordinator therefore bounds the whole cancel send-and-wait interval and can kill the worker through a separate supervisor channel.

## Coordinator responsiveness and state polling

No coordinator lock is held while SQLite executes. State locks protect only short updates and serialization. Per-worker input locks protect writes to a pipe, not execution. Other workers and HTTP requests remain available while one busy handler retries.

The interface polls `/api/state` every 200 ms. A poll reads coordinator memory and file sizes and performs no SQL on experiment connections. Environment metadata comes from a separate in-memory connection that does not touch the experiment.

Polls supply the current `experiment` identifier and an `after` event count. The response contains only later events, with `event_offset` identifying the retained prefix. A mismatched experiment or invalid cursor returns the full history with offset zero, and requests without a cursor return a full snapshot. Idle polling therefore does not transfer large result previews repeatedly. In the interface, background and command-triggered refreshes are serialized within one experiment, and Reset starts a new experiment without waiting for stale requests.

## Statement and result lifecycle

An operation moves through `created → running → completed / failed / cancelled / timed_out`. Its connection cannot accept another operation until a terminal event has been observed.

A failed SQLite statement does not imply rollback: the actual transaction and autocommit state is sampled after the statement is finalized. Errors that a scenario expects remain failed SQL operations with primary and extended codes. Matching them against expectations is a separate scheduler concern.

### One statement per operation

SQL boundaries use SQLite's `sqlite3_complete` and the tail returned by `sqlite3_prepare_v3`, never string splitting. Trigger bodies and quoted semicolons remain valid. Comments and a trailing terminator are accepted. Leading empty statements before an executable statement are intentionally rejected.

Some PRAGMAs take effect during prepare. A preflight therefore rejects a second executable statement before the first is prepared, using an authorizer that denies every action. Direct PRAGMA statements with bound parameters are denied during prepare, before a setting can change. Parameterized SELECTs over table-valued PRAGMAs remain supported.

### Parameters and results

Parameters are positional in SQLite's bind-index order. The number of provided cells must equal `sqlite3_bind_parameter_count`. Input REAL parameters must be finite; SQLite itself maps NaN to NULL.

Every statement is stepped through `SQLITE_DONE` and then finalized. The preview limit suppresses collection of rows, not stepping.

- Row and change counts are strings, because they are 64-bit.
- INTEGER cells are decimal strings.
- BLOB previews are hex.
- TEXT previews are UTF-8, with replacement characters for invalid byte sequences. Use BLOB when exact bytes matter.
- REAL infinities are serialized as `"Infinity"` and `"-Infinity"`, not JSON null.

## Busy policy

Each worker installs its own busy handler. It counts every call, and the terminal event contains the exact final count. To avoid event floods it emits a `busy` event for the first callback and then for power-of-two counts. A `busy` event therefore means that SQLite called the handler. It is not guessed contention and does not identify a lock owner.

The handler compares the elapsed time of the operation with `busy_ms`. Retry sleeps are a fixed 10 ms, and cancellation is checked before and after each sleep. With a zero budget a callback is still observed if SQLite invokes the handler, and the handler then returns immediately.

This policy is not SQLite's native `sqlite3_busy_timeout`. SQLite may also bypass a handler, for example for an obsolete WAL snapshot. `PRAGMA busy_timeout=...` would replace the instrumentation and is denied. Assignments to `wal_autocheckpoint` and `journal_mode` are denied as well; change those settings through a fresh experiment.

The other mutable PRAGMAs that are allowed are `foreign_keys`, `defer_foreign_keys`, `read_uncommitted` and explicit `wal_checkpoint`. The read-only allowlist in `worker.rs` includes argument-free `foreign_key_check`; its forms with arguments are rejected. The [FK rebuild guide](fk-rebuild-lab.md) explains FK checks and per-connection settings.

## Cancellation, deadlines and cleanup

Cancellation uses `sqlite3_interrupt` and a progress handler that runs every 1000 virtual-machine instructions. The atomic reason also ends busy retries, where a progress callback may not run.

The coordinator starts deadline monitoring before it writes the execute command, and samples deadlines every 25 ms. It allows 300 ms for the entire cooperative cancel interval, including pipe backpressure. A worker that is still active after that is killed, reaped and marked `state_lost`; it cannot be reused until Reset. This unplanned kill is distinct from the [`crash` step](#process-crash-and-reopen).

Cancellation can race with statement completion, so terminal data keeps the worker's reported outcome next to the cancellation request. Cancellation does not promise to undo a commit that already happened.

Stop and Reset cancel active work, kill and reap every owned worker, and remove the owned directory. Ctrl+C performs the same cleanup. If the coordinator itself is killed in a way it cannot catch, a temporary directory can remain; workers still terminate on end of input. Cleanup is limited to directories owned by the running experiment. Closing the browser tab leaves the server running.

## Process crash and reopen

A `crash` step models the abrupt end of one process that had a connection open. It does not model power loss, a coordinator failure or storage faults, and it happens only between statements: the coordinator rejects it while the connection has an operation without a terminal event. A kill during a statement, including during COMMIT, remains the unplanned path above.

Each connection is in one of four planned states: open, crashing, closed or reopening. Transitions are admitted under the same state lock as SQL submission.

- **crash** (open → crashing → closed): the supervisor task that owns the child kills it and waits for it. `Child::kill` sends SIGKILL on Unix and calls TerminateProcess on Windows, so the worker runs no SQLite cleanup, rollback or close. The step completes only after the exit status has been collected. If the exit is not confirmed within 5 s the step fails and the connection is marked lost.
- **reopen** (closed → reopening → open): the same supervisor starts a new worker on the same `experiment.db` and waits up to 5 s for its `ready` message, the bound used at startup. The coordinator does not open, delete or recreate the database, WAL, SHM or rollback journal, and does not rerun initialization SQL. If the process cannot start, exits early, misses the deadline, or the step is cancelled or the experiment stopped, the new process is killed and reaped, the step ends as failed, timed_out or cancelled, and the connection stays closed so reopen can be retried.

A planned crash is not an infrastructure failure: it adds no `worker_lost` event, does not stop the experiment and leaves other connections running. An unexpected worker exit, a cancellation kill or an unconfirmed exit still marks the connection lost, and a lost connection cannot be reopened; it needs Reset. SQL on a closed connection, a second crash, and reopen of an open connection are rejected at submission with a message naming the connection and its state.

The new worker applies the normal startup contract: trusted schema off, in-memory temporary storage, autocheckpoint off, page and value limits, the authorizer, the instrumented busy policy and the row limit from the experiment settings. The journal mode is a property of the database file and is not set again. Everything held by the old connection is gone: its open transaction and locks, connection PRAGMA values such as `foreign_keys` and `defer_foreign_keys`, temporary tables and prepared statements. TxnStage does not infer recovery from the presence or size of a WAL or journal file. Whether a change survived is established by SQL after reopen; file sizes are diagnostics.

Every worker process has a generation number per connection. The task that reads a worker's output passes its generation with each message, and the coordinator drops messages from any generation other than the current one, messages for a closed connection, and worker messages naming a lifecycle step. Output buffered by a terminated process therefore cannot reach the session opened later under the same connection ID.

Cancelling a connection has no effect on a crash in flight, which is already bounded, and ends a reopen in flight. Stop and Reset kill and reap the current worker of every connection, including one started by an unfinished reopen, then wait for lifecycle steps in flight to record their terminal events before removing the directory.

## Resources and local isolation

### SQL restrictions

The authorizer denies ATTACH and DETACH (including the attachment that `VACUUM INTO` performs), extension and file functions, and PRAGMAs outside the allowlist. No user-defined functions or shell commands are installed.

### Resource limits

- SQLite temporary storage is memory only.
- Each worker has a 64 MiB SQLite heap hard limit, a 1 MiB value limit, 64 KiB of SQL, 100 columns and no attached databases.
- `max_page_count=16384` at the fixed default 4096-byte page size limits the database to about 64 MiB.
- A watchdog stops workers if the observed database plus WAL size exceeds 128 MiB. This is a sampled guard with possible overshoot, not a filesystem quota.
- One result keeps at most 200 rows and roughly 512 KiB of previews, then drops further previews while still draining the statement. The final row can exceed that soft budget by at most the column and cell preview limits.
- The event store has a 5 MiB soft budget. When necessary, row previews are omitted with an explicit `recording_limit` marker.
- New submissions stop at that budget, at 512 KiB of submitted operation descriptions (including initialization), at 200 user operations, or at the event count limit. Terminal events of operations that are already active are still recorded.
- Imported JSON is limited to 8 MiB and scenarios to 1 MiB. CLI and browser exports use compact JSON and check the serialized UTF-8 byte length against the same import limit.

This is an application-level sandbox for the bundled SQLite API, not an operating-system security boundary for arbitrary native code. No HTTP endpoint opens a path, and the internal worker mode is not a public interface for opening databases.

### HTTP interface

The server accepts only its actual `127.0.0.1:port` Host and, if one is supplied, the matching Origin. Every `/api/*` request requires a random per-launch token. The token is not placed in query parameters or artifacts. No permissive CORS is enabled, and the Content-Security-Policy restricts scripts and network requests to the local origin. Request bodies are bounded. Import validation only parses data; execution requires an explicit reset, replay or execute request.

## Observation and interpretation

Sequence numbers are assigned as events reach the coordinator. A worker emits a busy event before it resumes retrying, so a schedule can wait for that event before a second connection releases the contention. Timestamps across processes, internal lock actions, exact retry counts and durations are not deterministic. Dependencies express only requested order and confirmed observations.

Transaction state and autocommit are API values sampled after a statement completes, not continuously polled. File sizes are filesystem samples labeled with the elapsed time of the measurement. WAL byte size is not a live frame count. Checkpoint frame counts come only from explicit user statements. No hidden checkpoint updates the dashboard; SQLite's own behavior when a connection closes still applies when workers are terminated.

Keep facts and interpretations apart. Facts: "busy callback observed", "extended code 517", "log 3, checkpointed 2". Interpretation: "a reader probably prevented further checkpoint progress". Do not infer lock ownership from `SQLITE_BUSY`. A PASSIVE checkpoint can return successfully while incomplete.

SQLite references: [isolation](https://www.sqlite.org/isolation.html), [WAL](https://www.sqlite.org/wal.html), [atomic commit](https://www.sqlite.org/atomiccommit.html), [busy callbacks](https://www.sqlite.org/c3ref/busy_handler.html), [checkpoint modes](https://www.sqlite.org/c3ref/wal_checkpoint_v2.html) and [transaction modes](https://www.sqlite.org/lang_transaction.html).

## Editing experiments in the interface

Editing an operation changes the executable scenario. Recorded operations and events keep their original SQL and parameters, and the schedule labels differing observations as historical. Reset initializes a fresh database with the edited settings; Run schedule also executes the edited operations. After a successful Run schedule, editors offer the schema-independent query `SELECT 1` for custom work.

Each interface action remembers which experiment it started in. A late success, error or completion callback from an earlier experiment cannot replace current editors, notices, errors or pending state, and a state response older than the last applied one is rejected. The event inspector uses the same typed result table as connection cards, including pagination and preview-limit indicators.
