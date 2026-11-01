---
title: "Optional device attestation"
summary: "How platform attestation raises a device's assurance attribute without becoming an identity root or a mandatory dependency."
read_when:
  - Configuring attestation anchors or official apps
  - Reviewing how attestation affects trust decisions
  - Supporting self-built, development or self-hosted clients
source_adjacent_reason: "Defines the attestation invariants implemented beside attestation.rs and its exclusive fixture."
---

# Optional device attestation

Tracks #788. Attestation is an **assurance attribute** of an enrolled device
key:

| Attribute | Meaning |
|---|---|
| `unattested_device` | the default; no verifiable evidence, or none configured |
| `verified_hardware_key` | the device key is hardware-backed (TEE or StrongBox) under an owner anchor |
| `verified_official_app` | the above, and the key belongs to an app the owner named as official |

Each level implies the ones below it. Attestation never creates an identity,
a grant, an introducer or any authority. Absent, malformed, foreign,
software-only, expired or replayed evidence evaluates to `unattested_device`.
It is never an error, so self-built clients, development builds,
alternative distributions and self-hosted deployments keep working exactly
as before. Only an owner-chosen policy minimum can require more.

## Owner-supplied trust, no vendor roots

Coven compiles in **no** vendor root certificate. The owner configures
anchors and official app identities in `mobile/attestation-trust.json`:

```sh
coven device attestation add-android-root --certificate ROOT.pem
coven device attestation add-official-android-app --package PKG --signature-digest SHA256_B64URL
coven device attestation add-apple-root --certificate APPLE_APP_ATTEST_ROOT.pem
coven device attestation add-official-apple-app --team-id TEAMID1234 --bundle-id org.example.app
coven device attestation show
coven device attestation clear
```

With nothing configured, nothing is verifiable and every device is
`unattested_device`.

## Android key attestation

The evidence is the leaf-first DER chain for the device's own possession key.
Every condition below must hold:

- each certificate is valid now
- each certificate is signed by the next
- the last certificate is, or is signed by, an owner anchor
- the leaf certifies exactly the device's P-256 key
- the leaf's KeyDescription (`1.3.6.1.4.1.11129.2.1.17`) has
  `attestationChallenge` equal to the request challenge
- its `attestationSecurityLevel` is TrustedEnvironment or StrongBox

The challenge is SHA-256 of `COVEN-ATTESTATION/1\0` followed by the exact
transcript's possession bytes, so evidence cannot be replayed onto another
request. The same challenge serves both platforms.

The official-app level additionally requires that `attestationApplicationId`
(`[709]` in the software-enforced list) contains an owner-named package
*and* signing-certificate digest.

A strict, minimal DER reader parses only the fields above: definite lengths,
exact tags, and no trailing data.

## Apple App Attest

The evidence is the base64url CBOR attestation object that
`DCAppAttestService.attestKey` returns for the device's own possession key,
made with the request challenge as the `clientDataHash`. The object is
`{ fmt: "apple-appattest", attStmt: { x5c, receipt }, authData }`; it is
decoded with a bounded size (32 KiB) and nesting depth, with no duplicate or
unknown members and no trailing bytes. Every condition below must hold:

- the leaf-first `x5c` chain is valid now, each certificate signed by the
  next, and the last is, or is signed by, an owner-supplied Apple anchor
- the leaf certifies exactly the device's P-256 key
- the leaf's nonce extension (`1.2.840.113635.100.8.2`, a `SEQUENCE` holding
  `[1] OCTET STRING`) equals SHA-256 of `authData` followed by the challenge
- `authData` has the attested-credential flag, a zero counter, the
  production `appattest` AAGUID, and a credential ID equal to SHA-256 of the
  device's uncompressed key
- the `rpIdHash` equals SHA-256 of an owner-named App ID, `TEAMID.bundle.id`

App Attest always names the app and keys always live in the Secure Enclave,
so the only attribute it can produce is `verified_official_app`. An object
for an app the owner has not named, a development (`appattestdevelop`) key,
or any failure above is `unattested_device`. The `receipt` is material for
Apple's servers; Coven ignores it and makes no network call to Apple. Apple
anchors verify only Apple objects and Android anchors only Android chains.

## Owner policy minimum

`IntroductionPolicy.minimumAttestation` defaults to `unattested_device`. At
that default it is omitted from the policy's canonical encoding, so every
existing policy digest and request transcript is unchanged.

The owner can raise it through a root-signed install or replace:

```sh
coven device introduction-policy replace … --minimum-attestation verified-hardware-key
```

Raising it is a tightening. Lowering it is a downgrade
(`minimum attestation` in the broadened dimensions), so it needs the
owner-root key.

At owner-socket admission, the optional `attestation` evidence in the
enrollment envelope is evaluated against the destination key. The challenge
is derived from the request's possession bytes. A device below the minimum
is refused before any introducer is asked, and the check repeats at commit.
On success, the raised attribute is recorded for that device key, and
`coven device inspect` shows it.

## Records

`mobile/attestations.json` keeps only raised attributes, keyed by device ID
**and** key. A device with a rotated key does not inherit the old key's
attribute. A missing record means `unattested_device`.

## Fixture

`tests/fixtures/device-attestation/mod.rs` builds synthetic Android chains
and Apple attestation objects with rcgen; no real vendor root or device data
is checked in:

```sh
cargo test -p coven-cli --bin coven mobile_memory::attestation --locked
```
