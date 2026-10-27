---
title: "Automations command acceptance"
summary: "Exact-release behavioral acceptance matrix and evidence gaps for issue 1054."
read_when:
  - Extending SDK or Cave Automations release canaries
  - Reconciling command parity and Runtime Authority certification
source_adjacent_reason: "Maps the native command matrix and protocol artifacts to consumer acceptance tests and authority gates."
---

# Automations command acceptance

Status: **incomplete**, inspected 2026-10-19. Owners: Coven #1054 (command
parity), #857 (authority), #858 (certification), SDK #80 and Cave #5217
(consumer paths). This packet does not authorize Runtime Authority activation
or replace any original acceptance criterion.

## Source and release reconciliation

Coven main `365013d54a86923d230753d94510821f48d5e492` implements all **19/19
base command names** in `crates/coven-cli/src/automations/command_matrix.rs`.
The historical 15/19 checkpoint is superseded. The matrix lists every command
in `command-envelope.schema.json`, so it now also carries the three mission
commands (`mission.submit.v1`, `mission.get.v1`, `mission.cancel.v1`; Coven
#1313, #1347), which are outside this packet's 19-command scope. A supported command name does
not establish support for every payload variant, wire form or authority profile.
`command_envelope.rs` now routes envelope `run.cancel.v1` through the correlated
cancel executor (`execute_run_cancel_envelope`, Coven #1323). It hashes the
envelope request in its own digest domain, so envelope and flat adoptions can
never replay one another. Its typed refusals are recorded in the
`run.cancel.v1` row below. This reconciles source behavior only; do not claim
19/19 executable envelope coverage, published consumer coverage or release
eligibility from it.
Authority-bearing manual grants, retry and recovery still follow the trusted
adapter described in `docs/architecture/coven-automations-runtime-authority.md`.

Selected published producer: OpenCoven/coven release `v0.4.10`,
annotated tag resolving to `d6afbffde4f815a538f65185c80da2d0ee951d2d`.
Downloaded base protocol asset:

- Asset ID: `641273453`; size: `42929` bytes.
- Name: `coven-automations-v1-contract-<sourceCommit>.tar.gz`, with the exact source commit above.
- SHA-256: `bd0d97ea257d851cfd45338ca56d62d485a75c1911e4bddead53ab2fb722fa1e`.
- Contract content SHA-256: `528b900cde6db1dda7e5ae62affa0db30cd56c440e42532d7d00c75504f86e4a`.
- Verified downloaded bytes against the GitHub asset digest; verified all 19
  manifest file sizes/hashes and byte equality with the exact producer commit.
  Release `SHA256SUMS` covers native archives only, not this protocol bundle.

The pre-repin SDK snapshot `d19fdbe3912c012b3730932d5a03eff10ba3d32d` locked producer
`c93a8a936f9b7f1bd070a1cbb608d38d4e13c92a` (v0.4.7), content
`ef266d16d76d7380f5cd2b30a110b2e84f1310b154b262e0ade9aa6bf60b3445`.
Six files differ at the selected release: `README.md`,
`command-envelope.schema.json`, `common.schema.json`,
`compatibility-matrix.json`, `event-envelope.schema.json`, and
`state-machines.json`. Re-pin through the SDK artifact verifier, including
producer workflow/job/archive provenance; do not replace only the digest or
substitute release asset metadata for the lock's CI artifact fields.
SDK #348 delivered this re-pinning, as recorded below. Complete SDK/Cave
run-control acceptance remains outstanding.

## Consumer boundaries

SDK `scripts/verify-automations-v1-daemon.mjs` executes definition mutations,
replay/conflict cases and reconnect, but merely checks advertisement of the
five newer run-control commands. SDK #80 gates its approval/retry/cancel API
phase on #857/#858. Do not expose these public methods just to satisfy a test.
A proposed base-only test transport must retain the public API gate and be
reviewed as such before it can count as SDK consumer evidence.

The earlier Cave snapshot `47b0cd43de11864b1189683c0b0a024318ecf3eb`,
`scripts/coven-automations-release-canary.ts`, drives unversioned
create/update/delete/get/list/runs and versioned event subscriptions. Keep this
compatibility canary. It is not v1 command-envelope mutation coverage.

Cave's release fixture at that commit already pins v0.4.10. The earlier
lifecycle-controls ownership record (OpenCoven/coven-cave#5217, comment `6246083218`)
covered the release refresh, canonical reads and revision-aware controls on
`feat/issue-5217-lifecycle-controls`. The newer delivered evidence is recorded below. Preserve owner coordination
and do not create a competing Cave implementation.
GitHub issue/PR discovery establishes this same-work match; private execution
liveness was not inspected. Run controls remain outside that continuation's
scope and gated on #857/#858/SDK #80.

Each row below needs separate SDK and Cave result records. An advertised name,
a Rust unit test, or a successful direct socket request cannot fill either
consumer result field. Use public supported consumer paths; record a missing
path as a gap. No row has complete SDK-and-Cave evidence for every column; partial SDK
release evidence appears in the checkpoint below.

## Behavioral matrix

This is the required acceptance matrix, not a claim of shipped consumer coverage.
The selected v0.4.10 release refuses all six run-control commands below over
the envelope with `CAPABILITY_UNSUPPORTED`. Its positive cases describe flat
versioned actions. Newer source support is recorded separately below; it does
not change the released artifact or establish published consumer coverage.
Verify that each released envelope refusal leaves execution state unchanged.

Use isolated daemon homes and disposable projects. Collect IDs from actual
responses, never seed lifecycle state by editing SQLite. Capture requests,
typed responses, exact definition revision/integrity, event IDs/sequences,
occurrence/run/attempt correlations and consumer checkpoints. Redact private
paths and prompts. Never use real external effects to create fixture failures.

| Command | Positive case | Refusal/conflict case | Duplicate/restart proof |
| --- | --- | --- | --- |
| `definition.create.v1` | Rich draft revision 1; JCS integrity round-trip | Unknown variant, unsupported rich field, created-active definition | Same adoption replays one definition/event before and after restart; changed wire body conflicts |
| `definition.revise.v1` | Draft to paused, revision +1; historical rich body retained | Stale revision; illegal lifecycle change; unsupported field | Exact replay preserves original revision/digest after restart |
| `definition.activate.v1` | Paused to active through adoption transaction | Disabled/tombstoned reactivation; stale revision | One activation/event; restart replay cannot reactivate newer state |
| `definition.pause.v1` | Active to paused | Stale revision; invalid source state | Replay adds no revision/event after restart |
| `definition.disable.v1` | Supported source state to disabled | Stale revision | Replay preserves terminal lifecycle and history |
| `definition.tombstone.v1` | Disabled definition tombstoned without deleting history | Stale revision; forbidden transition; envelope rejects reason | Replay after restart retains historical pins |
| `occurrence.runNow.v1` | Flat action: ordinary active routine creates and dispatches one manual occurrence | Ineligible lifecycle; overlap; stale revision; unauthorized context | Same adoption yields one occurrence/dispatch, including lost-response replay after restart |
| `occurrence.cancel.v1` | Flat action: cancel undispatched work; request stop for dispatched work | Wrong revision; unresolved stop fence; deadline precedence | One cancellation history; replay through restart never reports unacknowledged stop as success |
| `run.cancel.v1` | Source (#1323): the envelope and the flat versioned action cancel the occurrence's running run, correlated to its current attempt and session; the released v0.4.10 envelope returns CAPABILITY_UNSUPPORTED | Envelope: NOT_FOUND for an unknown run; ILLEGAL_TRANSITION for a run that is not its occurrence's running run, or a run with no occurrence (#1358, fixed by #1409 as `365013d5`). Flat action covers missing run/stale context/completion race | Flat duplicate stop preserves original evidence; an envelope replay returns its stored terminal result (also after restart) or its stored rejection; a stored `recovery_required` result still replays after a later pass settles the run `failed` on terminal session evidence (#1457, #1461); a new flat request naming a held run's latest failed attempt releases the run `cancelled` as the envelope does, so a replayed `occurrence.cancel.v1` delegation no longer depends on which side of its commit the daemon died, while an `attempt.cancel.v1` delegation still refuses (#1486); a refusal writes no execution state |
| `attempt.cancel.v1` | Flat action: stop the named current attempt | Old attempt or old dispatch fence; blank reason handled per contract | Duplicate/restart replay cannot stop a replacement attempt |
| `attempt.retry.v1` | Flat action: held known failure, original deadline open, exact prior attempt/disposition; new attempt dispatches | Ambiguous prior outcome; expired deadline; ten-attempt cap; quarantine; pending cancel/stop fence | Same adoption opens one attempt; prior attempt and cancellation history remain immutable after restart |
| `occurrence.recover.v1` | Flat action: separate recovery-required occurrences exercise failed_deterministic and retry_with_new_attempt | Wrong occurrence state; stale revision; authority run lacking authenticated evidence; a run cancellation still being reconciled (#1463) | Replay preserves determination and creates at most one new attempt across restart |
| `definition.list.v1` | Unpaginated listing; include tombstones through lifecycleState=all | Envelope rejects limit, cursor and lifecycle filters other than all | Repeated reads and restart preserve ordering without writes |
| `definition.get.v1` | Exact current rich body and digest | Missing/tombstoned ID according to contract; envelope rejects revision selection | Restart preserves revision/integrity and historical attribution |
| `run.history.v1` | Keyset pagination and occurrence filter over actual runs | Invalid cursor/filter | No missing/duplicate records while reconnecting |
| `definition.health.v1` | Health reflects authoritative lifecycle/failure | Missing definition | Read after restart reflects durable state, not cached success |
| `events.read.v1` | Definition and occurrence/run/attempt events ordered by cursor | Invalid/expired cursor has explicit typed outcome | Resume checkpoint across restart; no gaps or duplicate projection |
| `events.subscribe.v1` | Consumer observes committed lifecycle transitions | Disconnect/backpressure/expired cursor remains explicit | Duplicate/out-of-order replay cannot regress projection; reconnect uses retained checkpoint |
| `legacy.import.v1` | Dry-run then adopted import creates draft; duplicate source report agrees | Unsupported source; changed adoption body | Replay/restart does not duplicate import; unversioned import remains paused |

For every mutating row, exercise changed JSON under a reused adoption key and
stale expected revision wherever the command accepts it. A refusal must leave
execution state unchanged; distinguish a contract-required retained rejection
adoption from an illicit state mutation. No generic assertion of "zero writes"
may erase that distinction.

Run control requires a deterministic runtime fixture that can announce launch,
hold, fail conclusively and stop. Synchronize on explicit events, not sleeps.
Restart the actual published daemon over the same isolated home. Distinguish
clean restart replay from crash/fault injection; neither proves the other.
Recovery-required fixture construction must use a supported lifecycle path. If
a published consumer cannot produce the state safely, mark the case missing
and retain native evidence separately; do not manufacture a passing result.

## Authority and certification gates

Threads PRs #82 and #84 were rechecked open and unmerged on 2026-10-18.
#857's cancelled #1212/#1217 drafts do not supply a production adapter.
On 2026-10-18 Val selected a Coven-owned scoped Rust read service as the
filesystem boundary to prepare for review. The reviewed replacement contract,
implementation/enforcement proofs and merged dependency pins remain required. After adapter delivery, repeat run-now/retry/recovery with:

- Fresh principal, exact familiar revision, scoped capabilities, runtime
  descriptor, original definition/adoption and stop/dispatch fence bindings.
- One-run manual grant and operation-specific approval semantics; stale,
  revoked, expired, replayed and widened-scope refusals before launch.
- Authenticated terminal receipts; unknown/ambiguous outcomes held for explicit
  recovery; no receipt fabricated from exit status.
- Independent receipt verification and direct/Psyche interoperability under
  #857/#858, including privacy and retention checks.

Do not count ordinary base execution as authority evidence or an audit-only
suite as production certification.

## Completion packet still required

1. Exact-source 19-command matrix plus explicit supported variant limits.
2. Rich revision/digest persistence and historical binding evidence, retaining
   the digest fix and old-event compatibility behavior.
3. Atomic event/state publication with injected event-write failure and crash
   boundaries; no false terminal success, replay-safe cursors.
4. Release-store migration/rollback proof from the existing v0.4.3/v0.4.6
   fixtures, paused legacy imports, draft v1 imports, unchanged historical pins
   and no implicit activation. Retain the evidence delivered by #1190; verify
   it against the final source rather than assuming the old test result holds.
5. Re-pinned SDK/Cave artifact provenance and a per-row positive/refusal/
   conflict/duplicate/restart record from exact published daemon bytes.
6. Authority adapter and run-control evidence after the gates above, plus the
   #858 independent certification packet and #854 go/no-go disposition.

Each result must identify producer commit, native/package integrity, protocol
bundle/content hashes, consumer commit and packed package integrity, platform,
invocation, observed outcome, retained artifact location and proof limits.
Failed attempts remain visible beside reruns. Record final evidence on #1054;
keep it open while any original acceptance item is missing or unverified.

## Active delivery checkpoint (2026-10-18)

SDK PR #348, source `d6a75fe41b09977f0ee890e3d05c7080abcb863a`, now aligns the protocol
lock, CI reproduction and npm daemon fixture to v0.4.10. It merged as
`785f2a69cf0e8b621a69a4757a0866896b98f497` in
OpenCoven/sdk#348.
Live provenance verification passed for CI
artifact `11868030042`, producer run `38505518198`, job `115662069315`;
the CI-contained bundle is byte-identical to the GitHub release bundle above.
Npm verified registry signatures and attestations for both installed packages.
The 19 focused artifact-evidence/workflow-pin tests passed.

The existing SDK daemon scenario passed against the installed release:
`daemonStarts=2 commands=9 replays=2 rejections=2 events=5 subscribePages=2`.
Definition integrity matched the SDK, lifecycle event digests matched definition
integrity, adoption replay and checkpoint resume both passed across restart.
The peer-identity proof remains `harness-asserted`. This scenario does not send
run-control commands and cannot satisfy those matrix rows.

The normal SDK suite passed 3,160 tests with two existing skips. Coverage
exposed a hanging-daemon fixture that closed its response on client EOF. The
corrected fixture keeps the response half-open and waits for request receipt
before interruption; its 35 focused tests pass. All 3,160 coverage tests and
thresholds then passed, as did typecheck, stress properties and lint. Packed
package, contract and release-configuration checks passed. Independent review
found no remaining findings. SDK #348 is merged; its existing daemon scenario
still does not execute run-control commands.

The SDK artifact canary also passed manifest integrity, receipt-definition
fixture binding, typechecking, duplicate delivery, out-of-order refusal and
reconnect replay. Result-envelope authentication remains unperformed by that
canary. Independently packaging the exact release source reproduced the
manifest and bundle byte-for-byte. The first artifact-canary invocation
mistakenly supplied the CI ZIP where the command expects the inner tarball;
its digest rejection was expected, and the corrected tarball invocation passed.
The first reproduction invocation omitted required SOURCE_COMMIT and failed;
the corrected invocation supplied the exact release commit. Neither failure
was suppressed or treated as producer behavior evidence.

## Source delivery and consumer checkpoint

Keep these source changes separate from the selected published v0.4.10 daemon:

| Surface | Delivery evidence | Remaining proof |
| --- | --- | --- |
| Retry/recovery envelopes | Coven #1312 merged as `0207d4a21bf5939959b203e87c409b5f1b209f12` | Authority-bearing behavior and exact published consumer execution |
| Occurrence/attempt cancellation envelopes | Coven #1316 merged as `60ed9cf2d55bdc1b4b7ca9c9b7bf43c546d893d7` | Exact published consumer execution and authority acceptance |
| Run cancellation envelope | Coven #1323 merged as `24f487738ec42d69b980c2ecf5588c425b8135b0`; #1358's no-occurrence refusal merged as `365013d54a86923d230753d94510821f48d5e492` (#1409) | Exact published consumer execution and authority acceptance |
| Run-now envelope | Coven #1335 merged as `7508fb2b8fb428479efcee1f01898a4bd3304e8d` | Exact published consumer execution and authority acceptance |

#1335 retains immutable dispatch results and typed refusals, validates original
attempt/session correlation, and refuses ambiguous replay without relaunching.
Its local workspace, lint, Policy guard, and 22-suite native audit passed. The
audit scope is `audit_only`; it does not prove production authority or released
SDK/Cave behavior. #1323 and #1335 overlapped in the router and portable
vectors and landed sequentially on main.

Cave PR OpenCoven/coven-cave#6077 merged as
`e7a5cc602a07dc996f4a5263a9386c1c3729a358` from head
`5d37fbbaf840a30d5420e1a3bec246160f8478ba`. Its
owner-recorded canary (Cave #5217, comment `6250720884`)
against v0.4.10 reports two daemon starts, ten mutations, three history pages,
four lifecycle commands, typed revision conflicts, run-now failure reporting,
one occurrence/attempt, six feed events, and restart/resume checks. This is
released flat compatibility evidence, not run-control envelope or Runtime
Authority coverage. Cave #5217's owner has continued overview occurrence-state
work on `feat/issue-5217-row-occurrences`; preserve that active lane.

The scoped Rust read service design from Coven #1308 remains a review proposal.
Run-now still dispatches with `AutomationAuthorityMode::BaseV1`; owner grants
currently cover definition create/activate/revise, not an adopted manual run.
Manual authorization, trusted adapter integration, exact grants, approval and
revocation handling, verified receipts, Threads #82/#84 human review and merged
pins, and #858 certification remain required. Do not close #1054 from the base
command count, audit results, or flat compatibility canaries alone.
