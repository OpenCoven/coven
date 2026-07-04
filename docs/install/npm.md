---
summary: "Install the @opencoven/cli wrapper from npm."
read_when:
  - Using npm or pnpm to install Coven
title: "Install via npm"
description: "Install Coven with npm: run npm install -g @opencoven/cli to fetch the wrapper plus a prebuilt native daemon binary for supported macOS, Linux, and Windows targets."
---

# Install via npm

The fastest workstation install is the universal npm wrapper:

```sh
npm install -g @opencoven/cli
coven --version
coven doctor
```

The wrapper exposes the `coven` command and selects the native package for the current platform.

## Supported npm targets

| Platform | Native package |
| --- | --- |
| macOS Apple Silicon | `@opencoven/cli-macos` |
| glibc-based Linux x64 | `@opencoven/cli-linux-x64` |
| Windows x64 | `@opencoven/cli-windows` |

If the wrapper cannot find the native package, reinstall without disabling optional dependencies:

```sh
npm uninstall -g @opencoven/cli
npm install -g @opencoven/cli
coven doctor
```

On Linux, use a glibc-based distribution for the prebuilt package. For Alpine or another musl-based environment, use [Install from source](/install/from-source).

## Install harness CLIs

Coven supervises existing harness CLIs. Install and authenticate at least one:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Run `coven doctor` again after harness installation. If a harness is still missing, open a new terminal and verify the harness command is on `PATH` in that same shell.

## First run

```sh
cd /path/to/project
coven doctor
coven daemon start
coven run codex "describe this repo"
coven sessions
```

Use Claude Code instead when that is the authenticated harness:

```sh
coven run claude "describe this repo"
```

## Updating

```sh
npm update -g @opencoven/cli
coven daemon restart
coven doctor
```

See [Updating Coven](/install/updating) before updating shared automation hosts or long-running daemon environments.
