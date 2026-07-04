---
summary: "Recommended path for Windows users — Coven inside WSL2."
read_when:
  - Bridging Coven into a Windows desktop
title: "WSL2"
description: "Coven inside WSL2: run the Linux daemon binary, pin COVEN_HOME on the WSL filesystem, and connect Windows clients to the same-user Unix socket."
---

## Install path

Inside WSL2, install Coven as Linux software:

```sh
npm install -g @opencoven/cli
coven --version
coven doctor
```

Use the Linux filesystem for projects and state:

```sh
mkdir -p "$HOME/code"
export COVEN_HOME="$HOME/.coven"
```

Avoid active Coven state under `/mnt/c`; Windows filesystem semantics make
socket, permission, and file-watch behavior harder to reason about.

## Environment boundary

Treat WSL2 and native Windows as separate installs. Install `codex`, `claude`,
and any other harness CLI inside the same WSL distro where `coven doctor` runs.

Do not share one `COVEN_HOME` between WSL2 and native Windows.

## Verify

```sh
coven doctor
coven daemon start
coven daemon status
cd "$HOME/code/project"
coven run codex "describe this repo"
coven sessions
```

If the npm native package does not match your distro, use
[Install from source](/install/from-source).

## Related

- [WSL2 install](/install/wsl2)
- [Windows](/platforms/windows)
- [Linux](/platforms/linux)
