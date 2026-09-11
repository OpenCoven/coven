# Mobile Pairing Protocol v2

**Status:** implementation contract  
**Authority:** Coven mobile gateway  
**Compatibility:** pairing v1 remains accepted; newly rendered QR offers use v2

## Purpose

Pairing v2 hardens the existing QR enrollment flow without replacing its proven transport and confirmation model. The QR introduces a short-lived Coven installation. TLS 1.3 protects the direct connection and the QR-pinned host public-key fingerprint authenticates the endpoint. A transcript-derived six-word phrase confirms that the phone and host observed the same offer and enrollment request before a device grant is persisted.

The QR is not a bearer login credential. A captured offer is useful only during its short invitation window, succeeds at most once, and still requires confirmation on both endpoints.

## Version separation

The existing protected mobile HTTP API remains `COVEN-MEMORY/1` and uses `x-coven-protocol: 1`. Pairing negotiation is a separate version space:

| Value | Meaning |
| --- | --- |
| Pairing minimum | `1` |
| Pairing current/maximum | `2` |
| Protected mobile API | `1` |

A v1 enrollment keeps its existing transcript byte-for-byte. A v2 enrollment selects pairing protocol `2` only when the client-declared range contains `2`.

## QR offer

The terminal renders a `coven-memory://pair` URL with these query members:

| Member | Contract |
| --- | --- |
| `version` | Selected pairing protocol; exactly `2` |
| `minimumVersion` | Oldest pairing protocol accepted by the host; `1` |
| `maximumVersion` | Newest pairing protocol accepted by the host; `2` |
| `pairingId` | Canonical lowercase UUID |
| `endpoint` | Canonical HTTPS mobile-gateway endpoint; root path only, with no user info, query, or fragment |
| `fingerprint` | Unpadded base64url encoding of SHA-256 over the host's canonical uncompressed 65-byte P-256 X9.63 public key |
| `nonce` | Unpadded base64url encoding of the 32-byte single-use pairing nonce |
| `expires` | Signed Unix timestamp in seconds |
| `scope` | Requested authority; currently exactly `memory_read` |
| `offerDigest` | Unpadded base64url SHA-256 digest of the canonical offer below |

The endpoint is a routing hint authenticated by requiring the TLS certificate
to use the public key whose fingerprint appears in the QR. It is intentionally
not included in the offer digest: changing an address without possessing the
corresponding host private key cannot impersonate the host, while allowing the
same installation identity to remain reachable through an equivalent route.

## Canonical offer digest

The offer digest is SHA-256 over length-prefixed fields. For each field, append its byte length as an unsigned 32-bit big-endian integer, then append the field bytes.

Fields, in order:

1. ASCII domain `COVEN-PAIR-OFFER/2`
2. minimum pairing version as unsigned 16-bit big-endian (`1`)
3. maximum pairing version as unsigned 16-bit big-endian (`2`)
4. raw 32-byte host fingerprint
5. raw 16-byte UUID representation of `pairingId`
6. raw 32-byte pairing nonce
7. expiry as signed 64-bit big-endian Unix seconds
8. UTF-8 scope name `memory_read`

Unknown, duplicate, non-canonical, or malformed URL members must fail closed in clients. Clients must compare the transmitted `offerDigest` to a locally recomputed value before enrollment.

## Enrollment request

The existing JSON request shape remains closed:

```json
{
  "protocolVersion": 2,
  "pairingNonce": "<base64url nonce>",
  "deviceName": "Val’s iPhone",
  "devicePublicKey": "<canonical P-256 X9.63 public key>",
  "appVersion": "1.0.0",
  "supportedProtocol": {
    "minimum": 1,
    "maximum": 2
  }
}
```

Requirements:

- `protocolVersion` is exactly `1` or `2` and lies inside the declared range.
- v2 clients select `2` only after validating the v2 QR offer.
- `pairingNonce` decodes canonically to the QR nonce.
- `devicePublicKey` is a canonical uncompressed 65-byte P-256 X9.63 public key.
- `deviceName` is non-empty, trimmed, control-character free, and no more than 80 Unicode scalar values.
- `appVersion` is non-empty ASCII, control-character free, and no more than 64 bytes.
- The first enrollment attempt consumes the nonce whether the request succeeds or fails.

## Pairing transcript v2

The v2 transcript digest is SHA-256 over the same unsigned 32-bit length-prefixed encoding with these fields:

1. ASCII domain `COVEN-PAIR/2`
2. raw 32-byte canonical offer digest
3. selected pairing version as unsigned 16-bit big-endian (`2`)
4. client minimum pairing version as unsigned 16-bit big-endian
5. client maximum pairing version as unsigned 16-bit big-endian
6. raw canonical device public-key bytes
7. UTF-8 device name
8. ASCII application version

This binds the host installation, invitation identity, nonce, expiry, requested authority, selected version, supported version range, device key, and human-visible device metadata into one confirmation value.

The first 66 bits of the transcript digest select six words from the existing fixed 2,048-word list. Both endpoints display those words in the same order. Persistence occurs only after both endpoints submit an exact phrase match before the original expiry.

## Completion and retries

- Enrollment changes the invitation from `invited` to `pending confirmation` and consumes the nonce.
- Either endpoint may confirm first.
- The authority persists the device record/grant exactly once after both confirmations.
- Matching retries return the same redacted completed device until the original invitation expires.
- A wrong phrase before completion destroys the pending pairing.
- A wrong phrase after completion does not create another device and does not erase the bounded idempotent retry result.
- Expired pairings are pruned opportunistically and never register a device.

## Cancellation and owner status

- The owner-only local daemon control reports `waiting_for_device`,
  `waiting_for_confirmation`, `completed`, `cancelled`, `expired`, or
  `unavailable` after a rejected single-use enrollment. Only the confirmation
  phase includes the six-word phrase; a rejected enrollment cannot resume.
- Explicit cancellation is idempotent until the invitation is pruned. It
  immediately erases the nonce hash, transcript hash, pending device metadata,
  and partial-confirmation flags.
- Cancellation before completion prevents later enrollment or confirmation
  from registering a device. Cancellation after completion reports the
  completed terminal state and never revokes the enrolled device.
- The terminal pairing command attempts cancellation on host decline,
  interruption, expiry, and other pre-confirmation exits. Once host
  confirmation is accepted, the device retains the rest of the original
  invitation window to finish, even if an interrupt arrives while the host
  confirmation response is in flight. A decline after expiry reports expiry,
  not successful cancellation.
- Remote callers receive the same unavailable response for expired, consumed,
  and locally cancelled invitations. Only the owner-only local control exposes
  the more precise terminal state.

## Security properties

1. Modifying any offer security field changes `offerDigest` and the six-word phrase.
2. Modifying the device key, selected/range versions, name, or app version changes the phrase.
3. Pairing v1 vectors remain unchanged for older compatible clients.
4. A relay or network observer cannot substitute a host without the private key corresponding to the QR-pinned P-256 public key.
5. Successful pairing creates a key-bound scoped grant, not a reusable bearer credential.
6. Biometric authorization is outside this transcript: the mobile operating system may gate use of the enrolled private key, while only signatures and assurance evidence reach Coven.
7. Cancelling an incomplete pairing destroys its pending secrets and cannot be
   distinguished from an unavailable invitation through the remote pairing
   API.

## Portable vector

The canonical synthetic vector lives at:

`crates/coven-cli/tests/fixtures/mobile-pairing-v2/transcript-vector.json`

It contains no live credential. Independent clients should reproduce both `offerDigest` and `transcriptDigest` exactly before interoperating.
