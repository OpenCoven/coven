# Device-Bound Credentials, Biometric Authorization, and Scoped Grants — Plan

Status: Plan and implementation contract for issue #786

Tracks: #786. Parent architecture: #784 (`mobile-device-trust.md`, `mobile-device-pairing-v1.md`). Depends on pairing bootstrap (#785). Sits at step 5 ("Pocket credentials and biometric authorization") of the delivery train in `mobile-device-pairing-delivery-plan.md`.

Companion documents:

- [`mobile-device-pairing-v1.md`](mobile-device-pairing-v1.md) — the canonical protocol contract this plan implements
- [`../design/mobile-assurance-step-up-v1.md`](../design/mobile-assurance-step-up-v1.md) — the `COVEN-ASSURANCE/1` step-up proof contract (#815, landed by #871); §§6–7 of this plan defer to it
- [`../design/mobile-device-trust.md`](../design/mobile-device-trust.md) — accepted architecture and authority boundary
- [`../security/mobile-device-pairing-threat-model.md`](../security/mobile-device-pairing-threat-model.md) — adversaries and required controls
- [`../../spec/device-pairing/v1/`](../../spec/device-pairing/v1/README.md) — diagnostic schemas, capability vocabulary, conformance gates

Normative language: MUST / MUST NOT / SHOULD / MAY per RFC 2119, as used in `mobile-device-pairing-v1.md`.

## 1. Purpose and scope

Issue #786 turns the paired phone into a **delegated principal**: a device whose private key is generated and protected by the platform, whose authority is expressed by a revocable, scoped `DeviceGrant`, and whose local biometrics are a gate on key use — never a network credential.

This plan specifies:

1. the platform device-credential policy (iOS Secure Enclave/Keychain, Android Keystore/BiometricPrompt) and how PIN/passcode fallback is kept distinct from biometric-only assurance;
2. the device-grant object model as it maps onto the existing protocol schemas and the current Rust authority, including the capability/assurance vocabulary alignment the implementation needs;
3. assurance-level policy (possession → recent user verification → fresh biometric → step-up), where each level is enforced, and how anything above possession is proven — exclusively through the `COVEN-ASSURANCE/1` step-up proof contract (`../design/mobile-assurance-step-up-v1.md`, #815/#871);
4. exact transaction authorization with a worked, reproducible golden example;
5. the device-management command surface, lost-device workflow, key rotation, and audit-event taxonomy;
6. the state machines and registry migration that generalize today's `memory_read`-only pairing into scoped grants.

Out of scope: the rendezvous relay and reconnection protocol (#787), passkey recovery and trusted-device introduction (#788), and the Pocket client implementation itself. This document specifies the contracts those deliverables must satisfy.

## 2. Current state and gap analysis

### 2.1 What exists today

The mobile authority already lives in `crates/coven-cli/src/mobile_memory/` (the module listed as the current authority implementation in `docs/design/mobile-device-trust.md`, "Authority boundary"):

| Concern | Code | Notes |
| --- | --- | --- |
| Grant object | `crates/coven-cli/src/mobile_memory/grant.rs` — `DeviceGrant` | `version`, `id` (UUIDv5), `subject_key_id`, `audience`, `scopes`, `restrictions`, `minimum_assurance`, `issued_at`, `not_before`, `expires_at`, `revocation_epoch` |
| Capability vocabulary | `grant.rs` — `DeviceScope` | 12 snake_case scopes, serialized canonically sorted/unique (`validate_scope_set`) |
| Assurance ranks | `grant.rs` — `AssuranceLevel` | 5 ordered levels: `possession` < `recent_user_verification` < `fresh_user_verification` < `fresh_biometric` < `step_up` |
| Grant authorization | `grant.rs` — `DeviceGrant::authorize()` | scope membership + assurance-rank comparison + validity window; per-scope fresh-verification floor via `restrictions.require_fresh_user_verification_for` |
| Request authentication | `auth.rs` — `MobileAuthenticator::verify` | canonical request `COVEN-MEMORY/1\n{method}\n{path}\n{timestamp}\n{nonce}\n{body_digest}`; P-256 ECDSA (DER); ±300 s window (`MOBILE_REQUEST_WINDOW_SECONDS`, `mod.rs`); 10,000-entry replay cache; 120 req/60 s per-device rate limit |
| Session-time recheck | `auth.rs` — `ensure_still_active` | TOCTOU defense: grant id + `revocation_epoch` must still match at execution time |
| Device registry | `registry.rs` — `DeviceRecord`, `GrantedDeviceRecord { device, grant }` | atomic private storage; legacy `LegacyDeviceRecord`/`LegacyDeviceScope` migration; `register_with_grant`, `replace_grant`, `rename`, `revoke`, `forget_all` |
| Pairing/enrollment | `pairing.rs` | protocol v2 (min 1), domain tags `COVEN-PAIR/2` / `COVEN-PAIR-OFFER/2`, transcript digest, six-word confirmation phrase, single-use invitation, `enroll()` fixes `scopes: vec![DeviceScope::MemoryRead]` |
| Gateway | `gateway.rs` | private-network TLS 1.3 gateway; 5-minute pairing lifetime (`PAIRING_LIFETIME`); bounded inflight connections |
| Audit | `audit.rs` — `MobileAuditEvent` | 8 events; 0600 `O_NOFOLLOW` JSONL; 4 MiB truncation; coarse-fields redaction test |
| CLI | `main.rs` — `MobileMemoryCommand` | `coven memory mobile enable|disable|status|pair|devices [--json] [revoke <id>]` |
| Wire schemas | `spec/device-pairing/v1/` | `device-grant.schema.json`, `transaction-authorization.schema.json`, `enrollment-request.schema.json`, `revocation-record.schema.json`, `capabilities.json` |

Two structural facts matter for everything below:

1. **Possession is already proof-of-possession-bound.** A copied grant or registry record without the device private key cannot authenticate: `auth.rs` verifies an ECDSA signature over `COVEN-MEMORY/1`-canonicalized request bytes against the enrolled device public key, and `grant.rs::DeviceGrant::validate` rejects a grant whose `subject_key_id` does not match the presented key (`GrantError::SubjectMismatch`).
2. **Biometrics are absent from the protocol by design.** v1 authenticates every request at `AssuranceLevel::Possession` (`auth.rs` passes `AssuranceLevel::Possession` to `DeviceGrant::authorize`). Nothing biometric exists to transmit — which is the correct baseline; this plan adds the *policy* layer that demands stronger assurance before sensitive operations.

### 2.2 Gap against issue #786

| Issue requirement | Today | Gap |
| --- | --- | --- |
| Non-exportable, hardware-protected device key | Device key is an opaque public key to Coven; storage policy is entirely client-side and unspecified | No documented key-generation/storage policy for iOS/Android (§4) |
| Platform assurance mapping | `AssuranceLevel` enum and `require_fresh_user_verification_for` restriction exist, but nothing produces evidence above `possession` | No assurance presentation, no platform policy mapping (§6) |
| Biometric vs passcode fallback distinction | Architecture docs require it (`mobile-device-trust.md` level 4); `capabilities.json` and `device-grant.schema.json` know only 4 levels | Vocabulary mismatch with the 5-level Rust enum (§5.3) |
| Generalized scoped grants | `grant.rs` implements the full object, but enrollment hardcodes `memory_read` and `auth.rs` maps every protected route to `DeviceScope::MemoryRead` | No issuance path for the other 11 scopes; scope taxonomy mapping unspecified (§5.2) |
| Exact transaction authorization | `DeviceActionIntent` exists with domain-separated canonicalization and tests | No endpoint consumes it; no presentation-binding contract for the mobile UI (§7) |
| `coven device list/inspect/rename/revoke` | `coven memory mobile devices [--json]`, `revoke <id>` only (`main.rs`); `registry.rs` has `rename()` but no CLI; no `inspect` | Missing commands and JSON contract (§8) |
| Scope editing/reissuance, key rotation | `registry.rs::replace_grant` exists (used at re-pair); no scope-editing or rotation flow | No succession semantics (§5.5, §5.6) |
| Lost-device workflow | `revoke()` + `forget_all()` exist | No documented workflow or epoch semantics (§8.3, §9.2) |
| Audit events | 8 coarse events | No step-up, grant-lifecycle, or key-rotation events (§10) |

## 3. Target experience (golden example)

The operator pairs their phone (issue #785), then this happens:

```text
$ coven memory mobile devices --json
[ { "id": "00000000-0000-4000-8000-00000000000a", "state": "active",
    "displayName": "Tim's phone", "pairedAt": "...", "scopes": ["memory_read"] } ]

$ coven device inspect 00000000-0000-4000-8000-00000000000a
id:            00000000-0000-4000-8000-00000000000a
name:          Tim's phone
state:         active
key:           P-256 / subjectKeyId 5f176879aa1dfa922aa8fcb3fd213537fce4047fe84455eb70a9664de65ca548 (hex)
grant:         VhQT9TpXWFOzhimwT_5Z3g
scopes:        sessions.metadata.read, conversations.read, messages.send,
               tools.request, tools.approve
assurance:     possession (min); fresh_user_verification required for tools.approve
expires:       2026-08-31T15:00:00Z (never auto-renews)
revocationEpoch: 0

# A deploy arrives from a familiar. The phone must approve it.
#   1. daemon posts the action transaction (every displayed field; §7.2) and a
#      COVEN-ASSURANCE/1 challenge
#   2. phone renders EVERY digest-covered field, then Face ID
#   3. the authorization key signs the COVEN-ASSURANCE/1 action proof; nothing
#      biometric leaves the phone
#   4. daemon verifies possession request, challenge, and proof, and computes the
#      effective assurance itself; the action executes once
#   5. audit: step-up verified, grant id, device id — never the biometric, never the payload
```

Reproducible golden vectors for that flow are in §7.3.

## 4. Device credential

### 4.1 Key generation and storage policy

The device credential is a non-exportable signing key created at enrollment time. The remote world only ever sees the public key (`EnrollmentRequest.deviceKey`, `spec/device-pairing/v1/enrollment-request.schema.json`) and signatures.

Requirements (extending Gate E of `spec/device-pairing/v1/implementation-gates.md`):

- C-1. The key MUST be generated inside the platform key facility on the device — never imported, never derivable from a seed the app stores.
- C-2. Key-use gating is role-scoped (§4.7): the possession key MUST remain usable without a user-verification prompt on every request, and any key enrolled as an authorization key MUST be bound to the platform access-control policy that enforces its declared assurance class (see §4.2/§4.3). The mobile client MUST NOT enroll as an authorization key a key whose use is ungated when the platform offers gating.
- C-3. The private key MUST be non-extractable when the platform supports it. If a platform cannot provide hardware protection (older Android, developer devices), the client MUST fall back to software keys and the grant MUST record `unattested_device` attestation (`spec/device-pairing/v1/capabilities.json`), and policy MAY restrict what an unattested device may hold.
- C-4. The key MUST be bound to the enrollment transcript from #785: enrollment signs the transcript hash (already required by the enrollment-request schema); if the key is destroyed the device re-enrolls as a new device — a key never silently re-binds to a new transcript.
- C-5. One keypair per Coven relationship: the device generates a fresh keypair per pairing ceremony. No identifier derived from the key or hardware may be reused across unrelated trust domains (threat: cross-Coven correlation).
- C-6. Losing the platform's unlock factors (biometrics re-enrolled, passcode change) MUST be detected via key invalidation (`biometryCurrentSet` on iOS, `setInvalidatedByBiometricEnrollment` on Android) and treated as a re-enrollment trigger, not silently retried.
- C-7. The possession key and the step-up authorization key are distinct keys (`COVEN-ASSURANCE/1`, "Key separation"): one key MUST NOT serve both roles, the possession key stays prompt-free so ordinary requests remain frictionless, and all biometric/passcode gating attaches to the separate authorization key (§4.7, §6).

### 4.2 iOS evaluation (Secure Enclave + Keychain access control)

Recommended client contract for the Pocket/iOS client. The possession key is created with `kSecAccessControlPrivateKeyUsage` alone — no user-verification policy — so ordinary `COVEN-MEMORY/1` requests stay prompt-free (§4.7); the access-control choices below configure the separate **authorization key** (`COVEN-ASSURANCE/1`):

- Generate the key with `kSecAttrTokenIDSecureEnclave`, algorithm P-256 (ECDSA/ES256) — the only algorithm Secure Enclave keys support. This matches the existing P-256 device-key profile (`auth.rs` verifies P-256 sec1 points; `docs/architecture/mobile-device-pairing-v1.md` allows "platform-native hardware keys MAY use P-256 with an explicitly encoded algorithm identifier").
- Create the key with `SecAccessControlCreateFlags` combining `kSecAccessControlPrivateKeyUsage` and one of:
  - `kSecAccessControlBiometryCurrentSet` — biometric-gated; invalidated when enrolled biometrics change;
  - `kSecAccessControlDevicePasscode` (or `.userPresence`) — the documented device-credential fallback path, kept as a **separate** key or a separately-audited policy branch, never silently merged with biometric-only assurance (§4.4).
- For `fresh_biometric`, sign inside `LAContext`-gated key use (`evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics)` immediately before the signing operation). The reuse window (`touchIDAuthenticationAllowableReuseDuration`) is bounded by the platform and maps to `recent_user_verification` at best, never `fresh_biometric`.
- The host observes only: a signature over the exact challenge/action digest, or an error. A biometric denial surfaces as "user verification failed", never as any biometric datum.

### 4.3 Android evaluation (Keystore + BiometricPrompt)

- Generate the key in Android Keystore with `KeyGenParameterSpec` and hardware-backed storage; the possession key carries no `setUserAuthentication*` policy (§4.7), and the authorization key attaches the authentication policy that matches its declared assurance class:
  - `setUserAuthenticationParameters(0, AUTH_BIOMETRIC_STRONG)` — per-operation auth with the `KeyProperties` strong-biometric-only constant → the credential qualifies for `fresh_biometric`;
  - `setUserAuthenticationParameters(timeout, AUTH_BIOMETRIC_STRONG | AUTH_DEVICE_CREDENTIAL)` — time-window auth → maps to `recent_user_verification` while the window is open; the credential-fallback path is recorded as its own method (§4.4);
  - `setInvalidatedByBiometricEnrollment(true)` so new biometrics invalidate the key rather than silently widening access;
  - `setUnlockedDeviceRequired(true)` where policy wants the device unlocked;
  - StrongBox (`setIsStrongBoxBacked(true)`) when available, as optional attestation evidence, never an identity root.
- Perform the sensitive operation through `BiometricPrompt` with a `CryptoObject(Signature)` wrapping the exact canonical action digest (§7). The OS releases the key only on successful biometric authentication, and the signature covers the digest shown in the UI — middleware cannot separate what was displayed from what was signed.
- `BIOMETRIC_WEAK` and `DEVICE_CREDENTIAL` results are recorded as `device_credential`-class verification, never as `fresh_biometric`.

### 4.4 PIN/passcode fallback is a distinct assurance

Platform PIN/passcode fallback is accepted when policy allows it, but it MUST be recorded and evaluated as its own class:

- The assurance enum distinguishes `fresh_biometric` (biometric-only, strong class) from `fresh_user_verification` (any approved local method). `crates/coven-cli/src/mobile_memory/grant.rs` already defines both; the diagnostic vocabulary must follow (§5.3).
- A client that can only offer `.deviceOwnerAuthentication` (iOS biometric-or-passcode) or `DEVICE_CREDENTIAL` fallback (Android) MUST report `fresh_user_verification`, and MUST NOT claim `fresh_biometric`. A grant that demands `fresh_biometric` from a platform that cannot enforce biometric-only MUST fail closed for those operations rather than downgrade silently.

### 4.5 Pairwise / pseudonymous device identity

- The device uses one keypair per Coven trust domain (per `mobile-device-pairing-v1.md`, "Device identity"). The enrollment request's `pairwiseDeviceId` is generated per trust domain; hardware serials, advertising IDs, and Apple/Google account identifiers are forbidden inputs.
- `docs/design/mobile-device-trust.md` requires this; current code is compatible: `DeviceRecord.id` is a host-assigned UUID and the protocol transmits only the public key and a display name (`pairing.rs` `validate_pairing_request`).
- Recommendation: keep per-relationship keys "where practical" as the issue words it — i.e. the client SHOULD regenerate a key when pairing to a second Coven, but a user-chosen reuse of a key across their own Covens is not a protocol violation; correlatability protection is a property the *verifier* must not depend on (unknown-key pairs still fail closed).

### 4.6 Binding the device key to the #785 enrollment transcript

- The enrollment request signs `{transcriptHash, deviceKey, pairwiseDeviceId, capabilities, nonce, expiresAt}` (`spec/device-pairing/v1/enrollment-request.schema.json`).
- Current implementation: `pairing.rs` builds `PairingTranscript::for_request(...)` over the v2 offer (`COVEN-PAIR/2`, `COVEN-PAIR-OFFER/2` domains), derives the phrase and `transcript_hash`, and records the device public key in `PendingDevice`. The transcript fixture (`crates/coven-cli/tests/fixtures/mobile-pairing-v2/transcript-vector.json`) locks the digest derivation.
- Gap to close in #786's implementation slice: the *grant issuance* step must refuse to issue a grant whose subject key was not the key that signed the accepted enrollment request for the stored transcript hash. Today `enroll()` stores whatever public key the request carried; the issuance rule (§5.4) makes the binding explicit and testable (Gate C: "capability digest is bound from QR creation through grant issuance").

### 4.7 Key separation and the `COVEN-ASSURANCE/1` authorization key

Every paired device holds two distinct, non-exportable P-256 keys (`COVEN-ASSURANCE/1`, "Key separation"):

- the **possession key**, enrolled via the #785 transcript, which authenticates every request under `COVEN-MEMORY/1` and stays prompt-free; and
- the **step-up authorization key**, a separate key protected by the platform policy of its declared assurance class, which signs `COVEN-ASSURANCE/1` proofs only (§6).

Rules:

- One key MUST NOT serve both roles; the registry MUST reject an authorization key equal to the possession key (`COVEN-ASSURANCE/1`, "Key separation" rule 1).
- The authorization key is optional and additive: devices without one keep exactly today's possession-only behavior, and grants whose policy demands more than possession fail closed for them (`AssuranceRequired`).
- Enrollment binds the authorization public key and its declared class (`biometric_only` | `user_verification` | `device_credential`) into the pairing-v2 transcript, so the class is an enrollment fact confirmed by the human phrase comparison — never a runtime client assertion. The class caps what proofs from that key can ever prove: `biometric_only` → `fresh_biometric`; `user_verification` and `device_credential` → `fresh_user_verification`.
- Authorization-key metadata lives in its own store (`authorization-keys.json`), not in the device registry, so the two lifecycles stay independent (`COVEN-ASSURANCE/1`, "Storage").
- Only signatures cross the trust boundary at any point; biometric material never does (§6).

## 5. Device grant

### 5.1 Object model: spec schema ↔ Rust implementation

The canonical object is defined in `docs/architecture/mobile-device-pairing-v1.md` ("Device grant") and validated diagnostically by `spec/device-pairing/v1/device-grant.schema.json` (CBOR/COSE canonical; JSON diagnostic only). The Rust authority implements it in `grant.rs`:

| Spec field (`device-grant.schema.json`) | Rust (`DeviceGrant`, `grant.rs`) | Note |
| --- | --- | --- |
| `version` (1) | `DeviceGrant.version` (`DEVICE_GRANT_VERSION = 1`) | equal |
| `grantId` (22-char b64url of 16 bytes) | `DeviceGrant.id: Uuid` (`Uuid::new_v5(&device_id, b"coven-device-grant-v1")`) | diagnostic encoding differs; same 128-bit id. v1's derivation is deterministic per device — the #786 contract requires a distinct id per issued grant (§5.5) |
| `subject` keyReference | `subject_key_id: String` = SHA-256 over raw key bytes | spec carries full public key; Rust stores only the digest and binds it via `validate(public_key)` |
| `issuer` identityReference | (not modeled) | Rust v1 has no issuer field; the issuing installation is implicit in the private registry. Portable, externally verifiable grants arrive with the CBOR/COSE canonical form (`mobile-device-pairing-v1.md`) in the shared-protocol-library slice |
| `audience` identityReference | `DeviceGrantAudience::LocalCovenAuthority` (single variant) | spec allows owner/device/trust-domain forms |
| `capabilities` (dot-form enum) | `DeviceScope` (snake-form) | mapping in §5.2 |
| `restrictions` | `DeviceGrantRestrictions { transport, require_fresh_user_verification_for }` | Rust is stricter today (transport + assurance); spec has familiars/projects/sessions/maxIdleSeconds/attestation |
| `issuedAt` / `notBefore` / `expiresAt` | `issued_at`, `not_before`, `expires_at: Option` | same semantics (`validate` time-window rules) |
| `revocationEpoch` | `revocation_epoch: u64` | enforced by `auth.rs::ensure_still_active` |
| `confirmationKey` | (not modeled) | spec keeps subject/confirmation separate for future delegation; v1 MAY be the same key |
| `signature` | (implicit — the grant is stored inside the private registry, not a portable token; registry integrity is enforced by file permissions and atomic replace) | signed portable grants arrive with the CBOR/COSE canonical form |

Diagnostic JSON form of the same grant as the current Rust registry serializes it (`serde(rename_all = "camelCase")`, `deny_unknown_fields`). Digests and key ids are shown in hex here for readability; the registry encodes `subjectKeyId` as canonical base64url (`URL_SAFE_NO_PAD`, `grant.rs::subject_key_id`):

```json
{
  "version": 1,
  "id": "561413f5-3a57-5853-b386-29b04ffe59de",
  "subjectKeyId": "5f176879aa1dfa922aa8fcb3fd213537fce4047fe84455eb70a9664de65ca548",
  "audience": "local_coven_authority",
  "scopes": ["conversation_read", "session_metadata_read", "tool_execution_approve", "tool_invocation_request"],
  "restrictions": {
    "transport": "any_authenticated",
    "requireFreshUserVerificationFor": ["tool_execution_approve"]
  },
  "minimumAssurance": "possession",
  "issuedAt": "2026-08-30T15:00:00.000Z",
  "notBefore": "2026-08-30T15:00:00.000Z",
  "expiresAt": "2026-08-31T15:00:00.000Z",
  "revocationEpoch": 0
}
```

### 5.2 Capability taxonomy alignment

Two vocabularies exist today and must be reconciled before clients rely on grants:

- `spec/device-pairing/v1/capabilities.json` and `device-grant.schema.json` use **dot-form** ids (`messages.send`, `tools.approve`, …, 13 values including `identity.export` and `memory.export`).
- `grant.rs` `DeviceScope` uses **snake-form** strings (`message_send`, `tool_execution_approve`, …) — 12 values in total: `memory_read`, which the spec taxonomy does not have, and `familiar_memory_admin`/`device_admin` where the spec splits `memory.familiar.write` and `devices.enroll`/`devices.revoke`.

Recommended canonical mapping (dot-form is the wire vocabulary; snake-form remains the internal serde encoding):

| Spec (dot) | Rust `DeviceScope` | Issue taxonomy |
| --- | --- | --- |
| `sessions.metadata.read` | `session_metadata_read` | session metadata/read |
| `conversations.read` | `conversation_read` | conversation read |
| `messages.send` | `message_send` | message send |
| `tools.request` | `tool_invocation_request` | tool invocation request |
| `tools.approve` | `tool_execution_approve` | tool execution approval |
| `secrets.read` | `secrets_read` | secrets access |
| `memory.familiar.read` | `familiar_memory_admin` (read side) | familiar-memory access |
| `memory.familiar.write` | `familiar_memory_admin` (write side) | — |
| `identity.admin` | `identity_admin` | identity administration |
| `devices.enroll` + `devices.revoke` | `device_admin` | device enrollment/revocation |
| `identity.export` | `identity_export` | identity export |
| `memory.export` | `memory_export` | identity/memory export |
| (not in spec) | `memory_read` | mobile-memory read (v1 legacy scope) |

Gaps the implementation must resolve, with recommendation:

- **Split `familiar_memory_admin` into `memory.familiar.read` / `memory.familiar.write`.** The Rust enum collapses read/write into one scope, while the spec (and least privilege generally) separates them. The contract module (`contract.rs` currently mirrors only `memory_read`; `capabilities.json` already models both read and write surfaces) can honor the split without a registry format change if the enum gains a variant and `LegacyDeviceScope` migration maps old grants to the pair.
- **Split `device_admin` into `devices.enroll` / `devices.revoke`** to match the spec enum and the issue's "device enrollment/revocation" distinction; a device holding both is the exception, not the default.
- **Add `memory_read` to the spec vocabulary** (as `memory.read`) or deprecate it in favor of `conversations.read` during the Stage B registry migration (§11).
- Unknown scopes already fail closed (`validate_scope_set`; `additionalProperties: false` in the schema; "Unknown capabilities MUST be rejected" in `mobile-device-pairing-v1.md`) — preserve that on every rename.

### 5.3 Assurance vocabulary alignment

Three vocabularies exist and disagree:

| Source | Levels |
| --- | --- |
| `grant.rs` `AssuranceLevel` | `possession`, `recent_user_verification`, `fresh_user_verification`, `fresh_biometric`, `step_up` |
| `spec/device-pairing/v1/capabilities.json` `assuranceLevels` and `device-grant.schema.json` `requiredAssurance` | `possession`, `recent_user_verification`, `fresh_biometric`, `step_up` |
| `docs/design/mobile-device-trust.md` | 5 levels, including both `fresh user verification` and `fresh biometric-only` |

Recommendation: extend the spec vocabulary to five levels (add `fresh_user_verification`) to match the implementation and the accepted architecture document. `capabilities.json` ranks are a total order; `authorize()` already relies on rank ordering (`presented_assurance < required_assurance`), so the extension is monotonic and backwards compatible. Alternative considered: collapse `fresh_user_verification` and `fresh_biometric` — rejected because the issue explicitly requires distinguishing biometric-only from broader device-credential fallback, and the Rust enum would lose the distinction it already encodes. The `COVEN-ASSURANCE/1` enrollment classes are the device-side counterparts of these levels and cap the claimable proof levels: `biometric_only` → `fresh_biometric`; `user_verification`, `device_credential` → `fresh_user_verification` (§4.7, §6.3).

### 5.4 Grant issuance and verification lifecycle

Issuance (host authority):

1. Enrollment completes with the #785 transcript bound (offer digest, host fingerprint, device public key, requested capability digest, nonce, expiry — `pairing.rs` transcript, `enrollment-request.schema.json`).
2. The authority derives the grant from **what the operator approved in the UI**, never from what the device requested: scopes are the intersection of displayed/approved capabilities and policy floors (per-capability minimum assurance derived from the `risk` classes in `spec/device-pairing/v1/capabilities.json` — `critical` capabilities default to `fresh_biometric`/`step_up`).
3. The grant is stored atomically with the device record (`registry.rs` `register_with_grant`). The **device id** is the stable audit anchor across a device's lifetime; each issued grant carries its own unique `grant_id` (v1 code derives it deterministically via `uuidv5`; the #786 contract requires per-issuance uniqueness — see §5.5, which also requires an epoch advance on every replacement).

Verification (every protected interaction, already implemented and retained):

1. `auth.rs::verify` — registry reload, revocation check, ±300 s timestamp window, body-digest match, canonical-request ECDSA verify, grant authorize at the required scope, replay-nonce insertion, per-device rate limit;
2. `auth.rs::ensure_still_active` — before acting, grant id + `revocation_epoch` are re-checked against the registry so a device revoked mid-request cannot win the race (the TOCTOU re-check `docs/design/mobile-device-trust.md` requires).

### 5.5 Scope editing and reissuance

- Editing scope = issuing a **new grant** (`replace_grant` already exists in `registry.rs`); a grant never widens itself. Every authority-changing replacement (scope edit, restriction change, rotation, re-issuance) MUST mint a **fresh `grant_id`** — never a deterministic re-derivation of the old id — and MUST **monotonically advance the device's authorization (revocation) epoch by at least one**. Note the current code is weaker on both counts: `for_device` derives the id deterministically from the device id, and `replace_grant` only rejects epoch *decreases*, silently accepting equal epochs — the #786 implementation slice must enforce strict advance. After replacement, `ensure_still_active` rejects the old grant id and epoch at once, narrowed authority takes effect at the next request without device cooperation, and outstanding `COVEN-ASSURANCE/1` challenges and transaction replay state (both bound to `grant_id` + `revocation_epoch`) are invalidated.
- Narrowing MUST be monotone at reissuance: the new scope set is a subset of the union of the current grant and the operator's explicit selection, validated by `validate_scope_set` (sorted, unique, non-empty).
- Reissuance is an audited event (§10) that records the old and new grant ids and the old and new authorization epoch (digests only, per the audit redaction rules).
- Recommended API: `coven device rescope <device> --grant/--scope ...` writing a replacement grant through `replace_grant`; display the before/after scope diff before commit (same preview discipline as pairing scope selection).

### 5.6 Key rotation and re-enrollment

- Rotation = new enrollment ceremony (#785) for a **new** key, followed by grant succession: issue the new grant (same or narrower scopes) — a fresh grant id and an advanced authorization epoch per §5.5 — then revoke the old key with `reason: key_rotation` (`revocation-record.schema.json`). Both events are audited; the succession is explicit, never implicit. Outstanding `COVEN-ASSURANCE/1` challenges and transaction state die with the old grant (they bind `grant_id` + `revocation_epoch`).
- The registry keeps revoked records (revocation tombstones) so stale resumption is rejected by `active_device()`/`ensure_still_active`; rotation MUST NOT use `forget_all` (that path exists only for `coven memory mobile disable --forget-devices --confirm-forget-devices`, `mod.rs`).
- Platform re-enrollment on the device is a fresh key generation (§4.1); no private material ever migrates between devices.

## 6. Assurance policy

### 6.1 Levels and enforcement points

The controlling insight: **assurance is enforced by which key operations the platform permits, not by claims the client makes.** The host never receives, and never evaluates, biometric data; it sees only signatures that could only have been produced if the platform's gating policy allowed a signing operation at that moment.

| Level | Platform mechanism (what actually gates the key) | Host-side proof |
| --- | --- | --- |
| 1 `possession` | Possession key usable; request signed | `COVEN-MEMORY/1` canonical-request verification + grant `authorize` (`auth.rs`) |
| 2 `recent_user_verification` | Authorization-key access control with a bounded reuse window (iOS `touchIDAuthenticationAllowableReuseDuration`; Android `setUserAuthenticationParameters(timeout, …)`) — the OS refuses to sign after the window lapses | server-side policy notion only: no ceremony exists to sign, so `COVEN-ASSURANCE/1` defines no proof class for it (`mobile-assurance-step-up-v1.md`) |
| 3 `fresh_user_verification` / `fresh_biometric` | Per-operation key use on the authorization key: iOS access-control flag + `evaluatePolicy` immediately before signing; Android `setUserAuthenticationParameters(0, …)` + `BiometricPrompt` with `CryptoObject(Signature)` over the exact action digest | `COVEN-ASSURANCE/1` proof: authorization-key signature over a server-issued single-use challenge and a server-recomputed context digest; the server computes the effective assurance itself (§6.3) |
| 4 `step_up` | Second trusted device, recovery credential, or owner ceremony (#788) | reserved rank, never minted by `COVEN-ASSURANCE/1` |

Above `possession`, assurance is proven — never asserted — exactly as `COVEN-ASSURANCE/1` specifies (`../design/mobile-assurance-step-up-v1.md`, #815, landed by #871): the separate platform-gated authorization key (§4.7) signs canonical proof bytes binding the device, grant, authorization (revocation) epoch, a server-issued single-use challenge, and a context digest the server recomputes from the actual request or action bytes. The server caps the signed claim by the enrolled key's declared class and passes the resulting **effective assurance** to `DeviceGrant::authorize`; it never evaluates a client-declared level, and `ensure_still_active` revalidates with that same value at effect time (§6.3, §6.4).

Because level-3 signatures can only be produced when the platform just verified the user, "biometric material never leaves the platform biometric subsystem" holds by construction: Coven receives a signature, not an authentication state.

### 6.2 Platform mapping matrix

| Platform | Non-exportable key | `recent_user_verification` | `fresh_biometric` | Biometric vs PIN distinction |
| --- | --- | --- | --- | --- |
| iOS (Secure Enclave) | Secure Enclave P-256, `kSecAttrTokenIDSecureEnclave` | access control with bounded reuse duration | `.biometryCurrentSet` + `evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics)` just-in-time | `.deviceOwnerAuthenticationWithBiometrics` (biometric-only) vs `.deviceOwnerAuthentication` (biometric-or-passcode) are distinct APIs; use the former for level 3 |
| Android (Keystore/TEE/StrongBox) | hardware-backed key, non-exportable | `setUserAuthenticationParameters(timeout > 0, …)` | per-operation auth (`timeout = 0`) + `BiometricPrompt` `CryptoObject`, strong-biometric class | the `BIOMETRIC_STRONG` vs `DEVICE_CREDENTIAL` authenticator classes (via `BiometricManager`); `canAuthenticate` result recorded as method class |
| Fallback platforms | software key (exportable) | n/a | not claimable | grant records `unattested_device`; scopes whose policy demands fresh verification fail closed — no enrolled authorization key ⇒ effective assurance stays `possession` ⇒ `AssuranceRequired` |

The `COVEN-ASSURANCE/1` enrollment classes are the device-side declaration of this matrix: `biometric_only` (ceiling `fresh_biometric`), `user_verification` and `device_credential` (ceiling `fresh_user_verification`). The class is confirmed in the pairing transcript at enrollment (§4.7); the host trusts it only as an enrollment fact and caps every proof by it at verification time.

Policy default proposal (derivable from `spec/device-pairing/v1/capabilities.json` risk classes): `low` → possession; `moderate`/`sensitive` → recent_user_verification; `high` → fresh_user_verification (fresh_biometric where the platform distinguishes it); `critical` → fresh_biometric, with `identity.admin`, `devices.*`, `*export` additionally requiring step_up. Defaults are issuance-time policy encoded in the grant's `restrictions` (`require_fresh_user_verification_for` / `requiredAssurance`), so self-hosted owners can override without code changes.

### 6.3 Step-up proofs are `COVEN-ASSURANCE/1`, not a client-signed assertion

Earlier drafts of this plan proposed a device-signed **AssuranceAssertion** diagnostic object. That design is withdrawn: a client-signed assertion is precisely the "client upgrades assurance by sending an asserted enum" shortcut that [`COVEN-ASSURANCE/1`](../design/mobile-assurance-step-up-v1.md) (#815, landed by #871) exists to prevent. This plan defers to that contract in full — canonical proof bytes, challenge store, verification procedure, transport headers, portable golden vector — and records only the binding decisions #786 adds on top:

- **Separate authorization key.** Proofs are signed by the enrolled step-up authorization key (§4.7) — never by the possession key that authenticates requests; the two are distinct keys and the registry rejects an authorization key equal to the possession key.
- **Server-issued single-use challenge.** Every proof covers a 32-byte challenge minted by the possession-authenticated route `POST /api/v1/mobile/assurance/challenge`, bound to `(device_id, grant_id, revocation_epoch)`, atomically consumed on first successful verification within its ≤120 s window. Grant rotation or revocation immediately invalidates outstanding challenges; a daemon restart cannot resurrect a spent challenge.
- **Server-recomputed context.** The server recomputes `context_digest` itself — the exact `COVEN-MEMORY/1` canonical request bytes (`request` mode) or the canonical action-transaction bytes (`action` mode, §7) — and never trusts a client-supplied digest; `context_mode` keeps the two domains non-substitutable.
- **Server-computed effective assurance.** `effective = min(requested_assurance, class_ceiling)`; the claim lives inside the signature, the ceiling is the enrolled key's declared class, and any verification failure collapses the effective assurance to `possession`, after which the grant's own policy fails the request closed (`GrantError::AssuranceRequired`).
- **Revalidation at effect time.** `ensure_still_active` re-checks grant id + `revocation_epoch` before the action executes and reuses the request's effective assurance (`VerifiedMobileDevice.effective_assurance`), so a grant replaced or revoked mid-flight kills the pending action — and a legitimately step-up-authorized request is not failed by the re-check.
- **No biometric data.** The proof carries only the platform-reported verification *class* of the enrolled key, an enrollment-time fact; nothing biometric exists anywhere in the protocol. `recent_user_verification` remains a server-side policy notion with no proof class; `step_up` is reserved for #788.

Any UI/display vocabulary for the verification method (strong biometric vs device credential) is derived locally from the enrolled class and is display-only; it is never a wire input to the authorization decision.

### 6.4 Step-up decision flow

```text
request (scope S) arrives
  │ auth.rs verify: possession checks pass?
  ├─ no  → reject (audit AuthenticationRejected)
  ├─ grant lacks scope S → ScopeDenied
  ├─ S requires assurance above possession (restrictions / minimum_assurance)
  │     → client fetches a COVEN-ASSURANCE/1 challenge (single-use, bound to
  │        device_id / grant_id / revocation_epoch)
  │        device performs the platform verification gating its authorization key
  │        → authorization key signs the COVEN-ASSURANCE/1 proof over the
  │          server-recomputed context (request or action bytes)
  │        → server verifies: challenge unspent + unexpired, signature, ≤120 s window
  │        → effective = min(requested, enrolled class ceiling)   (server-computed, §6.3)
  ├─ rank(effective) ≥ rank(required) → authorize
  │     any proof failure ⇒ effective = possession ⇒ AssuranceRequired ⇒ reject
  └─ before executing: ensure_still_active re-checks grant id + revocation epoch
        with the same effective assurance (revalidation at effect time; TOCTOU)
```

The host never asks the device "is the user verified?"; it demands a `COVEN-ASSURANCE/1` proof that could only have been produced under the required verification, caps it by the enrolled key's own class, and treats the absence of that capability as an authorization failure.

## 7. Transaction authorization

### 7.1 Exact-action canonicalization today

`grant.rs` `DeviceActionIntent` already implements the issue's core demand — sign the exact canonical action, never `approve=true`:

- fields: `scope`, `operation` (≤ 64 chars, `[a-z0-9._-]`), `target` (≤ 512, trimmed, no control chars), `effect_digest`, `nonce`, `issued_at`, `expires_at`;
- `canonical_bytes()` = `"COVEN-ACTION/1\0"` + u32 big-endian length-prefixed fields, RFC 3339 millisecond timestamps;
- `nonce` and `effect_digest` must be canonical base64url of exactly 32 bytes;
- lifetime is validated to (0, `MAX_ACTION_LIFETIME_SECONDS = 300`] seconds;
- the unit tests bind every material field (target/effect/expiry mutations change the canonical bytes).

The issue's example fields map as: `operation: deploy`, `target: production-eu`, `repository`, `commit`, `effect: modifies production` → `operation` + `target` strings and the `effect_digest`; `nonce` and the short deadline are first-class validated fields.

The gap: `effect_digest` is an opaque, client-contracted digest — the descriptor format it hashes is unspecified (decision point D5). The phone can render `operation` and `target`, but it cannot render `repository`, `commit`, or the effect text from a digest, so it cannot verify that what it displays is what it signs; display/authorize substitution on exactly those fields is undetectable on the device. §7.2 resolves D5 by folding the effect descriptor into one canonical typed transaction.

### 7.2 Canonical typed transaction (`COVEN-ACTION/2`) — D5 resolution

The #786 exact-action contract is **one canonical typed transaction**, `COVEN-ACTION/2`, superseding `COVEN-ACTION/1`'s opaque `effect_digest` for new implementations. Every displayed material field is a typed transaction field; nothing the user approves exists only as a digest of an unspecified input.

Framing follows the repo's canonical-byte conventions: `"COVEN-ACTION/2\0"` then each field as a u32 big-endian length followed by its bytes — the same discipline as `DeviceActionIntent::canonical_bytes` and `COVEN-ASSURANCE/1`. Fields, in order:

| # | Field | Encoding | Displayed |
| --- | --- | --- | --- |
| 1 | `device_id` | raw 16-byte UUID | actor reference (grant/device binding) |
| 2 | `grant_id` | raw 16-byte UUID | actor reference (grant/device binding) |
| 3 | `revocation_epoch` | u64 big-endian | validity/replay binding (§5.5) |
| 4 | `scope` | UTF-8 snake-form `DeviceScope` | yes |
| 5 | `operation` | UTF-8, ≤ 64 chars, `[a-z0-9._-]` | yes |
| 6 | `target` | UTF-8, ≤ 512, trimmed, no control chars | yes |
| 7 | `effect` | UTF-8 canonical JSON effect descriptor (below) | yes |
| 8 | `request_digest` | raw 32-byte SHA-256 | binding only |
| 9 | `presentation_digest` | raw 32-byte SHA-256 | binding only |
| 10 | `nonce` | raw 32 bytes | yes (hex in this plan's vector; wire encoding is canonical base64url) |
| 11 | `issued_at` | RFC 3339 UTC, millisecond precision | yes |
| 12 | `expires_at` | RFC 3339 UTC, millisecond precision | yes (deadline) |

- **Typed effect descriptor.** `effect` is canonical JSON (lexicographically sorted keys, no insignificant whitespace) with a closed schema (`additionalProperties: false`): `verbs` (non-empty array), `repository`, `commitIds` (array), and `summary` — for the deploy example exactly `{"commitIds":["abc1234"],"repository":"OpenCoven/psyche","summary":"modifies production","verbs":["deploy"]}`. Repository, commit ids, effect verbs, and the effect summary are therefore transaction fields the phone renders and signs; unknown effect members are rejected like any other unknown protocol value. The diagnostic effect schema ships next to `transaction-authorization.schema.json` in the implementation slice.
- **Request binding.** `request_digest` = SHA-256 over the exact `COVEN-MEMORY/1` canonical request bytes that carried the transaction — recomputed by the server, never client-asserted (the same discipline as the `COVEN-ASSURANCE/1` context digest) — tying the approval to the request that submitted it.
- **Presentation binding.** `presentation_digest` = SHA-256 over the canonical human-readable rendering (§7.3), as required by `transaction-authorization.schema.json` alongside `requestDigest`.
- **Replay state.** The `nonce` plus a server-side consumption record make the transaction single-use: the server atomically marks `(device_id, grant_id, revocation_epoch, nonce)` consumed on acceptance — the same single-winner discipline as the challenge spend and `auth.rs::insert_nonce` — persisted so a daemon restart cannot resurrect it, inside the ≤ 300 s window (`MAX_ACTION_LIFETIME_SECONDS`). Grant replacement or revocation invalidates outstanding transaction state (§5.5 epoch rule).
- **Authorization.** The transaction rides in the body of a possession-authenticated request: the `COVEN-MEMORY/1` signature covers it through the body digest, and `request_digest` binds the transaction to that exact request. Operations whose grant policy requires fresh verification additionally carry a `COVEN-ASSURANCE/1` `action`-mode proof (§6.3) whose server-recomputed `context_digest` is SHA-256 of these canonical transaction bytes; for `action` mode the proof window nests inside the transaction window. The host re-derives the canonical bytes from the received fields and verifies; any mismatch — including a display/authorize substitution — fails verification.

### 7.3 Presentation binding and the mobile UI MUST

The threat-model adversary "compromised middleware that changes an action after showing approval UI" is neutralized by a two-digest contract:

1. **`presentationDigest`** — SHA-256 over the canonical human-readable rendering (exact field set rendered by the UI). The `TransactionAuthorization` diagnostic schema (`spec/device-pairing/v1/transaction-authorization.schema.json`) requires it alongside `requestDigest`.
2. The mobile UI MUST render every material field covered by the transaction digests — operation, target, effect (verbs, repository, commit ids, summary), nonce, expiry — and MUST obtain its display strings from the same canonical transaction object it signs (§7.2). A UI that renders from any other source (push body, relay-provided preview, cached text) violates the contract.

Golden test (required by Gate D and `mobile-device-pairing-v1.md` "Required security tests: transaction presentation/signature mismatch"): mutate any presented field after rendering and verify verification fails.

### 7.4 Worked golden example

Canonical `COVEN-ACTION/2` transaction (the issue's deploy example; values are fixed so the vector is reproducible from this text alone):

```text
device_id            = 00000000-0000-4000-8000-00000000000a
grant_id             = 0b6f2864-c085-57aa-93a0-a2634f3b946c
                       (uuidv5(device_id, "coven-device-grant-v3:1") — §5.5 succession naming)
revocation_epoch     = 0
scope                = tool_execution_approve
operation            = deploy.production
target               = production-eu
effect               = {"commitIds":["abc1234"],"repository":"OpenCoven/psyche","summary":"modifies production","verbs":["deploy"]}
                       (canonical JSON: sorted keys, no whitespace — every displayed effect field present)
request_digest       = 1111111111111111111111111111111111111111111111111111111111111111
                       (synthetic fixed vector input; real values are the server-recomputed
                       SHA-256 of the COVEN-MEMORY/1 canonical request that carried the
                       transaction, never a client-asserted digest)
presentation_digest  = 334d917e2208ea2a5e20a75aac4135fe4696da5d4d754dc1f5729276a8edd264
                       (SHA-256 of the canonical rendering below, trailing newline included)
nonce                = 30663363316536346139623234643565386337663061316432653362346335643630
                       (the 32 ASCII bytes "0fc1e4a9b24d5e8c7f0a1d2e3b4c5d60"; displayed to
                       the user in hex, this vector's convention)
issued_at            = 2026-08-30T15:00:00.000Z
expires_at           = 2026-08-30T15:02:00.000Z        (120 s ≤ 300 s cap)

canonical bytes (hex):
434f56454e2d414354494f4e2f3200000000100000000000004000800000000000000
a000000100b6f2864c08557aa93a0a2634f3b946c0000000800000000000000000000
0016746f6f6c5f657865637574696f6e5f617070726f7665000000116465706c6f792
e70726f64756374696f6e0000000d70726f64756374696f6e2d65750000006c7b2263
6f6d6d6974496473223a5b2261626331323334225d2c227265706f7369746f7279223
a224f70656e436f76656e2f707379636865222c2273756d6d617279223a226d6f6469
666965732070726f64756374696f6e222c227665726273223a5b226465706c6f79225
d7d000000201111111111111111111111111111111111111111111111111111111111
11111100000020334d917e2208ea2a5e20a75aac4135fe4696da5d4d754dc1f572927
6a8edd264000000203066633165346139623234643565386337663061316432653362
34633564363000000018323032362d30382d33305431353a30303a30302e3030305a0
0000018323032362d30382d33305431353a30323a30302e3030305a

transaction digest = SHA-256(canonical) = 3919f1957068dec15460e244181204357f959f685a3f6018fffa543c932b8fbe
```

The `presentation_digest` input — the canonical rendering the phone displays (trailing newline included; the nonce is shown in hex):

```text
operation  deploy.production
target     production-eu
repository OpenCoven/psyche
commit     abc1234
effect     modifies production
nonce      3066633165346139623234643565386337663061316432653362346335643630
expires    2026-08-30T15:02:00.000Z
```

Every displayed field — operation, target, repository, commit ids, effect verbs and summary, nonce, expiry — is inside the transaction digest, so display/authorize substitution is detected on the device. Digests and the nonce are shown in hex; the protocol encodes these fields as canonical base64url (`URL_SAFE_NO_PAD` in `grant.rs` — the hex and base64url forms are the same bytes). The implementation locks this vector (canonical bytes + digests) alongside the shared protocol library vectors; the `COVEN-ACTION/1` scheme and its tests remain the current-code baseline in `grant.rs`.

## 8. Device management

### 8.1 CLI surface

Existing surface (`main.rs`): `coven memory mobile enable|disable|status|pair|devices [--json]`, with `devices revoke <device-id>`. The registry already supports rename (`registry.rs::rename`), grant replacement, and revocation, so the issue's command list is mostly exposed plumbing:

| Issue command | Today | Plan |
| --- | --- | --- |
| `coven device list` | `coven memory mobile devices [--json]` (`run_devices`) | new `coven device list` verb tree; keep the old path as an alias through a deprecation window |
| `coven device inspect <device>` | absent (only list status) | new runner over `registry::device()` + `authorization_record()`: grant scopes, restrictions, minimum assurance, issuance/expiry, revocation epoch, last audit events |
| `coven device rename <device>` | registry only | CLI wrapper over `DeviceRegistry::rename` |
| `coven device revoke <device>` | `coven memory mobile devices revoke` (`run_revoke_device`) | same runner; bumps revocation epoch (§8.3) |
| Grant scope editing/reissuance | `replace_grant` exists; no CLI | `coven device rescope` (§5.5) |
| Key rotation/re-enrollment | re-pair only | §5.6 succession flow |

Recommendation: introduce the `coven device …` tree now (the delivery plan's PR 3 already names `device list|inspect|rename|revoke`), implemented against the same registry module, and deprecate the `memory mobile devices` spelling on a documented window. Alternative considered: keep the memory-scoped commands only — rejected because #786 explicitly generalizes devices beyond memory and the threat model treats the device as a first-class principal.

### 8.2 Grant lifecycle operations

All administration goes through audited registry operations (`register_with_grant`, `replace_grant`, `rename`, `revoke`); the CLI never edits registry files directly. `forget_all` remains the "disable and forget" nuclear path requiring the double flag guard.

### 8.3 Lost-device and suspected-compromise workflow

```text
$ coven device revoke <device> --reason suspected_compromise
→ registry.revoke(device_id, now)            # DeviceRecord.revoked_at set
→ revocation epoch raised on the device's grant lineage
→ audit: device_revoked (reason classified; no key material)
→ device's next request fails DeviceRevoked at lookup; sessions re-check
   ensure_still_active() and die on epoch mismatch — no resumption possible
```

Revocation invalidates the device's authority only — familiar identities and memory are untouched (accepted architecture, `mobile-device-trust.md` "Revoking a device…"). The revocation reasons are already enumerated by `revocation-record.schema.json` (`user_requested`, `lost`, `suspected_compromise`, `key_rotation`, `policy_change`, `expired`, `other`); the CLI should classify the workflow (`--lost`, `--suspected-compromise`) into those reasons so the audit trail distinguishes "lost" from "compromised" (the latter additionally triggers the #788 recovery review).

## 9. State machines

### 9.1 Device credential lifecycle (device side)

```text
uninitialized
  → generating (platform key facility; access-control policy attached)
  → awaiting_enrollment        (key exists; signs enrollment request bound to #785 transcript)
  → enrolled                   (grant received; possession works)
  → rotating                   (new key generated; old grant narrowed to revoke-only, optional)
  → revoked_or_wiped           (local wipe on revocation notice; keys destroyed)
terminal: uninitialized | enrolled | revoked_or_wiped
biometric state is NOT part of this machine — it gates key use, not key existence
```

Invalid transitions fail closed; enrollment can only complete through the #785 ceremony (offer → handshake → human verification → grant), never by importing a key.

### 9.2 Grant lifecycle (host authority)

```text
absent → issued (active)
issued → narrowed      (scope editing; new grant id, authorization epoch +≥1)
issued → reissued      (rotation/re-issue; new grant id, authorization epoch +≥1)
issued → expired       (expires_at passes; irreversible)
issued → revoked       (revocation record + epoch bump; ensure_still_active kills sessions)
revoked/expired → terminal; re-authorization requires a fresh enrollment ceremony
```

`auth.rs::ensure_still_active` already enforces the epoch/grant-id check mid-session. The migration (§11) must preserve `revocation_epoch` monotonicity per device, and every authority-changing replacement advances it with a fresh grant id (§5.5) — grant-id or epoch reuse across replacements is an implementation error.

### 9.3 Assurance evaluation (host)

Possession verified → scope present? → per-scope fresh-verification restriction? → presented assurance rank ≥ required? → action is transaction-bound (nonce fresh, expiry ≤ 300 s, digest matches request)? → grant still active at execution time.

## 10. Audit events

`audit.rs` today emits eight events (`GatewayStarted`, `GatewayStopped`, `PairingCreated`, `PairingCompleted`, `PairingRejected`, `DeviceRevoked`, `AuthenticationRejected`, `RateLimited`) as coarse JSONL records (0600, `O_NOFOLLOW`, 4 MiB truncation; the `audit_records_only_allowed_coarse_fields` test forbids path/memoryId/endpoint/fingerprint/nonce/signature/body fields — an invariant to preserve).

The issue requires audit events for enrollment, authentication, step-up approval, scope changes, and revocation. Proposed taxonomy extension (same coarse-field invariant, extend the forbidden-field test as new fields land):

| Event | Trigger |
| --- | --- |
| `DeviceEnrolled` | grant issued at enrollment completion (complements `PairingCompleted`) |
| `AuthenticationSucceeded` | successful `MobileAuthenticator::verify` (device id only, never path/nonce) |
| `StepUpVerified` / `StepUpRejected` | §6.4 flow outcomes (names follow `COVEN-ASSURANCE/1`'s audit recommendation) |
| `GrantIssued` / `GrantScopeNarrowed` / `GrantReissued` | §5.5 (record old + new grant id and old + new authorization epoch) |
| `DeviceRenamed` | §8.1 |
| `DeviceKeyRotated` | §5.6 |
| (existing) `DeviceRevoked` | extend to record the reason class from §8.3 |

Records stay free of private keys, biometric material, nonces, and signatures (the current test asserts exactly this class of redaction — extend it, do not weaken it).

## 11. Registry migration

`registry.rs` already carries the legacy migration shape (`LegacyDeviceRecord`/`LegacyDeviceScope` → `GrantedDeviceRecord`), and `mobile-device-trust.md` Stage B prescribes: preserve v1 `memory_read` devices by representing them as grants with exactly their existing authority — one `memory_read` scope, `minimum_assurance: possession`, audience `local_coven_authority`, no expiry — and never widening access.

Deterministic migration rule: for each legacy device record, synthesize `DeviceGrant::for_device(device_id, public_key_x963, scopes_from_record, paired_at)`; keep the same `device_id` so audit history survives. Fail closed on unknown legacy scopes (`LegacyDeviceScope` must map 1:1 or the migration aborts) — the same unknown-value-fails-closed rule the schemas require.

## 12. Self-hosted and cloudless operation

Acceptance criterion: self-hosted deployments use the credential/grant model without an OpenCoven cloud account. Current implementation satisfies the shape: the grant issuer is `DeviceGrantAudience::LocalCovenAuthority` (`grant.rs`), the registry/audit/state live under the local Coven home (`config.rs` private-file discipline: atomic replace, 0600, `O_NOFOLLOW`), and pairing runs over the local daemon socket (`mod.rs::run_pair`) and a TLS gateway on the private network. Nothing in this plan introduces a cloud dependency: the relay (#787) and any account/recovery service (#788) remain optional transports/factors that never mint authority. The plan's additions (assurance challenges and proofs, audit events, CLI verbs) are all local artifacts, and the challenge issuer is the local daemon.

## 13. Test plan

Extends `docs/architecture/mobile-device-pairing-v1.md` "Required security tests" and Gate D/E:

Platform credential (device-side, where the platform allows):

- key generated non-exportable; export attempts fail on hardware-backed profiles;
- biometric-gated key refuses to sign without fresh verification (level 3);
- time-window key refuses to sign after the reuse window (level 2 boundary);
- PIN/passcode-only enrollment records the `device_credential` class (ceiling `fresh_user_verification`) and never `biometric_only`, so its proofs can never satisfy a `fresh_biometric` requirement;
- biometric enrollment change invalidates `.biometryCurrentSet`/`setInvalidatedByBiometricEnrollment` keys → re-enrollment flow.

Authority (Rust, host-side — citable paths for the test file locations):

- grant: subject binding, scope-set canonicality, time windows, assurance floor per restricted scope (existing tests in `grant.rs`; extend for the five-level vocabulary and the split familiar-memory scopes);
- transaction: `COVEN-ACTION/2` canonical-byte binding for every field, including each effect member (repository, commit ids, verbs, summary); operation/target charset and length limits; closed canonical effect JSON (unknown members rejected); lifetime ≤ 300 s; single-use nonce consumption — replay, post-restart, and post-grant-replacement submissions rejected;
- stolen-grant-without-key: replayed grant bytes without the private key fail `verify_signature` (existing path in `auth.rs`);
- revocation: mid-session revocation defeats `ensure_still_active` (epoch mismatch);
- grant replacement: every authority-changing replacement mints a fresh grant id and strictly advances the authorization epoch — same-id or equal-epoch replacement is rejected (tightened over the current `replace_grant` behavior), and outstanding challenges/transaction state die with the old grant;
- step-up: `COVEN-ASSURANCE/1` verification order — challenge single-use/expired/unknown/foreign-device/foreign-epoch rejection, possession-key-as-authorization-key rejection, `effective = min(requested, enrolled class ceiling)`, fail-closed `AssuranceRequired` on any proof failure, and `ensure_still_active` reusing the effective assurance at effect time (per `mobile-assurance-step-up-v1.md`'s adversarial test list);
- migration: v1 registry fixture converts byte-for-byte to the expected grant set without widening scopes;
- audit: taxonomy records are coarse-fields-only (extend the existing test).

Conformance (spec level, feeding `spec/device-pairing/v1/test-vectors.json`):

- golden vectors from §7.4 (canonical action bytes + digests) locked by the shared protocol library;
- grant diagnostic JSON validating against `device-grant.schema.json` for both the four-level and five-level vocabularies during the transition;
- the `COVEN-ASSURANCE/1` portable vector: implementations verify the proof signature over `canonicalProofBytesHex` with the step-up public key (ECDSA signature randomness means signatures are verified, not byte-reproduced);
- unknown capability/restriction/assurance values rejected.

## 14. Acceptance criteria mapping

| Issue acceptance criterion | Satisfied by |
| --- | --- |
| Biometric material never leaves the platform biometric subsystem | §4 (key-use gating), §6.1 (enforcement model), §6.3 (`COVEN-ASSURANCE/1` proofs carry no biometric data), audit redaction invariant |
| Assurance above possession is proven, never asserted | §4.7, §6.1, §6.3 (`COVEN-ASSURANCE/1`: separate authorization key, server-issued single-use challenge, server-recomputed context, server-computed effective assurance, revalidation at effect time) |
| Copying a grant/token without the device private key cannot authenticate | §2.1 (already enforced: ECDSA over canonical request against enrolled public key; `GrantError::SubjectMismatch`), §5.4 |
| Revoking the phone invalidates its authority without rotating familiar/root identities | §8.3, §9.2 (epoch + `revoked_at`), registry revoke path; familiar identity is a distinct subject (`mobile-device-trust.md` credential table) |
| High-risk approvals are nonce-bound, expiring, replay-resistant, transaction-specific | §7 (`COVEN-ACTION/2` canonical typed transaction: every displayed field, request/presentation digests, 300 s cap, 32-byte nonce, single-use replay state, tests) |
| Policy distinguishes biometric-only from device-credential fallback | §4.2–§4.4, §5.3 (five-level vocabulary), §6.2 (platform matrix) |
| Self-hosted deployments need no OpenCoven cloud account | §12 |

## 15. Maintainer decision points

- **D1 — Canonical capability vocabulary (dot vs snake).** Recommend dot-form as the protocol canonical (matches `spec/device-pairing/v1`), snake-form only as the Rust serde encoding, with a total mapping table in the contract layer (§5.2). Alternatives: snake everywhere (breaks published schemas), dual vocabulary without mapping (guarantees drift).
- **D2 — Add `fresh_user_verification` to the spec assurance enum.** Recommended (§5.3); the implementation and accepted architecture already carry five levels. Alternative: drop `fresh_user_verification` from Rust — loses the documented recent/fresh distinction and forces passcode fallback into the biometric tier.
- **D3 — Split `familiar_memory_admin` into read/write scopes.** Recommended (least privilege; `capabilities.json` already models both). Alternative: keep the coarse scope and rely on transaction authorization for writes — weaker and inconsistent with the capability taxonomy.
- **D4 — `coven device` command tree vs `coven memory mobile devices`.** Recommended new top-level tree with alias window (§8.1). Alternative: leave as-is and document; the issue names the `device` verbs explicitly, so deferral only moves the rename later.
- **D5 — Effect-descriptor canonicalization (resolved in this revision).** Resolved as **one canonical typed transaction**: `COVEN-ACTION/2` (§7.2) makes every displayed material field — repository, commit ids, effect verbs, summary — a transaction field, adds `request_digest`/`presentation_digest`, and carries replay state (nonce + server-side single-use consumption bound to device, grant, and epoch). Alternative considered and rejected: keep the opaque `effect_digest` and only specify its input format — the phone still could not render and verify what it signs.
- **D6 — Where assurance policy lives.** Recommended: policy is encoded in the grant at issuance (`restrictions.requiredAssurance` / `require_fresh_user_verification_for`), defaults derived from capability risk classes; per-host policy file as a later extension. Alternatives: per-request policy headers (widens the wire contract), device-side policy (unenforceable — the device would grade its own homework).
- **D7 — Grant algorithm profile.** Keep P-256/ECDSA for device keys (matches iOS Secure Enclave and Android hardware keystores; already used end-to-end in `auth.rs`/`grant.rs`); Ed25519 remains the portable software-identity profile per `mobile-device-pairing-v1.md`. Alternative: Ed25519 device keys via platform support where available — rejected for v1 to keep one interoperability profile.
- **D8 — Audit event extensibility.** Recommended: extend `MobileAuditEvent` in place (new variants, unchanged record shape) rather than versioning the JSONL format; the redaction test stays the invariant guard.

## 16. Risks

- **Platform variance:** biometric-vs-credential distinction is only as strong as the platform reports it (e.g. Class 2 biometrics on Android are `BIOMETRIC_WEAK`). Mitigation: policy treats anything weaker than `BIOMETRIC_STRONG`/biometric-only LAContext as `device_credential`; grants that require `fresh_biometric` fail closed on platforms that cannot produce it.
- **Assurance downgrade by configuration:** a self-hosted owner can set `minimum_assurance: possession` everywhere; the model permits it, the docs must say plainly that this trades security for convenience and that defaults derive from capability risk.
- **Registry migration:** converting legacy records must be atomic and idempotent (`config.rs` already writes private files atomically); a partial migration must never leave a device authority-less or doubly-authorized — tests in §13.
- **UX pressure to weaken step-up:** every "ask less often" request is a policy change; the design keeps the knobs in grant restrictions (`require_fresh_user_verification_for`, `maxIdleSeconds`) rather than in ad-hoc bypasses.
- **Alias window for CLI:** two spellings for the same operations invite drift; deprecation window must be short and documented.

## 17. Issue checklist mapping

| Issue item | Section |
| --- | --- |
| Non-exportable signing key on enrollment | §4.1 |
| iOS Secure Enclave + Keychain access-control/LocalAuthentication | §4.2 |
| Android Keystore + BiometricPrompt/strong-biometric | §4.3 |
| PIN/passcode fallback distinct from biometric-only | §4.4, §5.3, §6.2 |
| Bind device public key to #785 enrollment transcript | §4.6, §5.4 |
| Pairwise/pseudonymous device identity per relationship | §4.5 |
| Signed grant ≈ DeviceGrant sketch | §5.1 (schemas exist; Rust mapping + deltas) |
| Capability taxonomy (all ten listed classes) | §5.2 |
| Assurance levels 1–4 | §6.1, §6.2 |
| Transaction authorization, exact canonical action | §7 |
| Mobile UI renders digest-covered fields | §7.3 |
| `coven device list/inspect/rename/revoke` | §8.1 |
| Grant scope editing/reissuance | §5.5 |
| Key rotation/re-enrollment | §5.6 |
| Lost-device / suspected-compromise workflow | §8.3 |
| Audit events for enrollment/auth/step-up/scope/revocation | §10 |
