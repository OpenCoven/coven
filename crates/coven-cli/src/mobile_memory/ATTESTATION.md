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
request.

The official-app level additionally requires that `attestationApplicationId`
(`[709]` in the software-enforced list) contains an owner-named package
*and* signing-certificate digest.

A strict, minimal DER reader parses only the fields above: definite lengths,
exact tags, and no trailing data.

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

Apple App Attest needs a CBOR attestation object and is a follow-up. Until it
lands, Apple devices are `unattested_device`.

## Fixture

`tests/fixtures/device-attestation/mod.rs` builds synthetic chains with
rcgen:

```sh
cargo test -p coven-cli --bin coven mobile_memory::attestation --locked
```
