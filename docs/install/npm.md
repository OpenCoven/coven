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

The wrapper also declares `@opencoven/coven-memory-dashboard` as an optional
companion on its own release train. With optional dependencies enabled, launch
the packaged loopback-only dashboard with:

```sh
coven memory open
```

The wrapper passes only the resolved dashboard entrypoint and the current Node
executable to the native CLI. It does not put memory content, daemon transport
proofs, or credentials in the environment.

The core npm wrapper supports Node.js 18 or newer. The optional dashboard
requires Node.js 24 or newer. On Node.js 18–23, `coven memory open` prints an
upgrade instruction; list output and every other Coven command remain
available.

## Supported npm targets

| Platform | Native package |
| --- | --- |
| macOS Apple Silicon | `@opencoven/cli-macos` |
| Intel macOS x64 | `@opencoven/cli-macos-x64` |
| glibc-based Linux x64 | `@opencoven/cli-linux-x64` |
| Windows x64 | `@opencoven/cli-windows` |

If the wrapper cannot find the native package, reinstall without disabling optional dependencies:

```sh
npm uninstall -g @opencoven/cli
npm install -g @opencoven/cli
coven doctor
```

On Linux, use a glibc-based distribution for the prebuilt package. For Alpine or another musl-based environment, use [Install from source](/install/from-source).

If Coven was installed as a direct native binary, install the dashboard
executable separately and keep it on `PATH`:

```sh
npm install -g @opencoven/coven-memory-dashboard
coven memory open
```

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
