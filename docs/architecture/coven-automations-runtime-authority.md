# Coven Automations Runtime Authority: trust decisions

Status: maintainer decisions recorded 2026-10-03; slices 1–3 and 5
implemented, Runtime Authority not yet constructed

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
| 1 | What authenticates the principal who authorizes a run? | The **owner-local OS identity**: the account that owns the daemon, authenticated by the owner-only Unix socket or Windows named pipe (`RequestAuthority::OwnerLocalIpc`). Each run is bound to a durable **owner grant**, the adopted owner command that authorized it, by that command's adoption key and request digest. For a scheduled run, the grant is the command that made its current revision active. |
| 2 | Who issues familiar embodiment bindings and runs the authoritative familiar ledger? | The **Coven daemon**, with a dedicated familiar-binding key, keeping the familiar root and revision ledger in the Coven store. |
| 3 | Who signs Threads automation-authority decisions? | The **daemon**, evaluating the automation-authority profile through the `coven-threads-core` library and signing with a dedicated decision key. |
| 4 | Where are signed terminal observations produced? | **Coven's session executor**, which owns the session's process, signing with a dedicated observer key. coven-runtimes keeps manifests, probes and the registry. |

Decision 1 is the daemon-owned scope decision that coven-threads
`docs/reviews/2026-09-17-maintainer-decisions.md` (Decision 2) requires before
an authenticated, operation-bound principal path may exist. It names what
authenticates the principal and what binds the operation. It does not by
itself reopen the Threads promotion seam (`ward_updated`); that remains a
separate Threads decision.

Decision 2 supersedes #1137's "Coven must not mint roots, provision trust": the
daemon mints familiar roots and signs their evidence with its own
`familiar-binding` key. The rest of that line still holds: Coven shells out to
no verifier, accepts no request-supplied ledger observation, and trusts a
binding only through its own key policy, never the key the binding carries.

## Familiar ledger

These follow from Decision 2 and were decided on 2026-10-03 for slice 3.

- **The verifier belongs to the contract.** The `familiar.embodiment_binding.v1`
  verifier is the `familiar-contract` crate in OpenCoven/familiar-contract,
  tested there against the contract's vectors and against the JavaScript
  reference. Coven pins it by revision, as it pins `coven-threads-core`.
- **Only owner commands change the ledger.** Coven has no committed declaration
  state: `SOUL.md`, `IDENTITY.md` and `ward.toml` are read from disk, and the
  roster can change out of band. So nothing is minted on observation.
  Owner-local commands register a familiar (genesis), adopt its current
  declarations as a new revision, retire it, revoke a revision, or restore a
  retired familiar. The issuer re-reads the declarations and refuses a binding
  when they no longer match the head, so a changed declaration needs an
  explicit adopt before it can be embodied.
- **What a revision declares.** Each revision retains a
  `familiar.identity_bundle.v1` whose components wrap the exact declaration
  text in JSON objects: the identity declaration holds the roster's identity
  fields (`id`, `name`, `displayName`, `role`, `pronouns`, `person`, `coven`)
  and `IDENTITY.md`; the soul declaration holds `SOUL.md`; the Ward declaration
  holds `ward.toml` when there is one. Renaming or re-roling a familiar is
  therefore an identity change. Display-only fields and the workspace path are
  not declared. A familiar without both Markdown files cannot be registered.
- **Bundles are kept exactly.** A bundle's digest covers its retention state,
  so the retained bundle of every revision is stored as built and never
  rewritten. Redacted forms, when they exist, are history only.
- **Identifiers.** A root is `familiar:<32 hex>`, never derived from the roster
  id, which is an alias that can be reused; at most one live root answers to
  it. A revision is `familiar-revision:<root hex>:<position>`.
- **Vocabulary.** The ledger stores the contract's statuses (`active`,
  `superseded`, `retired`, `revoked`). The issuer maps `superseded` to Coven's
  `stale`, and the contract's optional, inclusive `notAfter` to Coven's
  required, exclusive one.
- **Generation.** Every change to a root advances its generation by one; the
  trusted-ledger observation a binding is decided against carries it.
- **Issuance.** A binding is issued only for a live root whose head is active
  and whose declarations still match it. Within one transaction the daemon:
  - reads the head;
  - builds and signs the binding with the `familiar-binding` key;
  - runs the pinned contract verifier against the head's retained bundle and
    a fresh trusted-ledger observation;
  - requires the binding, and every lineage transition it cites, to be signed
    by a `familiar-binding` key the daemon trusted when each was signed;
  - records the binding.

  A rotated key still vouches for the transitions it signed; a revoked one
  stops every binding that cites them. The binding states no `notAfter`, so
  Coven's projection closes the window at the decision time plus the
  300-second freshness bound.

### Ledger commands

Control actions sent to `POST /api/v1/actions`. Every one, reads included,
requires owner-local IPC: revisions hold declaration text, so the transport
guard refuses them over TCP before the store opens, and the ledger refuses
them again for any other caller. Each mutation takes an `adoptionKey`; a
retry with the same key and request replays the first answer, a different
request under the key is `ADOPTION_REPLAY_MISMATCH`, and a refused command
adopts nothing. Commands that change a root's head take `expectedRevisionId`
and answer `REVISION_CONFLICT` when the head has moved.

| Action | Request | Effect |
| --- | --- | --- |
| `coven.familiars.ledger.register.v1` | `familiarId` | Mints a root and its genesis revision from the roster entry and declarations. |
| `coven.familiars.ledger.adopt.v1` | `familiarId`, `expectedRevisionId` | Records changed declarations as the next revision and supersedes the head; unchanged declarations answer `unchanged`. |
| `coven.familiars.ledger.retire.v1` | `familiarId`, `expectedRevisionId` | Retires the root and its active head; the roster id is free to register again. |
| `coven.familiars.ledger.revoke.v1` | `revisionId`, `reason` | Revokes one revision. A revoked head can only be replaced by adopting changed declarations. |
| `coven.familiars.ledger.restore.v1` | `rootId`, `expectedRevisionId` | Restores a retired root with a restoration revision, when no other root holds its roster id. |
| `coven.familiars.ledger.get.v1` | `familiarId` or `rootId` | Every root under the roster id, or one root, with revision history but no declaration text. |

## Signing roles

Each kind of signed artifact has its own role key:

| Role | Signs |
| --- | --- |
| `dispatch-authority` | The `AutomationExecutionBinding` at dispatch, and the receipt-correlated `AutomationReceiptAuthorityEvidence` at settlement |
| `familiar-binding` | Familiar Contract embodiment bindings (decision 2) |
| `threads-decision` | Threads automation-authority decisions (decision 3) |
| `owner-principal` | Authorization requests on the owner principal's behalf (producer remains gated by #1212) |
| `terminal-observer` | Runtime terminal observations (decision 4) |

The execution binding and the receipt authority evidence share one key: both
are the dispatcher's own attestations about the same run, made by the same
component, so a second key would separate no trust. All five roles share one
lifecycle: one current key per role, rotation that closes its window, and
revocation that makes it authenticate nothing.

Existing four-role stores upgrade atomically when the key schema is initialized.
The upgrade preserves every public record, retired and revoked history, and
current private key file. It can roll back with a caller transaction. Adding
`owner-principal` key support does not enable Threads decisions or Runtime
Authority dispatch; those remain gated by their implementation prerequisites.

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
  per-install identifier for the owner, not the numeric uid. Every run binds
  to an owner grant: the adopted owner command that authorized it. That
  command can only have arrived over owner-local IPC, because #1164 refuses
  mutations on every other transport, and the grant records the transport.
  - A manual run's grant is its own run command.
  - A scheduled run is created by the scheduler, so no command arrives with
    it. Its grant is the owner command that made the current revision active:
    `definition.activate.v1`, or a `definition.create.v1` or
    `definition.revise.v1` that leaves the definition active. The grant
    authorizes that revision only: once a revise, pause, disable or tombstone
    supersedes it, its occurrences no longer dispatch under it.

  `operation` names the dispatch kind. `requestId` and `requestDigest` are the
  grant's adoption key and request digest. The scheduler authenticates no one;
  it presents the grant with a nonce and validity window issued for each
  attempt, and replay is checked against the occurrence fence and the attempt:
  the runner refuses a binding that does not match the claimed attempt's
  identity and fence generation. A revision that became active without an
  owner command (a legacy import, a migration or an unversioned update) has no
  grant, and cannot dispatch under Runtime Authority.
- **`familiar`.** The root and identity revision come from the daemon's
  familiar ledger, with the declaration and embodiment digests from the
  binding the daemon issues. Status, revocation, retirement and freshness are
  read at decision time, never cached from definition time, and a familiar
  whose declarations have changed since its head revision gets no binding
  until the owner adopts them.
- **`threads`, `capabilities`, `approval`, `risk`.** These come from the
  signed Threads decision for this exact request: the protected-surface
  manifest digest, requested, granted, denied and degraded capabilities, the
  approval requirement, and the side-effect class. Prompt text cannot declare
  its own risk or grant itself capabilities.
- **`runtime`.** The exact runtime descriptor that dispatch pins.
- **`authentication`.** The binding is signed with the `dispatch-authority`
  key, as is the receipt authority evidence at settlement. Each signed
  artifact carries its role key's `keyId` and `proofRef`, and the #1192
  verifiers check it against the trusted set built from the daemon's key
  records.

## Runtime Authority launches

These were decided on 2026-10-04 for slice 6.

- **The grant is the owner's activation.** #857 lets R0 and R1 run unattended
  only under an explicit narrow grant. That grant is the owner command that
  activates a revision declaring R0 or R1 authority, which is the slice 2
  owner grant. It is bound to:
  - the revision's digest;
  - its familiar and runtime;
  - its declared capabilities and scopes.

  It lapses when the revision stops being current. There is no separate grant
  store.
- **A definition declares its own authority.** An optional `authority` block
  on the routine definition holds the action type, risk class, capabilities,
  scopes and whether the action is proposal-safe. Only owner create and revise
  commands set it, and the definition digest covers it. Its presence opts the
  definition into Runtime Authority. Prompt text never declares risk.
- **v1 launches only what the harness can enforce.** Only an R0
  `analysis.read` grant launches. It runs on claude with only the read tools,
  in plan (read-only) permission mode, in restricted mode with no MCP servers.
  Every other grant gets a no-launch receipt until a scoped enforcement
  exists. A runtime accepts an authority projection only for an envelope it
  enforces, and an environment override such as
  `COVEN_CLAUDE_BYPASS_PERMISSIONS` never widens one.
- **The launch is observed, not assumed.** Each launch shape is a runtime
  envelope. The execution binding pins its descriptor digest as
  `runtime.descriptorDigest`, so the terminal observer knows from signed
  evidence that a session's output is the harness's own event stream, not
  model text that looks like one.
  - **Complete stream.** The observer classifies the stream. A stream that
    accounts for everything yields complete evidence, and a clean R0 run
    settles on its own:
    - an init event within the envelope;
    - only known events;
    - only the envelope's tools;
    - exactly one result;
    - no dropped or redacted output.
  - **Anything else.** Coverage is partial, and the run is held.

## Ordered slices

Each slice is one pull request. Runtime Authority stays unconstructed and
unadvertised until slice 6.

1. **Role keys.** Generate and load the four role keys (`dispatch-authority`,
   `familiar-binding`, `threads-decision`, `terminal-observer`) under
   `COVEN_HOME`, owner-only. Record
   each public key, `keyId`, `proofRef`, producer and validity window in the
   store, with rotation and revocation. Build `TrustedKeys` from the active
   records. Do not ship fixture keys.
2. **Owner grants and the authorization binding.** Record an owner grant
   whenever an owner command makes a revision active (activate, or a create or
   revise that leaves the definition active). Each grant stores the command's
   adoption key, request digest, revision and transport. Derive the principal
   and authorization from the grant, and refuse a revision that has none or
   is no longer current and active at dispatch. Issue the nonce and validity
   for each attempt, and check replay against the fence and the attempt. The
   unversioned run carries no adoption key, so the manual run's grant lands
   with `occurrence.runNow.v1` in slice 7.
3. **Familiar ledger and embodiment bindings.** In two pull requests:
   - **Ledger.** The root and revision ledger with its owner commands,
     retained bundles, signed lineage transitions and trusted-ledger
     observation, as set out under "Familiar ledger" above.
   - **Issuer.** Issue Familiar Contract embodiment bindings signed by the
     `familiar-binding` key, verify every one with the pinned
     `familiar-contract` crate, and map them into the execution binding. The
     contract's own vectors run in its repository.
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

   As built, the observation is written by whichever writer ends the session,
   inside the transaction that records the ending:
   - the event writer's exit;
   - a confirmed timeout or cancellation stop;
   - restart containment recovery.

   A writer that loses the race to end the session observes nothing.
   Plain-text launches (`--print`, codex without `--json`) are the only kind
   today. For them, capabilities, side effects, result and delivery are all
   `unknown`, so the evidence consumer holds those runs. An ending recovered
   after a restart is `ambiguous`, because the exit was not seen. A failed
   observation stores nothing and goes to the daemon recovery log, and the run
   is held. An operator kill records no evidence, so its run is held too. The
   observer key must exist before dispatch, and slice 6 creates it.
6. **Trusted adapter.** First, the runtime envelopes and the structured
   observation that classifies their streams, as set out under "Runtime
   Authority launches" above. Then compose slices 1–5 into
   `AutomationDispatchAuthority` and the verifiers. Sign the execution binding
   and the receipt authority evidence with the `dispatch-authority` key.
   Construct Runtime Authority only for definitions that declare authority,
   launch only what an envelope enforces, apply the #857 approval policy for
   R3 and R4, and advertise the profile only once its conformance passes.
7. **Held commands, then approvals.** The five versioned commands #1054
   holds for #857:
   - `occurrence.runNow.v1`;
   - `occurrence.cancel.v1`;
   - `attempt.cancel.v1`;
   - `attempt.retry.v1`;
   - `occurrence.recover.v1`.

   Decided on 2026-10-05:
   - **Commands first.** These commands come first. The approval lifecycle
     waits for an envelope that can write, because v1 launches only the R0
     read envelope, and an approval would unlock nothing.
   - **runNow.** An owner's `occurrence.runNow.v1` for a routine that
     declares authority is a one-run owner grant, exactly like an
     activation. It is not a per-run approval.

   `occurrence.runNow.v1` is implemented for ordinary routines: it plans,
   claims and dispatches a manual occurrence. Its one-run grant for a routine
   that declares authority follows with the adapter.

   `occurrence.cancel.v1` and `attempt.cancel.v1` are implemented. Work not
   yet dispatched is cancelled at once, and dispatched work is stopped through
   the run cancellation.

   `attempt.retry.v1` is implemented for ordinary runs. A known failure with
   no automatic retry left holds its run `running` while the original deadline
   is open and fewer than ten attempts have been used. The occurrence retains
   `claimed` after a pre-ownership refusal or `running` after a started attempt.
   You can open the next attempt by naming the exact failed attempt and its
   disposition. The command replans that occurrence without rewriting the
   failed attempt or extending the deadline.

   Release retry quarantine explicitly before retrying. A pending cancellation
   or unresolved stop fence blocks both retry and cancellation of a held
   failure. Stop-lease expiry alone never proves success or authorizes retry.
   Unknown outcomes enter recovery; conclusive successful base completion still
   wins a losing cancellation. Once the deadline passes, a held failure settles `failed`, even
   if its provisional session has been deleted. Under `overlap: forbid`, the
   held run blocks the next occurrence until it settles. Within the retry
   window, `occurrence.cancel.v1` can release a hold with no unresolved stop.
   Retrying a Runtime Authority run follows with the adapter.

   `occurrence.recover.v1` is implemented. The owner settles a
   `recovery_required` occurrence `failed_deterministic`, or opens its next
   attempt with `retry_with_new_attempt`. Recovering a Runtime Authority run
   needs authority-evidenced receipts, so it follows with the adapter.
8. **Certification.** #858 Runtime Authority certification, then SDK Phase 3
   authority-bearing commands.
