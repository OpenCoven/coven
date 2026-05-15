---
summary: "The on-disk session JSON format that comux and external clients can consume."
read_when:
  - Building a client that replays Coven sessions
  - Designing the comux demo loop
title: "comux JSON sessions"
---

Coven exposes finished sessions as **comux JSON** — a stable record shape that comux, OpenMeow, and external clients can replay without depending on the live PTY.

## Shape

```json
{
  "id": "ses_01HQ...",
  "projectRoot": "/absolute/path",
  "cwd": "/absolute/path/subdir",
  "harness": "codex",
  "title": "Fix the failing tests",
  "status": "completed",
  "exitCode": 0,
  "createdAt": "2026-05-15T19:31:02Z",
  "completedAt": "2026-05-15T19:38:55Z",
  "archived": false,
  "events": [
    { "seq": 1, "type": "output", "ts": "...", "payload": "..." },
    { "seq": 2, "type": "metadata", "ts": "...", "payload": { "title": "..." } },
    { "seq": 3, "type": "exit", "ts": "...", "payload": { "code": 0 } }
  ]
}
```

## Retrieval

<Tabs>
  <Tab title="CLI">
    ```bash
    coven sessions --json
    ```
  </Tab>
  <Tab title="Socket API">
    ```http
    GET /api/v1/sessions/:id
    GET /api/v1/events?sessionId=...
    ```
  </Tab>
</Tabs>

## Guarantees

- **Append-only events.** Events for a given session are never rewritten. New events can be added if the session is reopened by summoning.
- **Stable seq.** The `seq` integer is monotonically increasing within a session and survives daemon restarts.
- **ISO-8601 timestamps.** All `ts` and `*At` fields use UTC ISO-8601 with second precision (or finer).
- **Idempotent ids.** Session ids are ULID-like, lexicographically sortable, and never reused.

## What this format is for

- **Replay** — comux and OpenMeow can render a finished session without touching the live PTY.
- **Audit** — the event log is enough to reconstruct what the harness saw and emitted.
- **Handoff (future)** — Phase 1 orchestration will include this shape in the handoff payload.

## Related

- [Events](/sessions/events)
- [API: sessions endpoints](/reference/api-sessions)
- [API: events endpoint](/reference/api-events)
