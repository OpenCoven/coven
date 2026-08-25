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

The packaged loopback-only dashboard is a separate opt-in install on its own
release train. The wrapper stays thin: it depends only on the native binary for
your platform, so a CLI install never pulls the dashboard's application
dependencies. Install it when you want `coven memory open`:

```sh
npm install -g @opencoven/coven-memory-dashboard
coven memory open
```

Without it, `coven memory open` prints this install instruction and exits; every
other Coven command is unaffected.

Upgrading from a wrapper older than 0.4.1 removes a dashboard that arrived as an
implicit dependency, so `coven memory open` stops working until you install the
companion explicitly with the command above. Nothing else changes.

The wrapper passes only the resolved dashboard entrypoint and the current Node
executable to the native CLI. It does not put memory content, daemon transport
proofs, or credentials in the environment.

The core npm wrapper supports Node.js 18 or newer. The dashboard companion
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

If Coven was installed as a direct native binary, the dashboard is found on
`PATH` rather than through the wrapper. The same global install puts it there:

```sh
npm install -g @opencoven/coven-memory-dashboard
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
