# Mobile Device Trust Architecture

**Status:** accepted architecture for the `#784`–`#788` mobile connection track  
**Authority owner:** Coven daemon / Rust authority layer  
**Applies to:** Coven CLI/TUI, Cave and other native clients, rendezvous/relay services, and future recovery providers

## Decision summary

A phone or other client is a **delegated device**. It is not the owner identity, a familiar identity, a Coven installation identity, or a session identity.

The canonical trust chain is:

```text
trusted introduction
    → transcript-bound key enrollment
    → device-bound proof of possession
    → scoped and revocable device grant
    → fresh session authentication
    → optional step-up user verification
```

A QR code is an out-of-band introduction. A biometric is local user verification. A relay is an untrusted ciphertext router. None of those mechanisms is the identity root by itself.

This architecture extends the existing `coven-cli::mobile_memory` implementation rather than replacing it. Protocol v1 already provides a strong foundation:

- terminal QR invitation with a single-use random nonce;
- stable P-256 host identity and pinned host-key fingerprint;
- P-256 device public-key enrollment;
- transcript-derived six-word confirmation on both endpoints;
- signed canonical requests with timestamp, nonce, and body digest;
- replay protection, per-device rate limiting, revocation, and audit events;
- private, atomically replaced device-registry storage;
- idempotent pairing completion within the invitation lifetime.

The next protocol generation generalizes those primitives beyond read-only mobile memory while preserving v1 compatibility during migration.

## Identity and credential separation

OpenCoven must keep the following subjects and credentials distinct.

| Subject or credential | Purpose | Must not become |
| --- | --- | --- |
| Owner/root identity | Issues, constrains, rotates, and revokes delegations | A routine online session key |
| Coven installation identity | Identifies one authority-bearing Coven/Psyche node | The familiar identity |
| Familiar identity | Portable, named agent identity across runtimes | A phone or machine credential |
| Device key | Proves possession of an enrolled device | A globally correlatable user identifier |
| Device grant | Encodes authority delegated to a device key | An unconstrained bearer token |
| Session key | Protects one connection or resumption epoch | A durable device identity |
| Passkey/account credential | Optional account login, recovery, or remote-introduction factor | The sole familiar or owner root |
| Biometric result | Local operating-system user-verification signal | Data transmitted to Coven or a relay |
| Attestation evidence | Optional assurance about an app/key environment | Mandatory ownership proof |

A device should use a pairwise identity per Coven trust domain where practical. The protocol must not expose hardware serial numbers, advertising identifiers, Apple/Google account identifiers, raw biometric metadata, or a reusable public identifier across unrelated Covens.

## Authority boundary

All decisions that answer **whether a device may act** belong in the Rust authority layer. UI clients may collect intent, display scopes, invoke platform biometric APIs, and hold non-exportable private keys, but they do not define authorization semantics.

The current authority implementation lives under:

- `crates/coven-cli/src/mobile_memory/pairing.rs`
- `crates/coven-cli/src/mobile_memory/auth.rs`
- `crates/coven-cli/src/mobile_memory/registry.rs`
- `crates/coven-cli/src/mobile_memory/gateway.rs`
- `crates/coven-cli/src/mobile_memory/audit.rs`

Future extraction into a shared crate is allowed when multiple binaries need the same contracts, but the migration must not create two competing authorities.

## Trust graph

```text
Owner identity
    │
    ├── delegates → Coven installation
    │                  │
    │                  └── hosts → sessions and familiar surfaces
    │
    └── authorizes → Device grant
                         │
                         ├── bound to → device public key
                         ├── scoped to → one Coven trust domain
                         ├── constrained by → expiry / policy / assurance
                         └── authorizes → named capabilities and actions
```

Revoking a device invalidates that device's grants and resumptions. It must not rotate, delete, or otherwise rewrite familiar identity or memory.

## Pairing and enrollment state machine

```text
absent
  │ create invitation
  ▼
invited ── expiry/cancel/malformed attempt ──▶ terminal
  │ device presents nonce + public key + protocol range
  ▼
enrolled-pending-confirmation
  │ both sides confirm transcript-derived phrase
  ▼
completed
  │ matching retry before original expiry
  ├──────────────────────────────────────────▶ completed (idempotent)
  │ grant expiry/revocation/key rotation
  ▼
revoked-or-expired
```

### Pairing offer

A pairing offer is versioned, short-lived, single-use, and safe to reveal to the camera path. It may contain:

- protocol version or supported range;
- random pairing nonce/session identifier;
- host public-key fingerprint or ephemeral handshake key;
- rendezvous/direct-connection hints;
- requested-capability digest;
- absolute expiry.

It must not contain a durable bearer token, private key, biometric value, or familiar/root key material.

Protocol v1 encodes this information in a `coven-memory://pair` URL. A v2 offer may use canonical CBOR and a universal/app link, but migration must retain a copyable fallback and strict canonical decoding.

### Transcript binding

The enrollment transcript must cover every security-relevant value, including at minimum:

```text
protocol version/range
host installation fingerprint or handshake key
pairing identifier and nonce
client device public key
requested scopes/restrictions digest
expiry
rendezvous context when material to endpoint selection
```

The same transcript drives the human-verification phrase and the durable grant request. An attacker must not be able to change scopes, substitute a key, downgrade a protocol, or redirect the endpoint without changing the transcript.

### Completion

Both endpoints explicitly confirm the same transcript. The authority writes the device record/grant once, then retains a redacted completed result only for the original invitation window so transport retries are idempotent. A mismatched phrase fails closed and never returns a device record.

## Device authentication

After enrollment, every protected request or session establishment proves possession of the device private key.

Protocol v1 signs a canonical request containing:

```text
method
exact path and query
request timestamp
fresh random nonce
body digest
```

That design remains valid for direct request/response access. Long-lived or relayed transports must perform an authenticated handshake and derive fresh session keys. Resumption material must be sender-constrained or cryptographically bound to the device key and current grant state; it must not be a reusable bearer secret.

The authority re-checks revocation and effective scopes before releasing a response or executing an action so a device revoked during request processing cannot win a time-of-check/time-of-use race.

## Device grants

Protocol v1 records a fixed `memory_read` scope directly on the device record. Protocol v2 generalizes this into an explicit signed or authority-authenticated grant:

```text
DeviceGrant {
  version
  grant_id
  subject_key_id
  issuer_installation_id
  audience_trust_domain
  scopes[]
  restrictions
  assurance_policy
  issued_at
  not_before?
  expires_at?
  revocation_epoch
}
```

The first capability taxonomy must distinguish at least:

- session metadata read;
- conversation read;
- message send;
- memory read;
- familiar-memory administration;
- tool invocation request;
- tool execution approval;
- secrets access;
- device enrollment and revocation;
- identity or memory export.

Unknown scopes fail closed. Scope ordering and encoding are canonical. Grants are audience-bound, cannot widen themselves, and are re-evaluated on session resumption.

## Biometrics and step-up authorization

OpenCoven never receives a face image, fingerprint template, biometric hash, or platform biometric identifier.

The secure flow is:

```text
Face ID / Touch ID / strong biometric
    → operating system verifies local user presence
    → secure hardware permits use of the device private key
    → device signs a fresh challenge or exact action digest
    → Coven verifies signature, freshness, grant, and policy
```

The remote proof is cryptographic. The biometric is a local gate on key use.

Policy should support graduated assurance:

1. **Possession** — valid device key and grant; suitable for low-risk reconnect.
2. **Recent user verification** — platform reports a recent approved unlock/authentication.
3. **Fresh user verification** — required immediately before a sensitive operation.
4. **Fresh biometric-only verification** — used only where the platform can enforce and attest that local policy; not silently conflated with passcode fallback.
5. **Step-up/recovery** — another trusted device, recovery credential, owner key, or threshold approval.

For transaction authorization, the signature covers a canonical action object—not a generic `approve=true` value. The client must render the material fields that are included in the digest, such as target, repository, commit, side effects, nonce, and expiry.

## Transport model

Transport discovery and authentication are separate concerns.

### Direct connection

The existing mobile gateway uses TLS 1.3 with a stable self-signed host identity pinned through the pairing transcript. This remains a valid direct path.

### Relay/rendezvous

A relay may:

- match opaque rendezvous identifiers;
- enforce bounded connection/frame/resource limits;
- forward opaque encrypted frames;
- provide liveness and routing metadata that does not reveal owner/familiar identity.

A relay must not:

- receive application plaintext or durable private keys;
- mint device grants;
- impersonate either endpoint;
- treat room knowledge as authorization;
- silently downgrade end-to-end encryption;
- retain traffic by default.

Both endpoints should make outbound connections so the path works across NAT, SSH hosts, and restrictive networks. End-to-end endpoint authentication and session encryption occur above the relay transport.

### Local discovery

mDNS/DNS-SD or BLE may advertise an opaque, rotating candidate identifier. Discovery is never authentication. Stable owner, familiar, or device names must not be broadcast by default. An authenticated direct connection is attempted when available, with relay fallback that preserves the same endpoint identity and grant semantics.

## Passkeys, trusted-device introduction, and recovery

Passkeys are appropriate for optional account access, recovery authentication, browser/native login, and approving a new installation from an already trusted device. A synced passkey is not proof of one physical phone and must not become the sole familiar/root identity.

A trusted-device introduction signs a transcript covering the new endpoint public key, requested scopes, nonce, expiry, and human-readable device context. Root policy may require one device, another recovery factor, or N-of-M approval.

Recovery is explicit and auditable. Recovering account access is distinct from replacing or rotating familiar identity. A compromised account or relay service alone cannot mint device authority.

## Optional attestation

App or hardware attestation is an assurance attribute, for example:

```text
unattested-device
verified-app-instance
verified-hardware-backed-key
```

It is optional and policy-controlled. It must not break self-built clients, development builds, self-hosted deployments, or user-controlled runtimes. Attestation never replaces proof of possession, owner delegation, or local user verification.

## Threat model and required properties

The design assumes networks, rendezvous infrastructure, push providers, and local discovery broadcasts can be observed or manipulated. A paired endpoint or owner device may later be lost or compromised.

Required properties:

1. Relay compromise cannot reveal application plaintext or forge an authenticated endpoint session.
2. Capturing a QR or pairing link cannot authorize a device after success, cancellation, or expiry.
3. Copying a grant or registry record without the bound private key is insufficient to authenticate.
4. Enrollment resists key substitution, scope substitution, protocol downgrade, replay, and wrong-endpoint pairing.
5. Request and transaction signatures are nonce-bound, expiring, and canonically encoded.
6. Revocation invalidates reconnect and resumption material without changing familiar identity.
7. One Coven cannot correlate the same device across unrelated Covens through a global protocol identifier.
8. Platform biometrics never leave the platform biometric subsystem.
9. Unknown protocol fields, scopes, assurance claims, or registry versions fail closed unless explicitly declared forward-compatible.
10. Resource exhaustion is bounded per connection, rendezvous, and enrolled device.

## Migration from mobile protocol v1

The migration is additive and staged:

### Stage A — lock the contract

- Treat the current v1 P-256 request-signing path as supported behavior.
- Publish canonical schemas/test vectors for pairing, request signing, and grants.
- Add a protocol-version dispatch boundary before adding v2 semantics.

### Stage B — generalize device records into grants

- Preserve existing v1 `memory_read` devices through a deterministic registry migration.
- Represent v1 devices as grants with exactly the existing authority.
- Add explicit expiry, audience, restrictions, and revocation epoch without widening access.

### Stage C — add relayed sessions

- Implement a bounded opaque rendezvous relay.
- Add an endpoint-authenticated, forward-secret session protocol above it.
- Keep direct TLS as a local/self-hosted path and select transport independently from identity.

### Stage D — add platform step-up and recovery

- Define mobile-client adapters for non-exportable keys and fresh local user verification.
- Add trusted-device introductions, optional passkey recovery, and optional attestation attributes.
- Keep proprietary services optional.

## Issue-to-delivery mapping

- `#784` — this architecture, threat model, authority boundary, and migration contract.
- `#785` — versioned pairing offer, transcript hardening, portable test vectors, and E2EE rendezvous handshake.
- `#786` — generalized grants, assurance policy, exact-action authorization, device management, and registry migration.
- `#787` — relay-first reconnect, local discovery fast path, session resumption, and push-as-wakeup semantics.
- `#788` — trusted-device introduction, passkey/recovery contracts, threshold policy, and optional attestation.

## Merge gates for implementation PRs

Every implementation PR in this track must include focused adversarial tests and pass the repository gates:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

No PR may weaken existing v1 privacy, replay, revocation, canonicalization, or audit guarantees in order to add a smoother connection path.
