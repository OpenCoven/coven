# Coven Gateway Wire — TECH

**Status:** Draft v1 · 2026-05-26
**Companion to:** [PRODUCT.md](./PRODUCT.md)

## Module layout

Existing:

- `crates/coven-cli/src/api.rs` — request routing for `/api/v1/*`.
- `crates/coven-cli/src/daemon.rs` — Unix-socket bind and listener loop.

New:

- `crates/coven-cli/src/gateway.rs` — TCP listener on 127.0.0.1, bearer-token check, subset router.
- `crates/coven-cli/src/api_subset.rs` — declarative map of which `/v1/*` paths exist on TCP and what fields to strip.

Both `daemon.rs` (Unix) and `gateway.rs` (TCP) parse HTTP and dispatch into the same handler functions in `api.rs`. The TCP path inserts a thin "subset filter" middleware that consults `api_subset.rs` before the handler runs.

## `api_subset.rs` — the gate

```rust
// crates/coven-cli/src/api_subset.rs
use http::Method;

pub struct TcpSurface {
    pub path_pattern: &'static str,        // e.g. "/v1/sessions/:id/handoffs"
    pub allowed_methods: &'static [Method],
    pub redact_response: ResponseRedaction,
}

pub enum ResponseRedaction {
    None,
    StripArtifactBodies,   // for /sessions/:id/artifacts/:id
    Custom(fn(&mut serde_json::Value)),
}

pub static TCP_SURFACE: &[TcpSurface] = &[
    TcpSurface { path_pattern: "/health",                  allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions",             allowed_methods: &[Method::GET, Method::POST], redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions/:id",         allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions/:id/manifest",allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions/:id/events",  allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions/:id/handoffs",allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions/:id/input",   allowed_methods: &[Method::POST],        redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions/:id/kill",    allowed_methods: &[Method::POST],        redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/sessions/:id/artifacts/:artifact_id",
                                                           allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::StripArtifactBodies },
    TcpSurface { path_pattern: "/v1/cast",                 allowed_methods: &[Method::POST],        redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/cast-codes",           allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/familiars",            allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/skills",               allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/memory",               allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/research",             allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/messages",             allowed_methods: &[Method::POST],        redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/messages/stream",      allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    TcpSurface { path_pattern: "/v1/config",               allowed_methods: &[Method::GET],         redact_response: ResponseRedaction::None },
    // Privileged endpoints not in TCP_SURFACE are 404 on TCP.
];
```

A request comes in over TCP → `gateway.rs` matches it against `TCP_SURFACE` → if no match, 404. If match, method check → 405 if wrong. If pass, run the same handler the Unix side runs. After handler runs, apply `redact_response` to the body before serializing.

The TCP path mounts at the daemon's known prefix; CastCodes calls `/v1/sessions` so the wire path is `/v1/sessions`, no double prefix.

## Bearer-token check

```rust
// crates/coven-cli/src/gateway.rs
fn check_bearer(req_headers: &HeaderMap, expected: &[u8]) -> Result<(), AuthError> {
    let Some(h) = req_headers.get(http::header::AUTHORIZATION) else {
        return Err(AuthError::Missing);
    };
    let Ok(s) = h.to_str() else { return Err(AuthError::Malformed); };
    let Some(token) = s.strip_prefix("Bearer ") else { return Err(AuthError::Malformed); };
    // constant-time compare
    if subtle::ConstantTimeEq::ct_eq(token.as_bytes(), expected).into() {
        Ok(())
    } else {
        Err(AuthError::Invalid)
    }
}
```

The token is loaded into memory at daemon start and on rotation. A failed check returns 401 with `{ "error": "auth_failed" }` and a small (~20ms) jitter to make brute force impractical without being annoying. The `/health` endpoint requires auth too, to avoid token-presence probing.

## Artifact-body strip

For `/v1/sessions/:id/artifacts/:artifact_id` the Unix handler returns either the encrypted blob or, with `?decrypt=true`, the plaintext. Over TCP:

```rust
// inside ResponseRedaction::StripArtifactBodies handler
fn strip_artifact_bodies(json: &mut serde_json::Value) {
    if let Some(obj) = json.as_object_mut() {
        obj.remove("ciphertext");
        obj.remove("plaintext");
        obj.remove("nonce");
        // keep: id, session_id, event_id, kind, byte_count, sha256, created_at, expires_at
    }
}
```

Plus: if the request includes `?decrypt=true` over TCP, return 403 with `{ "error": "not_allowed_over_tcp", "hint": "decrypt is unix-socket-only" }`. Don't silently strip; tell the caller why.

## `daemon.json` shape

```toml
# ~/.coven/daemon.json (already exists; gateway block is additive)
[gateway]
enabled = false              # opt-in; default false
bind    = "127.0.0.1"
port    = 3000
```

Daemon startup:

1. Read `daemon.json`.
2. If `[gateway].enabled = false` → only bind Unix socket. Done.
3. If `enabled = true`:
   a. Assert `bind` is a loopback address (`127.0.0.1`, `::1`); refuse and log a clear error otherwise.
   b. Load `~/.coven/token`; if absent, generate (32 bytes from `getrandom`, base64-url, write mode 0600).
   c. Bind TCP listener.
   d. Spawn TCP listener loop alongside Unix listener loop.

The two loops share daemon state (live session registry, store handle) behind `Arc`.

## CastCodes compatibility

Verify against `cast-codes/crates/cast_agent/src/gateway.rs`:

- `GET /health` → 200 with the same payload Unix returns.
- `GET /v1/sessions` → 200 with `Vec<SessionRecord>` shape CastCodes deserializes today. Field check: `id`, `name`, `status`, `last_active`, `cwd`.
- `POST /v1/sessions` → 200 with a single `SessionRecord`.
- `DELETE /v1/sessions/:id` → not currently in TCP surface; if CastCodes calls it, we add `POST /v1/sessions/:id/kill` alias. Decision: keep CastCodes' existing call path working — add `DELETE` as a method alias for `kill` on the TCP surface, with a deprecation notice in the response header.
- `POST /v1/messages` → spawns or reuses a session, runs one turn, returns final message.
- `WS /v1/messages/stream` → upgrades, pushes `MessageChunk` frames.

The two endpoints CastCodes currently calls that have no Unix counterpart (`/v1/messages` and `/v1/messages/stream`) are implemented in `api_subset.rs`-mounted handlers that internally use the normal session-create + event-stream machinery.

## `coven doctor` checks for the wire

Adds:

- `[ok|warn] gateway.enabled = <bool>`
- `[ok|warn] gateway.bind = <addr>` (warn if non-loopback configured)
- `[ok] gateway.token present, mode 0600` (only when enabled)
- `[ok] gateway.tcp_listening = <bool>` (live check)
- `[ok] gateway.subset_surface_count = <N>` (sanity check against TCP_SURFACE table)

## Test plan

- **404 regression:** every `/api/v1/*` path that should be Unix-only is tested over TCP for 404. Test is driven by `api_subset.rs` — if a new privileged endpoint is added to the Unix router without explicit `TcpSurface` entry, the test catches the omission and the new endpoint is correctly 404 by default.
- **Method restriction:** 405 for the methods not in `allowed_methods`.
- **Auth failures:** missing token, malformed header, wrong token → 401; correct token → handler runs.
- **Artifact-body strip:** synthetic artifact in store; GET over TCP → response body lacks `ciphertext`/`plaintext`/`nonce`; over Unix with `?decrypt=true` → plaintext present.
- **CastCodes contract test:** spin up daemon with `[gateway].enabled = true`, run CastCodes `GatewayClient` (in test mode) against it, assert health/list/messages/stream all succeed.

## Migration / rollout

1. Land `api_subset.rs` and `gateway.rs` with `enabled = false` default. Zero behavior change for existing users.
2. Land `coven doctor` checks. Existing users see `gateway disabled` cleanly.
3. CastCodes integration: on first launch with a Coven daemon running, prompt user once to enable the gateway. CastCodes writes `[gateway].enabled = true` via the Unix-socket `POST /api/v1/config` (Unix-only). Daemon hot-reloads, listener comes up. (CastCodes can't call this itself over TCP because TCP can't write config — by design. The CastCodes UI walks the user through `coven gateway enable` for v1.)

## Dependencies

- `coven-trust-layer` for the artifact-body strip rule and pruning.
- `coven-session-artifacts` for the manifest endpoint shape.
- `coven-handoff-packet` for the handoff endpoints (read-only over TCP).

## Out of scope for v1

- Multi-tenant token model (one token per machine for now).
- TLS termination inside the daemon (loopback-only, plaintext HTTP is fine).
- Rate limiting beyond the 20ms auth-fail jitter.
- A separate `coven-gateway` binary (deferred until a real remote story exists).
