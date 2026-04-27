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
