# Coven Gateway Wire — PRODUCT

**Status:** Draft v1 · 2026-05-26
**Owner:** Coven runtime
**Acceptance target:** Closes the trust boundary so CastCodes can consume Coven without exposing privileged operations.

## Problem

Coven and CastCodes don't currently talk. Coven daemon serves `/api/v1/*` over a Unix socket at `~/.coven/coven.sock`. CastCodes' `GatewayClient` opens TCP to `http://localhost:3000` and calls `/v1/*` with a bearer token. Two different transports, two different path prefixes, two different auth assumptions — no path between them works.

This spec defines a **two-tier transport** inside the existing Coven daemon:

- **Privileged path:** Unix socket at `~/.coven/coven.sock`, full `/api/v1/*` surface, implicit user trust via file-mode 0600. CLI uses this. Privileged operations (key rotation, decrypt, daemon shutdown, manual prune) live here.
- **Consumer path:** loopback TCP at `127.0.0.1:3000`, narrower `/v1/*` surface, bearer-token auth via `~/.coven/token`. CastCodes uses this. Read-mostly; only safe, scoped writes allowed.

The two paths share the daemon process. There is no separate gateway binary.

## Why one process, two listeners

Considered alternatives and why they were rejected:

- **Unix-socket-only, force CastCodes to switch.** Strongest local trust, but CastCodes already speaks TCP HTTP and switching to a Unix-socket HTTP client is a non-trivial refactor that buys us nothing CastCodes' current shape needs. Also locks out any future remote story.
- **Separate `coven-gateway` binary.** Cleanest long-term separation but ships two binaries to solve a problem one process can solve. The active /goal says "no magic router until handoffs, provenance, and review are reliable" — that maps directly to "don't add extra processes for orchestration yet."

If a real remote/multi-user story arrives later, the TCP listener lifts out of the daemon into a `coven-gateway` binary cleanly, because the HTTP contract is already stable. The cost of starting one-process and migrating later is low; the cost of starting two-process now is high.

## Hard rules

These are non-negotiable; the implementation enforces them, not just the docs:

1. **TCP is off by default.** Requires explicit `[gateway] enabled = true` in `daemon.json`. Users who only run `coven` CLI have no network listener.
2. **TCP `/v1/*` is a strict subset of Unix `/api/v1/*`.** Every TCP endpoint maps to a Unix endpoint and may *omit* fields or refuse certain methods; no TCP endpoint adds capability the Unix socket doesn't have.
3. **Privileged operations are Unix-socket only.** Hard list: key rotation, raw artifact decrypt, daemon shutdown, `coven prune --aggressive`, session deletion, config writes.
4. **Bearer-token check is constant-time** and present on every TCP request. No "soft auth" mode.
5. **No CORS.** The TCP listener serves CastCodes (and other local consumers), not browsers. `Access-Control-Allow-*` headers are absent. Any browser-based consumer needs its own backend.
6. **No artifact bodies over TCP.** Encrypted artifact ciphertext and decrypted plaintext are never serialized to a TCP response. Metadata only.
7. **Bind only to `127.0.0.1`.** Never `0.0.0.0`. A future remote story moves the listener into a separate binary that does its own bind decisions.

## Endpoint subset table

Every entry is either fully supported, supported-with-fields-stripped, or absent on TCP.

| Path | Methods (Unix) | Methods (TCP) | TCP redactions vs Unix |
|---|---|---|---|
| `/health` | GET | GET | None |
| `/sessions` | GET, POST | GET, POST | POST body must not include privileged launch flags (e.g., `--allow-keychain`); future flags default-deny |
| `/sessions/:id` | GET | GET | None |
| `/sessions/:id/manifest` | GET | GET | None — manifest is already redacted-safe |
| `/sessions/:id/events` | GET (with cursor) | GET (with cursor) | None — events are already redacted at write time |
| `/sessions/:id/handoffs` | GET, POST | GET only | TCP cannot emit handoffs (read-only) |
| `/sessions/:id/input` | POST | POST | Body redacted before storage as usual |
| `/sessions/:id/kill` | POST | POST | None |
| `/sessions/:id/artifacts/:artifact_id` | GET (returns ciphertext or, with `?decrypt=true`, plaintext) | GET (metadata only: id, kind, byte_count, sha256, created_at, expires_at) | TCP **never** returns ciphertext or plaintext |
| `/sessions/:id/archive` | POST | absent | TCP cannot archive |
| `/sessions/:id/delete` | DELETE | absent | TCP cannot delete |
| `/cast` | POST | POST | None |
| `/cast-codes` | GET | GET | None |
| `/familiars`, `/skills`, `/memory`, `/research` | GET | GET | None |
| `/messages`, `/messages/stream` | absent | POST/WS | TCP-only convenience endpoints that wrap session create + event stream for the CastCodes Cast Agent flow |
| `/daemon/shutdown` | POST | absent | Unix-socket only |
| `/keys/rotate` | POST | absent | Unix-socket only |
| `/config` | GET, POST | GET | TCP cannot write config |
| `/prune` | POST | absent | Unix-socket only |

The `/v1/messages*` endpoints exist on TCP to satisfy what CastCodes' `GatewayClient` already calls (it expects streaming chat). They are TCP-only because they're an external-consumer convenience surface; the privileged path uses richer session-creation calls directly.

## Auth model

Coven generates `~/.coven/token` on first daemon start with `[gateway].enabled = true`. File mode 0600. The token is a 32-byte URL-safe base64 random value. CastCodes reads its own copy (also mode 0600 because it's the same file) and sends `Authorization: Bearer <token>`.

Rotation: `coven token rotate` (Unix-socket only). Generates a new token, writes the file atomically, sets the daemon's in-memory copy. Any in-flight CastCodes request with the old token fails with 401; CastCodes re-reads the file and retries.

No expiry on tokens in v1. Token lifetime equals file lifetime.

## Bind, port, address

- Bind: `127.0.0.1` only.
- Port: 3000 by default (matches CastCodes' current `COVEN_GATEWAY_URL` default).
- Configurable via `daemon.json`:
  ```toml
  [gateway]
  enabled = true
  bind   = "127.0.0.1"        # not user-overridable to a non-loopback in v1
  port   = 3000
  ```
- If `bind` is set to anything other than a loopback address, daemon refuses to start with a clear error. Future remote story is a separate binary.

## Health checks and discovery

CastCodes already polls `GET /health` every 30s. TCP `/health` response is identical to Unix `/health` response. CastCodes uses the health response to gate the "Coven Gateway online" pill — no additional discovery needed.

## What about Cast Agent's `/v1/messages/stream`?

CastCodes' `GatewayClient` opens a WebSocket to `/v1/messages/stream` for live chat (see `cast-codes/crates/cast_agent/src/gateway.rs`). This is the one TCP endpoint that has no Unix-socket counterpart. It's a thin convenience wrapper:

- WebSocket upgrade, server pushes JSON `MessageChunk` frames.
- Internally implemented as: spawn (or reuse) a Coven session for the message; forward the harness's stream-mode output as `MessageChunk::Delta` frames; terminate with `MessageChunk::Done` or `MessageChunk::Error`.
- The full session is recorded in the normal way (transcript events, redacted, with provenance). The WebSocket is just a live-render path — replay still goes through `/sessions/:id/events`.

## Acceptance for v1

The gateway wire is "v1 done" when:

1. Daemon starts with TCP disabled by default. `coven doctor` reports `[gateway] disabled` cleanly.
2. With `[gateway].enabled = true`, daemon binds 127.0.0.1:3000 and serves the subset in the table above.
3. `~/.coven/token` is auto-generated with mode 0600 on first enable.
4. Every privileged endpoint (those marked "absent" on TCP) returns 404 over TCP regardless of token.
5. `GET /v1/sessions/:id/artifacts/:artifact_id` returns metadata only; ciphertext is never serialized into the body.
6. CastCodes' current `GatewayClient` works against this surface without code changes (health, list sessions, messages, messages/stream).
7. A regression test exercises every Unix-socket-only endpoint over TCP and asserts 404.
