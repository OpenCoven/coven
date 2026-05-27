# Security Policy

Coven is an early local-first harness substrate for project-scoped coding-agent sessions.
Please treat the repository as pre-1.0 software and avoid running untrusted harnesses or prompts in sensitive repositories.

OpenClaw integration is externalized through the `@opencoven/coven` plugin. OpenClaw core is not part of Coven's trust root; the plugin should be treated as a local socket client, and the Rust daemon must continue validating launch paths, harness ids, input, and kill requests before acting.

## Reporting vulnerabilities

Please report suspected vulnerabilities privately through GitHub Security Advisories for this repository.
If advisories are unavailable, contact the maintainer privately and avoid posting exploit details in public issues.

## Local data and credentials

Coven should not require repository-stored secrets. Runtime state belongs outside source control:

- `.coven/`
- `*.sqlite`, `*.sqlite3`, `*.db`
- `*.sock`
- `.env*` files
- private keys and certificates

The CI secret guard scans both the current tree and git history for common token/key patterns without printing matched values.

## Session logs and sensitive artifacts

Coven treats session logs, prompts, harness output, tool payloads, and event history as sensitive local data. Do not place secrets in prompts or session context.

Default session event payloads are redacted before they are stored in SQLite or returned from `/events`, `/sessions/:id/events`, or `/sessions/:id/log`. Redaction covers common authorization headers, cookies, provider token shapes, private key blocks, secret-like `.env` assignments, private gateway URLs, and configured extra patterns.

Raw sensitive artifact persistence is disabled by default. If `privacy.toml` sets `persist_raw_artifacts = true` or `COVEN_PERSIST_RAW_ARTIFACTS=1`, Coven stores raw payload artifacts separately from normal logs using authenticated local encryption. The encryption key is generated under `<COVEN_HOME>/keys/session-artifacts.key` with private file permissions and is not stored in the repository or SQLite database.

The local key-file provider is an MVP for local-first encryption. It protects raw artifact rows from casual database inspection, but it is not a replacement for OS keychain-backed key management on shared or higher-risk machines.

Default retention is short for raw encrypted artifacts and bounded for operational logs:

- Raw encrypted artifacts: 7 days.
- Redacted event logs: 30 days.
- Manual pruning: `coven logs prune`.
