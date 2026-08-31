# Mobile Recovery, Trusted-Device Introduction, and Optional Attestation Plan

**Status:** proposed plan and implementation contract for the `#788` scope of the `#784`–`#788` mobile connection track
**Parent architecture:** [`mobile-device-trust.md`](mobile-device-trust.md) (accepted, `#784`)
**Builds on:** `#786` (device-bound credentials, grants, assurance), `#787` (reconnection, discovery, relay)
**Delivery slot:** PR 7 — "Recovery and trusted-device introduction" in [`../architecture/mobile-device-pairing-delivery-plan.md`](../architecture/mobile-device-pairing-delivery-plan.md)
**Threat model base:** [`../security/mobile-device-pairing-threat-model.md`](../security/mobile-device-pairing-threat-model.md)
**Authority owner:** Coven daemon / Rust authority layer

## 1. Purpose and scope

This plan defines the concrete contracts for the `#788` issue:

1. **Trusted-device introduction** — an already trusted device authorizes a new installation with a fresh step-up verification and a signed enrollment transcript.
2. **Passkey recovery** — optional, account-backed recovery and remote-enrollment paths that never make a cloud account, synced passkey, Apple/Google ecosystem, or platform attestation the canonical OpenCoven/familiar identity.
3. **Optional attestation** — app/hardware assurance represented as policy attributes (`verified_official_app`, `verified_hardware_key`, `unattested_device`) that increase assurance without breaking the open/self-hosted trust model.
4. **Recovery as a protocol** — explicit, auditable ceremonies that keep *recovering access* strictly separate from *replacing or rotating a familiar's identity*.

Non-goals: implementing WebAuthn servers, Apple App Attest, or Android attestation verification in this repository; hosting an account service; changing the pairing v1/v2 QR enrollment path; widening any v1 grant.

## 2. Current implementation baseline

Claims about the present system cite the code:

| Fact | Path |
| --- | --- |
| Authority layer (pairing, auth, registry, gateway, audit) | `crates/coven-cli/src/mobile_memory/` (`pairing.rs`, `auth.rs`, `registry.rs`, `gateway.rs`, `audit.rs`) |
| Grant model: `DeviceScope` (12 capabilities), `AssuranceLevel` (`possession`, `recent_user_verification`, `fresh_user_verification`, `fresh_biometric`, `step_up`), `DeviceGrant` with `subject_key_id`, audience, restrictions, `revocation_epoch` | `crates/coven-cli/src/mobile_memory/grant.rs` |
| Exact-action authorization: `DeviceActionIntent` canonicalizes scope, operation, target, effect digest, nonce, and a ≤300 s window over `COVEN-ACTION/1` | `crates/coven-cli/src/mobile_memory/grant.rs` (`canonical_bytes`) |
| Device records: `DeviceRecord` + per-device `DeviceGrant`, atomic replacement, bounded at 128 records, legacy v1 migration | `crates/coven-cli/src/mobile_memory/registry.rs` |
| Request authentication: `COVEN-MEMORY/1` canonical string (method, path+query, timestamp, nonce, body digest), P-256 ECDSA, ±300 s window, replay cache (10 000 entries), 120 requests/window rate limit | `crates/coven-cli/src/mobile_memory/auth.rs` |
| Pairing: single-use nonce, `COVEN-PAIR/2` transcript domain, transcript-derived six-word phrase, idempotent completion | `crates/coven-cli/src/mobile_memory/pairing.rs` |
| Audit: append-only JSONL `audit.jsonl` with `MobileAuditEvent` (`PairingCreated`, `PairingCompleted`, `PairingRejected`, `DeviceRevoked`, `AuthenticationRejected`, `RateLimited`, …) | `crates/coven-cli/src/mobile_memory/audit.rs` |
| CLI surface: `coven memory mobile enable|disable|status|pair|devices [revoke <id>]` | `crates/coven-cli/src/main.rs`, `crates/coven-cli/src/mobile_memory/mod.rs` |
| Gateway routes under `/api/v1/mobile/*`, private-network HTTPS-only bind | `crates/coven-cli/src/mobile_memory/gateway.rs`, `config.rs` |
| Diagnostic JSON schemas for offer/grant/enrollment/transaction/revocation objects | `spec/device-pairing/v1/*.schema.json` |
| Capability and assurance vocabularies | `spec/device-pairing/v1/capabilities.json` |
| Domain-separation label registry | `spec/device-pairing/v1/domain-separation.md` |
| Rendezvous relay (bounded, opaque) | `crates/coven-relay/src/ws.rs` |
| Familiar identity (manifest, resolver, effective familiar) | `crates/coven-cli/src/familiar_identity.rs`, `docs/familiars/identity.md` |

What does **not** exist yet, and what this plan adds: trusted-device introduction objects, a passkey/account factor contract, recovery ceremonies and events, identity-rotation semantics, and attestation assurance attributes. The accepted architecture already sketches these in [`mobile-device-trust.md`](mobile-device-trust.md) §"Passkeys, trusted-device introduction, and recovery", §"Optional attestation", and migration Stage D; this document turns those paragraphs into normative objects, state machines, policies, and examples.

## 3. Protocol objects

New objects are added to the pairing protocol family at object version 1. Their canonical encoding is deterministic CBOR in a COSE envelope, exactly like the v1 objects (`spec/device-pairing/v1/conformance-manifest.json`); the JSON below is the diagnostic form, in the style of `spec/device-pairing/v1/device-grant.schema.json`. The implementation PR lands these schemas as files under `spec/device-pairing/v2/` and registers the new objects in a v2 conformance manifest.

New domain-separation labels (to be appended to `spec/device-pairing/v1/domain-separation.md`; code-level short forms follow the `COVEN-PAIR/2` convention in `pairing.rs`):

```text
OpenCoven/IntroductionRequest/v1     (short form: COVEN-INTRO-REQ/1)
OpenCoven/IntroductionApproval/v1    (short form: COVEN-INTRO-APPR/1)
OpenCoven/IntroductionTranscript/v1  (short form: COVEN-INTRO-TX/1)
OpenCoven/RecoveryPolicy/v1          (short form: COVEN-RECOVERY-POLICY/1)
OpenCoven/RecoveryEvent/v1           (short form: COVEN-RECOVERY-EVENT/1)
OpenCoven/AttestationClaim/v1        (short form: COVEN-ATTEST-CLAIM/1)
```

Shared `$defs` reused from the v1 schemas: `keyReference` (algorithm `Ed25519 | P-256`, `keyId`, `publicKey`), `identityReference` (type `owner | installation | device | trust-domain`), integer timestamps.

Fresh step-up evidence reuses the `COVEN-ASSURANCE/1` proof contract from [`mobile-assurance-step-up-v1.md`](mobile-assurance-step-up-v1.md) (#815, #871): a signature by a separately enrolled, platform-policy-protected **step-up authorization key** over canonical bytes binding the device, its current grant and revocation epoch, the exact context digest, and a server-issued single-use challenge; the authority computes effective assurance server-side. This plan adds one context mode, `introduction`, whose context digest is the introduction transcript hash; every other rule of that contract (challenge issuance and atomic consumption, the ≤120 s window, class ceilings, fail-closed verification order) applies unchanged. Assurance proof requirements here are kept consistent with the #786 credentials plan, which applies the same rule — proofs bound to the transcript, the current grant/epoch, and a server-issued single-use challenge, with assurance computed server-side — to its step-up-gated operations.

### 3.1 IntroductionRequest

Created by the **new endpoint**, delivered over the authenticated E2EE channel (#787) or shown as a follow-on request to an existing trusted device. It commits to everything the approval will be about.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://opencoven.ai/spec/device-pairing/v2/introduction-request.schema.json",
  "title": "OpenCoven IntroductionRequest v1 diagnostic JSON representation",
  "description": "Request from a new endpoint asking an existing trusted device or the owner to introduce it into one trust domain. Canonical signed form is deterministic CBOR in COSE_Sign1.",
  "type": "object",
  "additionalProperties": false,
  "required": [
    "version", "trustDomain", "installationFingerprint", "endpointKey",
    "pairwiseDeviceId", "requestedScopes", "requestedAssurance",
    "deviceContext", "nonce", "expiresAt", "signature"
  ],
  "properties": {
    "version": { "const": 1 },
    "trustDomain": { "type": "string", "minLength": 8, "maxLength": 256 },
    "installationFingerprint": {
      "description": "SHA-256 over the target installation's canonical host key, base64url; binds the request to one installation.",
      "type": "string", "minLength": 43, "maxLength": 43,
      "pattern": "^[A-Za-z0-9_-]{43}$"
    },
    "endpointKey": { "$ref": "#/$defs/keyReference" },
    "pairwiseDeviceId": {
      "description": "Domain-separated identifier derived per (endpoint key, trust domain); never a global device ID.",
      "type": "string", "minLength": 8, "maxLength": 128,
      "pattern": "^[A-Za-z0-9._:-]+$"
    },
    "requestedScopes": {
      "type": "array", "uniqueItems": true, "minItems": 1,
      "items": { "enum": [
        "sessions.metadata.read", "conversations.read", "messages.send",
        "tools.request", "tools.approve", "secrets.read",
        "memory.familiar.read", "memory.familiar.write",
        "identity.admin", "devices.enroll", "devices.revoke",
        "identity.export", "memory.export"
      ] }
    },
    "requestedAssurance": {
      "enum": ["possession", "recent_user_verification", "fresh_user_verification", "fresh_biometric", "step_up"]
    },
    "deviceContext": {
      "type": "object",
      "additionalProperties": false,
      "required": ["platform", "humanReadableName"],
      "properties": {
        "platform": { "enum": ["ios", "android", "macos", "linux", "windows", "other"] },
        "humanReadableName": { "type": "string", "minLength": 1, "maxLength": 80 },
        "appVariant": {
          "description": "Free-text build context such as 'official build' or 'self-built'; never an authority input by itself.",
          "type": "string", "maxLength": 80
        }
      }
    },
    "nonce": {
      "description": "Fresh 32-byte random value, base64url; single-use.",
      "type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"
    },
    "expiresAt": { "type": "integer", "minimum": 0 },
    "signature": {
      "description": "endpointKey signature over OpenCoven/IntroductionRequest/v1 canonical bytes.",
      "type": "string", "pattern": "^[A-Za-z0-9_-]+$"
    }
  },
  "$defs": {
    "keyReference": {
      "type": "object",
      "additionalProperties": false,
      "required": ["algorithm", "keyId", "publicKey"],
      "properties": {
        "algorithm": { "enum": ["Ed25519", "P-256"] },
        "keyId": { "type": "string", "minLength": 8, "maxLength": 128, "pattern": "^[A-Za-z0-9._:-]+$" },
        "publicKey": { "type": "string", "minLength": 43, "maxLength": 128, "pattern": "^[A-Za-z0-9_-]+$" }
      }
    }
  }
}
```

Privacy constraints (extend `spec/device-pairing/v1/privacy.md` rule 8): `deviceContext` must not carry hardware serials, advertising IDs, account-provider identifiers, phone numbers, or biometric metadata. `humanReadableName` is display-only and capped at 80 characters, matching `MAX_DEVICE_NAME_CHARS` in `registry.rs`.

### 3.2 IntroductionApproval

Created by each **approving device** (or the owner root credential) after a fresh step-up verification. Each approval is independently verifiable; the installation authority counts them against policy.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://opencoven.ai/spec/device-pairing/v2/introduction-approval.schema.json",
  "title": "OpenCoven IntroductionApproval v1 diagnostic JSON representation",
  "description": "One approval of an IntroductionRequest, signed by an existing trusted device key or the owner root credential. Fresh user verification is proven by an attached COVEN-ASSURANCE/1 step-up proof (§4.3), never client-asserted.",
  "type": "object",
  "additionalProperties": false,
  "required": [
    "version", "introductionTranscriptHash", "approver", "approverRole",
    "assuranceProof", "issuedAt", "expiresAt", "signature"
  ],
  "properties": {
    "version": { "const": 1 },
    "introductionTranscriptHash": {
      "description": "SHA-256 over OpenCoven/IntroductionTranscript/v1 canonical bytes; commits to the full request and grant template.",
      "type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"
    },
    "approver": { "$ref": "#/$defs/keyReference" },
    "approverRole": {
      "description": "Only roles that can hold enrollment authority. A trusted-device approver must hold an active devices.enroll grant (§4.4); owner-root approves from the owner root credential.",
      "enum": ["trusted-device", "owner-root"]
    },
    "approverGrantId": {
      "description": "The approving device's current DeviceGrant id at signing time (trusted-device approvers only; the authority re-resolves and re-checks it together with the proof's grant/epoch binding, §4.4).",
      "type": "string", "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    },
    "assuranceProof": {
      "description": "COVEN-ASSURANCE/1 step-up proof by the approver's enrolled step-up authorization key (mobile-assurance-step-up-v1.md). Fresh user verification is proven, never asserted; see §4.3.",
      "$ref": "#/$defs/assuranceProof"
    },
    "attestation": {
      "description": "Optional assurance attributes held by the approver, minimally included when policy requires them.",
      "type": "array", "uniqueItems": true,
      "items": { "enum": ["verified_official_app", "verified_hardware_key"] }
    },
    "issuedAt": { "type": "integer", "minimum": 0 },
    "expiresAt": { "type": "integer", "minimum": 0 },
    "signature": {
      "description": "approver signature over OpenCoven/IntroductionApproval/v1 canonical bytes.",
      "type": "string", "pattern": "^[A-Za-z0-9_-]+$"
    }
  },
  "if": { "properties": { "approverRole": { "const": "trusted-device" } } },
  "then": { "required": ["approverGrantId"] },
  "$defs": {
    "keyReference": {
      "type": "object",
      "additionalProperties": false,
      "required": ["algorithm", "keyId", "publicKey"],
      "properties": {
        "algorithm": { "enum": ["Ed25519", "P-256"] },
        "keyId": { "type": "string", "minLength": 8, "maxLength": 128, "pattern": "^[A-Za-z0-9._:-]+$" },
        "publicKey": { "type": "string", "minLength": 43, "maxLength": 128, "pattern": "^[A-Za-z0-9_-]+$" }
      }
    },
    "assuranceProof": {
      "description": "COVEN-ASSURANCE/1 proof fields (context_mode introduction). Canonical bytes are defined in docs/design/mobile-assurance-step-up-v1.md; the authority rebuilds them from its own state and never trusts a client-supplied digest (§4.3).",
      "type": "object",
      "additionalProperties": false,
      "required": ["deviceId", "grantId", "revocationEpoch", "authorizationKeyId", "contextMode", "contextDigest", "challenge", "issuedAt", "expiresAt", "requestedAssurance", "signature"],
      "properties": {
        "deviceId": { "type": "string", "format": "uuid" },
        "grantId": { "type": "string", "format": "uuid" },
        "revocationEpoch": { "type": "integer", "minimum": 0 },
        "authorizationKeyId": {
          "description": "base64url SHA-256 over the enrolled step-up authorization public key (the grant.rs subject_key_id convention).",
          "type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"
        },
        "contextMode": { "const": "introduction" },
        "contextDigest": {
          "description": "SHA-256 over OpenCoven/IntroductionTranscript/v1 canonical bytes; MUST equal introductionTranscriptHash.",
          "type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"
        },
        "challenge": {
          "description": "Server-issued, single-use 32-byte challenge (base64url) bound to (deviceId, grantId, revocationEpoch); consumed atomically at verification.",
          "type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"
        },
        "issuedAt": { "type": "integer", "minimum": 0 },
        "expiresAt": { "type": "integer", "minimum": 0 },
        "requestedAssurance": { "enum": ["fresh_user_verification", "fresh_biometric"] },
        "signature": {
          "description": "ECDSA P-256 over SHA-256, DER, base64url, by the enrolled step-up authorization key, over the COVEN-ASSURANCE/1 canonical bytes.",
          "type": "string", "pattern": "^[A-Za-z0-9_-]+$"
        }
      }
    }
  }
}
```

The **introduction transcript** is canonical and covers at minimum: protocol version, `trustDomain`, `installationFingerprint`, the new endpoint public key and `pairwiseDeviceId`, the full `requestedScopes` set, `requestedAssurance`, the `deviceContext` digest, `nonce`, `expiresAt`, the *grant template* (audience, restrictions, `minimum_assurance`, planned `expires_at` policy), and the identity-reference of the expected issuer. This is the same transcript principle as `docs/design/mobile-device-trust.md` §"Transcript binding" and the `COVEN-PAIR/2` transcript in `pairing.rs` (`PairingTranscript::hash`).

### 3.3 RecoveryPolicy

Owner-controlled policy document, stored and interpreted by the authority layer (never by clients or the account service).

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://opencoven.ai/spec/device-pairing/v2/recovery-policy.schema.json",
  "title": "OpenCoven RecoveryPolicy v1 diagnostic JSON representation",
  "type": "object",
  "additionalProperties": false,
  "required": ["version", "trustDomain", "introductionPolicy", "recoveryFactors", "attestationPolicy", "updatedAt"],
  "properties": {
    "version": { "const": 1 },
    "trustDomain": { "type": "string", "minLength": 8, "maxLength": 256 },
    "introductionPolicy": {
      "type": "object",
      "additionalProperties": false,
      "required": ["standardMinApprovals", "rootMinApprovals", "requireDistinctApprovers"],
      "properties": {
        "standardMinApprovals": { "type": "integer", "minimum": 1, "maximum": 9 },
        "rootMinApprovals": { "type": "integer", "minimum": 1, "maximum": 9 },
        "requireDistinctApprovers": { "type": "boolean" }
      }
    },
    "recoveryFactors": {
      "description": "Factor combination that is sufficient to restore owner access. At least two distinct factor kinds are REQUIRED by this plan.",
      "type": "array", "minItems": 2, "uniqueItems": true,
      "items": { "enum": [
        "passkey_account", "recovery_key", "trusted_device",
        "owner_root_credential", "attested_device", "n_of_m_devices"
      ] }
    },
    "recoveryThreshold": {
      "type": "object",
      "additionalProperties": false,
      "required": ["of", "minimum"],
      "properties": {
        "of": { "type": "integer", "minimum": 1, "maximum": 9 },
        "minimum": { "type": "integer", "minimum": 1, "maximum": 9 }
      }
    },
    "attestationPolicy": {
      "type": "object",
      "additionalProperties": false,
      "required": ["default", "requireFor"],
      "properties": {
        "default": { "enum": ["ignore", "prefer"] },
        "requireFor": {
          "type": "array", "uniqueItems": true,
          "items": { "enum": ["secrets_read", "identity_admin", "devices_enroll", "devices_revoke", "identity_export", "memory_export"] }
        }
      }
    },
    "updatedAt": { "type": "integer", "minimum": 0 }
  }
}
```

### 3.4 RecoveryEvent

Append-only audit record, same storage discipline as `audit.jsonl` in `audit.rs` (private directory, atomic append, no secret material).

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://opencoven.ai/spec/device-pairing/v2/recovery-event.schema.json",
  "title": "OpenCoven RecoveryEvent v1 diagnostic JSON representation",
  "description": "Explicit, auditable record of a recovery or rotation ceremony. Never contains private keys, passkey identifiers, or account-provider identifiers.",
  "type": "object",
  "additionalProperties": false,
  "required": ["version", "eventId", "kind", "subject", "actors", "epoch", "occurredAt"],
  "properties": {
    "version": { "const": 1 },
    "eventId": { "type": "string", "pattern": "^[A-Za-z0-9_-]{22}$" },
    "kind": {
      "enum": [
        "recovery_started", "recovery_denied", "access_restored",
        "device_key_rotated", "identity_rotated", "root_rotated",
        "familiar_identity_adopted", "recovery_policy_changed"
      ]
    },
    "subject": { "$ref": "#/$defs/identityReference" },
    "actors": {
      "description": "Abstract factor kinds only (see privacy.md rule 8): no credential IDs, no key material.",
      "type": "array", "uniqueItems": true, "minItems": 1,
      "items": { "enum": ["passkey_account", "recovery_key", "trusted_device", "owner_root_credential", "attested_device", "n_of_m_devices"] }
    },
    "epoch": {
      "description": "revocation_epoch value after this event; rotations increment it.",
      "type": "integer", "minimum": 0
    },
    "rotatesIdentity": { "type": "boolean" },
    "outOfBandContext": {
      "description": "Human-readable ceremony description safe for audit; no secrets.",
      "type": "string", "maxLength": 200
    },
    "occurredAt": { "type": "integer", "minimum": 0 }
  },
  "$defs": {
    "identityReference": {
      "type": "object",
      "additionalProperties": false,
      "required": ["type", "id"],
      "properties": {
        "type": { "enum": ["owner", "installation", "device", "trust-domain", "familiar"] },
        "id": { "type": "string", "minLength": 8, "maxLength": 256, "pattern": "^[A-Za-z0-9._:-]+$" }
      }
    }
  }
}
```

### 3.5 AttestationClaim

An **assurance attribute** bound to a device key inside one trust domain. It is evidence about the app/key environment, never an identity, and never sufficient by itself for anything.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://opencoven.ai/spec/device-pairing/v2/attestation-claim.schema.json",
  "title": "OpenCoven AttestationClaim v1 diagnostic JSON representation",
  "type": "object",
  "additionalProperties": false,
  "required": ["version", "attribute", "evidence", "subject", "audience", "nonce", "issuedAt", "expiresAt", "verifierSignature"],
  "properties": {
    "version": { "const": 1 },
    "attribute": { "enum": ["verified_official_app", "verified_hardware_key", "unattested_device"] },
    "evidence": {
      "enum": ["apple_app_attest", "android_key_attestation", "android_play_integrity", "self_declared", "none"]
    },
    "subject": { "$ref": "#/$defs/keyReference" },
    "audience": { "$ref": "#/$defs/identityReference" },
    "nonce": { "type": "string", "pattern": "^[A-Za-z0-9_-]{43}$" },
    "issuedAt": { "type": "integer", "minimum": 0 },
    "expiresAt": { "type": "integer", "minimum": 0 },
    "verifierSignature": {
      "description": "Signature of the attestation verifier over OpenCoven/AttestationClaim/v1 canonical bytes.",
      "type": "string", "pattern": "^[A-Za-z0-9_-]+$"
    }
  },
  "$defs": {
    "keyReference": {
      "type": "object",
      "additionalProperties": false,
      "required": ["algorithm", "keyId", "publicKey"],
      "properties": {
        "algorithm": { "enum": ["Ed25519", "P-256"] },
        "keyId": { "type": "string", "minLength": 8, "maxLength": 128, "pattern": "^[A-Za-z0-9._:-]+$" },
        "publicKey": { "type": "string", "minLength": 43, "maxLength": 128, "pattern": "^[A-Za-z0-9_-]+$" }
      }
    },
    "identityReference": {
      "type": "object",
      "additionalProperties": false,
      "required": ["type", "id"],
      "properties": {
        "type": { "enum": ["trust-domain", "installation"] },
        "id": { "type": "string", "minLength": 8, "maxLength": 256, "pattern": "^[A-Za-z0-9._:-]+$" }
      }
    }
  }
}
```

### 3.6 TypeScript reference types

Reference shapes for the TypeScript integration packages (`packages/`). They are documentation of the diagnostic JSON, not a second authority; the Rust authority layer remains the only decision point (see `docs/design/mobile-device-trust.md` §"Authority boundary").

```typescript
export type Capability =
  | 'sessions.metadata.read' | 'conversations.read' | 'messages.send'
  | 'tools.request' | 'tools.approve' | 'secrets.read'
  | 'memory.familiar.read' | 'memory.familiar.write'
  | 'identity.admin' | 'devices.enroll' | 'devices.revoke'
  | 'identity.export' | 'memory.export';

/**
 * Attestation policy classes (underscore form) — a distinct vocabulary from
 * Capability ids (dot form). These are the only values accepted by
 * `RecoveryPolicy.attestationPolicy.requireFor` (see the §3.3 JSON Schema).
 */
export type AttestationPolicyClass =
  | 'secrets_read'
  | 'identity_admin'
  | 'devices_enroll'
  | 'devices_revoke'
  | 'identity_export'
  | 'memory_export';

export type Assurance =
  | 'possession' | 'recent_user_verification' | 'fresh_user_verification'
  | 'fresh_biometric' | 'step_up';

export type AttestationAttribute =
  | 'verified_official_app' | 'verified_hardware_key' | 'unattested_device';

export interface KeyReference {
  algorithm: 'Ed25519' | 'P-256';
  keyId: string;
  publicKey: string; // unpadded base64url
}

export interface IntroductionRequest {
  version: 1;
  trustDomain: string;
  installationFingerprint: string; // base64url SHA-256 of host key
  endpointKey: KeyReference;
  pairwiseDeviceId: string;
  requestedScopes: Capability[];
  requestedAssurance: Assurance;
  deviceContext: {
    platform: 'ios' | 'android' | 'macos' | 'linux' | 'windows' | 'other';
    humanReadableName: string;
    appVariant?: string;
  };
  nonce: string;
  expiresAt: number; // unix seconds
  signature: string;
}

/**
 * COVEN-ASSURANCE/1 step-up proof fields for the introduction context
 * (context_mode "introduction"); canonical bytes per
 * mobile-assurance-step-up-v1.md. The authority recomputes the context
 * digest and computes effective assurance server-side — never client-asserted.
 */
export interface AssuranceProof {
  deviceId: string; // approving device UUID
  grantId: string; // approving device's current DeviceGrant id
  revocationEpoch: number; // approver grant's revocation epoch at signing time
  authorizationKeyId: string; // base64url SHA-256 over the step-up public key
  contextMode: 'introduction';
  contextDigest: string; // SHA-256 over the introduction transcript canonical bytes
  challenge: string; // server-issued, single-use
  issuedAt: number; // unix seconds
  expiresAt: number; // unix seconds; issuedAt + ≤120 s
  requestedAssurance: 'fresh_user_verification' | 'fresh_biometric';
  signature: string; // base64url DER ECDSA P-256 by the step-up authorization key
}

export interface IntroductionApproval {
  version: 1;
  introductionTranscriptHash: string;
  approver: KeyReference;
  approverRole: 'trusted-device' | 'owner-root';
  approverGrantId?: string; // required for trusted-device approvers (delegated authority, §4.4)
  assuranceProof: AssuranceProof;
  attestation?: Exclude<AttestationAttribute, 'unattested_device'>[];
  issuedAt: number;
  expiresAt: number;
  signature: string;
}

export type RecoveryFactor =
  | 'passkey_account' | 'recovery_key' | 'trusted_device'
  | 'owner_root_credential' | 'attested_device' | 'n_of_m_devices';

export interface RecoveryPolicy {
  version: 1;
  trustDomain: string;
  introductionPolicy: {
    standardMinApprovals: number;
    rootMinApprovals: number;
    requireDistinctApprovers: boolean;
  };
  recoveryFactors: RecoveryFactor[];
  recoveryThreshold?: { of: number; minimum: number };
  attestationPolicy: { default: 'ignore' | 'prefer'; requireFor: AttestationPolicyClass[] };
  updatedAt: number;
}

export type RecoveryEventKind =
  | 'recovery_started' | 'recovery_denied' | 'access_restored'
  | 'device_key_rotated' | 'identity_rotated' | 'root_rotated'
  | 'familiar_identity_adopted' | 'recovery_policy_changed';

export interface RecoveryEvent {
  version: 1;
  eventId: string;
  kind: RecoveryEventKind;
  subject: { type: 'owner' | 'installation' | 'device' | 'trust-domain' | 'familiar'; id: string };
  actors: RecoveryFactor[];
  epoch: number;
  rotatesIdentity?: boolean;
  outOfBandContext?: string;
  occurredAt: number;
}

export interface AttestationClaim {
  version: 1;
  attribute: AttestationAttribute;
  evidence: 'apple_app_attest' | 'android_key_attestation' | 'android_play_integrity' | 'self_declared' | 'none';
  subject: KeyReference;
  audience: { type: 'trust-domain' | 'installation'; id: string };
  nonce: string;
  issuedAt: number;
  expiresAt: number;
  verifierSignature: string;
}
```

## 4. Trusted-device introduction

### 4.1 Flow

```text
New Psyche/TUI node                       Existing trusted phone
      │                                           │
      │ 1. IntroductionRequest                     │
      │─────────── over authenticated E2EE ───────▶│
      │                                           │ 2. render material fields
      │                                           │    fetch server-issued assurance challenge
      │                                           │    fresh step-up (biometric / user verification)
      │                                           │    IntroductionApproval signed by device key
      │                                           │    + COVEN-ASSURANCE/1 proof by step-up key (§4.3)
      │ 3. approvals collected                     │
      │◀───────────────────────────────────────────┘
      ▼
Installation authority (Rust)
      │ 4. verify signatures + COVEN-ASSURANCE/1 proofs,
      │    check delegated authority (§4.4),
      │    count approvals vs RecoveryPolicy,
      │    re-check transcript, bind grant to endpointKey
      ▼
scoped grant → new installation (DeviceGrant, registry.rs)
```

Delivery reuses the #787 channels: the authenticated relayed session (`crates/coven-relay/`) or the direct mobile gateway (`gateway.rs`). The push notification, when used, is a wake-up that names an opaque pending request — it carries no authorization (privacy rule 6, `spec/device-pairing/v1/privacy.md`).

### 4.2 Transcript binding (issue checkbox 1)

The approval MUST bind, in the signed `introductionTranscriptHash`:

- the new endpoint public key (`endpointKey`) and its `pairwiseDeviceId`;
- the exact `requestedScopes` set and `requestedAssurance`;
- the single-use `nonce` and `expiresAt`;
- the human-readable `deviceContext` (digest of the rendered text the approver saw);
- the target `installationFingerprint` and trust domain;
- the grant template the authority will issue.

Any change to any of these changes the transcript hash and invalidates every collected approval. This is the same fail-closed principle as capability substitution defense in the threat model, and it reuses the canonical-bytes discipline of `DeviceActionIntent::canonical_bytes` (`grant.rs`).

### 4.3 Fresh step-up authentication — a `COVEN-ASSURANCE/1` proof, never client-asserted (issue checkbox 2)

An introduction approval is a step-up operation by definition, and the approving device's ordinary possession key can never upgrade assurance by asserting it — the same gap `mobile-assurance-step-up-v1.md` (`COVEN-ASSURANCE/1`, #815/#871) closes for requests and `DeviceActionIntent`s. The approval therefore MUST carry a step-up proof signed by the approver's separately enrolled, platform-policy-protected **step-up authorization key** (`assuranceProof`, §3.2). A signature by the ordinary device key over the approval alone is not sufficient and never raises the approval's assurance above possession.

The proof is bound to:

1. the **introduction transcript** — `contextMode: "introduction"` with `contextDigest = SHA-256(OpenCoven/IntroductionTranscript/v1 canonical bytes)`; the authority recomputes the transcript bytes from the request and grant template it holds, so a proof over any other transcript fails the digest check;
2. the **approver's current grant and epoch** — the proof's `deviceId`/`grantId`/`revocationEpoch` must match the approver's active `DeviceGrant` in the registry at verification time (delegated authority, §4.4), so proofs minted against a stale or revoked grant do not count;
3. a **server-issued, single-use challenge** obtained from the authority before signing, under exactly the issuance/consumption discipline of `mobile-assurance-step-up-v1.md` (bound to device, grant, and epoch; atomically consumed; grant rotation or revocation invalidates outstanding challenges) — proofs cannot be banked offline while the device is unlocked;
4. a **≤120 s proof window** — the approval's own `expiresAt` may be longer, but proof freshness is what bounds the step-up.

The authority verifies the proof in the normative order of `mobile-assurance-step-up-v1.md` and computes effective assurance itself — `min(requested_assurance, enrolled-class ceiling)` — requiring at least `fresh_user_verification` (ceiling `fresh_biometric` where the platform enforces biometric-only policy) for a `trusted-device` approval to count. Fail-closed: an absent, expired, replayed, or invalid proof means the approval does not count; a trusted-device approval can never be counted on the strength of its possession-key signature. The passcode-fallback rule is preserved: passcode fallback is representable only as `fresh_user_verification`, never `fresh_biometric` (threat model: "Biometric exfiltration or false representation"). The biometric itself never enters the protocol — only signatures cross the trust boundary.

### 4.4 Delegated authority of approvers and scope caps

Signatures and approval counts alone do not authorize enrollment: an approval carries authority only from the approver's **current, active grant**. Before any approval is counted, the authority MUST, for every approver:

1. **Resolve the approver to an active grant.** `approver.keyId` must resolve to a registered device whose active `DeviceGrant` matches `approverGrantId` and the proof's `grantId`/`revocationEpoch` (and trust-domain epoch, §6.2). Unresolved, revoked, expired, or epoch-stale ⇒ the approval does not count. An `owner-root` approver signs with the owner root credential instead and holds the full capability vocabulary by definition.
2. **Check the delegation capability.** A `trusted-device` approver MUST hold `devices.enroll` in that active grant. No other capability, factor kind, or attestation attribute confers enrollment authority; a recovery provider (a role that exists only in recovery ceremonies, §6) can never approve an introduction.
3. **Cap scopes to delegable authority.** The delegable authority of a `trusted-device` approver is exactly the capability set of its active grant; the delegable authority of an `owner-root` approver is the full capability vocabulary. The grant issued to the introduced device MUST satisfy `issuedScopes ⊆ ⋂ delegable(approver)` over every counted approval. Because the transcript commits to the exact `requestedScopes` (§4.2), a scope outside some approver's delegable set makes quorum unreachable — the introduction is denied with an error naming the missing delegation, not silently narrowed.
4. **Issue from the grant template.** The issued grant's capabilities, `minimum_assurance`, restrictions, and expiry come from the transcript's grant template, re-verified at completion (§4.5); the authority never widens a grant beyond what was committed and delegated.

An approver whose grant is revoked, expired, or epoch-bumped after signing but before completion fails the re-check at completion — collected approvals do not outlive their grant.

### 4.5 Introduction state machine

```text
drafted
  │ endpoint signs request
  ▼
requested ── expiresAt / cancel / malformed ──▶ terminal (erased)
  │ approvals collected (one or more, per policy)
  ▼
approved ── authority verifies proofs, delegated authority
           (§4.4), counts ──▶ denied → terminal (RecoveryEvent: recovery_denied)
  │ transcript re-verified at authority
  ▼
enrolling
  │ grant written (registry.register_with_grant)
  ▼
completed ── later revoke/expire/rotate ──▶ revoked-or-expired (existing registry semantics)
```

Rules:

- Nonces are single-use and bounded; expired requests are pruned like pairing invitations (`PairingManager::prune_expired`, `pairing.rs`).
- Approval counting happens only in the authority layer; a relay, the account service, or the requesting endpoint can never count its own approvals.
- Approval counting is preceded by the delegated-authority checks of §4.4: approvers without a currently valid enrollment authority never reach the count, regardless of valid signatures.
- The completed introduction is auditable: the authority appends a `RecoveryEvent` with `kind: access_restored` (for recovery paths) or extends the device lifecycle events in `audit.rs` with the introduction outcome.

### 4.6 One device or N-of-M? (issue checkbox 3)

**Recommendation:** one fresh-step-up trusted device is sufficient by default; root-level policy can require N-of-M.

| Enrollment class | Default policy | Rationale |
| --- | --- | --- |
| Standard endpoint (conversation/message scopes) | `standardMinApprovals: 1` | Matches the QR bootstrap trust level; friction proportional to risk. |
| Elevated endpoint (`tools.approve` capability, `secrets_read` attestation class) | `standardMinApprovals: 1` + `attestationPolicy.requireFor` or narrower grant | Scope narrowing substitutes for ceremony overhead. |
| Root-level endpoint (`identity.admin`, `devices.enroll`, `devices.revoke`, `identity.export`, `memory.export`) | `rootMinApprovals: 2` with `requireDistinctApprovers: true` | A single compromised trusted device must not be able to mint root-level authority silently. |

Alternatives considered: (a) always N-of-M — rejected as the default because a sole owner with one phone could never bootstrap; (b) always 1 — rejected for root-level classes because it concentrates trust in one device; (c) time-delayed approval (24 h cooling) — kept as an optional owner knob, not a default. The policy knob lives in `RecoveryPolicy.introductionPolicy`; the owner decides; the authority enforces.

Quorum counting rules: approvals are distinct only when made by distinct device keys under distinct active grants (one device approving twice counts once, including re-approval after a transcript change); the requesting endpoint can never be its own approver; and every counted approval must independently pass the delegated-authority checks of §4.4.

### 4.7 Compromised relay / account service (issue checkbox 4)

The property to hold: **a compromised relay or account service alone cannot enroll an endpoint.**

- Enrollment authority is derived only from signatures over the introduction transcript made by trusted-device keys or the owner root credential. Neither the relay (`crates/coven-relay/`) nor any account service holds those keys or a role in the signature chain.
- The account service can, at most, deliver requests and echo opaque state. Its artifacts are consumed only as *assurance attributes* (`AttestationClaim`), which never mint authority (threat-model invariant 8: "A cloud account, passkey, push provider, or attestation provider alone cannot mint root authority").
- An offline/self-hosted Coven completes introductions with no account service at all: the account path is optional and additive.
- The authority re-derives and re-verifies the transcript server-side before issuing the grant, so a malicious intermediary cannot present an approval for a different request (transcript substitution fails the hash check).

## 5. Passkeys

### 5.1 Where passkeys are used

| Use | Mechanism | Authority effect |
| --- | --- | --- |
| Optional OpenCoven account sign-in | WebAuthn/passkey ceremony against the (optional) account provider | Account-session only; no Coven authority |
| Account/recovery authentication | Passkey ceremony as one `passkey_account` factor | Counts toward `RecoveryPolicy.recoveryFactors` |
| Remote enrollment authorization | Account-authenticated session *initiates* an introduction request delivery | Still requires trusted-device/owner approvals (§4.7) |
| Approving a new installation from an already trusted device | Passkey as an additional factor on the approving device, never a substitute for its device key | Assurance attribute at most |
| Cross-platform browser/native login | Standard WebAuthn; platform-neutral | Account-session only |

### 5.2 Hard prohibitions (issue checkboxes)

A passkey — in particular a synced passkey — MUST NOT be treated as:

1. **the sole root key of a familiar** — familiar identity keys are managed by the authority layer and the identity resolver (`familiar_identity.rs`, `docs/familiars/identity.md`); a synced passkey exists in a platform sync fabric outside OpenCoven's control;
2. **proof of one particular physical device** — a synced passkey may be available on every device in a platform account; it proves account possession, not device possession. Device identity remains the pairwise, non-exportable device key (`grant.rs` `subject_key_id`);
3. **a globally reused familiar/device identifier** — passkey credential IDs are scoped to the account RP and MUST NOT appear in Coven protocol surfaces (privacy rule 8);
4. **the only recovery mechanism** — `RecoveryPolicy.recoveryFactors` must always offer at least one account-independent path (recovery key, owner root credential, or trusted-device quorum).

Mechanistically: a successful passkey ceremony yields an account-layer session that may *request* ceremonies and may contribute the `passkey_account` factor to recovery counting. It never signs `DeviceGrant`s, never appears in `DeviceGrant.subject_key_id`, and never substitutes for `fresh_user_verification`/`fresh_biometric` in an `IntroductionApproval`.

## 6. Recovery

### 6.1 Recovery is a first-class protocol

Recovery ceremonies are explicit objects and events (`RecoveryPolicy`, `RecoveryEvent`), not a password-reset fallback. Evaluated combinations:

| Combination | Use | Notes |
| --- | --- | --- |
| passkey/account + trusted device | Owner regains account, then approves re-enrollment from the trusted phone | Common "new laptop" path |
| recovery key/seed | Offline, account-independent restoration | Generated at first enrollment, displayed once, stored by the owner outside OpenCoven; the seed never transits the account service |
| another trusted device | Surviving-device approval of a new endpoint | §4 |
| N-of-M trusted-device approval | Owner policy for high-assurance households/teams | `recoveryThreshold` |
| owner root credential | Last-resort local ceremony on the installation itself | Works with all network paths down |

Minimum viable default (recommended): **recovery key + passkey account**, with the trusted-device path always available while any enrolled device survives. All combinations are owner-selectable via `RecoveryPolicy`.

### 6.2 Recovering access ≠ rotating identity (issue requirement)

Two distinct ceremonies with distinct event kinds:

```text
Recovery (restore access)                    Rotation (replace identity)
recovery_started                             identity_rotated proposed
  │ factors collected per RecoveryPolicy       │ quorum per introductionPolicy.rootMinApprovals
  ▼                                            ▼
access_restored                              rotation quorum met
  │ same familiar/installation identity         │ revocation_epoch += 1 (DeviceGrant.revocation_epoch)
  │ new DeviceGrant(s) for the new endpoint     │ old keys revoked; new keys enrolled
  │ familiar identity untouched                 │ familiar identity key replaced deliberately
  ▼                                            ▼
RecoveryEvent(kind: access_restored,         RecoveryEvent(kind: identity_rotated,
              rotatesIdentity: false)                       rotatesIdentity: true)
```

Invariants:

- Recovery NEVER rotates, deletes, or rewrites familiar identity or memory (threat-model invariant: "Revoking a device does not rotate or destroy familiar identity").
- Rotation is always explicit: it requires the root-level threshold (§4.6), produces `RecoveryEvent` records with `rotatesIdentity: true`, bumps `revocation_epoch` (the same epoch semantics `DeviceGrant` and `registry.rs` already use to invalidate pre-rotation state), and is refused while any recovery ceremony is mid-flight.
- Every step of both ceremonies appends `RecoveryEvent` records to the same private append-only audit stream as `audit.rs` — recovery events and key rotations are explicit and auditable.

### 6.3 Golden example — recovery event after a lost laptop

Example literals in this section are intentionally synthetic placeholders (repeated low-entropy tokens), not real key material.

```json
{
  "version": 1,
  "eventId": "evt-evt-evt-evt-evt-ev",
  "kind": "access_restored",
  "subject": { "type": "device", "id": "trust-alpha:pairwise-7f3a91" },
  "actors": ["passkey_account", "trusted_device"],
  "epoch": 4,
  "rotatesIdentity": false,
  "outOfBandContext": "Lost laptop re-enrolled as replacement endpoint after passkey account auth and phone approval",
  "occurredAt": 1790000000
}
```

## 7. Optional attestation

### 7.1 Attribute registry

Canonical snake_case values match `restrictions.attestation` in `spec/device-pairing/v1/device-grant.schema.json` (display labels from the issue: *verified-official-app*, *verified-hardware-key*, *unattested-device*):

| Attribute | Meaning | Typical evidence |
| --- | --- | --- |
| `unattested_device` | Default for every device; self-built clients, dev builds, self-hosted runtimes | `none` or `self_declared` |
| `verified_official_app` | App binary provenance verified (e.g. Apple App Attest; Android Play integrity signals) | `apple_app_attest`, `android_play_integrity` |
| `verified_hardware_key` | Key is hardware-protected (e.g. Secure Enclave; Android hardware-backed Keystore attestation) | `apple_app_attest` key assurance, `android_key_attestation` |

Mapping notes: Apple App Attest and Android key attestation are evaluated by an optional verifier component; OpenCoven protocol surfaces see only the resulting `AttestationClaim` — opaque receipts, no attestation payloads, per privacy rule 7 ("attestation values are minimized to policy-relevant assurance claims").

### 7.2 Policy integration and the open trust model

- Absence of attestation is **not** a protocol failure: `unattested_device` is a full participant subject to owner policy (threat model: "Attestation lock-in" controls).
- `attestationPolicy.default` is `prefer` or `ignore`; `requireFor` may name only high-risk capabilities (`secrets_read`, `identity_admin`, device enrollment/revocation, exports) and only the owner can set it.
- Attestation never replaces proof of possession, owner delegation, or local user verification; it can only *add* assurance to an operation that already passed those checks.
- Attestation claims are audience-bound to one trust domain and expire; they cannot correlate a device across Covens (privacy rules 1 and 8).
- Self-hosted deployments can run their own verifier or none at all; nothing in the protocol requires a proprietary service.

### 7.3 Golden example — policy-gated secrets approval

```json
{
  "version": 1,
  "attribute": "verified_hardware_key",
  "evidence": "android_key_attestation",
  "subject": {
    "algorithm": "P-256",
    "keyId": "trust-alpha:device-key-placeholder",
    "publicKey": "pk-pk-pk-pk-pk-pk-pk-pk-pk-pk-pk-pk-pk-pk-p"
  },
  "audience": { "type": "trust-domain", "id": "trust-alpha" },
  "nonce": "nonce-nonce-nonce-nonce-nonce-nonce-nonce-n",
  "issuedAt": 1790000000,
  "expiresAt": 1790086400,
  "verifierSignature": "sig-sig-sig-sig-sig-sig-sig-sig-sig-sig-sig-s"
}
```

With `attestationPolicy.requireFor: ["secrets_read"]`, a `tools.approve` transaction touching a `secrets_read`-scoped grant (see `DeviceActionIntent`, `grant.rs`) succeeds only when the approving device key carries a valid `verified_hardware_key` or `verified_official_app` claim. The same transaction from an `unattested_device` key fails with a policy error that names the missing attribute — never a generic denial.

## 8. Policy examples (issue table, mapped)

| Scenario | Required policy |
| --- | --- |
| Normal conversation | Valid enrolled device: device key + live `DeviceGrant` (`possession`) — `auth.rs` request verification path |
| Tool approval | Enrolled device + fresh local user verification — `DeviceActionIntent` with `require_fresh_user_verification_for` restriction (`grant.rs`) |
| Secrets/root administration | Hardware-backed or attested device and/or second factor when `attestationPolicy.requireFor` / `rootMinApprovals` say so (§4.6, §7.2) |
| New-device enrollment | Fresh step-up on an existing trusted device; threshold approval per `introductionPolicy`; optionally attested approvers (§4) |

## 9. Threat-model deltas

Extends `docs/security/mobile-device-pairing-threat-model.md` without weakening any existing control:

- **Compromised trusted device**: can introduce one scoped endpoint within `expiresAt`; root-level classes require `rootMinApprovals ≥ 2` distinct approvers; the epoch bump and audit trail bound and expose the damage; owner revokes via the existing `registry.revoke` path.
- **Passkey/account provider compromise**: cannot enroll endpoints (§4.7); cannot read or forge E2EE sessions; can at most request attention.
- **Attestation forgery**: verifier compromise yields only assurance attributes, which cannot mint authority; owner policy that never sets `requireFor` is unaffected.
- **Recovery-ceremony phishing**: transcript-bound `COVEN-ASSURANCE/1` proofs with server-issued single-use challenges make collected approvals useless for any other request; recovery events are user-visible in `coven memory mobile status` output (which already surfaces device/grant state, `registry.list_status`).

## 10. Acceptance criteria mapping

| Issue acceptance criterion | Where satisfied |
| --- | --- |
| Recover/enroll remotely without exposing familiar/root private material to OpenCoven infrastructure | §4.1 (transcript-bound approvals), §5.1, §6.1 (recovery key stays owner-held); grants issued locally by the authority |
| Synced passkeys do not masquerade as physical-device identity | §5.2 prohibitions; `subject_key_id` remains the pairwise device key |
| Attestation increases assurance without breaking the open/self-hosted model | §7.2; `unattested_device` full participation; no mandatory proprietary dependency |
| A compromised account service alone cannot mint arbitrary device authority | §4.7; threat-model invariant 8 |
| Recovery events and identity/key rotations are explicit and auditable | §3.4 `RecoveryEvent`, §6.2 ceremonies, §6.3 example; same audit stream as `audit.rs` |
| Owner can choose stricter policies without imposing proprietary platforms on every Coven | §3.3 `RecoveryPolicy`, §4.6 defaults, §7.2 policy knobs |

## 11. Implementation staging

Aligned with delivery-plan PR 7; each sub-stage is independently mergeable and preserves v1/v2 behavior:

1. **7a — objects and policy engine (Rust, authority layer):** new module (suggested: `crates/coven-cli/src/mobile_memory/recovery.rs`) with the five objects, canonical encoding, domain labels, `RecoveryPolicy` storage and evaluation; registry extension for introduction state; JSON schemas under `spec/device-pairing/v2/` plus a v2 conformance manifest and test-vector files; adversarial tests (substitution, replay, threshold bypass, expired approvals).
2. **7b — introduction flow:** CLI (`coven memory mobile introduce`), approval UI surface on the trusted device, delivery over #787 transports, gateway/internal routes mirroring the existing `/api/v1/internal/mobile/pairings` pattern (`gateway.rs`), audit integration.
3. **7c — recovery ceremonies and events:** `RecoveryPolicy` ceremony runner, `RecoveryEvent` append-only log, epoch-bump rotation path, `coven memory mobile recovery` command group, export/redaction rules.
4. **7d — optional attestation adapter contract:** verifier adapter interface (no proprietary SDK in core), claim cache with expiry, policy gating in the authority, `attestationPolicy` enforcement tests for both `prefer` and `requireFor` modes and for the no-verifier/self-hosted configuration.

Merge gates: the repository gates (`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --locked`, `python scripts/check-secrets.py`, `python3 scripts/check-coven-privacy.py --staged`) plus the delivery-plan merge policy: positive, negative, replay, downgrade, and malformed-input tests appropriate to the layer; migration/compatibility notes for stored registry state.

## 12. Decisions and alternatives considered

| Decision | Choice | Alternatives rejected/deferred |
| --- | --- | --- |
| Who may approve an introduction | Trusted-device keys and the owner root credential, after delegated authority is checked (§4.4) | Account service as approver — rejected (invariant 8); relay as approver — rejected; recovery-provider approvals — rejected (no enrollment authority, §4.4) |
| Fresh step-up authentication | `COVEN-ASSURANCE/1` proof by the step-up authorization key over the introduction transcript, bound to grant/epoch and a server-issued single-use challenge (§4.3) | Client-asserted assurance enum — rejected (a possession-key signature cannot mint assurance; same rationale as #815/#871) |
| 1 vs N-of-M | 1 by default, N-of-M for root classes via owner policy (§4.6) | Always-N — locks out single-device owners; always-1 — concentrates root trust |
| Passkey → familiar root | Never; account-layer factor only (§5.2) | Synced-passkey-as-root — rejected by issue and architecture |
| Recovery default factors | recovery key + passkey account + surviving trusted device (§6.1) | Hosted Shamir/social recovery — deferred (privacy tradeoff, needs its own threat model); security questions — rejected |
| WebAuthn prf-derived owner keys | Deferred | Ties the owner root to account-provider availability; revisit only with an explicit owner opt-in |
| Attestation requirement | Optional, policy-gated, per-operation (§7.2) | Global mandatory attestation — rejected (breaks self-built/self-hosted clients) |
| Attestation value space | Reuse the v1 grant restriction enum (§7.1) | New vocabulary — rejected (fragmentation, migration cost) |
| Where the account factor lives | Optional external provider; protocol sees only abstract factor kinds (§5) | First-party required account — rejected; protocol-embedded account — out of scope for this repo |

## 13. Verification plan

Implementation PRs must add, at minimum:

- deterministic CBOR/COSE vectors for each new object (canonical-encoding suite, `conformance-manifest.json` suites);
- negative tests: transcript substitution, scope substitution, expired/single-use nonce replay, threshold bypass, approval-for-another-request, unknown attribute/evidence values (fail closed), attestation claim expiry;
- `COVEN-ASSURANCE/1` proof tests: challenge single-use/atomic consumption, proof replay, proof bound to a different transcript, proof against a stale grant or revocation epoch, possession-key signature offered as a step-up proof (must fail), expired proof window, effective assurance computed server-side (claimed level above the enrolled-class ceiling is capped);
- integration tests with a malicious relay/account shim: no enrollment without valid approvals; account service compromise reduces to a no-op on the authority path;
- privacy tests: no passkey credential IDs, attestation receipts, hardware IDs, or biometric metadata in any protocol object, audit record, or log line (extends `scripts/check-coven-privacy.py` coverage);
- revocation/rotation tests: epoch bump invalidates pre-rotation grants and resumption material while leaving familiar identity untouched.
