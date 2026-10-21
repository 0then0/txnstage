# Artifact format v1

There are two JSON documents. Both use `format_version: 1`. Unknown scenario/operation/settings/recording envelope fields and unknown versions are rejected. Event data is extensible, with validated envelopes, result shapes and ordered sequence numbers. The server caps imports at 8 MiB and does not execute them.

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

Expectations support exact typed `rows`, an `extended` SQLite code, `partial_checkpoint: true`, and `full_checkpoint: true`. Checkpoint expectations require exactly one row with integer busy/log/checkpointed columns, a busy flag of 0 or 1, and nonnegative counts with checkpointed <= log. Unavailable (-1) or missing counts are rejected. Full requires busy=0 and equal counts; partial requires log > checkpointed; the TRUNCATE runbook also expects `[0,0,0]`. Null expectation means successful completion is required. An expected error never changes the recorded SQL error's category or lifecycle.

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

REAL outputs may use `"Infinity"`/`"-Infinity"`; REAL inputs must be finite numbers. INTEGER inputs must fit i64 and be decimal strings. BLOB inputs are even-length hex strings up to 64 KiB. TEXT inputs are at most 64 KiB. Display truncation is explicit at result level (`rows_truncated`, `cell_previews`, optionally `recording_limit`). Counters such as `row_count` and `changes` are strings to avoid JavaScript rounding. TEXT preview uses replacement for invalid UTF-8. BLOB preview is at most 128 bytes, TEXT at most 4096 bytes; exact byte-length metadata for truncated cells is not exposed in v0.1.

## Recorded run

This schematic envelope shows the recording fields. Replace the abbreviated `scenario` and environment values with complete data when constructing a recording; use the [sample recordings](../examples/recordings/) for importable files.

```json
{
  "format_version": 1,
  "scenario": {"...": "the executed scenario, not the current editor draft"},
  "environment": {"sqlite_version":"...","sqlite_source_id":"...","compile_options":[],"os":"...","arch":"...","txnstage":"0.1.0","settings":{},"busy_policy":"..."},
  "events": [{"seq":1,"elapsed_ms":0,"connection":"A","operation":"","kind":"ready","data":{"pid":123}}]
}
```

`created` and `running` data include the exact SQL and parameters. Terminal data includes result or error, busy call count, duration, autocommit, transaction state and a timestamped filesystem sample. SQL errors contain `category: "sql"`, `primary`, `extended`, and message. Validation, interrupted and infrastructure failures have distinct categories. Cancellation requests are separate events; a forced kill adds `state_lost: true` and `worker_lost`. PIDs, sizes, elapsed times and callbacks are observations of that run, not portable expectations.

Exported run scenarios contain the operations actually submitted. Their dependencies are reconstructed from the latest confirmed terminal, busy or running event of the most recent earlier operation on each connection at each submission boundary. This captures observed user ordering without inventing sleeps or inferring internal lock states. Concurrent requests with no confirmed prior event retain a weaker order. Busy barriers can have different retry counts on replay. The executable template can separately retain steps not yet run through Export scenario.

A recording may be incomplete or cancelled. It is still viewable; rerunning is not guaranteed to satisfy its successful-completion expectations. To replay cancellation itself, v0.1 requires an explicit user cancel; cancel actions are recorded events rather than an executable operation kind. Fresh reruns never reuse a database file or a worker state. Expected 517 is a portable SQLite assertion; exact timeline bytes are not.

The viewer compares recorded SQLite version, source ID, application version, OS and architecture. The headless CLI prints platform/SQLite differences to stderr before replay. Compile options and settings remain available for inspection. Differences are informational; execution remains explicit and never promises identical observations.

Exports use compact JSON. Both CLI and browser check the serialized UTF-8 bytes against the 8 MiB import cap, including JSON overhead and multibyte text. Pretty-printing an exported recording externally can exceed that cap.
