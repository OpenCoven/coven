# Domain Separation Registry

All signatures, transcript hashes, derived identifiers, and key-derivation contexts MUST include an ASCII domain-separation label and protocol version before canonical payload bytes.

Initial v1 labels:

```text
OpenCoven/PairingOffer/v1
OpenCoven/PairingTranscript/v1
OpenCoven/EnrollmentRequest/v1
OpenCoven/DeviceGrant/v1
OpenCoven/DeviceChallenge/v1
OpenCoven/TransactionAuthorization/v1
OpenCoven/RevocationRecord/v1
OpenCoven/ResumptionContext/v1
OpenCoven/PairwiseDeviceId/v1
OpenCoven/ShortAuthenticationString/v1
```

Labels are protocol constants. Changing a label is a breaking interoperability change and requires a new version or an explicit negotiated profile.
