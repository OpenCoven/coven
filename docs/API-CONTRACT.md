---
title: "Coven local API contract (coven.daemon.v1)"
summary: "The versioned coven.daemon.v1 contract under /api/v1: health negotiation, capability discovery, error envelopes, and additive compatibility rules."
read_when:
  - Pinning a client to the daemon contract
  - Checking stable API shapes and error envelopes
description: "The versioned coven.daemon.v1 contract under /api/v1: health negotiation, capability discovery, error envelopes, and additive compatibility rules."
---

# Coven local API contract

The Coven daemon socket API is a public compatibility boundary for comux and external clients such as external OpenClaw bridge plugin.

## Current stable version

- `GET /api/v1/health` exposes `apiVersion: "coven.daemon.v1"`, `covenVersion`, and a machine-readable `capabilities` object.
- Clients should read `/api/v1/health` before assuming any response shape from other endpoints.
- Legacy unversioned routes such as `GET /health` remain early-MVP aliases; new clients should use `/api/v1`.
- Control-plane clients should discover capabilities before sending action ids.
- All API failures are returned as structured `{ "error": { "code", "message", "details" } }` envelopes.
- Events include a monotonic `seq` cursor for incremental reads.
- Event payloads are redacted by default before API display.

## `GET /api/v1/health`

`GET /api/v1/health` returns daemon reachability, the named contract version, coven version, and machine-readable capabilities:

```json
{
  "ok": true,
  "apiVersion": "coven.daemon.v1",
  "covenVersion": "0.0.0",
  "capabilities": {
    "sessions": true,
    "events": true,
    "travel": true,
    "scheduler": true,
    "eventCursor": "sequence",
    "structuredErrors": true
  },
  "daemon": {
    "pid": 12345,
    "startedAt": "2026-05-09T06:43:00Z",
    "socket": "/Users/alice/.coven/coven.sock"
  }
}
```

If the daemon metadata is unavailable, `daemon` may be `null`.

### Capability fields

| Field             | Type    | Description                                                       |
|-------------------|---------|-------------------------------------------------------------------|
| `sessions`        | boolean | Sessions API (`/sessions`, `/sessions/:id`) is available.        |
| `events`          | boolean | Events API (`/events`) is available.                             |
| `travel`          | boolean | Travel profile, delta, and state APIs are available.             |
| `scheduler`       | boolean | Scheduler decision and recovery APIs are available.              |
| `eventCursor`     | string  | Cursor type supported; `"sequence"` means `afterSeq` is stable.  |
| `structuredErrors`| boolean | All errors use the `{ error: { code, message, details } }` shape.|

## Structured error envelope

```mermaid
flowchart TD
  Req[Incoming request] --> Parse{Parse + version check}
  Parse -- bad shape --> ErrInvalid["400 invalid_request"]
  Parse -- unknown version --> ErrInvalid
  Parse -- ok --> Route{Route exists?}
  Route -- no --> ErrNotFound["404 not_found"]
  Route -- yes --> Validate{Field validation}
  Validate -- cwd outside root --> ErrInvalid
  Validate -- unknown harness/action --> ErrInvalid
  Validate -- ok --> Action{Resource lookup}
  Action -- session missing --> ErrSession["404 session_not_found"]
  Action -- session not live --> ErrLive["409 session_not_live"]
  Action -- launch (PTY/pipe spawn, init write, harness startup) fails --> ErrLaunch["500 launch_failed"]
  Action -- send_input fails --> ErrSend["500 send_input_failed"]
  Action -- kill_session fails --> ErrKill["500 kill_failed"]
  Action -- runtime down --> ErrRuntime["503 runtime_unavailable"]
  Action -- internal panic --> ErrInternal["500 internal_error"]
  Action -- ok --> Success[Documented success shape]

  ErrInvalid & ErrNotFound & ErrSession & ErrLive & ErrLaunch & ErrSend & ErrKill & ErrRuntime & ErrInternal -->|"{ error: { code, message, details } }"| Client[Client branches on code]
```

All API errors use the following stable envelope. Clients must branch on `error.code`, not `error.message`:

```json
{
  "error": {
    "code": "session_not_found",
    "message": "Session was not found.",
    "details": {
      "sessionId": "abc-123"
    }
  }
}
```

`details` is optional and included when extra context is useful.

### Stable error codes

| Code                   | HTTP status | Description                                      |
|------------------------|-------------|--------------------------------------------------|
| `not_found`            | 404         | Generic route not found.                         |
| `invalid_request`      | 400 or 404  | Malformed request, unknown harness id, missing required field, or unsupported API version. |
| `session_not_found`    | 404         | Session id does not exist.                       |
| `session_not_live`     | 409         | Session exists but is not running.               |
| `project_root_violation`| 400        | Reserved. Cwd-outside-root currently emits `invalid_request` with the violation message in the body; promoting to its own code would let clients branch without parsing prose. |
| `pty_spawn_failed`     | 500         | Reserved. PTY spawn failures currently emit `launch_failed`; promoting to its own code would let clients distinguish "the PTY couldn't open" (likely a host issue) from "the harness CLI errored at startup" (likely an auth/config issue). |
| `launch_failed`        | 500         | Daemon accepted the launch payload but the runtime (PTY/pipe spawn, initial-message write, harness CLI startup) failed. `details.sessionId` is the row that was inserted and marked `failed`. |
| `send_input_failed`    | 500         | Daemon accepted the input payload but the runtime write failed (closed pipe, killed process, IO error). `details.sessionId` is the affected session. |
| `kill_failed`          | 500         | Daemon accepted the kill request but the runtime signal/kill call failed (permission, missing process, IO error). `details.sessionId` is the affected session. |
| `runtime_unavailable`  | 503         | The session runtime is unavailable.              |
| `internal_error`       | 500         | Unexpected internal error.                       |
| `raw_artifacts_disabled` | 403       | Raw artifact retrieval was requested without explicit raw artifact persistence enabled. |
| `raw_artifact_requires_raw_flag` | 400 | Raw artifact retrieval omitted the required `raw=1` query flag. |
| `artifact_not_found`   | 404         | Sensitive artifact id does not exist for the session. |
| `travel_profile_not_found` | 404     | Travel profile id does not exist.                |
| `travel_profile_expired` | 409       | Travel profile is expired and cannot accept deltas. |
| `source_hub_mismatch`  | 409         | Delta source hub does not match the travel profile source hub. |
| `no_scheduler_target`  | 409         | No available scheduler node matches the requested capabilities and policy. |
| `scheduler_decision_not_found` | 404 | Scheduler decision id does not exist.            |
| `scheduler_loop_not_found` | 404     | Scheduler loop state does not exist.             |

## Capability catalog shape (`v1`)

`GET /api/v1/capabilities` returns the daemon/control-plane capability catalog. This is the intended intake-client handshake for deciding which actions to show or route through Coven.

```json
{
  "capabilities": [
    {
      "id": "coven.control.actions",
      "label": "Coven control-plane action router",
      "adapter": "coven-daemon",
      "status": "available",
      "policy": "allow",
      "actions": ["coven.capabilities.refresh"]
    },
    {
      "id": "coven.travel",
      "label": "Travel profiles and offline delta reconciliation",
      "adapter": "coven-daemon",
      "status": "available",
      "policy": "allow",
      "actions": []
    },
    {
      "id": "coven.scheduler",
      "label": "Multi-host scheduler decisions and recovery",
      "adapter": "coven-daemon",
      "status": "available",
      "policy": "allow",
      "actions": []
    },
    {
      "id": "desktop.automation",
      "label": "Desktop automation adapters",
      "adapter": "desktop-use",
      "status": "planned",
      "policy": "requiresApproval",
      "actions": []
    }
  ]
}
```

Known enum values in `v1`:

- `status`: `available`, `planned`
- `policy`: `allow`, `requiresApproval`

Clients should ignore unknown future capability ids and action ids unless they explicitly support them.

## Control action shape (`v1`)

`POST /api/v1/actions` accepts a policy-shaped action envelope. The daemon validates the action id before any adapter work is allowed.

```json
{
  "action": "coven.capabilities.refresh",
  "origin": "external-client",
  "intentId": "intent-1",
  "args": {}
}
```

Immediately completed safe actions return `200`:

```json
{
  "ok": true,
  "accepted": true,
  "action": "coven.capabilities.refresh",
  "status": "completed",
  "event": {
    "kind": "capabilities.refreshed",
    "action": "coven.capabilities.refresh",
    "origin": "external-client",
    "intentId": "intent-1",
    "payload": { "capabilities": 5 }
  }
}
```

Unknown action ids return `400` and fail closed:

```json
{
  "ok": false,
  "accepted": false,
  "action": "desktop.deleteEverything",
  "status": "rejected",
  "reason": "unknown action `desktop.deleteEverything`"
}
```

## Session record shape (`v1`)

In `v1`, session responses stay as raw JSON objects using the Rust daemon's snake_case field names.

Endpoints that return this shape:

- `GET /api/v1/sessions` → `SessionRecord[]`
- `POST /api/v1/sessions` → `SessionRecord`
- `GET /api/v1/sessions/:id` → `SessionRecord`

```json
{
  "id": "session-1",
  "project_root": "/repo",
  "harness": "codex",
  "title": "Fix the tests",
  "status": "running",
  "exit_code": null,
  "archived_at": null,
  "created_at": "2026-05-09T06:43:00Z",
  "updated_at": "2026-05-09T06:43:05Z"
}
```

## Event record shape and cursor pagination (`v1`)

`GET /api/v1/events` returns a paginated envelope with monotonic `seq` cursors. `GET /api/v1/sessions/:id/events` is the session-scoped alias with the same response shape and cursor query parameters except that `sessionId` comes from the path.

### Query parameters

| Parameter     | Required | Description                                             |
|---------------|----------|---------------------------------------------------------|
| `sessionId`   | Yes      | Session to fetch events for.                           |
| `afterSeq`    | No       | Return only events with `seq > afterSeq` (preferred).  |
| `afterEventId`| No       | Compatibility cursor — resolves to a sequence position.|
| `limit`       | No       | Maximum number of events to return (daemon-enforced, max 1000). |

### Response envelope

```json
{
  "events": [
    {
      "seq": 42,
      "id": "event-uuid",
      "session_id": "session-uuid",
      "kind": "output",
      "payload_json": "{\"data\":\"hello\"}",
      "created_at": "2026-05-09T06:43:10Z"
    }
  ],
  "nextCursor": {
    "afterSeq": 42
  },
  "hasMore": false
}
```

`nextCursor` is `null` when there are no events. `hasMore` is `true` when a `limit` was applied and more events may exist.

`payload_json` is the redacted preview payload used by clients. Raw sensitive artifacts are never included in this envelope.

## Log preview shape (`v1`)

`GET /api/v1/sessions/:id/log` currently returns the full redacted log preview for the session as an unbounded array:

```json
[
  {
    "ts": "2026-05-09T06:43:10Z",
    "level": "info",
    "message": "> hello"
  }
]
```

## Travel mode profile and delta shapes (`v1`)

Travel mode lets a same-user local client export a bounded, read-only working profile for laptop/offline work and later reconcile appended results back into the hub store. It is additive to sessions/events: uploaded offline events are persisted as ordinary redacted event-log entries on a reconciliation session.

### `POST /api/v1/travel/profiles`

Request:

```json
{
  "familiarId": "sage",
  "workspaceId": "workspace-1",
  "expiresInSeconds": 604800,
  "staleAfterSeconds": 172800
}
```

`familiarId` is required. `workspaceId` defaults to `"default"`. Expiry values must be positive when supplied; defaults are 7 days for `expiresInSeconds` and 2 days for `staleAfterSeconds`, capped at the expiry.

Response `201`:

```json
{
  "profileId": "travel_...",
  "version": "0.1",
  "generatedAt": "2026-07-04T12:00:00Z",
  "expiresAt": "2026-07-11T12:00:00Z",
  "staleAfter": "2026-07-06T12:00:00Z",
  "sourceHub": {
    "hubId": "hub_...",
    "displayName": "Coven hub"
  },
  "scope": {
    "familiarId": "sage",
    "workspaceId": "workspace-1"
  },
  "sourceRevision": {
    "memoryRevision": "mem_...",
    "loopRevision": "loop_..."
  },
  "permissions": {
    "mode": "travel-read-only",
    "allowedLocalAgents": ["lightweight"],
    "allowMemoryOverwrite": false,
    "allowHeavyweightLocalWork": false
  },
  "encoding": "gzip+base64",
  "contentHash": "sha256:...",
  "profileBlob": "..."
}
```

The daemon also writes a gzip profile artifact under `<covenHome>/travel/profiles/` and marks it read-only. The profile payload may include familiar memory context for the requested familiar; clients must treat it as a snapshot, not a write target.

### `POST /api/v1/travel/deltas`

Request:

```json
{
  "profileId": "travel_...",
  "sourceHubId": "hub_...",
  "sourceRevision": {
    "memoryRevision": "mem_...",
    "loopRevision": "loop_..."
  },
  "clientId": "laptop-1",
  "events": [
    { "id": "local-event-1", "kind": "assistant", "text": "offline result" }
  ],
  "artifacts": [
    { "id": "artifact-1", "kind": "summary" }
  ],
  "proposedMemoryAdditions": [
    { "path": "MEMORY.md", "text": "append this" }
  ]
}
```

`profileId`, `sourceHubId`, and `clientId` are required. Query `state` may be `handoff_pending`, `syncing_delta`, or `hub_resumed`; omitted state defaults to `hub_resumed`. Query `defer=1` is a compatibility alias for `state=handoff_pending`.

Response `202`:

```json
{
  "deltaId": "delta_...",
  "state": "hub_resumed",
  "acceptedEvents": 1,
  "acceptedArtifacts": 1,
  "memoryReviewState": "queued",
  "canonicalMemoryOverwriteApplied": false,
  "reconciliationSessionId": "travel-delta_...",
  "hubRevision": {
    "memoryRevision": "mem_...",
    "loopRevision": "loop_..."
  }
}
```

The daemon appends offline events as `travel.offline_event` and offline artifacts as `travel.offline_artifact` entries on the reconciliation session. Proposed memory additions are queued for review; canonical memory overwrite is never applied by this endpoint.

### `GET /api/v1/travel/state`

Query parameters:

| Parameter   | Required | Description                                      |
|-------------|----------|--------------------------------------------------|
| `clientId`  | Yes      | Client whose latest travel delta state is read. |
| `profileId` | No      | Profile to evaluate before any delta exists.    |

Response `200`:

```json
{
  "state": "travel_local",
  "profileId": "travel_...",
  "pendingDeltaBytes": 0,
  "lastSyncError": null,
  "hubReachable": false,
  "profileFreshness": "fresh",
  "travelExecutionAllowed": true,
  "validStates": [
    "hub_active",
    "travel_local",
    "travel_stale",
    "handoff_pending",
    "syncing_delta",
    "hub_resumed"
  ]
}
```

`profileFreshness` is `fresh`, `stale`, `expired`, `none`, or `unknown`. Expired profiles return `travelExecutionAllowed: false`; local clients should fail closed when that flag is false.

## Scheduler decision and recovery shapes (`v1`)

The scheduler routes multi-host work across local laptop, stationary, hub, and compute executor roles. Decisions are stored so clients can inspect prior routing and recover loop state after daemon restart.

### `POST /api/v1/scheduler/decisions`

Request:

```json
{
  "jobId": "job-gpu-loop",
  "requiredCapabilities": ["gpu", "long-running-loop"],
  "taskWeight": "heavyweight",
  "travelState": "hub_active",
  "allowHeavyweightLocalWork": false,
  "nodes": [
    {
      "nodeId": "node-compute-idle",
      "role": "compute_executor",
      "available": true,
      "capabilities": ["gpu", "long-running-loop"],
      "queuePressure": 1
    }
  ]
}
```

Response `201`:

```json
{
  "decisionId": "sched_...",
  "jobId": "job-gpu-loop",
  "target": {
    "role": "compute_executor",
    "nodeId": "node-compute-idle"
  },
  "reason": "compute_executor has required capability set and low queue pressure",
  "inputs": {
    "requiredCapabilities": ["gpu", "long-running-loop"],
    "queuePressure": "low",
    "travelState": "hub_active",
    "taskWeight": "heavyweight"
  },
  "createdAt": "2026-07-04T12:00:00Z"
}
```

The daemon filters unavailable nodes, required capability misses, low-battery `laptop_local` nodes during travel, and heavyweight laptop-local work while `travelState` is `travel_local` or `travel_stale` unless explicitly allowed.

### `GET /api/v1/scheduler/decisions/:id`

Returns the same shape as `POST /api/v1/scheduler/decisions` for a persisted decision, or `404 scheduler_decision_not_found`.

### `POST /api/v1/scheduler/redispatch`

Request:

```json
{
  "loopId": "loop-gpu",
  "jobId": "job-gpu-loop",
  "currentNodeId": "compute-primary",
  "requiredCapabilities": ["gpu", "long-running-loop"],
  "loopResumable": true,
  "nodes": [
    {
      "nodeId": "compute-primary",
      "role": "compute_executor",
      "available": false,
      "capabilities": ["gpu", "long-running-loop"],
      "queuePressure": 3,
      "queuedJobIds": ["job-gpu-loop"]
    },
    {
      "nodeId": "compute-fallback",
      "role": "compute_executor",
      "available": true,
      "capabilities": ["gpu", "long-running-loop"],
      "queuePressure": 1
    }
  ]
}
```

Response `202`:

```json
{
  "decisionId": "sched_...",
  "state": "redispatched",
  "loopId": "loop-gpu",
  "jobId": "job-gpu-loop",
  "target": {
    "role": "compute_executor",
    "nodeId": "compute-fallback"
  },
  "reason": "compute-primary went offline; redispatched resumable loop to compute-fallback",
  "preservedSubqueue": {
    "nodeId": "compute-primary",
    "jobIds": ["job-gpu-loop"]
  },
  "nodeAvailability": [
    {
      "nodeId": "compute-primary",
      "role": "compute_executor",
      "available": false,
      "queuePressure": "medium"
    }
  ],
  "createdAt": "2026-07-04T12:00:00Z"
}
```

If the loop is not resumable or no alternate node matches, `state` is `paused` and `target` is `{ "role": "paused", "nodeId": null }`. In both cases, the failed node subqueue is preserved.

### `GET /api/v1/scheduler/loops/:loopId`

Returns the persisted redispatch/pause state with the same fields as `POST /api/v1/scheduler/redispatch`, plus `updatedAt`, or `404 scheduler_loop_not_found`.

## Raw artifact access (`v1`)

`GET /api/v1/sessions/:id/artifacts/:artifactId?raw=1` is intentionally narrow. It is unavailable unless raw artifact persistence is explicitly enabled in local privacy settings. Disabled installs return:

```json
{
  "error": {
    "code": "raw_artifacts_disabled",
    "message": "Raw artifact persistence is not enabled.",
    "details": {
      "sessionId": "session-1",
      "artifactId": "event-1"
    }
  }
}
```

### Incremental read pattern

1. Poll `GET /events?sessionId=<id>` to get all events (with optional `limit`).
2. Use `nextCursor.afterSeq` in subsequent requests: `GET /events?sessionId=<id>&afterSeq=<seq>`.
3. Repeat until `hasMore` is `false`.

This gives clients stable incremental reads. Exactly-once delivery also requires client-side checkpointing and idempotency.

```mermaid
sequenceDiagram
  participant Client
  participant Daemon as /api/v1/events

  Client->>Daemon: GET ?sessionId=S1
  Daemon-->>Client: { events: [seq 1..50], nextCursor: { afterSeq: 50 }, hasMore: true }
  Client->>Client: persist last seq = 50
  Client->>Daemon: GET ?sessionId=S1&afterSeq=50
  Daemon-->>Client: { events: [seq 51..78], nextCursor: { afterSeq: 78 }, hasMore: false }
  Client->>Client: persist last seq = 78

  note over Client,Daemon: Client crash + restart
  Client->>Daemon: GET ?sessionId=S1&afterSeq=78
  Daemon-->>Client: { events: [seq 79..82], nextCursor: { afterSeq: 82 }, hasMore: false }
```

Persisting `afterSeq` survives daemon restarts: events are append-only and seq numbers are monotonic, so a resumed poll always picks up where it stopped.

## Live control response shapes (`v1`)

Both live-control endpoints return the same accepted response shape on success:

- `POST /api/v1/sessions/:id/input`
- `POST /api/v1/sessions/:id/kill`

```json
{
  "ok": true,
  "accepted": true
}
```

Shared non-success responses use the structured error envelope:

- `404` when the session does not exist:

```json
{
  "error": {
    "code": "session_not_found",
    "message": "Session was not found.",
    "details": { "sessionId": "session-1" }
  }
}
```

- `409` when the session exists but is not live:

```json
{
  "error": {
    "code": "session_not_live",
    "message": "Session is not live.",
    "details": { "sessionId": "session-1" }
  }
}
```

## comux and OpenClaw bridge compatibility

- comux reads the `capabilities` object from `/health` to decide which features to use.
- The external OpenClaw bridge plugin OpenClaw bridge (`packages/openclaw-coven`) is updated in this repo alongside the daemon and uses `apiVersion === "coven.daemon.v1"` as its contract guard.
- Client updates to use `afterSeq` cursors and paginated event envelopes may happen independently of the daemon update; the daemon-enforced shape is the source of truth.
- The `supportedApiVersions` field has been removed from the health response in `coven.daemon.v1`; clients should check `apiVersion` directly.

## Compatibility and migration policy

- `coven.daemon.v1` clients may rely on the documented field names and top-level response shapes above.
- Additive fields are backward compatible. Clients should ignore unknown fields when safe.
- Any incompatible change must ship under a new `apiVersion` value exposed by `GET /api/v1/health` or its successor route.
- Before a client switches to a new major contract, the Coven repo should publish updated contract docs and a migration note that maps the old shape to the new one.

## Recommended client handshake

1. Call `GET /api/v1/health`.
2. Verify `apiVersion === "coven.daemon.v1"` and `capabilities.structuredErrors === true`.
3. Check `capabilities.eventCursor === "sequence"` before using `afterSeq` pagination.
4. Only then depend on the documented `v1` sessions/events shapes.

## Scope boundary

The `coven.daemon.v1` contract covers daemon health, capability discovery, action routing, sessions, events, live input, live kill, travel-mode profile/delta reconciliation, and scheduler decision/recovery routes. Do not treat route names outside this document as reserved API until they are implemented and documented here.
