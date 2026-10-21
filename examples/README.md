# Example experiments

These scenarios are also available in the workbench's experiment library. Each file contains initial SQL, connection settings, operations, event dependencies and expected outcomes.

- [01 · Stale snapshot](01.scenario.json): A keeps its read snapshot after B commits. Promoting that snapshot to a writer returns `SQLITE_BUSY_SNAPSHOT` (517); a fresh transaction reads B's value.
- [02 · Two writers with retry](02.scenario.json): B reports a busy callback while A holds the writer slot. A commits, then B completes.
- [03 · Two writers without waiting](03.scenario.json): B's write returns `SQLITE_BUSY` (5) with a zero retry budget.
- [04 · DEFERRED transactions](04.scenario.json): a read/modify sequence encounters an obsolete snapshot when another connection commits first.
- [05 · IMMEDIATE transactions](05.scenario.json): contention appears at `BEGIN IMMEDIATE`; subsequent updates proceed after the first writer commits.
- [06 · Reader and checkpoint](06.scenario.json): a reader limits a PASSIVE checkpoint. Releasing the reader allows a full PASSIVE checkpoint, followed by TRUNCATE.

Run a scenario from the repository root:

```sh
./target/release/txnstage run examples/01.scenario.json --output snapshot.recording.json
```

Import either a scenario or recording in the workbench to inspect it. Import does not execute SQL. Choose **Rerun in fresh DB** to execute it on a new disposable database.

## Sample recordings

The files in [recordings/](recordings/) are captured executions of scenarios 01–06. They include the executed schedule, SQLite environment and events. Replay one with:

```sh
./target/release/txnstage run examples/recordings/01.recording.json --output replay.recording.json
```

Recordings preserve observed facts, including process IDs and elapsed times. Those fields are not inputs to replay or promises about future runs. SQLite/environment differences are reported when replaying a recording. See the [artifact format](../docs/artifact-format.md) for schema and compatibility details.
