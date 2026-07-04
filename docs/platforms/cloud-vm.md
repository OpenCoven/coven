---
summary: "Patterns for cloud-hosted Coven daemons."
read_when:
  - Hosting on a cloud VM
title: "Cloud VM"
description: "Run Coven on a cloud VM: daemon-only headless setup, supervised by systemd, with SSH tunnels to reach the local Unix socket from a laptop client."
---

## Install path

Treat a cloud VM as a headless Linux host:

```sh
npm install -g @opencoven/cli
coven doctor
```

Use [Install from source](/install/from-source) when the VM image does not match
the native npm target.

## Operating model

Run the daemon as a same-user local process and keep state under that user:

```sh
export COVEN_HOME="$HOME/.coven"
```

Use systemd only after manual daemon commands work. Keep the unit environment
explicit and make sure it can find `coven`, `codex`, and `claude`.

Do not expose the daemon socket or any future HTTP surface directly to the
public internet. Use SSH, a private network, or a tunnel you control when you
need remote access.

## Verify

```sh
coven doctor
coven daemon start
coven daemon status
cd /path/to/project
coven run codex "summarize this repo"
coven sessions
```

Check the state from the same SSH user that runs the daemon:

```sh
coven sessions --plain
```

## Related

- [Headless servers](/platforms/headless)
- [Headless server install](/install/headless-server)
- [systemd unit](/install/systemd)
