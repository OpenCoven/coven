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

---

## OpenCoven Security Disclosure Addendum

## Security Policy

### Reporting a Vulnerability

If you discover a security vulnerability in OpenCoven, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

Contact the maintainers directly:
- Discord: https://discord.gg/OpenCoven (DM @BunsDev)
- Or open a GitHub Security Advisory on the repository

We will acknowledge receipt within 48 hours and aim to address confirmed vulnerabilities within 14 days.

### Scope

Security reports are welcome for:
- OpenCoven core harness and routing logic
- OpenTrust memory and session substrate
- Authentication and identity handling
- Agent sandbox and execution boundaries
- Any mechanism that could allow one agent or user to access another's context

### Out of Scope

- Issues in third-party dependencies (report to the dependency maintainer)
- Issues in model provider APIs (report to the provider)

### Our Commitment

We take security seriously because OpenCoven handles personal context and agent execution on behalf of users. We will credit researchers who responsibly disclose vulnerabilities (with their permission).

---

## Architectural Security Properties

The following properties are design goals of OpenCoven. If you find a way to violate them, that's a security report:

1. **Session isolation** — one user's agent context must not be accessible to another user or agent without explicit permission
2. **Memory ownership** — a user's stored memory and context must remain under their control
3. **Agent identity integrity** — a familiar's identity must not be forgeable by another agent or external caller
4. **Execution boundaries** — agent tool calls must not escape their intended scope

---

*Last updated: 2026-07-04*
