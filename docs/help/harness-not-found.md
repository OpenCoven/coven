---
summary: "How Coven discovers harnesses and what to install."
read_when:
  - coven run says the harness is missing
title: "Harness not found"
description: "Fixes when Coven reports a harness CLI is not found: PATH, npm or pip install location, and version checks for Codex and Claude Code."
---

`coven run` does not install or authenticate model-provider CLIs for you. Coven
looks for supported harness executables on the `PATH` of the same shell that
launches `coven`, then starts those CLIs inside a project-scoped session.

Start with:

```sh
coven doctor
```

In the `Harnesses` section, `ready` means Coven can find the executable. `missing`
means the binary is not visible from this shell, even if another terminal or app
can run it.

## Install a supported harness

Codex:

```sh
npm install -g @openai/codex
codex login
coven doctor
```

Claude Code:

```sh
npm install -g @anthropic-ai/claude-code
claude doctor
coven doctor
```

Install at least one harness before running:

```sh
coven run codex "describe this repo"
```

or:

```sh
coven run claude "describe this repo"
```

## Same-shell PATH checks

Run these from the exact shell where `coven doctor` reports the harness missing:

```sh
command -v codex
command -v claude
node --version
npm --version
```

PowerShell:

```powershell
Get-Command codex
Get-Command claude
node --version
npm --version
```

If install succeeded but `command -v` or `Get-Command` cannot find the binary,
open a new terminal so the shell refreshes `PATH`. Then run:

```sh
coven doctor
```

## Platform notes

- macOS: global npm binaries often live under a Node manager directory such as
  `~/.nvm/.../bin` or an npm prefix. GUI terminals, launchd jobs, and shells may
  not share the same `PATH`.
- Linux and WSL2: install the harness inside the environment where Coven runs.
  A Windows-side Codex install is not visible from WSL2.
- Windows: install and authenticate from the same PowerShell, Windows Terminal,
  or native shell where you will run Coven. If `Get-Command` still fails, inspect
  the npm global prefix with `npm prefix -g`.
- Headless hosts: authenticate the provider CLI over SSH before starting Coven
  under systemd, tmux, or another supervisor.

## Authentication failures

If `coven doctor` says the harness is ready but `coven run` exits immediately,
the binary is visible but the provider CLI is not authenticated. Run the provider
setup command directly:

```sh
codex login
claude doctor
```

Then verify again:

```sh
coven doctor
coven run codex "say hello"
```

## Still failing

Capture the exact `Harnesses` section from `coven doctor`, the output of
`command -v codex` or `Get-Command codex`, and your platform/install method.
Redact home-directory details if needed before filing an issue.
