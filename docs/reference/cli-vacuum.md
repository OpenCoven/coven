---
summary: "Repair and compact the local Coven session store."
read_when:
  - Looking up vacuum
  - Repairing the session store after corruption
title: "coven vacuum"
description: "Reference for coven vacuum: rebuild the session event index, run a SQLite integrity check, and compact the local Coven store."
---

`coven vacuum` repairs and compacts the local session store
(`<covenHome>/coven.sqlite3`): it rebuilds the full-text event index when
needed and runs a SQLite integrity check.

```sh
coven vacuum
```

```text
Coven store: vacuumed (event index rebuilt, integrity ok, path /home/alex/.coven/coven.sqlite3)
```

Run it when `coven sessions search` misbehaves or after unclean shutdowns.
The command is safe to run while the daemon is stopped; prefer stopping the
daemon first for large repairs.

The daemon exposes the same operation as `POST /api/v1/store/vacuum`, which
returns `{ ok, eventIndexRebuilt, integrityCheck }` — see the
[API reference](api.md).

For retention pruning of raw artifacts and old events, see
[cli-logs](cli-logs.md).
