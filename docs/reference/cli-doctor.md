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

## Machine-readable output

`coven doctor --json` emits one JSON document for scripts and CI gates, with
the same exit contract as the prose output (exit 1 when a blocking problem is
found):

```json
{
  "ok": true,
  "blocking": false,
  "store": "/home/alex/.coven",
  "project": "/home/alex/src/app",
  "checks": [
    { "id": "daemon", "status": "pass", "message": "running (pid 12345, socket /home/alex/.coven/coven.sock)" },
    { "id": "harness:codex", "status": "pass", "message": "`codex` is ready (built-in)" },
    { "id": "harnesses", "status": "pass", "message": "1 of 3 configured harnesses available" },
    { "id": "engine", "status": "pass", "message": "/home/alex/.coven/engine/bin/coven-code (managed install), version 0.6.1 (pin 0.6.1)" }
  ],
  "nextSteps": ["coven run codex \"explain this repo in 5 bullets\"", "coven sessions"]
}
```

Check `status` is `pass`, `warn`, or `fail`. Every `fail` is blocking — `ok`
is false and the command exits 1 — while `warn` needs attention but does not
block (for example a daemon that has not been started yet). Failing checks
carry a `hint` with the repair command. Gate scripts on the envelope:

```sh
coven doctor --json | jq -e .ok
```

`coven adapter doctor [id] --json` uses the same envelope for adapter
availability, where any missing adapter is a `fail`.

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

When no supported harness is visible, `doctor` prints a per-harness install
hint. For Codex, Claude Code, and GitHub Copilot CLI those hints boil down to:

```sh
npm install -g @openai/codex
codex login
npm install -g @anthropic-ai/claude-code
claude doctor
npm install -g @github/copilot
copilot login
coven doctor
```

If you installed a harness in another shell, open a new terminal and run
`coven doctor` again. Coven can only launch CLIs that are visible from the
environment where the daemon/session starts.

## Daemon status

`doctor` summarizes daemon state, but use the daemon command for scriptable
status:

```sh
coven daemon status --json
```

Typical human output from `coven daemon status`:

```text
Coven daemon: running (pid 12345, socket /home/alex/.coven/coven.sock)
```

`not running` means no background daemon is running yet. Start it with:

```sh
coven daemon start
```

`stale` means metadata exists for a process/socket that no longer looks
healthy. Try:

```sh
coven daemon stop
coven daemon start
```

## Exit behavior

`coven doctor` exits `0` when the environment can run Coven end to end, so
scripts can gate on it (`coven doctor && …`). It exits `1` when it finds a
blocking problem:

- no supported harness is available on `PATH`
- the daemon is stale (`running` and `stopped` are both healthy states)
- a registered repo entry points at a missing or non-git path
- `coven-code` is missing

A missing harness prints a `[!!]` line with an install hint but does not fail
the check while another harness is available — one working harness makes Coven
usable.

`coven adapter doctor` is stricter about its own subject: it exits `1` if any
listed adapter is unavailable. `coven wt --doctor` exits `1` when managed hooks
are missing or a worktree sits outside the protocol layout.
