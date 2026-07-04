---
summary: "Install Coven inside WSL2 for the full Unix-socket experience."
read_when:
  - Installing on WSL2
title: "WSL2 install"
description: "Install Coven inside WSL2: run the Linux daemon binary, pin COVEN_HOME on the WSL filesystem, and connect Windows clients to the socket."
---

# WSL2 install

Inside WSL2, install Coven as Linux software. Keep Coven, harness CLIs, project files, and `COVEN_HOME` in the WSL environment for the least surprising daemon and PTY behavior.

```sh
npm install -g @opencoven/cli
coven --version
coven doctor
```

The npm wrapper uses the Linux x64 native package when your WSL distribution is glibc-based.

## Recommended layout

Use Linux paths for projects and state:

```sh
mkdir -p "$HOME/code"
cd "$HOME/code"
export COVEN_HOME="$HOME/.coven"
```

Avoid putting active Coven state under `/mnt/c` because Windows filesystem semantics can make socket, permission, and file-watch behavior harder to reason about.

## Harness setup

Install harness CLIs inside WSL2:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Native Windows harness installs do not automatically appear inside WSL2. Run `coven doctor` from WSL after installing harnesses.

## First session

```sh
cd "$HOME/code/project"
coven daemon start
coven daemon status
coven run codex "describe this repo"
coven sessions
```

## WSL2 versus native Windows

Pick one environment for each working session:

- Native Windows: install Coven and harness CLIs in PowerShell or Windows Terminal; use [Windows install](/install/windows).
- WSL2: install Coven and harness CLIs inside the Linux distro; use Linux paths and Linux `COVEN_HOME`.

Do not point native Windows Coven and WSL2 Coven at the same state directory.

## Source fallback

If the npm native package is not available for your WSL distribution:

```sh
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
```

See [Install from source](/install/from-source).
