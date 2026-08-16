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

It travels over same-user local IPC: `<COVEN_HOME>/coven.sock` on Unix-like
hosts, or an owner-only named pipe selected by `COVEN_HOME` on Windows. Health
and `coven daemon status` report the active endpoint; clients must not
construct a Windows pipe name from the Unix convention.

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
    "sessionHandoff": true,
    "sessionLaunchPolicy": true,
    "afs": true,
    "afsMount": false,
    "afsCommit": true,
    "afsCommitDryRun": true,
    "executionBindingContracts": ["psyche.execution_binding.v1"],
    "requestAdoptionContracts": ["psyche.request_adoption.v1"]
  },
  "daemon": { "pid": 12345, "startedAt": "2026-07-14T12:00:00Z", "socket": "<local IPC endpoint>" },
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

The health `capabilities` object contains all 16 fields: `sessions`, `events`,
`travel`, `scheduler`, `hub`, `executorDispatch`, `eventCursor`,
`structuredErrors`, `sessionHandoff`, `sessionLaunchPolicy`, `afs`, `afsMount`,
`afsCommit`, `afsCommitDryRun`, `executionBindingContracts`, and
`requestAdoptionContracts`.

`sessionLaunchPolicy` is `true` only over owner-gated local IPC. TCP health
always reports it as `false`, and TCP rejects any `POST /api/v1/sessions`
payload containing `launchPolicy` with `403 forbidden`; passing Host or Origin
checks does not elevate TCP authority. The initial exact policy supports Codex
`nonInteractive` approval `never`, sandbox `workspace-write`, and explicit
absolute existing `addDirs`, including a named external mission workspace.

When present, `eventWriter.state` is `healthy`, `pressured`, or `failed`.
Pressure reports explicitly rejected raw output; lifecycle and terminal events
reserve queue capacity and are never discarded for queue pressure. Each
contiguous pressure episode yields one ordered `output_truncated` event in the
affected session stream, inserted before the next accepted event.

If a client requires a capability the daemon does not advertise, the client should fail loudly with a remediation hint (`upgrade Coven to >= N`).

Adopted launch and input require the exact O3 literal in
`requestAdoptionContracts` before either dedicated route is used. That O3
capability advertises the composite route contract, including the mandatory
exact O2 proof in every request. The bundled adopted client does not
independently gate these methods on `executionBindingContracts`; that separate
field remains the additive discovery surface for standalone O2 support.

| Route | First adoption | Exact replay | Adoption errors |
|---|---|---|---|
| `POST /api/v1/adopted-sessions` | `201 SessionRecord` | `200 SessionRecord` | `400 request_adoption_required`, `request_adoption_invalid`, `request_adoption_unsupported`; `409 request_adoption_conflict` |
| `POST /api/v1/sessions/:id/adopted-input` | `202 {"adopted":true,"replayed":false,"delivery":"not_asserted"}` | `200 {"adopted":true,"replayed":true,"delivery":"not_asserted"}` | `400 request_adoption_required`, `request_adoption_invalid`, `request_adoption_unsupported`; `409 request_adoption_conflict` |

Legacy bound launch/input is rejected; unbound legacy behavior remains
compatible. Adopted clients fail locally when negotiation is absent or
malformed and never retry a legacy mutation. Adoption asserts durable
responsibility, not delivery or runtime outcome. The canonical
[request-adoption contract](/API-CONTRACT#psyche-request-adoption-contract-v1)
defines the closed shape, ordering, privacy, retention, and non-goals.
Only synchronous HTTP errors returned after adoption carry the marker-only
`{"adopted":true,"delivery":"not_asserted"}` details. Asynchronous terminal or
event-persistence failures cannot revise a response that has already returned.

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
