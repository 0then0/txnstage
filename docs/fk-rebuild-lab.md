# Foreign keys and data preservation during a table rebuild

A successful SQLite transaction can delete application records while leaving the database structurally valid. These experiments show how `ON DELETE CASCADE` acts during a parent-table rebuild, how PRAGMA transaction boundaries affect the result, and how an old WAL snapshot can hide committed loss.

TxnStage runs the SQL in its own disposable local databases. The examples demonstrate SQLite behavior. They provide context for reported Drizzle/D1 migrations, without executing an ORM or Cloudflare D1.

## Open an experiment

Start `txnstage serve` and choose an experiment numbered **07–12 · FK rebuild**. Use **Next ready step** to execute one operation, or **Run schedule** to run the full experiment in a fresh database. Select a completed timeline event to see its SQL, typed result table and transaction state.

For a source build, follow [Build and run](development.md#build-and-run). The [example catalog](../examples/README.md) contains the same scenarios and recorded runs for CLI use and import.

Each experiment begins with two parent records and three children. The child table references the parent through `ON DELETE CASCADE`. Connection A performs this rebuild, with each line submitted as a separate operation:

```sql
BEGIN;
CREATE TABLE replacement(id INTEGER PRIMARY KEY, label TEXT NOT NULL);
INSERT INTO replacement(id, label) SELECT id, label FROM parent;
DROP TABLE parent;
ALTER TABLE replacement RENAME TO parent;
COMMIT;
```

With FK enforcement enabled, DROP performs an implicit DELETE and invokes the cascade. Copying the parents to `replacement` does not protect their children, and renaming the replacement does not bring deleted children back. [SQLite foreign keys](https://www.sqlite.org/foreignkeys.html#fk_schemacommands).

## Compare the six variants

- **07 · cascade loss**: A enables FK enforcement before BEGIN and reads it back. Both copied parents survive the rebuild; all three children disappear. COMMIT succeeds, `integrity_check` returns `ok` and `foreign_key_check` returns no violations.
- **08 · OFF inside BEGIN**: A attempts `foreign_keys=OFF` after BEGIN. The statement completes, but A still reads `foreign_keys=1`. The children are deleted as in 07.
- **09 · OFF before BEGIN**: A disables enforcement before BEGIN and reads `0`, while B independently reads `1`. Children survive the rebuild. A re-enables enforcement after COMMIT; fresh reads check all records and links. This control uses local SQLite transaction boundaries unavailable in D1 migrations.
- **10 · deferred checks**: A reads `foreign_keys=1` and `defer_foreign_keys=1` inside the transaction. DROP still deletes the children. COMMIT succeeds and resets deferral to `0`.
- **11 · WAL observer**: B begins a read transaction and reads all five records before A rebuilds. After A commits the loss, B's existing transaction still sees all five. B ends its transaction; its next read sees only the two parents.
- **12 · preserve children**: A saves every child column in a temporary table inside its transaction. After DROP and rename, A restores the children and removes the saved copy. FK enforcement stays ON. Fresh B reads verify all original records and all three parent-child links.

Enforcement and deferral settings are read on **A**, the connection executing the rebuild. B's settings cannot establish A's enforcement state. Completed-event dependencies order all steps, including B's first read before A's rebuild. BEGIN alone does not pin a read snapshot.

The save/restore control covers this example's simple schema and fully represented values. Other FK graphs, triggers, schema changes, concurrent application writers and D1 require separate analysis.

## Inspect the data at each boundary

The result tables use the columns `entity`, `id`, `parent_id`, `label`, `amount`, `weight`, `payload` and `memo`. They include every parent and child column. Values for child-only columns are NULL in parent rows.

- `a_copy_check` reads the replacement before DROP. It contains both original parent records; the children still exist.
- `a_before_drop` reads the original parent and child tables: two parents and three children.
- `a_after_drop` reads **replacement** and **child** before rename or COMMIT. The logical entity label remains `parent`. It returns only the copied parents, except in 09 where the children also survive. In 12, this read occurs before child restoration.
- `a_commit` completes with `autocommit=true`.
- `b_after_commit` reads from a fresh snapshot. It returns two parents in 07, 08, 10 and 11, and all five records in 09 and 12. `b_links` verifies all three relationships in the preservation controls.
- `b_integrity` returns `ok`; `b_fk_check` returns no rows, including in variants where the children were deleted.

In 11, compare `b_stale` with `b_after_commit`. The first read reports a read transaction with autocommit OFF and still sees the original data. The second runs after `b_end` and sees the committed loss.

The fixtures distinguish values as well as row counts. Parent IDs `9007199254740993` and `9007199254740995` exceed JavaScript's exact integer range. Child IDs are `101`, `102` and `103`; the first two share a parent. Labels are `north`, `south`, `alpha`, `beta` and `gamma`. Child records include INTEGER amounts up to `9223372036854775807`, REAL weights, nonempty and empty BLOBs, NULLs and empty TEXT. Ordered assertions compare complete typed cells. INTEGER values stay decimal strings in recordings and the interface.

## Observation and preservation

The empty child table in 07 is its expected observation. The experiment passes when it reproduces that loss. To test preservation instead, run the separate [preservation assertion example](../examples/fk-preservation-fails.scenario.json). It runs 07's rebuild and diagnostic queries, then performs a final fresh read, `b_preservation`, requiring every original typed record. The same record expectation passes in 12 after restoration.

From the repository root, run these commands separately:

```sh
./target/release/txnstage run examples/07.scenario.json --output loss.recording.json
./target/release/txnstage run examples/fk-preservation-fails.scenario.json --output negative.recording.json
./target/release/txnstage run examples/12.scenario.json --output preserved.recording.json
```

The exit codes are **0, 1 and 0**, respectively. The middle command reports `TxnStage: Row expectation mismatch for b_preservation`. All SQL statements, including COMMIT and both diagnostic PRAGMAs, complete successfully. The final assertion fails because the result lacks the children. SQL completion and expectation matching remain separate outcomes.

Import the scenario or its [recording](../examples/recordings/fk-preservation-fails.recording.json), then choose **Rerun in fresh DB** to see the same assertion error. A recording stores observed SQL events and expected results; it has no separate overall assertion-status field. Historical `completed` events establish SQL completion, so rerun the scenario to evaluate its expectations.

## What integrity checks establish

COMMIT accepts a transaction; it does not compare application records with an earlier baseline. [integrity_check](https://www.sqlite.org/pragma.html#pragma_integrity_check) examines database structure and supported constraints but does not detect FK errors. [foreign_key_check](https://www.sqlite.org/pragma.html#pragma_foreign_key_check) finds violated relationships among remaining rows. Deleted children leave no orphan for it to report. Neither check proves preservation.

[WAL snapshot isolation](https://www.sqlite.org/isolation.html) lets an existing reader continue to see earlier records after another connection commits. A preservation check needs an appropriate fresh snapshot as well as explicit expectations for the data.

TxnStage permits the read-only, argument-free `PRAGMA foreign_key_check`. Its argument-bearing forms are rejected by the SQL allowlist.

## Replay a recording

The [recordings directory](../examples/recordings/) contains all six variants and the failing preservation assertion:

```sh
./target/release/txnstage run examples/recordings/11.recording.json --output observer-rerun.recording.json
./target/release/txnstage run examples/recordings/12.recording.json --output preserved-rerun.recording.json
./target/release/txnstage run examples/recordings/fk-preservation-fails.recording.json --output negative-rerun.recording.json
```

The last command exits **1** with the preservation mismatch. Import displays a recording without executing SQL; an explicit rerun creates new workers and a fresh database. Process IDs, timing and file sizes describe the captured execution and can differ on replay. See the [artifact format](artifact-format.md) for compatibility and result limits.

The FK recordings use TxnStage **0.1.1**, bundled SQLite **3.53.2**, on **macOS aarch64**, with SQLite source ID:

```text
2026-06-03 19:12:13 d6e03d8c777cfa2d35e3b60d8ec3e0187f3e9f99d8e2ee9cac695fd6fcdf1a24
```

Compile options and settings are included in each recording. Run `txnstage version` to inspect your build. A capture from one platform does not establish results on another; [verification](verification.md) describes the available checks.

## Sources and provenance

Source context was checked on **2026-10-24**:

- [SQLite foreign keys](https://www.sqlite.org/foreignkeys.html), [foreign_keys](https://www.sqlite.org/pragma.html#pragma_foreign_keys) and [defer_foreign_keys](https://www.sqlite.org/pragma.html#pragma_defer_foreign_keys) document enforcement, transaction boundaries and DROP actions.
- [Drizzle issue #4938](https://github.com/drizzle-team/drizzle-orm/issues/4938) reports table-recreation data loss with `drizzle-orm 0.44.2` and `drizzle-kit 0.31.4`. This lab does not execute those or current ORM versions and makes no claim that current releases retain the reported defect.
- [ZerGo0/drizzle-d1-reprod](https://github.com/ZerGo0/drizzle-d1-reprod/tree/8b1001684ed2ae30dcfe065970b5cb7acad9b0bb) provides external reproduction context at revision `8b1001684ed2ae30dcfe065970b5cb7acad9b0bb`, dated 2026-02-10. Its manifest lists Drizzle packages `1.0.0-beta.11-fc31a5f`, Wrangler `^4.59.2` and an ISC license declaration. The inspected tree has no separate LICENSE file, and its workaround script credits another source. TxnStage distributes its own SQL examples and captures; no materials from that repository are included.
- [Cloudflare D1 foreign keys](https://developers.cloudflare.com/d1/sql-api/foreign-keys/) documents enforcement and implicit transactions during queries and migrations. D1's deferral support does not make the local SQLite controls a D1 reproduction or migration recipe.
