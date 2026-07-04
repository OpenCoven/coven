---
summary: "Coven on Linux — systemd, AppArmor/SELinux, and socket permissions."
read_when:
  - Operating on Linux
title: "Linux"
description: "Coven on Linux: supported distributions, install options, daemon supervision, and how COVEN_HOME and the local socket interact with systemd."
---

## Install path

Use the npm wrapper on glibc-based Linux x64:

```sh
npm install -g @opencoven/cli
coven --version
coven doctor
```

Alpine and other musl-based systems should use
[Install from source](/install/from-source) until the release matrix includes a
matching native package.

## Operating model

The daemon runs as the same Unix user that owns `COVEN_HOME`:

```sh
export COVEN_HOME="$HOME/.coven"
```

Keep state on the Linux filesystem and avoid shared network mounts for active
daemon state. For always-on use, prove the manual path first, then wrap it with
[systemd](/install/systemd).

## Verify

```sh
coven doctor
coven daemon start
coven daemon status
cd /path/to/project
coven run codex "describe this repo"
coven sessions
```

If systemd cannot find a harness that works in your shell, the unit environment
is missing `PATH` entries. Fix the unit environment rather than sharing state
between users.

## Related

- [Linux install](/install/linux)
- [systemd unit](/install/systemd)
- [COVEN_HOME layout](/daemon/coven-home)
