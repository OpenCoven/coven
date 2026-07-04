---
summary: "How to update Coven and what release channels exist."
read_when:
  - Moving to a newer version of Coven
title: "Updating Coven"
description: "Update Coven safely: upgrade the @opencoven/cli wrapper, drain or stop the daemon, migrate the store, and verify the contract version on restart."
---

# Updating Coven

Update the wrapper or binary, restart the daemon, then run the same verification loop you used at install time.

## npm wrapper

```sh
npm update -g @opencoven/cli
coven --version
coven daemon restart
coven doctor
```

If the native package is missing after update, reinstall the wrapper without disabling optional dependencies:

```sh
npm uninstall -g @opencoven/cli
npm install -g @opencoven/cli
coven doctor
```

## Source checkout

```sh
cd /path/to/coven
git pull --ff-only
cargo build -p coven-cli --release
cp target/release/coven "$HOME/.local/bin/coven"
coven daemon restart
coven doctor
```

On Windows, copy `target\release\coven.exe` to the directory where your shell resolves `coven`.

## Harness updates

Coven supervises harness CLIs; it does not own their provider credentials. Update and verify harnesses separately:

```sh
npm update -g @openai/codex
codex login
```

```sh
npm update -g @anthropic-ai/claude-code
claude doctor
```

Then run:

```sh
coven doctor
```

## Verification loop

```sh
coven --version
coven doctor
coven daemon restart
coven daemon status
cd /path/to/project
coven run codex "say hello from the updated Coven install"
coven sessions
```

Use `coven run claude ...` when Claude Code is your active harness.

## Rollback notes

Before changing package source, binary location, or `COVEN_HOME`, stop the daemon:

```sh
coven daemon stop
```

If an update leaves the daemon unreachable, run:

```sh
coven daemon restart
coven doctor
```

If `doctor` points at a `PATH` problem, open a new shell and verify:

```sh
command -v coven
command -v codex
command -v claude
```

PowerShell:

```powershell
Get-Command coven
Get-Command codex
Get-Command claude
```

## Related

- [Install overview](/install/index)
- [Troubleshooting](/TROUBLESHOOTING)
- [Daemon lifecycle](/daemon/lifecycle)
