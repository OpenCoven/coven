<p align="center">
  <img src="assets/opencoven.svg" alt="OpenCoven logo" width="96" height="96">
</p>

# @opencoven/cli

Node package wrapper for the native **Coven** CLI.

Coven is the OpenCoven harness substrate: a local Rust CLI/daemon for project-scoped Codex, Claude Code, and future harness sessions.

OpenCoven turns AI from a blank chatbox into a living workspace of agents that remember, coordinate, and belong to you. Coven is the local command layer for running those agent workflows inside explicit project boundaries.

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

This wrapper is live for early adopters. The release workflow publishes `@opencoven/cli`, `@opencoven/cli-macos`, `@opencoven/cli-macos-x64`, `@opencoven/cli-linux-x64`, and `@opencoven/cli-windows`; check npm for the current `latest` tag before making version-specific claims. Coven itself is still an early local-first MVP, so command/API compatibility should be tracked in the repo docs and release notes.
