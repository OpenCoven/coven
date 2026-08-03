---
summary: "What lives under COVEN_HOME and how to relocate it."
read_when:
  - Customizing where Coven keeps state
title: "COVEN_HOME"
description: "Reference for COVEN_HOME: the on-disk root the Coven daemon uses for the SQLite ledger, append-only event log, sockets, and per-session state."
---

COVEN_HOME is the per-user state root for the Coven CLI and daemon. If unset,
Coven uses `<home>/.coven`.

Check the active state directory with:

```sh
coven doctor
```

The `Store:` line is the effective COVEN_HOME path.

## What lives there

The exact contents can change between releases, but a normal state directory can
contain:

| Path | Purpose |
| --- | --- |
| `coven.sqlite3` | Local session/event store. |
| `coven.sock` | Local daemon socket on Unix-like hosts. |
| `daemon.json` | Daemon pid/socket metadata. |
| `state.lock` | Shared/exclusive coordination between normal commands and reset. |
| `reset-transaction.json` | Durable marker used to roll back an interrupted reset before normal commands resume. |
| `daemon-recovery.log` | Background start/recovery notes. |
| `familiars.toml` | Optional familiar identity registry. |
| `repos.toml` | Optional known repository registry. |
| `keys/session-artifacts.key` | Key coupled to encrypted session artifact records. |
| `mobile/` | Mobile gateway configuration, pairings, host identity, and audit state. |
| `logs/` or artifacts directories | Redacted logs and retained session artifacts when enabled. |
| `reset-backups/` | Timestamped, recoverable backups created by `coven reset --apply`. |

Do not edit the SQLite store by hand. Use the CLI when possible:

```sh
coven sessions
coven daemon status
coven logs prune --dry-run
coven reset --list-features
```

For partial debugging resets, use [`coven reset`](/reference/cli-reset) rather
than deleting `COVEN_HOME`. It previews by default and backs up only selected
registered local state after `--apply`.

## Defaults by platform

- macOS: `/Users/<you>/.coven`
- Linux: `/home/<you>/.coven`
- Windows: `C:\Users\<you>\.coven`
- WSL2: `/home/<you>/.coven` inside the WSL distribution
- Containers: whatever path you set with `COVEN_HOME`

## Override for a separate profile

macOS/Linux/WSL2:

```sh
export COVEN_HOME="$HOME/.coven-work"
coven doctor
coven daemon start
```

PowerShell:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven-work"
coven doctor
coven daemon start
```

Every command that should use that profile must receive the same environment
variable. If a daemon is already running under another COVEN_HOME, stop or
restart the daemon for the profile you intend to use.

## Placement rules

Use a local, per-user, persistent directory. Avoid:

- World-writable directories.
- Synced folders that can rewrite socket or SQLite files.
- Network filesystems for the daemon socket.
- Windows-mounted paths from WSL2.
- Ephemeral container filesystems when you need session history.

If you move COVEN_HOME, restart:

```sh
coven daemon restart
coven daemon status
```

See [Daemon overview](/daemon/index) and [Daemon health](/daemon/health).
