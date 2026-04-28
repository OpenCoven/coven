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

Coven is an early MVP. The public package shape is here, but stable installation is not the primary promise yet.

From a checkout today:

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
```

The user-facing command is always `coven`. Once the npm wrapper is ready, the package path is:

```sh
npm exec @opencoven/cli -- doctor
pnpm dlx @opencoven/cli doctor
```

## Quick Start

```sh
cd /path/to/your/project
coven doctor
coven daemon start
coven run codex "fix the failing tests"
coven run claude "polish this UI"
coven sessions
coven attach <session-id>
```

`coven doctor` checks whether supported local harness CLIs are available. `coven run` creates a project-scoped session record, validates the working directory, and launches the selected harness through Coven-managed PTY execution.

## What it does

Coven is the local harness substrate for OpenCoven. It does not replace your coding agent, your UI, or OpenClaw. It gives them a shared room where project work can happen visibly and safely.

- **Project-root boundaries** — every launch is tied to an explicit repository/project root.
- **Harness-neutral runtime** — v0 focuses on Codex and Claude Code, with a clean adapter path for future harnesses.
- **Attachable PTY sessions** — live work can be listed and reattached from the CLI.
- **Local daemon API** — comux, OpenMeow, and the external OpenClaw plugin can coordinate through the same socket contract.
- **SQLite-backed history** — session metadata and event logs survive daemon restarts.
- **Rust authority layer** — launch, cwd, input, kill, and path-sensitive requests are revalidated in Rust.
- **External OpenClaw bridge** — `@opencoven/coven` is an opt-in plugin; OpenClaw core does not include Coven code.
- **OpenCoven package shape** — CLI wrapper packages live under the `@opencoven/*` namespace while the command stays `coven`.

## Commands

| Command | Action |
|---|---|
| `coven doctor` | Detect supported harness CLIs and print install hints |
| `coven daemon start` | Start the local Coven daemon |
| `coven daemon status` | Show daemon health, pid, and socket path |
| `coven daemon stop` | Stop the local daemon |
| `coven run <harness> <prompt>` | Launch a project-scoped harness session |
| `coven run <harness> <prompt> --cwd <path>` | Launch from a cwd inside the project root |
| `coven run <harness> <prompt> --title <title>` | Set a readable session title |
| `coven sessions` | List known sessions |
| `coven attach <session-id>` | Replay/follow session output and forward input |

## Local API

The daemon exposes a small HTTP API over a Unix socket for first-party and external clients:

| Endpoint | Purpose |
|---|---|
| `GET /health` | Check daemon health and metadata |
| `GET /sessions` | List sessions |
| `POST /sessions` | Launch a session |
| `GET /sessions/:id` | Fetch one session |
| `GET /events?sessionId=...` | Read session events |
| `POST /sessions/:id/input` | Forward input to a live session |
| `POST /sessions/:id/kill` | Kill a live session |

Treat the socket API as the product contract. Clients may validate for UX, but the Rust daemon remains the authority boundary.

## Requirements

- Rust stable toolchain
- Git
- macOS or another Unix-like system for daemon socket / PTY behavior today
- At least one supported harness CLI:
  - [Codex](https://github.com/openai/codex)
  - [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
- Node.js 18+ only for npm wrapper/plugin package development

## OpenCoven integrations

- **comux** is the visual cockpit for agent panes and can consume Coven-managed sessions through the local API.
- **OpenClaw** integrates only through the external `@opencoven/coven` plugin package.
- **OpenMeow** can consume Coven session status, intake, or notifications as the desktop companion surface matures.

Coven is the room where harnesses run. The clients decide how to present and route that work.

## Documentation

- [Product spec](docs/PRODUCT-SPEC.md)
- [Operational model](docs/OPERATIONAL-MODEL.md)
- [MVP plan](docs/MVP-PLAN.md)
- [Future harnesses](docs/FUTURE-HARNESSES.md)
- [Brand assets](docs/BRAND.md)
- [Security policy](SECURITY.md)

## Contributing

See **[CONTRIBUTING.md](./CONTRIBUTING.md)** for the recommended local development loop, release checks, and OpenCoven documentation rules.

## Community

- Discord: `discord.gg/opencoven`
- X / Twitter: `@OpenCvn`

## License

MIT
