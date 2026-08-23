# OpenCoven Device Pairing v1

This directory contains the machine-readable diagnostic schemas for the OpenCoven mobile-device pairing protocol defined in [`docs/architecture/mobile-device-pairing-v1.md`](../../../docs/architecture/mobile-device-pairing-v1.md).

## Canonical wire format

The JSON schemas validate diagnostic and tooling representations. They are not the signed wire encoding. The protocol uses deterministic CBOR and COSE structures, with explicit domain separation and protocol-version binding.

## Schemas

- `pairing-offer.schema.json` — short-lived, single-use QR/bootstrap offer
- `device-grant.schema.json` — proof-of-possession-bound capability grant
- `transaction-authorization.schema.json` — exact, expiring authorization for a sensitive action

## Compatibility rules

- Unknown protocol versions fail closed unless an explicit compatible negotiation exists.
- Unknown capabilities never grant authority.
- Unknown required restrictions fail closed.
- Optional extension data may be preserved for forwarding but cannot influence authorization until understood.
- Signatures and transcript hashes are always computed over canonical CBOR, never arbitrary JSON serialization.

## Conformance work

The shared protocol implementation must add deterministic encoding vectors and adversarial tests before clients rely on these objects for authorization. The required test categories are listed in the architecture document.
