# Native Automations readiness

Coven has a durable native scheduler, but it is not yet certified for
identity-bound unattended automation. Use this source-adjacent map to separate
implemented safeguards, executable audit coverage, and the remaining release
gates in #854.

From a clean, committed checkout on Linux or macOS with Node.js 24 and the Rust
build prerequisites installed:

```sh
node conformance/automations/runner/audit.mjs \
  --output /tmp/coven-automations-audit
```

Choose an unused output directory outside the checkout. The command builds the
native target with locked dependencies, packages the source-bound protocol,
and runs the reviewed suite inventory through the independent audit runner.
It retains the job, result, and protocol artifacts without changing tracked
files. Repository build and audit inputs come from one private checkout of the
pinned commit, not mutable working files. The host toolchain/configuration is
caller-trusted, not isolated or attested; byte-for-byte build reproducibility
is not assessed. Source repository metadata is derived from configured GitHub
`origin`, not proof of remote commit membership. A missing suite cannot silently disappear because the target stops
advertising it. See the [audit runner contract](../../conformance/automations/runner/README.md)
for the artifact and platform limits.

**A passing audit is not release certification.** Its decision scope is
`audit_only`; the base and authority manifests remain `releaseState: proposed`
and `productionReady: false`. A profile result describes only its enumerated
suites, not every acceptance criterion assigned to that profile.

## Implemented baseline

Reviewed on 2026-09-14 against Coven `a566c26d2bf7`, before the audit-entrypoint
and terminal-recovery conformance additions in this change:

| Surface | Implemented evidence | Still missing |
| --- | --- | --- |
| Definitions and scheduling | Durable revisions, adopted requests, UTC/IANA schedules, DST, latest-only misfire, occurrence fences, overlap refusal | Executable rich-definition and versioned command parity, complete lifecycle publication (#1054) |
| Scheduler recovery | Persisted retry/backoff, quarantine, stop-fence cancellation/timeout arbitration, leadership fencing, startup and definition-change wake | Complete crash-boundary matrix, storage fault injection, measured load/retention limits (#858) |
| Portable audit | Ten structural and nine scheduler-reliability suites in the native target; versioned vectors and independent audit-result runner | Full authority, continuity, privacy, interoperability, and release-eligibility evidence (#858) |
| Authority | Exact dispatch-binding validation/persistence seam and bounded runtime projection | Trusted production Familiar/Threads/principal/approval/runtime adapters (#857) |
| Receipts | Immutable base receipt and authority sidecar commitment, atomic run link/event, verified reopen, owner-local base read | Launched-session authenticated observation producer, accepting verifier, evidence consumption and terminal receipt construction (#857) |
| No-launch evidence | Dispatcher-controlled refusal of an authority-unaware runtime can commit a truthful no-launch receipt through the explicit authority seam | This is not proof of a launched runtime's effects, and the production authority adapter remains unavailable |
| Terminal uncertainty | Authority-bound terminal sessions without consumed trusted evidence remain on recovery hold, without receipt fabrication or automatic retry | Authorized operator reconciliation and authenticated complete/partial/unknown evidence consumption (#857, #858) |
| Operator visibility | Routine health, retry/quarantine details, scheduler status, occurrence reads, and event reads | Principal-filtered evidence, comprehensive explain/doctor coverage, retention and incident runbooks (#857, #858) |

The implementation source is under
[`crates/coven-cli/src/automations/`](../../crates/coven-cli/src/automations/).
The [receipt delivery map](../architecture/coven-automations-receipt-delivery.md)
owns the distinction between authorization, execution, and verification.

## Progress added by this revision

- A repeatable native audit runs every suite in the checked-in inventory,
  binds the result to source/protocol/runner/vector/binary digests, and retains
  machine-readable evidence. Linux PR CI runs it as a required Rust-job step
  and uploads the evidence, including non-passing results. Source-isolated
  build caches, native binary architecture, provenance integrity, and actual
  symlink entrypoint execution are part of the audit boundary. Toolchain trust
  and remote membership are explicitly unverified, not implied by those
  digests; trusted release/build attestation remains under #805.
- The `runtime-authority-terminal-recovery` suite makes the existing
  fail-closed reconciliation boundary independently executable in five
  terminal-observation cases, with repeated settlement and store reopen. It
  seeds a correlated post-launch fixture rather than launching a harness.
  It does not provide a production authority adapter, consume authenticated
  terminal evidence, or certify authorized execution.
- #1054 now explicitly owns executable contract work deferred when #855
  closed. Closed specification issues no longer hide this implementation gap.
- OpenCoven/coven-runtimes#48 now owns the missing authenticated terminal
  observation producer. Registry/descriptor signing is not execution evidence.

The audit result, not this prose, records the exact suite inventory and source
revision that ran. Native target tests also run on Windows; the standalone
runner remains unsupported there until kill-on-close process containment is
implemented. Unsupported is not passed.

## Certification coverage boundaries

| Profile | Native portable coverage in this revision | Remaining acceptance examples |
| --- | --- | --- |
| Structural | Definition parsing/lifecycle, RRULE vocabulary, adoption replay/conflict, occurrence uniqueness, terminal immutability, event reduction, receipt integrity, capability negotiation | Full supported command matrix, randomized operation sequences, typed transport/domain parity, packed consumer compatibility |
| Scheduler reliability | Calendar/DST, misfire, lease recovery, overlap, retry/backoff/quarantine, leadership fencing, startup/wake, cancellation/timeout arbitration | Process kill at every durable/external boundary, database busy/I/O/corruption, delivery failure, bounded load and retention |
| Runtime Authority | Terminal-evidence recovery hold only | Authenticated production dispatch, revocation/approval races, runtime capability downgrade, signed terminal producer and trusted verification |
| Continuity | No native portable profile advertised | Exact familiar root/revision, alias ambiguity, historical rehydration, and direct/Psyche correlation |
| Privacy | No native portable profile advertised | Principal-aware history/changefeed access, redaction/omission, retention/erasure, leakage tests on actual producer/consumer paths |
| Interoperability | No native portable profile advertised | Current immutable SDK/Cave/Psyche/Familiar/Threads/runtime artifacts, real receipt reads, reconnect/replay, installed-package canaries |
| Full/release | Intentionally unavailable | All profiles at one exact compatibility set, measured SLOs, supported-platform evidence, trusted release authentication and go/no-go |

Existing unit and integration tests can cover more than the portable inventory.
Do not count that broader coverage as an independently certified profile until
the corresponding exact-artifact evidence is included.

Platform investigations #1047, #1050, and #1051 remain separate release-evidence
risks; later green CI does not establish their cause or resolution. #1053 was
resolved by #1057, which separates positive-process hang guards from the
unchanged deadline contracts and proves the hostile output was actually
observed. Keep unresolved observations visible rather than loosening product
deadlines or privacy guards to make a readiness run pass.

#1055 separately records an inherited `COVEN_MAINTENANCE_PARTICIPANT` entering
a different disposable test repository. Its maintenance guard correctly refuses
the mismatched participant. Run those fixtures without the ambient participant;
don't weaken production validation to make them pass.
#1056 records four client lifecycle fixtures whose checkout-relative socket
paths exceed macOS `SUN_LEN` in a long worktree. A short worktree isolates that
test limitation; it is not a production socket-policy change or a source fix.

## Dependency-ordered remaining work

| Order | Owner | Exit evidence |
| --- | --- | --- |
| 1. Executable protocol parity | #1054 | Tested command matrix; rich-definition persistence without silent field loss; revision/adoption/fence-safe commands; transactional lifecycle events; migration/rollback and reconnect vectors |
| 2. Trusted dispatch | #857, consuming OpenCoven/familiar-contract#17 and OpenCoven/coven-threads#29 | Production adapters resolve fresh authenticated principal, exact familiar revision, operation-specific grant/approval and runtime descriptor; stale/revoked/replayed/changed bindings cannot launch |
| 3. Observed terminal truth | #857, OpenCoven/coven-runtimes#48 | Signed durable complete/partial/unknown observations, producer-key history, exact session/run/attempt/binding correlation, verified consumption, atomic receipt/sidecar/event commitment, restart replay |
| 4. Privacy and operations | #857, #858 | Authorized receipt/history/changefeed projections, explicit redaction/omission, safe recovery decisions, storage-failure/crash matrix, bounded retention, and measured SLO thresholds |
| 5. Consumer acceptance | OpenCoven/sdk#80, OpenCoven/coven-cave#5217, OpenCoven/psyche#18 | Immutable producer artifacts consumed by real read/verify/subscribe and execution paths; no legacy-log fallback masquerading as receipt verification |
| 6. Release evidence | #858, #805, OpenCoven/coven-docs#76, OpenCoven/.github#2 | All required profiles at one compatibility set, packed/installable-artifact canaries, supported platform matrix, release authentication, operational docs, canonical agent bootstrap/check interfaces, and exact-source go/no-go packet |
| 7. Bounded demonstration | #937 | Disabled-by-default disposable caretaker; verified receipt/UI states; protected-action refusal or proposal; no external mutation; deterministic rollback to the initial fixture digest |

Steps 1 and 2 can advance independently. Step 3 needs trusted runtime evidence.
SDK read-only APIs, Cave degraded/unverifiable projections, documentation, and
consumer canaries can advance now against the existing capability-negotiated
reads and pinned fixtures. Full authority-bearing commands and authenticated
receipt acceptance still need the production trust path. Step 7 is not a
shortcut around steps 2 through 6.
No unattended real-repository, release, deployment, merge, or publication
automation is enabled by this work.

## Upstream and consumer evidence

Reviewed GitHub main branches and open PRs on 2026-09-14:

| Owner | Immutable delivered evidence | Next boundary / existing work to reuse |
| --- | --- | --- |
| OpenCoven/familiar-contract#17 | Profile at `13d150a32a81` | Validator requires caller-supplied trusted ledger state; production fresh-state acquisition remains #857 integration work |
| OpenCoven/coven-threads#29 | Profile at `c3bd46bcadb6` | Reference approval/signature semantics are not a deployed authority provider; production key/policy/runtime snapshots and durable consumption remain necessary |
| OpenCoven/coven-runtimes#48 | Reviewed main `2be31ec0b38e`; no terminal emitter found | Existing OpenCoven/coven-runtimes#45 and OpenCoven/coven-runtimes#46 concern probes/registry publication, not signed terminal observations |
| OpenCoven/sdk#80 | Base artifact canary at `942aaea71bc0`; OpenCoven/sdk#248 integrity hardening merged at `99fa244a6f88` | Reuse the merged canary work. It still pins the historical producer artifact; public read/verify/subscribe and current-producer certification remain distinct |
| OpenCoven/coven-cave#5217 | Projection/reducer slice at `8a25d8e0950f` | Reuse bounded daemon projection; wire live reads/changefeed before claiming authority/receipt oversight |
| OpenCoven/psyche#18 | Generic `CovenPort` at `1e47b40dbbb9` | Automation invocation/result adapter, adopted execution correlation, and cancellation/restart contract remain |
| OpenCoven/coven-docs#76 | Existing automation reference at `6127ea1d46ba` | Reconcile implemented/unsupported command and operator behavior now; don't wait to correct stale public reference text |
| OpenCoven/.github#2 | Reviewed main `c8b4ad3f9f97` | Coordinate with active OpenCoven/.github#7 governance work; reusable immutable-artifact/profile evidence enforcement is not yet landed |

No consumable production terminal emitter or trusted binding resolver was found
in those reviewed source boundaries. This is not a claim about private or
unpushed implementations. Record an immutable implementation and acceptance
canary before removing either blocker.

## Tracker interpretation

#816, #855, #856, #859, OpenCoven/familiar-contract#17, and
OpenCoven/coven-threads#29 were closed when reviewed. Their delivered foundation
or profile artifacts are prerequisites, not proof that #854's broader gates
passed. #857, #858, and #937 remain open; #1054 records the deferred executable
contract work.

The [Beads mapping](coven-automations-v1.md) is a reviewed 2026-09-03 graph
snapshot. Its generated dispositions are not a live readiness report. This
source assessment does not rewrite the canonical Cave Beads/Dolt database or
pretend it has been synchronized. Reconcile that graph through its designated
writer and drift check before the final #854 rollup, including child-work
tracking for #1054 and OpenCoven/coven-runtimes#48. A passing check of the
existing mapping does not prove those new work items have been provisioned.
