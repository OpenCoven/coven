# Security review checklist

- [ ] Reviewed cryptographic library and Noise pattern selected
- [ ] Deterministic CBOR and duplicate-map-key rejection specified
- [ ] Transcript binds offer, keys, capabilities, nonces, versions, and negotiation
- [ ] Pairing offer is atomic, single-use, expiring, and bounded
- [ ] Grant requires proof of possession and current revocation evaluation
- [ ] Resumption cannot restore revoked or narrowed authority
- [ ] Transaction approval signs every material displayed field
- [ ] Biometric-only assurance is not conflated with device passcode fallback
- [ ] Discovery exposes no stable owner, familiar, or device identifiers
- [ ] Relay, push, account, and attestation providers cannot mint authority
- [ ] Recovery distinguishes access, enrollment, owner rotation, and familiar rotation
- [ ] Sensitive state and secrets are erased on terminal/error paths
- [ ] Positive, negative, replay, downgrade, malformed-input, and malicious-relay tests pass
