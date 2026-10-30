---
source_adjacent_reason: "Defines the endpoint contract for the Rust mobile gateway and opaque relay under issue 787."
---

# Returning-device transport contract

Status: **approved by Val to proceed with implementation and validation on 2026-10-22; the outbound relay client shipped in [#1551](https://github.com/OpenCoven/coven/pull/1551) on 2026-10-28; the independent security review and the routing review remain open on [#1526](https://github.com/OpenCoven/coven/issues/1526)**.

Approval: [recorded on #1526](https://github.com/OpenCoven/coven/issues/1526#issuecomment-6349914514). The approval covers this contract direction and operator-configured endpoint policy. It is not evidence of cryptographic correctness, passing acceptance tests, or hosted deployment authorization.

Parent: [#787](https://github.com/OpenCoven/coven/issues/787). Read with [pairing v1](mobile-device-pairing-v1.md) and the [delivery plan](mobile-device-pairing-delivery-plan.md). This proposal neither approves a hosted service nor proves runtime acceptance.

## Current boundary

`coven-relay` implements bounded opaque WebSocket forwarding. It does not authenticate application peers or encrypt their payloads. Its room credential controls routing only. `identity.rs` persists a P-256 host key and derives its fingerprint from the uncompressed public key, not the certificate bytes. `auth.rs` verifies signed requests and rechecks current registry/grant state before releasing a response. These remain authoritative.

Since #1551, `mobile_memory/transport/` implements the candidate channel below as `Noise_NK_25519_ChaChaPoly_SHA256` with the P-256 binding, `transport/storage.rs` holds the private relay configuration and pairwise material, and `gateway/relay_runtime.rs` runs daemon-owned outbound workers. The shipped rendezvous is the v1 derivation: one room and credential per pairing, transport and 600-second slot of the host clock, with no clock-skew window, consumed-room tracking or offline recovery; a peer in another slot is never paired and recovers by rejoining the current slot. An authenticated relay channel is closed after 15 seconds without a request and the worker reconnects. The shipped behavior and its evidence are described in [`transport/README.md`](../../crates/coven-cli/src/mobile_memory/transport/README.md); the routing construction that replaces the v1 slots is the open review on #1526.

The protocol architecture specifies Noise/X25519 but the existing mobile keys are P-256. Do not reinterpret, convert, replace, or silently rotate existing identity keys to fit a transport library. A reviewed binding is required.

## Approved direction and remaining review

1. **Val: endpoint ownership (approved).** Use explicit operator-configured relay URLs, disabled by default, with no built-in OpenCoven endpoint. Tests use a local relay. Self-hosted operation is supported. Selecting an OpenCoven hosted endpoint, its operator and its deployment remains a separate decision.
2. **Security review: channel profile.** Review the candidate below, including P-256-to-Noise key binding and identity confidentiality. Approve an exact wire profile and independent vectors before production implementation.
3. **Security/product review: bootstrap and routing.** Approve an authenticated upgrade path for existing devices and a bounded rotating rendezvous scheme. Missing routing material must not prompt a recurring QR or fall back to cloud-account trust. The recovery behavior below needs explicit acceptance.

Record decisions with reviewer, exact document revision, scope and unresolved conditions on the contract child issue. The recorded approval permits implementation and validation of this direction. Security review must assess the concrete wire construction and evidence before delivery; hosted deployment remains a separate decision.

## Configuration and trust ownership

Proposed versioned configuration separates direct listener settings from `relay { enabled, endpoints }`. Existing configuration remains valid and does not implicitly enable relay access. Unknown versions/fields fail closed; updates use the existing private-directory and atomic-write protections. Secrets live in separate protected state, never in URLs or diagnostics.

Production endpoints must be explicit `wss` URLs with validated TLS hostname and chain; reject credentials, query/fragment, redirects and implicit endpoint substitution. The configured path is `/ws` for the existing server. Test-only loopback `ws` must not become a production insecure-mode switch. Operator configuration controls destination/network access; a relay response or LAN advertisement cannot change that configuration. Endpoint changes never update pinned peer identities. Private/self-hosted certificate trust requires explicit operator configuration, never disabling TLS verification.

Outer TLS authenticates the configured broker only. The paired installation key, pairwise device key, current device registry and grant policy authenticate and authorize peers. Rust owns all decisions. A relay can deny service and see addresses, timing, sizes and room occupancy; it must neither obtain application plaintext nor mint authority. No availability or traffic-analysis anonymity is claimed against the relay operator.

## Candidate channel for security review

Propose `Noise_NK_25519_ChaChaPoly_SHA256`, with device as initiator and Coven as responder. The responder Noise key is scoped to the pairing relationship and certified by the existing pinned installation P-256 key. The device signing key remains the existing pairwise P-256 credential. A new Noise key is a transport credential, not a new installation or device identity.

Noise NK requires the initiator to know the responder key before starting. Provision its signed binding over the existing pinned, authenticated direct channel or during confirmed first enrollment. Never fetch an unverified binding from the broker and treat it as trusted. Bind the certificate to the trust domain, installation fingerprint, pairwise relationship, profile, key generation and validity. Rotation is authenticated under existing trust with rollback protection; unknown or stale generations fail closed. An existing device that has never received routing/key material cannot magically reconnect through a broker: its first upgrade needs an existing authenticated direct connection. No QR is required for that upgrade. Whether a separately authenticated remote bootstrap is required is an explicit product decision, not an assumed capability.

The reference is the [Noise specification](https://noiseprotocol.org/noise.html), particularly handshake patterns, channel binding and application responsibilities. Noise supplies the cryptographic channel; application trust bindings and current authorization still require review and verification. Do not implement a bespoke replacement for Noise or claim this candidate has already passed crypto review.

Proposed connection sequence:

1. Both peers join a matching opaque room. The public Noise prologue binds the application domain, wire/profile versions, roles, selected transport and room identifier (or fresh direct connection context). It contains no stable identity.
2. Initiator and responder perform a fresh Noise handshake, using fresh ephemeral X25519 keys on every connection. No device/grant identifiers or application requests are sent in early handshake payloads. Reject unknown profiles and downgrades; no fallback to unsigned HTTP or broker-terminated encryption.
3. Inside the resulting channel, Coven sends a fresh random challenge. Both peers exchange domain-separated deterministic-CBOR possession proofs binding the final handshake hash, both fresh challenges, roles, trust domain, installation fingerprint, device key/reference, current grant reference/epoch and profile. Coven signs with its pinned P-256 identity; the phone signs with its enrolled P-256 possession key. Verify the host proof before sending device identity. Each challenge is single-use, connection-bound and expires on a monotonic deadline; close destroys all pending challenge and ephemeral state.
4. Coven validates the current stored grant and registry, including audience, validity, suspension, revocation, superseded key, scopes and assurance policy. Both peers send channel-bound completion acknowledgments. Only then is the connection authenticated; no application request is admitted earlier.
5. Carry the existing exact per-request signatures inside encrypted records. Channel authentication does not replace request signatures, nonce replay checks, assurance proofs or `ensure_still_active_at`. Bind the authenticated channel device to each request's device ID. Share replay/rate state across direct and relay ingress. Apply the same request-target rules on both ingresses: a query-string target is rejected before routing or verification, never stripped (#1610). Recheck before effects/responses, not merely at connection establishment.

Before implementation, security review must freeze canonical object schemas, proof ordering and signed bytes, challenge lifetime, certificate rotation and rollback rules, and conformance vectors. These are approval deliverables, not parameters to guess during coding. In particular, confirm that the pairing-scoped Noise key and public prologue do not permit cross-Coven correlation.

## Framing and failure semantics

Use one bounded binary message per encrypted record, below Noise's 65,535-byte message limit and the relay's limits. Reject text and oversized/noncanonical messages before allocation or dispatch. The wire profile must fix fragmentation, maximum request size, bounded reassembly, concurrent handshakes, queues and backoff. Use ordered record counters; duplicate, reordered, modified or truncated records abort the channel. Never reuse transport keys/counters after disconnect. No early application data, zero-RTT authorization or transparent action replay. A dropped response has unknown outcome; reconcile an operation before retrying.

Errors and doctor expose coarse connection/authorization state only. Logs and metrics exclude room IDs, credentials, discovery identifiers, keys, grants, proofs, raw URLs and plaintext, including dependency debug logs. Cancellation, timeout and malformed input must release bounded resources and erase secrets.

## Rendezvous and resumption

The current server requires both sides to already know a 32-byte room identifier and separate 32-byte credential. It is not a durable directory or an offline mailbox. Provision independent pairwise routing material over an authenticated channel; never derive it from a device name, public key, owner identifier or IP. Room credentials remain bearer routing secrets and never authorize application access. This is distinct from any application resumption credential.

The routing review must specify domain-separated derivation or preprovisioned single-use rooms, rotation intervals, clock-skew windows, consumed-room tracking, restart persistence and bounded recovery when peers miss rotations. Require independent material across Covens and devices, and rotation after revocation. A stale room may at worst route an unauthenticated attempt; it cannot restore revoked authority. Do not claim seamless reconnection until restart, missed-window and prolonged-offline cases work without re-enrollment. The v1 slot derivation shipped as an explicitly interim construction; the reviewed construction replaces it before seamless-reconnect acceptance is claimed.

Every reconnect performs fresh possession proof and fresh key agreement. An optional resumption handle references the pairwise device key, audience, grant and revocation generation, expires, and is usable only after fresh proof by that key. No handle skips current registry/grant checks or permits early data. Tests must steal, replay and race stale handles after policy changes. No resumption optimization is needed to eliminate recurring QR, but the sender-constrained resumption contract and its evidence remain required by #787.

## Connection state, LAN and privacy

Connection states are `offline`, `connecting`, `authenticating`, and `online`, with selected transport as separate metadata. Authorization remains independently active, suspended, expired, revoked or key-rotated. Network failure never revokes or re-enrolls a device; reaching `online` never clears an authorization failure.

A migration opens a freshly authenticated channel to the same pinned identity before handing over work. An old connection cannot transfer its authority or assurance implicitly. Direct and relay use the same end-to-end channel and request verifier. Preserve pending operation identity and prevent duplicate side effects during overlap. A failed candidate can trigger another configured candidate, but never weaker authentication.

LAN service instance, target hostname, TXT records and address announcements must be audited together: an opaque TXT value alone is insufficient. Advertise only rotating opaque application identifiers, no owner/familiar/device names, stable public keys or globally reused identifiers. Discovery chooses candidates; forged advertisements cannot authenticate peers or change trust. Direct candidates use the authenticated channel; direct failure triggers configured relay fallback. Keep LAN discovery and local authority usable with DNS/cloud/relay/push blocked. Platform network addresses and timing remain a separately documented correlation limit; do not promise network-layer anonymity.

Push carries only bounded opaque wake-up/routing hints. Fetch and sign exact approval requests over E2EE with current assurance policy. Forged/replayed pushes must have no authorization effect. BLE remains optional proximity research with an authenticated first-introduction ceremony, not a substitute transport or key.

## Acceptance and evidence plan

| Parent requirement | Required evidence; currently missing |
| --- | --- |
| Returning users normally need no QR | Two persistent enrolled peers; initial authenticated upgrade, restart and prolonged-offline reconnect, no QR/new grant; NAT-separated run and client UX evidence |
| Cryptographic recognition only | Wrong key with copied name/IP/grant/room fails; correct key after address/name changes succeeds; both endpoint proofs checked |
| No stable identity in LAN discovery | Captured full DNS-SD/mDNS records across rotations, restart, two devices and two Covens; active-probe review |
| Transport changes preserve auth | Wi-Fi/cellular/direct/relay transitions under traffic; fresh keys, unchanged peer identity, no downgrade or duplicate actions |
| Revocation defeats stale resumption | Revoke/suspend/expire/rotate/tighten between challenge, completion and response; stolen handle/grant, concurrent reconnect and process restart cases |
| Local use with cloud down | Real local peers with external services blocked; LAN discovery/direct auth and useful operation; recovery without re-enrollment |

Malicious-relay fixtures must exercise real wire traffic and controlled substitution, replay, reflection, reorder, truncation, injection, downgrade, disconnect and slow peers. Inspect captured bytes for plaintext and stable application identities; absence of a marker alone is not cryptographic proof. Pair negative cases with successful controls and deterministic synchronization. Local fixtures cannot substitute for real network/mobile acceptance or independent security review.

Each child issue must retain exact source revision, tests, limitations and delivery links. Full local CI, Policy guard, dependency audit, signed and signed-off commits, security review and protected CI remain required before delivery. New WebSocket, Noise and discovery dependencies need maintenance/license/target-support review and justification; an audit failure stops implementation, not the audit.

## Phase ownership

1. [#1526](https://github.com/OpenCoven/coven/issues/1526): Approve returning-device endpoint and E2EE contract.
2. [#1527](https://github.com/OpenCoven/coven/issues/1527): Implement outbound encrypted relay client and malicious-relay fixture.
3. [#1528](https://github.com/OpenCoven/coven/issues/1528): Implement offline state and authenticated network migration.
4. [#1529](https://github.com/OpenCoven/coven/issues/1529): Implement private LAN discovery and authenticated relay fallback.
5. [#1530](https://github.com/OpenCoven/coven/issues/1530): Prove multi-Coven unlinkability.
6. [#1531](https://github.com/OpenCoven/coven/issues/1531): Implement push-assisted authenticated approval.
7. [#1532](https://github.com/OpenCoven/coven/issues/1532): Research BLE-assisted first introduction.
