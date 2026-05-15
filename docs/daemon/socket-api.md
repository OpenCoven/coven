---
summary: "HTTP-over-Unix-socket contract under /api/v1. Versioned, capability-discovered, structured-error."
read_when:
  - Building a client for the Coven daemon
  - Auditing what the socket exposes
title: "Socket API"
---

Coven exposes a small versioned HTTP API over a Unix socket. The current public contract is **`coven.daemon.v1`** served under the `/api/v1` prefix.

The daemon does not use OAuth, JWTs, bearer tokens, API keys, or browser cookies. Trust is **same-user local access** to the Unix socket at `<covenHome>/coven.sock`. See [Auth posture](/daemon/auth-posture) before adding a new client, dashboard, remote bridge, or browser-facing transport.

## Handshake

Always start with:

```http
GET /api/v1/health
```

```json
{
  "ok": true,
  "apiVersion": "coven.daemon.v1",
  "capabilities": {
    "sessions": true,
    "events": true,
    "actions": true,
    "harnesses": ["codex", "claude"]
  },
  "daemon": {
    "pid": 31415,
    "uptime": 4823,
    "startedAt": "2026-05-15T19:31:02Z"
  }
}
```

Negotiate against `apiVersion` and `capabilities` before depending on session or event response shapes.

## Endpoints

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/api-version` | Read the active API version and supported versions. |
| `GET /api/v1/health` | Check daemon health and metadata. |
| `GET /api/v1/capabilities` | Discover routable capabilities and owning adapters. |
| `POST /api/v1/actions` | Send a known intent through the control plane. |
| `GET /api/v1/sessions` | List sessions. |
| `POST /api/v1/sessions` | Launch a session. |
| `GET /api/v1/sessions/:id` | Fetch one session. |
| `GET /api/v1/events?sessionId=...` | Read session events. |
| `POST /api/v1/sessions/:id/input` | Forward input to a live session. |
| `POST /api/v1/sessions/:id/kill` | Kill a live session. |

Detailed shapes live in the [API reference](/reference/api).

## Error envelope

All error responses use:

```json
{
  "error": {
    "code": "session.cwd_outside_root",
    "message": "cwd must canonicalize inside project root",
    "details": {
      "projectRoot": "/Users/me/work/proj",
      "cwd": "/tmp/wander"
    }
  }
}
```

See [Error envelope](/daemon/error-envelope) for the full code list.

## Versioning

The `apiVersion` field is the contract clients pin against. Coven follows additive compatibility: new fields and new capabilities are added under existing versions; breaking changes require a new version. See [API versioning](/daemon/api-versioning).

## Calling the socket

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
