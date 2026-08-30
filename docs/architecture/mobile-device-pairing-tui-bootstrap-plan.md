# Coven TUI QR Bootstrap and End-to-End Encrypted Mobile Pairing — Plan

Status: Proposed plan (implementation has not started)
Tracks: #785 (parent architecture #784)
Governing protocol contract: [`mobile-device-pairing-v1.md`](mobile-device-pairing-v1.md)
Elaborates: PR 3 ("TUI enrollment and device administration") and the rendezvous slice of PR 4 in the [delivery plan](mobile-device-pairing-delivery-plan.md)

## 1. Purpose and scope

This plan turns the pairing protocol contract into a concrete, reviewable
implementation plan for the first-time mobile enrollment UX:

```text
$ coven device pair        → capability preview → QR → E2EE handshake over a
rendezvous relay → six-word phrase confirmed on both endpoints → scoped,
revocable device grant
```

In scope:

- the `coven device` command family (`pair`, `pair --scope`, status, cancel,
  device administration entry points);
- a canonical, versioned pairing offer encoded in deterministic CBOR and
  carried by a Universal Link plus the existing custom-scheme URL;
- an authenticated, forward-secret Noise handshake between TUI host and mobile
  device, with a transcript that binds the offer, keys, capabilities, nonces,
  endpoint identities, and protocol versions;
- a rendezvous/relay MVP so pairing works across NAT, SSH hosts, and
  restrictive networks using outbound connections from both endpoints;
- human verification (short authentication phrase) and explicit grant
  confirmation;
- the adversarial test matrix (replay, substitution, MITM, downgrade, relay,
  malformed input).

Out of scope (separate delivery-plan PRs, linked where they touch this plan):

- returning-device reconnection, local discovery, and push wake-up
  (delivery-plan PR 6);
- recovery, trusted-device introduction, and attestation (PR 7);
- the mobile client (Pocket) UI and platform key storage internals (PR 5);
- production rendezvous fleet operations beyond the single-reference relay.

The QR is an out-of-band introduction, never a reusable login token. Every
requirement in this plan defers to [`mobile-device-pairing-v1.md`](mobile-device-pairing-v1.md)
where the two disagree; conflicts should be resolved by amending that contract.

## 2. Current state and gap analysis

The mobile track already shipped a working, memory-scoped pairing flow. The
plan extends it; nothing here starts from zero.

### 2.1 What exists today

| Capability | Current implementation | Code path |
| --- | --- | --- |
| Terminal pairing command | `coven memory mobile pair` renders a QR invitation, polls status, and asks the operator to confirm a six-word phrase | `crates/coven-cli/src/mobile_memory/mod.rs` (`run_pair`, `run_pair_unix`), command enum `MobileMemoryCommand` in `crates/coven-cli/src/main.rs` |
| Pairing engine | Single-use nonce, expiry pruning, host+device phrase confirmation, idempotent completion, bounded retry windows | `crates/coven-cli/src/mobile_memory/pairing.rs` (`PairingManager`, `PendingPairing`, `PairingError`) |
| Pairing v2 offer | `coven-memory://pair` URL with versioned fields and a canonical offer digest over length-prefixed fields | `crates/coven-cli/src/mobile_memory/pairing.rs` (`build_pairing_url`, `PairingOfferV2::hash`), contract in [`docs/design/mobile-pairing-protocol-v2.md`](../design/mobile-pairing-protocol-v2.md) |
| Transcript binding v2 | Offer digest, selected/supported versions, device key, device name, and app version bound into a digest that derives the six-word phrase | `crates/coven-cli/src/mobile_memory/pairing.rs` (`PairingTranscript::V2`), fixture `crates/coven-cli/tests/fixtures/mobile-pairing-v2/transcript-vector.json` |
| QR rendering | Unicode half-block rendering of the pairing URL plus a printed copyable URL and expiry line | `crates/coven-cli/src/mobile_memory/pairing.rs` (`render_pairing_invitation`), `qrcode` crate 0.14 in `crates/coven-cli/Cargo.toml` |
| Device grant model | Versioned grant object with scopes, restrictions, assurance levels, audience, and exact-action intents | `crates/coven-cli/src/mobile_memory/grant.rs` (`DeviceGrant`, `DeviceScope`, `AssuranceLevel`, `DeviceActionIntent`) |
| Request authentication | Canonical signed requests with timestamp, nonce, and body digest; replay window; per-device rate limiting | `crates/coven-cli/src/mobile_memory/auth.rs` (`canonical_request`, `MobileAuthenticator`) |
| Host identity | Stable P-256 host key, self-signed certificate, SHA-256 public-key fingerprint pinned in the QR | `crates/coven-cli/src/mobile_memory/identity.rs` (`load_or_create_host_identity`, `HostIdentity`) |
| Mobile gateway | Private-network rustls TLS listener with bounded routes, body caps, and inflight-connection limits; 5-minute pairing lifetime | `crates/coven-cli/src/mobile_memory/gateway.rs` (`MobileRoute`, `PAIRING_LIFETIME`) |
| Device registry | Atomically persisted, privacy-guarded device records with revocation | `crates/coven-cli/src/mobile_memory/registry.rs` (`DeviceRegistry`, `DeviceRecord`) |
| Audit events | Structured pairing/authentication/revocation audit records | `crates/coven-cli/src/mobile_memory/audit.rs` (`MobileAuditEvent`) |
| Rendezvous relay | Standalone bounded opaque WebSocket room relay (one `host` + one `client`, constant-time credential check, frame/idle/queue caps) — not yet used by the CLI | `crates/coven-relay/src/main.rs`, `crates/coven-relay/src/ws.rs` |
| Protocol contract and schemas | v1 protocol contract, diagnostic JSON schemas, domain-separation and conformance notes | [`mobile-device-pairing-v1.md`](mobile-device-pairing-v1.md), `spec/device-pairing/v1/*` |
| Accepted architecture | Trust-chain decision record that explicitly extends `coven-cli::mobile_memory` | [`docs/design/mobile-device-trust.md`](../design/mobile-device-trust.md) |

### 2.2 Gap against issue #785

| Issue requirement | Today | Gap |
| --- | --- | --- |
| `coven device pair` / `--scope` | `coven memory mobile pair` with a fixed `memory_read` scope (`PAIRING_SCOPE_MEMORY_READ` in `pairing.rs`) | New top-level `device` command family; selectable, previewed scopes |
| Canonical CBOR offer + compact URL-safe encoding | URL query members (JSON-flavored, not CBOR) in `build_pairing_url` | Deterministic CBOR offer and base64url encoding per §5 |
| Universal Link/App Link | `coven-memory://pair` custom scheme only | HTTPS Universal Link carrying the offer in a fragment per §5.4 |
| Forward-secret E2EE handshake (Noise) | TLS 1.3 transport plus phrase confirmation; no application-layer AKEX, no session keys | Noise_XK handshake per §6; new crypto dependencies |
| Rendezvous for cross-network pairing | Gateway requires the phone to reach the host's advertised HTTPS endpoint | Relay session derived from the offer per §7; `coven-relay` already provides the room semantics |
| Countdown / status / cancel | Expiry printed once; Ctrl-C cancels the CLI loop only | Live countdown, explicit status/cancel commands, both-endpoint cancel per §8/§10 |
| Offer-bound capability approval | Phrase binds a fixed scope string | `requested_capabilities_hash` over the exact selected scope set, bound into offer and transcript per §5/§6/§11 |
| Short authentication phrase | Already implemented (six words, 2,048-word list, 66 bits) in `pairing.rs` | Re-derive from the Noise handshake hash per §9; keep six words |
| Bounded failed attempts | One enrollment attempt consumes the nonce (`pairing.rs` `enroll`); phrase failures destroy pending pairings | Add a bounded handshake-attempt counter per §8.4 |
| Replay, substitution, MITM, downgrade, relay, and malformed-input tests | Strong coverage for phrase and nonce paths in `pairing.rs` tests; no relay or handshake tests | Test matrix in §12 |

The mobile gateway, grants, registry, and audit survive unchanged as the
authority plane; this plan adds a transport and handshake layer in front of
them.

## 3. Target experience (golden example)

```text
$ coven device pair --scope sessions.metadata.read,messages.send

Pair OpenCoven Mobile
Requesting:
  ✓ View sessions            (sessions.metadata.read)
  ✓ Send messages            (messages.send)
  ✕ Execute tools without approval (not requested: tool_execution_approve)
  ✕ Export identity or memory (never grantable over pairing)

[ QR CODE ]

Link (if you cannot scan): https://pair.opencoven.ai/p#<offer>
Expires in 01:47
Status: waiting for device · [c] cancel
```

After the device connects and the handshake completes, both endpoints display
the same six words; the host confirms only on an exact match:

```text
Device "Val's iPhone" (app 1.0.0) requests the scopes above.

Compare these words with the device:
1. willow   2. cinder   3. moon   4. harbor   5. linen   6. ridge

[c]onfirm / [r]eject: c
Device enrolled. Grant id: 9f14... (revocable with `coven device revoke`)
```

The six-word phrase is the existing v2 mechanism (`phrase_for_hash` in
`pairing.rs`); the issue's three-word example (`willow-cinder-moon`) is
illustrative. Six words from a 2,048-word list carry 66 bits, which keeps the
phrase the strong second factor it is today; §15.5 recommends keeping six.

## 4. Design overview

### 4.1 Components and data flow

```text
┌────────────────────── TUI host (coven device pair) ─────────────────────┐
│  CLI: capability preview, QR render, countdown, confirm/cancel          │
│    │ local unix-socket control API (existing daemon)                    │
│  Daemon: PairingSession authority                                       │
│    · offer minting (CBOR), session store, attempt bounds                │
│    · Noise responder (X25519 static = host pairing key)                 │
│    · grant issuance via mobile_memory::grant, registry, audit           │
└───────┬─────────────────────────────────────────────────┬───────────────┘
        │ outbound WSS (rendezvous)                       │ optional direct
        ▼                                                 ▼ LAN TLS (existing)
┌─────────────────────── rendezvous relay ────────────────┐   gateway path
│  coven-relay: opaque room match + ciphertext forward    │
│  no plaintext, no keys, no authority                    │
└───────▲─────────────────────────────────────────────────┘
        │ outbound WSS
┌───────┴────────────── Mobile device ────────────────────┐
│  scan QR / open Universal Link → offer validation       │
│  Noise initiator, enrollment request signature,         │
│  phrase confirmation, grant receipt                     │
└─────────────────────────────────────────────────────────┘
```

Both endpoints make outbound connections only. The relay matches opaque room
identifiers and forwards binary frames; it never receives application
plaintext, keys, or grants. This is the delivery-plan PR 4 behavior applied to
pairing first.

### 4.2 What stays, what changes

Stays (authority plane unchanged):

- `mobile_memory::grant` issuance/verification semantics, scope vocabulary,
  assurance levels, and exact-action intents (`grant.rs`);
- the device registry, revocation, and audit surfaces (`registry.rs`,
  `audit.rs`);
- the direct-LAN TLS gateway as the high-bandwidth path after pairing
  (`gateway.rs`), with the phrase/handshake replacing "trust the LAN";
- request authentication for post-pairing API calls (`auth.rs`).

Changes (new or extended):

- a new `device` command family in `crates/coven-cli/src/main.rs` (§10);
- a new pairing-session authority module (proposed
  `crates/coven-cli/src/device_pairing/`) that owns offers, the Noise
  handshake, and relay transport, and hands confirmed enrollments to
  `mobile_memory::grant` + `registry`;
- CBOR offer encoding and Universal Link rendering (§5);
- new crate dependencies: `snow` (Noise), `x25519-dalek`, `hkdf`, `ciborium`
  or `serde_cbor`-successor for deterministic CBOR (§15.2);
- `coven-relay` gains nothing conceptually — the CLI becomes its second
  consumer; only small additions for derived-room validation if needed (§15.6).

## 5. Pairing offer (version 1, deterministic CBOR)

The offer follows the contract's `PairingOffer` (mobile-device-pairing-v1.md,
"Pairing offer") with concrete encodings. The existing v2 URL offer remains
accepted for one deprecation window (§15.7).

### 5.1 Canonical CBOR layout

Deterministic CBOR per RFC 8949 §4.2.1 (core deterministic encoding): map
keys in bytewise lexicographic order, shortest-form integers, no indefinite
lengths. All bstr fields are fixed length, so no length ambiguity exists.

| Field | CBOR key (text string) | Type | Notes |
| --- | --- | --- | --- |
| version | `"v"` | uint (1) | Offer format version; handshake protocol version negotiated separately (§6) |
| pairing_session | `"s"` | bstr 32 | Cryptographically random, single-use session id (room derivation input, §7.1) |
| ephemeral_public_key | `"k"` | COSE_Key map | X25519 host ephemeral key for this pairing attempt; fresh every attempt (issue checklist item 1) |
| host_static_key_id | `"hf"` | bstr 32 | SHA-256 fingerprint of the host's X25519 pairing static key; the QR pin (§6.2) |
| rendezvous_hint | `"r"` | array of maps | Ordered transport hints (§7.2) |
| local_discovery_hint | `"d"` | tstr, optional | Opaque rotating local-discovery token; omitted in the MVP |
| requested_capabilities_hash | `"c"` | bstr 32 | SHA-256 over the canonical CBOR array of selected scope strings (§11.2) |
| expires_at | `"e"` | uint | Unix seconds; host rejects use after expiry (5-minute default, matching `PAIRING_LIFETIME` in `gateway.rs`) |

An offer is ~170 bytes in CBOR (~230 base64url characters), well inside QR
byte-mode capacity at ECC level M.

Forbidden in the offer (contract "MUST NOT" list, enforced by schema and
review): permanent API or bearer credentials, owner/familiar/installation
private keys, biometric material, hardware serials or advertising IDs, and
unnecessary identity metadata (no device name, owner name, or account id).

### 5.2 TypeScript types (diagnostic/tooling form)

Mirrors `spec/device-pairing/v1/pairing-offer.schema.json` (diagnostic JSON is
for tooling only; the wire format is CBOR):

```ts
export interface PairingOfferV1 {
  version: 1;
  /** 32-byte cryptographically random single-use session id, base64url. */
  pairingSession: string;
  /** COSE_Key (kty OKP, crv X25519), fresh per pairing attempt. */
  ephemeralPublicKey: CoseKey;
  /** SHA-256 fingerprint of the host's X25519 pairing static key, base64url. */
  hostStaticKeyId: string;
  rendezvousHints: RendezvousHint[];
  localDiscoveryHint?: string;
  /** SHA-256 over canonical CBOR of the selected scope string array. */
  requestedCapabilitiesHash: string;
  /** Unix seconds. */
  expiresAt: number;
}

export interface CoseKey {
  kty: "OKP";
  crv: "X25519";
  x: string; // base64url 32 bytes
}

export interface RendezvousHint {
  transport: "wss" | "https" | "local";
  endpoint: string;
  priority?: number; // 0..255, lower is preferred
}
```

The diagnostic JSON schema gains the same fields (`spec/device-pairing/v1/
pairing-offer.schema.json` already carries `rendezvous` and
`requestedCapabilitiesHash`; add `hostStaticKeyId`, keep `additionalProperties:
false`). Schema changes land with the implementation PR that emits them.

### 5.3 Single-use and expiry rules

- `pairing_session` is 32 bytes from the OS CSPRNG (same generator class as
  the existing `begin_pairing` nonce in `pairing.rs`).
- An offer is consumable by exactly one successful handshake. First use pins
  the session; second use fails closed (`PairingConsumed` semantics already
  proven in `pairing.rs` tests).
- Offers expire after 5 minutes (default; `--ttl` may shorten, never extend,
  with a hard maximum of 15 minutes).
- Terminal states destroy all pairing secrets (§8.4).

### 5.4 Universal Link and QR payloads

Primary payload — Universal Link with the offer in the URL fragment so
ordinary HTTP request processing never receives it (contract: "A Universal
Link/App Link MAY encode the offer in a URL fragment"):

```text
https://pair.opencoven.ai/p#<base64url(canonical CBOR offer)>
```

The fragment never reaches a server; the domain is a routing hint only (same
model as the existing `endpoint` member, which `mobile-pairing-protocol-v2.md`
excludes from the offer digest because the key fingerprint authenticates the
endpoint). The mobile client validates the offer digest and keys locally;
scanning a forged link fails at offer validation.

Secondary payload — compact custom scheme for terminal copy/paste without a
browser round-trip:

```text
coven://pair#<base64url(canonical CBOR offer)>
```

The printed link uses the Universal Link form; both decoders share one CBOR
validator. QR mode: byte mode, ECC level M, quiet zone 4 modules; the
terminal renderer keeps the existing `qrcode` half-block output
(`render_pairing_invitation`) with added blank-line padding and an
`aria`-style plain-text link fallback printed beside it (§10.4).

## 6. Handshake

### 6.1 Pattern evaluation

The contract requires "an established protocol/construction (evaluate Noise
patterns rather than inventing cryptography)". Who knows which static key
before the handshake decides the pattern:

| Pattern | Initiator (device) knows responder (host) static | Responder knows initiator static | Fit |
| --- | --- | --- | --- |
| Noise_XX | no | no | Works, but the host is authenticated only after the human phrase check; the QR pin is unused cryptography |
| Noise_KK | yes | yes | Fails: first-time enrollment means the host cannot pre-know the device static key |
| Noise_XK | yes (QR-pinned fingerprint) | no (learned encrypted in message 2) | Fits exactly: device authenticates the host cryptographically in message 2, before any human action |
| Noise_IK | yes | yes | Fails like KK |

**Recommendation: `Noise_XK_25519_ChaChaPoly_SHA256`** with:

- initiator = mobile device, responder = TUI host (the device scans, the host
  answers — matching the offer's direction of trust);
- responder static = a dedicated host **X25519 pairing key** (new; see §6.2),
  whose SHA-256 fingerprint is `host_static_key_id` in the offer;
- prologue = the canonical CBOR offer bytes (binds every offer field —
  including `requested_capabilities_hash` and expiry — into the handshake
  transcript, per issue checklist item 3);
- initiator static = the device's new durable **device identity key**. The
  architecture contract separates X25519 agreement from Ed25519 signatures;
  the enrollment signature key is Ed25519 (§6.4), and the Noise static binds
  the same device identity cryptographically. Implementations generate both
  keys at first enrollment and store them together.

Alternatives considered:

- **TLS 1.3 + phrase only (today's model):** proven, but there are no
  forward-secret application session keys off TLS, no binding of the offer
  into a cryptographic transcript beyond the digest, and no protection if the
  gateway TLS termination is ever exposed off-LAN; the contract asks for a
  Noise handshake.
- **Noise_XX + phrase:** one less host key to manage, but weakens the
  QR-pinned host authentication that already exists in v2
  (`fingerprint` member, `mobile-pairing-protocol-v2.md`).
- **Noise_KK with a pre-registered device key:** only applies to re-pairing a
  known device; use it later as the reconnection optimization, not first
  enrollment.

The handshake implementation MUST use the `snow` crate (the maintained,
widely reviewed Rust Noise implementation) rather than hand-rolled
Noise state machines (§15.2).

### 6.2 Host pairing key

The existing host identity is P-256 and is pinned by TLS certificate
fingerprint (`identity.rs`). Noise needs X25519. Do not convert P-256 keys to
X25519 (non-standard and error-prone); instead:

- generate a dedicated X25519 host pairing static key on first use, stored in
  the same private directory with the same atomic-write and permission
  discipline as `identity.rs` (`atomic_create_private`,
  `ensure_private_mobile_dir`);
- its SHA-256 fingerprint goes in the offer (`host_staticKeyId`), so a relay
  or MITM cannot substitute a host without forging the pinned key;
- rotate it only with an explicit operator action; rotation changes QR
  fingerprints and therefore requires a fresh offer — which pairing already
  is.

The host certificate fingerprint mechanism (`identity.rs` `public_key_fingerprint`)
remains for the direct-LAN TLS path and is unchanged.

### 6.3 Message flow over the rendezvous

```text
device → relay → host : Noise message 1 (e)                    [XK: -> e]
host  → relay → device: Noise message 2 (e, ee, s, es)         [XK: <- e, ee, s, es]
device → relay → host : Noise message 3 (s, se) + enrollment   [XK: -> s, se]
                       : encrypted frames both ways
```

- Message 3 carries the first encrypted application payload: the canonical
  enrollment request (transcript hash, Ed25519 device public key, requested
  scopes digest, device display name, app version, nonce, expiry) signed with
  the Ed25519 device key — the contract's "Device enrollment request".
- The host replies with the signed `DeviceGrant` (COSE_Sign1 semantics already
  modeled by `DeviceGrant` in `grant.rs`) or a rejection.
- Frames are length-prefixed (u32 big-endian) and capped at the relay's
  existing `MAX_FRAME_BYTES` (64 KiB); enrollment payloads are far smaller
  than `MAX_MOBILE_REQUEST_BYTES` (64 KiB in `mobile_memory/mod.rs`).
- Key material: Noise chaining key → HKDF-SHA-256 with domain string
  `COVEN-PAIR-SESSION/1` splits into the post-handshake transport keys. Both
  peers MUST zeroize handshake buffers and ephemeral secrets after use;
  `Zeroizing` (already used in `identity.rs`) is the storage discipline.

### 6.4 Transcript binding (issue checklist item 3)

The final handshake hash MUST cover, directly or via the prologue and
encrypted payloads:

1. offer format version and canonical CBOR offer bytes (prologue) — binds
   `pairing_session`, ephemeral key, host fingerprint, rendezvous hints,
   capabilities hash, expiry;
2. protocol version: Noise protocol name string plus the pairing protocol
   range (min/max) from both sides, exchanged inside message 3's encrypted
   payload (downgrade detection, §12);
3. host X25519 static (authenticated by XK message 2 against the QR pin);
4. device ephemeral keys (Noise-managed) and device Ed25519 enrollment key
   (message 3);
5. the exact requested capability digest, restated inside the signed
   enrollment request so the signature covers it independently;
6. fresh nonces from both peers (message-3 payload nonce + the existing
   enrollment nonce semantics);
7. both endpoint identity references (host fingerprint, device key digest).

Any mismatch aborts before any grant exists (contract: "Any mismatch MUST
abort the enrollment").

## 7. Rendezvous transport MVP

### 7.1 Room derivation from the offer

`coven-relay` rooms are `(32-byte room id, separate bearer credential, one
host + one client)` (`ws.rs`). Derive both from the offer's
`pairing_session` with domain separation:

```text
room_id     = SHA-256("COVEN-RENDEZVOUS-ROOM/1"  || pairing_session)  → base64url
room_token  = SHA-256("COVEN-RENDEZVOUS-TOKEN/1" || pairing_session)  → base64url
```

Both values are derivable only from the offer, so possession of the QR is the
capability to attempt pairing — which is exactly the threat model: the QR is
a short-lived single-use introduction, and a QR photographed by an attacker
still fails at host authentication (XK), phrase confirmation, and expiry.
The host creates the room (relay "first peer creates the room"); the device
joins as `client`. The relay sees only opaque ids and ciphertext.

`rendezvous_hint` entries name the relay URL(s), ordered by `priority`
(e.g. `wss://relay.opencoven.ai/ws`). The relay deployment URL is a
maintainer decision (§15.6). A `local` hint may advertise the direct gateway
endpoint for same-network fast paths; discovery is never authentication
(contract §"Local discovery and direct transport").

### 7.2 Frame and abuse bounds

Reuse the relay's existing bounds unchanged (they were built for this):
`MAX_MESSAGE_BYTES` 4 MiB, `MAX_FRAME_BYTES` 64 KiB, 120 s idle timeout,
bounded rooms/channels/queues, one host + one client per room, constant-time
credential comparison (`secret_eq` in `ws.rs`). Host-side additions:

- at most 3 handshake attempts per pairing session; the session is destroyed
  after the bound (§8.4);
- handshake frames must complete within 30 s of room join or the session is
  cancelled (stale-session cleanup matching the relay's idle timeout).

### 7.3 Direct-LAN fallback

If a `local` hint is present and reachable, the device MAY complete the same
Noise handshake over the existing TLS gateway socket instead of the relay
(transport swap, identical protocol). Changing transport MUST NOT change
endpoint identity or authorization (contract requirement). The MVP ships
relay-first with the direct path opportunistic; if the direct path is
unreachable the relay path is always available — this is what makes pairing
work across different networks (acceptance criterion 1).

## 8. Enrollment state machine

Extends the contract's state machine with the host-side session lifecycle.
Invalid transitions fail closed.

### 8.1 Host (TUI/daemon)

```text
IDLE ──pair──▶ OFFER_CREATED ──device joined room──▶ RENDEZVOUS_CONNECTED
  ▲                │ expired/cancelled                    │ handshake started
  │                ▼                                      ▼
  │            EXPIRED/CANCELLED                 HANDSHAKE_ESTABLISHED
  │                                                     │ both phrases confirmed
  │             any state ──cancel/expiry/failure──▶ CANCELLED/FAILED/EXPIRED
  ▼                                                     ▼
ENROLLED ◀──────────────────────────────────── GRANT_PENDING
```

### 8.2 Device

```text
OFFER_SCANNED ──validate──▶ RENDEZVOUS_CONNECTING ──joined──▶ HANDSHAKE_STARTED
     │ invalid/expired            │ unreachable (all hints)        │ complete
     ▼                            ▼                                ▼
  REJECTED                     FAILED                    PHRASE_PENDING ──match──▶ ENROLLED
                                                                │ mismatch ×N
                                                                ▼
                                                             REJECTED
```

### 8.3 Countdown, status, cancel

- The TUI renders a live `Expires in mm:ss` countdown from `expires_at` and a
  one-line status (`waiting for device`, `device connected`, `verifying`,
  `enrolled`, `cancelled`, `expired`) — extending the static output of
  `render_pairing_invitation`.
- `coven device pair` blocks until a terminal state; `c` or Ctrl-C cancels.
- `coven device status [--json]` reports the active pairing session (state,
  remaining seconds, connected device name once known).
- `coven device cancel` cancels the active session from the host. The device
  can cancel by closing the relay room or sending the (already encrypted)
  `cancel` frame; both destroy the session (acceptance criterion 5).
- The daemon-side control API gains the same endpoints as today's internal
  pairing routes (`POST /api/v1/internal/mobile/pairings…` in `mod.rs`), moved
  under the device-pairing module with status/cancel verbs.

### 8.4 Secret erasure and failure bounds (issue checklist items 5–6)

Every terminal state (`ENROLLED`, `EXPIRED`, `CANCELLED`, `REJECTED`,
`FAILED`) synchronously erases: the pairing session id, host ephemeral
private key, derived transport keys, and the pending-device record — the
`PendingPairing` lifecycle in `pairing.rs` already prunes this way and is the
template. Durables after success are exactly: the device registry record and
its grant (`registry.rs`, `grant.rs`). Nothing else persists.

Bounded failures: 3 handshake attempts per session (§7.2); a phrase mismatch
before completion destroys the pending pairing (existing behavior,
`incomplete_pairing_mismatch_invalidates_the_retry_window` test); enrollment
nonce single-use (existing behavior, `pairing_nonce_is_consumed_on_first_enrollment_attempt`).

## 9. Human verification

- Both endpoints derive the six-word phrase from the **final Noise handshake
  hash** (not the standalone transcript digest): HKDF-SHA-256 with info string
  `COVEN-PAIR-SAS/1` over the handshake hash, then the existing 66-bit → 6 ×
  11-bit word mapping (`phrase_for_hash`, 2,048-word list in
  `pairing_words.txt`). Deriving from the handshake hash binds the phrase to
  the full E2EE transcript automatically; the word list and rendering stay
  identical to today's UX.
- The phrase is displayed on both endpoints after `HANDSHAKE_ESTABLISHED`.
  The host requires typed confirmation (or `confirm` on the CLI) before
  `GRANT_PENDING` completes; the device requires explicit user confirmation
  too (existing two-sided confirmation semantics in `confirm()`).
- The phrase is defense in depth. In Noise_XK the host is already
  authenticated by the QR pin; the phrase catches a QR-substitution attack
  (attacker swaps the printed QR) which pin-verification alone would also
  catch — the phrase additionally catches wrong-endpoint pairing where the
  attacker holds a valid relay position but not the device's intent.
- Keep six words (66 bits) rather than the issue's three-word example;
  §15.5 records the tradeoff.

## 10. TUI command surface

### 10.1 Commands

```text
coven device pair [--scope LIST] [--ttl SECONDS] [--json]
coven device status [--json]          # active pairing session, if any
coven device cancel                   # cancel the active pairing session
coven device list [--json]            # paired devices (wraps mobile registry)
coven device inspect DEVICE_ID        # grant details for one device
coven device rename DEVICE_ID NAME
coven device revoke DEVICE_ID         # wraps registry revoke + audit event
```

`coven memory mobile …` remains as a thin compatibility alias for
enable/disable/status during the transition; new capability lives under
`coven device`. Rationale: the issue's target UX is `coven device pair`, the
delivery plan's PR 3 lists `coven device pair` and
`device list|inspect|rename|revoke`, and today's `MobileMemoryCommand`
(`main.rs`) couples device administration to the memory-gateway feature flag.

### 10.2 Scope selection and preview (issue: `--scope`)

- `--scope` accepts a comma-separated list from the `DeviceScope`
  vocabulary (`grant.rs`: `memory_read`, `session_metadata_read`,
  `conversation_read`, `message_send`, `tool_invocation_request`,
  `tool_execution_approve`, `secrets_read`, `familiar_memory_admin`,
  `device_admin`, `identity_admin`, `memory_export`, `identity_export`).
- Default (no flag): `session_metadata_read,messages_send` equivalent — the
  issue's "view sessions, send messages" preview — never a silent
  everything-grant.
- The preview renders each selected scope with ✓, and renders the salient
  withheld classes with ✕ (at minimum: tool execution without approval and
  any export class), matching the issue's target UX.
- `identity_export` and `memory_export` MUST be rejected by `device pair` in
  v1 (they remain registry-manageable for other flows) — the issue's
  "Export identity or memory" ✕ line is a hard rule, not styling.
- The exact selected set is hashed into `requested_capabilities_hash` (§5.1)
  and bound into the offer digest, the Noise prologue, and the signed
  enrollment request (§6.4) — the permission request is cryptographically
  bound to what the user approves (acceptance criterion 4).

### 10.3 Grant issuance

On successful confirmation, the daemon issues a `DeviceGrant`
(`grant.rs::DeviceGrant::for_device`) with:

- capabilities = the selected scope set (no broader — contract: "The grant
  MUST be no broader than the permissions displayed and approved");
- audience/restrictions per the existing restriction model (transport
  constraint may record `relay` for relay-paired devices);
- the grant id returned to the TUI for the confirmation line and audit event
  (`MobileAuditEvent::PairingCompleted` in `audit.rs`).

### 10.4 QR rendering and accessibility (issue checklist items 4–5)

- Keep the `qrcode`-crate unicode half-block renderer; add: blank-line quiet
  zone, automatic fallback to ASCII (`#`/space) when the terminal reports
  non-UTF-8, and a minimum-size check (offer URL ~230 chars → version ~11 QR
  at ECC M, still legible at typical TUI widths).
- Always print the Universal Link on its own line for copy/paste (existing
  behavior in `render_pairing_invitation`), plus `--json` output carrying
  `{link, expiresAt, scopes}` so scripted/assistive clients can surface it.
- Document screen-reader behavior in `coven-docs` (public docs), not here;
  this repo carries the contract, the public docs carry the tutorial.

## 11. Capability mapping

### 11.1 Contract vocabulary ↔ grant vocabulary

The contract (mobile-device-pairing-v1.md) uses dotted names; the
implementation uses `DeviceScope` snake_case (`grant.rs`). The mapping is
1:1 and total:

| Contract | `DeviceScope` |
| --- | --- |
| `sessions.metadata.read` | `session_metadata_read` |
| `conversations.read` | `conversation_read` |
| `messages.send` | `message_send` |
| `tools.request` | `tool_invocation_request` |
| `tools.approve` | `tool_execution_approve` |
| `secrets.read` | `secrets_read` |
| `memory.familiar.read` | `memory_read` (familiar-scoped via restrictions) |
| `memory.familiar.write` | `familiar_memory_admin` |
| `identity.admin` | `identity_admin` |
| `devices.enroll` / `devices.revoke` | `device_admin` |
| `identity.export` | `identity_export` (not pairable in v1) |
| `memory.export` | `memory_export` (not pairable in v1) |

### 11.2 Capabilities hash

```text
requested_capabilities_hash = SHA-256(canonical CBOR array of selected
                                       DeviceScope strings, in sorted order)
```

Sorted order makes the hash independent of CLI argument order; duplicates are
rejected at parse time (`validate_scope_set` in `grant.rs` already validates
scope sets — extend it with the pairable-subset rule from §10.2).

## 12. Test plan (issue checklist item 7 + contract "Required security tests")

New tests live next to the implementation: unit tests in the new
`device_pairing` module, integration tests under `crates/coven-cli/tests/`
(the existing `mobile-pairing-v2` fixture and `pairing.rs` test style are the
template), relay adversarial cases in `crates/coven-relay/src/ws/tests.rs`.

| Class | Case | Level |
| --- | --- | --- |
| Replay | Offer reuse after success fails (`PairingConsumed`) | unit |
| Replay | Offer reuse after expiry fails | unit |
| Replay | Duplicate enrollment request over a replayed message 3 | integration |
| Substitution | Any offer field change (session, key, capabilities hash, expiry) breaks the prologue → handshake abort | unit |
| Substitution | Device key/name/app-version substitution changes the SAS phrase (extends `pairing_v2_binds_offer_and_client_metadata`) | unit |
| MITM | Relay-position attacker with wrong host static fails at XK message 2 | integration |
| MITM | QR substitution: attacker's offer fails host pin check on device | unit |
| Downgrade | Peer offering min>current or max<current versions rejected (extends `unsupported_pairing_protocol_is_rejected`) | unit |
| Downgrade | Offer version 0 or unknown → fail closed (contract compatibility rule) | unit |
| Relay | Frame reorder, duplication, truncation, or injection → handshake fails or session resets; relay never sees plaintext (assert in relay shim) | integration |
| Malformed input | Non-canonical CBOR, unknown map keys, wrong bstr lengths, over-long strings → reject before any state change | unit |
| Bounded attempts | 4th handshake attempt on one session destroyed | unit |
| Cancel | Cancel from host mid-handshake and from device; secrets erased (assert zeroized buffers where the type system allows, and no registry row) | integration |
| Cross-correlation | Two independent Coven installs pairing the same device produce pairwise identifiers and unrelated sessions (contract cross-Coven check) | integration |
| Golden vectors | Offer CBOR bytes, room derivation, SAS phrase, and full transcript digest vectors under `crates/coven-cli/tests/fixtures/device-pairing-v1/` | integration |

Positive E2E: device and host complete pairing through an in-process relay
shim across two loopback namespaces (different "networks"), producing a
scoped grant whose capabilities equal the preview exactly.

## 13. Acceptance criteria mapping

| Issue acceptance criterion | Plan element |
| --- | --- |
| Pairing works when phone and TUI are on different networks | §7 relay MVP (outbound-only both endpoints); positive E2E test across namespaces (§12) |
| Relay compromise does not reveal plaintext or enable impersonation | §4.1 (relay sees ciphertext only), §6 (XK host authentication + forward secrecy), §12 relay tests |
| A captured QR cannot be reused after success/expiry | §5.3 single-use/expiry, §8.4 erasure, §12 replay tests |
| Permission request cryptographically bound to what the user approves | §5.1/§11.2 capabilities hash in offer + prologue + signed enrollment; §12 substitution tests |
| Cancellation from either endpoint leaves no durable credential | §8.3 both-endpoint cancel, §8.4 erasure, §12 cancel tests |
| Protocol versioned, downgrade tested | §5.1 `version`, §6.4 version binding, §12 downgrade tests |

## 14. Workstream graph and sequencing

| ID | Workstream | Depends on | Exit result |
| --- | --- | --- | --- |
| D1 | Offer codec + Universal Link + schemas | — | Deterministic CBOR round-trip, golden vectors, diagnostic schema updated; no authority changes |
| D2 | Host pairing key + Noise responder/initiator plumbing | D1 | XK handshake passes locally between two in-process peers; secrets zeroized; snow pinned |
| D3 | Relay session derivation + device/CLI transport | D2 | Pairing completes through a real `coven-relay` instance over loopback; direct-LAN fallback compiles behind the existing gateway |
| D4 | `coven device` command family + preview/countdown/status/cancel | D1 | Full UX per §10 with in-process fake device; compatibility alias for `memory mobile` |
| D5 | Grant issuance wiring + audit + registry integration | D2, D4 | Confirmed pairing produces a correctly scoped `DeviceGrant`; revoke paths intact |
| D6 | Adversarial matrix + golden vectors (§12) | D3, D5 | Every §12 row has a named passing test; CI green |

Dependency-gated: D6 blocks any release flag flip; the feature stays behind
the experimental capability gate per the delivery plan's rollout section
until D6 passes and revocation is exercised end to end.

Suggested PR slicing mirrors the delivery train: D1+D2 = one PR (protocol
library slice), D3+D5 = one PR (transport + authority wiring), D4 = one PR
(TUI), D6 = final conformance PR.

## 15. Maintainer decision points

Each item is a recommendation with alternatives; the maintainer owns the call.

### 15.1 Noise pattern
Recommend `Noise_XK_25519_ChaChaPoly_SHA256` (§6.1). Alternatives: XX (simpler
host key story, weaker QR-pin usage), KK (requires pre-registered device
keys). SHA-256 matches the contract's hash choice and the existing transcript
hash discipline; ChaChaPoly matches the contract's AEAD choice.

### 15.2 Cryptography crates
Recommend `snow` (Noise), `x25519-dalek` + `ed25519-dalek`, `hkdf`,
`chacha20poly1305` (already a dependency), and a deterministic CBOR codec
(`ciborium`). Alternatives: RustCrypto-only stack without snow (re-implements
Noise state machines — rejected per "evaluate Noise patterns rather than
inventing cryptography"); ring-only (no Noise framework). Exact versions are
an implementation PR decision, pinned in `Cargo.lock`.

### 15.3 Dedicated X25519 host pairing key vs reusing the P-256 identity
Recommend a dedicated X25519 pairing key (§6.2). Alternatives: P-256→X25519
conversion (non-standard; rejected), TLS-terminating and reusing the
certificate key for Noise (couples TLS and pairing attack surfaces).

### 15.4 Universal Link domain and path
Recommend `https://pair.opencoven.ai/p#…` (§5.4). Alternatives: a path-based
link on `opencoven.ai` ( couples marketing site deploys to pairing), or
custom-scheme-only (loses camera hand-off on iOS). The domain must be
owned by OpenCoven; the fragment carries all data, so the domain serves
no security role beyond branding and app-link ownership.

### 15.5 Phrase length
Recommend keeping six words (66 bits, existing list and rendering). The
issue's three-word example (33 bits) is guessable in ~2³³ attempts online if
rate limiting ever regresses; six words keep the phrase a real second factor
at negligible UX cost. Alternative: make length policy-configurable
(3/6 words) with a floor of 4 for interactive confirmation.

### 15.6 Reference relay deployment
Recommend one reference `wss://relay.opencoven.ai/ws` instance operated like
other OpenCoven infrastructure, with the relay implementation already in-tree
(`coven-relay`). Alternatives: self-hosted-only (works but fails the
"fast first-time UX" goal for non-operators), third-party relays (out — the
relay is part of the trust story even though it holds no authority).

### 15.7 Compatibility window for `coven-memory://pair` v2 offers
Recommend: new offers are CBOR/Universal Link; the gateway continues to
accept v2 URL offers for one release cycle with a deprecation audit event,
then rejects them (fail closed). Alternatives: permanent dual-accept (larger
attack/maintenance surface), immediate cutover (breaks shipped mobile
clients).

### 15.8 Where the module lives
Recommend a new `crates/coven-cli/src/device_pairing/` module that owns
offers/handshake/transport and calls into `mobile_memory::grant`/`registry`
(§4.2). Alternative: extend `mobile_memory/` in place (smaller diff, but
couples the general device trust plane to the memory-gateway feature flag —
the exact boundary `docs/design/mobile-device-trust.md` says to keep clean).

## 16. Risks

| Risk | Mitigation |
| --- | --- |
| New crypto dependencies expand the audit surface | Pin minimal, maintained crates (§15.2); golden vectors + adversarial matrix gate merge (D6) |
| Relay availability becomes a pairing SPOF | Direct-LAN fallback (§7.3); relay holds no authority so any instance works; hints are ordered with fallback |
| QR density on small terminals | ECC M + compact CBOR (~230-char URL) keeps version ≈11; ASCII fallback (§10.4) |
| Scope sprawl makes previews unreadable | Default scope set, hard-blocked export scopes, sorted vocabulary (§10.2) |
| Timing regressions in countdown/cancel | Wall-clock assertions follow the AGENTS.md jitter rules (discriminating thresholds at midpoints, hang guards far above load) |

## 17. References

- Protocol contract: [`mobile-device-pairing-v1.md`](mobile-device-pairing-v1.md)
- Delivery train: [`mobile-device-pairing-delivery-plan.md`](mobile-device-pairing-delivery-plan.md)
- Accepted architecture: [`../design/mobile-device-trust.md`](../design/mobile-device-trust.md)
- Shipped pairing v2 contract: [`../design/mobile-pairing-protocol-v2.md`](../design/mobile-pairing-protocol-v2.md)
- Diagnostic schemas: `spec/device-pairing/v1/` (especially
  `pairing-offer.schema.json`, `device-grant.schema.json`,
  `domain-separation.md`, `test-vectors.json`)
- Implementation under extension: `crates/coven-cli/src/mobile_memory/`
  (`pairing.rs`, `grant.rs`, `auth.rs`, `gateway.rs`, `registry.rs`,
  `identity.rs`, `audit.rs`)
- Rendezvous implementation: `crates/coven-relay/src/ws.rs`
- Issue: OpenCoven/coven#785 (parent #784)
