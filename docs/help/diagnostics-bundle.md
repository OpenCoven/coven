---
summary: "Generate a redacted diagnostics archive for an issue."
read_when:
  - Filing a useful bug report
title: "Diagnostics bundle"
description: "How to collect a diagnostics bundle for Coven, including daemon logs, doctor output, and harness versions, ready to attach to a GitHub issue."
---

Coven does not currently ship a single `coven diagnostics bundle` command. Until
that exists, collect a small redacted bundle manually from the same shell and
user account that reproduces the issue.

Create a scratch directory:

```sh
mkdir -p coven-diagnostics
coven --version > coven-diagnostics/version.txt
coven doctor > coven-diagnostics/doctor.txt
coven daemon status > coven-diagnostics/daemon-status.txt
```

If you are on macOS and using `coven pc`, include a read-only system snapshot:

```sh
coven pc status --json > coven-diagnostics/pc-status.json
```

If the problem involves sessions, add a plain session list:

```sh
coven sessions --plain > coven-diagnostics/sessions.txt
```

## Include context

Add a short text file with:

- OS and version.
- Install method: npm wrapper, cargo, source checkout, Docker, Podman, Nix,
  launchd, systemd, WSL2, or another route.
- Whether `COVEN_HOME` is default or overridden.
- Which harness you tried: Codex or Claude Code.
- The exact command that failed.

## Redaction checklist

Before sharing the bundle, inspect every file:

```sh
grep -RniE "token|secret|key|password|authorization|bearer" coven-diagnostics || true
```

Remove provider tokens, private prompts, customer data, home-directory details
you do not want to share, and any repository paths that identify confidential
work. Do not attach raw session artifacts unless a maintainer specifically asks
for them.

## Compress

macOS/Linux/WSL2:

```sh
tar -czf coven-diagnostics.tgz coven-diagnostics
```

PowerShell:

```powershell
Compress-Archive -Path coven-diagnostics -DestinationPath coven-diagnostics.zip
```

Attach the archive to a GitHub issue or share the individual redacted text files
in Discord. If you cannot share files, paste only the relevant `coven doctor`
section and the failing command output.
