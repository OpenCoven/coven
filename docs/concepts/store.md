---
summary: "Coven's local SQLite database: session ledger plus append-only event log."
read_when:
  - Inspecting Coven state on disk or recovering after a crash
title: "Store"
---

The store lives under `$COVEN_HOME` and holds two logical tables:

- **Sessions** — id, project root, harness, status, exit code, archive state, timestamps.
- **Events** — append-only output/exit/metadata records keyed by session id.

Do not commit `.coven/`, databases, sockets, logs, or environment files to source control.
