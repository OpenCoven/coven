---
title: "Automations doctor implementation plan"
summary: "Read-only, minimized operator diagnostics and their verification for issue 1355."
read_when:
  - Implementing Automations doctor
source_adjacent_reason: "Task execution plan for the Rust authority layer and owner-local CLI."
---

# Automations Doctor Implementation Plan

> **For agentic workers:** Use executing-plans to implement this plan task by task. GitHub issue #1355 owns status and delivery evidence; this file describes implementation only.

**Goal:** Implement the complete doctor observation taxonomy requested by #858 without inferring health from missing evidence or enabling authority operations.

**Architecture:** A Rust report builder reads a coherent store snapshot and emits a closed, minimized DTO. A dedicated owner-local control action returns it in `result`, with no lifecycle event. The CLI validates the DTO and prints only checked fields before normal shared-state initialization. Missing instrumentation produces explicit unobserved categories and safe next steps, and remains an acceptance gap until the category has sufficient evidence.

**Tech Stack:** Rust, rusqlite, serde, chrono, existing identity-bound coven-client IPC, disposable daemon integration tests.

## Task 1: Observation contract and failing tests

Files: new `crates/coven-cli/src/automations/doctor.rs`; register in `automations/mod.rs`; new `tests/automations_doctor.rs`.

- [ ] Enumerate closed categories: leadership, last successful pass, schema/integrity/migrations, queue counts, leases/fences, planning/start lag, repeated failures/quarantine, authority/identity/approval refusals, delivery/receipt failures, event/changefeed health, disk/retention pressure.
- [ ] Map each field to actual production schema/instrumentation. `diagnostics.rs` retains only the last attempted pass: a failed latest pass cannot supply last-success time. `health.rs` and `inspection.rs` contain raw private fields: never serialize these types directly.
- [ ] Define observed, unhealthy, invalid and unobserved results with fixed finding/next-step codes. No aggregate compliant/healthy flag that conceals gaps. Do not invent missing measurements or ratify thresholds.
- [ ] Add tests for empty store, each unhealthy category, missing observations, malformed bounded metadata and private sentinels. Run the focused test and retain a real failing behavior before implementation.

## Task 2: Coherent minimized report

Files: `doctor.rs`; schema definitions only if necessary and explicitly reviewed.

- [ ] Build read-only aggregate queries in one transaction. Inspect table/index definitions first; bound text extraction in bytes (including NUL handling), numeric ranges and output cardinality. Never materialize all histories or scan private logs.
- [ ] Use explicit field validity and observation provenance. Persisted leader metadata is not process liveness. Counts cannot prove zero silent loss or absence of external effects.
- [ ] Report schema observation without triggering migration. A default expensive SQLite integrity scan is not acceptable; label the precise limited check and leave full integrity unverified unless a bounded explicit contract exists.
- [ ] Do not add incidental OS/process/path probes or application-data writes. SQLite WAL/SHM reader coordination can create sidecars and update read marks; zero filesystem writes are not promised. Missing disk/retention telemetry is unobserved, not zero pressure. Determine what production instrumentation is needed before claiming that category complete.
- [ ] Fixed next steps may direct owner inspection and documented escalation; no blind retry, raw SQL, fence clearing, or destructive repair.
- [ ] Verify all table projections remain byte-for-byte unchanged across the report, and malformed metadata never leaks private values through errors.

## Task 3: Owner-local control action

Files: `control_plane.rs`, `automations/command_matrix.rs`, `automations/transport_authority_tests.rs`.

- [ ] Register `coven.automations.doctor.v1` following the result-only lease action pattern once #1351 lands; do not rewrite that reviewed pending branch.
- [ ] Return a versioned DTO in `result`, `event: null`; refusal text is fixed. Enforce owner-local admission and refuse TCP even with ordinary client credentials.
- [ ] Add action tests proving complete before/after state equality and no checkpoint publication, scheduler wake, migration, renewal, recovery or dispatch.

## Task 4: CLI and actual daemon

Files: new `automations/doctor_cli.rs`, `automations/mod.rs`, `main.rs`, `tests/automations_doctor.rs`.

- [ ] Add `coven automations doctor [--json]`, bypass shared-state initialization alongside operator inspection commands.
- [ ] Discover only identity-bound owner IPC. No autostart, client SQLite or TCP fallback; missing daemon must leave missing home absent.
- [ ] Validate envelope/action/result schema, closed category/code sets, count ranges, timestamp semantics, fixed recommendations, duplicate observations and bounded arrays before printing.
- [ ] Use disposable daemon with terminal synthetic fixtures so asynchronous startup reconciliation cannot legitimately mutate the baseline. Verify TCP refusal, minimized JSON, no private sentinels and full table equality. Readiness comes from explicit events, not short sleeps.
- [ ] Native Windows tests remain distinct from standalone independent-runner containment.

## Task 5: Runbooks and delivery

Files: source-adjacent operator guide or existing canonical documentation chosen after ownership inspection; owning issue #1355.

- [ ] Explain every code/category, safe next steps, observation limits, unavailable categories, and ambiguous-effect escalation. Do not publish unimplemented repair commands as available.
- [ ] Review the implementation against every #858 doctor bullet. Missing telemetry remains explicit incomplete work; a DTO category alone does not satisfy the operational requirement.
- [ ] Run format, workspace/all-target Clippy with warnings denied, full workspace locked tests, complete Policy guard, secrets, staged and committed-range privacy/documentation ownership checks.
- [ ] Obtain independent source review; fix required findings with verified regressions. Only then create signed/DCO commit with provenance and no tool byline.
- [ ] Push a scoped PR, inspect exact-head CI and fresh review state, preserve queue protection, verify fetched-main contents after landing, record evidence, release claim and retire only owned clean worktree/branches.

## Current coordination

Starting base: `f393990c672da7e06a15defa4b5d2ac29163ef30` (#1331 merged). Claim `issue-1355` belongs to this task worktree. #1351 has landed on main with bounded lease paging. Preserve the current staged and unstaged doctor work, then integrate current main after the active regression build terminates; retain the operator, lease and missions CLI registrations. #857 gates still prohibit unapproved production authority activation. Full #858 profiles, golden scenarios, fault/security/privacy matrix, SLO ratification, platform/consumer canaries and authenticated exact-release eligibility remain open.

## Queue semantics implementation seam

The eligible count must share `occurrences.rs::eligible_occurrences_for_active_definition_ids` predicates with scheduler claiming. The query includes latest-only selection, active definition validation or an admitted retry, quarantine exclusion, active occurrence/run overlap exclusion, retry `not_before`, and timeout exclusion. A doctor-only approximation of these predicates is not acceptable. Factor the fixed candidate query so an aggregate can count its result without materializing every occurrence; retain bounded row selection for the scheduler and inspection callers. Cover equivalence with latest-only, quarantine, overlap, delayed retry and elapsed timeout fixtures before adding an eligible fact. A stored due count must have its own explicit definition and must not be labeled eligible.

The default report must not silently run a full integrity scan. Schema/migration facts need an explicit expected-schema contract; page count and table count are not evidence of integrity. Independent malformed categories should become minimized invalid observations while preserving unaffected observations, rather than preventing all diagnostics. These remain implementation requirements.

## Queue implementation checkpoint

The current WIP factors the scheduler candidate relation into
`occurrences.rs::ELIGIBLE_OCCURRENCES_SQL`. Paging retains ordering and its
100-row limit; the doctor total streams only 65-byte timestamp projections from
the same relation, validates each selected timestamp, and counts all candidates.
Existing active-definition validation supplies the same definition set. Its
current implementation materializes definitions; this checkpoint does not claim
constant memory for definition validation or measured load/SLO bounds.

The report now distinguishes stored states, due planned rows, and eligible
scheduler candidates. Runtime Authority admission remains explicitly unproven.
The regression includes more than 100 candidates, latest-only history, future
rows, inactive/quarantined definitions, claimed overlap, retry admission and
not-before/timeout boundaries, running overlap, and malformed timestamps.
The queue tests now run in the actual doctor Cargo module. Extracted native
probes remain focused historical evidence; they do not replace workspace gates.


## Recorded fence and migration checkpoints

The read-only fence observation now shares the exact stale-claim predicate with
recovery. It counts mismatched claimed daemon generations (including NULL and
future generations), excludes any associated run, and does not filter expiry.
It returns numeric metadata only; it never constructs a scheduler fence or
acquires leadership. Missing/released authority omits the count, invalid metadata
preserves independent categories, and DTO validation requires the mismatch count
to fit within the lease-bearing total. Independent review identified that subset
bound; a red/green forged-report probe and Rust regression cover the correction.

The store category now reads the existing contract-migration ledger for the
production profile. A valid record contributes its six historical counters and
bounded timestamp, a missing record omits them, and malformed counters or time
produce a minimized metadata gap. These counters describe the migration event,
not current verifiability. No initialization or repair is invoked. Expected-schema
compatibility, integrity, event-baseline/timezone migrations and other required
store evidence remain incomplete. This checkpoint does not satisfy the complete
store category or full doctor acceptance.

The task worktree is based on `d8bfb864bd152bb8935226c8e269320436eea1cb`.
Pre-integration staged/unstaged work is retained in stash
`454555fefdd65323c44cf4d899bed8efb65481d1` and explicit patch/hash backups.
Before the migration addition, 759 Automations tests and all three actual-daemon
integration tests passed, including JSON/human CLI output and all-table equality
under a writer reservation. The focused migration regression passed; subsequent
current-source focused checks and full delivery gates are separate evidence.


## Current-main and schema-presence checkpoint

The task worktree now includes main `1450f97f` (run cancellation, read-only
conformance and mission conformance). Retained pre-integration stash:
`13c0d1a1de87e359c3d44aeeaeafad8c2b9a74b4`. Fifteen task files were
byte-identical afterward; transport tests also retain the incoming run-cancel
case. The earlier locked workspace failure in mobile_pairing_control is retained;
its inherited nonblocking accepted socket cause is isolated in issue #1361.

The schema component started with 73 fixed table/index/trigger presence checks.
It now also compares bounded known per-object forms and separately counts
unexpected Automations namespace objects. Main-schema PRAGMA pinning prevents
TEMP shadows from substituting metadata. Missing successful-pass history and
contract-migration ledger tables have separate unavailable outcomes that retain
independent observations and do not recreate storage. The actual doctor Cargo
suite passes 38 tests, including the missing-table regressions and all four
fixture histories. This does not replace full workspace verification or prove
remaining dependencies, missing-column behavior, broader query-failure isolation,
whole-schema compatibility or integrity. #1363 passed exact-head CI and landed through the merge queue as
`b912dc5b`. Fetched-main patch accounting matched the reviewed three-line change;
its archived worktree/branches and claim are cleaned up. The doctor worktree
has integrated that revision. The subsequent full workspace, Clippy and Policy guard passes are historical snapshots: newer query-isolation changes require fresh final verification.


## Schema-form review evidence (in progress)

The matcher feeds bounded counts into the store observation; the 38-test
doctor suite verifies the current behavior. The readable
`crates/coven-cli/src/automations/doctor-schema-form-review.json` records the
73 object names, 85 distinct forms, four fixture histories, and source fixture
hashes. The separate hash manifest is a draft, not an approval record.
Matching individual object forms does not establish a coherent supported schema,
SQLite integrity, current migration eligibility, or runtime authority.

Regenerate the raw observation using the production-initialization capture test:

```sh
COVEN_DOCTOR_SCHEMA_CAPTURE_PATH=/tmp/coven-doctor-schema-capture.json \
  cargo test -p coven-cli --bin coven --locked \
  automations::doctor::tests::capture_supported_schema_forms_from_production_initialization
node scripts/check-automations-doctor-schema-forms.mjs /tmp/coven-doctor-schema-capture.json
```

The guard verifies the capture against the review artifact and manifest; it
never rewrites the contract. Review changed definitions and their production
owners before updating accepted forms. A local macOS capture provides no proof
for other platforms or bundled SQLite versions. Unexpected-object counting has
a defined Automations namespace scope; it does not inspect arbitrary unrelated
application objects. Keep remaining dependencies, query failures and whole-store
integrity as separate acceptance requirements.


## Query isolation checkpoint (in progress)

The focused doctor suite passed 44 tests after independent main-schema
prerequisites were added for successful-pass history, migration-ledger columns,
receipt and event tables, latest-pass duration, and latest-pass lag. The lag
regression covers 18 combinations of missing timestamp columns, duration
availability/validity and TEMP shadows. Unavailable numeric evidence is omitted;
valid independent facts remain and concurrent gaps retain their defined order.

Latest-pass status/table/identifier isolation then passed the 45-test focused
suite, including an 18-case missing-status/history/TEMP-shadow matrix. Independent
source review found no required findings. Other transitive
query dependencies, authority/claim and lease-state helper separation, complete
doctor taxonomy and final delivery remain open. These focused tests do not
refresh the earlier full-workspace or guard evidence and do not establish
release certification.


Authority metadata is now independent of claim-mismatch counting. The doctor
suite passed 47 tests: missing authority prerequisites retain independent lease
counts, and missing claim generation retains assigned leadership and valid lease
expiry evidence while omitting the unavailable mismatch count. Preflight and
observation queries target main; the bounded private owner stays inside the
read helper and never becomes a SchedulerFence or diagnostic field. Independent
source review found no required findings. All 46 occurrence tests passed after
main qualification of the shared recovery predicate. Other shared dependencies,
full taxonomy and final full gates remain open.


Delivery-state counts now independently require the main runs delivery_state
column. The focused suite passed 48 tests after a four-case matrix for missing
delivery state, receipt availability and TEMP shadows. Two main receipt rows
remain visible despite a one-row TEMP shadow; unavailable delivery counts are
omitted and concurrent receipt loss has an exact ordered gap pair. Independent
source review found no required findings. Other run-table and shared query
prerequisites, complete taxonomy and final full gates remain open.


Failure-class prerequisites are now independent of failed-attempt state counts.
The focused suite passed 49 tests, including missing/present classification with
and without TEMP shadows: main has two failed attempts and one each of the known
launch-refused/unsupported-authority classes. A conflicting TEMP count cannot
substitute its evidence. Missing classification omits both refusal facts with
an explicit gap; it does not invent zero refusals. Independent source review
found no required findings. Other attempt/eligibility dependencies and complete
identity/approval refusal instrumentation remain open.


Retry exhaustion history now has an independent main-column prerequisite. The
focused suite passed 50 tests, including missing/present counters with TEMP
shadows: two main quarantined routines remain visible with missing counters;
valid main history count two and maximum three cannot be replaced by TEMP 999.
Present counters retain all-row numeric validation. Independent source review
found no required findings. Other retry/quarantine/eligibility dependencies,
complete taxonomy and final full gates remain open.


Eligibility now preflights all six direct/transitive relations and their 24
required columns in the read snapshot. Missing prerequisites omit only eligible
count with an explicit gap when independent report queries remain readable.
Canonical eligibility SQL, definition/mission/quarantine readers, direct queue
counts and lease summary use main. The 46-case missing-dependency matrix retains
planned/due/claimed and active lease evidence; six positive TEMP-shadow cases
retain eligibility one. A minimal declared-projection schema executes the real
canonical reader to catch undeclared SQL dependencies.

Verification: 804 Automations tests passed, including 53 doctor/CLI tests;
coven-cli all-target Clippy with -D warnings, formatting and whitespace checks
passed. Independent source review found no required findings. An initial
missing-occurrence-ID fixture could not be seeded under foreign keys; fixture
construction now temporarily suspends enforcement, restores it before doctor,
and asserts it remains enabled. Other shared missing state/schedule/quarantine
queries, bounded definition-reader resources and full taxonomy remain open.
These scoped checks do not refresh historical full-workspace or policy gates.


Due schedule prerequisites now have independent main preflight. Missing
scheduled_for omits due and eligible counts with an exact ordered gap pair,
retaining four state counts and lease evidence. Two shadow/non-shadow cases
retain planned two, claimed one and active lease one. The DTO now also enforces
present due <= planned and eligible <= due, including partial observations;
missing counts remain omitted. Regression first demonstrated the validator
accepted contradictory planned zero/due one. The focused suite passed 55 tests;
coven-cli all-target Clippy -D warnings, formatting and whitespace passed.
Independent source review found no required findings. The previous 804-test
Automations pass predates these changes; missing state/quarantine and other
shared dependencies, full taxonomy/resource bounds and final full gates remain
open.


Quarantine now independently requires the main quarantined_at column. Missing
metadata retains failed-attempt and valid exhaustion evidence; concurrent
unavailable/invalid history has exact ordered gap pairs. Eight cases cover
history modes and TEMP shadows, retaining failed attempts two and valid history
two/max three. Missing retry storage is not recreated. The focused suite passed
56 tests; coven-cli all-target Clippy -D warnings, formatting and whitespace
passed. Source review found no required findings. Shared state/lease/attempt
and malformed-metadata isolation, full taxonomy/resources and final full gates
remain open.


Failed-attempt counting now independently requires main attempts.state. Missing
column/table preserves readable quarantine/history/refusal facts. A closed
ordered failure-gap grammar composes independent missing sources and invalid
history, preserving old shapes without enumerating every combination. The
24-case matrix covers missing state/table, quarantine availability, counter
validity and TEMP shadows; duplicate/reversed/extra gaps and fabricated failed
zero are rejected. Missing attempt storage is not recreated. Review caught an
omitted optional-count declaration; it was fixed and independently rechecked.
The corrected focused suite passed 57 tests and coven-cli all-target Clippy
-D warnings, formatting and whitespace passed. Other occurrence-state/lease
and malformed-data isolation, resources, full taxonomy and final gates remain
open.

Lease expiry partitions now scan independently from occurrence state. Missing
owner/expiry/table omits the whole cohort; missing state retains expiry facts
and omits outside-state counting. Independent authority and claim preflights
retain measurable fence evidence, including mismatch counts when only expiry
is missing. Missing occurrence state/table produces explicit unknown queue
facts. A closed ordered lease-gap grammar rejects fabricated zero and reordered
gaps. The 32-case matrix covers missing dependencies, assigned/released/invalid/
unavailable authority and TEMP shadows, without writes, private output or table
recreation. Review found that a forged state-unavailable report could retain a
measured mismatch count; its regression reproduced the failure before the
validator fix. The wider Automations suite passed 809 tests, including 58 focused
doctor checks. Full taxonomy, malformed-data coverage, bounded whole-read
resources and final workspace/policy/protected delivery gates remain open.

### Eligibility materialization budget design (in progress)

Before invoking the canonical scheduler definition reader, inspect numeric byte
lengths only for the exact main-schema non-tombstoned/non-mission definition
rows that it materializes, plus quarantined retry IDs. Bound each relation to
256 rows (read at most one overflow sentinel) and the combined text inputs to
1 MiB. Include incidental record fields, not only definition_json; the canonical
reader loads those fields too. Keep the preflight and canonical read in the same
transaction. TEMP tables cannot satisfy or bypass the preflight. No truncated
or partial eligibility count is allowed: exceeding the budget omits eligibility
and emits eligibility_read_budget_exceeded while retaining independent counts.
The limits bound diagnostic input materialization, not scheduler capacity or a
ratified SLO. SQL execution time and the remaining diagnostic scans still need
separate resource evidence; this does not establish whole-read bounds.

The materialization preflight is implemented. The first regression run exposed
missing required occurrence timestamps in the test fixture; after correcting
the fixture, the old implementation failed because over-budget eligibility
remained a measured zero. The production change then passed 59 focused tests.
Added exact byte/row boundary coverage, including multibyte and embedded-NUL
text, oversized TEMP shadows, and excluded tombstone/mission records. The wider
Automations suite passed 811 tests including 60 doctor checks. Source review
found no required findings within the materialization-budget scope. This closes
the unbounded definition-input materialization gap only; SQL time, other scans,
malformed definition findings, complete diagnostics taxonomy and final delivery
gates still require work.

### Invalid active-definition evidence (in progress)

The scheduler's active-definition loader returns validated records plus failures.
The diagnostic count currently discards those failures via active_definition_ids,
allowing an empty or partial count to hide malformed active persisted input.
The doctor count must use the same loader, fail with a closed typed metadata
classification when that loader reports failures, and omit eligibility while
retaining independently readable queue facts. Do not change scheduler claim
behavior or expose loader failure text. Inactive/quarantined inputs remain
outside this validation scope exactly as in the canonical loader; this does not
establish integrity of every stored definition. Cover JSON decoding, ordinary
validation, durable validation, excluded scopes and TEMP shadows.

The invalid-definition regression reproduced the old partial-count behavior.
The diagnostic count now retains the canonical loader's failure signal as a
fixed payload-free error marker, mapped to the existing invalid-metadata queue
gap. No scheduler selection path changed. The 36-case matrix covers invalid
JSON/ordinary/durable validation, active/inactive/quarantined records, valid
companion present/absent and TEMP shadows. Invalid active inputs omit eligibility
rather than a partial count or zero; independent planned/due counts survive.
The focused suite passed 61 tests; source review found no required findings.
Wider tests and Clippy remain pending at this checkpoint.

Wider verification passed 812 Automations tests using the exact freshly built
focused-test binary (path/hash retained in checks JSON; no intervening compiled
source edits). Coven-cli all-target Clippy -D warnings, formatting and whitespace
passed. Full diagnostics taxonomy, other malformed metadata, SQL/runtime bounds,
final workspace/policy guards and protected delivery remain open.

### Occurrence state metadata (in progress)

Validate bounded main-schema occurrence-state byte projections with the canonical
OccurrenceState deserializer before queue aggregates. Unknown/null/non-text,
invalid UTF-8, oversized and embedded-NUL states must not disappear behind a
measured empty/partial queue. A dedicated invalid_occurrence_state gap omits
queue counts together; independent lease/schema/history categories survive.
No persisted state values or decoder errors may enter the report. Retain all
canonical states, including those outside the four counted queue cohorts. This
adds bounded per-row decoding, not a whole-read SQL execution-time guarantee.

The state regression reproduced the old empty/partial queue. The implementation
now validates main state metadata using bounded BLOB projections and the
canonical enum; invalid states skip queue/due/eligibility reads, emit one closed
gap and reject all accompanying queue facts. Independent lease evidence remains.
The 36-case invalid-state matrix passed in the 62-test focused run. It includes
invalid UTF-8 stored as TEXT, plus null, integer, real, BLOB, case, NUL, size and
unknown-token cases with/without companion rows and TEMP shadows. Source review
found no required findings. A positive all-13-canonical-state case with invalid
TEMP shadows is now added; its verification and wider gates remain pending.

The first positive fixture collided on the required automation/slot uniqueness
constraint. Giving each canonical-state occurrence a distinct automation fixed
the fixture. All 814 Automations tests now pass, including 63 doctor checks and
the positive all-13-state/TEMP case. Clippy remains pending at this point.
Next substantive coverage work is event cursor/checkpoint and stream-head
consistency against contract/events.rs semantics; existing row totals do not
prove replay health. Full taxonomy/resources and final delivery gates stay open.

Event follow-up must not call read_events/resume_events: the read path expires
and inserts checkpoints. Derive read-only observations from the canonical
sequence/retention semantics without invoking that mutation path, and retain
subscriber replay as unobserved unless actual runtime evidence exists.

### Read-only checkpoint expiry observations (in progress)

Add stored/unexpired/expired/invalid-expiry checkpoint counts. Use only bounded
numeric/type/expiry projections from main in the existing snapshot; never read
checkpoint tokens or call event paging/resume, whose implementation mutates
checkpoints. Match canonical expiration at now >= expires_at, including offsets.
Partition every retained checkpoint; non-text/null/invalid UTF-8/NUL/oversized
expiry is invalid. Unexpired does not mean resumable: identity/after-sequence,
stream head, retention and subscriber replay remain separate acceptance work.
Missing table omits all checkpoint facts; missing expiry retains independently
measurable total. Compose ordered source gaps with existing missing-event facts
without dropping independent counts or widening the strict DTO.

Checkpoint expiry now contributes four explicit facts, bringing the closed fact
vocabulary to 51. The nine-row expiry fixture with/without TEMP shadows passed
64 focused checks. The added 24-case source matrix covers all event-table
availability masks, checkpoint table/expiry availability, and TEMP shadows.
It retains nonzero independent counts, rejects invented zero, enforces ordered
gaps and checks no private output/writes. All 816 Automations tests passed,
including 65 doctor checks. Clippy is pending at this checkpoint. Unexpired
checkpoints still do not establish valid identity/cursor/retention/replay; that
remaining event-health scope, other full taxonomy/resource requirements and
final delivery gates remain open.

Coven-cli all-target Clippy -D warnings, formatting and whitespace also passed.
This checkpoint remains uncommitted and is not delivery/certification evidence.

### Retained checkpoint cursor/retention observations (in progress)

Validate checkpoint after_sequence as an integer -1..MAX_SAFE, stream kind with
the canonical enum, and stream identity type/global-feed all convention without
selecting IDs/tokens. Compare valid cursors to matching main head boundaries.
Partition retained checkpoints into invalid cursor metadata, no recorded
retention boundary, invalid boundary metadata, or completed comparisons. Count
comparisons before retention separately using canonical after+1 < earliest.
Compare all retained checkpoints, independently of expiry. Absence of a head
(including a normal global feed) is an unobserved boundary, not corruption.
A cursor ahead of a head is not labeled resumable by these observations.
Missing cursor inputs omit all cursor/retention facts; missing head inputs retain
invalid-cursor counts but omit retention observations. Keep expiry independent.
No event paging/resume, event JSON or checkpoint-token read is permitted. Stream
IDs are compared inside SQLite but not selected into host memory or the report.

Input-validation refinement: SQLite TEXT is not proof of valid UTF-8 or the
canonical 320-character EventStreamId limit. Add a bounded 1281-byte transient
ID projection (four UTF-8 bytes per character plus an overflow sentinel) and
validate with the canonical string type, then discard it. IDs remain absent
from reports/errors; checkpoint tokens remain entirely unread. This supersedes
the draft's no-host-ID-projection approach, which would classify malformed TEXT
identities merely as unobserved boundaries. Include multibyte exact/overflow
limits and canonical embedded-NUL behavior rather than inventing restrictions.

The retention fixture reproduced missing observations before implementation.
Initial focused verification passed 65 tests; an old missing-event expectation
needed the newly independent retention-source gap. A separate identity regression
reproduced malformed UTF-8 being treated as an unobserved boundary; canonical
bounded ID decoding fixed it. Source review found no required findings within
this delta. New coverage comprises an 11-row retention partition fixture with
TEMP shadows, nine identity boundaries, and 40 cursor/head/expiry prerequisite
combinations with independent nonzero evidence and no private output/writes.
The first wider run passed 818 tests but caught an extra JSON array wrapper in
the updated old test expectation. That assertion is corrected; a fresh full
rerun and Clippy are pending. Duplicate/corrupt head relations, head/event
consistency, actual resumability/replay, whole-read resources and final delivery
requirements remain open.

The corrected wider run passed all 819 Automations tests, including 68 doctor
checks. Coven-cli all-target Clippy -D warnings, formatting and whitespace passed.
The closed vocabulary now has 56 facts. This remains in-progress source work,
not protected delivery, certification, actual checkpoint resumability or replay
proof. Next event work: duplicate/corrupt head handling and head/event sequence
consistency, followed by complete resource/runtime and remaining taxonomy gates.

### Ambiguous head keys (in progress)

The checkpoint/head LEFT JOIN multiplies both valid and invalid cursor rows
when a damaged head relation contains duplicate stream keys. Replace that join
with one projected observation per checkpoint: bound matching-head cardinality
at two (zero/one/ambiguous), and use numeric boundary values only when exactly
one key matches. Identical duplicates are ambiguous too. Classify each valid
cursor with ambiguous matches as one invalid boundary; classify each invalid
cursor once regardless of head multiplicity. Preserve independent checkpoint
expiry, queue and schema observations. No GROUP BY materialization, arbitrary
winning boundary, token/payload reads, mutation or subscriber-health inference.
Cover identical/conflicting/invalid-first/invalid-second/many duplicates and TEMP.

The duplicate-head regression reproduced whole-report refusal from an inflated
retention partition. The per-checkpoint correlated cardinality fix passed all
69 focused doctor checks, including ten duplicate-mode/TEMP cases. Source review
found no required findings. Wider Automations tests and Clippy are pending. The
change does not establish integrity of unreferenced head keys, event sequences,
actual resumability/replay or whole-read resource bounds.

The wider run passed all 820 Automations tests, including 69 doctor checks, using
the exact freshly compiled focused-test binary (path/hash retained in checks).
No compiled-source edits intervened. Clippy remains pending at this checkpoint.
Live issue #1355 and authority gate #857 remain open.

Coven-cli all-target Clippy -D warnings, formatting and whitespace passed. This
checkpoint remains uncommitted/incomplete; head/event sequence consistency and
the full resource/taxonomy/delivery acceptance are still open.

### Bounded head/event sequence metadata (in progress)

Check valid unique head windows against unique event sequence metadata, without
reading event JSON, event IDs or checkpoint tokens. Cap default scan inputs at
256 head rows, 4096 event rows and 1 MiB combined bounded stream-key bytes. Read
one overflow sentinel; exceeding any cap omits the entire sequence observation,
not a partial count. Canonical key/numeric validation and duplicate keys are
explicit invalid-metadata outcomes; unavailable prerequisites remain separate.
Keep raw row counts and independent checkpoint observations.

For valid inputs, report heads checked, mismatched windows, orphan event rows,
retained rows below earliest, and rows at/beyond next. Distinct integral rows
inside [earliest,next) must number next-earliest. A future row marks that head
mismatched. Historical rows below earliest are counted but do not alone imply
corruption, because logical retention can precede physical pruning. Include empty
windows and missing events. This is per-record-stream sequence metadata, not
payload validation, global feed_position order, actual resumability/replay or a
whole-report SQL-time bound. Use a dedicated Rust child module for the bounded
scan, with fixed minimized failure states and a last ordered sequence-scope gap.

DTO validation must reject future rows with zero mismatched streams and complete
sequence observations beyond the head/event row caps. Forged-report regressions
reproduced both omissions before the fix. The full Automations binary test set
passed 827 tests after the fix (72 doctor unit tests); final delivery gates remain
pending. This metadata slice does not complete #1355 or #858.

### Delivery state metadata classification

Before reporting pending/failed delivery counts, validate the entire main-schema
state cohort against the production run-store vocabulary: NULL (no recorded
delivery), pending, delivered, failed. Unknown, non-text, malformed and oversized
values must produce `invalid_delivery_state` and omit both delivery counts;
independent receipt totals survive. Compose a missing receipt-store suffix in
that order. Use an aggregate existence query that returns no persisted text,
with binary comparison independent of column collation. Preserve the explicit
receipt-integrity gap for valid metadata; state validity does not prove delivery
obligation/receipt correlation. Include forged DTO, no-write, TEMP-shadow and
canonical positive cases. This is not a whole-read runtime bound.

### Bounded reciprocal receipt links

Read only run id/optional receipt_id and receipt id/run_id from main tables.
Preflight all four columns. Cap each cohort at 256 rows plus a sentinel, keys at
160 canonical entity-ID bytes (161 projected bytes including sentinel), and
combined key input at 128 KiB. Validate via canonical RunId/ReceiptId types and
reject duplicate keys or multiple receipts for one run as invalid metadata.
NULL run receipt references remain valid for legacy/nonterminal runs, but any
stored receipt requires a reciprocal run reference; missing/wrong/one-sided
links produce an explicit inconsistency finding. Withhold link conclusions on
missing prerequisites, malformed metadata or exceeded budgets. Compose the
link gap after independent delivery-state and receipt-store gaps. No payload,
digest, authority or external delivery proof; retain ReceiptIntegrityNotVerified
when no stronger conditional gap is present. No repair or writes.

Reciprocal-link checkpoint: the initial regression reproduced one-sided link
acceptance. Review-driven forged DTO regressions reproduced contradictory
missing-store/observed-link reports and both receipt-row and available
failed-plus-pending run-subset cap violations. All are rejected now. Final
Automations binary suite passed 835 tests, including 80 doctor unit tests.
Independent source review found no residual required link invariants. Full
receipt evidence/digest/authority/delivery proof and #1355/#858 remain incomplete.

### Owned diagnostic read boundary

The owner-local control action must open a dedicated existing-store read-only
connection from the daemon connection's main filename, with existing-store
symlink checks and no initialization. Reject absent/temporary/in-memory paths;
never fall back to the caller connection or let the CLI open the database.
Set zero busy wait on the owned connection and install an owned cooperative
SQLite progress budget. Dropping it must not touch caller handlers/settings.
Use a 2,000,000 approximate VM-instruction budget sampled every 1000 operations,
and a two-second cooperative elapsed budget, checked again before accepting a report.
These are diagnostic safety limits, not ratified SLOs or hard OS/I/O deadlines.
Keep one coherent read transaction. Exhaustion returns a fixed minimized
unavailable failure, never zero/partial healthy counts. Add deterministic budget,
handler isolation, read-only/no-create, invalid-path and lock-refusal tests.

Owned-read checkpoint: caller-handler SQL execution reproduced RED before
isolation. Review found the existing-store helper repaired permissive modes; a
0640 permission regression reproduced the mutation before replacing that helper
with non-mutating metadata checks. Final verification: 844 Automations binary
tests and all three disposable-daemon doctor integration tests passed; coven-cli
all-target Clippy with warnings denied, formatting and whitespace checks passed.
Source review found no residual required read-boundary issues. The cooperative
budget does not complete whole-read memory/I/O/platform SLO evidence or #1355.

### Snapshot page geometry

Extend disk/retention observations with main-schema freelist pages, logical
snapshot bytes (page_count * page_size), and non-freelist bytes
((page_count - freelist_count) * page_size). Validate supported SQLite page
sizes (powers of two from512 through65536), freelist<=pages, exact derived
relations and safe integer bounds. Zero pages is permitted for an empty store.
Use numeric main PRAGMAs only, under the existing snapshot/budget. TEMP geometry
cannot substitute. No physical-file/WAL size, available disk, write capacity,
reclaimability, retention policy or healthy pressure claim follows; preserve
FreeDiskAndRetentionNotObserved. Do not checkpoint, vacuum or prune.

Page-geometry checkpoint: initial missing-fact regression reproduced RED.
Final verification passed 846 Automations binary tests (82 doctor unit tests),
all three disposable-daemon doctor integration tests, coven-cli all-target
Clippy with warnings denied, formatting and whitespace checks. Independent
source review found no required fixes. Fact vocabulary is64; complete storage
pressure/retention and the full #1355/#858 outcomes remain unproven.

### Filesystem capacity estimate and sample scope

Use the existing fs2 available_space backend against the main database file's
filesystem. Emit FilesystemAvailableBytesEstimate with a separate actual probe
completion timestamp FilesystemCapacitySampledAtMillis. This OS estimate is
outside SQLite's coherent snapshot and is not quota/reservation/write-capacity
or pressure proof; native platform semantics differ. Keep RetentionPressureNotObserved.
On missing file-backed source or probe I/O failure, omit both fields and append
FilesystemCapacityUnavailable before the retention gap. Out-of-safe-range bytes
or invalid sample timestamp produce FilesystemCapacityMetadataInvalid with the
same omitted pair. Preserve all5 main geometry fields. No paths, volume names,
raw OS errors or private payloads may enter the report. Zero is a valid measured
estimate, never the fallback for failed probes. Keep the existing cooperative
elapsed guard around the probe; it does not cancel a blocked OS syscall.

Capacity checkpoint: missing native estimate/sample facts reproduced RED.
The first expanded run found a test-only macOS /var vs /private/var path
alias assertion; compare resolved files instead. Final verification passed
851 Automations binary tests (85 doctor unit tests), all three disposable-daemon
doctor CLI tests, coven-cli all-target Clippy with warnings denied, formatting
and whitespace checks. Independent source review found no required fixes.
Fact vocabulary is66. Probe failures omit the pair with explicit gaps while
geometry survives; native quota/clock/platform/retention/SLO proof remains open.

Full readiness audit found malformed attempt state/class cohorts could
disappear into partial counts, and NULL pass timestamps escaped category error
boundaries. Timestamp NULL/non-text regression reproduced whole-report refusal;
use nullable typed cells and classify within success/lag, preserving queue/store.
Attempt cohort validation remains a required follow-up before draft readiness.
The first working-snapshot policy run passed25/26; the secret scanner flagged a
state-serialization local named token. Rename it state_name, without changing
the scanner or adding an allowlist. Final guards must be rerun after fixes.


### Metadata readiness fixes and populated daemon acceptance

The required NULL/non-text timestamp, status-collation and attempt-state/class
source findings are addressed. Regression evidence reproduced whole-report
NULL refusal, NOCASE uppercase status acceptance, and partial attempt counts.
Canonical bounded AttemptState decoding and exact persisted failure-class
validation preserve independently valid facts. Closed DTO gaps reject forged
zero counts and conflicting unavailable/invalid state markers. All854
Automations tests passed, then89 doctor tests passed after test-only controls;
all26 full-working-snapshot policy checks passed. These are checkpoint evidence,
not final PR-range or complete certification proof.

Workspace Clippy subsequently found the nullable lag row's type complexity;
name its tuple type, without lint suppression or behavior changes. Strengthen
the existing disposable daemon/CLI test with terminal occurrence/run/attempt/
receipt rows, over1MiB private bodies, explicit32KiB action/CLI output ceilings,
private-value/path absence, positive observed counters, retained receipt-integrity
gap and full application-table equality under a reserved WAL writer. These
fixtures grant no runtime authority and are not authenticated receipt evidence.
Repeat current verification before delivery. Full #1355 and #858 remain open.


Pre-integration verification passed full locked workspace tests, workspace
all-target Clippy with warnings denied, and all3 populated daemon/CLI tests on
base b912dc5b with Homebrew Rust1.98.1. These are not current-main/pinned-toolchain
proof. Archived all26 source files and retained integration stash
986b06ca45b4ac30cf21065afe19c1f97e2c7aa2. Fast-forwarded the task branch to
f6049f5a and reapplied all26 files without conflicts. Current main pins Rust1.99.1
(installed), and adds release-governance, action-pin and release-tag policy
tests. Use rustup Cargo explicitly for integrated verification; preserve the
old successful receipts as historical evidence only. No task commit or PR yet.


### WAL sidecar metadata scope

While #1377 CI runs, add a metadata-only observation of the exact main filename's
-wal sidecar. Report presence, file-length estimate and a separate UTC
completion sample as one closed group. Require an existing regular non-symlink
main path; inspect sidecar metadata without following symlinks or opening its
contents. Missing sidecar is observed absence with zero bytes; missing source,
permission/I/O error and non-file-backed source are unavailable. Symlink/non-file
sidecars or unsafe bytes/clocks are invalid metadata. Preserve geometry and the
independent filesystem estimate. Canonical gaps are capacity gap, sidecar gap,
then RetentionPressureNotObserved. No checkpoint, WAL open/read, repair or writes.

This observes a filesystem name outside SQLite's snapshot, potentially including
an inactive/orphaned sidecar. It proves no journal mode, frame integrity, active
reader, checkpoint progress, reclaimable bytes, retention policy, quota, write
capacity, pressure threshold or SLO. Source names can change during the probe;
metadata is an estimate, not a bound connection/file identity or atomic snapshot.
Keep full disk/retention acceptance open. Test present/absent paths, unknown and
invalid probes, safe bounds, closed DTO groups/relations and no writes/privacy.

### Retained scheduled-occurrence planning delay (in progress)

The run ledger's `started_at` is persisted during adoption, before launch, so it
cannot establish actual runtime-start latency. Add independent numeric facts for
the count of retained scheduled occurrences checked and their maximum planning
delay: max(created_at - scheduled_for, 0), in milliseconds. Planning ahead of
the due instant is valid; manual occurrences are excluded. Parse bounded TEXT
RFC3339 instants, compare offsets as instants, and reject malformed kinds or
selected timestamps without returning private values. Read only main-schema
projections in the existing transaction, at most 256 retained occurrence rows;
additional rows produce an explicit budget gap, not a partial maximum. Missing
prerequisites and malformed metadata have distinct gaps. Both facts are present
or absent together; count zero requires maximum zero. Keep latest-pass facts
independent, and keep full lag history, runtime-start and ratified SLO acceptance
open. Tests must first expose the missing producer facts, then cover lead/lag,
manual exclusion, malformed inputs, unavailable schema, row caps, TEMP shadows,
forged DTO groups and independent category preservation.

### Retained failures since latest retained success (in progress)

`health.rs::routine_health` currently labels failures scheduled after the latest
retained successful occurrence as consecutive failures. The diagnostic aggregate
must name that precise boundary rather than claim a strict consecutive terminal
streak. For each retained automation history, parse scheduled instants, find its
latest retained succeeded occurrence, and count failed occurrences strictly
after it (all retained failures if no success survives). Other canonical states
neither count as failure nor establish success. Report histories checked,
routines with at least two such failures, and the maximum retained failure count.
These are metadata counts, not a ratified alert threshold, lifetime history or
classified failure analysis.

Use main-only automation ID/state/scheduled timestamp projections in the existing
snapshot, at most 256 retained occurrence rows. Decode the canonical AutomationId
(96-byte limit) and OccurrenceState without returning either ID or timestamp.
Compare parsed instants including offsets. Manual writers use nanosecond
precision while scheduled rows can use milliseconds, so distinct retained rows
can validly share an instant. Preserve each row and use a strict after-success
cutoff; equal-time success excludes a failure at that same instant. Missing prerequisites, malformed
metadata and row-budget exhaustion each omit the entire group with distinct
gaps. Preserve independent attempt/quarantine/retry facts and their existing
gaps. Validate atomic DTO groups, repeated-history and maximum bounds, necessary
cohort lower bounds, and all independently observed occurrence subsets against a
completed scan. Keep incomplete groups free of completed-scan caps. Retain full
failure-history/classification and the other #1355/#858 acceptance gaps.

### API boundary correction (in progress)

Integration review found that generic action routing opened a writable store before
the owned diagnostic reader. API regressions reproduced missing-home database
creation, permission repair and unsanitized opening failures. Doctor now routes
after transport rejection and before that opener, using the shared fixed-error
response and existing-file reader. Resolve only parent directories so SQLite
NOFOLLOW works with macOS temporary-directory aliases while refusing a symlinked
database. Check initialized and deliberately incomplete uncached stores for
unchanged permissions and main bytes; incomplete schemas report explicit missing evidence without
migration. Fresh verification remains required.

The connection is read-only for application data and schema. Ordinary SQLite WAL
coordination may create WAL/SHM files or update read marks/locks. No zero-filesystem-
write guarantee is made, and immutable mode would give the wrong live-store
snapshot semantics.
