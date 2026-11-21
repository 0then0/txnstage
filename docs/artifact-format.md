# Artifact format

TxnStage saves experiments in two kinds of JSON file:

- A **scenario** defines the starting database, connection settings, SQL operations, event dependencies and expected results.
- A **recording** contains a replayable scenario, its SQLite environment and observed events. It can be inspected or rerun in a fresh database.

Version 1 contains SQL steps. Version 2 adds [process crash and reopen steps](#process-crash-and-reopen-steps). Version 3 adds [temporary database copy steps and checks](#database-copy-steps). TxnStage reads all three versions and raises the version of an exported run when one of its steps requires a newer format. A recording has the same version as the scenario inside it.

Import validates and displays a file without executing SQL; execution requires **Rerun in fresh DB** or the CLI `run` command. Files are limited to 8 MiB. Unknown scenario, operation, settings and recording envelope fields and unknown versions are rejected. Event data allows additional fields, while its envelopes, result shapes and sequence numbers are validated.

## Executable scenario

```json
{
  "format_version": 1,
  "name": "Read one value",
  "description": "A minimal example",
  "settings": {"journal_mode":"wal","busy_ms":2000,"deadline_ms":5000,"row_limit":100,"connections":2},
  "initial": ["CREATE TABLE t(v INTEGER)", "INSERT INTO t VALUES(1)"],
  "operations": [{
    "id": "read", "connection":"A", "sql":"SELECT v FROM t", "params":[],
    "after":[], "expect":{"rows":[[{"type":"integer","value":"1"}]]}, "note":"Read committed initial data"
  }]
}
```

Each initial entry is one statement. Operations have unique 1–80 character ASCII IDs using letters, digits, `_` or `-`; `seed_*` is reserved. Connections are A/B and optionally C. Dependencies refer to earlier entries, preventing cycles. Supported barrier events are running, busy, completed, failed, cancelled and timed_out. A failed dependency barrier is reported with the observed terminal event. For sequential work on one connection, depend on its terminal event; running/busy barriers are useful across different connections. Array order defines submission order; different workers can overlap when dependencies allow it. Omitted optional operation fields default to empty arrays, null expectation and empty note.

Expectations support exact typed `rows`, an `extended` SQLite code, `partial_checkpoint: true`, and `full_checkpoint: true`. Checkpoint expectations require exactly one row with integer busy/log/checkpointed columns, a busy flag of 0 or 1, and nonnegative counts with checkpointed <= log. Unavailable (-1) or missing counts are rejected. Full requires busy=0 and equal counts; partial requires log > checkpointed; the TRUNCATE experiment also expects `[0,0,0]`. Null expectation means successful completion is required. An expected error never changes the recorded SQL error's category or lifecycle.

SQL completion and expectation matching are separate results. A SELECT can complete successfully while its rows fail an expectation. An expected SQLite error can satisfy a scenario. The [FK preservation example](fk-rebuild-lab.md#observation-and-preservation) demonstrates this distinction.

Version 3 records the outcome of each explicit expectation in the terminal event. A version 3 dependency on `completed` also requires any expectation on that step to pass before the schedule proceeds. A checkpoint can therefore report `completed` at the SQL level and still block a copy when it left frames behind.

Parameters and result cells have these representations:

```json
[
  {"type":"null"},
  {"type":"integer","value":"9223372036854775807"},
  {"type":"real","value":1.25},
  {"type":"text","value":"a;b"},
  {"type":"blob","value":"00ff"}
]
```

REAL outputs may use `"Infinity"`/`"-Infinity"`; REAL inputs must be finite numbers. INTEGER inputs must fit i64 and be decimal strings. BLOB inputs are even-length hex strings up to 64 KiB. TEXT inputs are at most 64 KiB. Display truncation is explicit at result level (`rows_truncated`, `cell_previews`, optionally `recording_limit`). Counters such as `row_count` and `changes` are strings to avoid JavaScript rounding. TEXT preview uses replacement for invalid UTF-8. BLOB preview is at most 128 bytes, TEXT at most 4096 bytes. Truncated previews do not include the original cell's byte length.

## Process crash and reopen steps

A version 2 scenario can end one connection's worker process abruptly and then open the same database with a new worker:

```json
[
  {"id":"a_crash", "connection":"A", "kind":"crash", "after":[{"operation":"a_update","event":"completed"}], "note":"Terminate A's process"},
  {"id":"a_reopen", "connection":"A", "kind":"reopen", "after":[{"operation":"a_crash","event":"completed"}], "note":"New worker, same file"}
]
```

`kind` defaults to `sql`, which is omitted on export. The `crash` and `reopen` steps take no `sql`, `params` or `expect`. Assert the outcome with a following SQL step and its ordinary expectation. They report `created`, `running` and a terminal event like SQL steps, so other steps depend on them through `after` in the usual way.

- `crash` forcibly terminates the worker process of that connection and waits until the process has exited and been reaped. The step then ends as `completed` and the connection is closed. Other connections and all database files are untouched.
- `reopen` starts a new worker for the same connection ID on the same database file and waits for it to report ready. Initialization SQL is not rerun and no file is removed or recreated.

Validation follows the array order, which is the submission order. A lifecycle step must list a terminal event of the previous step on its connection in `after`, and so must the step that follows a lifecycle step on that connection. A scenario is rejected if it runs SQL on a closed connection, crashes a closed connection or reopens an open one. A recorded `reopen` that ended as failed, cancelled or timed_out leaves the connection closed, so a later retry remains valid.

## Database copy steps

Version 3 supports two creation steps. They take an operation ID, connection, dependencies and note.

- `copy_main` copies the bytes of the working database's main file into its own directory.
- `vacuum_into` takes a snapshot of the working database with SQLite's `VACUUM INTO`.

Each copy lives inside the current experiment's temporary directory. An SQL step can set `copy` to the ID of an earlier creation step. The copy must have completed successfully. The worker opens it on a fresh read-only connection for that statement and applies the usual SQL restrictions. An ordinary SQL step with no `copy` field still uses its connection to the working database.

```json
[
  {"id":"snapshot","connection":"A","kind":"vacuum_into"},
  {"id":"check","connection":"B","copy":"snapshot","sql":"PRAGMA integrity_check",
   "after":[{"operation":"snapshot","event":"completed"}],
   "expect":{"rows":[[{"type":"text","value":"ok"}]]}}
]
```

The copy reference is a logical ID. TxnStage assigns a fresh destination on each run. See [scenario 16](../examples/16.scenario.json) for the checkpoint barriers, integrity checks and separate exact row checks.

## Recorded run

Use **Export run** or `txnstage run --output` to create a recording. Its top-level fields are:

- `format_version` is the version of the recorded scenario.
- `scenario` contains the steps to replay, connection settings and initialization SQL.
- `environment`: SQLite version, source ID, compile options, TxnStage version, operating system, architecture, settings and busy policy.
- `events`: observations in sequence order. Each event has `seq`, `elapsed_ms`, `connection`, `operation`, `kind` and `data`.

The [sample recordings](../examples/recordings/) are complete importable examples.

`created` and `running` data include the exact SQL and parameters. Terminal data includes result or error, busy call count, duration, autocommit, transaction state and a timestamped filesystem sample. SQL errors contain `category: "sql"`, `primary`, `extended`, and message. Validation, interrupted and infrastructure failures have distinct categories. Cancellation requests are separate events; a forced kill adds `state_lost: true` and `worker_lost`. PIDs, sizes, elapsed times and callbacks are observations of that run, not portable expectations.

Events of a lifecycle step carry `lifecycle: "crash"` or `"reopen"` and the worker `generation`, which starts at 1 for each connection and increases with every reopen attempt. `ready` events carry the generation of the process that reported them. A completed crash records `planned: true`, the terminated `pid`, `termination` (`method`, `cooperative_cleanup: false`, `exit_confirmed`, `reaped`, `exit_status`) and `session_state` with the transaction state last sampled before the crash. The method is `SIGKILL` on Unix and `TerminateProcess` on Windows. A completed reopen records the new `generation` and `pid`, the sampled `transaction` and `autocommit` of the new connection, and text describing what was kept and reapplied. A failed reopen has an infrastructure error and `connection: "closed"`. The scenario holds only the replayable intent: step kind, connection and dependencies. Process IDs, exit status, timestamps, durations and file sizes stay in events and are never compared on replay.

Copy creation events identify the method and the copy ID. Successful creation records `copy_bytes`. SQL probes record that ID and whether the read-only open succeeded in `copy_opened`. Readings and errors use the standard result format. Version 3 terminal events for steps with expectations include `assertion.matched` and a message if the check failed.

Run exports normally contain the submitted operations. Their dependencies are reconstructed from the latest confirmed terminal, busy or running event of the most recent earlier operation on each connection at each submission boundary. This captures observed user ordering without inventing sleeps or inferring internal lock states. Concurrent requests with no confirmed prior event retain a weaker order. Busy barriers can have different retry counts on replay.

When a version 3 run follows the declared schedule and stops early, the recording keeps the remaining steps for a fresh replay. Steps that were never submitted have no events. Use Export scenario to save an editable schedule without the observations.

A recording may be incomplete or cancelled. It is still viewable; rerunning can fail its successful-completion expectations. Cancel actions are recorded events rather than executable operations, so repeating cancellation requires an explicit user action. Fresh reruns use a new database file and new workers. Crash and reopen are executable steps, unlike cancellation: a rerun terminates and restarts the worker at the same logical points.

Recordings store observations for individual steps. For version 1 and 2 recordings, `completed` refers to execution and the expectation can be evaluated by rerunning. Version 3 stores individual check outcomes alongside the observed data. Replay evaluates the expectations against fresh execution.

The viewer compares recorded SQLite version, source ID, application version, OS and architecture. The headless CLI prints platform/SQLite differences to stderr before replay. Compile options and settings remain available for inspection. Differences are informational; execution remains explicit and never promises identical observations.

Exports use compact JSON. Both CLI and browser check the serialized UTF-8 bytes against the 8 MiB import cap, including JSON overhead and multibyte text. Pretty-printing an exported recording externally can exceed that cap.
