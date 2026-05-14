# Coven Local API

_Last updated: 2026-05-09_

Coven exposes a small HTTP API over the local Unix socket at `<covenHome>/coven.sock`. The Rust daemon is the authority boundary: clients may validate for UX, but the daemon still validates project roots, cwd, harness ids, session ids, input, and live-session state before acting.

See [Authentication and local access](AUTH.md) for the current auth posture. In short: the daemon API does not use OAuth, JWTs, bearer tokens, API keys, or cookies today. Access is local Unix-socket based, provider credentials stay with the harness CLIs, and any remote, browser, or TCP exposure needs a separate auth design.

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
| `GET /api/v1/events?sessionId=...` | Read session events |
| `POST /api/v1/sessions/:id/input` | Forward input to a live session |
| `POST /api/v1/sessions/:id/kill` | Kill a live session |

Unversioned routes currently remain as legacy aliases during the early MVP window, but new clients should not rely on them.

Unknown `/api/<version>/...` prefixes fail closed with an `unsupported API version` JSON response.

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
    "eventCursor": "sequence",
    "structuredErrors": true
  },
  "daemon": {
    "pid": 12345,
    "startedAt": "2026-05-09T12:00:00Z",
    "socket": "/Users/example/.coven/coven.sock"
  }
}
```

When no daemon metadata is available, `daemon` is `null`.

## Control-plane capabilities

`GET /api/v1/capabilities` is the discovery point for first-party clients such as OpenMeow. It returns capability ids, adapter ownership, availability, policy hints, and action ids. This keeps clients from hard-coding what the daemon can do.

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

`POST /api/v1/actions` accepts an OpenMeow-style intent envelope. The daemon routes only known actions; unknown actions fail closed before any adapter can run.

```json
{
  "action": "coven.capabilities.refresh",
  "origin": "open-meow",
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
    "origin": "open-meow",
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
- Daemon changes that affect `/api/v1/health`, `/api/v1/sessions`, `/api/v1/events`, input, or kill behavior should update client compatibility tests in the same repo.
