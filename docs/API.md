---
title: "Coven local socket API"
summary: "The Coven local HTTP API served over a Unix socket: health, capabilities, actions, sessions, events, and input forwarding under /api/v1."
read_when:
  - Building a local Coven client
  - Looking up /api/v1 endpoint behavior
description: "The Coven local HTTP API served over a Unix socket: health, capabilities, actions, sessions, events, and input forwarding under /api/v1."
---

# Coven Local API

_Last updated: 2026-05-09_

Coven exposes a small HTTP API over the local Unix socket at `<covenHome>/coven.sock`. The Rust daemon is the authority boundary: clients may validate for UX, but the daemon still validates project roots, cwd, harness ids, session ids, input, and live-session state before acting.

```mermaid
flowchart LR
  Client[Local client] -->|connect| Sock["<covenHome>/coven.sock"]
  Sock -->|HTTP/1.1| Router["/api/v1 router"]
  Router --> Health["/health"]
  Router --> Capabilities["/capabilities"]
  Router --> Actions["/actions"]
  Router --> Sessions["/sessions[/:id[/input|/kill]]"]
  Router --> Events["/events + /sessions/:id/events"]
  Router --> Version["/api-version"]

  Health & Capabilities & Actions & Sessions & Events & Version -->|"{ ... } or { error: { code, message, details } }"| Client
```

Every route returns either a documented success shape or the structured error envelope. Unknown routes, unknown action ids, and unknown API versions all fail closed with `invalid_request` or `not_found`.

See [Authentication and local access](/AUTH) for the current auth posture. In short: the daemon API does not use OAuth, JWTs, bearer tokens, API keys, or cookies today. Access is local Unix-socket based, provider credentials stay with the harness CLIs, and any remote, browser, or TCP exposure needs a separate auth design.

## Versioning

The current public API contract is the named **`coven.daemon.v1`** contract served under the `/api/v1` route prefix.

Versioned clients should use the `/api/v1` prefix:

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/api-version` | Read the active API version and supported versions |
| `GET /api/v1/health` | Check daemon health and metadata |
| `GET /api/v1/capabilities` | Discover daemon/control-plane capabilities and policy hints |
| `POST /api/v1/actions` | Route a policy-shaped control-plane action |
| `GET /api/v1/sessions` | List active sessions |
| `POST /api/v1/sessions` | Launch a session |
| `GET /api/v1/sessions/:id` | Fetch one session |
| `GET /api/v1/events?sessionId=...` | Read redacted session events |
| `GET /api/v1/sessions/:id/events` | Read redacted session events |
| `GET /api/v1/sessions/:id/log` | Read bounded redacted log previews |
| `POST /api/v1/sessions/:id/input` | Forward input to a live session |
| `POST /api/v1/sessions/:id/kill` | Kill a live session |
| `GET /api/v1/hub/status` | Read hub role, node availability, and queue depths |
| `POST /api/v1/hub/nodes` | Register or re-register an executor node |
| `GET /api/v1/hub/nodes` | List registered nodes |
| `GET /api/v1/hub/nodes/:id` | Fetch one registered node |
| `POST /api/v1/hub/nodes/:id/health` | Record an executor health report (holds/resumes its subqueue) |
| `POST /api/v1/hub/nodes/:id/poll` | Poll executor availability outbound over its dispatch transport |
| `POST /api/v1/hub/nodes/:id/dispatch` | Dispatch a job outbound to a stateless executor |
| `GET /api/v1/hub/dispatches/:jobId` | Fetch a persisted dispatch record (job spec + result envelope) |
| `POST /api/v1/hub/jobs` | Enqueue a job on the persistent global queue |
| `GET /api/v1/hub/jobs?state=...` | List queued jobs |
| `GET /api/v1/hub/jobs/:id` | Fetch one job with its routing entry |
| `POST /api/v1/hub/jobs/:id/assign` | Assign a job to an executor from the node registry |
| `POST /api/v1/hub/jobs/:id/complete` | Mark a job completed/failed/cancelled |
| `GET /api/v1/hub/routing` | Read the persistent routing table |

Full request/response shapes for the hub control plane (node registry, routing table, global and per-executor queues) live in [`API-CONTRACT.md`](API-CONTRACT.md); hub restart and supervision guidance lives in [`HUB-OPERATIONS.md`](HUB-OPERATIONS.md).

Unversioned routes currently remain as legacy aliases during the early MVP window, but new clients should not rely on them.

Unknown `/api/<version>/...` prefixes fail closed with an `unsupported API version` JSON response.

## Log privacy

Event payloads returned by `/events`, `/sessions/:id/events`, and `/sessions/:id/log` are redacted by default. Raw sensitive artifacts are not included in broad responses. The narrow raw artifact route requires explicit local raw artifact persistence and `raw=1`; otherwise it returns a structured `raw_artifacts_disabled` error.

## Health response

`GET /api/v1/health` returns the API version alongside daemon status:

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
    "hub": true,
    "eventCursor": "sequence",
    "structuredErrors": true
  },
  "daemon": {
    "pid": 12345,
    "startedAt": "2026-05-09T12:00:00Z",
    "socket": "/Users/example/.coven/coven.sock"
  },
  "hub": {
    "role": "hub",
    "hubId": "hub_01J...",
    "nodesTotal": 2,
    "nodesAvailable": 1
  }
}
```

When no daemon metadata is available, `daemon` is `null`. The `hub` block summarizes the daemon's control-plane role and node availability; full node and queue detail lives at `GET /api/v1/hub/status`.

## Control-plane capabilities

`GET /api/v1/capabilities` is the discovery point for first-party clients such as the chat/intake client. It returns capability ids, adapter ownership, availability, policy hints, and action ids. This keeps clients from hard-coding what the daemon can do.

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

## Control-plane actions

`POST /api/v1/actions` accepts an intent envelope. The daemon routes only known actions; unknown actions fail closed before any adapter can run.

```json
{
  "action": "coven.capabilities.refresh",
  "origin": "external-client",
  "intentId": "intent-1",
  "args": {}
}
```

Immediately completed safe actions return `200` with an event-shaped payload that clients can render optimistically or fold into later event streams:

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
    "payload": { "capabilities": 3 }
  }
}
```

## Compatibility rules

- Additive JSON fields are allowed in `v1` responses.
- Existing required fields should not be removed or renamed inside `v1`.
- Breaking response-shape or behavior changes require a new API version prefix.
- External clients should call `/api/v1/health` before assuming compatibility.
- Daemon changes that affect `/api/v1/health`, `/api/v1/sessions`, `/api/v1/events`, `/api/v1/sessions/:id/events`, input, or kill behavior should update client compatibility tests in the same repo.
