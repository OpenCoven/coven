# Coven Local API Contract

The Coven daemon socket API is a public compatibility boundary for comux and external clients such as `@opencoven/coven`.

## Current stable version

- `GET /health` exposes `apiVersion: "v1"`.
- Clients should read `/health` before assuming any response shape from other endpoints.

## `GET /health`

`GET /health` returns daemon reachability plus the contract version:

```json
{
  "ok": true,
  "apiVersion": "v1",
  "daemon": {
    "pid": 12345,
    "startedAt": "2026-05-09T06:43:00Z",
    "socket": "/Users/alice/.coven/coven.sock"
  }
}
```

If the daemon metadata is unavailable, `daemon` may be `null`.

## Session record shape (`v1`)

In `v1`, session responses stay as raw JSON objects using the Rust daemon's snake_case field names.

Endpoints that return this shape:

- `GET /sessions` → `SessionRecord[]`
- `POST /sessions` → `SessionRecord`
- `GET /sessions/:id` → `SessionRecord`

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

## Event record shape (`v1`)

`GET /events?sessionId=<id>` returns `EventRecord[]` with append-only event records:

```json
[
  {
    "id": "event-1",
    "session_id": "session-1",
    "kind": "output",
    "payload_json": "{\"data\":\"hello\"}",
    "created_at": "2026-05-09T06:43:10Z"
  }
]
```

## Live control response shapes (`v1`)

Both live-control endpoints return the same accepted response shape on success:

- `POST /sessions/:id/input`
- `POST /sessions/:id/kill`

```json
{
  "ok": true,
  "accepted": true
}
```

Shared non-success responses:

- `404` when the session does not exist:

```json
{
  "error": "session not found"
}
```

- `409` when the session exists but is not live:

```json
{
  "error": "session not live",
  "sessionId": "session-1"
}
```

## Compatibility and migration policy

- `v1` clients may rely on the documented field names and top-level response shapes above.
- Additive fields are backward compatible. Clients should ignore unknown fields when safe.
- Any incompatible change must ship under a new `apiVersion` value exposed by `GET /health`.
- Before a client switches to a new major contract, the Coven repo should publish updated contract docs and a migration note that maps the old shape to the new one.

## Recommended client handshake

1. Call `GET /health`.
2. Verify `apiVersion === "v1"`.
3. Only then depend on the documented `v1` sessions/events shapes.
