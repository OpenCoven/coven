---
summary: "The named coven.daemon.v1 contract served under /api/v1. Versioning, additive compatibility, and break rules."
read_when:
  - Pinning a client to a Coven contract
  - Auditing whether a daemon upgrade is safe
title: "API contract"
description: "Reference for the coven.daemon.v1 local API contract: how Coven adds fields and endpoints safely and when breaking changes require a new contract version."
---

> **See also:** the fuller single-page contract — shapes, error codes, cursor pagination, hub control plane — lives in [`API-CONTRACT.md`](/API-CONTRACT) (`docs/API-CONTRACT.md`). This page is the condensed versioning and negotiation summary.

Coven's local API is versioned as a **named contract**. The current value is `coven.daemon.v1`.

## Compatibility rules

- New fields can be added inside an existing contract version. Clients must ignore unknown fields.
- New endpoints can be added inside an existing contract version. Clients must not assume the full URL space is fixed.
- New capabilities are advertised through `GET /api/v1/capabilities`.
- Breaking changes — field removal, type change, semantics change — require a new contract version (`coven.daemon.v2`, ...).
- The daemon will advertise both the current and previous version during a transition.

## Negotiation

Read the version from `GET /api/v1/api-version` and the capability flags from `GET /api/v1/health`:

```http
GET /api/v1/api-version
```

```json
{
  "apiVersion": "coven.daemon.v1",
  "supportedApiVersions": ["coven.daemon.v1"]
}
```

```http
GET /api/v1/health
```

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
    "executorDispatch": true,
    "eventCursor": "sequence",
    "structuredErrors": true
  },
  "daemon": { "pid": 12345, "startedAt": "2026-07-14T12:00:00Z", "socket": "/home/alex/.coven/coven.sock" }
}
```

If a client requires a capability the daemon does not advertise, the client should fail loudly with a remediation hint (`upgrade Coven to >= N`).

## Error envelope

All errors use the structured shape:

```json
{
  "error": {
    "code": "<snake_case_code>",
    "message": "<human-readable>",
    "details": { "<context>": "<value>" }
  }
}
```

See [Error envelope](/daemon/error-envelope) for the full code list.

## Related

- [Socket API](/daemon/socket-api)
- [API versioning](/daemon/api-versioning)
- [API reference](/reference/api)
