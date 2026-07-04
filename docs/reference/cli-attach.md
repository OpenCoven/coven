---
summary: "Replay and follow a live session."
read_when:
  - Looking up attach
title: "coven attach"
description: "Reference for coven attach: connect a terminal to a running harness session's PTY, with input forwarding and live event streaming from the daemon."
---

## Usage

```bash
coven attach <session-id>
```

Use `coven sessions` or `coven sessions --plain` first if you need to copy the
full id.

## Behavior

`coven attach` checks the local session store, prints the session status,
harness, and title to stderr, then replays printable output events. For a live
session, it keeps following new output until the session stops.

When stdin is a terminal, attach also forwards input lines to the daemon input
endpoint for that live session. When stdin is non-interactive, the input
forwarder is disabled and attach behaves like a replay/follow log command.

Completed, failed, killed, orphaned, and archived sessions can still be
replayed. They do not accept input because there is no live PTY left to write
to.

## Examples

```bash
coven sessions --plain
coven attach <session-id>

coven run claude "check the failing tests" --detach
coven attach <new-session-id>
```

## Related

- [Session lifecycle](/SESSION-LIFECYCLE)
- [Sessions](/reference/cli-sessions)
- [Daemon lifecycle](/daemon/lifecycle)
- [Session stuck](/help/session-stuck)
