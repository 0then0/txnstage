# Artifact format

TxnStage saves experiments in two kinds of JSON file:

- A **scenario** defines the starting database, connection settings, SQL operations, event dependencies and expected results.
- A **recording** contains the executed scenario, its SQLite environment and observed events. It can be inspected or rerun in a fresh database.

Two format versions are read. Version 1 contains SQL steps only. Version 2 adds the typed `crash` and `reopen` steps described under [Process crash and reopen steps](#process-crash-and-reopen-steps); everything else is identical. TxnStage raises the version to 2 only when a run contains such a step. Version 1 scenarios and their recordings therefore stay version 1 and remain readable by releases that know only version 1. A version 1 file containing a `kind` field is rejected. A recording has the same version as the scenario inside it.

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

SQL completion and expectation matching are separate results. A SELECT can complete successfully while its rows fail an expectation; conversely, an expected SQLite error can satisfy a scenario. The [FK preservation example](fk-rebuild-lab.md#observation-and-preservation) demonstrates this distinction.

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

`kind` is `sql` (the default, omitted on export), `crash` or `reopen`. Lifecycle steps take no `sql`, `params` or `expect`; assert the outcome with a following SQL step and its ordinary expectation. They report `created`, `running` and a terminal event like SQL steps, so other steps depend on them through `after` in the usual way.

- `crash` forcibly terminates the worker process of that connection and waits until the process has exited and been reaped. The step then ends as `completed` and the connection is closed. Other connections and all database files are untouched.
- `reopen` starts a new worker for the same connection ID on the same database file and waits for it to report ready. Initialization SQL is not rerun and no file is removed or recreated.

Validation follows the array order, which is the submission order. A lifecycle step must list a terminal event of the previous step on its connection in `after`, and so must the step that follows a lifecycle step on that connection. A scenario is rejected if it runs SQL on a closed connection, crashes a closed connection or reopens an open one. A recorded `reopen` that ended as failed, cancelled or timed_out leaves the connection closed, so a later retry remains valid.

## Recorded run

Use **Export run** or `txnstage run --output` to create a recording. Its top-level fields are:

- `format_version`: the recording format version, currently `1`.
- `scenario`: the operations actually submitted, with their settings and initialization SQL.
- `environment`: SQLite version, source ID, compile options, TxnStage version, operating system, architecture, settings and busy policy.
- `events`: observations in sequence order. Each event has `seq`, `elapsed_ms`, `connection`, `operation`, `kind` and `data`.

The [sample recordings](../examples/recordings/) are complete importable examples.

`created` and `running` data include the exact SQL and parameters. Terminal data includes result or error, busy call count, duration, autocommit, transaction state and a timestamped filesystem sample. SQL errors contain `category: "sql"`, `primary`, `extended`, and message. Validation, interrupted and infrastructure failures have distinct categories. Cancellation requests are separate events; a forced kill adds `state_lost: true` and `worker_lost`. PIDs, sizes, elapsed times and callbacks are observations of that run, not portable expectations.

Events of a lifecycle step carry `lifecycle: "crash"` or `"reopen"` and the worker `generation`, which starts at 1 for each connection and increases with every reopen attempt. `ready` events carry the generation of the process that reported them. A completed crash records `planned: true`, the terminated `pid`, `termination` (`method`, `cooperative_cleanup: false`, `exit_confirmed`, `reaped`, `exit_status`) and `session_state` with the transaction state last sampled before the crash. The method is `SIGKILL` on Unix and `TerminateProcess` on Windows. A completed reopen records the new `generation` and `pid`, the sampled `transaction` and `autocommit` of the new connection, and text describing what was kept and reapplied. A failed reopen has an infrastructure error and `connection: "closed"`. The scenario holds only the replayable intent: step kind, connection and dependencies. Process IDs, exit status, timestamps, durations and file sizes stay in events and are never compared on replay.

Exported run scenarios contain the operations actually submitted. Their dependencies are reconstructed from the latest confirmed terminal, busy or running event of the most recent earlier operation on each connection at each submission boundary. This captures observed user ordering without inventing sleeps or inferring internal lock states. Concurrent requests with no confirmed prior event retain a weaker order. Busy barriers can have different retry counts on replay. The executable template can separately retain steps not yet run through Export scenario.

A recording may be incomplete or cancelled. It is still viewable; rerunning can fail its successful-completion expectations. Cancel actions are recorded events rather than executable operations, so repeating cancellation requires an explicit user action. Fresh reruns use a new database file and new workers. Crash and reopen are executable steps, unlike cancellation: a rerun terminates and restarts the worker at the same logical points.

Recordings store SQL events and declared expectations without a separate overall assertion result. A historical `completed` event means that the SQL statement completed; it does not establish that its expectation matched. Rerun the scenario to evaluate its expectations against a fresh execution.

The viewer compares recorded SQLite version, source ID, application version, OS and architecture. The headless CLI prints platform/SQLite differences to stderr before replay. Compile options and settings remain available for inspection. Differences are informational; execution remains explicit and never promises identical observations.

Exports use compact JSON. Both CLI and browser check the serialized UTF-8 bytes against the 8 MiB import cap, including JSON overhead and multibyte text. Pretty-printing an exported recording externally can exceed that cap.
