# Mobile Device Pairing Threat Model

Status: Initial v1 implementation baseline

Related: #784, #785, #786, #787, #788

## Assets

- owner, familiar, installation, and device private keys
- device grants and revocation state
- pairing and authenticated-handshake secrets
- decrypted session/application traffic
- familiar memory and identity material
- exact transaction-authorization intent
- recovery credentials and ceremonies
- privacy of owner, familiar, installation, and device relationships

## Trust boundaries

1. Terminal/TUI process and local operating system
2. Pocket/mobile application and platform secure-key facilities
3. Local network and discovery plane
4. Rendezvous/relay infrastructure
5. Push-notification infrastructure
6. Optional OpenCoven account/recovery service
7. Application or hardware attestation providers
8. Familiar/runtime/tool execution boundary

The relay, local network, push provider, and account service are not trusted to mint device authority or read end-to-end encrypted application content.

## Adversaries

- passive network observer
- active network attacker
- malicious or compromised relay
- attacker who photographs or records a QR code
- attacker with a copied grant or local database
- attacker with temporary access to an unlocked endpoint
- malicious local-network discovery peer
- compromised middleware that changes an action after showing approval UI
- compromised account or push service
- malicious or vulnerable client implementation
- cross-service tracker correlating stable public identifiers
- attacker attempting protocol downgrade, replay, or resource exhaustion

## Threats and required controls

### Captured pairing offer

Threat: an attacker scans or replays a photographed QR.

Controls:

- cryptographically random session identifiers
- fresh ephemeral key per attempt
- short expiry
- atomic, single-use consumption
- bounded failures
- transcript-bound endpoint keys and requested capabilities
- explicit endpoint verification before grant issuance
- immediate secret erasure after terminal state

### Relay impersonation or plaintext access

Threat: a relay reads traffic, changes frames, or impersonates an endpoint.

Controls:

- endpoint-authenticated forward-secret encryption
- relay carries only opaque session IDs and ciphertext
- transcript commits to both endpoints and negotiation
- frame sequence/integrity checks
- replay and truncation detection
- relay is never an authorization issuer

### Capability substitution

Threat: requested permissions differ from the permissions displayed or granted.

Controls:

- canonical capability-set digest in QR offer
- complete capability set in handshake/enrollment transcript
- installation compares approved set before signing grant
- grant cannot exceed approved set
- unknown capabilities fail closed

### Stolen grant

Threat: an attacker copies a grant/token from storage.

Controls:

- grant is confirmation-key/proof-of-possession bound
- fresh challenge on connection
- hardware-protected, non-exportable key where available
- short-lived session keys
- revocation and key rotation

### Biometric exfiltration or false representation

Threat: raw biometric data leaves the OS, or passcode fallback is represented as a biometric.

Controls:

- application receives only local-auth success/failure and protected-key operation
- no biometric sample/template/hash in protocol objects or logs
- explicit assurance enum
- biometric-only and broader device-owner policies remain distinct
- recovery is an explicit separate ceremony

### Transaction substitution

Threat: the phone displays one action but signs another.

Controls:

- canonical transaction object
- material operation, target, effect, nonce, expiry, request digest, and presentation digest are signed
- UI renders all material signed fields
- signature is checked against current grant and policy
- short expiry and single-use nonce

### Stale resumption after revocation

Threat: a revoked device resumes an older session or restores previous scope.

Controls:

- resumption state is sender-constrained
- current grant and revocation epoch are evaluated during resume
- policy is re-evaluated after reconnect
- narrowed scope wins over cached session state
- revocation invalidates relevant resumption material

### Local discovery tracking

Threat: mDNS/BLE advertisements reveal stable owner, familiar, or device identity.

Controls:

- opaque rotating discovery identifiers
- no owner/familiar names by default
- discovery only locates candidates
- authenticated handshake establishes identity
- rate limits and user-controlled discoverability

### Cross-Coven correlation

Threat: unrelated Covens correlate the same mobile device.

Controls:

- pairwise device identifiers/keys where practical
- no globally reused public identifier in protocol surfaces
- no hardware IDs
- domain-separated derived identifiers

### Compromised account/recovery service

Threat: an account provider enrolls an arbitrary device.

Controls:

- account/passkey is one policy factor, not the identity root
- new endpoint key and requested authority are transcript-bound
- existing trusted device or owner authorization for sensitive enrollment
- optional N-of-M approval
- auditable recovery and rotation events

### Attestation lock-in

Threat: proprietary attestation silently becomes mandatory identity.

Controls:

- attestation represented as optional assurance attributes
- owner policy decides whether an operation requires attributes
- unattested and self-built clients remain protocol participants
- attestation cannot mint authority by itself

### Resource exhaustion

Threat: pairing requests exhaust CPU, memory, relay capacity, or terminal state.

Controls:

- bounded offers per installation
- expiry queues with deterministic cleanup
- pre-authentication rate limits and frame-size limits
- inexpensive validation before expensive cryptography
- bounded CBOR nesting and collection lengths
- cancellation support
- relay quotas that do not reveal identity

## Security invariants

1. Possessing a grant without its private key cannot authenticate.
2. Controlling the relay cannot decrypt or forge a valid endpoint session.
3. A biometric never becomes a network identifier or transmitted secret.
4. Pairing and transaction approvals are bound to the exact canonical transcript.
5. Revoking a device does not rotate or destroy familiar identity.
6. Discovery and transport metadata do not establish authorization.
7. A current, narrower policy overrides cached session or grant state.
8. A cloud account, passkey, push provider, or attestation provider alone cannot mint root authority.

## Verification plan

The implementation must include deterministic test vectors and negative tests for every control above. Fuzzing should target offer decoding, canonical CBOR, handshake-frame parsing, grant validation, capability/restriction handling, and transaction authorization. Integration tests should run with a malicious relay shim capable of dropping, duplicating, reordering, truncating, and mutating frames.
