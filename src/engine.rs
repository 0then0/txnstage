use crate::model::*;
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{Mutex, Notify, mpsc},
};

pub fn terminal(kind: &str) -> bool {
    ["completed", "failed", "cancelled", "timed_out"].contains(&kind)
}
struct Worker {
    input: Mutex<ChildStdin>,
    kill: mpsc::Sender<()>,
    done: Notify,
    exited: std::sync::atomic::AtomicBool,
}
#[derive(Default)]
struct State {
    events: Vec<Event>,
    active: HashMap<String, String>,
    lost: Vec<String>,
    stopped: bool,
    operations: Vec<Operation>,
    reasons: HashMap<String, String>,
    event_bytes: usize,
    operation_bytes: usize,
}
pub struct Engine {
    pub scenario: Scenario,
    pub environment: Value,
    state: Mutex<State>,
    workers: HashMap<String, Arc<Worker>>,
    pub changed: Notify,
    started: Instant,
    pub directory: TempDir,
}
impl Engine {
    pub async fn new(scenario: Scenario, executable: PathBuf) -> Result<Arc<Self>, String> {
        scenario.validate()?;
        let directory = tempfile::Builder::new()
            .prefix("txnstage-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let db = directory.path().join("experiment.db");
        {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute_batch(&format!(
                "PRAGMA journal_mode={};",
                scenario.settings.journal_mode
            ))
            .map_err(|e| e.to_string())?;
        }
        let mut workers = HashMap::new();
        let mut readers = Vec::new();
        for name in ["A", "B", "C"]
            .into_iter()
            .take(scenario.settings.connections)
        {
            let spawn = Command::new(&executable)
                .arg("--worker")
                .arg(&db)
                .arg(serde_json::to_string(&scenario.settings).unwrap())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn();
            let mut child = spawn.map_err(|e| e.to_string())?;
            let input = child.stdin.take().unwrap();
            let output = child.stdout.take().unwrap();
            let (kill, mut killed) = mpsc::channel(1);
            let worker = Arc::new(Worker {
                input: Mutex::new(input),
                kill,
                done: Notify::new(),
                exited: std::sync::atomic::AtomicBool::new(false),
            });
            let w = Arc::downgrade(&worker);
            tokio::spawn(async move {
                tokio::select! {_=child.wait()=>{},_=killed.recv()=>{let _=child.kill().await;let _=child.wait().await;}}
                if let Some(w) = w.upgrade() {
                    w.exited.store(true, std::sync::atomic::Ordering::SeqCst);
                    w.done.notify_waiters();
                }
            });
            workers.insert(name.into(), worker);
            readers.push((name.to_string(), output));
        }
        let mut env = environment();
        env["settings"] = json!(scenario.settings);
        let engine = Arc::new(Self {
            scenario,
            environment: env,
            state: Mutex::new(State::default()),
            workers,
            changed: Notify::new(),
            started: Instant::now(),
            directory,
        });
        for (name, output) in readers {
            let e = Arc::downgrade(&engine);
            tokio::spawn(async move {
                let mut lines = BufReader::new(output).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(e) = e.upgrade() {
                        match serde_json::from_str::<Value>(&line) {
                            Ok(v) => e.observe(&name, v).await,
                            Err(_) => break,
                        }
                    } else {
                        return;
                    }
                }
                if let Some(e) = e.upgrade() {
                    e.lost(&name, "Worker exited; connection state lost").await;
                }
            });
        }
        for n in engine.workers.keys() {
            if let Err(e) = engine.wait_event("", Some(n), "ready", 5000).await {
                engine.shutdown().await;
                return Err(e);
            }
        }
        for (i, sql) in engine.scenario.initial.iter().enumerate() {
            let op = Operation {
                id: format!("seed_{i}"),
                connection: "A".into(),
                sql: sql.clone(),
                params: vec![],
                after: vec![],
                expect: Value::Null,
                note: "Explicit experiment initialization".into(),
            };
            if let Err(e) = engine.dispatch(op).await {
                engine.shutdown().await;
                return Err(e);
            }
            let ev = match engine.wait_terminal(&format!("seed_{i}")).await {
                Ok(v) => v,
                Err(e) => {
                    engine.shutdown().await;
                    return Err(e);
                }
            };
            if ev.kind != "completed" {
                engine.shutdown().await;
                return Err(format!("Initialization failed: {}", ev.data));
            }
        }
        Ok(engine)
    }
    fn append(
        &self,
        s: &mut State,
        connection: &str,
        operation: &str,
        kind: &str,
        mut data: Value,
    ) {
        if s.events.len() >= 20000 {
            return;
        }
        if s.event_bytes + serde_json::to_vec(&data).unwrap_or_default().len() > 5 * 1024 * 1024
            && data.get("result").is_some()
        {
            data["result"]["rows"] = json!([]);
            data["result"]["rows_truncated"] = json!(true);
            data["result"]["recording_limit"] =
                json!("5 MiB event budget: row previews omitted; statement was fully drained");
        }
        s.event_bytes += serde_json::to_vec(&data).unwrap_or_default().len() + 256;
        s.events.push(Event {
            seq: s.events.len() as u64 + 1,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            connection: connection.into(),
            operation: operation.into(),
            kind: kind.into(),
            data,
        });
        self.changed.notify_waiters();
    }
    async fn observe(&self, connection: &str, v: Value) {
        let operation = v["operation"].as_str().unwrap_or("");
        let kind = v["kind"].as_str().unwrap_or("infrastructure");
        let mut data = v["data"].clone();
        let mut s = self.state.lock().await;
        if terminal(kind) {
            if s.active.get(connection).map(String::as_str) != Some(operation) {
                return;
            }
            s.active.remove(connection);
            data["files_at_completion"] = self.files();
            if let Some(reason) = s.reasons.remove(operation) {
                data["worker_outcome"] = json!(kind);
                data["cancellation_reason"] = json!(reason);
                self.append(&mut s, connection, operation, &reason, data);
                return;
            }
        }
        self.append(&mut s, connection, operation, kind, data);
    }
    async fn lost(&self, connection: &str, message: &str) {
        let mut s = self.state.lock().await;
        if s.lost.contains(&connection.to_string()) {
            return;
        }
        s.lost.push(connection.into());
        if let Some(op) = s.active.remove(connection) {
            let kind = s.reasons.remove(&op).unwrap_or_else(|| "failed".into());
            self.append(
                &mut s,
                connection,
                &op,
                &kind,
                json!({"error":{"category":"infrastructure","message":message},"state_lost":true}),
            );
        }
        self.append(
            &mut s,
            connection,
            "",
            "worker_lost",
            json!({"message":message,"state_lost":true}),
        );
    }
    pub fn files(&self) -> Value {
        let size = |name: &str| {
            std::fs::metadata(self.directory.path().join(name))
                .map(|m| m.len())
                .unwrap_or(0)
        };
        json!({"db_bytes":size("experiment.db"),"wal_bytes":size("experiment.db-wal"),"measurement":"filesystem stat only; no SQL or checkpoint","elapsed_ms":self.started.elapsed().as_millis() as u64})
    }
    pub async fn snapshot(&self) -> Value {
        self.snapshot_since(None, 0).await
    }
    pub async fn snapshot_since(&self, experiment: Option<&str>, after: u64) -> Value {
        let s = self.state.lock().await;
        let id = self.directory.path().file_name().unwrap().to_string_lossy();
        let offset = if experiment == Some(id.as_ref()) && after <= s.events.len() as u64 {
            after as usize
        } else {
            0
        };
        json!({"experiment":id,"event_offset":offset,"scenario":self.scenario,"environment":self.environment,"events":&s.events[offset..],"active":s.active,"lost":s.lost,"stopped":s.stopped,"operations":s.operations,"files":self.files()})
    }
    pub async fn recording(&self) -> Recording {
        let s = self.state.lock().await;
        let mut scenario = self.scenario.clone();
        // Reproduce observed user scheduling, including overlap barriers, independently of a template.
        scenario.operations = s
            .operations
            .iter()
            .filter(|o| !o.id.starts_with("seed_"))
            .cloned()
            .collect();
        for op in &mut scenario.operations {
            if let Some(created) = s
                .events
                .iter()
                .position(|e| e.operation == op.id && e.kind == "created")
            {
                op.after.clear();
                for prior in &scenario_ids_before(&s.operations, &op.id) {
                    if let Some(ev) = s.events[..created].iter().rev().find(|e| {
                        e.operation == *prior
                            && (terminal(&e.kind) || e.kind == "busy" || e.kind == "running")
                    }) {
                        op.after.push(Dependency {
                            operation: prior.clone(),
                            event: ev.kind.clone(),
                        });
                    }
                }
            }
        }
        Recording {
            format_version: FORMAT,
            scenario,
            environment: self.environment.clone(),
            events: s.events.clone(),
        }
    }
    pub async fn submit(self: &Arc<Self>, op: Operation) -> Result<(), String> {
        if op.id.starts_with("seed_") {
            return Err("seed_ operation IDs are reserved for initialization".into());
        }
        self.dispatch(op).await
    }
    async fn dispatch(self: &Arc<Self>, op: Operation) -> Result<(), String> {
        validate_operation(&op)?;
        let w = self
            .workers
            .get(&op.connection)
            .ok_or("Unknown connection")?
            .clone();
        {
            let mut s = self.state.lock().await;
            if s.stopped || s.lost.contains(&op.connection) {
                return Err("Experiment stopped or connection state lost; reset required".into());
            }
            if s.active.contains_key(&op.connection) {
                return Err("Connection already has a running operation".into());
            }
            let op_bytes = serde_json::to_vec(&op).unwrap().len();
            if (!op.id.starts_with("seed_")
                && s.operations
                    .iter()
                    .filter(|o| !o.id.starts_with("seed_"))
                    .count()
                    >= 200)
                || s.operations.len() >= 250
                || s.events.len() > 19000
                || s.event_bytes > 5 * 1024 * 1024
                || s.operation_bytes + op_bytes > 512 * 1024
            {
                return Err("Experiment resource limit reached; export and reset".into());
            }
            if s.operations.iter().any(|o| o.id == op.id) {
                return Err("Operation ID already used; create a new operation".into());
            }
            s.active.insert(op.connection.clone(), op.id.clone());
            s.operation_bytes += op_bytes;
            s.operations.push(op.clone());
            self.append(
                &mut s,
                &op.connection,
                &op.id,
                "created",
                json!({"sql":op.sql,"params":op.params}),
            );
        }
        // The deadline includes IPC backpressure, not only SQLite execution.
        let start = Instant::now();
        let e = Arc::downgrade(self);
        let id = op.id.clone();
        let conn = op.connection.clone();
        let deadline = self.scenario.settings.deadline_ms;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                let Some(e) = e.upgrade() else {
                    return;
                };
                if !e.is_active(&conn, &id).await {
                    return;
                }
                let files = e.files();
                let size = files["db_bytes"].as_u64().unwrap_or(0)
                    + files["wal_bytes"].as_u64().unwrap_or(0);
                if size > 128 * 1024 * 1024 {
                    e.state.lock().await.stopped = true;
                    e.lost(
                        &conn,
                        "Experiment storage limit exceeded (128 MiB); state lost",
                    )
                    .await;
                    for w in e.workers.values() {
                        let _ = w.kill.send(()).await;
                    }
                    return;
                }
                if start.elapsed().as_millis() >= deadline as u128 {
                    e.cancel(&conn, "timed_out").await;
                    return;
                }
            }
        });
        if let Err(e) = send(&w, json!({"command":"execute","operation":op})).await {
            self.lost(&op.connection, &e).await;
            return Err(e);
        }
        Ok(())
    }
    async fn is_active(&self, conn: &str, id: &str) -> bool {
        self.state.lock().await.active.get(conn).map(String::as_str) == Some(id)
    }
    pub async fn cancel(self: &Arc<Self>, connection: &str, reason: &str) {
        let id = {
            let mut s = self.state.lock().await;
            let Some(id) = s.active.get(connection).cloned() else {
                return;
            };
            if s.reasons.contains_key(&id) {
                return;
            }
            s.reasons.insert(id.clone(), reason.into());
            self.append(
                &mut s,
                connection,
                &id,
                "cancel_requested",
                json!({"reason":reason}),
            );
            id
        };
        // Sending cancel can itself block behind a full pipe. The supervisor's
        // kill channel must remain reachable throughout that bounded grace period.
        let cooperative = async {
            if let Some(w) = self.workers.get(connection) {
                let _ = send(
                    w,
                    json!({"command":"cancel","operation":id,"reason":reason}),
                )
                .await;
            }
            loop {
                let changed = self.changed.notified();
                if !self.is_active(connection, &id).await {
                    return;
                }
                changed.await;
            }
        };
        let _ = tokio::time::timeout(std::time::Duration::from_millis(300), cooperative).await;
        if self.is_active(connection, &id).await {
            self.crash(connection).await;
        }
    }
    pub async fn crash(&self, connection: &str) {
        if let Some(w) = self.workers.get(connection) {
            let _ = w.kill.try_send(());
        }
        self.lost(
            connection,
            "Worker forcibly terminated; connection state lost",
        )
        .await;
    }
    pub async fn stop(self: &Arc<Self>) {
        self.state.lock().await.stopped = true;
        for conn in self.workers.keys() {
            self.cancel(conn, "cancelled").await;
        }
        self.shutdown().await;
    }
    pub async fn shutdown(&self) {
        self.state.lock().await.stopped = true;
        for w in self.workers.values() {
            let _ = w.kill.send(()).await;
        }
        for w in self.workers.values() {
            loop {
                let notified = w.done.notified();
                if w.exited.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let _ = tokio::time::timeout(std::time::Duration::from_secs(3), notified).await;
            }
        }
        // All owned workers have been reaped; remove only this engine's TempDir.
        let _ = std::fs::remove_dir_all(self.directory.path());
    }
    pub async fn wait_event(
        &self,
        id: &str,
        connection: Option<&str>,
        kind: &str,
        timeout: u64,
    ) -> Result<Event, String> {
        tokio::time::timeout(std::time::Duration::from_millis(timeout), async {
            loop {
                let changed = self.changed.notified();
                {
                    let s = self.state.lock().await;
                    if let Some(e) = s.events.iter().find(|e| {
                        e.operation == id
                            && e.kind == kind
                            && connection.is_none_or(|c| c == e.connection)
                    }) {
                        return Ok(e.clone());
                    }
                    if let Some(e) = s
                        .events
                        .iter()
                        .find(|e| e.operation == id && terminal(&e.kind) && e.kind != kind)
                    {
                        return Err(format!("Expected {kind}; observed {}: {}", e.kind, e.data));
                    }
                    if s.stopped {
                        return Err("Experiment stopped".into());
                    }
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| format!("Event barrier timed out: {id}/{kind}"))?
    }
    pub async fn wait_terminal(&self, id: &str) -> Result<Event, String> {
        tokio::time::timeout(
            std::time::Duration::from_millis(self.scenario.settings.deadline_ms + 5000),
            async {
                loop {
                    let changed = self.changed.notified();
                    if let Some(e) = self
                        .state
                        .lock()
                        .await
                        .events
                        .iter()
                        .find(|e| e.operation == id && terminal(&e.kind))
                    {
                        return e.clone();
                    }
                    changed.await;
                }
            },
        )
        .await
        .map_err(|_| "Terminal event timeout".into())
    }
    pub async fn replay(self: &Arc<Self>) -> Result<(), String> {
        for op in self.scenario.operations.clone() {
            for d in &op.after {
                self.wait_event(
                    &d.operation,
                    None,
                    &d.event,
                    self.scenario.settings.deadline_ms + 2000,
                )
                .await?;
            }
            self.submit(op).await?;
        }
        for op in &self.scenario.operations {
            let ev = self.wait_terminal(&op.id).await?;
            let expected = &op.expect;
            if let Some(code) = expected["extended"].as_i64() {
                if ev.data["error"]["extended"].as_i64() != Some(code) {
                    return Err(format!(
                        "Expectation mismatch for {}: expected SQLite {code}, got {}",
                        op.id, ev.data
                    ));
                }
            } else if ev.kind != "completed" {
                return Err(format!("Operation {} ended as {}", op.id, ev.kind));
            }
            if let Some(rows) = expected.get("rows")
                && &ev.data["result"]["rows"] != rows
            {
                return Err(format!("Row expectation mismatch for {}", op.id));
            }
            if expected["full_checkpoint"] == true || expected["partial_checkpoint"] == true {
                let (busy, log, checkpointed) = checkpoint_counts(&ev.data).ok_or_else(|| {
                    format!("Invalid or unavailable checkpoint counts for {}", op.id)
                })?;
                if expected["full_checkpoint"] == true && (busy != 0 || log != checkpointed) {
                    return Err(format!("Expected complete checkpoint for {}", op.id));
                }
                if expected["partial_checkpoint"] == true && log <= checkpointed {
                    return Err(format!("Expected incomplete checkpoint for {}", op.id));
                }
            }
        }
        Ok(())
    }
}
fn checkpoint_counts(data: &Value) -> Option<(i64, i64, i64)> {
    let result = &data["result"];
    if result["columns"] != json!(["busy", "log", "checkpointed"]) {
        return None;
    }
    let rows = result["rows"].as_array()?;
    if rows.len() != 1 {
        return None;
    }
    let row = rows[0].as_array()?;
    if row.len() != 3 {
        return None;
    }
    let integer = |cell: &Value| {
        if cell["type"] != "integer" {
            return None;
        }
        cell["value"]
            .as_str()?
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
    };
    let (busy, log, checkpointed) = (integer(&row[0])?, integer(&row[1])?, integer(&row[2])?);
    if busy > 1 || checkpointed > log {
        return None;
    }
    Some((busy, log, checkpointed))
}
fn scenario_ids_before(ops: &[Operation], id: &str) -> Vec<String> {
    let before: Vec<_> = ops.iter().take_while(|o| o.id != id).collect();
    let mut connections = std::collections::HashSet::new();
    let mut ids: Vec<_> = before
        .into_iter()
        .rev()
        .filter(|o| !o.id.starts_with("seed_") && connections.insert(o.connection.clone()))
        .map(|o| o.id.clone())
        .collect();
    ids.reverse();
    ids
}

async fn send(w: &Worker, v: Value) -> Result<(), String> {
    let mut input = w.input.lock().await;
    input
        .write_all(format!("{v}\n").as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    input.flush().await.map_err(|e| e.to_string())
}
