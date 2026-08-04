---
summary: "Repository-wide exclusion for destructive maintenance."
read_when:
  - Coordinating a worktree or branch deletion with live Coven sessions
  - Integrating a maintenance client such as Coven Cave
title: "coven maintenance"
description: "Reference for the fenced maintenance gate shared by Coven sessions and claim mutations."
---

`coven maintenance` creates a repository-wide exclusion window below the Git
common directory. Unlike a worktree-local lock, every Coven worktree, direct
CLI session, and daemon-backed session uses the same writer-intent registry.

```sh
# Start a fence. Keep both values: generation is required to renew or release.
coven maintenance acquire cave-delete-42 --wait-ms 5000 --json

# Before each destructive boundary, renew and require phase=held.
coven maintenance heartbeat cave-delete-42 <generation> --json

# After post-verification, release the exact fence.
coven maintenance release cave-delete-42 <generation>
```

The acquire result is either `held` (no writer remains) or `draining` with a
list of active writer intents. Publishing `draining` already rejects new
writers, allowing the owner to wait for previously-started sessions and claim
mutations without admitting another one. Owners are fenced by a random
generation and an expiry deadline. Heartbeat and release reject a different or
expired generation; malformed owner or writer records also fail closed.

Direct `coven run`, `coven patch`, daemon `POST /sessions`, and claim
`acquire`, `release`, `heartbeat`, and `canary` register renewable writer
intents before their mutation/launch path. A daemon session retains the intent
until its child exits. Session launch errors caused by an owner are returned as
HTTP `423 maintenance_locked` with structured owner details for clients.

The protocol coordinates one shared Git common directory. It cannot stop raw
Git commands, uninstrumented harnesses, or a separate clone on another host;
those callers must use a supported Coven launch path (or the same maintenance
protocol) to participate.
