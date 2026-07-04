---
summary: "GET /api/v1/health and what every field means."
read_when:
  - Building a health probe
title: "Health"
description: "Reference for GET /api/v1/health, the Coven daemon liveness endpoint clients call first to confirm the socket is up and the API contract is negotiated."
---

Daemon health is the signal that the background process is reachable and using
the expected socket for the active `COVEN_HOME`.

Use:

```sh
coven daemon status
```

Typical healthy output:

```text
coven daemon status=running ok=true pid=12345 socket=/home/alex/.coven/coven.sock
```

`running` means Coven found daemon metadata and verified the process/socket.
`ok=true` means the daemon health response succeeded.

## Status values

| Status | Meaning | Next step |
| --- | --- | --- |
| `status=stopped` | No daemon metadata is present. | Run `coven daemon start`. |
| `status=running ok=true` | The daemon is reachable. | Run `coven doctor` or start a session. |
| `status=stale ok=false` | Metadata exists, but the daemon no longer looks healthy. | Run `coven daemon stop`, then `coven daemon start`. |

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
