---
summary: "Kill a running session's process."
read_when:
  - Looking up kill
title: "coven kill"
description: "Reference for coven kill: stop a running session's harness process through the daemon while keeping the session record and its event log."
---

## Usage

```bash
coven kill <session-id>
```

Kill stops a running session's harness process. The daemon (which owns the
process) performs the kill, marks the session `killed`, and records a kill
event. The session record and its append-only event log are kept.

## Behavior

`coven kill` only acts on running sessions. For anything else it refuses:

```text
session `<session-id>` is not running (status: completed); only running sessions can be killed
```

After a kill, the session shows up as `killed` in `coven sessions`. Clean it up
like any other finished session:

```bash
coven archive <session-id>
coven sacrifice <session-id> --yes
```

## Stale "running" sessions

A session whose process died externally keeps `status=running` until daemon
startup recovery marks it orphaned. If `coven kill` cannot reach a live
process for the session, run:

```bash
coven daemon restart
```

then check `coven sessions` again.

## Related

- [Session lifecycle](/SESSION-LIFECYCLE)
- [Sessions](/reference/cli-sessions)
- [Attach](/reference/cli-attach)
- [Archive](/reference/cli-archive)
- [Sacrifice](/reference/cli-sacrifice)
