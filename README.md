<p align="center">
  <img src="assets/opencoven/opencoven.svg" alt="OpenCoven logo" width="128" height="128">
</p>

# Coven

**Local harness substrate for project-scoped agent sessions**

Run Codex, Claude Code, and future coding harnesses inside explicit local project boundaries.
Launch, observe, attach, and coordinate agent work through one neutral runtime substrate.

[![MIT License](https://img.shields.io/badge/license-MIT-9A8ECD?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-9A8ECD?style=flat-square)](#requirements)
[![npm](https://img.shields.io/badge/npm-%40opencoven%2Fcli-9A8ECD?style=flat-square)](https://www.npmjs.com/package/@opencoven/cli)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-9A8ECD?style=flat-square)](https://www.rust-lang.org/)

| 🌐 **Ecosystem**                                      | 💬 **Community**                            | 🛠️ **Development**                                             |
| :---------------------------------------------------- | :------------------------------------------ | :------------------------------------------------------------- |
| [**Website**](https://opencoven.ai/)                  | [**Discord**](https://discord.gg/opencoven) | [**GitHub Issues**](https://github.com/OpenCoven/coven/issues) |
| [**Documentation**](https://docs.opencoven.ai/)       | [**X (\@OpenCvn)**](https://x.com/OpenCvn)  | [**Public Roadmap**](docs/ROADMAP.md)                          |
| [**Submit Feedback**](https://feedback.opencoven.ai/) |                                             | [**Contributing**](CONTRIBUTING.md)                            |

---

> **⚠️ Early MVP** — Coven is a local-first runtime in active development. It is usable by adventurous developers on macOS and Linux. The npm package is live. Expect rough edges.
>
> **External PRs are open** — Start from an issue for larger changes, keep PRs scoped, and include the readiness packet requested by the PR template.

---

## Table of Contents

- [What is Coven?](#what-is-coven)
- [Why Coven?](#why-coven)
- [Features](#features)
- [Requirements](#requirements)
- [Install](#install)
- [Quick Start](#quick-start)
- [Commands Reference](#commands-reference)
- [Local API](#local-api)
- [Architecture](#architecture)
- [Repository Structure](#repository-structure)
- [Configuration](#configuration)
- [OpenCoven Integrations](#opencoven-integrations)
- [Documentation](#documentation)
- [FAQ](#faq)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [Code of Conduct](#code-of-conduct)
- [Roadmap](#roadmap)
- [Security](#security)
- [License](#license)
- [Community & Support](#community--support)

---

## What is Coven?

Coven is the local harness substrate for the [OpenCoven](https://github.com/OpenCoven) ecosystem. It gives coding-agent CLIs like [Codex](https://github.com/openai/codex) and [Claude Code](https://docs.anthropic.com/en/docs/claude-code) a shared room where project work can happen visibly and safely.

> **One project. Any harness. Visible work.**

Coven doesn't replace your coding agent, your UI, or other clients. It acts as a neutral runtime layer:

- **You choose the harness** — Codex, Claude Code, or future adapters.
- **Coven owns the session** — project-scoped boundaries, PTY execution, event logging, SQLite persistence.
- **Clients present the work** — CastCodes, the CLI/TUI, comux, or your own integration over the local socket API.

The Rust daemon is the authority boundary. All clients — including the CLI itself — are convenience layers. Security decisions flow inward to the daemon, never outward to clients.

---

## Why Coven?

| Without Coven                                       | With Coven                                                  |
| --------------------------------------------------- | ----------------------------------------------------------- |
| Run `codex` directly; no persistent session history | Every run creates a session record with metadata and events |
| No project boundary enforcement                     | Agent is locked to an explicit project root; cannot escape  |
| Lose track of agent work when the terminal closes   | Sessions persist across daemon restarts via SQLite          |
| Manually juggle multiple harness CLIs               | One unified `coven run` entry point for all harnesses       |
| No API for clients to consume agent sessions        | Versioned `coven.daemon.v1` socket API for all clients      |
| No standard way to observe or replay past work      | `coven sessions` browser with Rejoin, View Log, and Archive |

---

## Features

- **🏠 Project-root boundaries** — Every launch is tied to an explicit repository or project root. The daemon rejects working directories that escape the declared boundary.
- **🔌 Harness-neutral runtime** — v0 focuses on Codex and Claude Code with a clean adapter path for future harnesses (Hermes, Aider, Gemini, and user-defined CLIs).
- **🖥️ Interactive session browser** — Live and completed work can be selected, rejoined, viewed, archived, restored, or sacrificed without memorizing IDs.
- **📡 Attachable PTY sessions** — Live sessions can be replayed or followed from explicit CLI verbs.
- **🔌 Local daemon API** — CastCodes, comux, and the OpenClaw plugin coordinate through one versioned socket contract (`coven.daemon.v1`).
- **🗄️ SQLite-backed history** — Session metadata and event logs survive daemon restarts.
- **🦀 Rust authority layer** — Launch, cwd, input, kill, and path-sensitive requests are revalidated in Rust. Clients are never the trust boundary.
- **🔒 External OpenClaw bridge** — `@opencoven/coven` is an opt-in plugin; OpenClaw core does not include Coven code.
- **📦 @opencoven namespace** — CLI wrapper packages live under `@opencoven/*`; the user-facing command is always `coven`.
- **🩺 System diagnostics** — `coven pc` (macOS-first) surfaces CPU, memory, disk, and process health without launching a harness.

---

## Requirements

| Requirement                  | Notes                                                                       |
| ---------------------------- | --------------------------------------------------------------------------- |
| **Rust stable toolchain**    | Required only when building from source                                     |
| **Git**                      | Required                                                                    |
| **macOS, Linux, or Windows x64** | Native npm packages are published for all three platforms                  |
| **Node.js 18+**              | Required only for npm wrapper or package/plugin development                 |
| **At least one harness CLI** | Codex and/or Claude Code (see below)                                        |

### Installing harness CLIs

Run `coven doctor` first — it prints specific install hints for any missing harness.

**Codex (OpenAI):**

```bash
npm install -g @openai/codex
# or: brew install --cask codex
codex login
```

**Claude Code (Anthropic):**

```bash
npm install -g @anthropic-ai/claude-code
claude doctor
```

After installing and authenticating, run `coven doctor` again to confirm the harness is detected. If `doctor` still reports missing, ensure the harness binary is on your `PATH`.

---

## Install

Coven is available as an npm wrapper for the fastest install, or you can build from source.

### npm (recommended)

Install globally:

```bash
npm install -g @opencoven/cli
coven doctor
```

**Available npm packages:**

| Package                    | Platform                                       |
| -------------------------- | ---------------------------------------------- |
| `@opencoven/cli`           | Universal wrapper — auto-selects your platform |
| `@opencoven/cli-macos`     | macOS Apple Silicon                            |
| `@opencoven/cli-linux-x64` | Linux x64                                      |
| `@opencoven/cli-windows`   | Windows x64                                    |

### Build from source (recommended for contributors)

```bash
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
```

> **Note:** Building from source requires Rust stable. See [Requirements](#requirements).

---

## Quick Start

### Option A — Interactive UI (recommended for new users)

```bash
cd /path/to/your/project
coven
# or explicitly:
coven chat
```

Bare `coven` opens the interactive Coven UI, powered by the **Coven engine**.
The first time you run it, `coven` offers to download and install the engine
automatically (or install it anytime with `coven engine install`); it's a
version-pinned, checksum-verified binary that `coven` manages for you — there's
no separate package to install. You can also pass a task directly —
`coven "fix the failing tests"` — and Coven will show a plan card and run it in
a recorded session.

### Option B — Direct commands

```bash
cd /path/to/your/project

# 1. Verify setup
coven doctor

# 2. Start the daemon
coven daemon start

# 3. Launch a session
coven run codex "fix the failing tests"
# or with Claude Code:
coven run claude "polish this UI"

# 4. Browse and manage sessions
coven sessions

# 5. Stop the daemon when done
coven daemon stop
```

### Option C — OpenClaw rescue loop

If OpenClaw breaks, Coven provides a predictable repair room:

```bash
coven patch openclaw
```

Choose a repo, choose a harness, get a verified patch.

---

## Commands Reference

### Core commands

| Command          | Action                                                          |
| ---------------- | --------------------------------------------------------------- |
| `coven`          | Open the interactive Coven UI (engine auto-installed on first run) |
| `coven "<task>"` | Plan and run a free-text task in a recorded session (Cast flow) |
| `coven chat`     | Explicitly open the interactive Coven UI                        |
| `coven tui`      | Same as `coven chat`                                            |
| `coven doctor`   | Detect supported harness CLIs and print install hints           |

### Harness adapters

| Command                       | Action                                        |
| ----------------------------- | --------------------------------------------- |
| `coven adapter list`          | List configured harness adapters              |
| `coven adapter list --json`   | Adapter reports as JSON                       |
| `coven adapter doctor [id]`   | Diagnose all adapters, or one adapter id      |
| `coven adapter install <id>`  | Install a trusted local adapter recipe        |

### Daemon lifecycle

| Command                | Action                                         |
| ---------------------- | ---------------------------------------------- |
| `coven daemon start`   | Start the local Coven daemon                   |
| `coven daemon status`  | Show daemon health, PID, and socket path       |
| `coven daemon restart` | Restart the local daemon and rebind the socket |
| `coven daemon stop`    | Stop the local daemon                          |

### Session management

| Command                                        | Action                                                           |
| ---------------------------------------------- | ---------------------------------------------------------------- |
| `coven run <harness> <prompt>`                 | Launch a project-scoped harness session                          |
| `coven run <harness> <prompt> --cwd <path>`    | Launch from a cwd inside the project root                        |
| `coven run <harness> <prompt> --title <title>` | Set a readable session title                                     |
| `coven run <harness> <prompt> --model <id>`    | Forward a model override to the harness                          |
| `coven run <harness> <prompt> --think`         | Request deeper reasoning when the harness supports it            |
| `coven run <harness> <prompt> --speed <level>` | Set a latency/reasoning hint: `fast`, `balanced`, or `thorough`  |
| `coven run <harness> --continue`               | Resume the latest active session for this project                |
| `coven run <harness> --continue <id>`          | Resume a specific session by id                                  |
| `coven run <harness> <prompt> --detach`        | Create the session record without launching the harness          |
| `coven run <harness> <prompt> --labels <l>`    | Attach comma-separated labels to the session                     |
| `coven run <harness> <prompt> --visibility <v>`| Set visibility: `private` (default), `workspace`, or `shared`   |
| `coven run <harness> <prompt> --archive`       | Auto-archive the session when the run completes                  |
| `coven run <harness> <prompt> --familiar <id>` | Inject a familiar identity preamble (e.g. `--familiar charm`)    |
| `coven run <harness> <prompt> --stream-json`   | Emit JSONL events on stdout (for tooling/piping)                 |
| `coven run claude <p> --stream-json --stream-json-input` | Also read JSONL user messages from stdin             |
| `coven sessions`                               | Open the session browser in a terminal; print a table when piped |
| `coven sessions --all`                         | Browse active and archived sessions; print all when piped        |
| `coven sessions --manage`                      | Force the interactive session browser                            |
| `coven sessions --plain`                       | Force plain table output for scripts or copying                  |
| `coven sessions --json`                        | Output sessions as JSON                                          |
| `coven sessions --json --all`                  | Output all sessions (including archived) as JSON                 |
| `coven attach <session-id>`                    | Replay/follow session output and forward input                   |
| `coven summon <session-id>`                    | Restore an archived session, then replay/follow it               |
| `coven archive <session-id>`                   | Hide a non-running session while preserving its events           |
| `coven sacrifice <session-id> --yes`           | Permanently delete a non-running session and its events          |

> `attach`, `summon`, `archive`, and `sacrifice` accept a unique prefix of the
> session id (e.g. `coven attach 9099`), so you don't have to paste full UUIDs.

> **Session rituals are intentionally explicit.** Archive is reversible and keeps the full event ledger. Summon brings an archived session back. Sacrifice is destructive, refuses live sessions, and requires `--yes` so beginners don't delete work by accident.

| Ritual        | Reversible? | Works on             | Description                                                  |
| ------------- | ----------- | -------------------- | ------------------------------------------------------------ |
| **Archive**   | ✅ Yes      | Non-running sessions | Hides from active list; all events preserved                 |
| **Summon**    | N/A         | Archived sessions    | Restores to active list                                      |
| **Sacrifice** | ❌ No       | Non-running sessions | Permanently deletes session and all events; requires `--yes` |
| **Rejoin**    | N/A         | Live sessions        | Reattaches to running session                                |

### System diagnostics (`coven pc`, macOS-first)

| Command                          | Action                                                                |
| -------------------------------- | --------------------------------------------------------------------- |
| `coven pc`                       | Full system report: CPU, memory, disk, top processes                  |
| `coven pc status`                | One-line health summary with 🟢/🟡/🔴 indicators                      |
| `coven pc status --json`         | Machine-readable health summary                                       |
| `coven pc top --n 10`            | Top-N processes by CPU usage                                          |
| `coven pc disk`                  | Disk usage breakdown                                                  |
| `coven pc kill <pid> --confirm`  | SIGTERM with PID identity re-check (requires `--confirm`)             |
| `coven pc cache clear --confirm` | Clear `~/Library/Caches` and `/Library/Caches` (requires `--confirm`) |

> All read operations are side-effect-free. Write operations (kill, cache clear) require `--confirm` and cannot be bypassed. Termination is SIGTERM only — no SIGKILL.

### Parallel work protocol (worktrees, claims, hooks)

| Command                     | Action                                                        |
| --------------------------- | ------------------------------------------------------------- |
| `coven wt <branch>`         | Create or enter a worktree in the sibling `<repo>.wt` dir     |
| `coven wt --list`           | List worktrees with claim and dirty state                     |
| `coven wt --doctor`         | Report protocol layout and hook issues                        |
| `coven wt --prune-merged`   | Remove clean worktrees whose branches are merged              |
| `coven claim acquire <b>`   | Claim a branch for the current agent (TTL-bounded)            |
| `coven claim status`        | Show active and expired claims for this repository            |
| `coven hooks install`       | Install the protocol's pre-commit and pre-push git hooks      |

### Other

| Command                | Action                                                  |
| ---------------------- | ------------------------------------------------------- |
| `coven patch openclaw` | Open the OpenClaw repair rescue loop                    |
| `coven logs prune`     | Manually prune session logs and raw encrypted artifacts |

---

## Local API

The daemon exposes a versioned HTTP API over a Unix socket. The current public contract is `coven.daemon.v1` (prefix: `/api/v1`).

### Endpoint reference

| Endpoint                                 | Method | Purpose                                                     |
| ---------------------------------------- | ------ | ----------------------------------------------------------- |
| `/api/v1/health`                         | `GET`  | Daemon health, API version, and capability catalog          |
| `/api/v1/api-version`                    | `GET`  | Active and supported API versions                           |
| `/api/v1/capabilities`                   | `GET`  | Machine-readable capability catalog for clients             |
| `/api/v1/sessions`                       | `GET`  | List sessions                                               |
| `/api/v1/sessions`                       | `POST` | Launch a session                                            |
| `/api/v1/sessions/:id`                   | `GET`  | Fetch one session                                           |
| `/api/v1/events`                         | `GET`  | Read session events (supports `afterSeq` cursor pagination) |
| `/api/v1/sessions/:id/events`            | `GET`  | Session-scoped events alias                                 |
| `/api/v1/sessions/:id/log`               | `GET`  | Redacted log preview for a session                          |
| `/api/v1/sessions/:id/input`             | `POST` | Forward input to a live session                             |
| `/api/v1/sessions/:id/kill`              | `POST` | Kill a live session                                         |
| `/api/v1/actions`                        | `POST` | Route a control-plane action (advanced clients)             |
| `/api/v1/travel/profiles`                | `POST` | Generate a read-only travel profile for offline laptop work |
| `/api/v1/travel/deltas`                  | `POST` | Upload offline travel results for hub reconciliation        |
| `/api/v1/travel/state`                   | `GET`  | Read hub/travel handoff state for a client                  |
| `/api/v1/scheduler/decisions`            | `POST` | Choose a node for a multi-host job                          |
| `/api/v1/scheduler/decisions/:id`        | `GET`  | Fetch a persisted scheduler decision                        |
| `/api/v1/scheduler/redispatch`           | `POST` | Redispatch or pause a loop after executor failure           |
| `/api/v1/scheduler/loops/:loopId`        | `GET`  | Recover persisted scheduler loop state                      |

### Recommended client handshake

All API clients should start with a health negotiation:

```bash
# Example: health check via Unix socket
curl --unix-socket ~/.coven/coven.sock http://localhost/api/v1/health
```

Example response:

```json
{
  "ok": true,
  "apiVersion": "coven.daemon.v1",
  "covenVersion": "0.0.10",
  "capabilities": {
    "sessions": true,
    "events": true,
    "travel": true,
    "scheduler": true,
    "eventCursor": "sequence",
    "structuredErrors": true
  },
  "daemon": {
    "pid": 12345,
    "startedAt": "2026-05-09T06:43:00Z",
    "socket": "/Users/alice/.coven/coven.sock"
  }
}
```

**Before depending on any other endpoint:**

1. Call `GET /api/v1/health`
2. Verify `apiVersion === "coven.daemon.v1"` and `capabilities.structuredErrors === true`
3. Check `capabilities.eventCursor === "sequence"` before using `afterSeq` pagination
4. Only then depend on the documented `v1` sessions/events shapes

All API errors use a structured envelope. Branch on `error.code`, never on `error.message`:

```json
{
  "error": {
    "code": "session_not_found",
    "message": "Session was not found.",
    "details": { "sessionId": "abc-123" }
  }
}
```

Treat the socket API as the product contract. Clients may validate for better UX, but the Rust daemon remains the authority boundary. See [`docs/API-CONTRACT.md`](docs/API-CONTRACT.md) for the full versioned contract including error codes, cursor pagination, session shapes, and compatibility rules.

---

## Architecture

### Runtime topology

Coven is a local-first harness substrate. The Rust daemon is the authority boundary. All clients — including the CLI/TUI — are untrusted for enforcement purposes.

```
Developer
  │
  ├── CastCodes workspace ─────────────────────┐
  ├── coven CLI / TUI ─────────────────────────┤ HTTP over Unix socket
  ├── comux (legacy/reference) ────────────────┤ ~/.coven/coven.sock
  └── @opencoven/coven (OpenClaw plugin) ───────┘
                                                │
                                ┌───────────────▼──────────────────┐
                                │         Coven Rust Daemon         │
                                │                                   │
                                │  ┌───────────────────────────┐   │
                                │  │    Authority boundary      │   │
                                │  │  • Canonicalize project root│  │
                                │  │  • Validate cwd in root   │   │
                                │  │  • Allowlist harness id   │   │
                                │  │  • Validate session state │   │
                                │  │  • Route action via policy │  │
                                │  └────────────┬──────────────┘   │
                                │               │                   │
                                │  ┌────────────▼─────────────┐    │
                                │  │   Harness adapter router  │    │
                                │  └───────┬──────────────┬───┘    │
                                │          │              │          │
                                │      ┌───▼──┐      ┌───▼───┐     │
                                │      │Codex │      │Claude │     │
                                │      │ PTY  │      │  PTY  │     │
                                │      └───┬──┘      └───┬───┘     │
                                │          │              │          │
                                │  ┌───────▼──────────────▼──────┐  │
                                │  │  SQLite session ledger +     │  │
                                │  │  append-only event log       │  │
                                │  └──────────────────────────────┘  │
                                └───────────────────────────────────┘
```

### Authority boundary

The Rust daemon validates every request before acting:

1. `projectRoot` must be explicit — no fallback
2. `cwd` must canonicalize inside the declared project root
3. Harness ID must be allowlisted (`codex`, `claude`)
4. Session IDs must exist and be in the expected state
5. All harness commands are built with argv APIs — never `sh -c`

Clients may improve UX by validating early, but they are never the enforcement boundary. A client cannot widen the project boundary, bypass the harness allowlist, or escape session state validation.

### Session lifecycle

```
coven run codex "fix tests"
        │
        ▼
POST /api/v1/sessions  { projectRoot, cwd, harness, prompt }
        │
        ▼
Daemon: canonicalize → validate → spawn or reject
        │
        ▼
Session record created in SQLite
        │
        ▼
Harness spawned in PTY → output events streamed to SQLite
        │
        ▼
coven sessions → Rejoin / View Log / Archive / Sacrifice
```

For full architecture diagrams (including Mermaid flow charts), see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Repository Structure

```
coven/
├── .github/                    # GitHub Actions workflows, issue templates
├── assets/opencoven/           # Project assets (logos, icons for npm packages)
├── brand/                      # OpenCoven brand system
│   ├── icons/                  # Brand icon set (trident, agent-node, etc.)
│   ├── social/                 # Social media assets (X, GitHub)
│   └── ui/                     # CSS color tokens and typography scale
├── crates/
│   ├── coven-cli/              # Main Rust binary — the `coven` command
│   └── coven-relay/            # Internal relay crate
├── docs/                       # Full documentation suite (see Documentation section)
├── npm/                        # npm wrapper package source for @opencoven/cli
├── packages/
│   └── openclaw-coven/         # External OpenClaw bridge plugin (@opencoven/coven)
├── scripts/
│   └── check-secrets.py        # CI / pre-release secret scanner
├── skills/opencoven-design/    # Design skill files
├── web/                        # Web surface files
├── Cargo.lock                  # Locked Rust dependency tree
├── Cargo.toml                  # Rust workspace manifest
├── CONTRIBUTING.md             # Contribution guidelines
├── DESIGN.md                   # Full brand and design system reference
├── LICENSE                     # MIT license
├── README.md                   # This file
└── SECURITY.md                 # Security policy
```

**Key directories:**

- **`crates/coven-cli`** — Everything that becomes the `coven` binary. This is where daemon, PTY adapter, session store, socket API, and CLI surface live in Rust.
- **`packages/openclaw-coven`** — The opt-in bridge between OpenClaw and Coven. Lives here (not in OpenClaw core) to keep the trust boundary clean. Published as `@opencoven/coven`.
- **`scripts/check-secrets.py`** — Required pre-release and pre-PR scan. Run it before pushing to avoid leaking credentials into git history.
- **`docs/`** — The canonical documentation suite. All product docs, architecture, API contract, safety model, and roadmap live here.

---

## Configuration

### Environment variables

| Variable                      | Default    | Description                                                                              |
| ----------------------------- | ---------- | ---------------------------------------------------------------------------------------- |
| `COVEN_HOME`                  | `~/.coven` | Root directory for all daemon state: SQLite database, Unix socket, logs, encryption keys |
| `COVEN_PERSIST_RAW_ARTIFACTS` | `0`        | Set to `1` to enable local encrypted storage of raw session artifact payloads (advanced) |

> **Tip:** If you run Coven in CI or need isolated environments, set `COVEN_HOME` to a unique path per environment. Coven will create the directory if it doesn't exist.

### Configuration file

Local privacy and storage settings live in `<COVEN_HOME>/privacy.toml`:

| Key                     | Default | Description                                             |
| ----------------------- | ------- | ------------------------------------------------------- |
| `persist_raw_artifacts` | `false` | Enable local encrypted storage of raw session artifacts |

When enabled, raw artifacts are encrypted using a local key at `<COVEN_HOME>/keys/session-artifacts.key`. This file is generated automatically with private permissions and is never stored in the repository or SQLite database.

### What should never enter your repository

```
.coven/          # Daemon state directory — local only
*.sqlite         # SQLite database files
*.sqlite3
*.db
*.sock           # Unix socket files
.env*            # Environment variable files
*.key            # Encryption key files
```

These patterns are covered by `.gitignore`. Before submitting any PR or publishing documentation, run the secret scanner:

```bash
python scripts/check-secrets.py
```

If the scan fails, remove the secret from your working tree. If a secret entered git history, rotate the credential before rewriting history or publishing.

### Data retention defaults

| Data type                            | Default retention | Manual control     |
| ------------------------------------ | ----------------- | ------------------ |
| Redacted session event logs          | 30 days           | `coven logs prune` |
| Raw encrypted artifacts (if enabled) | 7 days            | `coven logs prune` |

---

## OpenCoven Integrations

Coven is the runtime layer. Other surfaces in the OpenCoven ecosystem sit above it and connect through the local socket API.

| Integration                                              | Role                                                                       | How it connects                          |
| -------------------------------------------------------- | -------------------------------------------------------------------------- | ---------------------------------------- |
| **[CastCodes](https://github.com/OpenCoven/cast-codes)** | Primary public workspace; the local-first AI coding product built on Coven | HTTP over Unix socket                    |
| **comux**                                                | Legacy terminal cockpit (useful reference; not the future public story)    | HTTP over Unix socket                    |
| **OpenClaw**                                             | External coding agent; integrates via opt-in plugin only                   | `@opencoven/coven` plugin → socket       |

> **Important:** OpenClaw core does not contain Coven code. The integration lives exclusively in `packages/openclaw-coven` and publishes as `@opencoven/coven`. This separation keeps the trust boundary clean — the plugin is treated as an untrusted socket client, and the Rust daemon revalidates every request it makes.

### CastCodes

CastCodes is the primary product users open: terminal/code workspace, visible agent lanes, review flows, and approval UX. It is the first-contact public story for Coven.

The intended flow is:

```
User → CastCodes → coven run → Coven daemon → Harness PTY
Harness output → Coven event log → CastCodes session view
```

### comux (legacy reference)

comux is a standalone terminal cockpit that proved the tmux-cockpit model for parallel agent work. Its useful primitives (worktree isolation, pane menus, agent launcher registry) are being folded into CastCodes-native concepts. comux is no longer the future-facing public surface.

---

## Documentation

| Document                                                | What it covers                                                        |
| ------------------------------------------------------- | --------------------------------------------------------------------- |
| [Getting started](docs/GETTING-STARTED.md)              | Full install → first session walkthrough                              |
| [Concepts](docs/CONCEPTS.md)                            | Definitions: harness, session, project, ritual, daemon, store, client |
| [Glossary](docs/GLOSSARY.md)                            | Term definitions for the full OpenCoven ecosystem                     |
| [Architecture](docs/ARCHITECTURE.md)                    | Runtime topology, session lifecycle, authority boundary diagrams      |
| [API contract](docs/API-CONTRACT.md)                    | Full `coven.daemon.v1` contract: shapes, cursors, error codes         |
| [Session lifecycle](docs/SESSION-LIFECYCLE.md)          | Detailed state machine for sessions                                   |
| [Safety model](docs/SAFETY-MODEL.md)                    | Trust boundary, local access model, data rules                        |
| [Operational model](docs/OPERATIONAL-MODEL.md)          | Day-to-day operation and daemon management                            |
| [Client integration guide](docs/CLIENT-INTEGRATION.md)  | How to build a client against the socket API                          |
| [Harness adapter guide](docs/HARNESS-ADAPTERS.md)       | How to implement a new harness adapter                                |
| [Troubleshooting](docs/TROUBLESHOOTING.md)              | Diagnose and resolve common issues                                    |
| [Public roadmap](docs/ROADMAP.md)                       | Shipped, now, next, and later milestones                              |
| [Product spec](docs/PRODUCT-SPEC.md)                    | Product requirements and design decisions                             |
| [MVP plan](docs/MVP-PLAN.md)                            | Current MVP scope and checklist                                       |
| [Future harnesses](docs/FUTURE-HARNESSES.md)            | Research notes for upcoming adapter work                              |
| [Brand assets](docs/BRAND.md)                           | Logo, colors, and usage guidance                                      |
| [Design system](DESIGN.md)                              | Full brand reference: palette, typography, iconography                |
| [Brand adherence checklist](docs/BRANDING-ADHERENCE.md) | Checklist for brand-compliant contributions                           |
| [Documentation maintenance](docs/DOCS-MAINTENANCE.md)   | How docs are organized and kept up to date                            |
| [Security policy](SECURITY.md)                          | Vulnerability reporting and data handling                             |

---

## FAQ

**Q: What is Coven, exactly?**

Coven is a local Rust daemon and CLI that supervises coding-agent CLI sessions (like Codex or Claude Code) inside explicit project boundaries, records everything to SQLite, and exposes it all through a versioned local HTTP API over a Unix socket.

**Q: Does Coven replace Codex or Claude Code?**

No. Coven wraps them. You still use the harness CLI for its AI capabilities — Coven adds project-scoped boundaries, session persistence, and a unified API on top.

**Q: Does Coven require an internet connection or an account?**

No. Coven itself is fully local. Your harness CLIs (Codex, Claude Code) require their own provider authentication, but Coven stores no credentials and makes no outbound network calls.

**Q: Is Windows supported?**

Yes. `@opencoven/cli-windows` ships a native Windows x64 binary, and the universal `@opencoven/cli` wrapper selects it automatically. Run `coven doctor` from the same PowerShell, Windows Terminal, or WSL2 environment where your harness CLI is installed.

**Q: What is `coven pc`?**

A macOS-first system diagnostics and relief tool built into the CLI. It shows CPU, memory, disk, and process health without launching a harness — useful when sessions feel slow or the daemon is sluggish to start. All read operations are side-effect-free. Write operations (kill, cache clear) require an explicit `--confirm` flag and cannot be bypassed.

**Q: What does "Sacrifice" mean?**

Sacrifice is Coven's intentionally explicit verb for permanently deleting a session and all its event history. It requires `--yes` on the command line so beginners don't accidentally delete work. Archive + Summon are the reversible alternatives for non-destructive session management.

**Q: What is `COVEN_HOME`?**

The directory where Coven stores all local state: SQLite database, Unix socket, logs, and encryption keys. Defaults to `~/.coven`. To isolate environments (e.g., in CI), set `COVEN_HOME` to a separate path for each environment.

**Q: Is CastCodes the same as Coven?**

No. CastCodes is a separate product — the local-first AI coding workspace and primary public-facing product that runs on top of Coven. Coven is the runtime substrate. CastCodes is the workspace you open.

**Q: What is the relationship with OpenClaw?**

OpenClaw is an external coding agent that can optionally integrate with Coven through the `@opencoven/coven` plugin package. OpenClaw core contains no Coven code. The integration is strictly opt-in and requires installing the plugin separately.

**Q: Can I build my own client on top of Coven?**

Yes. The daemon exposes a stable `coven.daemon.v1` HTTP API over a local Unix socket. All clients are untrusted for enforcement, but the API surface is stable and versioned. See [`docs/API-CONTRACT.md`](docs/API-CONTRACT.md) and [`docs/CLIENT-INTEGRATION.md`](docs/CLIENT-INTEGRATION.md).

**Q: What if I want to add a new harness (like Aider or Gemini)?**

See [`docs/HARNESS-ADAPTERS.md`](docs/HARNESS-ADAPTERS.md) for the adapter contract. The v0 focus is Codex and Claude Code — new harnesses are planned for later milestones after adapter contracts are stable.

---

## Troubleshooting

The fastest first step for any broken setup:

```bash
coven doctor
```

`coven doctor` checks store readiness, project detection, daemon status, and harness availability — and prints specific next steps for every failure branch.

### Quick reference

| Symptom                                     | First step                                                                             |
| ------------------------------------------- | -------------------------------------------------------------------------------------- |
| `coven: command not found`                  | Run `npm install -g @opencoven/cli`; verify binary is on `PATH`                        |
| `doctor` reports missing harness            | Install and authenticate the harness CLI (see [Requirements](#requirements))           |
| Daemon won't start                          | Run `coven daemon restart`; check `$COVEN_HOME` ownership and permissions              |
| Session browser shows a table, not a UI     | Terminal isn't interactive; use `coven sessions --manage` to force the browser         |
| `cwd` rejected at launch                    | The working directory resolves outside the project root; use a path inside it          |
| Stale "running" sessions after daemon crash | Run `coven daemon restart` to mark dead sessions `orphaned`, then archive or sacrifice them via `coven sessions --all` |
| Sessions feel slow / daemon sluggish        | Run `coven pc status` to check system pressure; `coven pc top --n 10` for CPU culprits |
| `coven attach` won't accept input           | The session is not live; attach replays logs for completed or archived sessions        |
| Secret scan fails                           | Remove the secret from your working tree; rotate it if it entered git history          |
| API version mismatch                        | Update Coven to match the client's expected contract, or update the client             |

For the full diagnostic flowchart and detailed resolution steps, see [`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md).

---

## Contributing

> **Contribution Status — Updated July 2026**
>
> External Pull Requests are open. Please start from an issue for larger changes
> and include the readiness packet requested by the PR template.

### First 10 minutes (source checkout)

```bash
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
cargo test -p coven-cli --test smoke -- --nocapture
```

A healthy first pass: the workspace builds, `doctor` prints setup status, and the smoke test passes. The smoke test uses an isolated temporary `COVEN_HOME` and injects a fake `codex` binary into `PATH` — it does not require real harness credentials or a network connection.

### Local development loop

```bash
# Build
cargo build --workspace

# Rust checks (required before any PR)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked

# Secret scanner (required before any PR)
python scripts/check-secrets.py

# Smoke test (required for daemon/session/attach/ritual changes)
cargo test -p coven-cli --test smoke -- --nocapture

# Manual smoke run — use a throwaway project, not a real repository
cargo run -p coven-cli -- daemon start
cargo run -p coven-cli -- run codex "say hello from coven"
cargo run -p coven-cli -- sessions
cargo run -p coven-cli -- daemon stop
```

### Architecture rules for contributors

- **Rust is the authority layer.** Process launch, cwd/project-root validation, PTY lifecycle, session persistence, and socket request enforcement are all Rust's responsibility. TypeScript clients improve UX but are never the trust boundary.
- **All clients are untrusted for enforcement** — this includes comux and the OpenClaw plugin.
- **Keep harness support focused.** v0 targets Codex and Claude Code only until adapter contracts are stable.
- **OpenClaw separation.** Do not place Coven code in OpenClaw core. The integration belongs in `packages/openclaw-coven` as `@opencoven/coven`.
- **No future orchestration commands as user-facing** until they exist in the CLI and socket API.

### Documentation rules

- Use **OpenCoven** for the ecosystem and organization. Use **Coven** for the CLI and daemon product.
- The user-facing command is always `coven` — never `opencoven` or `@opencoven` in user-facing documentation.
- Use canonical community references: `discord.gg/opencoven` and `@OpenCvn`.
- Use placeholders in all examples: `/path/to/project`, `/Users/example`, `session-1`, `intent-1`.
- Run `python scripts/check-secrets.py` before submitting any PR, including docs-only changes.
- Update docs whenever command behavior, API behavior, or trust boundaries change.

### Maintainer release checklist

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
# For package releases: verify package contents with dry run, attach checksums for native binaries
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development loop, release checklist, and documentation standards.

---

## Code of Conduct

OpenCoven is committed to building a welcoming, respectful community where people of all backgrounds and experience levels can contribute and learn.

**We expect all contributors and community members to:**

- Be respectful and kind in all interactions — issues, PRs, Discord, and X
- Focus criticism on ideas and code, not people
- Welcome newcomers and answer questions with patience
- Assume good faith before assuming bad

**We do not tolerate:**

- Harassment, discrimination, or abuse in any form
- Personal attacks or derogatory language
- Sustained or repeated disruptive behavior

To report unacceptable behavior, contact the maintainers privately through GitHub or Discord. Reports will be handled with discretion.

---

## Roadmap

> **Last updated: May 2026.** See [`docs/ROADMAP.md`](docs/ROADMAP.md) for detailed milestone checklists with individual items.

| Status          | Milestone                         | Summary                                                                                                                       |
| --------------- | --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| ✅ **Shipped**  | A: Local runtime foundation       | `coven` CLI, Rust daemon, PTY sessions, SQLite ledger, versioned `coven.daemon.v1` API, Codex + Claude adapters, npm packages |
| 🔄 **Now**      | B: CastCodes workspace            | CastCodes as primary public workspace; Cast Agent + Coven integration direction                                               |
| 🔄 **Now**      | C: Community transparency         | Public roadmap, Discord update cadence, public issue board                                                                    |
| 📋 **Next**     | D: Harness expansion              | Generic command adapter from real usage, third harness proof, compatibility docs                                              |
| 🔬 **Next/Lab** | E: Visible lane → verify → review | CastCodes-native agent lanes, live session display, verification gates, explicit PR/merge workflow                            |
| 🔭 **Later**    | F: Multi-harness orchestration    | Handoff protocol, capability routing, multi-instance coordination, audit dashboard (Phases 1–4)                               |

The roadmap is written as a community-facing progress ledger, not an internal promise sheet. Items move when they are designed, implemented, tested, and released. Dates are avoided unless a release is already scheduled.

---

## Security

Coven is pre-1.0 software. Treat it accordingly:

- **Do not run untrusted harnesses or prompts in sensitive repositories.** Session logs capture harness output; if the harness dumps secrets, Coven logs them.
- **Do not commit runtime state.** `.coven/`, `*.sqlite`, `*.sock`, `.env*` files, and encryption keys should never enter source control.
- **Do not paste secrets into prompts.** Event payloads are redacted before API display, but defense in depth starts with not having secrets in prompts.

**Reporting vulnerabilities:** Please use [GitHub Security Advisories](https://github.com/OpenCoven/coven/security/advisories) for this repository. If advisories are unavailable, contact the maintainer privately. Do not post exploit details in public issues.

See [SECURITY.md](SECURITY.md) for the full security policy and [`docs/SAFETY-MODEL.md`](docs/SAFETY-MODEL.md) for the trust boundary and local access model.

---

## License

MIT © Valentina Alexander and the OpenCoven contributors — see [LICENSE](LICENSE) for full terms.

---

## Community & Support

| Channel                 | Link                                                       |
| ----------------------- | ---------------------------------------------------------- |
| 🌐 Website              | [opencoven.ai](https://opencoven.ai/)                      |
| 📝 Feedback             | [feedback.opencoven.ai](https://feedback.opencoven.ai/)    |
| 💬 Discord              | [discord.gg/opencoven](https://discord.gg/opencoven)       |
| 🐦 X / Twitter          | [@OpenCvn](https://x.com/OpenCvn)                          |
| 🐛 Issues & Bug Reports | [GitHub Issues](https://github.com/OpenCoven/coven/issues) |
| 📖 Documentation        | [`docs/` directory](docs/)                                 |
| 🗺️ Public Roadmap       | [docs/ROADMAP.md](docs/ROADMAP.md)                         |

---

<div align="center">

**[OpenCoven](https://github.com/OpenCoven)** — One project. Any harness. Visible work.

</div>
