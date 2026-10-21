use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const FORMAT: u32 = 1;
pub const MAX_ARTIFACT: usize = 8 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Cell {
    Null,
    Integer(String),
    Real(#[serde(with = "real_value")] f64),
    Text(String),
    Blob(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub journal_mode: String,
    pub busy_ms: u64,
    pub deadline_ms: u64,
    pub row_limit: usize,
    pub connections: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            journal_mode: "wal".into(),
            busy_ms: 2000,
            deadline_ms: 5000,
            row_limit: 100,
            connections: 3,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub operation: String,
    pub event: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: String,
    pub connection: String,
    pub sql: String,
    #[serde(default)]
    pub params: Vec<Cell>,
    #[serde(default)]
    pub after: Vec<Dependency>,
    #[serde(default)]
    pub expect: Value,
    #[serde(default)]
    pub note: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub format_version: u32,
    pub name: String,
    pub description: String,
    pub settings: Settings,
    pub initial: Vec<String>,
    pub operations: Vec<Operation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub elapsed_ms: u64,
    pub connection: String,
    pub operation: String,
    pub kind: String,
    pub data: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recording {
    pub format_version: u32,
    pub scenario: Scenario,
    pub environment: Value,
    pub events: Vec<Event>,
}
pub fn environment() -> Value {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let source: String = conn
        .query_row("SELECT sqlite_source_id()", [], |r| r.get(0))
        .unwrap();
    let opts: Vec<String> = conn
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    json!({"txnstage":env!("CARGO_PKG_VERSION"),"sqlite_version":rusqlite::version(),"sqlite_source_id":source,"compile_options":opts,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"busy_policy":"instrumented fixed 10ms retry; not sqlite3_busy_timeout"})
}
pub fn validate_operation(op: &Operation) -> Result<(), String> {
    if op.id.is_empty()
        || op.id.len() > 80
        || !op
            .id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(
            "Operation ID must be 1–80 ASCII letters, digits, hyphens or underscores".into(),
        );
    }
    if !["A", "B", "C"].contains(&op.connection.as_str()) {
        return Err("Unknown connection".into());
    }
    if op.note.len() > 4096
        || op.after.len() > 200
        || serde_json::to_vec(&op.expect)
            .map_err(|e| e.to_string())?
            .len()
            > 65536
    {
        return Err("Operation metadata exceeds limits".into());
    }
    if op.sql.is_empty() || op.sql.len() > 65536 || op.sql.contains('\0') || op.params.len() > 100 {
        return Err("SQL limit: 64 KiB, one statement, no NUL; at most 100 parameters".into());
    }
    if !op.expect.is_null() {
        let e = op
            .expect
            .as_object()
            .ok_or("Expectation must be an object or null")?;
        if e.keys().any(|k| {
            !["extended", "rows", "partial_checkpoint", "full_checkpoint"].contains(&k.as_str())
        }) {
            return Err("Unknown expectation field".into());
        }
        if e.get("extended")
            .is_some_and(|v| !v.as_u64().is_some_and(|n| n > 0 && n <= i32::MAX as u64))
        {
            return Err("Expected extended code must be a positive SQLite integer code".into());
        }
        for k in ["partial_checkpoint", "full_checkpoint"] {
            if e.get(k).is_some_and(|v| v != true) {
                return Err("Checkpoint expectation must be true".into());
            }
        }
        if let Some(rows) = e.get("rows") {
            let rows: Vec<Vec<Cell>> = serde_json::from_value(rows.clone())
                .map_err(|_| "Expected rows must be typed cells")?;
            if rows.len() > 200 || rows.iter().any(|r| r.len() > 100) {
                return Err("Expected rows exceed limits".into());
            }
        }
    }
    for p in &op.params {
        match p {
            Cell::Integer(v) => {
                v.parse::<i64>()
                    .map_err(|_| "INTEGER must be a signed 64-bit decimal string")?;
            }
            Cell::Text(v) if v.len() > 65536 => return Err("TEXT parameter limit is 64 KiB".into()),
            Cell::Blob(v)
                if v.len() > 131072
                    || v.len() % 2 != 0
                    || !v.bytes().all(|x| x.is_ascii_hexdigit()) =>
            {
                return Err("BLOB must be hex, at most 64 KiB".into());
            }
            Cell::Real(v) if !v.is_finite() => return Err("REAL must be finite".into()),
            _ => {}
        }
    }
    Ok(())
}
impl Scenario {
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != FORMAT {
            return Err("Unsupported scenario format_version".into());
        }
        if self.name.len() > 120 || self.description.len() > 4096 {
            return Err("Scenario title/description exceeds limits".into());
        }
        let s = &self.settings;
        if !["wal", "delete"].contains(&s.journal_mode.as_str())
            || s.connections < 2
            || s.connections > 3
            || s.busy_ms > 10000
            || !(50..=30000).contains(&s.deadline_ms)
            || !(1..=200).contains(&s.row_limit)
        {
            return Err("Invalid settings: WAL/DELETE, 2–3 connections, busy ≤10s, deadline 50ms–30s, 1–200 rows".into());
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
            return Err("Scenario exceeds 1 MiB".into());
        }
        if self.operations.len() > 200
            || self.initial.len() > 50
            || self
                .initial
                .iter()
                .any(|s| s.len() > 65536 || s.contains('\0'))
        {
            return Err("Scenario exceeds operation/SQL limits".into());
        }
        let mut seen = std::collections::HashSet::new();
        for op in &self.operations {
            validate_operation(op)?;
            if op.connection == "C" && s.connections < 3 {
                return Err("Connection C is disabled".into());
            }
            if op.id.starts_with("seed_") || !seen.insert(op.id.clone()) {
                return Err("Duplicate or reserved operation ID".into());
            }
            for d in &op.after {
                if !seen.contains(&d.operation)
                    || d.operation == op.id
                    || ![
                        "completed",
                        "failed",
                        "cancelled",
                        "timed_out",
                        "busy",
                        "running",
                    ]
                    .contains(&d.event.as_str())
                {
                    return Err(
                        "Dependencies must reference an earlier operation and confirmed event"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}
impl Recording {
    /// The exported bytes use the same compact encoding as the import size check.
    pub fn to_json(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != FORMAT
            || self.events.len() > 20000
            || !self.environment.is_object()
        {
            return Err("Unsupported recording version or too many events".into());
        }
        self.scenario.validate()?;
        for (i, e) in self.events.iter().enumerate() {
            if e.seq != i as u64 + 1
                || !["A", "B", "C"].contains(&e.connection.as_str())
                || ![
                    "ready",
                    "created",
                    "running",
                    "busy",
                    "completed",
                    "failed",
                    "cancelled",
                    "timed_out",
                    "cancel_requested",
                    "worker_lost",
                ]
                .contains(&e.kind.as_str())
                || !e.data.is_object()
            {
                return Err("Invalid event envelope".into());
            }
            if e.data.get("transaction").is_some_and(|v| !v.is_string()) {
                return Err("Invalid transaction metadata".into());
            }
            if e.data.get("autocommit").is_some_and(|v| !v.is_boolean()) {
                return Err("Invalid autocommit metadata".into());
            }
            for field in ["busy_calls", "duration_ms"] {
                if e.data.get(field).is_some_and(|v| v.as_u64().is_none()) {
                    return Err("Invalid numeric event metadata".into());
                }
            }
            if let Some(error) = e.data.get("error") {
                for field in ["primary", "extended"] {
                    if error.get(field).is_some_and(|v| v.as_u64().is_none()) {
                        return Err("Invalid SQLite error code".into());
                    }
                }
            }
            if let Some(v) = e.data.get("result") {
                let result: RecordedResult = serde_json::from_value(v.clone())
                    .map_err(|e| format!("Invalid recorded result: {e}"))?;
                if result.rows.len() > 200
                    || result.columns.len() > 100
                    || result.rows.iter().any(|r| r.len() != result.columns.len())
                {
                    return Err("Invalid recorded row shape or limits".into());
                }
            }
            if let Some(v) = e.data.get("error")
                && (!v["category"].is_string() || !v["message"].is_string())
            {
                return Err("Invalid recorded error".into());
            }
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_ARTIFACT {
            return Err("Recording exceeds 8 MiB".into());
        }
        Ok(())
    }
}

// SQLite REAL can contain infinity; JSON numbers cannot. Preserve it explicitly.
mod real_value {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_finite() {
            s.serialize_f64(*v)
        } else {
            s.serialize_str(if v.is_sign_positive() {
                "Infinity"
            } else {
                "-Infinity"
            })
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum R {
            Number(f64),
            Text(String),
        }
        match R::deserialize(d)? {
            R::Number(n) => Ok(n),
            R::Text(s) if s == "Infinity" => Ok(f64::INFINITY),
            R::Text(s) if s == "-Infinity" => Ok(f64::NEG_INFINITY),
            _ => Err(serde::de::Error::custom("Invalid REAL")),
        }
    }
}

#[derive(Deserialize)]
struct RecordedResult {
    columns: Vec<String>,
    rows: Vec<Vec<Cell>>,
    #[serde(rename = "row_count")]
    _row_count: String,
    #[serde(rename = "rows_truncated")]
    _rows_truncated: bool,
    #[serde(rename = "cell_previews")]
    _cell_previews: bool,
    #[serde(rename = "statement")]
    _statement: String,
}
