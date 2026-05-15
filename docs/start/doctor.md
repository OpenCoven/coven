---
summary: "What coven doctor checks and how to read its output."
read_when:
  - Diagnosing a fresh install or a broken environment
title: "Doctor"
---

`coven doctor` is the first command to run after install. It reports:

- Whether `$COVEN_HOME` is writable.
- Whether the daemon socket can bind.
- Whether `codex` and `claude` are on `PATH` and what version they are.
- Whether the SQLite store is reachable.

Each finding includes a remediation hint. Re-run `coven doctor` after fixing any line marked `needs attention`.
