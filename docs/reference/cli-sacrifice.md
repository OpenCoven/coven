---
summary: "Permanently delete a non-running session."
read_when:
  - Looking up sacrifice
title: "coven sacrifice"
description: "Reference for coven sacrifice: the destructive ritual that removes a session record and its events. Refuses live sessions and requires --yes."
---

## Usage

```bash
coven sacrifice <session-id> --yes
```

Sacrifice is the permanent delete command for a non-running session and its
event log. It is intentionally explicit so a copied session id is not enough to
delete history by accident.

## Safety rules

`coven sacrifice` requires `--yes`. Without it, the command fails and tells you
to rerun with confirmation.

The command also refuses live sessions:

```text
session `<session-id>` is still running; do not sacrifice live work — kill it first with `coven kill <session-id>`
```

Use `coven attach <session-id>` or `coven daemon status` first if you are not
sure whether the harness is still running. If the session really should stop,
`coven kill <session-id>` ends its process while keeping the event log.

## What gets deleted

Sacrifice deletes the session row from the local store. Session events are
removed with it, so replay, search, and archive recovery no longer work for that
session.

Use archive instead when you only want to clean up the active list:

```bash
coven archive <session-id>
```

## Related

- [Session lifecycle](/SESSION-LIFECYCLE)
- [Sessions](/reference/cli-sessions)
- [Attach](/reference/cli-attach)
- [Archive](/reference/cli-archive)
