---
summary: "What to include in a Coven GitHub issue."
read_when:
  - Filing a Coven issue
title: "Filing issues"
description: "How to file a useful Coven bug report: what to include from coven doctor, the daemon logs, harness versions, and the relevant session record."
---

File issues at the Coven repository when you can reproduce a bug, docs mismatch,
or install failure. Use GitHub Discussions or Discord for exploratory questions.

Before filing, collect the current local state:

```sh
coven --version
coven doctor
coven daemon status
```

If the issue involves sessions:

```sh
coven sessions --plain --all
```

If it involves a harness:

```sh
command -v codex
command -v claude
```

PowerShell:

```powershell
Get-Command codex
Get-Command claude
```

## Include

- Platform: macOS, Linux, Windows, WSL2, container, Raspberry Pi, or headless
  server.
- Install route: npm wrapper, cargo, source checkout, Docker, Podman, Nix,
  launchd, systemd, or another supervisor.
- The exact command that failed.
- Expected behavior and actual behavior.
- Whether `coven doctor` reports missing harnesses, stopped daemon, running
  daemon, or stale daemon.
- A redacted diagnostics bundle from [Diagnostics bundle](/help/diagnostics-bundle)
  when the issue is not obvious from the command output.

## Redact

Do not include provider tokens, private prompts, customer data, private repo URLs,
or raw session artifacts unless a maintainer asks for a narrow file. If a path is
sensitive, replace the prefix with `<project>` or `<home>`.

## Good issue title examples

- `npm install succeeds on Windows but coven doctor cannot find coven.exe`
- `coven daemon restart leaves stale status under systemd user service`
- `WSL2 install guide should warn about COVEN_HOME on /mnt/c`

## Where to file

Use GitHub Issues for reproducible bugs:

```text
https://github.com/OpenCoven/coven/issues
```

For setup uncertainty, ask in Discord first and include the same `coven doctor`
summary.
