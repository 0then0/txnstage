# Security policy

## Supported versions

Security fixes target the latest published release and the `main` branch. Older releases do not receive separate backports. Use [Releases](https://github.com/0then0/txnstage/releases/latest) to find the current release and [CHANGELOG.md](CHANGELOG.md) for published changes.

## Report a vulnerability privately

Use [GitHub's private vulnerability reporting form](https://github.com/0then0/txnstage/security/advisories/new). Do not put exploit details, launch tokens, secrets or private data in a public issue or pull request. If the form is unavailable, contact the maintainer through a private method published on their [profile](https://github.com/0then0) to arrange a confidential channel before sending details.

Include:

- The TxnStage version, operating system and architecture; include `txnstage version` output if relevant.
- A minimal reproduction using a disposable database and synthetic data.
- Expected and actual behavior, potential impact and any conditions required to trigger it.
- Relevant logs or artifacts with secrets and launch tokens removed.

Maintainers will investigate reports and coordinate fixes and disclosure with the reporter. This volunteer project does not promise a fixed response time or paid bounty.

## Security model and scope

TxnStage creates its own temporary databases. It does not open user or production databases. The HTTP server binds to loopback, checks Host and Origin, and requires a per-launch token. Worker SQL is restricted by SQLite's authorizer; resource limits, cancellation and process isolation bound experiments. This is an application-level sandbox, not an operating-system sandbox for arbitrary native code. See [Architecture](docs/architecture.md#resources-and-local-isolation).

Report a demonstrated escape from these boundaries, unauthorized HTTP access, unexpected external file access, sensitive-data exposure or a resource-limit bypass with concrete impact. Expected SQLite errors, contention, and the documented cascade-loss experiments are ordinary lab behavior unless they expose an additional security issue.

Keep the launch URL and token private. Review imported SQL before explicitly rerunning it, use synthetic data in shared artifacts, and stop the CLI server when finished. Verify release archives against their published SHA-256 files. macOS builds have no Developer ID signature or notarization; Windows binaries are unsigned.
