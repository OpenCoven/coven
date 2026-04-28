<p align="center">
  <img src="assets/opencoven.svg" alt="OpenCoven logo" width="96" height="96">
</p>

# @opencoven/cli

Node package wrapper for the native **Coven** CLI.

Coven is the OpenCoven harness substrate: a local Rust CLI/daemon for project-scoped Codex, Claude Code, and future harness sessions.

```bash
npx @opencoven/cli doctor
```

The user-facing command remains `coven`; OpenCoven is the package namespace.

## Commands

```bash
coven doctor
coven daemon start
coven run codex "fix tests"
coven run claude "polish this UI"
coven sessions
coven attach <session-id>
```

## Status

This wrapper is part of the early MVP package shape. Stable distribution depends on the native binary release flow being ready.
