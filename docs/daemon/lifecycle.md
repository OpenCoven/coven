---
summary: "start, status, restart, stop."
read_when:
  - Managing the daemon process
title: "Daemon lifecycle"
description: "How the Coven daemon starts, supervises sessions, and shuts down cleanly, including socket binding, store recovery, and graceful PTY teardown on exit."
---

# Daemon lifecycle

The Coven daemon owns live harness sessions, the local socket or pipe, and the SQLite-backed session ledger under `COVEN_HOME`.

Use the daemon commands as the operational surface:

```sh
coven daemon start
coven daemon status
coven daemon restart
coven daemon stop
```

## Start

```sh
coven daemon start
```

Start creates `COVEN_HOME` when needed, binds the local daemon transport, records daemon metadata, and returns after the daemon is reachable.
The launched daemon is the sole publisher of `daemon.json`; the launcher passes
one canonical start timestamp and verifies that same identity through health
before reporting success.

Run this after install:

```sh
coven doctor
coven daemon start
coven daemon status
```

## Status

```sh
coven daemon status
```

Status reports whether the daemon is running, stopped, stale, or unreachable. Use it before launching a session and after service-manager changes.

If status reports stale state:

```sh
coven daemon restart
```

## Restart

```sh
coven daemon restart
```

Restart is the normal repair path after:

- updating the `coven` binary or npm wrapper;
- changing `COVEN_HOME`;
- changing shell `PATH`;
- installing or updating harness CLIs;
- recovering from a stale socket or stale daemon metadata.

After restart:

```sh
coven doctor
coven daemon status
```

## Stop

```sh
coven daemon stop
```

Stop the daemon before uninstalling Coven, moving `COVEN_HOME`, changing service-manager configuration, or replacing a source-built binary.

On Unix, status, stop, and restart are bound to the selected profile. The
recorded `daemon.json` socket must exactly equal the canonical
`<COVEN_HOME>/coven.sock`; Coven never derives the selected home from the
recorded socket. Copying daemon status between profiles therefore cannot make
one profile connect to or shut down another.

On Windows, new status records also contain `processCreationTime`, a decimal
64-bit FILETIME string. Lifecycle commands compare it with the connected or
live process so a reused PID is never treated as the recorded daemon. Legacy
records without this field remain supported when owner-validated,
profile-bound pipe health authenticates the daemon; otherwise cleanup fails
closed.

## First session after daemon start

```sh
cd /path/to/project
coven run codex "describe this repo"
coven sessions
```

Use Claude Code instead with:

```sh
coven run claude "describe this repo"
```

## Service managers

Start manually before adding a service manager. Once `coven doctor` and `coven daemon status` work, use:

- [launchd service](/install/launchd) on macOS;
- [systemd unit](/install/systemd) on Linux;
- [Headless server](/install/headless-server) for SSH-oriented operation.

The service manager must inherit or define a `PATH` that exposes `coven` and any harness CLI you expect the daemon to launch.

## State directory

By default, Coven uses `<home>/.coven`. To isolate an environment:

```sh
export COVEN_HOME="$HOME/.coven-demo"
coven daemon restart
coven doctor
```

PowerShell:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven-demo"
coven daemon restart
coven doctor
```

See [COVEN_HOME](/daemon/coven-home) for the full layout.
