---
summary: "All ways to install Coven on a workstation or server."
read_when:
  - Choosing how to install Coven
title: "Install overview"
description: "Install overview for Coven: pick a platform and a method (npm, cargo, Docker, Nix, source) and verify the daemon with coven doctor."
---

# Install overview

Use this page to pick the right Coven install path, then verify the setup the same way on every platform:

```sh
coven doctor
coven daemon start
coven daemon status
```

After the daemon is running, launch the first session from a project directory:

```sh
cd /path/to/project
coven run codex "describe this repo"
```

Or use Claude Code:

```sh
coven run claude "describe this repo"
```

## Choose your route

| Environment | Recommended path | Notes |
| --- | --- | --- |
| macOS Apple Silicon | [npm wrapper](/install/npm) or [macOS install](/install/macos) | Uses the universal `@opencoven/cli` package and the native macOS package. |
| glibc-based Linux x64 | [npm wrapper](/install/npm) or [Linux install](/install/linux) | Alpine/musl is not part of the npm binary target today; build from source there. |
| Windows x64 | [Windows install](/install/windows) | Run Coven and harness CLIs from the same PowerShell, Windows Terminal, or native Windows shell. |
| WSL2 | [WSL2 install](/install/wsl2) | Treat WSL2 as a Linux environment and keep `COVEN_HOME` on the WSL filesystem. |
| Contributor checkout | [Install from source](/install/from-source) | Use this for unreleased changes and local development. |
| Rust-first install | [Install via cargo](/install/cargo) | Build the Rust CLI yourself and put the binary on `PATH`. |
| Server or automation host | [Headless server](/install/headless-server) | Use daemon commands over SSH or supervisor-managed shells. |
| macOS background service | [launchd service](/install/launchd) | Optional user-agent wrapper around `coven daemon start`. |
| Linux background service | [systemd unit](/install/systemd) | Optional user service wrapper around `coven daemon start`. |
| Raspberry Pi | [Raspberry Pi](/install/raspberry-pi) | Build from source on arm64 and keep state on persistent storage. |
| Container experiments | [Docker](/install/docker) or [Podman](/install/podman) | Build your own image; bind-mount state and project roots explicitly. |
| Nix-managed shell | [Nix](/install/nix) | Use Nix to pin prerequisites, then build or run Coven inside that shell. |

## Baseline requirements

- Node.js 18+ for the npm wrapper path.
- Git for source checkouts and project-root detection.
- Rust stable only when building from source or with cargo.
- At least one supported harness CLI on `PATH`: Codex or Claude Code.

Install and authenticate a harness before expecting `coven run` to launch work:

```sh
npm install -g @openai/codex
codex login
```

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
```

Then run:

```sh
coven doctor
```

`doctor` reports store readiness, daemon/socket status, project-root hints, and whether supported harness CLIs are available from the same shell.

## State directory

By default, Coven stores local state under `<home>/.coven`. Override it only when you need a separate state root:

```sh
export COVEN_HOME="$HOME/.coven"
coven doctor
```

PowerShell:

```powershell
$env:COVEN_HOME="$env:USERPROFILE\.coven"
coven doctor
```

See [COVEN_HOME layout](/daemon/coven-home) for what lives inside that directory.

## Common verification loop

Use the same loop after install, after updates, and after changing harness auth:

```sh
coven --version
coven doctor
coven daemon restart
coven daemon status
cd /path/to/project
coven run codex "say hello from Coven"
coven sessions
```

If `doctor` reports a missing harness after installation, open a new terminal so `PATH` refreshes, then run `coven doctor` again from the shell where you will use Coven.

## Related

- [Getting started](/GETTING-STARTED)
- [Quickstart](/start/quickstart)
- [Troubleshooting](/TROUBLESHOOTING)
- [CLI reference](/reference/cli)
