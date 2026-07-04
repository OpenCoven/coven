---
summary: "Restore an archived session."
read_when:
  - Looking up summon
title: "coven summon"
description: "Reference for coven summon: restore an archived session, then replay or follow it through the same attach path."
---

## Usage

```bash
coven summon <session-id>
```

Use `coven sessions --all` when you need to find an archived session id.

## Behavior

`coven summon` restores an archived session: it clears archived_at (the stored
`archived_at` timestamp), then continues into the same replay/follow path as
`coven attach`.

If the session was already active, summon does not duplicate it. It still replays
the session output and follows it if the session is live.

Summon does not create a new harness run and does not mutate the event log. It
only changes archive visibility and gives you the same terminal view you would
get from attach.

## Examples

```bash
coven sessions --all --plain
coven summon <session-id>
```

After a summon, the session appears in the default active session list again:

```bash
coven sessions
```

## Related

- [Session lifecycle](/SESSION-LIFECYCLE)
- [Sessions](/reference/cli-sessions)
- [Attach](/reference/cli-attach)
- [Archive](/reference/cli-archive)
