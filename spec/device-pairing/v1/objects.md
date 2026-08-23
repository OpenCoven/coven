# Protocol object ownership

| Object | Created by | Signed/authenticated by | Consumed by |
|---|---|---|---|
| PairingOffer | installation | bound by handshake transcript | mobile client |
| EnrollmentRequest | mobile client | device enrollment key | installation/owner authority |
| DeviceGrant | installation/owner authority | grant issuer key | installation and device clients |
| DeviceChallenge | verifier | authenticated session + freshness | device signer |
| TransactionAuthorization | mobile client | device key under grant policy | installation/tool gate |
| RevocationRecord | owner/authorized administrator | revocation authority | every verifier |
| ResumptionContext | both endpoints | sender-constrained session state | paired endpoints |

No transport, relay, account, push, discovery, or attestation component owns authorization semantics merely because it carries one of these objects.
