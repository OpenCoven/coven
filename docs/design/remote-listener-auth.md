---
title: "Authenticated remote listener for the daemon"
description: "Design for an opt-in, TLS-wrapped, device-paired remote listener so companion clients (Coven Pocket) can reach the daemon without a hand-managed SSH or Tailscale tunnel."
---

# Authenticated Remote Listener — Design

Status: Proposed — needs maintainer ratification of the open questions
Date: 2026-07-26
Scope: `crates/coven-cli` (`daemon.rs`, `api.rs`, `main.rs`, store) plus
`docs/AUTH.md` / `docs/daemon/*` updates when implemented. Companion client:
OpenCoven/coven-pocket (tracked there as coven-pocket#6). Tracking: #463.

## Summary

Today the daemon's only trust boundary is same-user filesystem access to the
Unix socket; the optional TCP listener is loopback-only and unauthenticated,
and remote reachability is delegated to SSH or Tailscale
([Remote access](/daemon/remote-access)). That is the deliberate MVP posture —
[AUTH.md](/AUTH) explicitly forbids tunneling the raw socket into a
network service and calling it authenticated.

This design adds the "separate auth design" AUTH.md calls for: an **opt-in
remote listener** (configured via `coven daemon start --remote <bind>` or
settings) that is TLS-wrapped, accepts only **paired devices** holding
per-device bearer tokens (hashed at rest), scopes each device to *observe*
or *control*, downgrades unauthenticated connections to a redacted `/health`, and supports immediate
revocation. Loopback-only remains the default; nothing changes for existing
local clients or for the tunnel-based patterns, which stay documented and
supported.

## Non-goals

- **Internet-facing relay or broker infrastructure.** The listener assumes
  direct reachability (LAN, Tailscale/WireGuard overlay, or port-forward the
  operator owns). No hosted rendezvous service.
- **Multi-user daemons.** The trust boundary stays "one user's devices". A
  paired phone is the *same* principal as the desktop user, possibly with a
  narrower scope — never a second user with separate data visibility.
- **Replacing the local socket model.** The Unix socket (and Windows named
  pipe) remain the primary transport and keep their existing
  filesystem-permission trust anchor.
- **OAuth / OIDC / hosted accounts.** Pairing is device-to-daemon, offline,
  operator-initiated. No third-party identity provider enters the loop.

## Current state (verified on main @ 3d2e54b)

What already exists — the remote listener composes with all of it:

- **Loopback-only TCP transport** (`daemon.rs`): `bind_tcp_listener` resolves
  the bind address and `ensure_loopback_addrs` refuses any non-loopback
  socket address outright. `--allow-host` widens only the *Host/Origin*
  guard (`HostGuard::Loopback`), never the bind.
- **Browser-attack defenses on TCP**: exact-match Host allowlist plus Origin
  check defend against CSRF and DNS rebinding; `TCP_IO_TIMEOUT` (30 s
  read/write) and `MAX_TCP_BODY_BYTES` (1 MiB) cap slowloris and allocation
  abuse. The remote listener inherits all of these.
- **Fail-closed local trust anchor**: `ensure_private_coven_home` refuses a
  `COVEN_HOME` not owned by the current uid (`check_owned_by_current_user`)
  before binding the socket. The remote listener's key material lives under
  the same directory and inherits the same posture.
- **Versioned protocol handshake**: clients probe `GET /health`, which
  returns `apiVersion: "coven.daemon.v1"`, the coven version, a capabilities
  block, `DaemonStatus` (including **pid** and socket path), and a hub
  summary. `X-Coven-Api-Version` negotiation and structured error envelopes
  already exist ([API contract](/reference/api-contract)).
- **Pocket MVP** (coven-pocket#6): the phone reaches a loopback daemon over
  Tailscale or `ssh -L`, probing `/health`. It authenticates at the network
  layer only.

## Design

### 1. Opt-in listener

```sh
coven daemon start --remote 0.0.0.0:7443        # explicit flag, or
# settings.json: { "daemon": { "remote": { "bind": "0.0.0.0:7443" } } }
```

- A **separate listener** from `--tcp`; the loopback listener and its
  `--allow-host` guard are untouched. `--remote` without at least one paired
  device logs a warning and still serves (pairing happens over it — §3).
- The remote listener **refuses to start without TLS material** (§2) — there
  is no plaintext-remote mode. Operators who want to terminate security at
  Tailscale/WireGuard instead keep using the existing loopback +
  `--allow-host` pattern, which stays documented as the tunnel alternative.
- `HostGuard` is `Disabled` on this listener (Host pinning is meaningless for
  a non-loopback bind); the browser threat model is instead closed by TLS +
  token auth, and `Origin`-bearing requests are rejected outright — companion
  clients are native apps, not browsers.

### 2. Transport security: TLS with pairing-time pinning

- First `--remote` start generates a self-signed server certificate
  (`rcgen`), stored at `<covenHome>/tls/remote-{cert,key}.pem` with `0600`
  permissions, covered by the existing `ensure_private_coven_home` check.
- The certificate's SHA-256 fingerprint is embedded in the pairing payload
  (§3). Clients **pin the fingerprint at pairing time** and refuse any other
  presented certificate thereafter — trust-on-first-pair, not
  trust-on-first-use: the fingerprint travels out-of-band in the QR code, so
  an active MITM during pairing is excluded rather than merely unlikely.
- v1 uses `rustls` server-side only. **Path to mTLS (v2)**: pairing mints a
  per-device client certificate alongside the token; the listener then
  requires client certs and tokens become a secondary check. The pairing
  payload format (§3) reserves a field for this so v1 QR codes stay valid.

### 3. Pairing and per-device tokens

```sh
coven daemon pair                 # opens a 120-second pairing window
```

- Prints a QR code (and the equivalent URL) embedding:
  `coven-pair://<host>:<port>?fp=<cert-sha256>&pin=<8-digit-code>&v=1`.
  The phone connects over TLS (pinning `fp`), presents the short-lived
  `pin` to `POST /pair`, and receives its **device token** in the response:
  `cvnd_<device-id>_<32-byte-secret>`. The pin is single-use and expires
  with the window; `POST /pair` is the only route (besides redacted
  `/health`) an unauthenticated connection can reach, and only while a
  window is open.
- The daemon stores `{ device_id, name, token_sha256, scope, created_at,
  last_seen_at }` in a new `paired_devices` store table. **Only the SHA-256
  of the secret is persisted**; comparison is constant-time. The plaintext
  token exists once, in the pairing response.
- Requests authenticate with `Authorization: Bearer cvnd_…`. The device id
  prefix selects the row; the hash comparison verifies the secret. Failures
  return the structured `401 unauthorized` envelope and are rate-limited
  per source address (token brute-force is already hopeless at 32 random
  bytes, but the limiter keeps the log signal clean).

### 4. Handshake gating and /health redaction

- Unauthenticated connections on the remote listener get exactly two
  routes: `GET /health` and `POST /pair` (window-gated). Everything else is
  `401` before any handler runs.
- Remote `/health` is **redacted**: `{ ok, apiVersion }` only — no pid, no
  socket path, no coven version, no capabilities, no hub summary. That is
  enough for Pocket's reachability probe and version handshake while
  keeping process details off an unauthenticated network surface. The
  full body returns once the bearer token is presented.
- Authenticated requests then follow the normal `coven.daemon.v1`
  handshake: same `X-Coven-Api-Version` negotiation, same structured
  errors, same routes — the remote listener adds authentication, not a
  second protocol.

### 5. Scoped authorization

Each paired device carries a scope, chosen at pairing (`coven daemon pair
--scope observe|control`, default `observe`):

- **observe** — read-only: `GET` sessions/events/health/capabilities and
  SSE attach in read-only mode.
- **control** — everything observe grants plus launch (`POST /sessions`),
  input forwarding, approvals, and kill.

Enforcement is an **explicit allowlist of observe-safe routes** in the
router, checked after authentication and before dispatch. Anything not on
the allowlist requires `control` — new routes are therefore
control-scoped (fail closed) until someone deliberately classifies them.
Scope violations return the structured `403 forbidden` envelope with the
required scope named.

### 6. Revocation and inspection

```sh
coven daemon devices list            # id, name, scope, created, last seen
coven daemon devices revoke <id>     # immediate
```

Revocation deletes the row; because every request re-checks the token
hash against the store, a revoked device loses access on its next
request — no token expiry machinery needed in v1 (tokens are
long-lived-until-revoked, which matches the "one user's devices" trust
model). `pair` re-runs replace a device's token (re-pair after a lost
phone plus revoke covers rotation).

## Rollout

1. **v1**: TLS listener + pairing + hashed bearer tokens + scopes +
   revocation + redacted health. Pocket switches its companion mode probe
   to the paired transport; tunnel patterns remain documented for users
   who prefer them.
2. **v2**: mTLS client certificates minted at pairing (payload field
   reserved), tightening the transport story from "pinned server cert +
   bearer" to mutual certificate auth.

## Open questions (need maintainer decisions)

1. **Dependency budget**: `rustls` + `rcgen` are new dependencies for the
   CLI crate. Acceptable, or should the TLS layer live behind a feature
   flag so the default build stays dependency-lean?
2. **Pairing pin transport**: is the 8-digit single-use pin inside the QR
   enough, or should pairing also require confirming a match code shown on
   both screens (SAS-style) before the token is released?
3. **Store vs sidecar file** for `paired_devices`: the store table gets
   transactions and the existing backup story; a sidecar
   `devices.json` would keep auth material out of the SQLite file that
   other tooling opens. Leaning store table.
4. **Scope granularity**: is observe/control enough for v1, or does
   Pocket's approval flow need approvals split out of `control` (a device
   that may approve but not launch)?

## Prior art referenced

- `crates/coven-cli/src/daemon.rs` — TCP transport, loopback guard,
  Host/Origin guard, timeouts, body caps, fail-closed uid checks.
- [`docs/AUTH.md`](/AUTH) — current same-user posture and the explicit
  requirement that remote surfaces get a separate auth + pairing design.
- [Remote access](/daemon/remote-access) — the tunnel patterns this design
  complements (and does not replace).
- OpenCoven/coven-pocket#6 — the companion-mode MVP this unblocks.
