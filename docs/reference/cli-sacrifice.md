---
summary: "Permanently delete an eligible non-running session without adoption evidence."
read_when:
  - Looking up sacrifice
title: "coven sacrifice"
description: "Reference for coven sacrifice: the destructive ritual for eligible non-running, unadopted sessions. Retains adopted/reserved evidence and requires --yes."
---

## Usage

```bash
coven sacrifice <session-id> --yes
```

Sacrifice is the permanent delete command for an eligible non-running session
without adopted or historical reserved evidence. It removes that session and
its event log. It is intentionally explicit so a copied session id is not
enough to delete history by accident.

## Safety rules

`coven sacrifice` requires `--yes`. Without it, the command fails and tells you
to rerun with confirmation.

The command also refuses live sessions:

```text
session `<session-id>` is still running; do not sacrifice live work — on Unix-like hosts, kill it first with `coven kill <session-id>`
```

Use `coven attach <session-id>` or `coven daemon status` first if you are not
sure whether the harness is still running. If the session really should stop,
`coven kill <session-id>` ends its process while keeping the event log on
Unix-like hosts. On Windows, use a named-pipe-capable local integration to
request `POST /api/v1/sessions/:id/kill` from the daemon that owns the session.

Eligibility is checked in order. The running/live-work denial above wins for a
running session even when that session also has adoption evidence. Only after
the session is otherwise eligible and non-running does Coven check for a
launch adoption, input adoption, or historical launch-attempt reservation.
That retained evidence returns:

```text
session adoption evidence is retained; sacrifice is unavailable until an approved retention/fence contract resolves it
```

O3 defines no retention expiry or fence-release mechanism. Do not interpret
the message as promising that a release exists.

The final delete remains race-safe. It conditionally deletes only a
non-running row, and the adoption foreign key prevents deletion if retained
evidence appears after preflight; that race is surfaced as the same typed
`AdoptionRetentionError` instead of deleting either record.

## What gets deleted

For an eligible unadopted session, sacrifice deletes the session row from the
local store. Session events are removed with it, so replay, search, and archive
recovery no longer work for that session. Adopted/reserved sessions and their
adoption evidence remain.

Use archive instead when you only want to clean up the active list:

```bash
coven archive <session-id>
```

## Related

- [Session lifecycle](/SESSION-LIFECYCLE)
- [Sessions](/reference/cli-sessions)
- [Attach](/reference/cli-attach)
- [Archive](/reference/cli-archive)
