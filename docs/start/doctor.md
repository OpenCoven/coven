---
summary: "What coven doctor checks and how to read its output."
read_when:
  - Diagnosing a fresh install or a broken environment
title: "Doctor"
description: "Run coven doctor after install. It reports local readiness without launching providers, contacting provider networks, or verifying provider authentication."
---

`coven doctor` is the first command to run after install. It reports:

- Whether `$COVEN_HOME` is writable.
- Whether the daemon socket can bind.
- Whether `codex`, `claude`, and `copilot` are on `PATH`.
- Whether the SQLite store is reachable.

Doctor is offline and hermetic: it launches no provider CLI process, performs
no provider network request, does not inspect provider tokens or credential
stores, and does not verify authentication. External harness credential rows
are advisory even when an executable is present.

Each finding includes a remediation hint. Missing or unverified harnesses point
to `coven setup`, where provider-owned login and optional verification require
explicit consent. Re-run `coven doctor` after fixing any line marked
`needs attention`.
