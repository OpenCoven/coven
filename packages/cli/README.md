<p align="center">
  <img src="assets/opencoven.svg" alt="OpenCoven logo" width="96" height="96">
</p>

# @opencoven/cli

Node package wrapper for the native **Coven** CLI.

Coven is the OpenCoven harness substrate: a local Rust CLI/daemon for project-scoped Codex, Claude Code, and future harness sessions.

```bash
npx @opencoven/cli
```

The user-facing command remains `coven`; OpenCoven is the package namespace.

Run `coven` with no arguments, or `coven tui` explicitly, for the beginner-friendly slash-command menu. It starts with setup checks and safe first commands before launching anything.

## Commands

```bash
coven
coven tui
coven doctor
coven daemon start
coven daemon restart
coven run codex "fix tests"
coven run claude "polish this UI"
coven sessions
coven sessions --all
coven sessions --plain
coven attach <session-id>
coven summon <session-id>
coven archive <session-id>
coven sacrifice <session-id> --yes
```

In a terminal, `coven sessions` opens the human session browser so you can select work and choose **Rejoin**, **View Log**, **Summon**, **Archive**, or **Sacrifice** without copying IDs. Use `coven sessions --plain` for scripts or table output.

Session rituals use Coven language while staying safe: archive hides old work without deleting it, summon restores archived work, and sacrifice permanently deletes only after explicit `--yes` confirmation.

## Status

This wrapper is live for early adopters. The current published npm latest is `0.0.10` for `@opencoven/cli`, `@opencoven/cli-macos`, and `@opencoven/cli-linux-x64`. Coven itself is still an early local-first MVP, so command/API compatibility should be tracked in the repo docs and release notes.
