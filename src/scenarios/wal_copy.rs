//! Compare a main-file copy with a SQLite snapshot, then checkpoint and copy again.
use super::*;

pub(super) fn scenario() -> Scenario {
    let mut scenario = base(
        "WAL · copy without the log",
        "A main-file copy can pass integrity_check while missing a committed row still in the WAL. Compare it with a VACUUM INTO snapshot and with a main-file copy made after a complete checkpoint. https://www.sqlite.org/howtocorrupt.html#_mispairing_database_files_and_hot_journals_",
    );
    scenario.format_version = FORMAT;
    scenario.settings.connections = 2;
    scenario.initial = vec![
        "CREATE TABLE entry(id INTEGER PRIMARY KEY, label TEXT NOT NULL)".into(),
        "INSERT INTO entry VALUES (1, 'baseline')".into(),
    ];

    let baseline = [Cell::Integer("1".into()), Cell::Text("baseline".into())];
    let committed = [Cell::Integer("2".into()), Cell::Text("committed".into())];
    let one_row = json!({"rows":[baseline]});
    let both_rows = json!({"rows":[baseline,committed]});
    let select = "SELECT id, label FROM entry ORDER BY id";
    let sql = |id, c, text, expect, note| op(id, c, text, vec![], expect, note);
    let create = |id, kind, note| Operation {
        kind,
        ..sql(id, "A", "", Value::Null, note)
    };
    let verify = |id, copy: &str, text, expect, note| Operation {
        copy: Some(copy.into()),
        ..sql(id, "B", text, expect, note)
    };
    let integrity = |id, copy| {
        verify(
            id,
            copy,
            "PRAGMA integrity_check",
            json!({"rows":[[Cell::Text("ok".into())]]}),
            "Open the copy read-only and check its structural integrity.",
        )
    };
    for mut step in [
        sql(
            "journal_mode",
            "A",
            "PRAGMA journal_mode",
            json!({"rows":[[Cell::Text("wal".into())]]}),
            "Confirm the working database uses WAL.",
        ),
        sql(
            "autocheckpoint",
            "A",
            "PRAGMA wal_autocheckpoint",
            rows(0),
            "Confirm automatic checkpoints are disabled on the writer.",
        ),
        sql(
            "baseline_checkpoint",
            "A",
            "PRAGMA main.wal_checkpoint(TRUNCATE)",
            json!({"full_checkpoint":true,"rows":[vec![Cell::Integer("0".into()); 3]]}),
            "Confirm the schema and baseline row are in the main file and the WAL is truncated.",
        ),
        sql(
            "begin",
            "A",
            "BEGIN IMMEDIATE",
            Value::Null,
            "Start a write transaction.",
        ),
        sql(
            "insert",
            "A",
            "INSERT INTO entry VALUES (2, 'committed')",
            Value::Null,
            "Add the second row.",
        ),
        sql(
            "commit",
            "A",
            "COMMIT",
            Value::Null,
            "Wait for COMMIT to complete and keep A open.",
        ),
        sql(
            "live_rows",
            "B",
            select,
            both_rows.clone(),
            "A fresh read on B confirms both rows in the working database.",
        ),
        create(
            "main_before_checkpoint",
            OperationKind::CopyMain,
            "Copy the main database file into its own directory. Leave the WAL and SHM behind.",
        ),
        create(
            "snapshot",
            OperationKind::VacuumInto,
            "Take a consistent VACUUM INTO snapshot of the same committed state.",
        ),
        integrity("main_integrity", "main_before_checkpoint"),
        verify(
            "main_rows",
            "main_before_checkpoint",
            select,
            one_row,
            "The exact row check expects only the baseline in the main-file copy.",
        ),
        integrity("snapshot_integrity", "snapshot"),
        verify(
            "snapshot_rows",
            "snapshot",
            select,
            both_rows.clone(),
            "The exact row check expects both rows in the snapshot.",
        ),
        sql(
            "control_checkpoint",
            "A",
            "PRAGMA main.wal_checkpoint(FULL)",
            json!({"full_checkpoint":true}),
            "Confirm all WAL frames were checkpointed before making the control copy.",
        ),
        create(
            "main_after_checkpoint",
            OperationKind::CopyMain,
            "Copy the main file again after the complete checkpoint.",
        ),
        integrity("control_integrity", "main_after_checkpoint"),
        verify(
            "control_rows",
            "main_after_checkpoint",
            select,
            both_rows,
            "The exact row check expects both rows in the control copy.",
        ),
    ] {
        if let Some(previous) = scenario.operations.last() {
            step.after = done(&previous.id);
        }
        scenario.operations.push(step);
    }
    scenario
}
