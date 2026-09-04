# Coven Automations Runtime Authority v1

`coven.automations.authority.v1` is a separately advertised companion profile
for the frozen `coven.automations.v1` contract. It does not create a whole
protocol v2 and does not change any byte or meaning under
`spec/coven-automations/v1/`.

The companion envelope is transported under the exact key
`AutomationRun.extensions["coven.automations.authority.v1"]`:

| Envelope member | Meaning |
| --- | --- |
| `executionBinding` | Required immutable `AutomationExecutionBinding` before dispatch. |
| `receiptEvidence` | `null` only before settlement; required after a terminal base receipt exists, then authenticated as the receipt-correlated `AutomationReceiptAuthorityEvidence` sidecar. |

A generic base-v1 consumer preserves this value as opaque JSON and never
interprets it. Preservation alone does not establish Runtime Authority
conformance. A Coven deployment claiming that conformance explicitly
advertises both profiles plus `automations.runtime-authority.v1`, requires the
companion envelope on each authoritative run, validates its closed
draft-2020-12 schema, exact-matches it against the trusted dispatch snapshot,
independently verifies integrity and authentication, and refuses dispatch or
evidence projection when the value is missing, malformed, stale, replayed,
mismatched, or unverifiable.

Ed25519 public-key DER and 64-byte signatures use lowercase hexadecimal in
the conformance inputs and contract objects.

Every field that references `capabilitySet` is a mathematical set serialized
as a unique JSON array. An implementation must preserve each signed object's
array order when computing or verifying its JCS bytes, but compares capability
members without regard to order when correlating trusted state, bindings,
receipt evidence, and runtime descriptors. `denied` remains an ordered array
of capability/reason records and exact-matches as serialized.

The base-v1 `AutomationReceipt` schema intentionally remains byte-for-byte
unchanged and has no `extensions` member. Therefore receipt authority evidence
is not injected into that frozen object. It is a companion sidecar inside the
run's authority extension and must exact-match the referenced base receipt.
It carries the authenticated base receipt digest; a terminal receipt with
missing, mismatched, or unverifiable authority evidence fails closed.

## Contract objects

- `AutomationExecutionBinding` is the immutable, one-attempt dispatch snapshot.
  It pins the base definition/occurrence/run/attempt anchors, authenticated
  principal, replay state, familiar root/revision/status, authorized projection
  identifiers, familiar validity/revocation/retirement checks, Threads decision
  and protected-surface manifest, capability and approval decisions, risk,
  exact runtime selection, policy/profile versions, privacy/redaction
  classification, producer, timestamp, integrity, and authentication.
- `AutomationReceiptAuthorityEvidence` is the minimized receipt projection
  carried beside the binding in the authority extension. It
  preserves the authenticated base receipt digest plus exact binding,
  authorization, capability, approval, risk, runtime, and decision correlation
  without copying credentials,
  prompts, memory content, familiar declarations, unrelated audit records, or
  unrestricted filesystem paths.

Only `permit` and satisfied `requires_approval` decisions can produce an
execution binding. `degrade_to_proposal` and `reject` authorize no dispatch.
Approval and decision consumption are replay-safe and bind the exact
definition, occurrence, attempt, fence, familiar embodiment, principal,
capability set, runtime descriptor, policy, and protected-surface manifest.

Per-run approvals are single-use: an approval identifier already present in
the trusted consumed set is refused. A `bounded_recurring` approval instead
binds the upstream Threads recurring grant identifier, `maxUses`,
`occurrencePrefix`, and trusted `priorUses`. Dispatch is allowed only when the
occurrence identifier starts with that prefix, `priorUses < maxUses`, and the
current consumption has `usageNumber == priorUses + 1`. The authenticated
consumption commits the request and decision digests, occurrence and run
identifiers, attempt number, and fence generation; those values must
exact-match the authorization and base dispatch anchors in both the binding
and receipt evidence. Replaying that exact tuple is refused. Receipt evidence
repeats the exact approval and consumption snapshot so it can be correlated to
the dispatched binding.

Dispatch chronology is
`issuedAt <= validFrom <= decisionTimestamp <= dispatchNow < validUntil`.
The authorization end is exclusive. Familiar verification must not follow the
decision or dispatch and must use the exact trusted freshness policy version
and bound. This profile permits bounds from 0 through 300 seconds; an age equal
to the bound is valid, while an age strictly greater than the bound is stale.

All values must be I-JSON-compatible before schema validation or RFC 8785
canonicalization. Implementations recursively reject unpaired UTF-16
surrogates in string values and object member names; malformed Unicode cannot
be signed, verified, or dispatched. Negative vectors encode invalid UTF-16 as
ASCII mutation instructions and construct the invalid value only in memory, so
the published `test-vectors.json` remains strict I-JSON and parses with `jq`.

Replay validation is phase-specific. Before dispatch, a nonce, adoption key,
per-run approval identifier, or recurring consumption tuple already present in
trusted committed state is refused. Terminal verification requires those
records to be present instead: the nonce and adoption indexes, the applicable
approval-consumption index, and exactly one dispatch ownership record must all
identify the signed binding, occurrence, run, attempt, fence, and approval
consumption. Missing records are unverifiable; overlapping records owned by a
different dispatch are mismatches. A terminal receipt never turns a consumed
authorization back into a fresh authorization.

## Normative inputs

- Familiar continuity and embodiment:
  `OpenCoven/familiar-contract@13d150a32a817da19bb4e5053f2205b15db0bb0a`
- Protected-action authority and approvals:
  `OpenCoven/coven-threads@c3bd46bcadb6396db8436c47411a4d0eac17192b`

`upstream-artifacts.json` pins the SHA-256 and byte length of the reviewed
Familiar schema/manifest/automation vectors and Threads
manifest/decision-schema/authority vectors at those commits. Consumers use
those canaries to detect a moved tag, substituted export, or locally copied
parallel contract.

This profile consumes those semantics without copying ownership into Coven.
Rust remains the runtime authority. The TypeScript declaration is a pinned
projection only.

## Validation seams

`scripts/validate-automations-authority-profile.mjs` exports
`negotiateAuthorityProfile`. Generic base-v1 consumers return the complete
extensions object unchanged and never inspect unknown profiles. Runtime
Authority consumers must advertise the base profile, companion profile, and
`automations.runtime-authority.v1` before the companion is parsed.

The Rust projection lives in
`crates/coven-cli/src/automations/contract/authority.rs`. Its
`validate_authority_profile` boundary performs closed typed projection,
chronology, familiar freshness at the signed decision, approval/base
correlation, digest, receipt-correlation, and capability negotiation checks,
then requires an `AuthorityEvidenceVerifier`. The deployment adapter also
checks familiar freshness at dispatch and is the deliberately narrow future
dispatch seam for trusted Familiar, Threads, replay, approval, runtime, and
signature evidence. A missing adapter returns
`AUTHORITY_ADAPTER_MISSING`; unavailable trusted data returns a typed refusal
instead of falling back to the base run's string references.

No scheduler or runtime launch path calls this seam in this slice.

## Artifact

CI packages this directory independently as
`coven-automations-authority-v1-contract-<source-commit>.tar.gz` using the same
canonical USTAR, normalized gzip, tracked-file, manifest, and content-digest
rules as the base bundle. The embedded manifest uses
`coven.contract-profile.bundle.v1` and names this companion profile.

Artifact `9909975069`, source commit
`8a796807b37d4ad33eaeca37498debf1ca55dd49`, bundle SHA-256
`512460db71d4257d7a4d33ea306578e66d9ac499d9384eb9c2b8e2b4e2e32363`,
and base contract-content SHA-256
`3c145eb92a93426ed64631f6487a8cd12903b0a49a6e752269f594ac50a779f5`
remain historical base-v1 evidence. They are never reused or relabeled as an
authority-profile artifact.
