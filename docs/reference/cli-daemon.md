---
summary: "Daemon lifecycle subcommands."
read_when:
  - Looking up daemon subcommands
title: "coven daemon"
description: "Reference for coven daemon: start, stop, restart, and status subcommands for the Rust daemon process that owns sessions and the local socket API."
---

`coven daemon` manages the local background process that owns session runtime,
socket API state, and health reporting.

```sh
coven daemon status
```

## Commands

| Command | Action |
| --- | --- |
| `coven daemon start` | Start the daemon if it is not already running. Reuses a verified live daemon. |
| `coven daemon status` | Print stopped/running/stale status, pid, socket path, and health flag. |
| `coven daemon restart` | Stop the current daemon if present, then start a daemon with the current binary. |
| `coven daemon stop` | Stop the daemon for the active `COVEN_HOME`. |
| `coven daemon serve` | Hidden foreground server entrypoint used by the background launcher and supervisors. |

## First-run sequence

```sh
coven doctor
coven daemon start
coven daemon status
```

Expected running output:

```text
coven daemon status=running ok=true pid=12345 socket=/home/alex/.coven/coven.sock
```

`ok=true` means the daemon health endpoint responded through the local socket.

## Restart after upgrades

Restart after replacing the CLI binary, changing `PATH`, or switching
`COVEN_HOME`:

```sh
coven --version
coven daemon restart
coven daemon status
```

The restarted process uses the executable that launched the command. This keeps
npm, cargo, and source-checkout installs from accidentally leaving an old daemon
behind.

## State directory

Daemon metadata and the socket live under the active `COVEN_HOME`.

macOS/Linux/WSL2:

```sh
export COVEN_HOME="$HOME/.coven"
coven daemon status
```

PowerShell:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven"
coven daemon status
```

Keep `COVEN_HOME` on a local per-user filesystem. For WSL2, use the WSL
filesystem, not a mounted Windows path. For containers, bind-mount a persistent
directory if sessions must survive container restarts.

## Supervisor use

For systemd, launchd, tmux, or container entrypoints, prefer the foreground
server command inside the supervisor:

```sh
coven daemon serve
```

Use `coven daemon status` from an interactive shell with the same `COVEN_HOME`
to verify the supervised daemon.

## Troubleshooting

- `status=stopped`: run `coven daemon start`.
- `status=stale`: run `coven daemon stop`, then `coven daemon start`.
- Permission errors: verify the daemon user owns `COVEN_HOME`.
- Version drift: run `coven --version`, then `coven daemon restart`.

See [Daemon will not start](/help/daemon-wont-start) for deeper recovery steps.
