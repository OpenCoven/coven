---
summary: "Install Coven on common Linux distros."
read_when:
  - Installing on Linux
title: "Linux install"
description: "Install Coven on Linux: install the @opencoven/cli wrapper, place the daemon binary on PATH, and verify the install with coven doctor."
---

# Linux install

Use the npm wrapper on glibc-based Linux x64 systems:

```sh
npm install -g @opencoven/cli
coven --version
coven doctor
```

The universal wrapper selects the native Linux x64 package. Alpine and other musl-based environments should use [Install from source](/install/from-source).

## Baseline packages

Install Node.js 18+ for the npm wrapper. Install Git for project-root detection and source checkouts.

Debian or Ubuntu:

```sh
sudo apt-get update
sudo apt-get install -y nodejs npm git ca-certificates
```

Fedora:

```sh
sudo dnf install -y nodejs npm git ca-certificates
```

Arch:

```sh
sudo pacman -S --needed nodejs npm git ca-certificates
```

## Harness setup

Install and authenticate at least one harness CLI:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Run `coven doctor` from the same shell after installing harnesses.

## First session

```sh
cd /path/to/project
coven daemon start
coven daemon status
coven run codex "describe this repo"
coven sessions
```

Use Claude Code instead with:

```sh
coven run claude "describe this repo"
```

## COVEN_HOME

The default state directory is:

```sh
$HOME/.coven
```

Keep it on the Linux filesystem, not a network mount, when possible. To override:

```sh
export COVEN_HOME="$HOME/.local/share/coven"
coven doctor
```

## Server use

For a non-desktop host, start with this page, then read [Headless server](/install/headless-server) for daemon lifecycle and SSH-oriented operation.

## Related

- [Install via npm](/install/npm)
- [WSL2 install](/install/wsl2)
- [Install from source](/install/from-source)
