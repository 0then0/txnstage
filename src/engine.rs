//! Coordinator of one experiment: starts and supervises worker processes,
//! admits operations, records events in observation order, and handles
//! cancellation, planned crash and reopen, replay and cleanup.
use crate::model::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    sync::{Mutex, Notify, mpsc},
};

/// A new worker must report `ready` within this bound, at startup and on reopen.
pub const READY_DEADLINE_MS: u64 = 5000;
/// A killed worker must be reaped within this bound for its exit to count as confirmed.
pub const EXIT_DEADLINE_MS: u64 = 5000;
/// What `Child::kill` does on this platform. Neither lets SQLite close the connection.
pub const KILL_METHOD: &str = if cfg!(unix) {
    "SIGKILL"
} else {
    "TerminateProcess"
};

pub fn terminal(kind: &str) -> bool {
    TERMINAL.contains(&kind)
}
struct Worker {
    input: Mutex<ChildStdin>,
    kill: mpsc::Sender<()>,
    done: Notify,
    exited: std::sync::atomic::AtomicBool,
    exit: std::sync::Mutex<Option<std::process::ExitStatus>>,
    generation: u64,
    pid: Option<u32>,
}
/// Planned lifecycle of one logical connection. Unexpected loss is tracked in `State::lost`.
#[derive(Clone, Copy, PartialEq, Default)]
enum Phase {
    #[default]
    Open,
    Crashing,
    Closed,
    Reopening,
}
impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Crashing => "crashing",
            Self::Closed => "closed",
            Self::Reopening => "reopening",
        }
    }
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
    phase: HashMap<String, Phase>,
    // Identifies the worker process session; messages from another generation are stale.
    generation: HashMap<String, u64>,
}
impl State {
    fn phase(&self, connection: &str) -> Phase {
        self.phase.get(connection).copied().unwrap_or_default()
    }
    fn kind(&self, operation: &str) -> Option<OperationKind> {
        self.operations
            .iter()
            .rev()
            .find(|o| o.id == operation)
            .map(|o| o.kind)
    }
}
pub struct Engine {
    pub scenario: Scenario,
    pub environment: Value,
    state: Mutex<State>,
    // The current worker of each connection. A replaced worker has already been reaped.
    workers: std::sync::Mutex<HashMap<String, Arc<Worker>>>,
    executable: std::sync::Mutex<PathBuf>,
    pub changed: Notify,
    started: Instant,
    pub directory: TempDir,
}
fn spawn_worker(
    executable: &Path,
    db: &Path,
    settings: &Settings,
    generation: u64,
) -> Result<(Arc<Worker>, ChildStdout), String> {
    let mut child = Command::new(executable)
        .arg("--worker")
        .arg(db)
        .arg(serde_json::to_string(settings).unwrap())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (kill, mut killed) = mpsc::channel(1);
    let worker = Arc::new(Worker {
        input: Mutex::new(input),
        kill,
        done: Notify::new(),
        exited: std::sync::atomic::AtomicBool::new(false),
        exit: std::sync::Mutex::new(None),
        generation,
        pid: child.id(),
    });
    let w = Arc::downgrade(&worker);
    // The only owner of the child: it kills on request and always reaps.
    tokio::spawn(async move {
        let status = tokio::select! {
            s=child.wait()=>s.ok(),
            _=killed.recv()=>{let _=child.kill().await;child.wait().await.ok()}
        };
        if let Some(w) = w.upgrade() {
            *w.exit.lock().unwrap() = status;
            w.exited.store(true, Ordering::SeqCst);
            w.done.notify_waiters();
        }
    });
    Ok((worker, output))
}
async fn wait_exit(w: &Worker, timeout_ms: u64) -> bool {
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        loop {
            let done = w.done.notified();
            if w.exited.load(Ordering::SeqCst) {
                return;
            }
            done.await;
        }
    })
    .await
    .is_ok()
}
fn exit_status(w: &Worker) -> Value {
    let status = *w.exit.lock().unwrap();
    #[cfg(unix)]
    if let Some(signal) = status.and_then(|s| std::os::unix::process::ExitStatusExt::signal(&s)) {
        return json!({"signal":signal});
    }
    json!({"code":status.and_then(|s| s.code())})
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
        let mut state = State::default();
        for name in ["A", "B", "C"]
            .into_iter()
            .take(scenario.settings.connections)
        {
            let (worker, output) = spawn_worker(&executable, &db, &scenario.settings, 1)?;
            workers.insert(name.to_string(), worker);
            state.generation.insert(name.into(), 1);
            readers.push((name.to_string(), output));
        }
        let mut env = environment();
        env["settings"] = json!(scenario.settings);
        let engine = Arc::new(Self {
            scenario,
            environment: env,
            state: Mutex::new(state),
            workers: std::sync::Mutex::new(workers),
            executable: std::sync::Mutex::new(executable),
            changed: Notify::new(),
            started: Instant::now(),
            directory,
        });
        for (name, output) in readers {
            engine.read_worker(name, 1, output);
        }
        for n in engine.connections() {
            if let Err(e) = engine
                .wait_event("", Some(&n), "ready", READY_DEADLINE_MS)
                .await
            {
                engine.shutdown().await;
                return Err(e);
            }
        }
        for (i, sql) in engine.scenario.initial.iter().enumerate() {
            let op = Operation {
                id: format!("seed_{i}"),
                connection: "A".into(),
                kind: OperationKind::Sql,
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
    fn read_worker(self: &Arc<Self>, name: String, generation: u64, output: ChildStdout) {
        let e = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut lines = BufReader::new(output).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(e) = e.upgrade() {
                    match serde_json::from_str::<Value>(&line) {
                        Ok(v) => e.observe_from(&name, generation, v).await,
                        Err(_) => break,
                    }
                } else {
                    return;
                }
            }
            if let Some(e) = e.upgrade() {
                e.lost(
                    &name,
                    Some(generation),
                    "Worker exited; connection state lost",
                )
                .await;
            }
        });
    }
    fn connections(&self) -> Vec<String> {
        let mut names: Vec<_> = self.workers.lock().unwrap().keys().cloned().collect();
        names.sort();
        names
    }
    fn worker(&self, connection: &str) -> Option<Arc<Worker>> {
        self.workers.lock().unwrap().get(connection).cloned()
    }
    fn all_workers(&self) -> Vec<Arc<Worker>> {
        self.workers.lock().unwrap().values().cloned().collect()
    }
    /// Test seam: the executable used by later `reopen` operations.
    #[doc(hidden)]
    pub fn set_worker_executable(&self, executable: PathBuf) {
        *self.executable.lock().unwrap() = executable;
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
    /// Entry point for one worker message. `generation` identifies the process
    /// session that produced it: a message from a replaced or reaped process
    /// must never be attributed to the connection's current session.
    #[doc(hidden)]
    pub async fn observe_from(&self, connection: &str, generation: u64, v: Value) {
        let operation = v["operation"].as_str().unwrap_or("");
        let kind = v["kind"].as_str().unwrap_or("infrastructure");
        let mut data = v["data"].clone();
        let mut s = self.state.lock().await;
        if s.generation.get(connection) != Some(&generation)
            || s.phase(connection) == Phase::Closed
            // Lifecycle operations are reported by the coordinator only.
            || s.kind(operation).is_some_and(|k| !k.is_sql())
        {
            return;
        }
        if kind == "ready" {
            data["generation"] = json!(generation);
        }
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
    /// Record an unexpected loss. A planned crash, the closed state after it and a
    /// reopen in flight are owned by their lifecycle operation and are not a loss.
    async fn lost(&self, connection: &str, generation: Option<u64>, message: &str) {
        let mut s = self.state.lock().await;
        if generation.is_some_and(|g| s.generation.get(connection) != Some(&g))
            || s.phase(connection) != Phase::Open
            || s.lost.contains(&connection.to_string())
        {
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
        json!({"db_bytes":size("experiment.db"),"wal_bytes":size("experiment.db-wal"),"journal_bytes":size("experiment.db-journal"),"measurement":"filesystem stat only; no SQL or checkpoint","elapsed_ms":self.started.elapsed().as_millis() as u64})
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
        let connections: serde_json::Map<String, Value> = s
            .generation
            .iter()
            .map(|(c, g)| {
                (
                    c.clone(),
                    json!({"phase":s.phase(c).as_str(),"generation":g}),
                )
            })
            .collect();
        json!({"experiment":id,"event_offset":offset,"connections":connections,"scenario":self.scenario,"environment":self.environment,"events":&s.events[offset..],"active":s.active,"lost":s.lost,"stopped":s.stopped,"operations":s.operations,"files":self.files()})
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
        // Version 1 readers reject lifecycle steps, so only runs containing them move to 2.
        if scenario.operations.iter().any(|o| !o.kind.is_sql()) {
            scenario.format_version = FORMAT;
        }
        Recording {
            format_version: scenario.format_version,
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
        let w = self.worker(&op.connection).ok_or("Unknown connection")?;
        let c = &op.connection;
        let generation = {
            let mut s = self.state.lock().await;
            if s.stopped || s.lost.contains(c) {
                return Err("Experiment stopped or connection state lost; reset required".into());
            }
            // Lifecycle transitions are validated here, under the same lock that
            // admits SQL, so a crash can only land between confirmed operations.
            match (s.phase(c), op.kind) {
                (Phase::Crashing, _) => {
                    return Err(format!(
                        "Connection {c} is being crashed; wait for that operation's terminal event"
                    ));
                }
                (Phase::Reopening, _) => {
                    return Err(format!(
                        "Connection {c} is being reopened; wait for that operation's terminal event"
                    ));
                }
                (Phase::Closed, OperationKind::Sql) => {
                    return Err(format!(
                        "Connection {c} is closed after a planned process crash; reopen it before running SQL"
                    ));
                }
                (Phase::Closed, OperationKind::Crash) => {
                    return Err(format!(
                        "Connection {c} is already closed: its worker process was terminated and reaped. Reopen it before another crash"
                    ));
                }
                (Phase::Open, OperationKind::Reopen) => {
                    return Err(format!(
                        "Connection {c} is already open; reopen is only available after a planned crash of this connection"
                    ));
                }
                _ => {}
            }
            if s.active.contains_key(c) {
                return Err(if op.kind == OperationKind::Crash {
                    format!(
                        "Connection {c} has a running statement; crash is limited to the boundary between confirmed statements. Wait for its terminal event or cancel it first"
                    )
                } else {
                    "Connection already has a running operation".into()
                });
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
            let data = if op.kind.is_sql() {
                json!({"sql":op.sql,"params":op.params})
            } else {
                json!({"lifecycle":op.kind.as_str()})
            };
            self.append(&mut s, c, &op.id, "created", data);
            match op.kind {
                OperationKind::Sql => {}
                OperationKind::Crash => {
                    s.phase.insert(c.clone(), Phase::Crashing);
                }
                OperationKind::Reopen => {
                    s.phase.insert(c.clone(), Phase::Reopening);
                    *s.generation.entry(c.clone()).or_default() += 1;
                }
            }
            s.generation[c]
        };
        // Lifecycle operations are bounded by their own deadlines and report
        // terminal events like SQL, so dependencies and replay treat them alike.
        match op.kind {
            OperationKind::Sql => {}
            OperationKind::Crash => {
                tokio::spawn(self.clone().run_crash(op, w));
                return Ok(());
            }
            OperationKind::Reopen => {
                tokio::spawn(self.clone().run_reopen(op, generation));
                return Ok(());
            }
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
                        None,
                        "Experiment storage limit exceeded (128 MiB); state lost",
                    )
                    .await;
                    for w in e.all_workers() {
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
            self.lost(&op.connection, None, &e).await;
            return Err(e);
        }
        Ok(())
    }
    async fn run_crash(self: Arc<Self>, op: Operation, w: Arc<Worker>) {
        let c = &op.connection;
        let termination = json!({"method":KILL_METHOD,"cooperative_cleanup":false});
        let sampled = {
            let mut s = self.state.lock().await;
            let sampled = s
                .events
                .iter()
                .rev()
                .find(|e| e.connection == *c && e.data.get("transaction").is_some())
                .map(|e| json!({"transaction":e.data["transaction"],"autocommit":e.data["autocommit"]}));
            self.append(
                &mut s,
                c,
                &op.id,
                "running",
                json!({"lifecycle":"crash","generation":w.generation,"pid":w.pid,"termination":termination}),
            );
            sampled
        };
        // A full channel means a kill is already pending for this process.
        let _ = w.kill.try_send(());
        let confirmed = wait_exit(&w, EXIT_DEADLINE_MS).await;
        let mut s = self.state.lock().await;
        s.active.remove(c);
        if confirmed {
            s.phase.insert(c.clone(), Phase::Closed);
            let mut termination = termination;
            termination["exit_confirmed"] = json!(true);
            termination["reaped"] = json!(true);
            termination["exit_status"] = exit_status(&w);
            self.append(
                &mut s,
                c,
                &op.id,
                "completed",
                json!({"lifecycle":"crash","planned":true,"generation":w.generation,"pid":w.pid,"termination":termination,"session_state":{"lost":true,"last_sampled":sampled,"note":"Open transaction, connection PRAGMA values, temporary objects and prepared statements ended with the process. What reached the database is established by SQL after reopen."},"files_at_completion":self.files()}),
            );
        } else {
            // The process state is unknown: this is an infrastructure failure, not a planned crash.
            s.phase.insert(c.clone(), Phase::Open);
            s.lost.push(c.clone());
            let message = format!(
                "Worker exit was not confirmed within {EXIT_DEADLINE_MS} ms; reset required"
            );
            self.append(
                &mut s,
                c,
                &op.id,
                "failed",
                json!({"lifecycle":"crash","generation":w.generation,"error":{"category":"infrastructure","message":message},"state_lost":true}),
            );
            self.append(
                &mut s,
                c,
                "",
                "worker_lost",
                json!({"message":message,"state_lost":true}),
            );
        }
    }
    async fn run_reopen(self: Arc<Self>, op: Operation, generation: u64) {
        let c = &op.connection;
        let executable = self.executable.lock().unwrap().clone();
        let db = self.directory.path().join("experiment.db");
        // Same worker contract as at startup; the coordinator never touches the
        // database, its WAL/SHM or rollback journal, and runs no initialization SQL.
        let spawned = spawn_worker(&executable, &db, &self.scenario.settings, generation);
        let mut worker = None;
        let outcome: Result<Value, (&str, String)> = match spawned {
            Err(e) => Err((
                "failed",
                format!("Worker process could not be started: {e}"),
            )),
            Ok((w, output)) => {
                worker = Some(w.clone());
                let registered = {
                    let mut s = self.state.lock().await;
                    // Stop and shutdown reap registered workers; an unregistered one is ours to reap.
                    if !s.stopped {
                        self.workers.lock().unwrap().insert(c.clone(), w.clone());
                        self.append(
                            &mut s,
                            c,
                            &op.id,
                            "running",
                            json!({"lifecycle":"reopen","generation":generation,"pid":w.pid}),
                        );
                    }
                    !s.stopped
                };
                if registered {
                    self.read_worker(c.clone(), generation, output);
                }
                tokio::time::timeout(Duration::from_millis(READY_DEADLINE_MS), async {
                    loop {
                        let changed = self.changed.notified();
                        let exited = w.done.notified();
                        {
                            let s = self.state.lock().await;
                            if let Some(e) = s.events.iter().rev().find(|e| {
                                e.kind == "ready"
                                    && e.connection == *c
                                    && e.data["generation"] == generation
                            }) {
                                return Ok(e.data.clone());
                            }
                            if s.stopped {
                                return Err((
                                    "cancelled",
                                    "Experiment stopped before the new worker reported ready"
                                        .into(),
                                ));
                            }
                            if s.reasons.contains_key(&op.id) {
                                return Err((
                                    "cancelled",
                                    "Reopen cancelled before the new worker reported ready".into(),
                                ));
                            }
                        }
                        if w.exited.load(Ordering::SeqCst) {
                            return Err((
                                "failed",
                                "New worker process exited before reporting ready".into(),
                            ));
                        }
                        tokio::select! {_=changed=>{},_=exited=>{}}
                    }
                })
                .await
                .unwrap_or_else(|_| {
                    Err((
                        "timed_out",
                        format!("New worker did not report ready within {READY_DEADLINE_MS} ms"),
                    ))
                })
            }
        };
        // A failed attempt must not leave a process behind.
        let reaped = match (&outcome, &worker) {
            (Err(_), Some(w)) => {
                let _ = w.kill.try_send(());
                wait_exit(w, EXIT_DEADLINE_MS).await
            }
            _ => true,
        };
        let mut s = self.state.lock().await;
        s.active.remove(c);
        s.reasons.remove(&op.id);
        match outcome {
            Ok(ready) => {
                s.phase.insert(c.clone(), Phase::Open);
                self.append(
                    &mut s,
                    c,
                    &op.id,
                    "completed",
                    json!({"lifecycle":"reopen","generation":generation,"pid":ready["pid"],"database":"existing experiment.db; database, WAL, SHM and journal files kept; initialization SQL not rerun","settings_reapplied":"worker startup contract: sandbox and limits, autocheckpoint off, instrumented busy policy, row limit","session_state":"new connection: no transaction, default connection PRAGMA values, no temporary objects","transaction":ready["transaction"],"autocommit":ready["autocommit"],"files_at_completion":self.files()}),
                );
            }
            Err((kind, message)) => {
                let mut data = json!({"lifecycle":"reopen","generation":generation,"error":{"category":"infrastructure","message":message}});
                if reaped {
                    // Still the confirmed closed state left by the crash; reopen can be retried.
                    s.phase.insert(c.clone(), Phase::Closed);
                    data["connection"] = json!("closed");
                } else {
                    s.phase.insert(c.clone(), Phase::Open);
                    s.lost.push(c.clone());
                    data["state_lost"] = json!(true);
                }
                self.append(&mut s, c, &op.id, kind, data);
            }
        }
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
            match s.kind(&id) {
                // A kill in flight cannot be undone; the operation is bounded and reports itself.
                Some(OperationKind::Crash) => return,
                // The reopen task observes the request, reaps its worker and stays closed.
                Some(OperationKind::Reopen) => {
                    s.reasons.insert(id.clone(), reason.into());
                    self.append(
                        &mut s,
                        connection,
                        &id,
                        "cancel_requested",
                        json!({"reason":reason}),
                    );
                    return;
                }
                _ => {}
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
            if let Some(w) = self.worker(connection) {
                let _ = send(
                    &w,
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
            self.force_kill(connection).await;
        }
    }
    /// Unplanned kill of a worker, possibly mid-statement. Unlike the `crash`
    /// operation this is an infrastructure loss and the connection needs a reset.
    pub async fn force_kill(&self, connection: &str) {
        if let Some(w) = self.worker(connection) {
            let _ = w.kill.try_send(());
        }
        self.lost(
            connection,
            None,
            "Worker forcibly terminated; connection state lost",
        )
        .await;
    }
    pub async fn stop(self: &Arc<Self>) {
        self.state.lock().await.stopped = true;
        for conn in self.connections() {
            self.cancel(&conn, "cancelled").await;
        }
        self.shutdown().await;
    }
    pub async fn shutdown(&self) {
        // Once `stopped` is set under the state lock, a reopen in flight no longer
        // registers its worker; it reaps that process itself.
        self.state.lock().await.stopped = true;
        let workers = self.all_workers();
        for w in &workers {
            let _ = w.kill.send(()).await;
        }
        for w in &workers {
            loop {
                let notified = w.done.notified();
                if w.exited.load(Ordering::SeqCst) {
                    break;
                }
                let _ = tokio::time::timeout(Duration::from_secs(3), notified).await;
            }
        }
        // Let a crash or reopen in flight record its terminal event. Both are bounded.
        let _ = tokio::time::timeout(
            Duration::from_millis(READY_DEADLINE_MS + EXIT_DEADLINE_MS),
            async {
                loop {
                    let changed = self.changed.notified();
                    if !self
                        .state
                        .lock()
                        .await
                        .phase
                        .values()
                        .any(|p| matches!(p, Phase::Crashing | Phase::Reopening))
                    {
                        return;
                    }
                    changed.await;
                }
            },
        )
        .await;
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
