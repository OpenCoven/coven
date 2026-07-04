---
summary: "Where Coven keeps state on each OS."
read_when:
  - Auditing where Coven writes
title: "Paths and state directories"
description: "Where Coven stores its state on disk: COVEN_HOME, the SQLite ledger, the append-only event log, sockets, and per-session directories on each host."
---

Coven keeps state in one per-user directory and uses the current project
directory only for project-root detection and harness execution.

Start with:

```sh
coven doctor
```

The `Store:` line shows the active state root.

## Default paths

| Host | Default state directory |
| --- | --- |
| macOS | `/Users/<you>/.coven` |
| Linux | `/home/<you>/.coven` |
| WSL2 | `/home/<you>/.coven` inside the WSL distribution |
| Windows | `C:\Users\<you>\.coven` |
| Containers | The value of `COVEN_HOME`, or the container user's home directory |

Override the state root only when you need a separate profile or an explicit
persistent mount:

```sh
export COVEN_HOME="$HOME/.coven-work"
coven doctor
```

PowerShell:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven-work"
coven doctor
```

## Files you may see

| Path | Purpose |
| --- | --- |
| `coven.sqlite3` | Local SQLite ledger for sessions and events. |
| `daemon.json` | Background daemon metadata. |
| `coven.sock` | Local daemon socket on Unix-like hosts. |
| `daemon-recovery.log` | Recovery/startup notes for daemon launches. |
| `familiars.toml` | Optional familiar identity registry. |
| `repos.toml` | Optional repository registry. |

Use CLI commands instead of editing state files:

```sh
coven sessions --plain
coven daemon status
coven logs prune --dry-run
```

## Project paths

Run sessions from inside a project:

```sh
cd /path/to/project
coven run codex "describe this repo"
```

`coven doctor` prints `Project: not inside a git/project root yet` when the
current directory is not a project. That is fine for install checks, but session
launches should happen from the project you want the harness to inspect.

## WSL2 and containers

Keep `COVEN_HOME` on the same filesystem where the daemon runs. In WSL2, avoid
placing it under `/mnt/c/...`; use the Linux home directory. In containers,
bind-mount a persistent directory if you want sessions and logs to survive
container replacement.

See [COVEN_HOME layout](/daemon/coven-home) for daemon-specific details.
