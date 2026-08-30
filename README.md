<p align="center">
  <img src="assets/opencoven/opencoven.svg" alt="OpenCoven logo" width="128" height="128">
</p>

# Coven

**Local harness substrate for project-scoped agent sessions**

Run Codex, Claude Code, GitHub Copilot CLI, and future coding harnesses inside explicit local project boundaries.
Launch, observe, attach, and coordinate agent work through one neutral runtime substrate.

[![MIT License](https://img.shields.io/badge/license-MIT-9A8ECD?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows%20x64-9A8ECD?style=flat-square)](https://docs.opencoven.ai/docs/guide/platforms)
[![npm](https://img.shields.io/badge/npm-%40opencoven%2Fcli-9A8ECD?style=flat-square)](https://www.npmjs.com/package/@opencoven/cli)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-9A8ECD?style=flat-square)](https://www.rust-lang.org/)

| 🌐 **Ecosystem**                                      | 💬 **Community**                            | 🛠️ **Development**                                             |
| :---------------------------------------------------- | :------------------------------------------ | :------------------------------------------------------------- |
| [**Website**](https://opencoven.ai/)                  | [**Discord**](https://discord.gg/opencoven) | [**GitHub Issues**](https://github.com/OpenCoven/coven/issues) |
| [**Documentation**](https://docs.opencoven.ai/)       | [**X (\@OpenCvn)**](https://x.com/OpenCvn)  | [**Public Roadmap**](docs/ROADMAP.md)                          |
| [**Submit Feedback**](https://feedback.opencoven.ai/) |                                             | [**Contributing**](CONTRIBUTING.md)                            |

---

> **⚠️ Early MVP** — Coven is a local-first runtime in active development. It
> is usable by adventurous developers on macOS, Linux, and Windows x64. The
> npm package is live. Expect rough edges.
>
> **External PRs are open** — Start from an issue for larger changes, keep PRs scoped, and include the readiness packet requested by the PR template.

---

## What is Coven?

Coven is the local harness substrate for the [OpenCoven](https://github.com/OpenCoven) ecosystem. It gives coding-agent CLIs like [Codex](https://github.com/openai/codex) and [Claude Code](https://docs.anthropic.com/en/docs/claude-code) a shared room where project work can happen visibly and safely.

> **One project. Any harness. Visible work.**

- **You choose the harness** — Codex, Claude Code, GitHub Copilot CLI, or future adapters.
- **Coven owns the session** — project-scoped boundaries, PTY execution, event logging, SQLite persistence.
- **Clients present the work** — CastCodes, the CLI/TUI, comux, or your own integration over the same-user local IPC API.

The Rust daemon is the authority boundary. All clients — including the CLI itself — are convenience layers. Security decisions flow inward to the daemon, never outward to clients. OpenClaw integrates only through the opt-in `@opencoven/coven` plugin in `packages/openclaw-coven`; OpenClaw core contains no Coven code.

---

## Install

```bash
npm install -g @opencoven/cli
coven doctor
```

| Package                    | Platform                                       |
| -------------------------- | ---------------------------------------------- |
| `@opencoven/cli`           | Universal wrapper — auto-selects your platform |
| `@opencoven/cli-macos`     | macOS Apple Silicon                            |
| `@opencoven/cli-macos-x64` | macOS Intel x64                                |
| `@opencoven/cli-linux-x64` | glibc-based Linux x64 (Alpine unsupported)    |
| `@opencoven/cli-windows`   | Windows x64                                    |

The memory dashboard is an opt-in companion installed separately with
`npm install -g @opencoven/coven-memory-dashboard` (Node.js 24+); see
[`docs/reference/cli-observe.md`](docs/reference/cli-observe.md).

Install routes (npm, cargo, source), platform behavior, service managers, and
containers are documented at **https://docs.opencoven.ai/docs/guide/install**.

---

## Quick start

```bash
cd /path/to/your/project

# 1. Complete provider-owned login
coven setup codex

# 2. Check local readiness
coven doctor

# 3. Start the daemon
coven daemon start

# 4. Launch a session
coven run codex "fix the failing tests"

# 5. Browse and manage sessions
coven sessions

# 6. Stop the daemon when done
coven daemon stop
```

Bare `coven` opens the interactive Coven UI instead — see
[Interactive UI](https://docs.opencoven.ai/docs/cli/interactive). The command
reference lives at [docs.opencoven.ai/docs/cli](https://docs.opencoven.ai/docs/cli).

---

## Documentation

Public installation, CLI, daemon, harness, API, memory, and troubleshooting
documentation is canonical at **[docs.opencoven.ai](https://docs.opencoven.ai/)**.
Start with:

- [Getting started](https://docs.opencoven.ai/docs/guide/getting-started)
- [CLI reference](https://docs.opencoven.ai/docs/cli)
- [Daemon](https://docs.opencoven.ai/docs/daemon)
- [Harnesses](https://docs.opencoven.ai/docs/harnesses)
- [Local API](https://docs.opencoven.ai/docs/reference/api)
- [Memory](https://docs.opencoven.ai/docs/memory-models)
- [Troubleshooting](https://docs.opencoven.ai/docs/reference/troubleshooting) — or run `coven doctor` first

This repository keeps only documentation that must evolve with the source:

| Local document | Why it remains here |
| --- | --- |
| [API contract](docs/API-CONTRACT.md) | Normative `coven.daemon.v1` request, response, error, and compatibility contract |
| [Architecture](docs/ARCHITECTURE.md) | Source-adjacent crate ownership, authority boundaries, and dependency direction |
| [Session lifecycle](docs/SESSION-LIFECYCLE.md) | Normative state-machine and persistence behavior |
| [Harness adapter contract](docs/HARNESS-ADAPTERS.md) | Maintainer and adapter-author implementation contract |
| [CLI core functionality](docs/development/cli-core-functionality.md) | Maintainer source map and verification loop |
| [Documentation maintenance](docs/DOCS-MAINTENANCE.md) | Ownership rules for public docs versus repository contracts |
| [Security policy](SECURITY.md) | Vulnerability reporting and repository security policy |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the first-10-minutes checkout path,
the full local development loop, and the PR readiness packet. The short rules:

- **Rust is the authority layer.** Launch, cwd validation, PTY lifecycle, session persistence, and IPC enforcement are Rust's responsibility; clients are never the trust boundary.
- **Keep harness support focused** on Codex, Claude Code, and GitHub Copilot CLI until adapter contracts are stable.
- **Run `python scripts/check-secrets.py` before every PR**, including docs-only changes, and never commit runtime state (`.coven/`, `*.sqlite*`, `*.sock`, `.env*`, `*.key`).

Performance baselines collect trend data without gating merges:

```bash
cargo build -p coven-cli --locked
node scripts/benchmark-cli.mjs --binary target/debug/coven --iterations 3 --output /tmp/coven-perf.json
node scripts/benchmark-chaos.mjs --binary target/debug/coven --output /tmp/coven-chaos.json
```

Both use disposable `COVEN_HOME` directories and a fixture-only fake harness; `scripts/benchmark-chaos.test.mjs` gates deterministically in CI.

---

## Security

Coven is pre-1.0 software. Treat it accordingly:

- **Do not run untrusted harnesses or prompts in sensitive repositories.** Session logs capture harness output; if the harness dumps secrets, Coven logs them.
- **Do not paste secrets into prompts.** Event payloads are redacted before API display, but defense in depth starts with not having secrets in prompts.

**Reporting vulnerabilities:** Please use [GitHub Security Advisories](https://github.com/OpenCoven/coven/security/advisories) for this repository. See [SECURITY.md](SECURITY.md) for the policy and [Safety](https://docs.opencoven.ai/docs/reference/safety) for the public trust boundary.

---

## Roadmap

The milestone ledger lives in [`docs/ROADMAP.md`](docs/ROADMAP.md); items move when they are designed, implemented, tested, and released.

---

## License

MIT © Valentina Alexander and the OpenCoven contributors — see [LICENSE](LICENSE) for full terms.

---

<div align="center">

**[OpenCoven](https://github.com/OpenCoven)** — One project. Any harness. Visible work.

</div>
