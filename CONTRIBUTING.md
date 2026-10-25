# Contributing to TxnStage

TxnStage is a local SQLite laboratory. Contributions should keep SQL execution real, experiments reproducible, and all database changes confined to disposable databases owned by TxnStage.

## Report a problem or propose a change

Use a [bug report](https://github.com/0then0/txnstage/issues/new?template=bug_report.yml) for incorrect behavior and a [feature request](https://github.com/0then0/txnstage/issues/new?template=feature_request.yml) for a proposed capability. Include a small scenario or recording when it helps reproduce the problem. Remove launch tokens, secrets and private data before sharing files.

Report suspected vulnerabilities through the private channel in [SECURITY.md](SECURITY.md). Community interactions follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Set up a checkout

Fork the repository, clone your fork and create a branch for the change. Requirements and build instructions are in [Development](docs/development.md). Use the existing Cargo and pnpm lockfiles; run pnpm from `frontend` to select the declared package-manager version.

`.editorconfig` describes editor defaults. Rustfmt and Biome are the formatting authorities for Rust and frontend code. Preserve the LF line endings defined by `.gitattributes`. Generated artifacts, secrets and temporary SQLite files belong outside version control.

## Make a focused change

- Follow the responsibilities in [Architecture](docs/architecture.md). The CLI and interface share the same engine and scheduler.
- Use confirmed event dependencies to order experiments. Do not replace SQLite observations with simulated lock states or sleep-based ordering.
- Preserve typed values, including decimal-string INTEGERs, explicit previews and the distinction between SQL completion and expectation matching.
- Add regression tests for meaningful behavior changes. Prefer existing libraries and verification tools; explain any dependency changes.
- Update user-facing documentation and add a concise entry under **Unreleased** in [CHANGELOG.md](CHANGELOG.md). Dependency-only maintenance can be grouped into one entry.
- When changing built-in scenarios, update their JSON files and affected recordings as described in [Update examples](docs/development.md#update-examples). Keep capture metadata factual.

## Verify the change

Run the commands in [Run checks](docs/development.md#run-checks), starting with the checks affected by your change. Native tests use disposable databases. Browser tests exercise the release binary, so rebuild the frontend and release executable before running them. Run repository and packaging checks from the root:

```sh
python3 -m unittest discover -s tests -p 'test_*.py'
```

Explain any check you could not run. A passing local run covers that environment; CI is configured to check Linux, macOS and Windows. See [Verification](docs/verification.md) for coverage and reports.

## Open a pull request

Describe the problem, the resulting behavior and the checks actually run. Include screenshots for visible interface changes and a small reproducer for SQL behavior when useful. Keep unrelated refactors out of the change. Use an English Conventional Commit title such as `fix: preserve custom SQL after reset`, with `feat`, `fix`, `refactor`, `chore`, `docs` or `test` as appropriate.

Submitting a contribution makes it available under the repository's [Apache-2.0 license](LICENSE). Maintainers handle version bumps, release tags and publication separately from ordinary pull requests.
