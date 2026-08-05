---
summary: "Harness-session lifecycle statuses, plus archive visibility, summon, and sacrifice behavior."
read_when:
  - Designing a client that follows session state
  - Debugging a stuck or orphaned session
title: "Session lifecycle for client developers"
description: "Coven harness-session statuses for client developers, including reusable idle sessions, terminal outcomes, archive visibility, summon, and sacrifice."
---

Harness-session rows use the same state vocabulary regardless of which harness is driving them.

```mermaid
stateDiagram-v2
  [*] --> Running: socket/chat POST creates a new row
  [*] --> Created: CLI/detached path inserts row
  Created --> Running: CLI/detached launch begins
  Created --> Failed: stale unowned recovery
  Running --> Failed: runtime launch fails
  Running --> Idle: daemon/socket exits 0 with conversation_id
  [*] --> Created: CLI --continue creates a sibling row
  Running --> Completed: direct CLI exits 0, or daemon/socket exits 0 without conversation_id
  Running --> Completed: external complete with exitCode absent/null/0
  Running --> Failed: harness exits non-zero
  Running --> Failed: external complete with nonzero exitCode
  Running --> Killed: kill dispatch accepted
  Running --> Orphaned: daemon-owned runtime ownership lost
  Created --> [*]: coven sacrifice --yes
  Idle --> [*]: coven sacrifice --yes
  Completed --> [*]: coven sacrifice --yes
  Failed --> [*]: coven sacrifice --yes
  Killed --> [*]: coven sacrifice --yes
  Orphaned --> [*]: coven sacrifice --yes
```

## State definitions

Classify the row kind before interpreting its status. Synthetic `active` rows can appear in raw store or list output, but `active` is not a harness-session state.

| Harness-session status | Ledger-terminal? | Meaning |
|---|---|---|
| `created` | No | Ledger row exists before runtime ownership. Stale unowned `created` rows recover to `failed`. |
| `running` | No | Reported live state. Inspect whether the row is external before inferring Coven runtime ownership. |
| `idle` | No | A daemon/socket-managed, conversation-grouped session (`conversation_id` present) exited successfully and is waiting for more work. CLI and socket/chat continuation create a new row. |
| `completed` | Yes | A direct CLI run persisted a successful harness result (even when `conversation_id` is present), a daemon-managed session without `conversation_id` exited successfully, or an externally registered running session was completed with an absent, `null`, or zero `exitCode`. |
| `failed` | Yes | Launch or execution failed, or an externally registered running session was completed with a nonzero `exitCode`. |
| `killed` | Yes | Terminal in the current ledger. This status is not proof that process termination was acknowledged. |
| `orphaned` | Yes | Runtime ownership was lost and the outcome remains unresolved. |

Archive visibility is stored separately in `archived_at`; it is not a status and therefore does not appear in the lifecycle graph. Archive may set this overlay on any non-running session, including one with `created` or `idle` status, and summon clears it. Both operations preserve the harness-session status.

On a clean exit, only the daemon/socket-managed path in `daemon.rs` converts a successful harness result to `idle`, and only when `conversation_id` is present. A direct CLI run persists the harness result, normally `completed`, even when its row has `conversation_id`; a daemon-managed row without `conversation_id` also persists as `completed` after a clean exit. Automatic CLI `coven run <harness> --continue` selects the latest non-archived row for the same project root and harness. Explicit `coven run <harness> --continue <ID>` performs a direct id or conversation lookup and may select an archived row, but rejects a source from another harness. Both forms create a fresh, initially unarchived sibling row grouped by the selected row's `conversation_id`, or by its ledger id when no conversation id exists. The selected row is never reopened or rewritten, so its status, exit code, archive overlay, and timestamps remain terminal evidence. Socket/chat continuation through `POST /api/v1/sessions` also creates a new row, but shares grouping only when the caller explicitly supplies the same `conversationId`; a resume hint does not derive that id automatically.

For an externally registered `running` session, `POST /api/v1/sessions/:id/complete` records the caller-owned outcome: an absent, `null`, or zero `exitCode` moves the row to `completed`, while a nonzero `exitCode` moves it to `failed`. This completion path does not give the daemon runtime ownership. External running sessions remain exempt from daemon orphan recovery, and daemon input and kill requests remain rejected.

## Launch

```bash
coven run codex "describe this repo"
```

Socket client launch (a separate entry path):

```http
POST /api/v1/sessions
Content-Type: application/json

{
  "projectRoot": "/absolute/path",
  "cwd": "/absolute/path/subdir",
  "harness": "codex",
  "prompt": "describe this repo"
}
```

For this socket path, the daemon inserts a new row as `running` before launching the runtime; a successful request returns `201 SessionRecord`, while a launch failure transitions that new row from `running` to `failed`. The new row shares a prior conversation's grouping only when the caller explicitly supplies the same `conversationId`; a resume hint alone does not derive it. The prior row is unchanged. A new direct CLI `coven run` inserts a `created` row, transitions it to `running`, and launches the harness. CLI continuation follows the same created-to-running path on a fresh sibling row while preserving the selected row exactly. When the harness exits, the direct CLI persists its result, normally `completed` on success, on the new row. The Rust authority layer validates `projectRoot` and `cwd` on each entry path before spawning the PTY. See [Authority boundary](/concepts/authority-boundary).

## Attach

```bash
coven attach <session-id>
```

`attach` streams output from the event log (replay) and then follows live output. Input is forwarded to the PTY. Use `Ctrl-]` to detach without killing the session.

## Archive / summon / sacrifice

These rituals control stored-session visibility and deletion:

<Columns>
  <Card title="Archive" href="/rituals/archive" icon="archive">
    Hide a non-running session. Reversible. Events preserved.
  </Card>
  <Card title="Summon" href="/rituals/summon" icon="moon-star">
    Clear an archived session's visibility overlay and restore it to the active list with its unchanged status, then replay/follow it.
  </Card>
  <Card title="Sacrifice" href="/rituals/sacrifice" icon="flame">
    Permanently delete any non-running session, visible or archived. Requires `--yes`.
  </Card>
</Columns>

## Orphan recovery

When the daemon starts, it recovers stale unowned rows:

1. Reads the session ledger.
2. Marks stale `created` rows as `failed`.
3. Marks stale daemon-owned `running` rows as `orphaned`, preserving their unresolved outcome. Externally registered running sessions are exempt because the daemon does not own their lifecycle.
4. Refuses to re-attach to a PTY it no longer owns.

See [Orphan recovery](/daemon/orphan-recovery).

## Related

- [Events](/sessions/events)
- [comux JSON sessions](/sessions/comux-json)
- [Rituals](/rituals)
- [CLI: coven sessions](/reference/cli-sessions)
