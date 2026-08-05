---
summary: "GET /api/v1/health and what every field means."
read_when:
  - Building a health probe
title: "Health"
description: "Reference for GET /api/v1/health, the Coven daemon liveness endpoint clients call first to confirm the socket is up and the API contract is negotiated."
---

Daemon health is the signal that the background process is reachable and using
the expected socket for the active `COVEN_HOME`.

## Storage health and retention

`GET /api/v1/health` also includes a `storage` object. It reports the SQLite
database and WAL sizes, free space on the `COVEN_HOME` filesystem, the oldest
retained event, ages of the last prune and checkpoint, and the event-writer
backlog. The optional `eventWriter` block reports the live queue snapshot, and
`storage.writerBacklogEvents` / `storage.writerBacklogBytes` mirror that same
snapshot.

The daemon runs retention in small background transactions after startup. It
uses the configured raw-artifact and redacted-event retention windows, never
runs `VACUUM` automatically, and uses a non-blocking WAL checkpoint only after
the WAL reaches its maintenance threshold. If free disk falls below 256 MiB,
maintenance is blocked rather than creating more WAL pressure; `storage.status`
becomes `critical` and `maintenanceBlocked` is true so an operator can free
space before retrying. `coven vacuum` remains the explicit repair/compaction
operation.

Recovery logging is rotated at 4 MiB with three retained archives
(`daemon-recovery.log.1` through `.3`).

Use:

```sh
coven daemon status
```

Typical healthy output:

```text
Coven daemon: running (pid 12345, socket <covenHome>/coven.sock)
```

`running` means Coven found daemon metadata and verified the process/socket.
For scripts, `coven daemon status --json` adds an `ok` field that reports
whether the daemon health response succeeded.

`GET /api/v1/health` also includes an optional `eventWriter` object for a
running daemon. `state: "healthy"` means live-session event persistence is
keeping up. `"pressured"` means one or more raw PTY chunks were rejected after
the bounded queue filled; inspect `droppedOutputEvents` and
`droppedOutputBytes`. `"failed"` means the writer could not commit, and
`lastError` carries the diagnostic. Lifecycle events are never dropped for
queue pressure. The global `droppedOutputEvents` and `droppedOutputBytes`
counters remain visible here. Each contiguous pressure episode later appears in
the affected session as one ordered `output_truncated` event, inserted before
the next accepted event.

The same response includes `storage` health. Its `writerBacklogEvents` and
`writerBacklogBytes` fields mirror the live `eventWriter` queue snapshot, while
the remaining fields report SQLite/WAL size, retention and checkpoint age, free
disk, and maintenance errors. `critical` means the free-disk safety watermark
has paused scheduled maintenance; `degraded` means storage health could not be
collected.

## Status values

| Status | Meaning | Next step |
| --- | --- | --- |
| `not running` | No daemon metadata is present. | Run `coven daemon start`. |
| `running` | The daemon is reachable. | Run `coven doctor` or start a session. |
| `stale` | Metadata exists, but the daemon no longer looks healthy. | Run `coven daemon stop`, then `coven daemon start`. |

## First-run health check

```sh
coven doctor
coven daemon start
coven daemon status
cd /path/to/project
coven run codex "say hello from Coven"
```

If you use Claude Code:

```sh
coven run claude "say hello from Coven"
```

## After upgrade or shell changes

Restart the daemon after replacing the CLI binary, changing `PATH`, or
authenticating a harness in a new shell:

```sh
coven --version
coven daemon restart
coven daemon status
```

## Supervisor and remote hosts

For systemd, launchd, SSH, tmux, or container entrypoints, verify health from
the same user and state directory:

```sh
echo "${COVEN_HOME:-$HOME/.coven}"
coven daemon status
```

PowerShell:

```powershell
if (-not $env:COVEN_HOME) { $env:COVEN_HOME="$env:USERPROFILE\.coven" }
coven daemon status
```

If a supervisor runs `coven daemon serve`, keep `PATH` and `COVEN_HOME` explicit
in that supervisor configuration.

## When health stays stale

1. Stop the daemon with `coven daemon stop`.
2. Confirm no old daemon process is still running for the same user.
3. Confirm the state directory is writable.
4. Start again with `coven daemon start`.
5. If it still fails, collect `coven doctor` and `coven daemon status` output for
   a diagnostics report.

See [Daemon will not start](/help/daemon-wont-start).
