---
title: "Operate Coven sessions"
summary: "List, inspect, replay, archive, restore, and deliberately delete project-scoped Coven sessions."
read_when:
  - Managing a recorded harness session
  - Recovering or cleaning up completed work
description: "Examples for Coven session listing, logs, attach, archive, summon, kill, and sacrifice."
---

# Operate Coven sessions

Sessions are the durable record of a harness run. Use the interactive browser for discovery; use explicit commands when scripting or when you already have an id.

## List and identify a session

```sh
coven sessions --plain
coven sessions --all --plain
coven sessions --json
```

The default list omits archived sessions. `--all` includes them. Session-id arguments accept a unique prefix, but prefer the full id in scripts.

Search recorded events or inspect one record:

```sh
coven sessions search "authentication"
coven sessions show session-1
coven sessions events session-1
coven sessions log session-1
```

## Replay or rejoin live work

```sh
coven attach session-1
```

`attach` replays recorded output and follows a live Coven-managed session. It is not a new harness launch. Use the session browser (`coven sessions`) when you want to choose Rejoin or View Log without copying an id.

## Archive and restore finished work

Archive hides a completed session from the default list without deleting its events:

```sh
coven archive session-1
coven sessions --all --plain
```

Restore it with `summon`:

```sh
coven summon session-1
```

`summon` clears the archive state and can replay or follow the selected session. Archive is the reversible cleanup action.

## Stop or delete deliberately

On Unix-like hosts, preserve the event log while ending a running session's
managed process:

```sh
coven kill session-1
```

On Windows, use a named-pipe-capable local integration to request
`POST /api/v1/sessions/:id/kill` from the daemon that owns the session.

For a non-running session you no longer need, permanently delete the record and event history only with explicit confirmation:

```sh
coven sacrifice session-1 --yes
```

There is no undo for sacrifice. Archive first when you only want to remove work from the active list.

## Related

- [Core access](/guides/core-access)
- [Sessions reference](/reference/cli-sessions)
- [Attach reference](/reference/cli-attach)
- [Session lifecycle](/sessions/lifecycle)
