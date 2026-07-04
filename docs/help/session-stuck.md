---
summary: "Recovering live sessions that no longer respond."
read_when:
  - A session stopped responding
title: "A session is stuck"
description: "What to do when a Coven session looks stuck: how to attach, inspect the PTY, read the event log, and decide between archive and sacrifice."
---

A session can look stuck because the harness is still running, the daemon lost
contact with the process, or the local terminal stopped following output.

Start with read-only checks:

```sh
coven daemon status
coven sessions --plain
```

Find the session id, then attach:

```sh
coven attach <session-id>
```

If output resumes, keep using the attached session. If the session already ended,
`attach` should replay the retained event stream and exit.

## Check daemon health

If no sessions update, verify the daemon:

```sh
coven doctor
coven daemon status
```

When the daemon is stale, restart it:

```sh
coven daemon restart
coven daemon status
```

Then list sessions again:

```sh
coven sessions --plain --all
```

## Safe cleanup choices

Archive hides a completed session without deleting events:

```sh
coven archive <session-id>
```

Summon restores an archived session:

```sh
coven summon <session-id>
```

Sacrifice permanently deletes a non-running session and its events. It requires
an explicit confirmation flag:

```sh
coven sacrifice <session-id> --yes
```

Do not sacrifice live work. The CLI refuses running sessions, but you should
still check `coven sessions --plain --all` first.

## When the harness is the issue

If only one harness type gets stuck, run its native setup check:

```sh
codex login
claude doctor
coven doctor
```

Then start a tiny session from a project directory:

```sh
coven run codex "say hello"
```

## What to report

When filing an issue, include:

- `coven --version`
- `coven doctor`
- `coven daemon status`
- The session id prefix and whether `coven attach <session-id>` replays output
- Install method and platform

Redact prompts, paths, and provider account details before sharing.
