---
summary: "Install Coven on a headless Linux server with systemd."
read_when:
  - Running Coven without a desktop
title: "Headless server"
description: "Install Coven on a headless server: daemon-only setup, no TUI, with systemd or launchd supervision and remote access through SSH tunnels."
---

# Headless server

On a headless host, install Coven the same way as Linux, then operate it through SSH and explicit daemon commands.

```sh
npm install -g @opencoven/cli
coven doctor
```

If the npm native package is not available on the server distribution, use [Install from source](/install/from-source).

## Server layout

Choose a dedicated user and keep state under that user's home directory:

```sh
export COVEN_HOME="$HOME/.coven"
mkdir -p "$COVEN_HOME"
coven doctor
```

Keep `COVEN_HOME` on a local disk owned by the service user. Avoid sharing one state directory between multiple Unix users.

## Harness setup

Install and authenticate the harness CLI as the same user that will run Coven:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Run:

```sh
coven doctor
```

## Daemon lifecycle

Start and inspect the daemon over SSH:

```sh
coven daemon start
coven daemon status
```

Restart after updates or environment changes:

```sh
coven daemon restart
coven doctor
```

Stop it before changing ownership, moving `COVEN_HOME`, or rebuilding the binary:

```sh
coven daemon stop
```

## First remote session

```sh
cd /path/to/project
coven run codex "summarize the current branch"
coven sessions
```

Use `coven attach <session-id>` to follow a live session from a later SSH connection.

## Supervisor note

The CLI daemon commands are the stable operational surface. If you wrap them with systemd, launchd, tmux, or another supervisor, keep the environment explicit:

```sh
COVEN_HOME="$HOME/.coven" coven daemon start
```

Make sure the supervisor has the same `PATH` that exposes `coven`, `codex`, and `claude`.

## Related

- [Linux install](/install/linux)
- [COVEN_HOME layout](/daemon/coven-home)
- [Daemon lifecycle](/daemon/lifecycle)
- [Troubleshooting](/TROUBLESHOOTING)
