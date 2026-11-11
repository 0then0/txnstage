# Documentation

Start with the [project README](../README.md) for installation, a first experiment and the terms used in the interface.

## Using TxnStage

- [Experiment catalog](../examples/README.md): what each built-in experiment shows, how to run it from the command line and how to replay the sample recordings.
- [FK rebuild guide](fk-rebuild-lab.md): a walkthrough of experiments 07–12, where a table rebuild deletes child rows through `ON DELETE CASCADE`.
- [Artifact format](artifact-format.md): the JSON files for scenarios and recordings, including expectations, typed values and crash and reopen steps.

## Understanding and changing TxnStage

- [Architecture](architecture.md): processes, the lifecycle of a statement, busy handling, cancellation, process crash and reopen, resource limits and how to interpret observations.
- [Development and distribution](development.md): prerequisites, repository layout, building, running checks, updating examples and packaging.
- [Testing and verification](verification.md): what each test suite covers and where reports are written.

## Project policies

- [Contributing](../CONTRIBUTING.md)
- [Code of Conduct](../CODE_OF_CONDUCT.md)
- [Security policy](../SECURITY.md)
- [Changelog](../CHANGELOG.md)
