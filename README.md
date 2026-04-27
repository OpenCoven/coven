# Coven

Coven is a private MVP for a Rust-first, standalone CLI/daemon harness substrate for project-scoped interactive agent sessions.

The goal is simple: run trusted coding harnesses like Codex and Claude Code inside explicit project boundaries, keep the work visible and attachable, and give clients like comux and OpenClaw a stable local runtime to coordinate with.

> One project. Any harness. Visible work.

## Status

Coven is private and not published yet. The repo is currently an early scaffold for the MVP implementation.

There is no public install path yet. Future distribution may include a private npm package under `@opencoven/*` that exposes the user-facing `coven` command.

## Community

Coven is private while the MVP matures. When we share community links publicly, use:

- Discord: `discord.gg/opencoven`
- X / Twitter: `@OpenCvn`


## Future CLI examples

```sh
coven doctor
coven run codex "fix tests"
coven run claude "polish this UI"
```

Only `coven doctor` exists in this scaffold.

## MVP direction

Coven v0 will focus on:

- A Rust CLI command named `coven`
- A local daemon for supervised harness sessions
- Interactive PTY sessions scoped to explicit project roots
- Built-in Codex and Claude Code adapters
- Session list, attach, kill, metadata, and event logs
- A minimal local API for comux and OpenClaw integration

Coven is intentionally private-first while the runtime, safety model, and developer experience mature.
