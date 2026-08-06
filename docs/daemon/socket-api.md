---
summary: "HTTP-over-local-IPC contract under /api/v1. Versioned, capability-discovered, structured-error."
read_when:
  - Building a client for the Coven daemon
  - Auditing what the socket exposes
title: "Socket API"
description: "The Coven daemon exposes a small versioned HTTP API over same-user local IPC. Reference for the coven.daemon.v1 contract under the /api/v1 prefix."
---

Coven exposes a small versioned HTTP API over same-user local IPC. On Unix-like
hosts, this is `<COVEN_HOME>/coven.sock`; on Windows, it is an owner-only named
pipe selected by `COVEN_HOME`. Health and `coven daemon status` report the
active endpoint, so clients must not construct a Windows pipe name from the
Unix convention. The current public contract is **`coven.daemon.v1`** served
under the `/api/v1` prefix.

The daemon does not use OAuth, JWTs, bearer tokens, API keys, or browser
cookies. Trust is **same-user local access** to this IPC endpoint. See [Auth
posture](/daemon/auth-posture) before adding a new client, dashboard, remote
bridge, or browser-facing transport.

## Handshake

Always start with `GET /api/v1/health` as the compatibility handshake for the
named `coven.daemon.v1` contract:

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
  "daemon": {
    "pid": 31415,
    "startedAt": "2026-05-15T19:31:02Z",
    "socket": "<local IPC endpoint>"
  }
}
```

`daemon` is `null` when daemon metadata is unavailable. When present,
`daemon.socket` reports the active local IPC endpoint. A `hub` summary may also
be present when the store-backed health summary is available.

Negotiate the named `coven.daemon.v1` contract against this health
`apiVersion`, then check every capability required by the operation before
depending on a response shape. Capabilities advertise availability and never grant permission.
`GET /api/v1/api-version` remains a legacy route-family
diagnostic whose literal `v1` values are not proof of named-contract support.

## Endpoints

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/api-version` | Read the legacy route-family token. |
| `GET /api/v1/health` | Check daemon health and metadata. |
| `GET /api/v1/capabilities` | Discover routable capabilities and owning adapters. |
| `POST /api/v1/actions` | Send a known intent through the control plane. |
| `GET /api/v1/sessions` | List sessions. |
| `POST /api/v1/sessions` | Launch a session. |
| `GET /api/v1/sessions/:id` | Fetch one session. |
| `GET /api/v1/events?sessionId=...` | Read session events. |
| `GET /api/v1/memory` | List familiar memory summaries with opaque ids. |
| `GET /api/v1/memory/overview` | Read memory totals and capability state. |
| `GET /api/v1/memory/:id` | Read validated markdown content for a list id. |
| `POST /api/v1/sessions/:id/input` | Forward input to a live session. |
| `POST /api/v1/sessions/:id/kill` | Kill a live session. |
| `POST /api/v1/sessions/:id/handoffs` | Offer a redacted, generation-fenced handoff. |
| `GET /api/v1/sessions/:id/handoffs` | Read stored handoff state. |
| `POST /api/v1/sessions/:id/handoffs/:handoffId/claim` | Claim a handoff generation. |
| `POST /api/v1/sessions/:id/handoffs/:handoffId/ack` | Acknowledge a quiesced source. |
| `POST /api/v1/sessions/:id/handoffs/:handoffId/continuations` | Record continuation import. |
| `POST /api/v1/store/vacuum` | Rebuild the event FTS index and compact the SQLite store. |

Detailed shapes live in the [API reference](/reference/api).

Session handoff is a **same-user local IPC** operation. A companion must use a
separately paired authenticated transport; exposing these endpoints directly
to a remote listener would bypass this IPC endpoint's same-user trust boundary. See
[Session handoff](/daemon/session-handoff).

Memory path entries must be UTF-8 regular `.md` files; invalid names,
symlinks, Windows reparse points, non-files/non-directories, and entries that
disappear during scanning are excluded. Unexpected enumeration, directory-open,
or metadata errors fail the request rather than returning partial data.
Overview reads metadata only. List omits the excerpt for an unreadable,
invalid-UTF-8, or over-4-MiB body by returning its metadata-valid row with an
empty `excerpt`. Detail reads only the selected validated handle and returns
`413 memory_content_too_large` above 4 MiB or `422 memory_content_invalid` for
invalid UTF-8. A missing or unsafe target at open time returns
`404 memory_not_found`; permission failures, unexpected open failures, and
post-open metadata/read failures return `503 memory_content_unavailable`.
Those path-safe errors include only `memoryId` in `details`, never a filesystem
path or raw I/O error.

## Error envelope

All error responses use:

```json
{
  "error": {
    "code": "session.cwd_outside_root",
    "message": "cwd must canonicalize inside project root",
    "details": {
      "projectRoot": "/workspace/project",
      "cwd": "/tmp/wander"
    }
  }
}
```

See [Error envelope](/daemon/error-envelope) for the full code list.

## Versioning

The health response's `apiVersion` field is the named contract clients pin against. Coven follows additive compatibility: new fields and new capabilities are added under existing versions; breaking changes require a new version. See [API versioning](/daemon/api-versioning).

## Calling the socket

The following curl, Node `socketPath`, and Rust `UnixStream` examples apply
only on Unix-like hosts. On Windows, use a named-pipe-capable local IPC client
against the daemon-reported endpoint; do not derive a pipe name from
`<COVEN_HOME>/coven.sock`.

<Tabs>
  <Tab title="curl">
    ```bash
    curl --unix-socket "$HOME/.coven/coven.sock" \
      http://localhost/api/v1/health
    ```
  </Tab>
  <Tab title="Node">
    ```js
    import http from "node:http";
    http.get(
      { socketPath: `${process.env.HOME}/.coven/coven.sock`, path: "/api/v1/health" },
      (res) => res.pipe(process.stdout)
    );
    ```
  </Tab>
  <Tab title="Rust">
    ```rust
    let home = std::env::var("HOME")?;
    let socket = std::path::PathBuf::from(home).join(".coven/coven.sock");
    let stream = tokio::net::UnixStream::connect(socket).await?;
    // wrap in hyper or your preferred HTTP client
    ```
  </Tab>
</Tabs>

## Related

- [API contract](/reference/api-contract)
- [Capabilities handshake](/daemon/capabilities-handshake)
- [Error envelope](/daemon/error-envelope)
- [Auth posture](/daemon/auth-posture)
