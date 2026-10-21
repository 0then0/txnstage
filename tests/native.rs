use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use txnstage::{engine::Engine, model::*, scenarios};
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
    s.settings.deadline_ms = 150;
    s.settings.busy_ms = 10000;
    let e = Engine::new(s, binary()).await.unwrap();
    let dir = e.directory.path().to_path_buf();
    e.submit(operation("deadline", "A", LONG)).await.unwrap();
    assert_eq!(e.wait_terminal("deadline").await.unwrap().kind, "timed_out");
    exec(&e, "hold", "A", "BEGIN IMMEDIATE").await;
    e.submit(operation("busy_deadline", "B", "UPDATE t SET v=3"))
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
    e.crash("C").await;
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
        e.crash("A").await;
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
