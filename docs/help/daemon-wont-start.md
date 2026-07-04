---
summary: "Socket binding, permission, and stale-pid checks."
read_when:
  - The daemon refuses to start
title: "The daemon will not start"
description: "Fixes when the Coven daemon will not start: socket permissions, stale lock files, conflicting processes, and COVEN_HOME write access on the host."
---

The daemon owns Coven sessions, local socket API state, and the session ledger.
If it will not start, keep the diagnosis in the same user account and the same
`COVEN_HOME` that your CLI uses.

Start with:

```sh
coven doctor
coven daemon status
```

`status=stopped` is not an error. Start it:

```sh
coven daemon start
coven daemon status
```

## Restart with the current binary

After updating Coven or changing `PATH`, restart the daemon so the background
process uses the binary and environment you expect:

```sh
coven --version
coven daemon restart
coven daemon status
```

If `status=running ok=true` appears, continue with:

```sh
coven run codex "say hello from Coven"
```

## Stale socket or pid file

A crashed daemon can leave metadata behind. First ask Coven to clean it up:

```sh
coven daemon stop
coven daemon status
coven daemon start
```

If the status still reports a stale daemon, inspect the state directory:

```sh
echo "${COVEN_HOME:-$HOME/.coven}"
ls -la "${COVEN_HOME:-$HOME/.coven}"
```

PowerShell:

```powershell
if (-not $env:COVEN_HOME) { $env:COVEN_HOME="$env:USERPROFILE\.coven" }
Get-ChildItem $env:COVEN_HOME
coven daemon status
```

Remove socket or status files only after confirming no daemon process is using
them. Then run `coven daemon start` again.

## Permission checks

The daemon needs write access to `COVEN_HOME` and permission to create its local
socket. Fix ownership or choose a state directory owned by your user:

```sh
mkdir -p "$HOME/.coven"
chmod 700 "$HOME/.coven"
COVEN_HOME="$HOME/.coven" coven doctor
```

Do not point `COVEN_HOME` at a shared world-writable directory, a synced folder,
or a Windows filesystem path from WSL2. Use a local per-user directory.

## Supervisor checks

For launchd, systemd, tmux, SSH, or container setups, run the same command the
supervisor runs from an interactive shell first:

```sh
coven daemon serve
```

Stop it with `Ctrl-C` after it binds successfully, then start the supervisor.
If the interactive command works but the supervisor fails, compare `PATH`,
`COVEN_HOME`, working directory, and user account.

## What to attach to an issue

Collect:

```sh
coven --version
coven doctor
coven daemon status
```

Also include your OS, install method, and whether you run Coven directly, under
systemd/launchd, in WSL2, or in a container. Redact project paths and prompts.
