---
summary: "Index of every coven command. Each subcommand has its own page with full flag reference."
read_when:
  - Looking up a Coven CLI flag
  - Scripting against the Coven CLI
title: "CLI reference"
---

The user-facing command is always `coven`. Wrapper packages like `@opencoven/cli`, `@opencoven/cli-macos`, and `@opencoven/cli-linux-x64` install the same binary.

## Top-level

| Command | Action |
|---|---|
| [`coven`](/reference/cli-coven) | Open the beginner-friendly interactive menu. |
| [`coven tui`](/reference/cli-coven) | Explicitly open the slash-command TUI. |
| [`coven doctor`](/reference/cli-doctor) | Detect supported harness CLIs and print install hints. |
| [`coven daemon`](/reference/cli-daemon) | Lifecycle: start, status, restart, stop. |
| [`coven run`](/reference/cli-run) | Launch a project-scoped harness session. |
| [`coven sessions`](/reference/cli-sessions) | Open the session browser; supports `--plain` and `--json`. |
| [`coven attach`](/reference/cli-attach) | Replay/follow session output and forward input. |
| [`coven summon`](/reference/cli-summon) | Restore an archived session, then replay/follow it. |
| [`coven archive`](/reference/cli-archive) | Hide a non-running session while preserving events. |
| [`coven sacrifice`](/reference/cli-sacrifice) | Permanently delete a non-running session. Requires `--yes`. |
| [`coven patch`](/reference/cli-patch) | Rescue loop, including the OpenClaw repair flow. |

## Flag conventions

- **Project-scoped commands** accept `--cwd <path>` for a launch directory inside the project root.
- **Pipe-friendly commands** accept `--plain` for tables and `--json` for machine output.
- **Destructive commands** require `--yes` (or `--confirm` for `coven pc` relief).
- **Daemon-touching commands** print install/repair hints when the socket is missing.

## Related

- [CLI: coven](/reference/cli-coven)
- [CLI: coven sessions](/reference/cli-sessions)
- [CLI: coven run](/reference/cli-run)
- [Rituals](/rituals)
