---
title: "Access recovery and identity rotation"
summary: "Which recovery and rotation operations exist, what each may change, and how each is authorized and audited."
read_when:
  - Recovering access after losing trusted devices
  - Changing owner-root, device or familiar identity
  - Reviewing whether an operation can mint device authority
source_adjacent_reason: "Defines the recovery invariants implemented beside recovery.rs, owner_policy.rs and introduction.rs and their fixtures."
---

# Access recovery and identity rotation

Tracks #788. The pairing protocol (`docs/architecture/mobile-device-pairing-v1.md`)
requires recovery to distinguish four operations. They are separate code
paths, separate signed transcripts and separate audit events. None of them
silently performs another.

| Operation | What changes | Authorized by | Audit event | Never changes |
|---|---|---|---|---|
| Enroll a device through a trusted device | adds one device and grant | N named introducers, each with fresh step-up | `device_introduced` | owner root, policy, familiars |
| **Recover access** | adds one device and grant | owner-root signature over the exact request | `access_recovered` | owner root, policy, other devices, familiars |
| Rotate or replace a device | moves a grant to an already-paired device | local owner (`coven device rotate`) | `device_authorization_reenrolled` | owner root, policy, familiars |
| Rotate the owner root | replaces the root key in the policy chain | old *and* new root keys (#788 slice 4b) | `owner_root_rotated` | devices, grants, familiars |
| Rotate a familiar identity | — | no such operation exists | — | — |

## Recovering access

`AccessRecoveryAuthority::recover` enrolls a device when no trusted introducer
is available, for example after every trusted device was lost. The owner-root
private key is the sole approver. It stays with the owner, outside
`COVEN_HOME` and off OpenCoven infrastructure.

`AccessRecoveryRequest` (v1) binds:

- the installation audience
- the **exact policy head**: revision plus chain digest, so the signature dies
  on any policy change
- the destination ID and P-256 key
- the complete grant
- the display name and context
- nonce, issued-at and expiry, with at most 300 seconds of validity

The owner-root key signs a `COVEN-ACTION/1` intent with operation
`owner.access_recovery.approve` and an effect digest over
`COVEN-ACCESS-RECOVERY/1\0` followed by the JCS-encoded request. The
destination signs the same intent with operation
`owner.access_recovery.accept`, proving it holds the key being enrolled.

Commit checks run under the policy lock and then the registry lock, in the
documented policy store → device registry order. The head is read and the
enrollment written under that one policy lock, so a rotation or replacement
cannot land between them:

- the installed policy head is still the one named
- the request is inside its validity window
- the grant has an explicit expiry, fits the policy lifetime, and stays within
  the policy's allowed scopes
- the destination's possession signature is valid
- the installed owner-root signature is valid

The commit shares the introduction nonce ledger, so a recovery cannot be
replayed and cannot reuse an introduction nonce. Its transition is marked
`kind: access_recovery`; plain introductions serialize exactly as before.

Recovery enrolls no authorization (step-up) key. A recovered device can use
its grant, but it cannot act as an introducer until a separate key-enrollment
ceremony exists. Recovery does not revoke the lost devices either. Revoke them
explicitly with `coven device revoke --reason lost`, and each revocation is
audited as its own event.

`coven device recover --request ENVELOPE --root-key PEM` prints every signed
field, plus what recovery will *not* do. It signs only after the owner types
`recover`. The root key must be the installed one and must live outside
`COVEN_HOME`.

## Why a compromised service cannot recover access

A relay or account service holds neither the owner-root key nor the
destination's private key.

- It cannot forge the root signature.
- It cannot swap in its own destination key, because the root signature binds
  that key.
- It cannot replay an approved request, because the nonce is consumed.
- It cannot reuse an unused signature after the owner changes policy, because
  the signature binds the policy head.

Remote transport (#787) will carry the same signed envelope. It adds no
authority.

## Familiar identity

Familiars in this codebase have no cryptographic identity key. A familiar is a
local record (ID, name, role). No recovery, introduction or rotation operation
reads or writes familiar state, and the fixtures check this. Any future
familiar key rotation must be its own explicit, separately audited operation.

## Fixtures

`tests/fixtures/access-recovery/mod.rs`:

```sh
cargo test -p coven-cli --bin coven mobile_memory::recovery --locked
```
