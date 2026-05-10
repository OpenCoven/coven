<p align="center">
  <img src="assets/opencoven/opencoven.svg" alt="OpenCoven logo" width="128" height="128">
</p>

<h1 align="center">OpenCoven / Coven</h1>

<h3 align="center">Project-scoped harness sessions for the OpenCoven ecosystem</h3>

<p align="center">
  Run Codex, Claude Code, and future harnesses inside explicit local project boundaries.<br/>
  Launch, observe, attach, and coordinate agent work through one neutral runtime substrate.
</p>

<p align="center">
  <a href="https://github.com/OpenCoven/coven/issues"><strong>Issues</strong></a>
  ·
  <a href="https://discord.gg/opencoven"><strong>Discord</strong></a>
  ·
  <a href="https://x.com/OpenCvn"><strong>@OpenCvn</strong></a>
</p>

---

## Install

Coven is still an early local-first MVP, but the npm wrapper is live for supported platforms. The user-facing command is always `coven`.

Try the published package:

```sh
npx @opencoven/cli doctor
pnpm dlx @opencoven/cli doctor
```

Or build from source:

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
```

Current npm package latest: `0.0.10` for `@opencoven/cli`, `@opencoven/cli-macos`, and `@opencoven/cli-linux-x64`. Windows x64 release wiring is staged for the next package release as `@opencoven/cli-windows`.

## Quick Start

New to Coven? Run the interactive menu first:

```sh
coven
# or explicitly:
coven tui
```

The menu starts with **Start here**, checks your local setup, and shows the safest first command to try.

Prefer copy/paste commands?

```sh
cd /path/to/your/project
coven doctor
coven daemon start
coven daemon restart
coven run codex "fix the failing tests"
coven run claude "polish this UI"
coven sessions
coven sessions --all
coven sessions --plain
```

`coven doctor` checks whether supported local harness CLIs are available. `coven run` creates a project-scoped session record, validates the working directory, and launches the selected harness through Coven-managed PTY execution. In a terminal, `coven sessions` opens a human session browser where you can select work and choose visible actions like **Rejoin**, **View Log**, **Summon**, **Archive**, and **Sacrifice** without copying IDs; use `--plain` for scripts or table output.

Coven also provides a rescue loop for OpenClaw contributors and users:

```sh
coven patch openclaw
```

If OpenClaw breaks, Coven gives you a predictable repair room: choose a repo, choose a harness, get a verified patch.

## What it does

Coven is the local harness substrate for OpenCoven. It does not replace your coding agent, your UI, or OpenClaw. It gives them a shared room where project work can happen visibly and safely.

- **Project-root boundaries** — every launch is tied to an explicit repository/project root.
- **Harness-neutral runtime** — v0 focuses on Codex and Claude Code, with a clean adapter path for future harnesses.
- **Human session browser** — live and completed work can be selected, rejoined, viewed, archived, restored, or sacrificed without memorizing ids.
- **Attachable PTY sessions** — live work can still be replayed/followed from explicit CLI verbs.
- **Local daemon API** — comux, OpenMeow, and the external OpenClaw plugin can coordinate through the same socket contract.
- **SQLite-backed history** — session metadata and event logs survive daemon restarts.
- **Rust authority layer** — launch, cwd, input, kill, and path-sensitive requests are revalidated in Rust.
- **External OpenClaw bridge** — `@opencoven/coven` is an opt-in plugin; OpenClaw core does not include Coven code.
- **OpenCoven package shape** — CLI wrapper packages live under the `@opencoven/*` namespace while the command stays `coven`.

## Commands

| Command | Action |
|---|---|
| `coven` | Open the beginner-friendly interactive menu |
| `coven tui` | Explicitly open the slash-command TUI |
| `coven doctor` | Detect supported harness CLIs and print install hints |
| `coven daemon start` | Start the local Coven daemon |
| `coven daemon status` | Show daemon health, pid, and socket path |
| `coven daemon restart` | Restart the local daemon and rebind the socket |
| `coven daemon stop` | Stop the local daemon |
| `coven run <harness> <prompt>` | Launch a project-scoped harness session |
| `coven run <harness> <prompt> --cwd <path>` | Launch from a cwd inside the project root |
| `coven run <harness> <prompt> --title <title>` | Set a readable session title |
| `coven sessions` | Open the active session browser in a terminal; print a table when piped |
| `coven sessions --all` | Browse active and archived sessions in a terminal; print all when piped |
| `coven sessions --manage` | Force the interactive session browser |
| `coven sessions --plain` | Force plain table output for scripts/copying |
| `coven attach <session-id>` | Replay/follow session output and forward input |
| `coven summon <session-id>` | Restore an archived session, then replay/follow it |
| `coven archive <session-id>` | Hide a non-running session from the active list while preserving events |
| `coven sacrifice <session-id> --yes` | Permanently delete a non-running session and its events |

Session rituals are intentionally explicit. **Archive** is reversible and keeps the ledger. **Summon** brings an archived session back into the active list. **Sacrifice** is destructive, refuses live sessions, and requires `--yes` so beginners do not delete work by accident.

## Local API

The daemon exposes a small versioned HTTP API over a Unix socket for first-party and external clients. The current public contract is **`v1`**; new clients should use the `/api/v1` prefix.

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/api-version` | Read the active API version and supported versions |
| `GET /api/v1/health` | Check daemon health and metadata |
| `GET /api/v1/sessions` | List sessions |
| `POST /api/v1/sessions` | Launch a session |
| `GET /api/v1/sessions/:id` | Fetch one session |
| `GET /api/v1/events?sessionId=...` | Read session events |
| `POST /api/v1/sessions/:id/input` | Forward input to a live session |
| `POST /api/v1/sessions/:id/kill` | Kill a live session |

Treat the socket API as the product contract. Clients may validate for UX, but the Rust daemon remains the authority boundary. See [`docs/API.md`](docs/API.md) for compatibility rules.

Current stable contract: [`docs/API-CONTRACT.md`](docs/API-CONTRACT.md). `GET /api/v1/health` exposes `apiVersion: "v1"` and `supportedApiVersions: ["v1"]` for client handshakes.

## Requirements

- Rust stable toolchain
- Git
- macOS or another Unix-like system for daemon socket / PTY behavior today
- At least one supported harness CLI:
  - [Codex](https://github.com/openai/codex)
  - [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
- Node.js 18+ only for npm wrapper/plugin package development

If `coven doctor` reports a missing harness, install or expose one CLI on `PATH`, run the harness once to finish local authentication/setup, then retry `coven doctor`:

```sh
npm install -g @openai/codex
# or: brew install --cask codex
codex login

npm install -g @anthropic-ai/claude-code
claude doctor
```

## OpenCoven integrations

- **comux** is the visual cockpit for agent panes and can consume Coven-managed sessions through the local API.
- **OpenClaw** integrates only through the external `@opencoven/coven` plugin package.
- **OpenMeow** can consume Coven session status, intake, or notifications as the desktop companion surface matures.

Coven is the room where harnesses run. The clients decide how to present and route that work.

## Documentation

- [Public roadmap](docs/ROADMAP.md)
- [Product spec](docs/PRODUCT-SPEC.md)
- [Architecture diagrams](docs/ARCHITECTURE.md)
- [Operational model](docs/OPERATIONAL-MODEL.md)
- [MVP plan](docs/MVP-PLAN.md)
- [Future harnesses](docs/FUTURE-HARNESSES.md)
- [Brand assets](docs/BRAND.md)
- [Design system](DESIGN.md)
- [Brand adherence checklist](docs/BRANDING-ADHERENCE.md)
- [Security policy](SECURITY.md)

## Contributing

See **[CONTRIBUTING.md](./CONTRIBUTING.md)** for the recommended local development loop, release checks, and OpenCoven documentation rules.

## Community

- Discord: `discord.gg/opencoven`
- X / Twitter: `@OpenCvn`

## License

MIT
