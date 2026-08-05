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
- Operation-group availability flags are advertised by the `capabilities`
  object from `GET /api/v1/health`. Control-plane catalog entries, policy
  hints, and action ids are discovered separately through
  `GET /api/v1/capabilities`.
- Breaking changes — field removal, type change, semantics change — require a new contract version (`coven.daemon.v2`, ...).
- Health advertises one active named contract value. A contract transition
  requires an explicit migration plan; clients must not infer plural named
  contract support from the legacy route-family response.

## Negotiation

Clients negotiate compatibility with `GET /api/v1/health`. Its `apiVersion`
field is the named contract `coven.daemon.v1`; clients must then check every
capability required by the operation before sending a dependent request.
Capabilities advertise availability and never grant permission.

`GET /api/v1/api-version` is a legacy route-family diagnostic. Its existing
`apiVersion: "v1"` and `supportedApiVersions: ["v1"]` values identify the
`/api/v1/*` route namespace, not the named compatibility contract. Existing
values remain wire-compatible, but new clients must not use this response as
proof of `coven.daemon.v1` support.

```http
GET /api/v1/api-version
```

```json
{
  "apiVersion": "v1",
  "supportedApiVersions": ["v1"]
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
    "structuredErrors": true,
    "sessionHandoff": true
  },
  "daemon": { "pid": 12345, "startedAt": "2026-07-14T12:00:00Z", "socket": "<covenHome>/coven.sock" },
  "eventWriter": {
    "state": "healthy",
    "queuedBytes": 0,
    "capacityBytes": 2097152,
    "droppedOutputEvents": 0,
    "droppedOutputBytes": 0,
    "connectionOpens": 1,
    "transactions": 42,
    "committedEvents": 513
  }
}
```

When present, `eventWriter.state` is `healthy`, `pressured`, or `failed`.
Pressure reports explicitly rejected raw output; lifecycle and terminal events
reserve queue capacity and are never discarded for queue pressure. Each
contiguous pressure episode yields one ordered `output_truncated` event in the
affected session stream, inserted before the next accepted event.

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
