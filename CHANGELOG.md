# Changelog

User-visible changes are grouped by release. Versions link to the published release notes; the Unreleased section describes changes on `main` after the latest release.

## [Unreleased]

### Added

- Contribution, conduct and security policies, GitHub issue forms and a pull request template.
- Editor settings matching Rustfmt and Biome, with LF line endings and protection for generated captures.
- Repository and packaging checks for documentation links, ignored local files and policy files in release archives.

### Changed

- Pin workflow actions to commit SHAs, cancel superseded CI runs and require release archive uploads to contain files.
- Include community policies and the changelog in future release archives.

## [0.2.0] - 2026-10-24

### Added

- Six SQLite FK rebuild experiments covering cascade loss, PRAGMA transaction boundaries, deferral, an old WAL reader and a child save/restore control.
- Complete typed result tables in event details, portable scenarios and recordings, and a separate failing preservation assertion.
- Read-only, argument-free `PRAGMA foreign_key_check` with regression coverage.

### Fixed

- Custom SQL after a successful schedule now defaults to `SELECT 1`, including with an empty imported schema.
- The interface shows the running application version. The no-wait writer experiment explains its actual failed-write outcome.

### Changed

- Public guides and workbench screenshot updated for the full experiment catalog.
- Artifact format remains version 1; bundled SQLite remains 3.53.2.

## [0.1.1] - 2026-10-21

### Added

- Qualified macOS Intel release archives.

### Fixed

- Large-recording responsiveness through result pagination and incremental event polling.
- Concurrent refreshes are serialized while preserving reset and stale-response protection.
- Regression coverage and test budgets for large-recording round trips.

## [0.1.0] - 2026-10-21

### Added

- Initial public release with a local workbench and CLI using real SQLite connections in independent worker processes.
- Six experiments for stale snapshots, writer contention, transaction modes and WAL checkpoints.
- Typed results, event timelines, cancellation, scenario export and recording import/replay.
- Standalone Linux x86_64, macOS ARM64 and Windows x86_64 archives with bundled SQLite 3.53.2.

[Unreleased]: https://github.com/0then0/txnstage/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/0then0/txnstage/releases/tag/v0.2.0
[0.1.1]: https://github.com/0then0/txnstage/releases/tag/v0.1.1
[0.1.0]: https://github.com/0then0/txnstage/releases/tag/v0.1.0
