# Security Policy

Coven is an early local-first harness substrate for project-scoped coding-agent sessions.
Please treat the repository as pre-1.0 software and avoid running untrusted harnesses or prompts in sensitive repositories.

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
