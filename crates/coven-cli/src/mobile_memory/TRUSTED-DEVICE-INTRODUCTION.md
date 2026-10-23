---
title: "Local trusted-device introduction checkpoint"
summary: "Exact-action authority, consumption and audit contract for the local Rust introduction seam."
read_when:
  - Reviewing trusted-device introduction authorization
  - Integrating a future enrollment transport
source_adjacent_reason: "Defines the authorization and persistence invariants implemented beside introduction.rs and its exclusive fixture."
---

# Local trusted-device introduction

Tracks #1256 under #788. `IntroductionAuthority` is the Rust authorization
seam. The only ingress is local enrollment (below): owner-socket admission plus
the introducer's approval over the authenticated private-network gateway. No
relay connection or remote enrollment is enabled. Tests use synthetic P-256
keys and disposable local state.

## Trusted inputs and explicit policy

The owner authority supplies the home, the installation's public-key fingerprint
(the pairing-v2 audience), and an explicit `IntroductionPolicy`. These inputs
are trusted local configuration, never fields selected by an enrollment caller.
The policy is immutable for an authority instance. `load_installed` reads it
from the protected owner policy store (below) and re-reads the store head on
every challenge and commit check, so a replacement retires authority instances
and requests bound to the previous policy.

There is no default policy or approval threshold. A policy must specify a
canonical nonempty eligible-introducer list, allowed scopes, maximum grant
lifetime, fresh user-verification or biometric assurance, and maximum step-up
age (1–120 seconds). It must also set an explicit approval threshold N, with
1 ≤ N ≤ min(eligible introducers, 16). There is no default, and the CLI flag
`--required-approvals` is mandatory. A requested threshold is never silently
reduced.

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

## Threshold (N-of-M) introduction

A request names exactly N approvers, and N must equal the policy threshold.

- A **v1** `IntroductionRequest` names one introducer, so it is valid only when
  N = 1. Its transcript is unchanged.
- A **v2** `ThresholdIntroductionRequest` names N ≥ 2 approvers. Each is a
  `{deviceId, grantId, revocationEpoch}`, and the list is strictly sorted by
  device ID. Every approver must be eligible.

v2 hashes under `COVEN-INTRODUCTION/2\0`, so a v1 signature never verifies as a
v2 action. Both versions use the same `devices.introduction.approve` and
`.accept` operations. Both versions are checked by the same
`IntroductionTerms` validation, so they cannot drift.

Each approver obtains its own challenge
(`issue_threshold_challenge`). It signs the same exact action with its
possession key and supplies its own action step-up. `introduce_threshold`
requires exactly one approval per named approver; a missing, extra, duplicate
or unnamed approval is refused before any lock is taken. All N approvals are
checked inside **one** registry commit, and that commit holds every
approver's device record and current authorization key together
(`with_active_keys`). For every approver it checks:

- source state, `DeviceAdmin` and transport
- the possession signature
- step-up freshness against the policy's maximum age at commit time
- assurance
- that the destination grant is no broader than that approver's grant

A single failure aborts the commit. Challenges consumed before the failure are
burned, but the nonce is not consumed, so fresh genuine approvals can still
succeed. Because freshness is checked at commit, all N approvers must act
within the maximum step-up age (at most 120 seconds).

The enrollment ledger collects approvals only from named approvers, once each.
It hides a request from an approver who has already approved, and reports
`approvalsReceived`/`approvalsRequired`. It commits on the N-th approval;
that commit is the request's single, terminal attempt.

**Stale earlier approvals.** When the N-th approval arrives, the ledger first
drops any collected approval whose step-up is already older than the maximum
age. Nothing is verified or consumed. The request stays `awaiting_approval`, and
its status lists those approvers in `stepUpExpired`. The request reappears in
their pending list, and each must approve again with a fresh step-up before the
request expires. Recovery only removes approvals: N, required assurance and every
commit-time check are unchanged, and a step-up time that only claims to be fresh
is still verified at commit. If the request expires first, the owner admits a
new request.

Under N > 1, an eligible introducer can still *tighten* the owner policy
alone. The strictness check never lets that lower N, and loosening requires
the owner-root key. Fixture:
`tests/fixtures/trusted-device-introduction/threshold.rs`.

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

## Protected owner policy store

Tracks #1533. `OwnerPolicyStore` persists the policy in
`mobile/introduction-policy.json` as a hash chain anchored to an **owner-root
credential**: a P-256 public key registered at install. Only the public key is
stored; the private half stays with the owner, outside `COVEN_HOME` and off
OpenCoven infrastructure.

Each record binds the audience, owner-root key, revision, previous digest and
policy. Its digest is SHA-256 of `COVEN-INTRODUCTION-POLICY/1\0` followed by
the JCS encoding of those fields, and it becomes the `effect_digest` of a
`COVEN-ACTION/1` intent (`DeviceAdmin`, audience target, nonce, at most 300
seconds of validity).

- **Install** (`owner.introduction_policy.install`) succeeds only when no policy
  exists, starts the chain at revision 1, and must be signed by the root key it
  registers, proving possession.
- **Replace** (`owner.introduction_policy.replace`) must name the current head as
  its predecessor. Committed and older signed changes therefore cannot be
  replayed or rolled back.
- The **owner-root signature** may approve any valid replacement, including a
  downgrade.
- A **current eligible introducer** may approve a replacement only when the new
  policy is at least as strict on every dimension: introducers ⊆, scopes ⊆,
  approvals ≥, lifetime ≤, step-up age ≤, assurance ≥. It signs
  `devices.introduction_policy.replace` with its possession key, and supplies a
  fresh `COVEN-ASSURANCE/1` action step-up meeting the *current* policy, under
  the same source checks as introduction.
- A downgrade without the root signature is refused. Replacing the root key is
  owner identity rotation, which this store does not implement.

Loads fail closed on an unknown version, another audience, a digest mismatch,
or a broken chain. Root-approved heads re-verify their signature on every load,
so a store edited to widen policy is refused even with a recomputed digest.
Device-approved heads record the approver's device, grant, epoch, key ID and
spent challenge.

An attacker with owner write access to `COVEN_HOME` already controls the
registry and is outside this store's threat model. The store defends against
caller-selected policy, relay or account services, and approval by a device.

Lock order is policy store → device registry → authorization-key registry →
assurance challenge store. An introduction commit checks the head and writes the
enrollment under one policy lock, in that order. A replacement therefore lands
either before the commit, which then refuses the request, or after it, never in
between. Challenge issuance and admission read the head without the lock;
commit is the authoritative check.
Audit uses `introduction_policy_installed`, `_replaced`, `_rejected` and
`_uncertain` with opaque transition IDs only. Success returns after audit
delivery. A committed-but-unaudited head is retried by `reconcile_audit`, which
never re-applies or re-authorizes a change. The fixture is
`tests/fixtures/owner-introduction-policy/mod.rs`:

```sh
cargo test -p coven-cli --bin coven mobile_memory::owner_policy --locked
```

### Owner CLI

`coven device owner-root generate --out PATH` writes a new P-256 PKCS#8 key
(`0600`, never overwriting) and refuses any path inside `COVEN_HOME`.
`coven device introduction-policy install|replace --root-key PATH …` reads the
key transiently and binds to the existing host identity. It never creates one;
run `coven memory mobile enable` first.

Before signing, the command prints every field the action binds:

- operation, scope and audience
- the owner-root key fingerprint
- revision and previous digest
- each policy dimension
- nonce, issued-at, expiry and effect digest

A replacement also names each broadened dimension as a `DOWNGRADE`. Nothing is
signed unless the owner types the exact word `install` or `replace`. A key that
is not the installed root is refused before the prompt. `--yes` skips the typed
word but not the printed review. `introduction-policy show [--json]` prints the
head without key material. Device-approved tightening has no CLI; it needs the
introducer's step-up through the enrollment route. Fixture:
`tests/fixtures/owner-introduction-policy/cli.rs`.

## Local enrollment

`enrollment.rs` connects the seam to two parties who must both act.

**1. Owner admission.** The local owner runs
`coven device introduce --request ENVELOPE`. The envelope holds the new
installation's `IntroductionRequest` and its `devices.introduction.accept`
signature. The command prints every material field of the request:

- operation and audience
- owner policy revision and digest
- introducer device, grant and epoch
- new device ID, key fingerprint, name and context
- grant ID, scopes, minimum assurance, fresh-verification scopes, transport,
  not-before and expiry
- nonce, issued-at, expiry and effect digest

After the owner types `introduce`, the command posts the envelope to the
owner-socket route `POST /api/v1/internal/mobile/introductions`.

Admission loads policy and audience only from the protected store and host
identity. `IntroductionAuthority::admit` re-runs every authority check and
verifies the destination's possession signature, consuming nothing. A
destination that is already enrolled is refused. The TLS gateway has no
admission route, so a relay or account service cannot put a request in front
of an introducer.

**2. Introducer approval.** `GET /api/v1/mobile/introductions` returns
pending requests, with their exact action, only to the device each request
names as introducer. Other devices get an empty list. The introducer app
renders the request, takes a step-up challenge from the existing
`/api/v1/mobile/assurance/challenge` route (same binding), and submits
`POST /api/v1/mobile/introductions/{id}/approve` with its possession signature
and action step-up proof.

Transport authentication requires no scope here; `introduce` enforces
`DeviceAdmin`, eligibility and fresh assurance at commit. Each admitted
request gets one approval attempt. Any failure is terminal (`rejected`, or
`uncertain` when the destination may have committed), so a forged or replayed
approval cannot be retried into success.

**Ledger.** The ledger is in memory and bounded at 16. A daemon restart drops
pending entries, but never committed state. `uncertain` is never presented as
failure. Owner status and cancel routes are `…/{id}/status` and
`…/{id}/cancel`. Fixtures: `tests/fixtures/trusted-device-introduction/enrollment.rs`
and the gateway route tests.

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

Remote (relay) transport, the mobile client's rendering/signing of the pending
request, passkeys, platform attestation, remote recovery and end-to-end
#788 acceptance remain pending. The future
integration must authenticate its channels, enforce transport restrictions,
source audience/policy from local authority and expose uncertain outcomes
without offering automatic retry-as-success. This checkpoint does not close #788.
