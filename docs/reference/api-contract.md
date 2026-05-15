---
summary: "The named coven.daemon.v1 contract served under /api/v1. Versioning, additive compatibility, and break rules."
read_when:
  - Pinning a client to a Coven contract
  - Auditing whether a daemon upgrade is safe
title: "API contract"
---

Coven's local API is versioned as a **named contract**. The current value is `coven.daemon.v1`.

## Compatibility rules

- New fields can be added inside an existing contract version. Clients must ignore unknown fields.
- New endpoints can be added inside an existing contract version. Clients must not assume the full URL space is fixed.
- New capabilities are advertised through `GET /api/v1/capabilities`.
- Breaking changes — field removal, type change, semantics change — require a new contract version (`coven.daemon.v2`, ...).
- The daemon will advertise both the current and previous version during a transition.

## Negotiation

```http
GET /api/v1/health
```

```json
{
  "apiVersion": "coven.daemon.v1",
  "supportedVersions": ["coven.daemon.v1"],
  "capabilities": {
    "sessions": true,
    "events": true,
    "actions": true,
    "harnesses": ["codex", "claude"]
  }
}
```

If a client requires a capability the daemon does not advertise, the client should fail loudly with a remediation hint (`upgrade Coven to >= N`).

## Error envelope

All errors use the structured shape:

```json
{
  "error": {
    "code": "<dotted.code>",
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
