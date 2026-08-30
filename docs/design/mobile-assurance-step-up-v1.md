# Mobile Step-Up Assurance Proofs (`COVEN-ASSURANCE/1`)

**Status:** plan / implementation contract for [#815](https://github.com/OpenCoven/coven/issues/815)
**Parent:** `#786` (generalized device grants) · Builds on `#791` (device grants) and `#812` (pairing v2)
**Authority owner:** Coven daemon / Rust authority layer (`crates/coven-cli/src/mobile_memory/`)
**Compatibility:** additive. Pairing v1/v2 requests without the extension are unchanged; devices without an enrolled authorization key keep exactly today's possession-only behavior.

## Purpose

The grant model can require `FreshUserVerification` or `FreshBiometric`
(`restrictions.require_fresh_user_verification_for`, `minimum_assurance` —
`crates/coven-cli/src/mobile_memory/grant.rs:73-103`), but request authentication
can never satisfy those requirements: `MobileAuthenticator::verify` presents the
constant `AssuranceLevel::Possession` to `DeviceGrant::authorize`
(`crates/coven-cli/src/mobile_memory/auth.rs:149`), because request
authentication only proves possession of the enrolled device key. Today, the
only way to express a stronger assurance level would be for a client to
*assert* it — and a client must never be able to upgrade assurance by sending
an asserted enum or string.

This document specifies an independently verifiable **step-up proof**: a fresh
signature made by a separately enrolled, platform-policy-protected
*authorization key*, over canonical bytes that bind the proof to exactly one
device, grant, request (or `DeviceActionIntent`), challenge, and time window.
The server verifies the signature, recomputes everything it checks, computes
the effective assurance itself, and passes that value to
`DeviceGrant::authorize` (`crates/coven-cli/src/mobile_memory/grant.rs:157-194`).
No biometric material ever leaves the device; OpenCoven receives signatures
only.

This plan is a design document, not an implementation PR: it defines the wire
bytes, state machines, storage, platform mapping, and portable vectors an
implementation must satisfy. No behavior change ships with this document.

## Problem statement, in current code

| Gap | Where today |
| --- | --- |
| `authorize` is always called with hardcoded `AssuranceLevel::Possession` | `crates/coven-cli/src/mobile_memory/auth.rs:149` and again in the post-response re-check at `auth.rs:174-181` |
| `minimum_assurance` can never exceed `Possession` in practice; `DeviceGrant::for_device` pins it | `crates/coven-cli/src/mobile_memory/grant.rs:119` |
| `AssuranceLevel` has no verifiable source: nothing but the server itself can mint anything above `Possession` | `crates/coven-cli/src/mobile_memory/grant.rs:50-58` |
| No second (step-up) key exists in the device record or registry schema | `crates/coven-cli/src/mobile_memory/registry.rs:26-33` |
| Replay protection exists only for ordinary request nonces, keyed `(device_id, nonce)` | `crates/coven-cli/src/mobile_memory/auth.rs:105,198-217` |
| The pairing-v2 transcript binds the device key but no authorization key | `crates/coven-cli/src/mobile_memory/pairing.rs:443-467` |

Note that pairing v2 deliberately excluded biometric *authorization* from the
transcript (`docs/design/mobile-pairing-protocol-v2.md`, "Security properties",
item 6). That statement remains true: the biometric *ceremony* never enters the
transcript. What this spec adds is binding of the *authorization public key*
and its declared assurance class — an enrollment fact — which is a different
thing from any biometric data.

## Key separation

A paired device holds two distinct, non-exportable P-256 keys:

| | Possession key | Step-up authorization key |
| --- | --- | --- |
| Identifies | the enrolled device | the authorization capability of that device |
| Enrolled via | pairing transcript (existing `devicePublicKey`, `crates/coven-cli/src/mobile_memory/pairing.rs:214-232`) | pairing transcript extension (below) |
| Unlocked for | every request (`COVEN-MEMORY/1`, `crates/coven-cli/src/mobile_memory/auth.rs:34-51`) | only sensitive actions, gated by platform policy |
| Stored as | `DeviceRecord.public_key_x963` + `grant.subject_key_id` (`registry.rs:26-33`, `grant.rs:242-250`) | separate authorization-key record (below) |
| iOS protection | none beyond secure key storage | `SecAccessControl` + LocalAuthentication policy |
| Android | Keystore, no user-authentication requirement | `BiometricPrompt` / strong-biometric policy |

Rules:

1. The two keys are distinct key pairs. A key MUST NOT serve both roles; the
   registry MUST reject an authorization key equal to the device possession key.
2. The server stores only public keys and policy metadata. It never receives
   biometric templates, hashes, or platform state, and does not infer platform
   state beyond the assurance class declared for the enrolled key.
3. The declared class is an enrollment-time property of the key, enforced by
   platform key policy on the device (see [Platform mapping](#platform-mapping)).
   The server trusts the class only because it was confirmed in the pairing
   transcript — never because a client asserted it at proof time.

## Enrollment (binding the step-up key into the pairing-v2 transcript)

Pairing v2's transcript (`COVEN-PAIR/2`, `crates/coven-cli/src/mobile_memory/pairing.rs:443-467`)
is extended with two optional, length-prefixed fields appended after
`app_version`, present exactly when the enrollment request carries them:

1. raw canonical 65-byte step-up authorization public key (same P-256 X9.63 encoding as the possession key);
2. UTF-8 declared assurance class (`biometric_only` | `user_verification` | `device_credential`).

Concretely, the closed enrollment request shape
(`crates/coven-cli/src/mobile_memory/contract.rs:237-246`, `deny_unknown_fields`)
gains one optional member:

```json
{
  "protocolVersion": 2,
  "pairingNonce": "<base64url nonce>",
  "deviceName": "Val’s iPhone",
  "devicePublicKey": "<canonical P-256 X9.63 public key>",
  "appVersion": "1.0.0",
  "supportedProtocol": { "minimum": 1, "maximum": 2 },
  "stepUpAuthorization": {
    "publicKey": "<canonical P-256 X9.63 public key>",
    "assuranceClass": "biometric_only"
  }
}
```

Rules:

- The field is optional. Absent ⇒ the transcript digest, phrase, and all v2
  behavior are byte-for-byte identical to today (v1 devices and v2 clients
  without step-up are unaffected).
- Present ⇒ the two extra fields are appended to the `COVEN-PAIR/2` transcript
  input, changing the transcript digest and therefore the six-word phrase
  (`derive_pairing_phrase`, `crates/coven-cli/src/mobile_memory/pairing.rs:542-556`).
  Because both endpoints display the phrase derived from the same host-side
  transcript, enrollment-time substitution of the step-up key by an attacker
  who only photographed the QR changes the phrase and is caught by the human
  comparison, exactly like device-key substitution.
- Older hosts reject the unknown member (`deny_unknown_fields`) and fail
  closed. That is intentional: a client that requires step-up learns the host
  does not support it instead of silently pairing without it.
- The step-up key MUST be a canonical uncompressed 65-byte P-256 X9.63 key,
  validated with the same routine as `devicePublicKey`
  (`crates/coven-cli/src/mobile_memory/pairing.rs:529-538`), and MUST differ
  from the possession key.
- The authorization-key record is persisted in the same critical step that
  registers the device and grant (the `both confirmed` branch,
  `crates/coven-cli/src/mobile_memory/pairing.rs:330-372`), so a grant is never
  issued with step-up requirements that no enrolled key can ever satisfy.

**Recommendation — enrollment-time proof of possession.** The enrollment
should additionally carry a signature by the step-up private key over
`"COVEN-STEPUP-ENROLL/1\0" || transcript_hash`, verified before the record is
persisted. Rationale: it proves the key exists in the declared policy domain
(producing the signature exercises the platform gate — on iOS, creating a
signature with a biometry-gated key triggers exactly the LocalAuthentication
ceremony the class claims) and rejects mistyped/unusable keys at pairing time
instead of at first sensitive use. Alternatives considered: (a) skip it —
simpler pairing ceremony, but a key that can never sign is only discovered
later (fail-closed, so acceptable, but degrades UX); (b) require it only for
`biometric_only` class. Recommend: require it for all classes in v1; a failed
ceremony at enrollment means the declared class does not match platform policy.

### Assurance classes and ceilings

The enrollment declares the platform policy that protects the step-up key. The
class caps what proofs from that key can ever prove (the server-side ceiling in
[effective assurance](#effective-assurance-server-side-never-client-asserted)):

| Class | Platform enforcement | Ceiling |
| --- | --- | --- |
| `biometric_only` | iOS `deviceOwnerAuthenticationWithBiometrics` on a Secure Enclave key; Android `BiometricPrompt` with `BIOMETRIC_STRONG` only | `FreshBiometric` |
| `user_verification` | iOS `deviceOwnerAuthentication` (biometric or passcode); Android `BIOMETRIC_WEAK\|DEVICE_CREDENTIAL` | `FreshUserVerification` |
| `device_credential` | PIN/pattern/password only (Android `DEVICE_CREDENTIAL` alone; iOS `kSecAccessControlDevicePasscode`) | `FreshUserVerification` |

`device_credential` is a distinct class, per the issue's Android mapping: a
fresh passcode entry proves fresh *user verification*, never fresh *biometric*.
`AssuranceLevel::RecentUserVerification` is a server-side policy concept
(platform "recently unlocked" state) with no cryptographic proof; it is not a
claimable proof class. `AssuranceLevel::StepUp` remains reserved for the
recovery/other-device flow described in
`docs/design/mobile-device-trust.md` ("Biometrics and step-up authorization")
and is never minted by `COVEN-ASSURANCE/1`.

### Storage (separate from the possession key)

Authorization-key metadata lives in its own store, deliberately not in
`devices.json` (`registry.rs:18`), so possession identity and authorization
enrollment have independent lifecycles:

`~/.coven/mobile/authorization-keys.json`

```json
{
  "version": 1,
  "keys": [
    {
      "deviceId": "00000000-0000-0000-0000-000000000001",
      "publicKeyX963": "<canonical P-256 X9.63>",
      "subjectKeyId": "<base64url SHA-256 over the step-up public key>",
      "assuranceClass": "biometric_only",
      "enrolledAt": "2026-07-29T12:00:00.000Z",
      "revokedAt": null,
      "keyEpoch": 1
    }
  ]
}
```

- Written with the same private, atomic-replace discipline as the device
  registry (`registry.rs:13` re-exports `config::atomic_replace_private`;
  `validate_private_file` on read — `registry.rs:353-407` shows the pattern).
- At most one active (non-revoked) key per device; `subjectKeyId` reuses the
  grant's key-id convention — base64url SHA-256 over the canonical public key
  (`grant.rs:242-250`).
- The subject/possession key and its `subject_key_id` stay exactly where they
  are (`DeviceRecord`, `DeviceGrant`), satisfying "store authorization-key
  metadata separately from the device subject/possession key".
- Forgetting or revoking a device (`registry.rs:revoke`, and the
  `--forget-devices` path, `mod.rs:65-88`) cascades to its authorization key.

## Canonical proof bytes (COVEN-ASSURANCE/1)

Framing follows the repo's canonical-byte conventions: a versioned ASCII domain
terminated by NUL (as `COVEN-ACTION/1\0`, `grant.rs:270`), then each field
framed as an unsigned 32-bit big-endian length followed by its bytes, exactly
like `DeviceActionIntent::canonical_bytes` (`grant.rs:266-284`) and
`update_length_prefixed` (`pairing.rs:503-506`).

```text
"COVEN-ASSURANCE/1\0"
u32(len) || bytes for each field, in order:
  1. device_id            — raw 16-byte UUID
  2. grant_id             — raw 16-byte UUID (DeviceGrant::id, v5-derived,
                            grant.rs:114)
  3. revocation_epoch     — unsigned 64-bit big-endian
  4. authorization_key_id — UTF-8 base64url(SHA-256(step-up public key)),
                            same derivation as grant.rs subject_key_id
  5. context_mode         — ASCII "request" or "action"
  6. context_digest       — raw 32-byte SHA-256 (defined below)
  7. challenge            — raw 32-byte server-issued challenge
  8. issued_at            — ASCII RFC 3339 UTC, millisecond precision
  9. expires_at           — RFC 3339 UTC, same encoding
 10. requested_assurance  — ASCII "fresh_user_verification" or "fresh_biometric"
```

The step-up key signs exactly these bytes with ECDSA P-256 over SHA-256,
DER-encoded, base64url — the same signature encoding the possession path
verifies (`auth.rs:239-260`, `Signature::from_der`).

### Context digest (server-recomputed, never client-asserted)

`context_digest = SHA-256(canonical_context_bytes)` where the server computes
the bytes itself — the client never sends a digest to trust:

- **`request` mode** — the exact `COVEN-MEMORY/1` canonical request bytes the
  possession key signed for this same request:
  `canonical_request(method, path_and_query, timestamp, nonce, body_digest)`
  (`crates/coven-cli/src/mobile_memory/auth.rs:34-51`). The proof therefore
  covers byte-for-byte the same request the possession signature covers; there
  is no gap in which one can be swapped.
- **`action` mode** — `DeviceActionIntent::canonical_bytes()`
  (`crates/coven-cli/src/mobile_memory/grant.rs:266-284`, `COVEN-ACTION/1`),
  recomputed by the server from the submitted intent. The intent already binds
  scope, operation, target, effect digest, nonce, and its own window.

The `context_mode` field makes the two domains non-substitutable.

### Validity

- `expires_at - issued_at ≤ 120` seconds (recommended default 60; the vector
  below uses 60). For `action` mode, additionally
  `proof.expires_at ≤ intent.expires_at` — the proof window is nested inside
  the intent window (intent lifetime is capped at 300 s,
  `grant.rs:12`).
- Server clock tolerance: none beyond the checks themselves; `issued_at ≤ now ≤
  expires_at` with `issued_at` within the challenge's own validity window.

## Challenge issuance and replay protection

A proof MUST cover a server-issued, single-use challenge:

- **Issuance.** New possession-authenticated mobile route
  `POST /api/v1/mobile/assurance/challenge` (protected exactly like today's
  routes: `x-coven-protocol: 1` + `COVEN-MEMORY/1` headers,
  `crates/coven-cli/src/mobile_memory/gateway.rs:677-717`). Response envelope
  carries `{ "challenge": <base64url 32B>, "expiresAt": <RFC 3339> }`.
- **Binding.** The stored record binds `device_id`, `grant_id`,
  `revocation_epoch`, `expires_at = issued + ≤120 s`, and `spent = false`.
  Grant rotation or revocation immediately invalidates outstanding challenges.
- **Consumption.** Verification atomically flips `spent` under the store lock
  before returning success (same single-winner pattern as
  `auth.rs::insert_nonce`, `auth.rs:198-217`, including the bounded-map
  discipline). A failed signature does not spend the challenge; a successful
  one does. Two concurrent submissions of the same proof: exactly one wins.
- **Storage.** `~/.coven/mobile/assurance-challenges.json` — separate from the
  request-nonce replay cache in `MobileAuthenticator` (`auth.rs:105`), which
  keys `(device_id, request_nonce)` and serves `COVEN-MEMORY/1` replay
  protection. Challenge state is persisted (not just in-memory) so a daemon
  restart cannot resurrect a spent challenge inside a live proof window.
- **Why a server challenge.** The threat model lists "attacker with temporary
  access to an unlocked endpoint"
  (`docs/security/mobile-device-pairing-threat-model.md`). A server challenge
  bounds pre-minting to one proof per challenge with a ≤120 s horizon — proofs
  cannot be banked offline in bulk while the phone is unlocked. Alternative
  considered: client-generated nonce + server replay cache (the
  `insert_nonce` pattern). Rejected as the default: it permits offline
  pre-minting of unlimited proofs while the device is unlocked. It remains a
  viable fallback if a zero-round-trip flow is ever required; if adopted, its
  replay store MUST still be a separate cache from the request-nonce cache.

### Verification procedure (normative order)

Given a possession-authenticated request carrying step-up proof headers:

1. **Possession first.** The ordinary `COVEN-MEMORY/1` verification must have
   succeeded (`gateway.rs:697-717`). A step-up proof is never evaluated for an
   unauthenticated or revoked device.
2. **Load the enrolled authorization key** for `device_id` from the
   authorization-key store. Absent or revoked → possession-only (or fail
   closed, step 8).
3. **Challenge check.** Look up the presented challenge: must exist, belong to
   this `device_id` and the grant's current `revocation_epoch`, be unspent, and
   be unexpired. Spent/expired/unknown → proof invalid.
4. **Recompute the context digest** from the actual request bytes
   (`canonical_request`, `auth.rs:34-51`) or the submitted
   `DeviceActionIntent` (`grant.rs:266`). Never trust a client-supplied digest.
5. **Rebuild the canonical bytes** from: registry device id, grant id and
   `revocation_epoch` (`registry.rs authorization_record`), the enrolled
   `authorization_key_id`, the presented mode/`issued_at`/`expires_at`/
   `requested_assurance`, and the server-recomputed values above.
6. **Verify the signature** against the enrolled step-up public key (DER
   P-256, `verify_signature` pattern, `auth.rs:239-260`).
7. **Compute effective assurance** (below) and pass it to
   `DeviceGrant::authorize` (`grant.rs:157-194`).
8. **Fail closed:** any failure → effective assurance is `Possession`. The
   grant's own policy then decides: if the requested scope requires stronger
   assurance (`require_fresh_user_verification_for` or `minimum_assurance`,
   `grant.rs:171-192`), `authorize` returns `GrantError::AssuranceRequired`
   and the request is rejected — it is not silently downgraded to a weaker
   success.

### Effective assurance (server-side, never client-asserted)

```text
effective = Possession                                  # default
if the proof verifies end-to-end:
    ceiling  = class_ceiling(enrolled_key.assurance_class)
    requested = parse(requested_assurance)               # claim in the signed bytes
    effective = min(requested, ceiling)                  # server caps the claim
# then, exactly as today:
DeviceGrant::authorize(required_scope, effective, now)   # grant.rs:157-194
```

- The client's requested level is inside the signature, so relabeling it after
  the fact is a signature failure; the server caps it by the enrolled key's
  declared class, so even a valid signature cannot mint a class the key's
  platform policy does not support ("possession proof cannot be relabeled as
  biometric proof" — a possession-key signature never verifies under the
  step-up public key, and `COVEN-MEMORY/1` bytes are not `COVEN-ASSURANCE/1`
  bytes).
- The existing `Ord` on `AssuranceLevel` (`grant.rs:50-58`) already gives the
  right lattice: a `FreshBiometric` proof satisfies a
  `FreshUserVerification` requirement.
- `RecentUserVerification` is not cryptographically provable (no ceremony to
  sign) and stays a server-side policy notion, out of scope for proofs.
- `ensure_still_active` re-checks must reuse the same effective assurance
  value for the request (the current re-check passes `Possession`,
  `auth.rs:174-181`; with step-up it must not fail a legitimately
  step-up-authorized request). `VerifiedMobileDevice`
  (`auth.rs:96-101`) gains an `effective_assurance` field for that purpose.

### Transport (wire shape)

Six flat headers, mirroring the existing `x-coven-*` convention
(`gateway.rs:917-929`):

| Header | Value |
| --- | --- |
| `x-coven-assurance-context` | `request` \| `action` |
| `x-coven-assurance-challenge` | base64url of the 32-byte challenge |
| `x-coven-assurance-issued-at` | RFC 3339 UTC, millis |
| `x-coven-assurance-expires-at` | RFC 3339 UTC, millis |
| `x-coven-assurance-level` | `fresh_user_verification` \| `fresh_biometric` |
| `x-coven-assurance-signature` | base64url DER ECDSA |

For `action` mode the same headers ride on the request that submits the
`DeviceActionIntent`; the server hashes the intent from that request body. The
action-submission route itself is the #786 exact-action work and is out of
scope here; the proof contract is independent of which route carries it.

## Rotation and revocation

Rotation and revocation of the authorization key never change familiar or root
identity (identity separation table, `docs/design/mobile-device-trust.md`,
"Identity and credential separation"):

- **Rotate** — enroll a replacement key for the device with a new `keyEpoch`
  (same transcript-bound ceremony as initial enrollment, plus proof of
  possession of the *old* step-up key or fresh possession-key authentication
  per owner policy). Exactly one active key per device; the previous record is
  retained with `revokedAt` for audit, and outstanding challenges are
  invalidated.
- **Revoke the step-up key** — device falls back to possession-only; grants
  requiring more fail closed. Does not revoke the device.
- **Revoke the device** — `registry.revoke` (`registry.rs:235-257`) cascades:
  possession and step-up both die; the revocation epoch bump invalidates
  outstanding challenges.
- **Compromise semantics** — possession-key compromise: revoke the device.
  Step-up-key compromise: revoke the key (and re-enroll); a relay/account
  compromise mints nothing — assurance requires a signature from a
  policy-protected key the attacker never holds.

## State machines

```text
Authorization key:  absent → enrolled → rotated (epoch+1) → …
                          ↘ revoked (per-key or device cascade)

Challenge:          issued ── verified+consumed (atomic) ──▶ spent
                       │ expires_at passed
                       ▼
                       expired (pruned opportunistically, bounded store)

Proof verification: possession OK
  → load key → check challenge → recompute digest → rebuild bytes
  → verify signature → check window/lifetime
  → effective = min(claimed, class ceiling)
  → DeviceGrant::authorize(required_scope, effective, now)
  any failure ⇒ effective = Possession; grant policy then decides
  (AssuranceRequired ⇒ reject; otherwise proceed)
```

## Platform mapping

### iOS

- Possession key: Secure Enclave P-256 (`kSecAttrTokenIDSecureEnclave`), no
  per-request biometric prompt — keeps reconnect frictionless.
- Step-up key: separate Secure Enclave P-256 key with
  `SecAccessControl` `.privateKeyUsage` plus the policy for its class:
  `.biometryCurrentSet` (+ `LAContext` `deviceOwnerAuthenticationWithBiometrics`
  for the biometric-only ceremony when policy demands biometric rather than
  passcode fallback), or `.devicePasscode` for the device-credential class.
- Signature algorithm: the X9.62 message-signature member of the
  `kSecKeyAlgorithm` family, `ECDSASignatureMessageX962SHA256` (the two halves
  concatenate into the full constant) — DER output, matching the server's
  `Signature::from_der` path (`auth.rs:252-259`).

### Android

- Hardware-backed Keystore P-256 where available;
  `setUserAuthenticationRequired(true)` and
  `setUserAuthenticationParameters(...)` / `setUserAuthenticationParameters(…, AUTH_BIOMETRIC_STRONG)` for the step-up key;
  `setInvalidatedByBiometricEnrollment(true)` to keep "current biometry" honest.
- `BiometricPrompt` with `BIOMETRIC_STRONG` authenticators for
  `biometric_only`; `DEVICE_CREDENTIAL`-only flows enroll as the separate
  `device_credential` class — never labeled `FreshBiometric`.
- `Signature.getInstance("SHA256withECDSA")` produces DER — same wire format.

Both platforms keep the possession key prompt-free and the step-up key
prompt-gated; only signatures cross the trust boundary.

## Security invariants → mechanism

| Invariant (issue) | Mechanism |
| --- | --- |
| Biometric material never leaves the OS subsystem | Only P-256 signatures transit; no biometric field exists anywhere in the protocol |
| Possession proof cannot be relabeled as biometric proof | Server computes effective assurance from a verified step-up signature; possession key ≠ step-up key; `COVEN-MEMORY/1` / `COVEN-ACTION/1` / `COVEN-ASSURANCE/1` domains are disjoint |
| Proof for action A cannot authorize action B | `context_digest` covers the exact canonical request/intent bytes, recomputed server-side |
| Proof for device/grant A cannot authorize device/grant B | `device_id` + `grant_id` (+ `revocation_epoch`) inside the signed bytes |
| Relay/account compromise cannot mint fresh-biometric assurance | Assurance requires the enrolled step-up private key; relays never hold it |
| Replayed proofs fail closed | Single-use server-issued challenge, atomically consumed; ≤120 s window; independent of request nonces |
| Self-hosted/unattested clients remain possible | Step-up is optional per grant/owner policy; possession always remains a valid baseline (`DeviceGrantRestrictions` defaults, `grant.rs:80-87`) |

## Portable golden vector

Synthetic, no live credential — same convention as
`crates/coven-cli/tests/fixtures/mobile-pairing-v2/transcript-vector.json` and
`crates/coven-cli/tests/fixtures/mobile-memory-v1/signature-vector.json`: every
byte string is documented as hex (the wire encodes challenges, digests, and
signatures as unpadded base64url; the vector stores raw bytes so any
implementation can reproduce them). An implementation PR adds this as
`crates/coven-cli/tests/fixtures/mobile-assurance-v1/assurance-vector.json`;
Swift/Android implementations must reproduce `canonicalProofBytesHex` exactly.
ECDSA P-256 signatures are randomized (`k` is per-signature; neither Secure
Enclave nor Android Keystore exposes deterministic RFC 6979 signing), so
implementations are not expected to reproduce `signatureDERHex` byte-for-byte:
they must **verify** it over `canonicalProofBytesHex` with
`stepUpPublicKeyX963Hex`, and their own signatures must verify the same way.

```json
{
  "fixtureNotice": "SYNTHETIC TEST KEY — NOT A CREDENTIAL",
  "deviceId": "00000000-0000-0000-0000-000000000001",
  "grantId": "a67b9d68-b8c8-5a84-923f-3158b93ee261",
  "revocationEpoch": 0,
  "stepUpPrivateKeyScalarHex": "0202020202020202020202020202020202020202020202020202020202020202",
  "stepUpPublicKeyX963Hex": "04550f471003f3df97c3df506ac797f6721fb1a1fb7b8f6f83d224498a65c88e24136093d7012e509a73715cbd0b00a3cc0ff4b5c01b3ffa196ab1fb327036b8e6",
  "authorizationKeyIdHex": "fe00ab0f341901f863a49160cf554588d6928282d531b799addc4123f45ce85a",
  "contextMode": "request",
  "protectedCanonicalRequestHex": "434f56454e2d4d454d4f52592f310a4745540a2f6170692f76312f6d6f62696c652f6d656d6f72792f6f766572766965770a313738353332363430300a414141414141414141414141414141414141414141414141414141414141414141414141414141414141410a3437444551706a38484253612d5f54496d572d354a4365755165526b6d354e4d704a575a47336853754655",
  "contextDigestHex": "dde33200a4ad41fa4d11d7f81713f74cdf0ce3971d6a5e5003b43fa789bdc12f",
  "challengeHex": "0909090909090909090909090909090909090909090909090909090909090909",
  "issuedAt": "2026-07-29T12:00:00.000Z",
  "expiresAt": "2026-07-29T12:01:00.000Z",
  "requestedAssurance": "fresh_biometric",
  "canonicalProofBytesHex": "434f56454e2d4153535552414e43452f3100000000100000000000000000000000000000000100000010a67b9d68b8c85a84923f3158b93ee2610000000800000000000000000000002b5f674372447a515a4166686a704a46677a315646694e6153676f4c564d62655a72647842495f526336466f000000077265717565737400000020dde33200a4ad41fa4d11d7f81713f74cdf0ce3971d6a5e5003b43fa789bdc12f00000020090909090909090909090909090909090909090909090909090909090909090900000018323032362d30372d32395431323a30303a30302e3030305a00000018323032362d30372d32395431323a30313a30302e3030305a0000000f66726573685f62696f6d6574726963",
  "signatureDERHex": "3044022035a34c02382512c29d05de88ceaff21b2141d60b592bc4ab2cc511bad976ab3702202f28af09e7c0343606682f927f349e7abf0d31b94346a8b73f0d88a7f4cb2c0c"
}
```

`protectedCanonicalRequestHex` decodes to the `COVEN-MEMORY/1` canonical
request from the existing memory-v1 fixture
(`GET /api/v1/mobile/memory/overview`, timestamp `1785326400`, the all-zero
nonce, and the SHA-256 body digest of the empty payload); its SHA-256 is
`contextDigestHex`. Notes for implementers: `grantId` is
`Uuid::new_v5(device_id, "coven-device-grant-v1")` (`grant.rs:114`); the
possession key of the protecting device is the existing memory-v1 vector key
(`signature-vector.json`, scalar `0x0101…`); the step-up key scalar is
`0x02`-repeated (the Rust tests' `public_key(seed)` convention). The signature
is DER-encoded ECDSA/P-256/SHA-256 over the exact `canonicalProofBytesHex`.

### Schemas

```typescript
// Client-facing assurance classes (declared at enrollment; platform policy).
type AssuranceClass = "biometric_only" | "user_verification" | "device_credential";

// Claim inside the signed bytes; server caps it by class ceiling.
type RequestedAssurance = "fresh_user_verification" | "fresh_biometric";

type AssuranceContextMode = "request" | "action";

interface StepUpAuthorizationEnrollment {   // optional MobilePairingRequest member
  publicKey: string;          // canonical P-256 X9.63, base64url
  assuranceClass: AssuranceClass;
  enrollmentSignature?: string; // base64url DER over "COVEN-STEPUP-ENROLL/1" || transcript hash
}

interface AssuranceProofHeaders {
  "x-coven-assurance-context": AssuranceContextMode;
  "x-coven-assurance-challenge": string;  // base64url 32B, server-issued
  "x-coven-assurance-issued-at": string;  // RFC 3339 UTC, millis
  "x-coven-assurance-expires-at": string; // RFC 3339 UTC, millis
  "x-coven-assurance-level": RequestedAssurance;
  "x-coven-assurance-signature": string;  // base64url DER
}

interface AssuranceChallenge {
  challenge: string;    // base64url 32B
  expiresAt: string;    // RFC 3339
}

interface DeviceAuthorizationKeyRecord {
  deviceId: string;           // UUID v4
  publicKeyX963: string;      // canonical P-256 X9.63, base64url
  subjectKeyId: string;       // base64url SHA-256 over publicKeyX963
  assuranceClass: AssuranceClass;
  enrolledAt: string;         // RFC 3339
  revokedAt: string | null;
  keyEpoch: number;           // monotonic per device
}
```

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "COVEN-ASSURANCE/1 golden vector",
  "type": "object",
  "additionalProperties": false,
  "required": ["fixtureNotice", "deviceId", "grantId", "revocationEpoch",
               "stepUpPrivateKeyScalarHex", "stepUpPublicKeyX963Hex",
               "authorizationKeyIdHex", "contextMode",
               "protectedCanonicalRequestHex", "contextDigestHex",
               "challengeHex", "issuedAt", "expiresAt", "requestedAssurance",
               "canonicalProofBytesHex", "signatureDERHex"],
  "properties": {
    "fixtureNotice": { "type": "string" },
    "deviceId": { "type": "string", "format": "uuid" },
    "grantId": { "type": "string", "format": "uuid" },
    "revocationEpoch": { "type": "integer", "minimum": 0 },
    "stepUpPrivateKeyScalarHex": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
    "stepUpPublicKeyX963Hex": { "type": "string", "pattern": "^04[0-9a-f]{128}$" },
    "authorizationKeyIdHex": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
    "contextMode": { "enum": ["request", "action"] },
    "protectedCanonicalRequestHex": { "type": "string", "pattern": "^[0-9a-f]+$" },
    "contextDigestHex": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
    "challengeHex": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
    "issuedAt": { "type": "string", "format": "date-time" },
    "expiresAt": { "type": "string", "format": "date-time" },
    "requestedAssurance": { "enum": ["fresh_user_verification", "fresh_biometric"] },
    "canonicalProofBytesHex": { "type": "string", "pattern": "^[0-9a-f]+$" },
    "signatureDERHex": { "type": "string", "pattern": "^30[0-9a-f]+$" }
  }
}
```

## Requirement checklist (issue → section)

| Issue requirement | Section |
| --- | --- |
| Bind optional step-up key + declared class into pairing-v2 transcript before grant issuance | [Enrollment](#enrollment-binding-the-step-up-key-into-the-pairing-v2-transcript) |
| Store authorization-key metadata separately from possession key | [Storage](#storage-separate-from-the-possession-key) (separate `authorization-keys.json`) |
| Canonical `COVEN-ASSURANCE/1` proof bytes | [Canonical proof bytes](#canonical-proof-bytes-coven-assurance1) |
| Verify signatures against enrolled key; never trust client assurance | [Verification procedure](#verification-procedure-normative-order), [Effective assurance](#effective-assurance-server-side-never-client-asserted) |
| Short validity + replay protection independent of request nonces | [Validity](#validity), [Challenge issuance](#challenge-issuance-and-replay-protection) |
| Effective assurance computed server-side, passed to `DeviceGrant::authorize` | [Effective assurance](#effective-assurance-server-side-never-client-asserted) |
| Absent/invalid/expired/replayed → possession-only or fail closed | [Verification procedure](#verification-procedure-normative-order) step 8 |
| Key rotation/revocation without touching familiar/root identity | [Rotation and revocation](#rotation-and-revocation) |
| Portable vectors for Swift/Android | [Portable golden vector](#portable-golden-vector) |
| Acceptance: `FreshBiometric` grant succeeds only with fresh signature from the enrolled biometric-policy key over the exact context | Verification steps 2–7 + the `biometric_only` ceiling mapping |

## Recommendations and alternatives considered

| Decision | Recommendation | Alternatives considered |
| --- | --- | --- |
| Transcript extension shape | Append optional fields to the `COVEN-PAIR/2` transcript when present | Bump to a `COVEN-PAIR/3` domain: cleaner versioning, but forks the phrase derivation for no security gain; the conditional fields are backward compatible (digest unchanged when absent) and both endpoints always agree on the request they hold |
| Server-issued challenge vs client nonce | Server-issued challenge (required) | Client nonce + replay cache: no new route/round trip, but permits bulk pre-minting while a device is unlocked; keep as documented fallback |
| Storage of authorization keys | Separate `authorization-keys.json` | Registry v3 with a nested field: entangles rotation with device-record migrations (`registry.rs:19` is at version 2 today) for no benefit |
| Error surfacing | Add `AssuranceRequired` to `MobileErrorCode` (`contract.rs:53-74`); today `auth.rs:150` maps every `authorize` failure to `DeviceRevoked`, which misreports assurance failures as revocation | Keep mapping into `DeviceRevoked`: breaks clients' ability to prompt for step-up |
| Enrollment-time possession proof of the step-up key | Require a signature over the transcript hash at enrollment (exercises the declared platform gate once, at pairing) | Skip it: pairing stays prompt-free, but an unusable/mismatched-policy key is discovered only at first sensitive use (still fail-closed) |
| New audit events | `StepUpVerified` / `StepUpRejected` in `MobileAuditEvent` (`audit.rs:19-28`) | Reuse `AuthenticationRejected`: loses the distinction operators need to tune step-up friction |
| Implementation home | `crates/coven-cli/src/mobile_memory/assurance.rs` alongside `auth.rs`/`grant.rs` | A new crate: premature until the #787 relay session work forces extraction (`docs/design/mobile-device-trust.md`, "Authority boundary") |

## Implementation plan (follow-up PRs)

1. `assurance.rs` — canonical bytes, challenge store, verification, effective
   assurance; adversarial tests (tamper every field, replay, cross-device,
   cross-grant, expired, absent-key, possession-key-as-step-up).
2. Pairing extension + authorization-key store + cascade revocation.
3. Gateway plumbing: headers, challenge route, error codes, audit events,
   `ensure_still_active` effective-assurance reuse.
4. Golden-vector fixture + conformance test; platform notes validated against
   the iOS/Android mappings above.

Every implementation PR in this track passes the repository gates (`cargo fmt
--check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace --locked`, secret scan, privacy guard — see
`AGENTS.md`) and may not weaken existing v1 privacy, replay, revocation,
canonicalization, or audit guarantees.
