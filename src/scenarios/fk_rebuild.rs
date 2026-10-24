//! SQLite table rebuild experiments with cascading foreign keys and typed data checks.
use super::*;

const PARENT: &str = "CREATE TABLE parent(id INTEGER PRIMARY KEY, label TEXT NOT NULL)";
const REPLACEMENT: &str = "CREATE TABLE replacement(id INTEGER PRIMARY KEY, label TEXT NOT NULL)";
const CHILD: &str = "CREATE TABLE child(id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parent(id) ON DELETE CASCADE, label TEXT NOT NULL, amount INTEGER NOT NULL, weight REAL NOT NULL, payload BLOB, memo TEXT)";

fn records(parent: &str) -> String {
    format!(
        "SELECT 'parent' AS entity, id, NULL AS parent_id, label, NULL AS amount, NULL AS weight, NULL AS payload, NULL AS memo FROM {parent} UNION ALL SELECT 'child', id, parent_id, label, amount, weight, payload, memo FROM child ORDER BY entity DESC, id"
    )
}

fn original_rows(children: bool) -> Value {
    use Cell::*;
    let mut records = vec![
        vec![
            Text("parent".into()),
            Integer("9007199254740993".into()),
            Null,
            Text("north".into()),
            Null,
            Null,
            Null,
            Null,
        ],
        vec![
            Text("parent".into()),
            Integer("9007199254740995".into()),
            Null,
            Text("south".into()),
            Null,
            Null,
            Null,
            Null,
        ],
    ];
    if children {
        records.extend([
            vec![
                Text("child".into()),
                Integer("101".into()),
                Integer("9007199254740993".into()),
                Text("alpha".into()),
                Integer("9223372036854775807".into()),
                Real(1.25),
                Blob("00ff".into()),
                Null,
            ],
            vec![
                Text("child".into()),
                Integer("102".into()),
                Integer("9007199254740993".into()),
                Text("beta".into()),
                Integer("-7".into()),
                Real(2.5),
                Blob("".into()),
                Text("".into()),
            ],
            vec![
                Text("child".into()),
                Integer("103".into()),
                Integer("9007199254740995".into()),
                Text("gamma".into()),
                Integer("0".into()),
                Real(-0.5),
                Null,
                Text("kept".into()),
            ],
        ]);
    }
    json!({"rows": records})
}

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Loss,
    OffInside,
    OffBefore,
    Deferred,
    Observer,
    Restore,
}

// Every step has a confirmed completion barrier, including across connections.
fn step(s: &mut Scenario, id: &str, c: &str, sql: &str, expect: Value, note: &str) {
    let after = s.operations.last().map_or_else(Vec::new, |o| done(&o.id));
    s.operations.push(op(id, c, sql, after, expect, note));
}

fn scenario(variant: Variant, name: &str, description: &str) -> Scenario {
    use Variant::*;
    let mut s = base(name, description);
    s.settings.connections = 2;
    s.initial = vec![
        PARENT.into(),
        CHILD.into(),
        "INSERT INTO parent VALUES(9007199254740993, 'north'), (9007199254740995, 'south')".into(),
        "INSERT INTO child VALUES(101, 9007199254740993, 'alpha', 9223372036854775807, 1.25, X'00ff', NULL), (102, 9007199254740993, 'beta', -7, 2.5, X'', ''), (103, 9007199254740995, 'gamma', 0, -0.5, NULL, 'kept')".into(),
    ];
    step(
        &mut s,
        "a_fk_on",
        "A",
        "PRAGMA foreign_keys=ON",
        Value::Null,
        "Set enforcement explicitly on migration connection A before BEGIN.",
    );
    step(
        &mut s,
        "a_fk_enabled",
        "A",
        "PRAGMA foreign_keys",
        rows(1),
        "Confirm that FK enforcement is enabled on migration connection A.",
    );
    step(
        &mut s,
        "a_fk_schema",
        "A",
        "PRAGMA foreign_key_list(child)",
        json!({"rows":[[{"type":"integer","value":"0"},{"type":"integer","value":"0"},{"type":"text","value":"parent"},{"type":"text","value":"parent_id"},{"type":"text","value":"id"},{"type":"text","value":"NO ACTION"},{"type":"text","value":"CASCADE"},{"type":"text","value":"NONE"}]]}),
        "Verify the actual child -> parent ON DELETE CASCADE definition.",
    );
    if variant == Observer {
        step(
            &mut s,
            "b_begin",
            "B",
            "BEGIN",
            Value::Null,
            "BEGIN alone does not pin a WAL snapshot; the next read does.",
        );
        step(
            &mut s,
            "b_snapshot",
            "B",
            &records("parent"),
            original_rows(true),
            "B pins a read snapshot of the complete original records before A rebuilds.",
        );
    }
    if variant == OffBefore {
        step(
            &mut s,
            "b_fk_on",
            "B",
            "PRAGMA foreign_keys=ON",
            Value::Null,
            "B's independent setting cannot configure A.",
        );
        step(
            &mut s,
            "a_fk_off",
            "A",
            "PRAGMA foreign_keys=OFF",
            Value::Null,
            "Disable enforcement on A before BEGIN. This control uses local SQLite transaction boundaries.",
        );
        step(
            &mut s,
            "b_fk_enabled",
            "B",
            "PRAGMA foreign_keys",
            rows(1),
            "B remains enabled while A is disabled.",
        );
        step(
            &mut s,
            "a_fk_disabled",
            "A",
            "PRAGMA foreign_keys",
            rows(0),
            "Confirm OFF on the connection that will execute DROP.",
        );
    }
    step(
        &mut s,
        "a_begin",
        "A",
        "BEGIN",
        Value::Null,
        "All rebuild operations run in one transaction on A.",
    );
    if variant == OffInside {
        step(
            &mut s,
            "a_fk_off_inside",
            "A",
            "PRAGMA foreign_keys=OFF",
            Value::Null,
            "SQLite accepts the statement but changing foreign_keys inside BEGIN is a no-op.",
        );
    }
    if variant == Deferred {
        step(
            &mut s,
            "a_defer_on",
            "A",
            "PRAGMA defer_foreign_keys=ON",
            Value::Null,
            "Deferring constraint checks does not cancel ON DELETE CASCADE actions.",
        );
    }
    step(
        &mut s,
        "a_fk_effective",
        "A",
        "PRAGMA foreign_keys",
        rows(i64::from(variant != OffBefore)),
        "Assert the effective enforcement setting inside the migration transaction on A.",
    );
    step(
        &mut s,
        "a_defer_effective",
        "A",
        "PRAGMA defer_foreign_keys",
        rows(i64::from(variant == Deferred)),
        "Read the actual deferral setting on A before rebuilding.",
    );
    if variant == Restore {
        step(
            &mut s,
            "a_save_child",
            "A",
            "CREATE TEMP TABLE saved_child AS SELECT id, parent_id, label, amount, weight, payload, memo FROM child",
            Value::Null,
            "Save all child columns in a temporary table within A's transaction.",
        );
    }
    step(
        &mut s,
        "a_create",
        "A",
        REPLACEMENT,
        Value::Null,
        "Create the replacement parent with the same schema.",
    );
    step(
        &mut s,
        "a_copy",
        "A",
        "INSERT INTO replacement(id, label) SELECT id, label FROM parent",
        Value::Null,
        "Copy parent IDs and values; child records have not been changed yet.",
    );
    step(
        &mut s,
        "a_copy_check",
        "A",
        &records("replacement"),
        original_rows(true),
        "The replacement contains every parent record before DROP.",
    );
    step(
        &mut s,
        "a_before_drop",
        "A",
        &records("parent"),
        original_rows(true),
        "Before DROP: two original parents and three complete child records.",
    );
    step(
        &mut s,
        "a_drop",
        "A",
        "DROP TABLE parent",
        Value::Null,
        "With FK enforcement ON, DROP performs an implicit DELETE and cascades immediately.",
    );
    step(
        &mut s,
        "a_after_drop",
        "A",
        &records("replacement"),
        original_rows(variant == OffBefore),
        "Read replacement and child after DROP. With enforcement ON, only the copied parents remain. With enforcement OFF, the children also survive.",
    );
    step(
        &mut s,
        "a_rename",
        "A",
        "ALTER TABLE replacement RENAME TO parent",
        Value::Null,
        "Restore the parent table name; this does not restore deleted children.",
    );
    if variant == Restore {
        step(
            &mut s,
            "a_restore_child",
            "A",
            "INSERT INTO child(id, parent_id, label, amount, weight, payload, memo) SELECT id, parent_id, label, amount, weight, payload, memo FROM saved_child",
            Value::Null,
            "Restore children after their parents exist, while FK enforcement remains ON.",
        );
        step(
            &mut s,
            "a_remove_saved",
            "A",
            "DROP TABLE saved_child",
            Value::Null,
            "Remove the transaction-local saved copy.",
        );
    }
    step(
        &mut s,
        "a_commit",
        "A",
        "COMMIT",
        Value::Null,
        "A successful COMMIT proves transaction acceptance, not preservation of original records.",
    );
    if variant == OffBefore {
        step(
            &mut s,
            "a_fk_restore",
            "A",
            "PRAGMA foreign_keys=ON",
            Value::Null,
            "Re-enable enforcement after COMMIT on A.",
        );
    }
    step(
        &mut s,
        "a_fk_after_commit",
        "A",
        "PRAGMA foreign_keys",
        rows(1),
        "Read back enabled enforcement on A after COMMIT.",
    );
    step(
        &mut s,
        "a_defer_after_commit",
        "A",
        "PRAGMA defer_foreign_keys",
        rows(0),
        "Deferral resets at COMMIT.",
    );
    if variant == Observer {
        step(
            &mut s,
            "b_stale",
            "B",
            &records("parent"),
            original_rows(true),
            "A has committed the loss, but B's pinned WAL snapshot still shows all original records.",
        );
        step(
            &mut s,
            "b_end",
            "B",
            "COMMIT",
            Value::Null,
            "End B's read transaction so the next read can see the committed loss.",
        );
    }
    let preserved = matches!(variant, OffBefore | Restore);
    step(
        &mut s,
        "b_after_commit",
        "B",
        &records("parent"),
        original_rows(preserved),
        if preserved {
            "Fresh reading after COMMIT: assert every record, ID, value and SQLite storage type, including BLOB/NULL and integers above 2^53."
        } else {
            "A fresh read confirms the expected loss: both original parent records remain and all three children are gone."
        },
    );
    if preserved {
        step(
            &mut s,
            "b_links",
            "B",
            "SELECT child.id, parent.id, parent.label FROM child JOIN parent ON child.parent_id=parent.id ORDER BY child.id",
            json!({"rows":[[{"type":"integer","value":"101"},{"type":"integer","value":"9007199254740993"},{"type":"text","value":"north"}],[{"type":"integer","value":"102"},{"type":"integer","value":"9007199254740993"},{"type":"text","value":"north"}],[{"type":"integer","value":"103"},{"type":"integer","value":"9007199254740995"},{"type":"text","value":"south"}]]}),
            "Fresh joined reading verifies all three child-parent relationships.",
        );
    }
    step(
        &mut s,
        "b_integrity",
        "B",
        "PRAGMA integrity_check",
        json!({"rows":[[{"type":"text","value":"ok"}]]}),
        "Structural/constraint checks can report ok despite deleted application data. This check does not check foreign keys.",
    );
    step(
        &mut s,
        "b_fk_check",
        "B",
        "PRAGMA foreign_key_check",
        json!({"rows":[]}),
        "No dangling foreign keys remain. Deleted children cannot violate a foreign key; empty output does not establish preservation.",
    );
    s
}

pub(super) fn builtins() -> Vec<Scenario> {
    use Variant::*;
    [
        (Loss, "FK rebuild · cascade loss", "Rebuild a parent table with ON DELETE CASCADE. The copied parents survive, all three children are deleted, and COMMIT succeeds. Both integrity checks pass despite the lost records. This experiment demonstrates local SQLite behavior; see the FK rebuild guide for Drizzle/D1 context."),
        (OffInside, "FK rebuild · OFF inside BEGIN", "Try foreign_keys=OFF inside BEGIN, then read the setting on connection A. Enforcement stays ON and the table rebuild deletes the children. Compare OFF before BEGIN to see why the transaction boundary matters."),
        (OffBefore, "FK rebuild · OFF before BEGIN", "Disable foreign_keys on A before BEGIN and verify that B remains independently enabled. All records survive the rebuild. After COMMIT, re-enable enforcement and check the complete records and their links. This control uses local SQLite transaction boundaries unavailable in D1 migrations."),
        (Deferred, "FK rebuild · deferred checks", "Enable defer_foreign_keys on A and read back both FK settings. DROP still cascades immediately and COMMIT succeeds with no children left. Deferral delays constraint checks; it does not cancel delete actions."),
        (Observer, "FK rebuild · WAL observer", "B reads the original records in a WAL transaction. A rebuilds the parent and commits the loss of its children, but B still sees the original records. End B's transaction and compare its fresh read with the old snapshot."),
        (Restore, "FK rebuild · preserve children", "Save every child column before DROP and restore the records after renaming the parent, within one transaction with FK enforcement ON. Fresh reads check every typed record and relationship. This control covers the example's simple schema; other FK graphs, triggers and D1 need separate analysis."),
    ]
    .into_iter()
    .map(|(v, name, description)| scenario(v, name, description))
    .collect()
}
