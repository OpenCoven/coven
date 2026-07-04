---
summary: "Run Coven without a desktop, behind SSH or a Tailscale tunnel."
read_when:
  - Hosting Coven on a server
title: "Headless servers"
description: "Run Coven on headless servers: daemon-only setup, no TUI, with systemd or launchd supervision and SSH tunnels for client access to the socket."
---

## Install path

On a headless host, install the same way as Linux, then operate through SSH:

```sh
npm install -g @opencoven/cli
coven doctor
```

If the package target does not fit the server, build from source.

## Operating model

Use one dedicated account and run Coven, the daemon, and harness CLIs as that
same user:

```sh
export COVEN_HOME="$HOME/.coven"
coven doctor
```

Do not share one state directory between Unix users. If you add systemd, make
the unit use the same `COVEN_HOME` and `PATH` that passed `coven doctor` over
SSH.

## Verify

```sh
coven daemon start
coven daemon status
cd /path/to/project
coven run codex "summarize this branch"
coven sessions
```

From a later SSH session, copy an id from `coven sessions --plain` and follow the
run:

```sh
coven attach <session-id>
```

## Related

- [Headless server install](/install/headless-server)
- [systemd unit](/install/systemd)
- [Daemon lifecycle](/daemon/lifecycle)
