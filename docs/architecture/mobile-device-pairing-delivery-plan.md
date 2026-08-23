# Mobile Device Pairing Delivery Plan

Tracks #784–#788.

## Delivery train

### PR 1 — Protocol foundation

- canonical identity/trust architecture
- threat model
- diagnostic schemas
- conformance-vector structure
- capability and assurance vocabularies

Exit criteria: architectural review complete; schemas validate; security invariants accepted.

### PR 2 — Shared protocol library

- deterministic CBOR codec
- domain-separated transcript hashing
- pairing-offer validation
- capability and restriction parser
- device-grant issue/verify API
- transaction-authorization issue/verify API
- deterministic cryptographic test vectors
- fuzz/property/adversarial tests

Exit criteria: language-level tests and cross-implementation vectors pass.

### PR 3 — TUI enrollment and device administration

- `coven device pair`
- scope selection and permission preview
- terminal QR plus copyable fallback
- expiry/cancel/status state machine
- short authentication string confirmation
- `device list|inspect|rename|revoke`
- structured audit events

Exit criteria: successful and negative end-to-end tests against an in-process mobile peer.

### PR 4 — Rendezvous and transport

- outbound relay transport
- opaque single-use rendezvous sessions
- frame bounds, rate limits, and cleanup
- malicious-relay integration test shim
- direct-transport abstraction

Exit criteria: relay cannot read plaintext or impersonate endpoints; NAT-separated integration test passes.

### PR 5 — Pocket credentials and biometric authorization

- device key generation and protected storage
- platform assurance mapping
- enrollment request signing
- returning-device challenge response
- exact transaction authorization UI and signing
- lost/revoked device UX

Exit criteria: platform tests prove no biometric data enters application/protocol state; copied grant cannot authenticate.

### PR 6 — Seamless reconnection and local discovery

- rotating opaque discovery IDs
- authenticated direct LAN path
- relay fallback
- sender-constrained resumption
- network migration
- push as wake-up/routing only

Exit criteria: transport changes preserve endpoint identity; revocation defeats stale resumption.

### PR 7 — Recovery and trusted-device introduction

- passkey-backed optional recovery/account factor
- trusted-device enrollment approval
- policy-selectable N-of-M step-up
- explicit owner/familiar rotation semantics
- optional attestation attributes

Exit criteria: compromised account/relay alone cannot enroll a device; self-hosted and unattested clients remain supported by protocol.

## Merge policy

Every implementation PR must:

1. link the governing issue(s) and this delivery plan;
2. preserve protocol versioning and fail-closed behavior;
3. include positive, negative, replay, downgrade, and malformed-input tests appropriate to its layer;
4. include migration or compatibility notes for stored state and clients;
5. pass repository CI and receive security-focused review;
6. avoid closing a plan issue until its acceptance criteria are demonstrated, not merely scaffolded.

## Rollout

The feature should remain behind an explicit experimental capability until the shared conformance vectors, TUI-to-Pocket integration path, revocation path, and recovery path are all exercised. Telemetry must be opt-in or privacy-preserving and must never include keys, offer payloads, biometric state, decrypted traffic, or sensitive familiar/session contents.
