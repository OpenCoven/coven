---
summary: "Current Coven CLI command surface."
read_when:
  - Looking up a Coven CLI flag
  - Scripting against the Coven CLI
title: "Coven CLI reference"
description: "Reference for the coven CLI commands: doctor, daemon, run, sessions, attach, archive, kill, summon, sacrifice, view, and TUI command flags."
---


The user-facing command is always `coven`. Wrapper packages like `@opencoven/cli`, `@opencoven/cli-macos`, and `@opencoven/cli-linux-x64` install the same binary.

## Top-level

| Command | Action |
|---|---|
| `coven` | Open the beginner-friendly interactive menu. |
| `coven tui` | Explicitly open the slash-command TUI. |
| `coven doctor` | Detect supported harness CLIs and print install hints. |
| `coven daemon start/status/restart/stop` | Manage the local daemon. |
| `coven run <harness> <prompt>` | Launch a project-scoped harness session. Current harness ids: `codex`, `claude`. |
| `coven sessions` | Open the session browser; supports `--plain`, `--json`, `--all`, and `--manage`. |
| `coven attach <session-id>` | Replay/follow session output and forward input when live. |
| `coven summon <session-id>` | Restore an archived session, then replay/follow it. |
| `coven archive <session-id>` | Hide a non-running session while preserving events. |
| `coven sacrifice <session-id> --yes` | Permanently delete a non-running session. |
| `coven patch openclaw <prompt>` | Local OpenClaw rescue loop. Does not commit or push. |
| `coven pc` | macOS-first diagnostics and explicit `--confirm` relief operations. |

## Flag conventions

- **Project-scoped commands** accept `--cwd <path>` for a launch directory inside the project root.
- **Pipe-friendly commands** accept `--plain` for tables and `--json` for machine output.
- **Destructive commands** require `--yes` (or `--confirm` for `coven pc` relief).
- **Daemon-touching commands** print install/repair hints when the socket is missing.

## Related

- [Getting started](/GETTING-STARTED)
- [Session lifecycle](/SESSION-LIFECYCLE)
- [Harness adapter guide](/HARNESS-ADAPTERS)
