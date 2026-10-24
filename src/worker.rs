//! Only this process owns an investigated SQLite connection. Stdout is JSONL only.
use crate::model::{Cell, Operation, Settings};
use rusqlite::{Connection, ffi};
use serde_json::{Value, json};
use std::{
    cell::Cell as Flag,
    ffi::{CStr, CString, c_char, c_int, c_void},
    io::{self, BufRead, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

fn emit(kind: &str, operation: &str, data: Value) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(
        out,
        "{}",
        json!({"kind":kind,"operation":operation,"data":data})
    );
    let _ = out.flush();
}
struct Context {
    reason: Arc<AtomicU8>,
    operation: String,
    started: Instant,
    busy_ms: u64,
    calls: u64,
    preparing_with_params: bool,
    pragma_params_denied: Flag<bool>,
}
unsafe extern "C" fn busy(ptr: *mut c_void, _count: c_int) -> c_int {
    let c = unsafe { &mut *(ptr as *mut Context) };
    c.calls += 1;
    // First callback is a barrier; later callbacks are counted, with sparse observations.
    if c.calls.is_power_of_two() {
        emit(
            "busy",
            &c.operation,
            json!({"calls":c.calls,"observation":"SQLite invoked the instrumented busy handler"}),
        );
    }
    if c.reason.load(Ordering::SeqCst) != 0 || c.started.elapsed().as_millis() >= c.busy_ms as u128
    {
        return 0;
    }
    std::thread::sleep(Duration::from_millis(10));
    i32::from(c.reason.load(Ordering::SeqCst) == 0)
}
unsafe extern "C" fn progress(ptr: *mut c_void) -> c_int {
    i32::from(
        unsafe { &*(ptr as *const Context) }
            .reason
            .load(Ordering::SeqCst)
            != 0,
    )
}
unsafe fn str_ptr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}
unsafe extern "C" fn authorize(
    ptr: *mut c_void,
    action: c_int,
    p1: *const c_char,
    p2: *const c_char,
    _: *const c_char,
    _: *const c_char,
) -> c_int {
    let a = unsafe { str_ptr(p1) }.to_ascii_lowercase();
    let b = unsafe { str_ptr(p2) }.to_ascii_lowercase();
    if action == ffi::SQLITE_ATTACH
        || action == ffi::SQLITE_DETACH
        || (action == ffi::SQLITE_FUNCTION
            && ["load_extension", "writefile", "readfile", "fts3_tokenizer"].contains(&b.as_str()))
    {
        return ffi::SQLITE_DENY;
    }
    if action == ffi::SQLITE_PRAGMA {
        // Direct PRAGMAs cannot bind parameters and may take effect in prepare.
        // Table-valued PRAGMAs inside parameterized SELECT run later, during step.
        if !ptr.is_null() {
            let context = unsafe { &*(ptr as *const Context) };
            if context.preparing_with_params {
                context.pragma_params_denied.set(true);
                return ffi::SQLITE_DENY;
            }
        }
        // Allow only experiment-relevant pragmas, never mutable instrumentation or paths.
        let read_only = [
            "journal_mode",
            "busy_timeout",
            "wal_autocheckpoint",
            "compile_options",
            "database_list",
            "page_count",
            "page_size",
            "freelist_count",
            "table_info",
            "table_xinfo",
            "index_info",
            "index_list",
            "foreign_key_list",
            "foreign_key_check",
            "schema_version",
            "user_version",
            "integrity_check",
            "quick_check",
        ];
        let mutable = [
            "wal_checkpoint",
            "foreign_keys",
            "read_uncommitted",
            "defer_foreign_keys",
        ];
        if !(mutable.contains(&a.as_str())
            || (read_only.contains(&a.as_str())
                && (p2.is_null()
                    || [
                        "table_info",
                        "table_xinfo",
                        "index_info",
                        "index_list",
                        "foreign_key_list",
                    ]
                    .contains(&a.as_str()))))
        {
            return ffi::SQLITE_DENY;
        }
    }
    ffi::SQLITE_OK
}
struct Statement(*mut ffi::sqlite3_stmt);
impl Drop for Statement {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_finalize(self.0);
        }
    }
}
unsafe extern "C" fn deny_tail(
    _: *mut c_void,
    _: c_int,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
) -> c_int {
    ffi::SQLITE_DENY
}

// Some PRAGMAs take effect during prepare. Reject an executable tail before
// preparing the first statement. sqlite3_complete understands quotes, comments,
// and CREATE TRIGGER bodies; this is not semicolon splitting.
unsafe fn preflight(db: *mut ffi::sqlite3, sql: &str, ctx: &Context) -> Result<(), Value> {
    for (i, b) in sql.bytes().enumerate() {
        if b != b';' {
            continue;
        }
        if ctx.reason.load(Ordering::SeqCst) != 0 {
            return Err(json!({"category":"interrupted","message":"Cancellation requested"}));
        }
        let prefix = CString::new(&sql[..=i]).unwrap();
        if unsafe { ffi::sqlite3_complete(prefix.as_ptr()) } == 0 {
            continue;
        }
        let tail = CString::new(&sql[i + 1..]).unwrap();
        let mut stmt = std::ptr::null_mut();
        unsafe {
            ffi::sqlite3_set_authorizer(db, Some(deny_tail), std::ptr::null_mut());
        }
        let rc = unsafe {
            ffi::sqlite3_prepare_v3(db, tail.as_ptr(), -1, 0, &mut stmt, std::ptr::null_mut())
        };
        let executable = !stmt.is_null();
        unsafe {
            ffi::sqlite3_finalize(stmt);
            ffi::sqlite3_set_authorizer(
                db,
                Some(authorize),
                (ctx as *const Context).cast_mut().cast(),
            );
        }
        if rc != ffi::SQLITE_OK || executable {
            return Err(
                json!({"category":"validation","message":"Exactly one SQL statement per operation; additional statements rejected before preparation"}),
            );
        }
        break;
    }
    Ok(())
}
unsafe fn error(db: *mut ffi::sqlite3) -> Value {
    let code = unsafe { ffi::sqlite3_extended_errcode(db) };
    json!({"category":"sql","primary":code&255,"extended":code,"message":if code&255==ffi::SQLITE_AUTH {"Sandbox denied this SQL. File/extension access is disabled. busy_timeout, wal_autocheckpoint and journal_mode cannot replace instrumentation; use experiment settings.".into()}else{unsafe{str_ptr(ffi::sqlite3_errmsg(db))}}})
}
unsafe fn execute(
    db: *mut ffi::sqlite3,
    op: &Operation,
    limit: usize,
    ctx: &mut Context,
) -> Result<Value, Value> {
    let sql = CString::new(op.sql.as_str())
        .map_err(|_| json!({"category":"validation","message":"NUL in SQL"}))?;
    unsafe { preflight(db, &op.sql, ctx) }?;
    let mut raw = std::ptr::null_mut();
    let mut tail = std::ptr::null();
    ctx.preparing_with_params = !op.params.is_empty();
    ctx.pragma_params_denied.set(false);
    let rc = unsafe { ffi::sqlite3_prepare_v3(db, sql.as_ptr(), -1, 0, &mut raw, &mut tail) };
    ctx.preparing_with_params = false;
    let stmt = Statement(raw);
    if ctx.pragma_params_denied.get() {
        return Err(
            json!({"category":"validation","message":"PRAGMA statements do not support bound parameters; no setting was changed"}),
        );
    }
    if rc != ffi::SQLITE_OK {
        return Err(unsafe { error(db) });
    }
    if raw.is_null() {
        return Err(
            json!({"category":"validation","message":"One executable statement is required"}),
        );
    }
    // SQLite parses the entire tail before any statement is stepped. Semicolons in triggers/strings are safe.
    while !tail.is_null() && unsafe { *tail } != 0 {
        let mut extra = std::ptr::null_mut();
        let mut next = std::ptr::null();
        let rc = unsafe { ffi::sqlite3_prepare_v3(db, tail, -1, 0, &mut extra, &mut next) };
        let has_extra = !extra.is_null();
        unsafe {
            ffi::sqlite3_finalize(extra);
        }
        if rc != ffi::SQLITE_OK || has_extra {
            return Err(
                json!({"category":"validation","message":"Exactly one SQL statement per operation; additional statements rejected before execution"}),
            );
        }
        if next == tail {
            break;
        }
        tail = next;
    }
    if unsafe { ffi::sqlite3_bind_parameter_count(raw) } as usize != op.params.len() {
        return Err(
            json!({"category":"validation","message":"Parameter count does not match placeholders"}),
        );
    }
    for (i, v) in op.params.iter().enumerate() {
        let idx = i as i32 + 1;
        let rc = match v {
            Cell::Null => unsafe { ffi::sqlite3_bind_null(raw, idx) },
            Cell::Integer(v) => unsafe { ffi::sqlite3_bind_int64(raw, idx, v.parse().unwrap()) },
            Cell::Real(v) => unsafe { ffi::sqlite3_bind_double(raw, idx, *v) },
            Cell::Text(v) => unsafe {
                ffi::sqlite3_bind_text(
                    raw,
                    idx,
                    v.as_ptr().cast(),
                    v.len() as i32,
                    ffi::SQLITE_TRANSIENT(),
                )
            },
            Cell::Blob(v) => {
                let bytes: Vec<u8> = (0..v.len())
                    .step_by(2)
                    .map(|x| u8::from_str_radix(&v[x..x + 2], 16).unwrap())
                    .collect();
                unsafe {
                    ffi::sqlite3_bind_blob(
                        raw,
                        idx,
                        bytes.as_ptr().cast(),
                        bytes.len() as i32,
                        ffi::SQLITE_TRANSIENT(),
                    )
                }
            }
        };
        if rc != ffi::SQLITE_OK {
            return Err(unsafe { error(db) });
        }
    }
    let n = unsafe { ffi::sqlite3_column_count(raw) };
    let columns: Vec<String> = (0..n)
        .map(|i| unsafe { str_ptr(ffi::sqlite3_column_name(raw, i)) })
        .collect();
    let mut rows = Vec::new();
    let mut total = 0u64;
    let mut bytes = 0usize;
    let mut preview = false;
    loop {
        if ctx.reason.load(Ordering::SeqCst) != 0 {
            return Err(json!({"category":"interrupted","message":"Cancellation requested"}));
        }
        match unsafe { ffi::sqlite3_step(raw) } {
            ffi::SQLITE_DONE => break,
            ffi::SQLITE_ROW => {
                total += 1;
                if rows.len() >= limit || bytes >= 512 * 1024 {
                    continue;
                }
                let mut row = Vec::new();
                for i in 0..n {
                    let cell = match unsafe { ffi::sqlite3_column_type(raw, i) } {
                        ffi::SQLITE_NULL => Cell::Null,
                        ffi::SQLITE_INTEGER => {
                            Cell::Integer(unsafe { ffi::sqlite3_column_int64(raw, i) }.to_string())
                        }
                        ffi::SQLITE_FLOAT => {
                            Cell::Real(unsafe { ffi::sqlite3_column_double(raw, i) })
                        }
                        ffi::SQLITE_BLOB => {
                            let len = unsafe { ffi::sqlite3_column_bytes(raw, i) } as usize;
                            let p = unsafe { ffi::sqlite3_column_blob(raw, i) } as *const u8;
                            let b = if len == 0 {
                                &[][..]
                            } else {
                                unsafe { std::slice::from_raw_parts(p, len.min(128)) }
                            };
                            preview |= len > 128;
                            Cell::Blob(b.iter().map(|x| format!("{x:02x}")).collect())
                        }
                        _ => {
                            let len = unsafe { ffi::sqlite3_column_bytes(raw, i) } as usize;
                            let p = unsafe { ffi::sqlite3_column_text(raw, i) };
                            let b = if len == 0 {
                                &[][..]
                            } else {
                                unsafe { std::slice::from_raw_parts(p, len.min(4096)) }
                            };
                            preview |= len > 4096;
                            Cell::Text(String::from_utf8_lossy(b).into_owned())
                        }
                    };
                    bytes += serde_json::to_vec(&cell).unwrap().len();
                    row.push(cell);
                }
                rows.push(row);
            }
            _ => return Err(unsafe { error(db) }),
        }
    }
    drop(stmt); // Fully drain, then finalize before publishing completion or transaction state.
    Ok(
        json!({"columns":columns,"rows":rows,"row_count":total.to_string(),"rows_truncated":total>limit as u64 || bytes>=512*1024,"cell_previews":preview,"changes":unsafe{ffi::sqlite3_changes64(db)}.to_string(),"statement":"drained and finalized"}),
    )
}
pub fn run(path: &str, settings: Settings) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        ffi::sqlite3_hard_heap_limit64(64 * 1024 * 1024);
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA temp_store=MEMORY; PRAGMA wal_autocheckpoint=0; PRAGMA max_page_count=16384;")?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 1024 * 1024)?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH, 65536)?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_COLUMN, 100)?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    let reason = Arc::new(AtomicU8::new(0));
    let active = Arc::new(Mutex::new(String::new()));
    let interrupt = conn.get_interrupt_handle();
    let (tx, rx) = std::sync::mpsc::sync_channel::<Operation>(1);
    let rc = reason.clone();
    let ac = active.clone();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = io::BufReader::new(stdin.lock());
        loop {
            let mut bytes = Vec::new();
            match input
                .by_ref()
                .take(1024 * 1024)
                .read_until(b'\n', &mut bytes)
            {
                Ok(0) | Err(_) => std::process::exit(0),
                _ => {}
            }
            if bytes.len() >= 1024 * 1024 {
                std::process::exit(2);
            }
            let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            match v["command"].as_str() {
                Some("execute") => {
                    if let Ok(op) = serde_json::from_value::<Operation>(v["operation"].clone()) {
                        let mut a = ac.lock().unwrap();
                        if !a.is_empty() {
                            continue;
                        }
                        rc.store(0, Ordering::SeqCst);
                        *a = op.id.clone();
                        drop(a);
                        if tx.send(op).is_err() {
                            return;
                        }
                    }
                }
                Some("cancel") => {
                    if v["operation"].as_str() == Some(ac.lock().unwrap().as_str()) {
                        rc.store(
                            if v["reason"] == "timed_out" { 2 } else { 1 },
                            Ordering::SeqCst,
                        );
                        interrupt.interrupt();
                    }
                }
                Some("shutdown") => std::process::exit(0),
                _ => {}
            }
        }
    });
    let mut context = Box::new(Context {
        reason,
        operation: String::new(),
        started: Instant::now(),
        busy_ms: settings.busy_ms,
        calls: 0,
        preparing_with_params: false,
        pragma_params_denied: Flag::new(false),
    });
    let db = unsafe { conn.handle() };
    let ptr = (&mut *context as *mut Context).cast();
    unsafe {
        ffi::sqlite3_extended_result_codes(db, 1);
        ffi::sqlite3_enable_load_extension(db, 0);
        ffi::sqlite3_set_authorizer(db, Some(authorize), ptr);
        ffi::sqlite3_busy_handler(db, Some(busy), ptr);
        ffi::sqlite3_progress_handler(db, 1000, Some(progress), ptr);
    }
    emit(
        "ready",
        "",
        json!({"pid":std::process::id(),"settings":settings}),
    );
    for op in rx {
        context.operation = op.id.clone();
        context.started = Instant::now();
        context.calls = 0;
        emit("running", &op.id, json!({"sql":op.sql,"params":op.params}));
        let result = unsafe { execute(db, &op, settings.row_limit, &mut context) };
        let why = context.reason.load(Ordering::SeqCst);
        let state = match why {
            1 => "cancelled",
            2 => "timed_out",
            _ => {
                if result.is_ok() {
                    "completed"
                } else {
                    "failed"
                }
            }
        };
        let tx_state = unsafe { ffi::sqlite3_txn_state(db, std::ptr::null()) };
        let mut data = json!({"duration_ms":context.started.elapsed().as_millis() as u64,"busy_calls":context.calls,"autocommit":conn.is_autocommit(),"transaction":match tx_state{0=>"none",1=>"read",2=>"write",_=>"unknown"}});
        match result {
            Ok(v) => data["result"] = v,
            Err(e) => data["error"] = e,
        };
        // Release control identity before coordinator can submit the next operation.
        active.lock().unwrap().clear();
        emit(state, &op.id, data);
    }
    Ok(())
}
