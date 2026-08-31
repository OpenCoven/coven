# Device-Bound Credentials, Biometric Authorization, and Scoped Grants — Plan

Status: Plan and implementation contract for issue #786

Tracks: #786. Parent architecture: #784 (`mobile-device-trust.md`, `mobile-device-pairing-v1.md`). Depends on pairing bootstrap (#785). Sits at step 5 ("Pocket credentials and biometric authorization") of the delivery train in `mobile-device-pairing-delivery-plan.md`.

Companion documents:

- [`mobile-device-pairing-v1.md`](mobile-device-pairing-v1.md) — the canonical protocol contract this plan implements
- [`../design/mobile-device-trust.md`](../design/mobile-device-trust.md) — accepted architecture and authority boundary
- [`../security/mobile-device-pairing-threat-model.md`](../security/mobile-device-pairing-threat-model.md) — adversaries and required controls
- [`../../spec/device-pairing/v1/`](../../spec/device-pairing/v1/README.md) — diagnostic schemas, capability vocabulary, conformance gates

Normative language: MUST / MUST NOT / SHOULD / MAY per RFC 2119, as used in `mobile-device-pairing-v1.md`.

## 1. Purpose and scope

Issue #786 turns the paired phone into a **delegated principal**: a device whose private key is generated and protected by the platform, whose authority is expressed by a revocable, scoped `DeviceGrant`, and whose local biometrics are a gate on key use — never a network credential.

This plan specifies:

1. the platform device-credential policy (iOS Secure Enclave/Keychain, Android Keystore/BiometricPrompt) and how PIN/passcode fallback is kept distinct from biometric-only assurance;
2. the device-grant object model as it maps onto the existing protocol schemas and the current Rust authority, including the capability/assurance vocabulary alignment the implementation needs;
3. assurance-level policy (possession → recent user verification → fresh biometric → step-up) and where each level is enforced;
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
#   1. daemon posts the action intent (operation/target/effect/nonce/expiry)
#   2. phone renders EVERY digest-covered field, then Face ID
#   3. Secure Enclave signs the canonical action bytes; nothing biometric leaves the phone
# 4. daemon verifies signature + grant + assurance; the action executes once
# 5. audit: step_up approved, grant id, device id — never the biometric, never the payload
```

Reproducible golden vectors for that flow are in §7.3.

## 4. Device credential

### 4.1 Key generation and storage policy

The device credential is a non-exportable signing key created at enrollment time. The remote world only ever sees the public key (`EnrollmentRequest.deviceKey`, `spec/device-pairing/v1/enrollment-request.schema.json`) and signatures.

Requirements (extending Gate E of `spec/device-pairing/v1/implementation-gates.md`):

- C-1. The key MUST be generated inside the platform key facility on the device — never imported, never derivable from a seed the app stores.
- C-2. The key MUST be bound to an access-control policy that gates every private-key operation (see §4.2/§4.3). The mobile client MUST NOT use a key whose use is ungated when the platform offers gating.
- C-3. The private key MUST be non-extractable when the platform supports it. If a platform cannot provide hardware protection (older Android, developer devices), the client MUST fall back to software keys and the grant MUST record `unattested_device` attestation (`spec/device-pairing/v1/capabilities.json`), and policy MAY restrict what an unattested device may hold.
- C-4. The key MUST be bound to the enrollment transcript from #785: enrollment signs the transcript hash (already required by the enrollment-request schema); if the key is destroyed the device re-enrolls as a new device — a key never silently re-binds to a new transcript.
- C-5. One keypair per Coven relationship: the device generates a fresh keypair per pairing ceremony. No identifier derived from the key or hardware may be reused across unrelated trust domains (threat: cross-Coven correlation).
- C-6. Losing the platform's unlock factors (biometrics re-enrolled, passcode change) MUST be detected via key invalidation (`biometryCurrentSet` on iOS, `setInvalidatedByBiometricEnrollment` on Android) and treated as a re-enrollment trigger, not silently retried.

### 4.2 iOS evaluation (Secure Enclave + Keychain access control)

Recommended client contract for the Pocket/iOS client:

- Generate the key with `kSecAttrTokenIDSecureEnclave`, algorithm P-256 (ECDSA/ES256) — the only algorithm Secure Enclave keys support. This matches the existing P-256 device-key profile (`auth.rs` verifies P-256 sec1 points; `docs/architecture/mobile-device-pairing-v1.md` allows "platform-native hardware keys MAY use P-256 with an explicitly encoded algorithm identifier").
- Create the key with `SecAccessControlCreateFlags` combining `kSecAccessControlPrivateKeyUsage` and one of:
  - `kSecAccessControlBiometryCurrentSet` — biometric-gated; invalidated when enrolled biometrics change;
  - `kSecAccessControlDevicePasscode` (or `.userPresence`) — the documented device-credential fallback path, kept as a **separate** key or a separately-audited policy branch, never silently merged with biometric-only assurance (§4.4).
- For `fresh_biometric`, sign inside `LAContext`-gated key use (`evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics)` immediately before the signing operation). The reuse window (`touchIDAuthenticationAllowableReuseDuration`) is bounded by the platform and maps to `recent_user_verification` at best, never `fresh_biometric`.
- The host observes only: a signature over the exact challenge/action digest, or an error. A biometric denial surfaces as "user verification failed", never as any biometric datum.

### 4.3 Android evaluation (Keystore + BiometricPrompt)

- Generate the key in Android Keystore with `KeyGenParameterSpec` and hardware-backed storage; then attach the authentication policy that matches the assurance the credential may claim:
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

## 5. Device grant

### 5.1 Object model: spec schema ↔ Rust implementation

The canonical object is defined in `docs/architecture/mobile-device-pairing-v1.md` ("Device grant") and validated diagnostically by `spec/device-pairing/v1/device-grant.schema.json` (CBOR/COSE canonical; JSON diagnostic only). The Rust authority implements it in `grant.rs`:

| Spec field (`device-grant.schema.json`) | Rust (`DeviceGrant`, `grant.rs`) | Note |
| --- | --- | --- |
| `version` (1) | `DeviceGrant.version` (`DEVICE_GRANT_VERSION = 1`) | equal |
| `grantId` (22-char b64url of 16 bytes) | `DeviceGrant.id: Uuid` (`Uuid::new_v5(&device_id, b"coven-device-grant-v1")`) | diagnostic encoding differs; same 128-bit id |
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

Recommendation: extend the spec vocabulary to five levels (add `fresh_user_verification`) to match the implementation and the accepted architecture document. `capabilities.json` ranks are a total order; `authorize()` already relies on rank ordering (`presented_assurance < required_assurance`), so the extension is monotonic and backwards compatible. Alternative considered: collapse `fresh_user_verification` and `fresh_biometric` — rejected because the issue explicitly requires distinguishing biometric-only from broader device-credential fallback, and the Rust enum would lose the distinction it already encodes.

### 5.4 Grant issuance and verification lifecycle

Issuance (host authority):

1. Enrollment completes with the #785 transcript bound (offer digest, host fingerprint, device public key, requested capability digest, nonce, expiry — `pairing.rs` transcript, `enrollment-request.schema.json`).
2. The authority derives the grant from **what the operator approved in the UI**, never from what the device requested: scopes are the intersection of displayed/approved capabilities and policy floors (per-capability minimum assurance derived from the `risk` classes in `spec/device-pairing/v1/capabilities.json` — `critical` capabilities default to `fresh_biometric`/`step_up`).
3. The grant is stored atomically with the device record (`registry.rs` `register_with_grant`); `grant_id = uuidv5(device_id, "coven-device-grant-v1")` keeps the audit chain stable per device.

Verification (every protected interaction, already implemented and retained):

1. `auth.rs::verify` — registry reload, revocation check, ±300 s timestamp window, body-digest match, canonical-request ECDSA verify, grant authorize at the required scope, replay-nonce insertion, per-device rate limit;
2. `auth.rs::ensure_still_active` — before acting, grant id + `revocation_epoch` are re-checked against the registry so a device revoked mid-request cannot win the race (the TOCTOU re-check `docs/design/mobile-device-trust.md` requires).

### 5.5 Scope editing and reissuance

- Editing scope = issuing a **new grant** (`replace_grant` already exists in `registry.rs`); a grant never widens itself. The new grant gets a fresh `grant_id`; `ensure_still_active` rejects the old grant id at once, so narrowed authority takes effect at the next request without device cooperation.
- Narrowing MUST be monotone at reissuance: the new scope set is a subset of the union of the current grant and the operator's explicit selection, validated by `validate_scope_set` (sorted, unique, non-empty).
- Reissuance is an audited event (§10) that records old and new grant ids (digests only, per the audit redaction rules).
- Recommended API: `coven device rescope <device> --grant/--scope ...` writing a replacement grant through `replace_grant`; display the before/after scope diff before commit (same preview discipline as pairing scope selection).

### 5.6 Key rotation and re-enrollment

- Rotation = new enrollment ceremony (#785) for a **new** key, followed by grant succession: issue the new grant (same or narrower scopes), then revoke the old key with `reason: key_rotation` (`revocation-record.schema.json`). Both events are audited; the succession is explicit, never implicit.
- The registry keeps revoked records (revocation tombstones) so stale resumption is rejected by `active_device()`/`ensure_still_active`; rotation MUST NOT use `forget_all` (that path exists only for `coven memory mobile disable --forget-devices --confirm-forget-devices`, `mod.rs`).
- Platform re-enrollment on the device is a fresh key generation (§4.1); no private material ever migrates between devices.

## 6. Assurance policy

### 6.1 Levels and enforcement points

The controlling insight: **assurance is enforced by which key operations the platform permits, not by claims the client makes.** The host never receives, and never evaluates, biometric data; it sees only signatures that could only have been produced if the platform's gating policy allowed a signing operation at that moment.

| Level | Platform mechanism (what actually gates the key) | Host-side check |
| --- | --- | --- |
| 1 `possession` | Key usable; request signed | `auth.rs` canonical-request verification + grant `authorize` |
| 2 `recent_user_verification` | Key access-control with a bounded reuse window (iOS `touchIDAuthenticationAllowableReuseDuration`; Android `setUserAuthenticationParameters(timeout, …)`) — the OS refuses to sign after the window lapses | same signature verification; freshness is the platform's to enforce |
| 3 `fresh_user_verification` / `fresh_biometric` | Per-operation key use: iOS access-control flag on the key + `evaluatePolicy` immediately before signing; Android `setUserAuthenticationParameters(0, …)` + `BiometricPrompt` with `CryptoObject(Signature)` over the exact action digest | signature over the exact action digest; freshness is structurally guaranteed because the signature cannot exist otherwise |
| 4 `step_up` | Second trusted device, recovery credential, or owner ceremony (#788) | policy outside this issue; reserved rank |

Because level 3 signatures can only be produced when the platform just verified the user, "biometric material never leaves the platform biometric subsystem" holds by construction: Coven receives a signature, not an authentication state.

For audit and UI, the device MAY attach a signed **AssuranceAssertion** (§6.3) describing which class of local verification preceded a signature. The assertion is evidence for audit/display; the host MUST NOT treat it as authorization in place of the signature and grant checks.

### 6.2 Platform mapping matrix

| Platform | Non-exportable key | `recent_user_verification` | `fresh_biometric` | Biometric vs PIN distinction |
| --- | --- | --- | --- | --- |
| iOS (Secure Enclave) | Secure Enclave P-256, `kSecAttrTokenIDSecureEnclave` | access control with bounded reuse duration | `.biometryCurrentSet` + `evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics)` just-in-time | `.deviceOwnerAuthenticationWithBiometrics` (biometric-only) vs `.deviceOwnerAuthentication` (biometric-or-passcode) are distinct APIs; use the former for level 3 |
| Android (Keystore/TEE/StrongBox) | hardware-backed key, non-exportable | `setUserAuthenticationParameters(timeout > 0, …)` | per-operation auth (`timeout = 0`) + `BiometricPrompt` `CryptoObject`, strong-biometric class | the `BIOMETRIC_STRONG` vs `DEVICE_CREDENTIAL` authenticator classes (via `BiometricManager`); `canAuthenticate` result recorded as method class |
| Fallback platforms | software key (exportable) | n/a | not claimable | grant records `unattested_device`; sensitive scopes require `step_up` |

Policy default proposal (derivable from `spec/device-pairing/v1/capabilities.json` risk classes): `low` → possession; `moderate`/`sensitive` → recent_user_verification; `high` → fresh_user_verification (fresh_biometric where the platform distinguishes it); `critical` → fresh_biometric, with `identity.admin`, `devices.*`, `*export` additionally requiring step_up. Defaults are issuance-time policy encoded in the grant's `restrictions` (`require_fresh_user_verification_for` / `requiredAssurance`), so self-hosted owners can override without code changes.

### 6.3 AssuranceAssertion diagnostic schema (proposed)

New diagnostic schema (proposed for `spec/device-pairing/v1/assurance-assertion.schema.json`; same draft-2020-12 style as its siblings):

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://opencoven.ai/spec/device-pairing/v1/assurance-assertion.schema.json",
  "title": "OpenCoven AssuranceAssertion v1 diagnostic JSON representation",
  "description": "Device-signed, diagnostic-only record of which class of local user verification preceded a key operation. Evidence for audit and UI; never a substitute for signature and grant verification. Contains no biometric data — only the platform-reported verification class.",
  "type": "object",
  "additionalProperties": false,
  "required": ["version", "deviceId", "grantId", "assurance", "method", "verifiedAt", "nonce", "signature"],
  "properties": {
    "version": {"const": 1},
    "deviceId": {"type": "string", "minLength": 16, "maxLength": 128, "pattern": "^[A-Za-z0-9._:-]+$"},
    "grantId": {"type": "string", "pattern": "^[A-Za-z0-9_-]{22}$"},
    "assurance": {"enum": ["possession", "recent_user_verification", "fresh_user_verification", "fresh_biometric", "step_up"]},
    "method": {"enum": ["biometric_strong", "biometric_weak", "device_credential", "combined", "unknown"]},
    "verifiedAt": {"type": "integer", "minimum": 0},
    "expiresAt": {"type": "integer", "minimum": 0},
    "nonce": {"type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"},
    "signature": {"type": "string", "minLength": 1, "pattern": "^[A-Za-z0-9_-]+$"}
  }
}
```

Notes: `method` records only the *class* of platform verification (strong biometric vs device credential) — exactly the distinction the issue's acceptance criteria demand, and nothing biometric. `expiresAt` bounds how long the assertion may be replayed for display; the canonical representation is COSE-signed like every other protocol object (per `spec/device-pairing/v1/README.md`).

### 6.4 Step-up decision flow

```text
request (scope S) arrives
  │ auth.rs verify: possession checks pass?
  ├─ no  → reject (audit AuthenticationRejected)
  ├─ grant lacks scope S → ScopeDenied
  ├─ S ∈ restrictions.require_fresh_user_verification_for
  │     and presented assurance < required floor
  │   → 409-style "assurance required" challenge (fresh nonce)
  │        device performs platform verification on the exact action digest
  │        → signs AssuranceAssertion + action digest → host verifies → step-up recorded
  ├─ rank(presented) ≥ rank(required) → authorize
  └─ before executing: ensure_still_active re-checks grant + revocation epoch (TOCTOU)
```

The host never asks the device "is the user verified?"; it demands a signature that could only be produced under the required verification, and treats the absence of that capability as an authorization failure.

## 7. Transaction authorization

### 7.1 Exact-action canonicalization today

`grant.rs` `DeviceActionIntent` already implements the issue's core demand — sign the exact canonical action, never `approve=true`:

- fields: `scope`, `operation` (≤ 64 chars, `[a-z0-9._-]`), `target` (≤ 512, trimmed, no control chars), `effect_digest`, `nonce`, `issued_at`, `expires_at`;
- `canonical_bytes()` = `"COVEN-ACTION/1\0"` + u32 big-endian length-prefixed fields, RFC 3339 millisecond timestamps;
- `nonce` and `effect_digest` must be canonical base64url of exactly 32 bytes;
- lifetime is validated to (0, `MAX_ACTION_LIFETIME_SECONDS = 300`] seconds;
- the unit tests bind every material field (target/effect/expiry mutations change the canonical bytes).

The issue's example fields map as: `operation: deploy`, `target: production-eu`, `repository`, `commit`, `effect: modifies production` → `operation` + `target` strings and the `effect_digest`; `nonce` and the short deadline are first-class validated fields.

### 7.2 Presentation binding and the mobile UI MUST

The threat-model adversary "compromised middleware that changes an action after showing approval UI" is neutralized by a two-digest contract:

1. **`presentationDigest`** — SHA-256 over the canonical human-readable rendering (exact field set rendered by the UI). The `TransactionAuthorization` diagnostic schema (`spec/device-pairing/v1/transaction-authorization.schema.json`) requires it alongside `requestDigest`.
2. The mobile UI MUST render every material field covered by the signed digest — operation, target, effect, nonce, expiry — and MUST obtain its display strings from the same canonical object it signs. A UI that renders from any other source (push body, relay-provided preview, cached text) violates the contract.

Golden test (required by Gate D and `mobile-device-pairing-v1.md` "Required security tests: transaction presentation/signature mismatch"): mutate any presented field after rendering and verify verification fails.

### 7.3 Worked golden example

Canonical action (issue's deploy example), computed with the exact `COVEN-ACTION/1` scheme from `grant.rs` (values are fixed so the vector is reproducible; the implementation should lock it alongside the shared protocol library vectors):

```text
DeviceActionIntent {
  version      = 1
  scope        = tool_execution_approve
  operation    = deploy.production
  target       = production-eu
  effect_digest = 34c6b1407e8785fb55c6e330dd844f74239fccf56df4db8ad0a5de5de9986aff
                  (SHA-256 of "deploy|production-eu|OpenCoven/psyche|abc1234|modifies production")
  nonce        = 3066336331653634613962323464356538633766306131643265336234633564
  issued_at    = 2026-08-30T15:00:00.000Z
  expires_at   = 2026-08-30T15:02:00.000Z        (120 s ≤ 300 s cap)
}

canonical bytes (hex):
434f56454e2d414354494f4e2f310000000016746f6f6c5f657865637574696f6e5f617070726f766500000011
6465706c6f792e70726f64756374696f6e0000000d70726f64756374696f6e2d65750000002b4e4d6178514836
486866745678754d773359525064434f667a505674394e754b304b586558656d596176380000002b4d47597a59
7a466c4e6a52684f5749794e4751315a54686a4e3259775954466b4d6d557a596a526a4e575100000018323032
362d30382d33305431353a30303a30302e3030305a00000018323032362d30382d33305431353a30323a30302e
3030305a

intent digest = SHA-256(canonical) = 4e87a8e1cc1276cf654ddfe520694042f893e9e66dfc10b6ae608a119a2bb464
```

Digests and the nonce are shown in hex; the protocol encodes these fields as canonical base64url (`URL_SAFE_NO_PAD` in `grant.rs` — the hex and base64url forms are the same bytes). The effect digest above is `SHA-256("deploy|production-eu|OpenCoven/psyche|abc1234|modifies production")` and the nonce is the 32-byte ASCII encoding of the fixed test value, chosen for reproducibility.

The device signs those canonical bytes; the host re-derives them from the received fields and verifies. Every displayed field (operation, target, effect, nonce, expiry) is inside the digest, so display/authorize substitution is detected.

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
issued → narrowed      (scope editing; new grant id)
issued → reissued      (rotation/re-issue; new grant id, epoch preserved per device)
issued → expired       (expires_at passes; irreversible)
issued → revoked       (revocation record + epoch bump; ensure_still_active kills sessions)
revoked/expired → terminal; re-authorization requires a fresh enrollment ceremony
```

`auth.rs::ensure_still_active` already enforces the epoch/grant-id check mid-session; the migration (§11) must preserve `revocation_epoch` monotonicity per device.

### 9.3 Assurance evaluation (host)

Possession verified → scope present? → per-scope fresh-verification restriction? → presented assurance rank ≥ required? → action is transaction-bound (nonce fresh, expiry ≤ 300 s, digest matches request)? → grant still active at execution time.

## 10. Audit events

`audit.rs` today emits eight events (`GatewayStarted`, `GatewayStopped`, `PairingCreated`, `PairingCompleted`, `PairingRejected`, `DeviceRevoked`, `AuthenticationRejected`, `RateLimited`) as coarse JSONL records (0600, `O_NOFOLLOW`, 4 MiB truncation; the `audit_records_only_allowed_coarse_fields` test forbids path/memoryId/endpoint/fingerprint/nonce/signature/body fields — an invariant to preserve).

The issue requires audit events for enrollment, authentication, step-up approval, scope changes, and revocation. Proposed taxonomy extension (same coarse-field invariant, extend the forbidden-field test as new fields land):

| Event | Trigger |
| --- | --- |
| `DeviceEnrolled` | grant issued at enrollment completion (complements `PairingCompleted`) |
| `AuthenticationSucceeded` | successful `MobileAuthenticator::verify` (device id only, never path/nonce) |
| `StepUpRequested` / `StepUpApproved` / `StepUpRejected` / `StepUpExpired` | §6.4 flow outcomes |
| `GrantIssued` / `GrantScopeNarrowed` / `GrantReissued` | §5.5 |
| `DeviceRenamed` | §8.1 |
| `DeviceKeyRotated` | §5.6 |
| (existing) `DeviceRevoked` | extend to record the reason class from §8.3 |

Records stay free of private keys, biometric material, nonces, and signatures (the current test asserts exactly this class of redaction — extend it, do not weaken it).

## 11. Registry migration

`registry.rs` already carries the legacy migration shape (`LegacyDeviceRecord`/`LegacyDeviceScope` → `GrantedDeviceRecord`), and `mobile-device-trust.md` Stage B prescribes: preserve v1 `memory_read` devices by representing them as grants with exactly their existing authority — one `memory_read` scope, `minimum_assurance: possession`, audience `local_coven_authority`, no expiry — and never widening access.

Deterministic migration rule: for each legacy device record, synthesize `DeviceGrant::for_device(device_id, public_key_x963, scopes_from_record, paired_at)`; keep the same `device_id` so audit history survives. Fail closed on unknown legacy scopes (`LegacyDeviceScope` must map 1:1 or the migration aborts) — the same unknown-value-fails-closed rule the schemas require.

## 12. Self-hosted and cloudless operation

Acceptance criterion: self-hosted deployments use the credential/grant model without an OpenCoven cloud account. Current implementation satisfies the shape: the grant issuer is `DeviceGrantAudience::LocalCovenAuthority` (`grant.rs`), the registry/audit/state live under the local Coven home (`config.rs` private-file discipline: atomic replace, 0600, `O_NOFOLLOW`), and pairing runs over the local daemon socket (`mod.rs::run_pair`) and a TLS gateway on the private network. Nothing in this plan introduces a cloud dependency: the relay (#787) and any account/recovery service (#788) remain optional transports/factors that never mint authority. The plan's additions (assurance assertions, audit events, CLI verbs) are all local artifacts.

## 13. Test plan

Extends `docs/architecture/mobile-device-pairing-v1.md` "Required security tests" and Gate D/E:

Platform credential (device-side, where the platform allows):

- key generated non-exportable; export attempts fail on hardware-backed profiles;
- biometric-gated key refuses to sign without fresh verification (level 3);
- time-window key refuses to sign after the reuse window (level 2 boundary);
- PIN-only policy records `device_credential` method and never `fresh_biometric`;
- biometric enrollment change invalidates `.biometryCurrentSet`/`setInvalidatedByBiometricEnrollment` keys → re-enrollment flow.

Authority (Rust, host-side — citable paths for the test file locations):

- grant: subject binding, scope-set canonicality, time windows, assurance floor per restricted scope (existing tests in `grant.rs`; extend for the five-level vocabulary and the split familiar-memory scopes);
- transaction: canonical-byte binding for every field; operation/target charset and length limits; lifetime ≤ 300 s; nonce replay rejection at the intent layer;
- stolen-grant-without-key: replayed grant bytes without the private key fail `verify_signature` (existing path in `auth.rs`);
- revocation: mid-session revocation defeats `ensure_still_active` (epoch mismatch);
- migration: v1 registry fixture converts byte-for-byte to the expected grant set without widening scopes;
- audit: taxonomy records are coarse-fields-only (extend the existing test).

Conformance (spec level, feeding `spec/device-pairing/v1/test-vectors.json`):

- golden vectors from §7.3 (canonical action bytes + digests) locked by the shared protocol library;
- grant diagnostic JSON validating against `device-grant.schema.json` for both the four-level and five-level vocabularies during the transition;
- unknown capability/restriction/assurance values rejected.

## 14. Acceptance criteria mapping

| Issue acceptance criterion | Satisfied by |
| --- | --- |
| Biometric material never leaves the platform biometric subsystem | §4 (key-use gating), §6.1 (enforcement model), §6.3 (assertion carries no biometric data), audit redaction invariant |
| Copying a grant/token without the device private key cannot authenticate | §2.1 (already enforced: ECDSA over canonical request against enrolled public key; `GrantError::SubjectMismatch`), §5.4 |
| Revoking the phone invalidates its authority without rotating familiar/root identities | §8.3, §9.2 (epoch + `revoked_at`), registry revoke path; familiar identity is a distinct subject (`mobile-device-trust.md` credential table) |
| High-risk approvals are nonce-bound, expiring, replay-resistant, transaction-specific | §7 (`DeviceActionIntent` canonicalization, 300 s cap, 32-byte nonce, replay cache, tests) |
| Policy distinguishes biometric-only from device-credential fallback | §4.2–§4.4, §5.3 (five-level vocabulary), §6.2 (platform matrix) |
| Self-hosted deployments need no OpenCoven cloud account | §12 |

## 15. Maintainer decision points

- **D1 — Canonical capability vocabulary (dot vs snake).** Recommend dot-form as the protocol canonical (matches `spec/device-pairing/v1`), snake-form only as the Rust serde encoding, with a total mapping table in the contract layer (§5.2). Alternatives: snake everywhere (breaks published schemas), dual vocabulary without mapping (guarantees drift).
- **D2 — Add `fresh_user_verification` to the spec assurance enum.** Recommended (§5.3); the implementation and accepted architecture already carry five levels. Alternative: drop `fresh_user_verification` from Rust — loses the documented recent/fresh distinction and forces passcode fallback into the biometric tier.
- **D3 — Split `familiar_memory_admin` into read/write scopes.** Recommended (least privilege; `capabilities.json` already models both). Alternative: keep the coarse scope and rely on transaction authorization for writes — weaker and inconsistent with the capability taxonomy.
- **D4 — `coven device` command tree vs `coven memory mobile devices`.** Recommended new top-level tree with alias window (§8.1). Alternative: leave as-is and document; the issue names the `device` verbs explicitly, so deferral only moves the rename later.
- **D5 — Effect-descriptor canonicalization.** The `effect_digest` input format is unspecified (client contract). Recommended: define a canonical effect JSON (operation, targets, effect verbs, commit ids) digested with SHA-256, documented next to the action-intent vector; the golden example fixes one concrete input so vectors are reproducible.
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
| Mobile UI renders digest-covered fields | §7.2 |
| `coven device list/inspect/rename/revoke` | §8.1 |
| Grant scope editing/reissuance | §5.5 |
| Key rotation/re-enrollment | §5.6 |
| Lost-device / suspected-compromise workflow | §8.3 |
| Audit events for enrollment/auth/step-up/scope/revocation | §10 |
