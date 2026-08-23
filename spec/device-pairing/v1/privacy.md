# Privacy requirements

1. Device identifiers exposed to unrelated Covens must not be globally correlatable.
2. Local discovery advertises opaque rotating identifiers only.
3. QR/bootstrap offers omit owner and familiar names unless an explicit UI-level policy requires them.
4. Relay routing uses opaque session identifiers and cannot inspect application plaintext.
5. Audit records omit private keys, raw offer secrets, biometrics, recovery secrets, and decrypted message/tool payloads.
6. Push payloads wake the app and identify an opaque pending request; they do not carry authorization or sensitive action content.
7. Attestation values are minimized to policy-relevant assurance claims.
8. Hardware serial numbers, advertising identifiers, account-provider identifiers, and biometric metadata are never protocol identity fields.
