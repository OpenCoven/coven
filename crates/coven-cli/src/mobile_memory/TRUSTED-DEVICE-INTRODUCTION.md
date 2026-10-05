---
title: "Local trusted-device introduction checkpoint"
summary: "Exact-action authority, consumption and audit contract for the local Rust introduction seam."
read_when:
  - Reviewing trusted-device introduction authorization
  - Integrating a future enrollment transport
source_adjacent_reason: "Defines the authorization and persistence invariants implemented beside introduction.rs and its exclusive fixture."
---

# Local trusted-device introduction

Tracks #1256 under #788. `IntroductionAuthority` is a local Rust authorization
seam; no CLI command, daemon/API route, relay connection or remote enrollment
is enabled. Tests use synthetic P-256 keys and disposable local state.

## Trusted inputs and explicit policy

The owner authority supplies the home, the installation's public-key fingerprint
(the pairing-v2 audience), and an explicit `IntroductionPolicy`. These inputs
are trusted local configuration, never fields selected by an enrollment caller.
The policy is immutable for an authority instance; a future live policy manager
must serialize policy replacement with in-flight enrollment and retire old
instances. This checkpoint does not implement such a manager.

There is no default policy or approval threshold. A policy must specify a
canonical nonempty eligible-introducer list, allowed scopes, maximum grant
lifetime, fresh user-verification or biometric assurance, and maximum step-up
age (1–120 seconds). The caller explicitly selects exactly one approval.
Zero, N-of-M and other thresholds are refused. A requested threshold is never
silently reduced. Full threshold execution remains part of #788.

The current source must have `DeviceAdmin`, an active device/grant, and a
separately enrolled active authorization key. Revocation, suspension, grant
replacement and authorization-key epoch changes are checked from disk under
locks, including after a challenge was issued. Source `DirectOnly` restrictions
are refused because this seam has no verified transport evidence. A future
transport must enforce its own restrictions before calling this seam.

## Exact authorization

`IntroductionRequest` binds the instance audience, policy digest, source
principal/grant/epoch, destination pairwise ID and canonical P-256 X9.63 key,
complete destination grant, human-readable device name and context, nonce,
issued-at and expiry. The request lifetime is at most 300 seconds. The
context is display material, never device-identity evidence.

Canonicalization reuses the repository's JCS policy encoding and exact-action
contract. `effect_digest` is SHA-256 of `COVEN-INTRODUCTION/1\0` followed by the
JCS request bytes. The `COVEN-ACTION/1` intent has `DeviceAdmin`, the audience as
target, and the same nonce/time bounds. The two possession operations are:

- `devices.introduction.accept`: the destination signs the canonical action
  using its possession key, proving ownership before registry admission.
- `devices.introduction.approve`: the introducer signs the canonical action
  using its current possession key.

The introducer also supplies the existing `COVEN-ASSURANCE/1` **action** proof
for that exact approval intent. It binds the server-issued challenge, current
device/grant/revocation epoch and authorization-key identifier. The challenge
store additionally binds the authorization-key epoch. P-256 signatures use
canonical base64url DER encoding. The authority computes effective assurance
from the verified signature and enrolled key class; the proof's requested
assurance is never trusted as evidence of a biometric ceremony. Platform
protection retains the existing enrollment-time trust model, not new attestation.

Both signatures and the step-up proof must verify. The step-up cannot predate
the request, exceed the configured age or outlive the request/challenge. New
requests cannot replay an old step-up signature. Audiences and policy digests
are recomputed/compared against trusted inputs. Unknown fields, noncanonical
keys/scope sets, malformed requests and incomplete proofs fail closed.

The destination grant must have an explicit expiry, fit the owner lifetime
limit and source expiry, and be no broader than both owner-allowed and current
source scopes. It cannot weaken source minimum assurance or per-scope fresh
verification requirements. Its revision starts at zero. The destination gets
no authorization key implicitly; grants requiring step-up cannot satisfy that
requirement until a separate approved key-enrollment ceremony exists.

## Commit, replay and restart

Lock order is device registry → authorization-key registry → assurance challenge
store. Each uses a process mutex plus the existing interprocess file lock.
Source state and the current key remain locked through verification and the
registry replacement. Other lifecycle writers cannot revoke, suspend or replace
authority between its check and commit.

After cryptographic validation, the server challenge is consumed durably.
Enrollment, nonce consumption and the introduction audit outbox are then written
in **one** existing atomic registry replacement. A failed write may burn a
challenge without enrolling anything. The caller must obtain fresh approval;
challenge consumption alone never proves enrollment.

Any failure after registry replacement begins reports an explicit uncertain
outcome, never inferred success or a definitive rejection. A successful registry
commit followed by audit failure reports committed/audit-pending. The public
method returns enrollment success only after audit delivery and its receipt.
A durable grant can exist while the call reports audit-pending/uncertain; callers
must not interpret any error as proof that no enrollment occurred.

`reconcile_audits` reads validated persisted outbox records, retries their audit
append and persists receipts. It does not re-enroll, authorize a request, restore
revoked devices or infer state from a matching display name/cached record.
Replayed enrollment calls are refused even when audit delivery is pending.

The consumption ledger is bounded at 128 introductions and fails closed when
full. It is not pruned, including by existing lifecycle writers and `forget_all`.
Deleting registry state is an owner trust-domain reset, outside replay guarantees.
No automatic cleanup removes anti-replay evidence. Existing registries without
introduction records remain readable. Older binaries with a closed registry
schema reject the new field; rolling back must preserve this state and use a
compatible reader, never delete the ledger to make an older binary accept it.

## Audit and remaining integration

Events distinguish `device_introduced`, `device_introduction_rejected` and
`device_introduction_uncertain`. Successful introduction events contain only
an event, timestamp and random transition ID. They contain no keys, signatures,
nonces, device/grant IDs, device names/context, recovery material or familiar
identifiers. Audit append is idempotent by transition ID within the retained
existing audit-log window. Audit reconciliation is not access recovery or
familiar identity rotation.

The exclusive fixture is `tests/fixtures/trusted-device-introduction/mod.rs`,
compiled into the CLI unit tests. Run it with:

```sh
cargo test -p coven-cli --bin coven mobile_memory::introduction --locked
```

Transport integration, UI rendering of every signed material field, protected
policy installation/replacement, passkeys, platform attestation, N-of-M,
remote recovery and end-to-end #788 acceptance remain pending. The future
integration must authenticate its channels, enforce transport restrictions,
source audience/policy from local authority and expose uncertain outcomes
without offering automatic retry-as-success. This checkpoint does not close #788.
