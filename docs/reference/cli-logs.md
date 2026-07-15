---
summary: "Prune raw artifacts and old redacted event logs."
read_when:
  - Looking up logs prune
  - Tuning local log retention
title: "coven logs"
description: "Reference for coven logs prune: retention windows for raw artifacts and redacted session events, dry-run reporting, and the COVEN_* environment overrides."
---

Session events are stored redacted; sensitive raw artifacts are kept
separately with a short retention window. `coven logs prune` applies both
retention windows to the local store.

```sh
coven logs prune --dry-run     # report what would be pruned
coven logs prune               # prune expired rows
```

## Retention windows

| Data | Default | Flag | Environment override |
| --- | --- | --- | --- |
| Raw artifacts | 7 days | `--raw-days <N>` | `COVEN_RAW_ARTIFACT_RETENTION_DAYS` |
| Redacted events | 30 days | `--event-days <N>` | `COVEN_LOG_RETENTION_DAYS` |

Flags win over configuration; both windows are clamped to at least 1 day.
Defaults can also be set in the privacy section of the Coven settings — see
[SETTINGS](../SETTINGS.md).

## Output

```text
$ coven logs prune --dry-run
logs prune dryRun=true rawArtifacts=3 events=120 rawDays=7 eventDays=30

$ coven logs prune
logs pruned rawArtifacts=3 events=120 rawCutoff=2026-07-08T... eventCutoff=2026-06-15T...
```

For store repair and compaction (rebuilding the event index, integrity
checks), see [cli-vacuum](cli-vacuum.md).
