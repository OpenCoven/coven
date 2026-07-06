---
summary: "Archive a non-running session."
read_when:
  - Looking up archive
title: "coven archive"
description: "Reference for coven archive: hide a non-running session from the active list while preserving its record and append-only event log."
---

## Usage

```bash
coven archive <session-id>
```

Archive is for completed, failed, killed, or orphaned sessions that you want out
of the active list without deleting their history.

## Behavior

`coven archive` refuses a running session. Kill it first with
`coven kill <session-id>`, or let the harness finish before archiving.

For a non-running session, archive sets the session's `archived_at` timestamp and
preserves the session record plus append-only event log. The session disappears
from the default `coven sessions` view, but it is still available through:

```bash
coven sessions --all
coven sessions --all --plain
coven sessions --all --json
```

Use `coven summon <session-id>` when you want the archived session back in the
active list and want to replay it.

Archive is not a privacy delete. If the event log contains data you no longer
want in the local store, use `coven sacrifice <session-id> --yes` after
confirming the session is not running.

## Related

- [Session lifecycle](/SESSION-LIFECYCLE)
- [Sessions](/reference/cli-sessions)
- [Summon](/reference/cli-summon)
- [Sacrifice](/reference/cli-sacrifice)
