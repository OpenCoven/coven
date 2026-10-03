# Coven Automations Runtime Authority: trust decisions

Status: maintainer decisions recorded 2026-10-03; implementation not started

Tracks: #857 (dispatch authority, receipts), #1137 (familiar identity),
OpenCoven/coven-runtimes#48 (terminal observations), #858 (certification)

Runtime Authority binds each automation run to an authenticated principal, an
exact familiar revision, a Threads authorization decision and a runtime
descriptor, and settles a launched run only from authenticated terminal
evidence. The contract, the immutable sidecar, the Ed25519 verifiers (#1192)
and the evidence consumer (#1193) have landed. Production still constructs no
Runtime Authority, because four producers had no owner. These are the
maintainer's decisions on them.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | What authenticates the principal who authorizes a run? | The **owner-local OS identity**: the account that owns the daemon, authenticated by the owner-only Unix socket or Windows named pipe (`RequestAuthority::OwnerLocalIpc`). The operation is bound by the adopted command's adoption key and request digest. |
| 2 | Who issues familiar embodiment bindings and runs the authoritative familiar ledger? | The **Coven daemon**, with a dedicated binding key, keeping the familiar root and revision ledger in the Coven store. |
| 3 | Who signs Threads automation-authority decisions? | The **daemon**, evaluating the automation-authority profile through the `coven-threads-core` library and signing with a dedicated decision key. |
| 4 | Where are signed terminal observations produced? | **Coven's session executor**, which owns the session's process, signing with a dedicated observer key. coven-runtimes keeps manifests, probes and the registry. |

Decision 1 is the daemon-owned scope decision that coven-threads
`docs/reviews/2026-09-17-maintainer-decisions.md` (Decision 2) requires before
an authenticated, operation-bound principal path may exist. It names what
authenticates the principal and what binds the operation. It does not by
itself reopen the Threads promotion seam (`ward_updated`); that remains a
separate Threads decision.

## Trust model

These decisions make the local daemon, running as the owner's OS account, the
trust root. Each signing role has its own key, so a key can be rotated or
revoked without touching the others, and every key is checked by the #1192
verifiers against explicit validity windows and revocation.

What this protects against:

- a client, a stale definition or a replayed request launching a run the
  principal did not authorize at dispatch time;
- a run executing under a familiar revision that has since changed, been
  revoked or been retired;
- a terminal receipt claiming effects that the executor did not observe;
- a non-owner process (loopback TCP, another account) authorizing anything:
  only owner-local IPC authenticates.

What it does not protect against:

- **A compromised owner account.** Any process running as the owner can use
  the owner-local channel, and can read keys that are readable by the owner.
  This is why the #857 risk policy still applies: R3 and R4 runs need a per-run
  or tightly scoped approval, and only R0/R1 may become unattended, and only
  under an explicit narrow grant.
- **Effects the executor cannot see.** With the `claude_cli` provider, tool
  calls reach Coven only as prose. Exercised capabilities and effects are then
  `unknown`, the evidence is partial, and #1193 keeps such a run on recovery
  hold rather than settling it. Structured tool events, where a harness
  provides them, can be recorded as observed.
- **Multiple or remote principals.** One local owner is the only principal.
  Per-principal signing keys or an external identity provider are later
  decisions.

## How each decision fills the execution binding

`AutomationExecutionBinding` (`automations/contract/authority.rs`):

- **`principal`, `authorization`.** `principalId` is a stable, opaque,
  per-install identifier for the owner, not the numeric uid. The
  authentication state is `authenticated` only when the command arrived over
  owner-local IPC; anything else refuses. `operation`, `requestId` and
  `requestDigest` come from the adopted command. The nonce and validity window
  are issued at dispatch, and the replay state comes from the adoption ledger.
- **`familiar`.** The root and identity revision come from the daemon's
  familiar ledger, with the declaration and embodiment digests from the
  binding the daemon issues. Status, revocation, retirement and freshness are
  read at decision time, never cached from definition time.
- **`threads`, `capabilities`, `approval`, `risk`.** These come from the
  signed Threads decision for this exact request: the protected-surface
  manifest digest, requested, granted, denied and degraded capabilities, the
  approval requirement, and the side-effect class. Prompt text cannot declare
  its own risk or grant itself capabilities.
- **`runtime`.** The exact runtime descriptor that dispatch pins.
- **`authentication`.** Each signed artifact carries its role key's `keyId`
  and `proofRef`, and the #1192 verifiers check it against the trusted set
  built from the daemon's key records.

## Ordered slices

Each slice is one pull request. Runtime Authority stays unconstructed and
unadvertised until slice 6.

1. **Role keys.** Generate and load three Ed25519 keys (binding issuer, Threads
   decision signer, terminal observer) under `COVEN_HOME`, owner-only. Record
   each public key, `keyId`, `proofRef`, producer and validity window in the
   store, with rotation and revocation. Build `TrustedKeys` from the active
   records. Do not ship fixture keys.
2. **Owner principal and authorization binding.** Derive the principal and
   authorization binding from owner-local IPC and the adopted command. Refuse
   every other transport. Bind the nonce and validity, and check replay
   against the adoption ledger.
3. **Familiar ledger and embodiment bindings.** Build a root and revision
   ledger from the familiar roster, with declaration digests and status,
   revocation and retirement. Issue Familiar Contract embodiment bindings
   signed by the binding key, and run all 87 Familiar Contract vectors against
   the issuer.
4. **Threads decisions.** Evaluate the automation-authority profile in Rust:
   risk class, capability grant, denial and downgrade, the approval
   requirement, and degrade-to-proposal. Put the evaluator in
   `coven-threads-core` and bump the pinned revision. Sign decisions with the
   decision key, and run all 130 profile vectors.
5. **Terminal observer.** The session executor records a durable observation
   before publication: exit, timeout or cancellation, and capabilities,
   effects and results as observed-complete, observed-partial or unknown,
   never derived from grants. It signs the observation with the observer key
   and is idempotent across restart.
6. **Trusted adapter.** Compose slices 1–5 into `AutomationDispatchAuthority`
   and the verifiers. Construct Runtime Authority only for definitions that opt
   in, with the #857 approval policy for R3 and R4, and advertise the profile
   only once its conformance passes.
7. **Approvals and held commands.** Implement the approval lifecycle, then the
   five versioned commands #1054 holds for #857. These are
   `occurrence.runNow.v1`, now compatibility-only, and the unsupported
   `occurrence.cancel.v1`, `attempt.cancel.v1`, `attempt.retry.v1` and
   `occurrence.recover.v1`.
8. **Certification.** #858 Runtime Authority certification, then SDK Phase 3
   authority-bearing commands.
