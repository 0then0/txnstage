use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use txnstage::{
    engine::{Engine, KILL_METHOD, READY_DEADLINE_MS},
    model::*,
    scenarios,
};
fn binary() -> PathBuf {
    env!("CARGO_BIN_EXE_txnstage").into()
}
fn blank() -> Scenario {
    let mut s = scenarios::builtins().remove(0);
    s.operations.clear();
    s
}
fn operation(id: &str, c: &str, sql: &str) -> Operation {
    Operation {
        id: id.into(),
        connection: c.into(),
        kind: OperationKind::Sql,
        sql: sql.into(),
        params: vec![],
        after: vec![],
        expect: Value::Null,
        note: String::new(),
    }
}
async fn exec(e: &Arc<Engine>, id: &str, c: &str, sql: &str) -> Event {
    e.submit(operation(id, c, sql)).await.unwrap();
    e.wait_terminal(id).await.unwrap()
}
#[tokio::test]
async fn all_runbooks_and_portable_recordings() {
    for scenario in scenarios::builtins() {
        let e = Engine::new(scenario.clone(), binary()).await.unwrap();
        e.replay()
            .await
            .unwrap_or_else(|err| panic!("{}: {err}", scenario.name));
        let record = e.recording().await;
        record.validate().unwrap();
        let encoded = serde_json::to_vec(&record).unwrap();
        let decoded: Recording = serde_json::from_slice(&encoded).unwrap();
        e.shutdown().await;
        let replay = Engine::new(decoded.scenario, binary()).await.unwrap();
        replay.replay().await.unwrap();
        replay.shutdown().await;
    }
}

#[test]
fn documented_scenario_runs_and_exports_a_valid_recording() {
    let document = include_str!("../docs/artifact-format.md");
    let example = document
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let scenario: Scenario = serde_json::from_str(example).unwrap();
    scenario.validate().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("documented.scenario.json");
    std::fs::write(&path, example).unwrap();
    let output = std::process::Command::new(binary())
        .arg("run")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recording: Recording = serde_json::from_slice(&output.stdout).unwrap();
    recording.validate().unwrap();
    assert_eq!(recording.scenario.name, scenario.name);
    assert!(
        recording
            .events
            .iter()
            .any(|event| event.operation == "read" && event.kind == "completed")
    );
}

#[tokio::test]
async fn foreign_key_check_is_read_only_and_reports_real_violations() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    assert_eq!(
        exec(&e, "fk_off", "A", "PRAGMA foreign_keys=OFF")
            .await
            .kind,
        "completed"
    );
    assert_eq!(
        exec(
            &e,
            "child",
            "A",
            "CREATE TABLE child(id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES t(id))"
        )
        .await
        .kind,
        "completed"
    );
    assert_eq!(
        exec(&e, "orphan", "A", "INSERT INTO child VALUES(7,999)")
            .await
            .kind,
        "completed"
    );
    // Like the lab, B reads the actual data before running diagnostics. In the
    // bundled SQLite, a bare FK-check PRAGMA can use an already cached schema.
    assert_eq!(
        exec(&e, "fresh_data", "B", "SELECT id,parent_id FROM child")
            .await
            .data["result"]["rows"],
        json!([[{"type":"integer","value":"7"},{"type":"integer","value":"999"}]])
    );
    for c in ["A", "B"] {
        let check = exec(&e, &format!("check_{c}"), c, "PRAGMA foreign_key_check").await;
        assert_eq!(check.kind, "completed");
        assert_eq!(
            check.data["result"]["rows"],
            json!([[{"type":"text","value":"child"},{"type":"integer","value":"7"},{"type":"text","value":"t"},{"type":"integer","value":"0"}]])
        );
    }
    for (i, sql) in [
        "PRAGMA foreign_key_check=OFF",
        "PRAGMA foreign_key_check(child)",
        "PRAGMA main.foreign_key_check('/tmp/outside.db')",
        "PRAGMA writable_schema=ON",
        "PRAGMA journal_mode=DELETE",
    ]
    .iter()
    .enumerate()
    {
        let event = exec(&e, &format!("deny_check_{i}"), "A", sql).await;
        assert_eq!(event.kind, "failed", "{sql}");
        assert_eq!(event.data["error"]["extended"], 23, "{sql}");
    }
    assert_eq!(
        exec(&e, "still_orphan", "B", "SELECT id,parent_id FROM child")
            .await
            .data["result"]["rows"],
        json!([[{"type":"integer","value":"7"},{"type":"integer","value":"999"}]])
    );
    assert_eq!(
        exec(&e, "still_off", "A", "PRAGMA foreign_keys").await.data["result"]["rows"],
        json!([[{"type":"integer","value":"0"}]])
    );
    e.shutdown().await;
}

#[tokio::test]
async fn fk_lab_artifacts_and_negative_preservation_assertion() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for (i, builtin) in scenarios::builtins().into_iter().enumerate() {
        let path = root.join(format!("examples/{:02}.scenario.json", i + 1));
        let scenario: Scenario = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&scenario).unwrap(),
            serde_json::to_value(builtin).unwrap()
        );
        let path = root.join(format!("examples/recordings/{:02}.recording.json", i + 1));
        let recording: Recording = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        recording.validate().unwrap();
        let e = Engine::new(recording.scenario, binary()).await.unwrap();
        e.replay().await.unwrap();
        e.shutdown().await;
    }
    let path = root.join("examples/fk-preservation-fails.scenario.json");
    let negative: Scenario = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    // Keep 07's damaging SQL and observations, then assert preservation after
    // diagnostics. A mismatch must not race CLI shutdown with pending diagnostics.
    let mut expected = scenarios::builtins().remove(6);
    let control = scenarios::builtins().remove(11);
    let preservation = control
        .operations
        .iter()
        .find(|o| o.id == "b_after_commit")
        .unwrap();
    let mut preservation = preservation.clone();
    preservation.id = "b_preservation".into();
    preservation.after = vec![Dependency {
        operation: "b_fk_check".into(),
        event: "completed".into(),
    }];
    preservation.note = "Read every original column from a fresh snapshot. Expect a preservation mismatch because the children were deleted; experiment 12 restores them and passes the same check.".into();
    expected.operations.push(preservation);
    expected.name = negative.name.clone();
    expected.description = negative.description.clone();
    assert_eq!(
        serde_json::to_value(expected).unwrap(),
        serde_json::to_value(&negative).unwrap()
    );

    let e = Engine::new(negative, binary()).await.unwrap();
    assert_eq!(
        e.replay().await.unwrap_err(),
        "Row expectation mismatch for b_preservation"
    );
    let record = e.recording().await;
    record.validate().unwrap();
    assert!(
        record
            .events
            .iter()
            .all(|e| !["failed", "cancelled", "timed_out"].contains(&e.kind.as_str()))
    );
    let commit = record
        .events
        .iter()
        .find(|e| e.operation == "a_commit" && e.kind == "completed")
        .unwrap();
    assert_eq!(commit.data["autocommit"], true);
    let fresh = record
        .events
        .iter()
        .find(|e| e.operation == "b_after_commit" && e.kind == "completed")
        .unwrap();
    assert_eq!(fresh.data["autocommit"], true);
    assert_eq!(fresh.data["result"]["row_count"], "2");
    assert_eq!(
        fresh.data["result"]["rows"][0][1]["value"],
        "9007199254740993"
    );
    e.shutdown().await;
    // Import and fresh replay of the deliberately failing recording keeps that failure.
    let path = root.join("examples/recordings/fk-preservation-fails.recording.json");
    let negative: Recording = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    negative.validate().unwrap();
    let e = Engine::new(negative.scenario, binary()).await.unwrap();
    assert_eq!(
        e.replay().await.unwrap_err(),
        "Row expectation mismatch for b_preservation"
    );
    e.shutdown().await;
}
#[tokio::test]
async fn busy_barrier_keeps_coordinator_and_other_workers_available() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    assert_eq!(
        exec(&e, "begin", "A", "BEGIN IMMEDIATE").await.kind,
        "completed"
    );
    e.submit(operation("blocked", "B", "UPDATE t SET v=9"))
        .await
        .unwrap();
    e.wait_event("blocked", None, "busy", 3000).await.unwrap();
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(1), e.snapshot())
        .await
        .unwrap();
    assert_eq!(snapshot["active"]["B"], "blocked");
    assert_eq!(exec(&e, "commit", "A", "COMMIT").await.kind, "completed");
    assert_eq!(e.wait_terminal("blocked").await.unwrap().kind, "completed");
    assert_eq!(
        exec(&e, "read", "C", "SELECT v FROM t").await.data["result"]["rows"][0][0],
        json!({"type":"integer","value":"9"})
    );
    e.shutdown().await;
}
#[tokio::test]
async fn types_returning_parser_and_error_recovery() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    let mut o = operation("types", "A", "SELECT ?, ?, ?, ?, ?");
    o.params = vec![
        Cell::Integer(i64::MAX.to_string()),
        Cell::Null,
        Cell::Blob("00ff".into()),
        Cell::Text("a;b".into()),
        Cell::Real(1.25),
    ];
    e.submit(o).await.unwrap();
    let ev = e.wait_terminal("types").await.unwrap();
    assert_eq!(
        ev.data["result"]["rows"][0],
        json!([{"type":"integer","value":"9223372036854775807"},{"type":"null"},{"type":"blob","value":"00ff"},{"type":"text","value":"a;b"},{"type":"real","value":1.25}])
    );
    assert_eq!(
        exec(&e, "returning", "A", "UPDATE t SET v=42 RETURNING v")
            .await
            .data["result"]["rows"][0][0]["value"],
        "42"
    );
    assert_eq!(exec(&e,"trigger","A","CREATE TRIGGER tr AFTER INSERT ON t BEGIN UPDATE t SET v=3 WHERE id=new.id; UPDATE t SET v=v+1 WHERE id=new.id; END;").await.kind,"completed");
    assert_eq!(
        exec(&e, "insert", "A", "INSERT INTO t VALUES(2,0)")
            .await
            .kind,
        "completed"
    );
    assert_eq!(
        exec(&e, "trigger_result", "A", "SELECT v FROM t WHERE id=2")
            .await
            .data["result"]["rows"][0][0]["value"],
        "4"
    );
    assert_eq!(
        exec(&e, "multi", "A", "UPDATE t SET v=99; SELECT 1")
            .await
            .data["error"]["category"],
        "validation"
    );
    assert_eq!(
        exec(&e, "unchanged", "A", "SELECT v FROM t WHERE id=1")
            .await
            .data["result"]["rows"][0][0]["value"],
        "42"
    );
    assert_eq!(
        exec(&e, "bad", "A", "SELECT missing FROM t").await.kind,
        "failed"
    );
    assert_eq!(
        exec(&e, "after_error", "A", "SELECT 'semi;colon'; -- ; comment")
            .await
            .kind,
        "completed"
    );
    e.shutdown().await;
}
#[tokio::test]
async fn sandbox_rejects_paths_extensions_and_instrumentation_changes() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("outside.db");
    for (i, sql) in [
        format!("ATTACH '{}' AS x", target.display()),
        format!("VACUUM INTO '{}'", target.display()),
        "PRAGMA busy_timeout=123".into(),
        "PRAGMA wal_autocheckpoint=1".into(),
        "PRAGMA temp_store_directory='/tmp'".into(),
        "SELECT load_extension('bad')".into(),
        "PRAGMA journal_mode=DELETE".into(),
    ]
    .iter()
    .enumerate()
    {
        let ev = exec(&e, &format!("deny_{i}"), "A", sql).await;
        assert_eq!(ev.kind, "failed", "{sql}");
    }
    assert!(!target.exists());
    assert_eq!(
        exec(&e, "ok", "A", "PRAGMA busy_timeout").await.kind,
        "completed"
    );
    e.shutdown().await;
}
const LONG: &str = "WITH RECURSIVE c(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM c WHERE x<1000000000) SELECT sum(x) FROM c";
#[tokio::test]
async fn cancel_long_sql_and_busy_retry() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    e.submit(operation("long", "A", LONG)).await.unwrap();
    e.wait_event("long", None, "running", 2000).await.unwrap();
    e.cancel("A", "cancelled").await;
    assert_eq!(e.wait_terminal("long").await.unwrap().kind, "cancelled");
    assert_eq!(
        exec(&e, "after_cancel", "A", "SELECT 1").await.kind,
        "completed"
    );
    exec(&e, "hold", "A", "BEGIN IMMEDIATE").await;
    e.submit(operation("busy_cancel", "B", "UPDATE t SET v=3"))
        .await
        .unwrap();
    e.wait_event("busy_cancel", None, "busy", 2000)
        .await
        .unwrap();
    e.cancel("B", "cancelled").await;
    assert_eq!(
        e.wait_terminal("busy_cancel").await.unwrap().kind,
        "cancelled"
    );
    assert_eq!(exec(&e, "release", "A", "ROLLBACK").await.kind, "completed");
    e.stop().await;
    assert_eq!(e.snapshot().await["stopped"], true);
}
#[tokio::test]
async fn deadlines_worker_crash_and_cleanup() {
    let mut s = blank();
    // Startup DDL is unrelated to the short operation deadline under test.
    s.initial.clear();
    s.settings.deadline_ms = 150;
    s.settings.busy_ms = 10000;
    let e = Engine::new(s, binary()).await.unwrap();
    let dir = e.directory.path().to_path_buf();
    e.submit(operation("deadline", "A", LONG)).await.unwrap();
    assert_eq!(e.wait_terminal("deadline").await.unwrap().kind, "timed_out");
    assert_eq!(
        exec(&e, "hold", "A", "BEGIN IMMEDIATE").await.kind,
        "completed"
    );
    e.submit(operation(
        "busy_deadline",
        "B",
        "CREATE TABLE deadline_probe(v)",
    ))
    .await
    .unwrap();
    e.wait_event("busy_deadline", None, "busy", 2000)
        .await
        .unwrap();
    assert_eq!(
        e.wait_terminal("busy_deadline").await.unwrap().kind,
        "timed_out"
    );
    e.submit(operation("crashing", "C", LONG)).await.unwrap();
    e.wait_event("crashing", None, "running", 2000)
        .await
        .unwrap();
    e.force_kill("C").await;
    let ev = e.wait_terminal("crashing").await.unwrap();
    assert_eq!(ev.data["state_lost"], true);
    assert_eq!(ev.data["error"]["category"], "infrastructure");
    assert!(e.submit(operation("lost", "C", "SELECT 1")).await.is_err());
    e.stop().await;
    drop(e);
    assert!(!dir.exists(), "owned experiment directory must be removed");
}
#[tokio::test]
async fn observing_state_never_checkpoints_and_row_limit_drains_returning() {
    let mut s = blank();
    s.settings.row_limit = 1;
    let e = Engine::new(s, binary()).await.unwrap();
    exec(&e, "reader_begin", "A", "BEGIN").await;
    exec(&e, "reader", "A", "SELECT * FROM t").await;
    exec(&e, "write", "B", "UPDATE t SET v=2 RETURNING v").await;
    let wal = e.directory.path().join("experiment.db-wal");
    let bytes = std::fs::read(&wal).unwrap();
    for _ in 0..10 {
        e.snapshot().await;
    }
    assert_eq!(bytes, std::fs::read(&wal).unwrap());
    let ev = exec(&e, "checkpoint", "C", "PRAGMA wal_checkpoint(PASSIVE)").await;
    let rows = &ev.data["result"]["rows"][0];
    let n = |v: &Value| v["value"].as_str().unwrap().parse::<i64>().unwrap();
    assert!(n(&rows[1]) > n(&rows[2]));
    exec(&e, "reader_end", "A", "ROLLBACK").await;
    let ev = exec(
        &e,
        "many",
        "B",
        "INSERT INTO t VALUES(2,2),(3,3),(4,4) RETURNING id",
    )
    .await;
    assert_eq!(ev.data["result"]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(ev.data["result"]["row_count"], "3");
    assert_eq!(ev.data["autocommit"], true);
    assert_eq!(
        exec(&e, "count", "A", "SELECT count(*) FROM t").await.data["result"]["rows"][0][0]["value"],
        "4"
    );
    e.shutdown().await;
}
#[test]
fn validation_limits_and_versions() {
    let mut s = blank();
    s.format_version = 99;
    assert!(s.validate().is_err());
    let mut s = blank();
    s.settings.deadline_ms = 999999;
    assert!(s.validate().is_err());
    let mut op = operation("x", "A", "SELECT ?");
    op.params = vec![Cell::Integer("9223372036854775808".into())];
    assert!(validate_operation(&op).is_err());
}
#[tokio::test]
async fn stale_snapshot_ignores_retry_budget_and_real_infinity_is_explicit() {
    for busy_ms in [0, 10000] {
        let mut s = scenarios::builtins().remove(0);
        s.settings.busy_ms = busy_ms;
        let e = Engine::new(s, binary()).await.unwrap();
        e.replay().await.unwrap();
        let snapshot = e.snapshot().await;
        let ev = snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["operation"] == "a_promote" && e["kind"] == "failed")
            .unwrap();
        assert_eq!(ev["data"]["error"]["extended"], 517);
        assert_eq!(ev["data"]["busy_calls"], 0);
        e.shutdown().await;
    }
    let e = Engine::new(blank(), binary()).await.unwrap();
    assert_eq!(
        exec(&e, "infinity", "A", "SELECT 1e999").await.data["result"]["rows"][0][0],
        json!({"type":"real","value":"Infinity"})
    );
    e.shutdown().await;
}
#[cfg(unix)]
#[tokio::test]
async fn cancellation_falls_back_to_killing_an_unresponsive_worker() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    let snapshot = e.snapshot().await;
    let pid = snapshot["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["connection"] == "A" && e["kind"] == "ready")
        .unwrap()["data"]["pid"]
        .as_u64()
        .unwrap();
    e.submit(operation("stuck", "A", LONG)).await.unwrap();
    e.wait_event("stuck", None, "running", 2000).await.unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-STOP", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    e.cancel("A", "cancelled").await;
    let ev = e.wait_terminal("stuck").await.unwrap();
    assert_eq!(ev.kind, "cancelled");
    assert_eq!(ev.data["state_lost"], true);
    e.stop().await;
}

#[tokio::test]
async fn rejected_multi_statement_has_no_prepare_time_pragma_side_effect() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    let before =
        exec(&e, "before", "A", "PRAGMA foreign_keys").await.data["result"]["rows"].clone();
    assert_eq!(
        exec(&e, "multi_pragma", "A", "PRAGMA foreign_keys=OFF; SELECT 1")
            .await
            .kind,
        "failed"
    );
    assert_eq!(
        exec(&e, "after", "A", "PRAGMA foreign_keys").await.data["result"]["rows"],
        before
    );
    let mut recording = e.recording().await;
    recording.events[0].data = json!({"result":{"columns":null}});
    assert!(recording.validate().is_err());
    e.shutdown().await;
}

#[tokio::test]
async fn long_manual_history_remains_a_bounded_replayable_artifact() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    assert!(
        e.submit(operation("seed_fake", "A", "SELECT 1"))
            .await
            .is_err()
    );
    for i in 0..100 {
        assert_eq!(
            exec(
                &e,
                &format!("manual_{i}"),
                if i % 2 == 0 { "A" } else { "B" },
                "SELECT 1"
            )
            .await
            .kind,
            "completed"
        );
    }
    let record = e.recording().await;
    record.validate().unwrap();
    assert!(
        record
            .scenario
            .operations
            .iter()
            .all(|o| o.after.len() <= 3)
    );
    e.shutdown().await;
    let replay = Engine::new(record.scenario, binary()).await.unwrap();
    replay.replay().await.unwrap();
    replay.shutdown().await;
    let mut op = operation("invalid", "A", "SELECT 1");
    op.expect = json!({"typo_assertion":42});
    assert!(validate_operation(&op).is_err());
}

#[test]
fn large_cli_recording_can_be_imported_and_replayed_within_byte_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let mut scenario = blank();
    scenario.settings.connections = 2;
    scenario.settings.row_limit = 200;
    let columns = (0..100)
        .map(|i| format!("1 AS c{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200) SELECT {columns} FROM n"
    );
    for i in 0..8 {
        let mut op = operation(&format!("read_{i}"), "A", &sql);
        if i > 0 {
            op.after.push(Dependency {
                operation: format!("read_{}", i - 1),
                event: "completed".into(),
            });
        }
        scenario.operations.push(op);
    }
    let source = tmp.path().join("large.scenario.json");
    let recording = tmp.path().join("large.recording.json");
    let rerun = tmp.path().join("rerun.recording.json");
    std::fs::write(&source, serde_json::to_vec(&scenario).unwrap()).unwrap();
    for (input, output) in [(&source, &recording), (&recording, &rerun)] {
        let result = std::process::Command::new(binary())
            .arg("run")
            .arg(input)
            .arg("--output")
            .arg(output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = std::fs::read(output).unwrap();
        assert!(bytes.len() <= MAX_ARTIFACT);
        let run: Recording = serde_json::from_slice(&bytes).unwrap();
        run.validate().unwrap();
        let completed: Vec<_> = run
            .events
            .iter()
            .filter(|e| e.operation.starts_with("read_") && e.kind == "completed")
            .collect();
        assert_eq!(completed.len(), 8);
        for event in completed {
            assert_eq!(event.data["result"]["row_count"], "200");
            assert_eq!(
                event.data["result"]["rows"][0][0],
                json!({"type":"integer","value":"1"})
            );
        }
    }
}

#[tokio::test]
async fn invalid_pragma_parameters_do_not_change_settings() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    exec(&e, "enable_fk", "A", "PRAGMA foreign_keys=ON").await;
    for (i, (sql, read)) in [
        ("PRAGMA foreign_keys=OFF", "PRAGMA foreign_keys"),
        (
            "/* comment */ PRAGMA main.\"foreign_keys\"(OFF)",
            "PRAGMA foreign_keys",
        ),
        ("PRAGMA read_uncommitted=ON", "PRAGMA read_uncommitted"),
        ("PRAGMA defer_foreign_keys=ON", "PRAGMA defer_foreign_keys"),
    ]
    .iter()
    .enumerate()
    {
        let before =
            exec(&e, &format!("before_{i}"), "A", read).await.data["result"]["rows"].clone();
        let mut op = operation(&format!("invalid_{i}"), "A", sql);
        op.params = vec![Cell::Integer("1".into())];
        e.submit(op).await.unwrap();
        let event = e.wait_terminal(&format!("invalid_{i}")).await.unwrap();
        assert_eq!(event.kind, "failed");
        assert_eq!(event.data["error"]["category"], "validation");
        assert_eq!(
            exec(&e, &format!("after_{i}"), "A", read).await.data["result"]["rows"],
            before
        );
    }
    let mut table = operation(
        "table_pragma",
        "A",
        "SELECT name FROM pragma_table_info(?) ORDER BY cid",
    );
    table.params = vec![Cell::Text("t".into())];
    e.submit(table).await.unwrap();
    assert_eq!(
        e.wait_terminal("table_pragma").await.unwrap().data["result"]["rows"],
        json!([[{"type":"text","value":"id"}],[{"type":"text","value":"v"}]])
    );
    assert_eq!(
        exec(&e, "valid_pragma", "A", "PRAGMA foreign_keys=OFF")
            .await
            .kind,
        "completed"
    );
    assert_eq!(
        exec(&e, "fk_disabled", "A", "PRAGMA foreign_keys")
            .await
            .data["result"]["rows"][0][0]["value"],
        "0"
    );
    e.shutdown().await;
}

#[tokio::test]
async fn checkpoint_expectations_reject_missing_invalid_and_unavailable_counts() {
    for (i, (sql, expected)) in [
        ("SELECT 1", "full_checkpoint"),
        ("SELECT 1", "partial_checkpoint"),
        (
            "SELECT 0 AS busy, 1 AS log, 1 AS checkpointed WHERE 0",
            "full_checkpoint",
        ),
        (
            "SELECT '0' AS busy, 1 AS log, 1 AS checkpointed",
            "full_checkpoint",
        ),
        (
            "SELECT 0 AS busy, -1 AS log, -1 AS checkpointed",
            "full_checkpoint",
        ),
        (
            "SELECT 0 AS busy, 1 AS log, 2 AS checkpointed",
            "full_checkpoint",
        ),
        (
            "SELECT 1 AS busy, 1 AS log, 1 AS checkpointed",
            "full_checkpoint",
        ),
    ]
    .iter()
    .enumerate()
    {
        let mut scenario = blank();
        let mut op = operation(&format!("invalid_{i}"), "A", sql);
        op.expect = json!({*expected:true});
        scenario.operations.push(op);
        let e = Engine::new(scenario, binary()).await.unwrap();
        let result = e.replay().await;
        e.shutdown().await;
        assert!(
            result.is_err(),
            "invalid checkpoint assertion passed: {sql}"
        );
    }
    for expected in ["full_checkpoint", "partial_checkpoint"] {
        let mut scenario = blank();
        scenario.settings.journal_mode = "delete".into();
        let mut op = operation("not_wal", "A", "PRAGMA wal_checkpoint(PASSIVE)");
        op.expect = json!({expected:true});
        scenario.operations.push(op);
        let e = Engine::new(scenario, binary()).await.unwrap();
        let result = e.replay().await;
        e.shutdown().await;
        assert!(
            result.is_err(),
            "-1 counters must mean unavailable, not complete"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn full_worker_pipe_does_not_block_deadline_cancel_or_stop() {
    for mode in ["deadline", "cancel", "stop"] {
        let mut scenario = blank();
        // This test exercises worker I/O, not database initialization.
        scenario.initial.clear();
        scenario.settings.deadline_ms = if mode == "deadline" { 100 } else { 5000 };
        let e = Engine::new(scenario, binary()).await.unwrap();
        let dir = e.directory.path().to_path_buf();
        let snapshot = e.snapshot().await;
        let pid = snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ev| ev["connection"] == "A" && ev["kind"] == "ready")
            .unwrap()["data"]["pid"]
            .as_u64()
            .unwrap();
        assert!(
            std::process::Command::new("kill")
                .args(["-STOP", &pid.to_string()])
                .status()
                .unwrap()
                .success()
        );
        let mut op = operation("full_pipe", "A", "SELECT ?,?,?,?,?");
        op.params = vec![Cell::Text("x".repeat(65536)); 5];
        let submitting = e.clone();
        let mut submit = tokio::spawn(async move { submitting.submit(op).await });
        e.wait_event("full_pipe", None, "created", 2000)
            .await
            .unwrap();
        assert_eq!(
            exec(&e, "other_worker", "B", "SELECT 1").await.kind,
            "completed"
        );
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            match mode {
                "cancel" => e.cancel("A", "cancelled").await,
                "stop" => e.stop().await,
                _ => {}
            }
            e.wait_terminal("full_pipe").await
        })
        .await;
        // Always reap the paused child before reporting a failed assertion.
        e.force_kill("A").await;
        e.shutdown().await;
        let submitted = tokio::time::timeout(std::time::Duration::from_secs(2), &mut submit).await;
        if submitted.is_err() {
            submit.abort();
        }
        let event = outcome
            .expect("deadline/cancel/stop blocked behind stdin")
            .unwrap();
        assert_eq!(
            event.kind,
            if mode == "deadline" {
                "timed_out"
            } else {
                "cancelled"
            }
        );
        assert_eq!(event.data["state_lost"], true);
        assert!(
            submitted.is_ok(),
            "execute sender did not unblock after worker exit"
        );
        assert!(!dir.exists());
    }
}

#[tokio::test]
async fn incremental_snapshots_preserve_history_and_reset_cursors() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    let first = e.snapshot().await;
    let id = first["experiment"].as_str().unwrap();
    let count = first["events"].as_array().unwrap().len() as u64;
    let idle = e.snapshot_since(Some(id), count).await;
    assert_eq!(idle["events"], json!([]));
    exec(&e, "read", "A", "SELECT 42").await;
    let delta = e.snapshot_since(Some(id), count).await;
    let full = e.snapshot().await;
    assert_eq!(delta["event_offset"], count);
    assert_eq!(
        delta["events"],
        json!(&full["events"].as_array().unwrap()[count as usize..])
    );
    assert_eq!(delta["active"], full["active"]);
    assert_eq!(
        e.snapshot_since(Some(id), u64::MAX).await["event_offset"],
        0
    );
    e.shutdown().await;
    let fresh = Engine::new(blank(), binary()).await.unwrap();
    let reset = fresh.snapshot_since(Some(id), count).await;
    assert_ne!(reset["experiment"], first["experiment"]);
    assert_eq!(reset["event_offset"], 0);
    assert!(!reset["events"].as_array().unwrap().is_empty());
    fresh.shutdown().await;
}

fn lifecycle(id: &str, c: &str, kind: OperationKind) -> Operation {
    Operation {
        kind,
        ..operation(id, c, "")
    }
}
async fn life(e: &Arc<Engine>, id: &str, c: &str, kind: OperationKind) -> Event {
    e.submit(lifecycle(id, c, kind)).await.unwrap();
    e.wait_terminal(id).await.unwrap()
}
fn first(ev: &Event) -> &str {
    ev.data["result"]["rows"][0][0]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("no first cell in {}", ev.data))
}
fn ready_pids(snapshot: &Value) -> Vec<u64> {
    snapshot["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "ready")
        .map(|e| e["data"]["pid"].as_u64().unwrap())
        .collect()
}
#[cfg(unix)]
fn alive(pid: u64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[tokio::test]
async fn crash_reopen_scenarios_hold_in_wal_and_rollback_journal_modes() {
    for mode in ["wal", "delete"] {
        for mut scenario in scenarios::builtins().split_off(12) {
            scenario.settings.journal_mode = mode.into();
            let label = format!("{} ({mode})", scenario.name);
            let e = Engine::new(scenario.clone(), binary()).await.unwrap();
            e.replay()
                .await
                .unwrap_or_else(|err| panic!("{label}: {err}"));
            let record = e.recording().await;
            record.validate().unwrap();
            assert_eq!(record.format_version, 2);
            let event = |id: &str, kind: &str| {
                record
                    .events
                    .iter()
                    .find(|e| e.operation == id && e.kind == kind)
                    .unwrap_or_else(|| panic!("{label}: no {id}/{kind}"))
            };
            let crash = &event("a_crash", "completed").data;
            assert_eq!(crash["lifecycle"], "crash");
            assert_eq!(crash["generation"], 1);
            assert_eq!(crash["termination"]["method"], KILL_METHOD);
            assert_eq!(crash["termination"]["exit_confirmed"], true);
            assert_eq!(crash["termination"]["reaped"], true);
            let reopen = &event("a_reopen", "completed").data;
            assert_eq!(reopen["lifecycle"], "reopen");
            assert_eq!(reopen["generation"], 2);
            assert_eq!(reopen["transaction"], "none");
            assert_eq!(reopen["autocommit"], true);
            assert_ne!(reopen["pid"], crash["pid"]);
            // The process exit and the new start are ordered, confirmed observations.
            assert!(event("a_crash", "completed").seq < event("a_reopen", "created").seq);
            // A planned crash is not an infrastructure failure.
            assert!(record.events.iter().all(|e| e.kind != "worker_lost"));
            assert_eq!(e.snapshot().await["lost"], json!([]));
            // Initialization ran once, before the crash.
            assert_eq!(
                record
                    .events
                    .iter()
                    .filter(|e| e.operation.starts_with("seed_") && e.kind == "created")
                    .count(),
                scenario.initial.len()
            );
            if let Some(journal) = record
                .events
                .iter()
                .find(|e| e.operation == "a_journal_mode" && e.kind == "completed")
            {
                assert_eq!(first(journal), mode);
            }
            e.shutdown().await;
            // The exported run replays its crash and reopen at the same logical points.
            let decoded: Recording =
                serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
            let lifecycle: Vec<_> = decoded
                .scenario
                .operations
                .iter()
                .filter(|o| !o.kind.is_sql())
                .map(|o| (o.id.as_str(), o.kind))
                .collect();
            assert_eq!(
                lifecycle,
                [
                    ("a_crash", OperationKind::Crash),
                    ("a_reopen", OperationKind::Reopen)
                ]
            );
            let replay = Engine::new(decoded.scenario, binary()).await.unwrap();
            replay
                .replay()
                .await
                .unwrap_or_else(|err| panic!("{label} replay: {err}"));
            replay.shutdown().await;
        }
    }
}

#[tokio::test]
async fn crash_loses_only_session_state_and_other_connections_continue() {
    for mode in ["wal", "delete"] {
        let mut s = blank();
        s.settings.journal_mode = mode.into();
        let seeds = s.initial.len();
        let e = Engine::new(s, binary()).await.unwrap();
        let dir = e.directory.path().to_path_buf();
        // Committed before the crash; it also proves the file is not recreated.
        exec(&e, "committed", "A", "UPDATE t SET v=7").await;
        exec(&e, "fk_off", "A", "PRAGMA foreign_keys=OFF").await;
        exec(&e, "temp", "A", "CREATE TEMP TABLE scratch(x)").await;
        // B holds a read transaction across A's crash.
        exec(&e, "b_begin", "B", "BEGIN").await;
        assert_eq!(
            first(&exec(&e, "b_read", "B", "SELECT v FROM t").await),
            "7"
        );
        exec(&e, "a_begin", "A", "BEGIN IMMEDIATE").await;
        let open = exec(&e, "a_insert", "A", "INSERT INTO t VALUES(2,2)").await;
        assert_eq!(open.data["transaction"], "write");

        let crash = life(&e, "crash", "A", OperationKind::Crash).await;
        assert_eq!(crash.kind, "completed", "{mode}: {}", crash.data);
        assert_eq!(crash.data["session_state"]["lost"], true);
        assert_eq!(
            crash.data["session_state"]["last_sampled"],
            json!({"transaction":"write","autocommit":false})
        );
        let snapshot = e.snapshot().await;
        assert_eq!(
            snapshot["connections"]["A"],
            json!({"phase":"closed","generation":1})
        );
        assert_eq!(snapshot["connections"]["B"]["phase"], "open");
        assert_eq!(snapshot["lost"], json!([]));
        assert_eq!(snapshot["active"], json!({}));
        assert_eq!(snapshot["stopped"], false);

        // Unambiguous transitions for a closed connection.
        let sql = e.submit(operation("closed_sql", "A", "SELECT 1")).await;
        assert!(
            sql.unwrap_err()
                .contains("closed after a planned process crash")
        );
        let again = e
            .submit(lifecycle("crash_twice", "A", OperationKind::Crash))
            .await;
        assert!(again.unwrap_err().contains("already closed"));

        // B's transaction survives; C can write while A is closed.
        let still = exec(&e, "b_still", "B", "SELECT count(*) FROM t").await;
        assert_eq!(first(&still), "1");
        assert_eq!(still.data["transaction"], "read");
        assert_eq!(exec(&e, "b_commit", "B", "COMMIT").await.kind, "completed");
        assert_eq!(
            exec(&e, "c_write", "C", "UPDATE t SET v=8").await.kind,
            "completed",
            "{mode}"
        );

        let reopen = life(&e, "reopen", "A", OperationKind::Reopen).await;
        assert_eq!(reopen.kind, "completed", "{mode}: {}", reopen.data);
        assert_eq!(reopen.data["generation"], 2);
        assert_eq!(e.snapshot().await["connections"]["A"]["phase"], "open");
        let twice = e
            .submit(lifecycle("reopen_twice", "A", OperationKind::Reopen))
            .await;
        assert!(twice.unwrap_err().contains("already open"));

        // Data: the uncommitted insert is gone, both commits remain.
        assert_eq!(
            exec(&e, "a_rows", "A", "SELECT id, v FROM t").await.data["result"]["rows"],
            json!([[{"type":"integer","value":"1"},{"type":"integer","value":"8"}]]),
            "{mode}"
        );
        // Session state: no transaction, default PRAGMA value, no temporary table.
        let commit = exec(&e, "a_commit", "A", "COMMIT").await;
        assert_eq!(commit.kind, "failed");
        assert_eq!(commit.data["error"]["extended"], 1);
        assert_eq!(
            first(&exec(&e, "fk", "A", "PRAGMA foreign_keys").await),
            "1"
        );
        assert_eq!(
            exec(&e, "no_temp", "A", "SELECT * FROM scratch").await.kind,
            "failed"
        );
        // The worker startup contract is applied to the new process.
        assert_eq!(
            first(&exec(&e, "autockpt", "A", "PRAGMA wal_autocheckpoint").await),
            "0"
        );
        assert_eq!(
            first(&exec(&e, "mode", "A", "PRAGMA journal_mode").await),
            mode
        );
        assert_eq!(
            exec(&e, "deny", "A", "PRAGMA journal_mode=OFF").await.kind,
            "failed"
        );
        assert_eq!(
            exec(&e, "a_write", "A", "INSERT INTO t VALUES(3,3)")
                .await
                .kind,
            "completed"
        );
        assert_eq!(
            first(&exec(&e, "b_count", "B", "SELECT count(*) FROM t").await),
            "2"
        );
        let snapshot = e.snapshot().await;
        assert_eq!(
            snapshot["operations"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|o| o["id"].as_str().unwrap().starts_with("seed_"))
                .count(),
            seeds
        );
        let record = e.recording().await;
        record.validate().unwrap();
        assert_eq!(record.format_version, 2);
        e.stop().await;
        assert!(!dir.exists());
    }
}

#[tokio::test]
async fn crash_needs_a_statement_boundary_and_unplanned_loss_still_needs_reset() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    e.submit(operation("long", "A", LONG)).await.unwrap();
    e.wait_event("long", None, "running", 2000).await.unwrap();
    let refused = e
        .submit(lifecycle("mid_statement", "A", OperationKind::Crash))
        .await
        .unwrap_err();
    assert!(refused.contains("running statement"), "{refused}");
    let snapshot = e.snapshot().await;
    assert_eq!(snapshot["active"]["A"], "long");
    assert_eq!(snapshot["connections"]["A"]["phase"], "open");
    assert!(
        snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|ev| ev["operation"] != "mid_statement")
    );
    e.cancel("A", "cancelled").await;
    assert_eq!(e.wait_terminal("long").await.unwrap().kind, "cancelled");
    // The refused crash left the worker and its generation untouched.
    assert_eq!(exec(&e, "alive", "A", "SELECT 1").await.kind, "completed");
    assert_eq!(e.snapshot().await["connections"]["A"]["generation"], 1);

    e.submit(operation("killed", "B", LONG)).await.unwrap();
    e.wait_event("killed", None, "running", 2000).await.unwrap();
    e.force_kill("B").await;
    assert_eq!(
        e.wait_terminal("killed").await.unwrap().data["state_lost"],
        true
    );
    for kind in [OperationKind::Reopen, OperationKind::Crash] {
        let err = e
            .submit(lifecycle(&format!("lost_{}", kind.as_str()), "B", kind))
            .await
            .unwrap_err();
        assert!(err.contains("reset required"), "{err}");
    }
    assert_eq!(e.snapshot().await["lost"], json!(["B"]));
    e.shutdown().await;
}

#[test]
fn lifecycle_steps_are_validated_statically() {
    let after = |id: &str, event: &str| {
        vec![Dependency {
            operation: id.into(),
            event: event.into(),
        }]
    };
    let step = |id: &str, kind, dep: &str| {
        let mut op = lifecycle(id, "A", kind);
        if !dep.is_empty() {
            op.after = after(dep, "completed");
        }
        op
    };
    let sql = |id: &str, dep: &str| {
        let mut op = operation(id, "A", "SELECT 1");
        if !dep.is_empty() {
            op.after = after(dep, "completed");
        }
        op
    };
    let check = |version: u32, operations: Vec<Operation>| {
        let mut s = blank();
        s.format_version = version;
        s.operations = operations;
        s.validate()
    };
    use OperationKind::{Crash, Reopen};
    assert!(
        check(
            2,
            vec![
                sql("q", ""),
                step("c", Crash, "q"),
                step("r", Reopen, "c"),
                sql("q2", "r")
            ]
        )
        .is_ok()
    );
    // A crash may open a schedule: initialization is confirmed before any step.
    assert!(check(2, vec![step("c", Crash, ""), step("r", Reopen, "c")]).is_ok());
    for (expected, version, operations) in [
        ("format_version 2", 1, vec![step("c", Crash, "")]),
        (
            "must wait for a terminal event of q",
            2,
            vec![sql("q", ""), step("c", Crash, "")],
        ),
        (
            "must wait for a terminal event of c",
            2,
            vec![step("c", Crash, ""), step("r", Reopen, "")],
        ),
        (
            "while it is closed",
            2,
            vec![step("c", Crash, ""), sql("q", "c")],
        ),
        (
            "already closed",
            2,
            vec![step("c", Crash, ""), step("c2", Crash, "c")],
        ),
        ("which is open", 2, vec![step("r", Reopen, "")]),
        (
            "must wait for a terminal event of r",
            2,
            vec![step("c", Crash, ""), step("r", Reopen, "c"), sql("q", "")],
        ),
    ] {
        let err = check(version, operations).unwrap_err();
        assert!(err.contains(expected), "{expected}: {err}");
    }
    // A failed reopen leaves the connection closed, so a recorded retry stays valid.
    let mut retry = step("r2", Reopen, "");
    retry.after = after("r", "timed_out");
    assert!(check(2, vec![step("c", Crash, ""), step("r", Reopen, "c"), retry]).is_ok());
    let mut with_sql = lifecycle("c", "A", Crash);
    with_sql.sql = "SELECT 1".into();
    assert!(validate_operation(&with_sql).is_err());
    let mut with_expectation = lifecycle("c", "A", Crash);
    with_expectation.expect = json!({"rows":[]});
    assert!(validate_operation(&with_expectation).is_err());
    assert!(
        serde_json::from_value::<Operation>(json!({"id":"x","connection":"A","kind":"restart"}))
            .is_err()
    );
    // Version 1 artifacts keep their shape: SQL steps do not gain a kind field.
    let encoded = serde_json::to_value(operation("q", "A", "SELECT 1")).unwrap();
    assert!(encoded.get("kind").is_none());
    assert_eq!(
        serde_json::to_value(lifecycle("c", "A", Crash)).unwrap()["kind"],
        "crash"
    );
}

#[tokio::test]
async fn stale_generation_messages_never_reach_the_reopened_session() {
    let e = Engine::new(blank(), binary()).await.unwrap();
    let count = |snapshot: &Value, kind: &str| {
        snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|ev| ev["connection"] == "A" && ev["kind"] == kind)
            .count()
    };
    let busy = json!({"kind":"busy","operation":"held","data":{"calls":1}});
    life(&e, "crash", "A", OperationKind::Crash).await;
    let closed = e.snapshot().await["events"].as_array().unwrap().len();
    // Buffered output of the reaped process arrives while the connection is closed.
    e.observe_from("A", 1, busy.clone()).await;
    assert_eq!(
        e.snapshot().await["events"].as_array().unwrap().len(),
        closed
    );
    life(&e, "reopen", "A", OperationKind::Reopen).await;
    e.submit(operation("held", "A", LONG)).await.unwrap();
    e.wait_event("held", None, "running", 2000).await.unwrap();
    // The same connection ID and operation ID, but the previous process session.
    e.observe_from("A", 1, busy.clone()).await;
    e.observe_from(
        "A",
        1,
        json!({"kind":"completed","operation":"held","data":{"transaction":"none","autocommit":true}}),
    )
    .await;
    e.observe_from(
        "A",
        1,
        json!({"kind":"ready","operation":"","data":{"pid":1}}),
    )
    .await;
    let snapshot = e.snapshot().await;
    assert_eq!(snapshot["active"]["A"], "held");
    assert_eq!(count(&snapshot, "busy"), 0);
    assert!(
        snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|ev| ev["operation"] != "held" || ev["kind"] != "completed")
    );
    assert_eq!(count(&snapshot, "ready"), 2);
    // Control: the current generation is accepted through the same entry point.
    e.observe_from("A", 2, busy).await;
    assert_eq!(count(&e.snapshot().await, "busy"), 1);
    // Workers can never report on a lifecycle operation, whatever their generation.
    e.observe_from(
        "A",
        2,
        json!({"kind":"failed","operation":"reopen","data":{}}),
    )
    .await;
    assert_eq!(count(&e.snapshot().await, "failed"), 0);
    e.cancel("A", "cancelled").await;
    assert_eq!(e.wait_terminal("held").await.unwrap().kind, "cancelled");
    e.shutdown().await;
}

#[tokio::test]
async fn failed_reopen_is_bounded_and_leaves_a_retryable_closed_connection() {
    let tmp = tempfile::tempdir().unwrap();
    // Written before any use so no concurrently forked child still holds it open.
    #[cfg(unix)]
    let silent = {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp.path().join("silent-worker");
        std::fs::write(&path, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let e = Engine::new(blank(), binary()).await.unwrap();
    let dir = e.directory.path().to_path_buf();
    exec(&e, "committed", "A", "UPDATE t SET v=5").await;
    life(&e, "crash", "A", OperationKind::Crash).await;
    let closed = |snapshot: Value| {
        assert_eq!(snapshot["connections"]["A"]["phase"], "closed");
        assert_eq!(snapshot["lost"], json!([]));
        assert_eq!(snapshot["active"], json!({}));
    };

    // The worker cannot be started at all.
    e.set_worker_executable(tmp.path().join("missing-worker"));
    let started = std::time::Instant::now();
    let missing = life(&e, "missing", "A", OperationKind::Reopen).await;
    assert!(started.elapsed().as_secs() < 2);
    assert_eq!(missing.kind, "failed");
    assert_eq!(missing.data["error"]["category"], "infrastructure");
    assert_eq!(missing.data["connection"], "closed");
    closed(e.snapshot().await);
    assert!(
        e.submit(operation("still_closed", "A", "SELECT 1"))
            .await
            .is_err()
    );

    #[cfg(unix)]
    {
        // The process starts but never reports ready: bounded by the ready deadline, then reaped.
        e.set_worker_executable(silent.clone());
        let started = std::time::Instant::now();
        e.submit(lifecycle("silent", "A", OperationKind::Reopen))
            .await
            .unwrap();
        let pid = e
            .wait_event("silent", None, "running", 2000)
            .await
            .unwrap()
            .data["pid"]
            .as_u64()
            .unwrap();
        let silent_end = e.wait_terminal("silent").await.unwrap();
        let elapsed = started.elapsed().as_millis() as u64;
        assert_eq!(silent_end.kind, "timed_out");
        assert!(
            (READY_DEADLINE_MS..READY_DEADLINE_MS + 3000).contains(&elapsed),
            "{elapsed}"
        );
        assert!(!alive(pid), "a failed reopen must reap its process");
        closed(e.snapshot().await);

        // Cancellation ends a reopen in flight without waiting for the deadline.
        let started = std::time::Instant::now();
        e.submit(lifecycle("cancelled", "A", OperationKind::Reopen))
            .await
            .unwrap();
        let pid = e
            .wait_event("cancelled", None, "running", 2000)
            .await
            .unwrap()
            .data["pid"]
            .as_u64()
            .unwrap();
        e.cancel("A", "cancelled").await;
        assert_eq!(
            e.wait_terminal("cancelled").await.unwrap().kind,
            "cancelled"
        );
        assert!(started.elapsed().as_secs() < 3);
        assert!(!alive(pid));
        closed(e.snapshot().await);
    }

    // The closed state is still retryable, and the data is what was committed.
    e.set_worker_executable(binary());
    let reopened = life(&e, "retry", "A", OperationKind::Reopen).await;
    assert_eq!(reopened.kind, "completed", "{}", reopened.data);
    assert_eq!(reopened.data["generation"], if cfg!(unix) { 5 } else { 3 });
    assert_eq!(first(&exec(&e, "read", "A", "SELECT v FROM t").await), "5");
    // Failed attempts are part of the run; the exported artifact remains valid.
    e.recording().await.validate().unwrap();

    #[cfg(unix)]
    {
        // Stop during a partially completed reopen: bounded, terminal, nothing left behind.
        life(&e, "crash_again", "A", OperationKind::Crash).await;
        e.set_worker_executable(silent);
        e.submit(lifecycle("stopped", "A", OperationKind::Reopen))
            .await
            .unwrap();
        e.wait_event("stopped", None, "running", 2000)
            .await
            .unwrap();
        let pids = ready_pids(&e.snapshot().await);
        tokio::time::timeout(std::time::Duration::from_secs(3), e.stop())
            .await
            .expect("stop blocked behind a reopen in flight");
        assert_eq!(e.wait_terminal("stopped").await.unwrap().kind, "cancelled");
        assert!(pids.into_iter().all(|pid| !alive(pid)));
    }
    e.stop().await;
    assert!(!dir.exists());
}

#[tokio::test]
async fn stop_and_reset_after_crash_and_reopen_reap_every_generation() {
    // Stop while a connection is closed.
    let e = Engine::new(blank(), binary()).await.unwrap();
    let dir = e.directory.path().to_path_buf();
    exec(&e, "begin", "A", "BEGIN IMMEDIATE").await;
    life(&e, "crash", "A", OperationKind::Crash).await;
    let pids = ready_pids(&e.snapshot().await);
    tokio::time::timeout(std::time::Duration::from_secs(5), e.stop())
        .await
        .unwrap();
    assert!(!dir.exists());
    assert!(
        e.submit(lifecycle("after_stop", "A", OperationKind::Reopen))
            .await
            .is_err()
    );
    #[cfg(unix)]
    assert!(pids.iter().all(|pid| !alive(*pid)));
    assert_eq!(pids.len(), 3);

    // Reset is stop plus a fresh experiment: nothing of the crashed one carries over.
    let fresh = Engine::new(blank(), binary()).await.unwrap();
    let snapshot = fresh.snapshot().await;
    for c in ["A", "B", "C"] {
        assert_eq!(
            snapshot["connections"][c],
            json!({"phase":"open","generation":1})
        );
    }
    // Stop with a reopened connection in the middle of a statement and a transaction.
    let dir = fresh.directory.path().to_path_buf();
    life(&fresh, "crash", "A", OperationKind::Crash).await;
    life(&fresh, "reopen", "A", OperationKind::Reopen).await;
    exec(&fresh, "begin", "A", "BEGIN IMMEDIATE").await;
    fresh.submit(operation("long", "A", LONG)).await.unwrap();
    fresh
        .wait_event("long", None, "running", 2000)
        .await
        .unwrap();
    let pids = ready_pids(&fresh.snapshot().await);
    assert_eq!(pids.len(), 4, "three initial workers and one reopened");
    tokio::time::timeout(std::time::Duration::from_secs(5), fresh.stop())
        .await
        .unwrap();
    assert_eq!(fresh.wait_terminal("long").await.unwrap().kind, "cancelled");
    assert!(!dir.exists());
    #[cfg(unix)]
    assert!(pids.iter().all(|pid| !alive(*pid)));
}

#[test]
fn cli_runs_crash_scenarios_and_keeps_version_1_artifacts_readable() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir().unwrap();
    let run = |input: &std::path::Path, name: &str| {
        let output = tmp.path().join(name);
        let result = std::process::Command::new(binary())
            .arg("run")
            .arg(input)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}: {}",
            input.display(),
            String::from_utf8_lossy(&result.stderr)
        );
        let recording: Recording =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        recording.validate().unwrap();
        (output, recording)
    };
    for number in 13..=15 {
        let scenario = root.join(format!("examples/{number}.scenario.json"));
        let (path, first_run) = run(&scenario, "run.recording.json");
        // Replaying the recording crashes and reopens again in a new disposable experiment.
        let (_, rerun) = run(&path, "rerun.recording.json");
        for recording in [&first_run, &rerun] {
            assert_eq!(recording.format_version, 2);
            let confirmed: Vec<_> = recording
                .events
                .iter()
                .filter(|e| e.kind == "completed" && e.data.get("lifecycle").is_some())
                .map(|e| (e.operation.as_str(), e.data["generation"].as_u64().unwrap()))
                .collect();
            assert_eq!(confirmed, [("a_crash", 1), ("a_reopen", 2)]);
        }
        // Intent is portable; observed process identity is not.
        assert_eq!(
            serde_json::to_value(&first_run.scenario.operations).unwrap(),
            serde_json::to_value(&rerun.scenario.operations).unwrap()
        );
    }
    // A recording made before lifecycle steps existed imports, replays and stays version 1.
    let old = root.join("examples/recordings/01.recording.json");
    let stored: Value = serde_json::from_slice(&std::fs::read(&old).unwrap()).unwrap();
    assert_eq!(stored["format_version"], 1);
    assert!(stored["events"][0]["data"].get("generation").is_none());
    let (_, replayed) = run(&old, "old.recording.json");
    assert_eq!(replayed.format_version, 1);
    assert_eq!(replayed.scenario.format_version, 1);
}
