---
summary: "Index of the Coven local socket API. Versioned, structured-error, same-user trust."
read_when:
  - Looking up an endpoint
  - Building a client against `/api/v1`
title: "API reference"
---

The Coven daemon exposes its public API as HTTP over a Unix socket under `<covenHome>/coven.sock`. The active contract is **`coven.daemon.v1`** served under `/api/v1`.

## Endpoints

| Endpoint | Page |
|---|---|
| `GET /api/v1/api-version` | [API contract](/reference/api-contract) |
| `GET /api/v1/health` | [Capabilities handshake](/daemon/capabilities-handshake) |
| `GET /api/v1/capabilities` | [Capabilities endpoint](/reference/api-capabilities) |
| `POST /api/v1/actions` | [Actions endpoint](/reference/api-actions) |
| `GET /api/v1/sessions` | [Sessions endpoints](/reference/api-sessions) |
| `POST /api/v1/sessions` | [Sessions endpoints](/reference/api-sessions) |
| `GET /api/v1/sessions/:id` | [Sessions endpoints](/reference/api-sessions) |
| `POST /api/v1/sessions/:id/input` | [Sessions endpoints](/reference/api-sessions) |
| `POST /api/v1/sessions/:id/kill` | [Sessions endpoints](/reference/api-sessions) |
| `GET /api/v1/events` | [Events endpoint](/reference/api-events) |

## Always begin with health

```http
GET /api/v1/health
```

The response tells you the active `apiVersion`, the daemon's `capabilities`, and the running pid/uptime. Treat the rest of the API as undefined until you have read those fields.

See [Socket API](/daemon/socket-api) for transport details and [Error envelope](/daemon/error-envelope) for failure shapes.

## Related

- [API contract](/reference/api-contract)
- [Auth posture](/daemon/auth-posture)
- [Capabilities handshake](/daemon/capabilities-handshake)
