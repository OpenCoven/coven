# Implementation gates

The v1 protocol is not considered interoperable or production-ready until every gate below is satisfied.

## Gate A — Canonical data model

- [ ] Deterministic CBOR profile is documented and enforced.
- [ ] Duplicate map keys, non-minimal integers, indefinite lengths, invalid UTF-8, and excess nesting are rejected.
- [ ] Domain-separation labels and version fields are included in every signed/hashed object.
- [ ] JSON diagnostic schemas and canonical CBOR model agree.

## Gate B — Cryptographic interoperability

- [ ] Noise pattern and prologue are fixed by specification.
- [ ] Algorithm negotiation cannot downgrade below policy.
- [ ] Deterministic vectors cover offer, transcript, enrollment, grant, challenge, revocation, resumption, and transaction authorization.
- [ ] A second implementation verifies every vector.

## Gate C — Enrollment safety

- [ ] Offers are random, expiring, atomic, and single-use.
- [ ] Capability digest is bound from QR creation through grant issuance.
- [ ] Both endpoints confirm the same short authentication string.
- [ ] Cancellation and every error path erase pairing secrets.

## Gate D — Authorization safety

- [ ] Grants are proof-of-possession bound.
- [ ] Unknown capabilities and required restrictions fail closed.
- [ ] Current revocation and narrowed policy override cached session state.
- [ ] Sensitive operations use exact transaction authorization with fresh nonce and expiry.

## Gate E — Platform identity and biometrics

- [ ] Device keys are non-exportable/hardware-backed when supported.
- [ ] Biometric data never enters app or protocol state.
- [ ] Biometric-only and passcode/device-owner fallback are distinct assurance results.
- [ ] Platform attestation is optional policy evidence, not an identity root.

## Gate F — Transport and privacy

- [ ] Relay sees only opaque routing metadata and ciphertext.
- [ ] Malicious-relay tests cover mutation, duplication, reordering, truncation, and replay.
- [ ] Discovery identifiers rotate and reveal no stable owner/familiar/device identity.
- [ ] Direct, relay, and network-migrated sessions preserve the same endpoint authorization.

## Gate G — Recovery

- [ ] Account/passkey compromise alone cannot enroll a privileged device.
- [ ] Trusted-device introduction binds endpoint key, scope, nonce, expiry, and displayed context.
- [ ] Recovery, owner rotation, familiar rotation, and device enrollment are separate audited operations.
- [ ] Lost-device revocation defeats stale resumption.

## Gate H — Operational readiness

- [ ] Audit events are structured and secret-free.
- [ ] Rate, size, timeout, and resource bounds are enforced before expensive work.
- [ ] Fuzz/property tests run in CI for parsers and validators.
- [ ] Rollback and stored-state migration behavior are documented.
- [ ] Feature remains explicitly experimental until TUI-to-Pocket E2E, revocation, and recovery exercises pass.
