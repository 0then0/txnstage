use crate::model::*;
use serde_json::{Value, json};
mod fk_rebuild;
fn op(
    id: &str,
    c: &str,
    sql: &str,
    after: Vec<Dependency>,
    expect: Value,
    note: &str,
) -> Operation {
    Operation {
        id: id.into(),
        connection: c.into(),
        sql: sql.into(),
        params: vec![],
        after,
        expect,
        note: note.into(),
    }
}
fn dep(id: &str, event: &str) -> Dependency {
    Dependency {
        operation: id.into(),
        event: event.into(),
    }
}
fn done(id: &str) -> Vec<Dependency> {
    vec![dep(id, "completed")]
}
fn rows(v: i64) -> Value {
    json!({"rows":[[{"type":"integer","value":v.to_string()}]]})
}
fn base(name: &str, description: &str) -> Scenario {
    Scenario {
        format_version: FORMAT,
        name: name.into(),
        description: description.into(),
        settings: Settings::default(),
        initial: vec![
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER NOT NULL)".into(),
            "INSERT INTO t VALUES (1, 1)".into(),
        ],
        operations: vec![],
    }
}
pub fn builtins() -> Vec<Scenario> {
    let mut snapshot = base(
        "Stale snapshot",
        "A read snapshot stays at V1 after B commits V2. Retrying cannot promote an obsolete snapshot; roll back and start again. Expected extended error 517. https://www.sqlite.org/isolation.html",
    );
    snapshot.operations = vec![
        op(
            "a_begin",
            "A",
            "BEGIN",
            vec![],
            Value::Null,
            "Start a deferred transaction.",
        ),
        op(
            "a_read",
            "A",
            "SELECT v FROM t WHERE id = 1",
            done("a_begin"),
            rows(1),
            "Pin A's snapshot at V1.",
        ),
        op(
            "b_write",
            "B",
            "UPDATE t SET v = 2 WHERE id = 1",
            done("a_read"),
            Value::Null,
            "B commits V2 in autocommit mode.",
        ),
        op(
            "a_still",
            "A",
            "SELECT v FROM t WHERE id = 1",
            done("b_write"),
            rows(1),
            "A still observes V1.",
        ),
        op(
            "a_promote",
            "A",
            "UPDATE t SET v = 3 WHERE id = 1",
            done("a_still"),
            json!({"extended":517}),
            "Expected SQLITE_BUSY_SNAPSHOT (517), even with a nonzero busy budget.",
        ),
        op(
            "a_rollback",
            "A",
            "ROLLBACK",
            vec![dep("a_promote", "failed")],
            Value::Null,
            "Discard the stale snapshot.",
        ),
        op(
            "a_fresh",
            "A",
            "SELECT v FROM t WHERE id = 1",
            done("a_rollback"),
            rows(2),
            "A fresh statement sees V2.",
        ),
    ];
    let mut writers = base(
        "Two writers · retry",
        "SQLite allows one writer at a time. Dispatch B, wait for an observed busy callback, then commit A. This retry policy is instrumented, not sqlite3_busy_timeout. https://www.sqlite.org/c3ref/busy_handler.html",
    );
    writers.operations = vec![
        op(
            "a_begin",
            "A",
            "BEGIN IMMEDIATE",
            vec![],
            Value::Null,
            "A obtains write access.",
        ),
        op(
            "a_write",
            "A",
            "UPDATE t SET v = 2 WHERE id = 1",
            done("a_begin"),
            Value::Null,
            "Keep A's transaction open.",
        ),
        op(
            "b_write",
            "B",
            "UPDATE t SET v = 3 WHERE id = 1",
            done("a_write"),
            Value::Null,
            "Dispatch without waiting; B may retry.",
        ),
        op(
            "a_commit",
            "A",
            "COMMIT",
            vec![dep("b_write", "busy")],
            Value::Null,
            "After a confirmed busy callback, release A so B can finish.",
        ),
        op(
            "b_read",
            "B",
            "SELECT v FROM t WHERE id = 1",
            vec![dep("b_write", "completed"), dep("a_commit", "completed")],
            rows(3),
            "B's write committed.",
        ),
    ];
    let mut zero = writers.clone();
    zero.name = "Two writers · no wait".into();
    zero.description = "With a zero retry budget, B's write returns SQLITE_BUSY (5) while A holds the writer slot. A then commits; a fresh read sees A's value. Compare with the retry experiment. https://www.sqlite.org/c3ref/busy_handler.html".into();
    zero.settings.busy_ms = 0;
    zero.operations[2].expect = json!({"extended":5});
    zero.operations[2].note =
        "B cannot write while A holds the writer slot; expect SQLITE_BUSY (5) without retrying."
            .into();
    zero.operations[3].after = vec![dep("b_write", "failed")];
    zero.operations[3].note = "After B's write fails, commit A's transaction.".into();
    zero.operations[4].after = done("a_commit");
    zero.operations[4].expect = rows(2);
    zero.operations[4].note =
        "Read A's committed value. B's failed write did not change it.".into();
    let mut deferred = base(
        "DEFERRED vs IMMEDIATE · deferred",
        "The same read-modify-write operation contends at promotion under DEFERRED. Compare with IMMEDIATE, where contention moves to BEGIN. Neither choice is universally better. https://www.sqlite.org/lang_transaction.html",
    );
    deferred.operations = vec![
        op(
            "a_begin",
            "A",
            "BEGIN DEFERRED",
            vec![],
            Value::Null,
            "DEFERRED does not request write access yet.",
        ),
        op(
            "a_read",
            "A",
            "SELECT v FROM t WHERE id = 1",
            done("a_begin"),
            rows(1),
            "Read before modifying.",
        ),
        op(
            "b_write",
            "B",
            "UPDATE t SET v = v + 1 WHERE id = 1",
            done("a_read"),
            Value::Null,
            "Another business operation commits.",
        ),
        op(
            "a_write",
            "A",
            "UPDATE t SET v = v + 1 WHERE id = 1",
            done("b_write"),
            json!({"extended":517}),
            "Contention appears at snapshot promotion.",
        ),
        op(
            "a_rollback",
            "A",
            "ROLLBACK",
            vec![dep("a_write", "failed")],
            Value::Null,
            "Restart the whole transaction to retry.",
        ),
    ];
    let mut immediate = base("DEFERRED vs IMMEDIATE · immediate", &deferred.description);
    immediate.operations = vec![
        op(
            "a_begin",
            "A",
            "BEGIN IMMEDIATE",
            vec![],
            Value::Null,
            "Request write access before reading.",
        ),
        op(
            "a_read",
            "A",
            "SELECT v FROM t WHERE id = 1",
            done("a_begin"),
            rows(1),
            "Same business operation and initial data.",
        ),
        op(
            "b_begin",
            "B",
            "BEGIN IMMEDIATE",
            done("a_read"),
            Value::Null,
            "Contention now appears at B's BEGIN.",
        ),
        op(
            "a_write",
            "A",
            "UPDATE t SET v = v + 1 WHERE id = 1",
            vec![dep("b_begin", "busy")],
            Value::Null,
            "A can finish its read-modify-write.",
        ),
        op(
            "a_commit",
            "A",
            "COMMIT",
            done("a_write"),
            Value::Null,
            "Release write access.",
        ),
        op(
            "b_read",
            "B",
            "SELECT v FROM t WHERE id = 1",
            vec![dep("b_begin", "completed"), dep("a_commit", "completed")],
            rows(2),
            "B reads the committed value.",
        ),
        op(
            "b_write",
            "B",
            "UPDATE t SET v = v + 1 WHERE id = 1",
            done("b_read"),
            Value::Null,
            "B performs the same increment.",
        ),
        op(
            "b_commit",
            "B",
            "COMMIT",
            done("b_write"),
            Value::Null,
            "Commit B.",
        ),
    ];
    let mut checkpoint = base(
        "Reader & checkpoint",
        "A reader can prevent a PASSIVE checkpoint from processing every WAL frame. Compare log and checkpointed counts, not just the busy flag or WAL size. Autocheckpoint is disabled on all workers. https://www.sqlite.org/c3ref/wal_checkpoint_v2.html",
    );
    checkpoint.operations = vec![
        op(
            "a_begin",
            "A",
            "BEGIN",
            vec![],
            Value::Null,
            "Hold a read transaction.",
        ),
        op(
            "a_read",
            "A",
            "SELECT v FROM t WHERE id = 1",
            done("a_begin"),
            rows(1),
            "Pin a read snapshot.",
        ),
        op(
            "b_write",
            "B",
            "UPDATE t SET v = 2 WHERE id = 1",
            done("a_read"),
            Value::Null,
            "Create frames newer than A's snapshot.",
        ),
        op(
            "c_partial",
            "C",
            "PRAGMA wal_checkpoint(PASSIVE)",
            done("b_write"),
            json!({"partial_checkpoint":true}),
            "Observe busy, log, checkpointed; success can still mean incomplete.",
        ),
        op(
            "a_end",
            "A",
            "ROLLBACK",
            done("c_partial"),
            Value::Null,
            "Release the reader.",
        ),
        op(
            "c_full",
            "C",
            "PRAGMA wal_checkpoint(PASSIVE)",
            done("a_end"),
            json!({"full_checkpoint":true}),
            "All frames can now be checkpointed.",
        ),
        op(
            "c_truncate",
            "C",
            "PRAGMA wal_checkpoint(TRUNCATE)",
            done("c_full"),
            json!({"rows":[[{"type":"integer","value":"0"},{"type":"integer","value":"0"},{"type":"integer","value":"0"}]]}),
            "Explicitly truncate the WAL.",
        ),
    ];
    let mut scenarios = vec![snapshot, writers, zero, deferred, immediate, checkpoint];
    scenarios.extend(fk_rebuild::builtins());
    scenarios
}
