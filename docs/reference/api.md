---
summary: "Current Coven local socket API endpoints."
read_when:
  - Looking up an endpoint
  - Building a client against `/api/v1`
title: "Coven API reference"
description: "Endpoint reference for the Coven local socket API under /api/v1: health, capabilities, actions, sessions, events, and input forwarding."
---


The Coven daemon exposes its public API as HTTP over a Unix socket under `<covenHome>/coven.sock`. The active contract is **`coven.daemon.v1`** served under `/api/v1`.

## Endpoints

| Endpoint | Page |
|---|---|
| `GET /api/v1/api-version` | Read the active API version and supported versions. |
| `GET /api/v1/health` | Check daemon health, `apiVersion`, and capabilities. |
| `GET /api/v1/capabilities` | Discover daemon/control-plane capabilities and policy hints. |
| `POST /api/v1/actions` | Route a known policy-shaped control-plane action. |
| `GET /api/v1/sessions` | List active sessions. |
| `POST /api/v1/sessions` | Launch a project-scoped harness session. |
| `GET /api/v1/sessions/:id` | Fetch one session. |
| `POST /api/v1/sessions/:id/input` | Forward input to a live session. |
| `POST /api/v1/sessions/:id/kill` | Kill a live session. |
| `GET /api/v1/events` | Read paginated session events. |

## Always begin with health

```http
GET /api/v1/health
```

The response tells you the active `apiVersion`, the daemon's `capabilities`, and the running pid/uptime. Treat the rest of the API as undefined until you have read those fields.

See [Coven Local API](/API) for response examples and [API contract](/API-CONTRACT) for stable shapes and failure envelopes.

## Related

- [API contract](/API-CONTRACT)
- [Authentication and local access](/AUTH)
- [Client integration](/CLIENT-INTEGRATION)
