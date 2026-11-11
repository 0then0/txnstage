//! Abrupt termination of one connection's worker process, then a new worker on the same database.
use super::*;

const ACCOUNTS: &str = "SELECT id, balance FROM account ORDER BY id";

fn accounts(values: &[(i64, i64)]) -> Value {
    let rows: Vec<Vec<Cell>> = values
        .iter()
        .map(|(id, balance)| {
            vec![
                Cell::Integer(id.to_string()),
                Cell::Integer(balance.to_string()),
            ]
        })
        .collect();
    json!({"rows":rows})
}
fn texts(values: &[&str]) -> Value {
    let rows: Vec<Vec<Cell>> = values
        .iter()
        .map(|v| vec![Cell::Text((*v).into())])
        .collect();
    json!({"rows":rows})
}
fn lifecycle(
    id: &str,
    c: &str,
    kind: OperationKind,
    after: Vec<Dependency>,
    note: &str,
) -> Operation {
    Operation {
        kind,
        ..op(id, c, "", after, Value::Null, note)
    }
}
fn ledger(name: &str, description: &str) -> Scenario {
    let mut s = base(name, description);
    s.format_version = FORMAT;
    s.settings.connections = 2;
    s.initial = vec![
        "CREATE TABLE account(id INTEGER PRIMARY KEY, balance INTEGER NOT NULL)".into(),
        "INSERT INTO account VALUES (1, 100), (2, 50)".into(),
    ];
    s
}

fn uncommitted() -> Scenario {
    let mut s = ledger(
        "Process crash · uncommitted transaction",
        "A changes a row inside an open transaction, then its worker process is forcibly terminated between statements. A new worker opens the same database file. Queries, not journal files, establish the outcome: the original rows remain, the old transaction is gone and new writes succeed. This models the abrupt end of one process, not power loss. https://www.sqlite.org/atomiccommit.html",
    );
    s.operations = vec![
        op(
            "a_begin",
            "A",
            "BEGIN IMMEDIATE",
            vec![],
            Value::Null,
            "A starts a write transaction.",
        ),
        op(
            "a_update",
            "A",
            "UPDATE account SET balance = balance - 30 WHERE id = 1",
            done("a_begin"),
            Value::Null,
            "Change a row without committing.",
        ),
        op(
            "a_own_view",
            "A",
            ACCOUNTS,
            done("a_update"),
            accounts(&[(1, 70), (2, 50)]),
            "Inside its transaction A reads its own uncommitted change.",
        ),
        lifecycle(
            "a_crash",
            "A",
            OperationKind::Crash,
            done("a_own_view"),
            "Forcibly terminate A's worker process after the confirmed statement. No ROLLBACK or close runs.",
        ),
        op(
            "b_while_closed",
            "B",
            ACCOUNTS,
            done("a_crash"),
            accounts(&[(1, 100), (2, 50)]),
            "B keeps working while A is closed and reads the original rows.",
        ),
        lifecycle(
            "a_reopen",
            "A",
            OperationKind::Reopen,
            vec![
                dep("a_crash", "completed"),
                dep("b_while_closed", "completed"),
            ],
            "Start a new worker for A on the same database file. Initialization SQL is not rerun.",
        ),
        op(
            "a_after_reopen",
            "A",
            ACCOUNTS,
            done("a_reopen"),
            accounts(&[(1, 100), (2, 50)]),
            "The uncommitted change did not survive.",
        ),
        op(
            "a_commit_lost",
            "A",
            "COMMIT",
            done("a_after_reopen"),
            json!({"extended":1}),
            "Expected SQLITE_ERROR (1): the new connection has no transaction to commit.",
        ),
        op(
            "a_write",
            "A",
            "UPDATE account SET balance = balance + 5 WHERE id = 2",
            vec![dep("a_commit_lost", "failed")],
            Value::Null,
            "A new autocommit write succeeds.",
        ),
        op(
            "b_final",
            "B",
            ACCOUNTS,
            done("a_write"),
            accounts(&[(1, 100), (2, 55)]),
            "B reads only the write made after reopen.",
        ),
    ];
    s
}

fn committed() -> Scenario {
    let mut s = ledger(
        "Process crash · confirmed COMMIT",
        "A commits a transaction and the scenario waits for the completed COMMIT before A's worker process is forcibly terminated. With the default WAL setting and autocheckpoint disabled, no checkpoint has run when the process ends. A new worker on the same file reads every committed change. WAL size is only a diagnostic; the queries establish durability across the process crash. https://www.sqlite.org/wal.html",
    );
    s.operations = vec![
        op(
            "a_begin",
            "A",
            "BEGIN IMMEDIATE",
            vec![],
            Value::Null,
            "A starts a write transaction.",
        ),
        op(
            "a_update",
            "A",
            "UPDATE account SET balance = balance - 30 WHERE id = 1",
            done("a_begin"),
            Value::Null,
            "Change an existing row.",
        ),
        op(
            "a_insert",
            "A",
            "INSERT INTO account VALUES (3, 30)",
            done("a_update"),
            Value::Null,
            "Add a row in the same transaction.",
        ),
        op(
            "a_commit",
            "A",
            "COMMIT",
            done("a_insert"),
            Value::Null,
            "The barrier: COMMIT must report completion before the crash.",
        ),
        lifecycle(
            "a_crash",
            "A",
            OperationKind::Crash,
            done("a_commit"),
            "Forcibly terminate A's worker process after the confirmed COMMIT.",
        ),
        op(
            "b_while_closed",
            "B",
            ACCOUNTS,
            done("a_crash"),
            accounts(&[(1, 70), (2, 50), (3, 30)]),
            "B reads the committed transaction while A is closed.",
        ),
        lifecycle(
            "a_reopen",
            "A",
            OperationKind::Reopen,
            vec![
                dep("a_crash", "completed"),
                dep("b_while_closed", "completed"),
            ],
            "Start a new worker for A on the same database file.",
        ),
        op(
            "a_after_reopen",
            "A",
            ACCOUNTS,
            done("a_reopen"),
            accounts(&[(1, 70), (2, 50), (3, 30)]),
            "Every committed change survived the process crash.",
        ),
        op(
            "a_journal_mode",
            "A",
            "PRAGMA journal_mode",
            done("a_after_reopen"),
            Value::Null,
            "Diagnostic only: the journal mode the new connection reports.",
        ),
        op(
            "a_write",
            "A",
            "INSERT INTO account VALUES (4, 1)",
            done("a_journal_mode"),
            Value::Null,
            "The reopened connection can write.",
        ),
        op(
            "b_final",
            "B",
            "SELECT count(*), sum(balance) FROM account",
            done("a_write"),
            json!({"rows":[[{"type":"integer","value":"4"},{"type":"integer","value":"151"}]]}),
            "B reads the committed rows and the new write.",
        ),
    ];
    s
}

const CONTACT: &str = "CREATE TABLE contact(customer_id INTEGER PRIMARY KEY REFERENCES customer(id), email TEXT NOT NULL)";
const SCHEMA: &str = "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name";

fn migration() -> Scenario {
    let mut s = base(
        "Process crash · partial migration",
        "A synthetic application contract: migration 2 creates a contact table, copies every customer email into it and records version 2. The application commits the CREATE separately, then the process is terminated during the copy. After reopen the table exists but is empty and version 2 is not recorded. A naive rerun stops at the existing table; a prepared resumable rerun completes and is checked independently. This is not an ORM migration runner and not a SQLite defect. https://www.sqlite.org/lang_transaction.html",
    );
    s.format_version = FORMAT;
    s.settings.connections = 2;
    s.initial = vec![
        "CREATE TABLE customer(id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL)".into(),
        "CREATE TABLE applied_migration(version INTEGER PRIMARY KEY, label TEXT NOT NULL)".into(),
        "INSERT INTO customer VALUES (1, 'Ada', 'ada@example.test'), (2, 'Lin', 'lin@example.test')".into(),
        "INSERT INTO applied_migration VALUES (1, 'customers')".into(),
    ];
    let mut last = String::new();
    let mut add = |s: &mut Scenario, mut o: Operation, event: &str| {
        if !last.is_empty() {
            o.after = vec![dep(&last, event)];
        }
        last = o.id.clone();
        s.operations.push(o);
    };
    let sql = |id: &str, c: &str, sql: &str, expect: Value, note: &str| {
        op(id, c, sql, vec![], expect, note)
    };
    for (o, event) in [
        (
            sql(
                "a_ddl_begin",
                "A",
                "BEGIN IMMEDIATE",
                Value::Null,
                "Migration 2, part one: schema change in its own transaction.",
            ),
            "completed",
        ),
        (
            sql(
                "a_ddl_create",
                "A",
                CONTACT,
                Value::Null,
                "Create the new table.",
            ),
            "completed",
        ),
        (
            sql(
                "a_ddl_commit",
                "A",
                "COMMIT",
                Value::Null,
                "Part one is committed separately. This is the application's choice.",
            ),
            "completed",
        ),
        (
            sql(
                "a_copy_begin",
                "A",
                "BEGIN IMMEDIATE",
                Value::Null,
                "Part two: copy data and record the version.",
            ),
            "completed",
        ),
        (
            sql(
                "a_copy",
                "A",
                "INSERT INTO contact SELECT id, email FROM customer",
                Value::Null,
                "Copy the emails without committing.",
            ),
            "completed",
        ),
        (
            lifecycle(
                "a_crash",
                "A",
                OperationKind::Crash,
                vec![],
                "Forcibly terminate the migrating process before part two commits.",
            ),
            "completed",
        ),
        (
            lifecycle(
                "a_reopen",
                "A",
                OperationKind::Reopen,
                vec![],
                "Start a new worker on the same database file.",
            ),
            "completed",
        ),
        (
            sql(
                "a_left_schema",
                "A",
                SCHEMA,
                texts(&["applied_migration", "contact", "customer"]),
                "What remained: the committed table exists.",
            ),
            "completed",
        ),
        (
            sql(
                "a_left_rows",
                "A",
                "SELECT (SELECT count(*) FROM contact), (SELECT max(version) FROM applied_migration)",
                json!({"rows":[[{"type":"integer","value":"0"},{"type":"integer","value":"1"}]]}),
                "The copy is gone and the recorded version is still 1.",
            ),
            "completed",
        ),
        (
            sql(
                "a_naive_create",
                "A",
                CONTACT,
                json!({"extended":1}),
                "Naive rerun from the top. Expected SQLITE_ERROR (1): table contact already exists.",
            ),
            "completed",
        ),
        (
            sql(
                "a_naive_state",
                "A",
                "SELECT (SELECT count(*) FROM contact), (SELECT max(version) FROM applied_migration)",
                json!({"rows":[[{"type":"integer","value":"0"},{"type":"integer","value":"1"}]]}),
                "The naive rerun stopped at its first statement and migrated nothing.",
            ),
            "failed",
        ),
        (
            sql(
                "a_resume_create",
                "A",
                "CREATE TABLE IF NOT EXISTS contact(customer_id INTEGER PRIMARY KEY REFERENCES customer(id), email TEXT NOT NULL)",
                Value::Null,
                "Resumable rerun: accept the table created before the crash.",
            ),
            "completed",
        ),
        (
            sql(
                "a_resume_begin",
                "A",
                "BEGIN IMMEDIATE",
                Value::Null,
                "Copy and version record share one transaction.",
            ),
            "completed",
        ),
        (
            sql(
                "a_resume_copy",
                "A",
                "INSERT OR IGNORE INTO contact SELECT id, email FROM customer",
                Value::Null,
                "Copy only the rows that are missing.",
            ),
            "completed",
        ),
        (
            sql(
                "a_resume_version",
                "A",
                "INSERT OR IGNORE INTO applied_migration VALUES (2, 'contacts')",
                Value::Null,
                "Record version 2 once.",
            ),
            "completed",
        ),
        (
            sql(
                "a_resume_commit",
                "A",
                "COMMIT",
                Value::Null,
                "Complete the migration.",
            ),
            "completed",
        ),
        (
            sql(
                "b_schema",
                "B",
                "SELECT name FROM pragma_table_info('contact') ORDER BY cid",
                texts(&["customer_id", "email"]),
                "Independent schema check from another connection.",
            ),
            "completed",
        ),
        (
            sql(
                "b_data",
                "B",
                "SELECT c.name, k.email FROM customer AS c JOIN contact AS k ON k.customer_id = c.id ORDER BY c.id",
                json!({"rows":[[{"type":"text","value":"Ada"},{"type":"text","value":"ada@example.test"}],[{"type":"text","value":"Lin"},{"type":"text","value":"lin@example.test"}]]}),
                "Independent data check: every customer has its email.",
            ),
            "completed",
        ),
        (
            sql(
                "b_version",
                "B",
                "SELECT version FROM applied_migration ORDER BY version",
                json!({"rows":[[{"type":"integer","value":"1"}],[{"type":"integer","value":"2"}]]}),
                "Version 2 is recorded exactly once.",
            ),
            "completed",
        ),
    ] {
        add(&mut s, o, event);
    }
    s
}

pub(super) fn builtins() -> Vec<Scenario> {
    vec![uncommitted(), committed(), migration()]
}
