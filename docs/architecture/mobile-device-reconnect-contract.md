---
source_adjacent_reason: "Defines the endpoint contract for the Rust mobile gateway and opaque relay under issue 787."
---

# Returning-device transport contract

Status: **approved by Val to proceed with implementation and validation on 2026-10-22; the outbound relay client shipped in [#1551](https://github.com/OpenCoven/coven/pull/1551) on 2026-10-28; the wire profile and [v2 routing](#v2-routing-accepted-2026-10-30) were accepted by owner review (BunsDev, sole maintainer) on 2026-10-30 on [#1526](https://github.com/OpenCoven/coven/issues/1526); no independent third-party review has been performed, and v2 is not yet implemented**.

Approval: [recorded on #1526](https://github.com/OpenCoven/coven/issues/1526#issuecomment-6349914514). The approval covers this contract direction and operator-configured endpoint policy. It is not evidence of cryptographic correctness, passing acceptance tests, or hosted deployment authorization.

Parent: [#787](https://github.com/OpenCoven/coven/issues/787). Read with [pairing v1](mobile-device-pairing-v1.md) and the [delivery plan](mobile-device-pairing-delivery-plan.md). This proposal neither approves a hosted service nor proves runtime acceptance.

## Current boundary

`coven-relay` implements bounded opaque WebSocket forwarding. It does not authenticate application peers or encrypt their payloads. Its room credential controls routing only. `identity.rs` persists a P-256 host key and derives its fingerprint from the uncompressed public key, not the certificate bytes. `auth.rs` verifies signed requests and rechecks current registry/grant state before releasing a response. These remain authoritative.

Since #1551, `mobile_memory/transport/` implements the candidate channel below as `Noise_NK_25519_ChaChaPoly_SHA256` with the P-256 binding, `transport/storage.rs` holds the private relay configuration and pairwise material, and `gateway/relay_runtime.rs` runs daemon-owned outbound workers. The shipped rendezvous is the v1 derivation: one room and credential per pairing, transport and 600-second slot of the host clock, with no clock-skew window, consumed-room tracking or offline recovery; a peer in another slot is never paired and recovers by rejoining the current slot. An authenticated relay channel is closed after 15 seconds without a request and the worker reconnects. The shipped behavior and its evidence are described in [`transport/README.md`](../../crates/coven-cli/src/mobile_memory/transport/README.md); the routing construction that replaces the v1 slots is [v2 routing](#v2-routing-accepted-2026-10-30), accepted on #1526 and not yet implemented.

The protocol architecture specifies Noise/X25519 but the existing mobile keys are P-256. Do not reinterpret, convert, replace, or silently rotate existing identity keys to fit a transport library. A reviewed binding is required.

## Approved direction and remaining review

1. **Val: endpoint ownership (approved).** Use explicit operator-configured relay URLs, disabled by default, with no built-in OpenCoven endpoint. Tests use a local relay. Self-hosted operation is supported. Selecting an OpenCoven hosted endpoint, its operator and its deployment remains a separate decision.
2. **Security review: channel profile (accepted by owner review, 2026-10-30).** Review the candidate below, including P-256-to-Noise key binding and identity confidentiality. Approve an exact wire profile and independent vectors before production implementation.
3. **Security/product review: bootstrap and routing (v2 routing accepted by owner review, 2026-10-30; bootstrap upgrade path is implemented by the follow-up issue).** Approve an authenticated upgrade path for existing devices and a bounded rotating rendezvous scheme. Missing routing material must not prompt a recurring QR or fall back to cloud-account trust. The recovery behavior below needs explicit acceptance.

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
5. Carry the existing exact per-request signatures inside encrypted records. Channel authentication does not replace request signatures, nonce replay checks, assurance proofs or `ensure_still_active_at`. Bind the authenticated channel device to each request's device ID. Share replay/rate state across direct and relay ingress. Recheck before effects/responses, not merely at connection establishment.

Before implementation, security review must freeze canonical object schemas, proof ordering and signed bytes, challenge lifetime, certificate rotation and rollback rules, and conformance vectors. These are approval deliverables, not parameters to guess during coding. In particular, confirm that the pairing-scoped Noise key and public prologue do not permit cross-Coven correlation.

## Framing and failure semantics

Use one bounded binary message per encrypted record, below Noise's 65,535-byte message limit and the relay's limits. Reject text and oversized/noncanonical messages before allocation or dispatch. The wire profile must fix fragmentation, maximum request size, bounded reassembly, concurrent handshakes, queues and backoff. Use ordered record counters; duplicate, reordered, modified or truncated records abort the channel. Never reuse transport keys/counters after disconnect. No early application data, zero-RTT authorization or transparent action replay. A dropped response has unknown outcome; reconcile an operation before retrying.

Errors and doctor expose coarse connection/authorization state only. Logs and metrics exclude room IDs, credentials, discovery identifiers, keys, grants, proofs, raw URLs and plaintext, including dependency debug logs. Cancellation, timeout and malformed input must release bounded resources and erase secrets.

## Rendezvous and resumption

The current server requires both sides to already know a 32-byte room identifier and separate 32-byte credential. It is not a durable directory or an offline mailbox. Provision independent pairwise routing material over an authenticated channel; never derive it from a device name, public key, owner identifier or IP. Room credentials remain bearer routing secrets and never authorize application access. This is distinct from any application resumption credential.

The routing review must specify domain-separated derivation or preprovisioned single-use rooms, rotation intervals, clock-skew windows, consumed-room tracking, restart persistence and bounded recovery when peers miss rotations. Require independent material across Covens and devices, and rotation after revocation. A stale room may at worst route an unauthenticated attempt; it cannot restore revoked authority. Do not claim seamless reconnection until restart, missed-window and prolonged-offline cases work without re-enrollment. The v1 slot derivation shipped as an explicitly interim construction; the reviewed construction replaces it before seamless-reconnect acceptance is claimed. The accepted replacement is [v2 routing](#v2-routing-accepted-2026-10-30) below.

Every reconnect performs fresh possession proof and fresh key agreement. An optional resumption handle references the pairwise device key, audience, grant and revocation generation, expires, and is usable only after fresh proof by that key. No handle skips current registry/grant checks or permits early data. Tests must steal, replay and race stale handles after policy changes. No resumption optimization is needed to eliminate recurring QR, but the sender-constrained resumption contract and its evidence remain required by #787.

## v2 routing (accepted 2026-10-30)

This section is the routing construction proposed on #1526 ([proposal](https://github.com/OpenCoven/coven/issues/1526#issuecomment-6468641453), [architectural evaluation](https://github.com/OpenCoven/coven/issues/1526#issuecomment-6481038221), [revision 2](https://github.com/OpenCoven/coven/issues/1526#issuecomment-6481069847)). The reviewer, BunsDev ([recorded](https://github.com/OpenCoven/coven/issues/1526#issuecomment-6513213577)), accepted it on 2026-10-30 with the scope wire profile plus routing. The decision covers Option A with the host-proof `(g, c, k)` binding, `W = 4`, a global listener cap of 512, direct-only recovery, and a crash-consistency guarantee with a restore ceremony. It also confirms that the `Noise_NK_25519_ChaChaPoly_SHA256` / P-256 binding is unchanged. This is an owner review, not an independent third-party cryptographic review. The values here are frozen. Changing any of them needs a new decision on the contract issue. Implementation belongs to a follow-up issue. The shipped v1 code and the evidence from #1551 and #1620 are unchanged.

### State and derivation

Routing state is kept per pairing and per transport (`direct` or `relay`), in generation `g`. The device stores `d`, the next room it will try, and `f`, the highest host floor it has authenticated. Coven stores `c`, its committed floor; every room below `c` is consumed. All three start at `0` and are written durably with the 32-byte `routing_seed` and `g` in one authenticated provisioning handover. No room depends on the clock.

```
PRK  = HKDF-SHA256-Extract(salt = "coven.mobile.rendezvous.v2", IKM = routing_seed)
info = lp(transport) ‖ lp(label) ‖ u64be(g) ‖ u64be(n)        lp(x) = u8(len(x)) ‖ x
room = HKDF-SHA256-Expand(PRK, info with label "room", 32)
cred = HKDF-SHA256-Expand(PRK, info with label "credential", 32)
```

`transport` is the ASCII string `direct` or `relay`. v1 vectors stay valid for v1 only; v2 adds separate vectors.

### Consumption ordering

Coven serves rooms `[c, c+W)`. Before connecting, the device requires `d − f < W` and uses `k = d`.

1. Coven's possession proof in room `k` binds `(g, c, k)` into its signed deterministic CBOR, alongside the fields in [the connection sequence](#candidate-channel-for-security-review).
2. The device verifies the host proof and rejects it if `c > k`. In one durable write it sets `d = k+1` and `f = max(f, c)`. It then sends its own proof. If the write fails or is uncertain, it aborts without sending anything.
3. Coven verifies the device proof and takes the per-pairing lock. It re-reads `(g, c)`, rejects the attempt if `g` changed or `k < c`, and rechecks the registry and grant. It then durably sets `c = k+1`, which consumes every room through `k`, releases the lock and sends the completion acknowledgment.

No counter moves on a relay join, a Noise message or any traffic before the peer is authenticated, so forged or unauthenticated traffic cannot advance either counter. A failed attempt before step 2 retries the same room. Completion acknowledgments are advisory: the next host proof carries `c`, so lost completions do not exhaust the window. Only one consuming attempt per pairing and transport is in flight, serialized across tasks and processes by `.transport.lock`.

A durable write is: write a temporary file, `fsync` it, rename it into place, then `fsync` the directory. Any error at any of these steps is uncertain, and the side aborts before its next protocol message. Every surviving image is safe:

- the device's old `d` retries a room Coven has not committed;
- the device's new `d`, or Coven's old `c`, leaves the device inside the window;
- Coven's new `c` matches the device.

Counters never wrap. If `k+1` or `c+W` would overflow `u64`, the pairing requires a generation handover. If `g+1` would overflow, it requires re-pairing.

### Window and budget

`W = 4`. It tolerates three device-only advances followed by a successful fourth attempt. It is a margin for mobile failures, not protection against relay malice. Measured reconnect behavior is part of the delivery evidence, and if the measurements contradict `W = 4`, changing it needs a new decision. A relay that forwards a genuine host proof and drops the device proof can advance the device by one room per attempt. After `W` such attempts the device moves to recovery. This bounded denial of service is accepted; it needs no key compromise, and it exhausts nothing beyond one pairing's window.

Coven listens on at most `W` rooms per pairing, per transport, per endpoint. At the current 32-pairing worker cap that is 128 rooms on one endpoint, or 512 across four endpoints. The global listener cap is 512 rooms per Coven installation across all endpoints. It is configurable downward only. Past the cap, Coven refuses new listeners instead of evicting ones it already holds. The existing connect budget and jittered backoff still apply to handshakes. Concurrent joins are correlatable by the relay; this falls within the existing no-traffic-analysis-anonymity boundary. The challenge lifetime is a 30-second monotonic deadline.

### Recovery, rotation and rollback

Recovery is **direct-channel only**:

- **Intact counters:** over a pinned authenticated direct channel, the device sends `d` and Coven sends `c`. Coven durably sets `c = max(c, d)` and replies `(g, c)`. The device sets `d = f = c`. Neither side lowers a counter or reopens a consumed room.
- **Missing, corrupt or uncertain state:** a generation handover. Coven durably writes a pending `(g+1, seed)` and sends it. The device durably adopts it with zeroed counters and acknowledges. Coven then commits `g+1`, which closes every generation-`g` room. Until the commit, Coven resends the same pending value. The device accepts that value again and rejects any other `g+1`, and Coven rejects any generation below its committed `g`.

A device that is offline for months resumes at its stored `d` while its grant and binding remain valid. A remote-only device stays unreachable after window exhaustion, state loss or an interrupted rotation until it reaches a direct path. This is a stated availability limit. Remote recovery, if required later, is a separately configured, default-off, **reusable** routing mode with its own review; it is never described as single-use.

Revocation, suspension and expiry stop Coven listening on the pairing's rooms at once. A re-grant, a host or device key rotation, or a revocation-epoch change forces a generation handover. Routing never confers authority, and every connection still runs the current registry and grant checks. v1 material moves to v2 only through an authenticated handover, and there is no downgrade from v2 to v1.

The guarantee is **crash consistency only**. Current-state checks, MACs and atomic writes cannot tell an authentic old snapshot from current state. Restoring host or device state from a snapshot is therefore outside the routing and revocation guarantees unless it goes through a fail-closed restore ceremony. In that ceremony the operator restore command bumps the installation revocation epoch and `g` for every pairing, re-applies revocations made after the snapshot, and leaves each pairing unusable until a fresh authenticated handover. A monotonic freshness anchor is possible later hardening; it is not part of this construction.

### Evidence required before delivery

These items are conditions on the implementation. Without them the decision doesn't hold.

- **Fixture format:** the fixtures carry a format version that is distinct from the protocol versions. They include independent positive and negative signature vectors, full Noise transcript vectors and v2 KDF vectors. Counters `0`, `2^32` and `2^64−1` are covered as encodings only, and wrapping is forbidden.
- **Conformance cases:**
  - each completion dropped, independently and repeatedly;
  - a genuine host proof forwarded and the device proof dropped `W` times;
  - process or power loss at every write and send boundary, including uncertain errors;
  - same-room and different-room races across processes and endpoints;
  - stale commits after a higher counter has been consumed;
  - restoration of host, device or joint snapshots, and missing or corrupt state;
  - revoke, rotate or tighten during proof, commit, completion and response;
  - overflow, and long-offline or expired-binding cases.
- **Channel profile:** a field-for-field mapping of the current binding and prologue to the trust-domain, fingerprint, generation, validity and role requirements in [the candidate channel](#candidate-channel-for-security-review). The v2 host-proof binding changes signed bytes, so it needs review even though the Noise primitives are unchanged.

The decision comment on #1526 records the reviewer, the scope, the merged SHA of this file, the fixture identity it was reviewed against, and any unresolved conditions. The fixture identity is the v1 `wire.json`, blob `9f81e440`, which has no version field. The follow-up implementation issue records the implementation tree, the versioned v2 fixtures and their hashes, and the evidence above. Any divergence from this section goes back to the contract issue; it is not fixed in code.

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
