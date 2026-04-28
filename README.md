<p align="center">
  <img src="assets/opencoven/opencoven-256.png" alt="Coven logo" width="128" height="128">
</p>

# Coven

Coven is a Rust-first, standalone CLI/daemon harness substrate for project-scoped interactive agent sessions.

The goal is simple: run trusted coding harnesses like Codex and Claude Code inside explicit project boundaries, keep the work visible and attachable, and give clients like comux and the external OpenClaw plugin a stable local runtime to coordinate with.

> One project. Any harness. Visible work.

## Status

Coven is an early MVP. It is public for transparency and review, but it is not published as a stable package yet.

The current implementation includes:

- `coven doctor` harness detection
- `coven daemon start/status/stop`
- project-scoped `coven run <harness> <prompt>` sessions
- detached PTY sessions backed by daemon runtime handles
- `coven sessions`
- `coven attach <session-id>`
- local daemon HTTP API over a Unix socket for comux and external plugin integrations
- SQLite-backed metadata and event logs under `COVEN_HOME` / `.coven`

## Safety model

Coven is local-first and intentionally explicit:

- Sessions are scoped to an explicit project root.
- Runtime state is ignored by git (`.coven/`, SQLite files, sockets, logs, env files, and private keys).
- The repository includes a CI secret guard that scans current files and git history without printing matched values.
- Coven does not require repository-stored credentials; harness auth should stay in the harness/provider's normal local auth flow.

Do not run untrusted prompts or harnesses in sensitive repositories unless you understand the harness' own permissions and tool behavior.

## Usage examples

```sh
coven doctor
coven daemon start
coven daemon status
coven run codex "fix tests"
coven run claude "polish this UI"
coven sessions
coven attach <session-id>
```

## Local API

The daemon exposes a local HTTP API over a Unix socket for clients such as comux and the external OpenClaw plugin:

- `GET /health`
- `GET /sessions`
- `POST /sessions`
- `GET /sessions/:id`
- `GET /events?sessionId=...`
- `POST /sessions/:id/input`
- `POST /sessions/:id/kill`


## OpenClaw plugin

Coven also carries an external OpenClaw plugin package at `packages/openclaw-coven`.
OpenClaw core does not include OpenCoven or Coven; the integration path is the opt-in ClawHub package `@opencoven/coven`.

Install it with:

```sh
openclaw plugins install clawhub:@opencoven/coven
```

The plugin is opt-in: enable `plugins.entries["opencoven-coven"].enabled` and set `acp.backend = "coven"` only when you want OpenClaw ACP sessions to route through a local Coven daemon.

The plugin is a socket client. It does not become part of Coven's trust root, and the Rust daemon still validates project roots, cwd, harness ids, input, and kill requests.

## Community

- Discord: `discord.gg/opencoven`
- X / Twitter: `@OpenCvn`

## Brand assets

The canonical Coven logo pack lives under [`assets/opencoven`](assets/opencoven). Package-local README logos are copied into each publishable package so package previews do not depend on files outside the package tarball.

## MVP direction

Coven v0 focuses on:

- a Rust CLI command named `coven`
- a local daemon for supervised harness sessions
- interactive PTY sessions scoped to explicit project roots
- built-in Codex and Claude Code adapters
- session list, attach, kill, metadata, and event logs
- a minimal local API for comux and external plugin integration
- npm wrapper package shape under `@opencoven/cli` once publishing is ready

For the current authority boundaries and release split, see [docs/OPERATIONAL-MODEL.md](docs/OPERATIONAL-MODEL.md).

## Security

See [SECURITY.md](SECURITY.md).
