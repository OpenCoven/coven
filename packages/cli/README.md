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
coven run codex "fix tests"
coven run claude "polish this UI"
coven sessions
coven sessions --all
coven attach <session-id>
coven summon <session-id>
coven archive <session-id>
coven sacrifice <session-id> --yes
```

Session rituals use Coven language while staying safe: archive hides old work without deleting it, summon restores archived work, and sacrifice permanently deletes only after explicit `--yes` confirmation.

## Status

This wrapper is part of the early MVP package shape. Stable distribution depends on the native binary release flow being ready.
