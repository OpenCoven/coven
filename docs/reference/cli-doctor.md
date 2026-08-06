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

The prose report uses `[OK]` for passing checks, `[--]` for advisory warnings,
and `[!!]` only for blocking failures. It is line-oriented plain text and does
not emit or pass through ANSI escape sequences, including when global color is
forced or configuration text contains terminal controls.

## Machine-readable output

`coven doctor --json` emits one JSON document for scripts and CI gates, with
the same exit contract as the prose output (exit 1 when a blocking problem is
found):

```json
{
  "ok": true,
  "blocking": false,
  "store": "<coven-home>",
  "project": "<project>",
  "checks": [
    { "id": "daemon", "status": "pass", "message": "running (pid 12345, socket <daemon-socket>)" },
    { "id": "harness:codex", "status": "pass", "message": "`codex` executable is available (built-in)" },
    { "id": "harnesses", "status": "pass", "message": "2 of 4 configured harness executables available" },
    { "id": "engine", "status": "pass", "message": "<engine> (managed install), version 0.6.1 (pin 0.6.1)" },
    { "id": "credentials:engine", "status": "warn", "message": "authentication configured; provider turn not verified", "hint": "run an explicitly authorized test turn to verify provider access" },
    { "id": "credentials:codex", "status": "warn", "message": "executable available; authentication not verified", "hint": "authenticate or inspect local setup with: codex login; verify provider access with an explicitly authorized test turn" }
  ],
  "nextSteps": ["coven run codex \"explain this repo in 5 bullets\"", "coven sessions"]
}
```

Known Doctor-owned absolute path roles are replaced with stable tokens such as
`<coven-home>`, `<project>`, `<engine>`, `<daemon-socket>`, `<repo>`, and
`<repos-config>`. This keeps repeated output comparable across machines and
safer to attach to CI logs or bug reports without implying that arbitrary
user-authored hint text is sanitized. Run the prose form locally when you need
the concrete paths. `project` is `null` when the command runs outside a project
root.

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
| `Engine` | Whether the Coven engine is installed and meets the minimum supported version. |
| `Familiars` | Configured familiar identities from `familiars.toml`, if present. |
| `Credentials` | Advisory local engine auth configuration and explicit `authentication not verified` rows for external harnesses. Doctor calls only the engine's contractually offline `auth status --json`; it never launches a provider harness, starts a provider turn, contacts a provider, or reads harness credentials. |
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

Typical Unix-like human output from `coven daemon status`:

```text
Coven daemon: running (pid 12345, socket /path/to/coven-home/coven.sock)
```

On Windows, `socket` is diagnostic pipe metadata. Clients needing a connection
path use `state.daemon_ipc` from `coven config paths --json`.

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

`coven doctor` exits `0` when local structural prerequisites are ready, so
scripts can gate on them (`coven doctor && …`). Provider access is deliberately
outside that claim and still requires an explicitly authorized test turn. The
command exits `1` when it finds a blocking local problem:

- no supported harness is available on `PATH`
- the daemon is stale (`running` and `stopped` are both healthy states)
- a registered repo entry points at a missing or non-git path
- `coven-code` is missing
- the installed `coven-code` version is older than the supported minimum

Each missing harness prints an advisory `[--]` line with an install hint. When
none is available, Doctor adds a blocking `[!!] No supported harness is
available` line and exits 1; one working harness keeps the aggregate usable.
Executable discovery does not prove provider authentication. A harness's own
login/status command can configure or inspect local authentication, while only
an explicitly authorized test turn verifies provider access.

`coven adapter doctor` is stricter about its own subject: it exits `1` if any
listed adapter is unavailable. `coven wt --doctor` exits `1` when managed hooks
are missing or a worktree sits outside the protocol layout.
