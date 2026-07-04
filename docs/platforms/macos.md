---
summary: "Coven on macOS — launchd, accessibility prompts, and Unix-socket behavior."
read_when:
  - Operating on macOS
title: "macOS"
description: "Coven on macOS: supported versions, install options, daemon supervision through launchd, and how COVEN_HOME and the local socket are arranged."
---

## Install path

Use the npm wrapper on Apple Silicon macOS:

```sh
npm install -g @opencoven/cli
coven --version
coven doctor
```

The wrapper selects the native macOS package for Apple Silicon. If you are on a
macOS host that is not covered by the release package, use
[Install from source](/install/from-source).

## Operating model

The daemon is a same-user local process. Keep `COVEN_HOME` on local storage:

```sh
export COVEN_HOME="$HOME/.coven"
```

Start the daemon manually during setup:

```sh
coven daemon start
coven daemon status
```

Use [launchd service](/install/launchd) only after the manual command path works.
The launchd environment must expose the same `coven`, `codex`, and `claude`
commands that `coven doctor` sees in your shell.

## Verify

```sh
coven doctor
coven daemon restart
coven daemon status
cd /path/to/project
coven run codex "describe this repo"
coven sessions
```

If `doctor` reports a missing harness after installation, open a new terminal so
`PATH` refreshes, then run `coven doctor` again.

## Related

- [macOS install](/install/macos)
- [COVEN_HOME layout](/daemon/coven-home)
- [Daemon lifecycle](/daemon/lifecycle)
