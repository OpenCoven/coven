---
summary: "Environment readiness check."
read_when:
  - Looking up doctor
title: "coven doctor"
description: "Reference for coven doctor: the first command to run after install. It checks COVEN_HOME, the socket, harness PATH, and the SQLite store."
---

`coven doctor` is the first command to run after installing Coven, changing
`PATH`, authenticating a harness, or moving `COVEN_HOME`.

```sh
coven doctor
```

The command is read-only. It prints local setup state and a next step without
starting a session.

## What it checks

| Section | Meaning |
| --- | --- |
| `Store` | The active Coven state directory. Defaults to `<home>/.coven` unless `COVEN_HOME` is set. |
| `Project` | The current git/project root when the command runs inside a project. |
| `Daemon` | Whether the background daemon is stopped, running, or stale. |
| `Repos` | Configured repositories from Coven repo settings, if present. |
| `Harnesses` | Supported harness executables that are visible on this shell's `PATH`. |
| `Familiars` | Configured familiar identities from `familiars.toml`, if present. |
| `Next steps` | The safest next command based on the detected state. |

## Expected first-run loop

```sh
coven --version
coven doctor
coven daemon start
coven daemon status
cd /path/to/project
coven run codex "explain this repo in 5 bullets"
```

If you use Claude Code instead:

```sh
coven run claude "explain this repo in 5 bullets"
```

## Missing harness output

When neither supported harness is visible, `doctor` prints install hints for
Codex and Claude Code:

```sh
npm install -g @openai/codex
codex login
npm install -g @anthropic-ai/claude-code
claude doctor
coven doctor
```

If you installed a harness in another shell, open a new terminal and run
`coven doctor` again. Coven can only launch CLIs that are visible from the
environment where the daemon/session starts.

## Daemon status

`doctor` summarizes daemon state, but use the daemon command for scriptable
status:

```sh
coven daemon status
```

Typical output:

```text
coven daemon status=running ok=true pid=12345 socket=/home/alex/.coven/coven.sock
```

`status=stopped` means no background daemon is running yet. Start it with:

```sh
coven daemon start
```

`status=stale` means metadata exists for a process/socket that no longer looks
healthy. Try:

```sh
coven daemon stop
coven daemon start
```

## Exit behavior

`coven doctor` should exit successfully when it can inspect the environment,
even if it reports missing harnesses or a stopped daemon. Treat the printed
status as the diagnostic result.
