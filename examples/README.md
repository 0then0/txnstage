# Experiment catalog

These scenarios are available in the workbench's **Experiments** list and as portable JSON files. Each defines a starting schema, connection settings, SQL operations, event dependencies and expected results. Every run uses a fresh database owned by TxnStage.

## Snapshots, writers and checkpoints

- [01 · Stale snapshot](01.scenario.json): A keeps its read snapshot after B commits. Promoting it to a writer returns `SQLITE_BUSY_SNAPSHOT` (517); a fresh transaction reads B's value.
- [02 · Two writers with retry](02.scenario.json): B reports a busy callback while A holds the writer slot. A commits, then B completes its write.
- [03 · Two writers without waiting](03.scenario.json): B's write returns `SQLITE_BUSY` (5) with a zero retry budget. A then commits, and B reads A's value.
- [04 · DEFERRED transactions](04.scenario.json): contention appears when a read transaction tries to write after another connection commits.
- [05 · IMMEDIATE transactions](05.scenario.json): contention appears at BEGIN IMMEDIATE; B proceeds after A commits.
- [06 · Reader and checkpoint](06.scenario.json): an old reader limits a PASSIVE checkpoint. Ending the read transaction permits a full checkpoint, followed by TRUNCATE.

## Foreign keys and table rebuilds

- [07 · FK cascade loss](07.scenario.json): copied parents survive a rebuild, children disappear, COMMIT succeeds and both diagnostic PRAGMAs pass.
- [08 · FK OFF inside BEGIN](08.scenario.json): disabling enforcement inside BEGIN has no effect; A reads back ON and the children are deleted.
- [09 · FK OFF before BEGIN](09.scenario.json): A disables enforcement before BEGIN while B remains ON. Records survive, and A re-enables enforcement after COMMIT.
- [10 · Deferred FK checks](10.scenario.json): deferral delays checking constraints while CASCADE still deletes children at DROP.
- [11 · WAL observer](11.scenario.json): B's old read snapshot hides A's committed child loss until B ends its transaction.
- [12 · Preserve children](12.scenario.json): explicit save/restore in one transaction preserves complete typed records and relationships in the example schema.

The [FK rebuild guide](../docs/fk-rebuild-lab.md) explains the SQL, inspection points, SQLite source ID and Drizzle/D1 context. These experiments demonstrate local SQLite behavior; the preservation control applies to the example's simple schema.

## Run a scenario

From the repository root, using a built executable:

```sh
./target/release/txnstage run examples/01.scenario.json --output snapshot.recording.json
./target/release/txnstage run snapshot.recording.json --output replay.recording.json
```

All twelve built-ins should exit **0** when their expectations match, including expected SQLite errors and observed cascade loss.

The separate [preservation assertion example](fk-preservation-fails.scenario.json) runs 07's damaging rebuild, then requires every original record. It intentionally exits **1** with `Row expectation mismatch for b_preservation`. Compare it with 12, which restores children and passes the same complete-record expectation.

```sh
./target/release/txnstage run examples/fk-preservation-fails.scenario.json --output negative.recording.json
./target/release/txnstage run examples/12.scenario.json --output preserved.recording.json
```

## Inspect and replay recordings

The [recordings directory](recordings/) contains captured executions of all twelve scenarios and the preservation assertion example. Open a scenario or recording with **Import** in the workbench. Import displays the file without executing SQL; **Rerun in fresh DB** starts an explicit fresh execution.

To replay a supplied recording from the command line:

```sh
./target/release/txnstage run examples/recordings/11.recording.json --output observer-rerun.recording.json
```

Recordings include SQLite environment metadata, submitted SQL and events. Process IDs, elapsed times, file sizes and retry counts describe the recorded run and can vary on replay. See the [artifact format](../docs/artifact-format.md) for field definitions, limits and compatibility.
