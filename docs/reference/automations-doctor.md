---
title: "Automations doctor"
summary: "Interpret owner-local Automations observations without mistaking incomplete evidence for operational health."
read_when:
  - Inspecting failed, delayed, or ambiguous Automations work
  - Interpreting Automations doctor evidence gaps
source_adjacent_reason: "Defines the operator interpretation of the Rust doctor report and CLI."
---

# Automations doctor

The doctor implementation under #1355 is in development. This page describes
its report contract and the remaining evidence gaps; it is not a released
feature or a certification result.

Run `coven automations doctor --json` to request the report from an already
running daemon through owner-local IPC. The CLI does not start a daemon,
initialize a store, or fall back to TCP. An unavailable daemon produces a fixed
error. Inspect `coven daemon status` before deciding whether to start it.

A report contains `schemaVersion`, `observedAt`, and eleven `observations`.
Each observation contains a category, numeric facts, explicit evidence gaps,
and a fixed next-step code. There is no overall `healthy` flag. A valid report
can contain unhealthy facts and incomplete checks at the same time.

`observedAt` supplies the observation cutoff for one coherent read transaction.
It does not establish the freshness of historical events or prove that a
recorded owner process is alive. Counts
refer to persisted records, not unrecorded external effects. Doctor neither
reconciles those effects nor certifies that an absent record means no effect.

The daemon opens a dedicated read-only connection to its existing main store.
It does not initialize missing stores, follow a symlinked database, repair file
permissions, or replace the caller connection's handlers or busy policy.
The read-only guarantee covers application data and schema, not zero filesystem
activity: [SQLite WAL readers](https://www.sqlite.org/wal.html#read_only_databases)
may create WAL/SHM coordination files and update shared-memory read marks or locks.
Doctor does not request a checkpoint or change journal mode. It preserves normal
WAL visibility rather than treating the live database as immutable.
Temporary/in-memory sources are refused. The diagnostic connection waits zero
time for database locks and uses an approximate two-million SQLite VM-instruction
budget, sampled every 1000 instructions, plus a two-second cooperative elapsed
budget. A final elapsed check also runs before accepting a report.

Budget exhaustion returns `Automation doctor read budget exceeded.` with no
partial report. Other read/open/lock failures return
`Automation doctor observation unavailable.`. Preserve that result alongside the
installed version and existing daemon status; it is not an empty healthy report.
These are diagnostic safety budgets, not measured or ratified release SLOs.
SQLite callback intervals, individual operations and OS/file I/O are not hard
wall-clock bounds. Whole-read memory, I/O and platform resource evidence remains
incomplete.

## Read each category within its scope

| Category | Observed persisted facts | Evidence still required |
| --- | --- | --- |
| `leadership` | Recorded owner presence and scheduler generation | Process liveness and current admission ownership |
| `last_successful_pass` | Latest pass success and retained successful completion time, when observed | Success before instrumentation when no successful observation survives |
| `store` | Automations table count, required-object presence/form counts, and recorded contract-migration timestamp/counters, when valid | Expected schema, other migrations, current row verifiability and integrity verification |
| `queue` | Stored states, due planned rows and scheduler-eligible candidates at the observation cutoff | Runtime authority admission is not proven by scheduler eligibility |
| `leases_fences` | Lease-bearing total, active/expired/missing/invalid expiry counts, leases outside claimed/running states, recorded authority presence, and mismatched unlaunched scheduler claims | Process liveness and complete attempt-fence consistency |
| `planning_start_lag` | Latest pass duration/start lag and bounded retained scheduled-occurrence planning delay, when observed | Actual runtime-start lag, full recovery/history evidence and ratified SLO comparisons |
| `repeated_failures` | Quarantine, failed attempts, retry exhaustion and retained failed occurrences since latest retained success | Strict consecutive-occurrence streaks, full history and historical classification |
| `authority_refusals` | Launch-refused and unsupported-authority attempt counts | Complete identity, authority and approval refusal evidence |
| `delivery_receipts` | Failed/pending deliveries, stored receipts and bounded reciprocal index links | Delivery obligations, full receipt correlation and integrity |
| `event_changefeed` | Stored events/streams and checkpoint expiry/cursor/retention observations | Cursor consistency, checkpoint resumability and actual subscriber reconnect/replay health |
| `disk_retention` | Main page geometry, separately sampled filesystem available bytes, and WAL-sidecar metadata estimates | Active WAL/frame/checkpoint semantics, quota/write guarantees, retention pressure and clock/platform evidence |

`due_planned_occurrences` counts persisted `planned` rows whose `scheduled_for`
meets the scheduler's canonical UTC millisecond cutoff at `observedAt`. It includes
older latest-only history and rows belonging to inactive or blocked automations.
It is not an eligible or claimable count.

`eligible_occurrences` streams a bounded timestamp projection of the same
candidate relation used by scheduler paging. It applies active-definition
validation, latest-only selection, quarantine and overlap exclusions, retry
`not_before`, and run timeout rules. It counts the entire candidate relation,
not only the first scheduler page. Selected malformed timestamps make the
eligible count invalid, as they refuse scheduler enumeration. The queue emits `invalid_persisted_metadata`, omits the
eligible total, and retains its independently read stored/due counts. Other
report categories remain available; no private timestamp or parse text is
emitted. This is persisted scheduler eligibility, not Runtime Authority approval or launch admission; the queue
retains `runtime_admission_not_proven`.

Eligibility checks the canonical candidate query's prerequisites and its
transitive definition-list, mission-exclusion and quarantine relations in the
same snapshot. A missing prerequisite omits `eligible_occurrences` with
`eligibility_store_unavailable` when the remaining report queries can run.
Stored state/due counts remain independent; unavailable eligibility is never a
measured zero. Selected invalid timestamps retain their separate metadata gap.
Preflight, canonical eligibility, definition/mission/quarantine reads, direct
queue counts and the lease summary use main-schema relations, so TEMP shadows
cannot substitute their records.

The due count separately requires main occurrence `state` and `scheduled_for`
columns. Missing schedule metadata produces the ordered pair
`due_queue_unavailable`, `eligibility_store_unavailable`, omitting both counts
while retaining independently readable stored-state counts and lease facts.
The CLI rejects an accompanying fabricated due count or reversed gap order.
When counts are present, it also requires due ≤ planned and eligible ≤ due;
unavailable counts stay omitted rather than being substituted with zero.
Present occurrence states are decoded from bounded byte projections with the
canonical occurrence-state enum. A null, non-text, invalid UTF-8, oversized or
unknown state reports `invalid_occurrence_state` and omits all queue counts;
the CLI rejects accompanying fabricated counts. Other categories, including
readable lease expiry facts, remain available. This avoids reporting an empty
or partial queue from malformed state data. State decoding does not prove
transition validity, referential integrity, or a whole-read execution-time bound.

Missing occurrence state or the table reports the ordered gaps
`queue_state_unavailable`, `due_queue_unavailable`, and
`eligibility_store_unavailable`, with every queue count omitted.

The eligibility missing-dependency regression covers 23 relation/column variants
with and without TEMP shadows. Lease and queue prerequisite combinations have
separate coverage. Active-definition validation reuses the scheduler's canonical
reader. Before that reader runs, the doctor measures only numeric byte lengths
for the exact main-schema definition records and quarantined retry IDs it will
load. Each source may contain at most 256 rows, with at most 1 MiB of combined
text inputs, including incidental fields on inactive definitions. Exceeding either
limit reports `eligibility_read_budget_exceeded` and omits eligibility; it never
returns a truncated count or a measured zero. Independent queue counts remain.
If the canonical loader reports a malformed active, non-quarantined definition,
the queue reports `invalid_persisted_metadata` and omits eligibility instead of
returning an empty or partial count. Independent state/due counts remain.
This includes JSON decoding, ordinary definition validation and durable timezone
validation. Loader reasons, definition IDs and prompts are never returned.
Inactive and quarantined definitions remain outside this validation scope, as
in the scheduler loader; a readable eligibility count does not establish the
integrity of every stored definition.

These are diagnostic input limits, not scheduler capacity or ratified SLOs.
They bound inputs to this materializing reader; SQL execution time and other
report scans still need separate resource evidence. Bounded inputs and timestamp
projections alone do not prove bounded resource use for the whole diagnostic.

Lease expiry counts partition every row carrying owner or expiry metadata. The
same bounded timestamp classifier serves lease paging and the doctor summary.
An expiry equal to the observation instant is expired; UTC offsets are compared
as instants. Missing expiry and malformed, oversized, NUL-containing or
undecodable timestamps have separate counts. These are observed metadata
findings, not proof of a live owner or runtime termination.

Lease expiry scanning requires main occurrence owner and expiry columns. Missing
either column or the table reports `lease_store_unavailable` and omits the
lease-bearing total, its four expiry partitions, and the outside-state count.
Missing only occurrence state reports `lease_state_unavailable`: expiry facts
remain readable, while the outside-state count is omitted. Independent recorded
authority facts remain available in both cases. With assigned authority, missing
state also requires `recorded_claim_store_unavailable`; the CLI rejects a
fabricated mismatch count even when it is zero. Missing expiry alone does not
prevent mismatch counting, whose predicate does not use expiry.

The four expiry counts must sum to the lease-bearing total. The count outside
`claimed`/`running` states cannot exceed it. The CLI rejects reports that violate
these relationships. `fence_consistency_not_measured` remains explicit: expiry
counts do not establish process liveness or valid attempt fences.

When a valid assigned authority record exists, the doctor also counts claimed
daemon occurrences whose generation differs from the recorded generation,
including NULL and future generations. Any associated run excludes the claim;
expiry does not affect this count. These are the recovery query predicates,
observed in the same read snapshot without acquiring leadership or constructing
a scheduler fence. A released or missing authority record omits the mismatch
count instead of inventing zero. Invalid authority metadata produces an explicit
metadata gap while preserving the independent lease-expiry observations.
A missing authority table or required `id`, `generation`, or `owner_id` column
reports `recorded_authority_unavailable` in leadership and lease observations.
Leadership facts and the recorded-fence flag are omitted; independent lease
expiry counts remain. The main-schema preflight cannot be satisfied by TEMP
objects. This differs from a readable authority record with no assigned owner.

Recorded authority is read separately from claim mismatch counting. If a valid
assigned record is readable but claim-query prerequisites are missing, leadership
and the recorded-fence flag remain available. The lease observation reports
`recorded_claim_store_unavailable` and omits only the mismatch count; unavailable
counting cannot become a measured zero. Lease gaps put the unavailable cohort
or state first, followed by any authority or claim-store gap. These observations
do not establish full storage integrity or bounded whole-read resource use.

A measured zero does not grant recovery permission or prove scheduler liveness.
An expired lease on a running occurrence does not authorize settlement or a
repeat launch. Inspect correlated run and attempt evidence first.

Receipt counting independently requires the main `automation_receipts` table.
If that table is missing or replaced by a view, `delivery_receipts` reports
`receipt_store_unavailable`, omits `stored_receipts`, and retains the failed and
pending delivery counts read from main runs. A TEMP shadow cannot supply a
receipt count. The CLI requires exactly those two delivery facts under this gap
and rejects an accompanying receipt count. These are recorded delivery states,
not proof of external delivery, receipt correlation or integrity.

Delivery counts separately require main `automation_runs.delivery_state`.
When that column is unavailable, `delivery_state_unavailable` omits both delivery
counts while retaining an independently available receipt count. If receipts
are also unavailable, both gaps appear in that order and no numeric delivery
facts are emitted. A TEMP run or receipt table cannot supply substitute evidence.

Before classifying runs, the doctor verifies delivery states against the run-store
vocabulary: NULL (no recorded delivery), `pending`, `delivered`, and `failed`.
Unknown or non-text states report `invalid_delivery_state` and omit both delivery
counts, preserving an independently available receipt count. A missing receipt
store follows that gap. Comparison is binary, so a damaged column's collation
cannot accept unknown case variants. No persisted state text is returned.
Valid delivery-state classification does not establish delivery obligations.

A separate bounded check compares reciprocal run/receipt index links. It reads
only main run `id`/optional `receipt_id` and receipt `id`/`run_id`, validating
canonical entity IDs of at most 160 bytes. A run may have no receipt, but each
stored receipt must point to a run that references that same receipt. Missing,
wrong or one-sided links report `receipt_links_inconsistent`. Duplicate keys,
multiple receipts bound to one run, non-text or malformed keys report
`receipt_link_metadata_invalid`. Missing required columns report
`receipt_link_store_unavailable`.

The check accepts at most 256 rows per relation and 128 KiB combined projected
key input; overflow reports `receipt_link_read_budget_exceeded`. Each key
projection is capped at 161 bytes including its overflow sentinel. No partial
link conclusion is reported. The CLI also rejects completed link observations
whose receipt count or available failed-plus-pending run count exceeds the
corresponding scan row cap. Link gaps follow independent delivery-state and
receipt-store gaps; raw counts remain available. TEMP tables cannot supply
links. Matching links do not validate receipt bodies, digests, attempt/occurrence
bindings, authority evidence or external delivery. `receipt_integrity_not_verified`
remains the default gap when no conditional finding applies. These caps do not
establish a whole-read time or release SLO bound.
Other run-table dependencies still need complete isolation before every missing
run-table case can return a report.

Event counts independently require the corresponding main-schema table. If
`automation_events` or `automation_event_stream_heads` is missing or replaced
by a view, the report omits that table's count, retains any independently
available count, and reports `event_store_unavailable`. A TEMP shadow cannot
supply rows or satisfy the prerequisite. Unavailable counts are never emitted
as zero, and the CLI rejects this gap when both event facts are present.

Checkpoint expiry observations count retained main checkpoints separately as
unexpired, expired or invalid-expiry metadata. An expiry equal to the observation
instant is expired; RFC3339 UTC offsets compare as instants. Null/non-text,
invalid UTF-8, embedded-NUL and oversized expiry values count as invalid. These
three partitions must sum to the retained checkpoint total; the CLI rejects an
inconsistent report. No checkpoint token, stream ID or event payload is read.
The doctor never calls event paging/resume, which would prune/issue checkpoints.

Unexpired does not mean resumable. Stream identity, sequence position, retention
and subscriber replay remain separate checks. Missing checkpoint storage omits
all four checkpoint expiry facts with `checkpoint_store_unavailable`; a missing expiry
column preserves the total and reports `checkpoint_expiry_unavailable`. Missing
event-source gaps precede checkpoint-source gaps; neither may hide independently
readable facts. These expiry counts do not establish a whole-read runtime bound.

Retained checkpoints are also partitioned into invalid cursor metadata, no
recorded retention boundary, invalid boundary metadata, or completed retention
comparisons. Invalid metadata covers integer cursor bounds, canonical stream
kind/ID validation and the `feed`/`all` convention. Valid IDs are transiently
decoded from at most 1281 bytes, checked against the canonical 320-character
limit, then discarded; no IDs or checkpoint tokens enter reports or errors.
A missing head (including a normal global feed) is an unobserved boundary,
not a corruption finding. A boundary is comparable only when exactly one
head matches. Duplicate keys, even with identical values, count once as an
invalid boundary for each valid checkpoint cursor; they cannot multiply cursor
or expiry partitions or choose a winning boundary. Invalid cursor metadata is
also counted once regardless of duplicate heads. `event_streams` counts stored
head rows; it does not deduplicate logical streams in a damaged table. Completed comparisons separately count cursors before
retention using the canonical strict `after + 1 < earliest_sequence` check.
That count cannot exceed comparisons, and the four cursor partitions must sum
to retained checkpoints. These observations include expired checkpoints.

Missing cursor inputs report `checkpoint_cursor_metadata_unavailable` and omit
all five cursor/retention facts. Missing head inputs report
`checkpoint_retention_store_unavailable`, retaining the independent invalid-cursor
count. Missing checkpoint storage subsumes both. Expiry gaps come before cursor
or retention gaps. Comparisons do not establish head/event consistency, a valid
future cursor, actual resumability or observed subscriber replay.

The storage category reports main snapshot `database_pages`, `page_size_bytes`
and `database_freelist_pages`. `database_logical_bytes` is pages multiplied by
page size; `database_non_freelist_bytes` excludes whole freelist pages. The CLI
rejects inconsistent derived values, unsupported page sizes, and a freelist
that includes every page of a nonempty database. An empty database has zero
pages and zero derived bytes. TEMP geometry cannot supply these observations.

These are logical snapshot metrics, not physical file allocation, WAL bytes,
filesystem free space, payload size, guaranteed write capacity or reclaimable
disk space. The doctor performs no checkpoint, vacuum or pruning and retains
`retention_pressure_not_observed`. See SQLite's
[page-count](https://www.sqlite.org/pragma.html#pragma_page_count),
[page-size](https://www.sqlite.org/pragma.html#pragma_page_size) and
[freelist](https://www.sqlite.org/pragma.html#pragma_freelist_count) definitions.

For file-backed stores, `filesystem_available_bytes_estimate` reports the
native filesystem estimate for the main database file, accompanied by
`filesystem_capacity_sampled_at_millis`. The timestamp is sampled after the
probe completes; it is separate from `observedAt` and SQLite's transaction
snapshot. It may therefore be later than the report's observation cutoff.

Zero is a valid measured estimate. A missing file-backed source or failed probe
omits both fields with `filesystem_capacity_unavailable`; unsafe numeric values
or an invalid sample clock use `filesystem_capacity_metadata_invalid`. Geometry
facts remain available. Either gap precedes `retention_pressure_not_observed`.
The CLI rejects missing sample pairs, retained values under a failure gap and
reordered/contradictory gaps. Paths, volume names and raw OS errors are omitted.

The estimate uses the existing native filesystem backend. Platform and quota
semantics vary; it does not reserve bytes, guarantee a future write, measure WAL
pressure or establish a retention threshold. The outer cooperative elapsed check
includes this probe but cannot cancel a blocked OS syscall. Installed-platform,
clock and resource/SLO evidence remains separate.

A separate metadata-only probe observes the main filename's `-wal` sidecar.
`database_wal_sidecar_present`, `database_wal_sidecar_bytes_estimate` and
`database_wal_sidecar_sampled_at_millis` form one required group. A regular
sidecar reports its file length from metadata; a missing sidecar reports presence
and bytes as zero with a valid sample. Missing/non-file-backed main source or
I/O/permission errors omit the group with `wal_sidecar_unavailable`. A symlink,
non-file sidecar, unsafe byte length or invalid clock omits the group with
`wal_sidecar_metadata_invalid`. The probe does not follow symlinks, open/read
WAL contents or invoke a checkpoint. Geometry and filesystem facts survive
independent probe failures. Gaps have a fixed order: filesystem gap, sidecar
gap, then `retention_pressure_not_observed`.

The name-based filesystem probe is outside SQLite's snapshot and is not an
atomic file-identity binding. Files may change during observation; an inactive
or orphaned sidecar can exist. These facts establish no journal mode, valid
frames, current reader, checkpoint progress, allocated filesystem blocks, reclaimability, write capacity,
retention threshold or healthy pressure. The cooperative elapsed guard does
not cancel a blocked metadata syscall. Keep retention evidence incomplete.

A separate bounded metadata scan compares each recorded stream head's retained
window `[earliest_sequence, next_sequence)` with unique stored event sequence
positions. It counts checked heads, mismatched streams, orphan event rows, rows
before retention and rows at or beyond the next sequence. Missing positions or
future rows mark a stream mismatched. Historical rows below retention alone do
not: logical retention can precede physical pruning. Empty windows are valid.
The scan does not inspect event payloads, global feed positions or replay.

The scan accepts at most 256 head rows, 4096 event rows and 1 MiB of combined
projected stream-key input. Crossing any limit reports
`event_sequence_read_budget_exceeded` and omits all five sequence facts.
Malformed metadata or duplicate head/event sequence keys report
`event_sequence_metadata_invalid`, also without partial counts. Missing required
columns report `event_sequence_store_unavailable`. These gaps follow checkpoint
gaps and preserve independently readable counts and checkpoint observations.
The input caps do not establish a whole-read time, memory or release SLO bound.
The CLI rejects complete observations exceeding the row caps or reporting future
rows without a mismatched stream.

Stored event counts and checkpoint expiry partitions do not prove cursor
consistency, checkpoint resumability, replay or subscriber health.

The store checks 73 fixed table, index and trigger names from production
initialization. It reports missing objects, unrecognized object forms and
incomplete bounded checks as separate counts, without returning names, SQL or
fingerprints. These outcomes are mutually exclusive per required object; their
sum cannot exceed 73. A same-named index with a changed definition is an
unrecognized form even when the table and presence counts are unchanged.

The form recipe includes table columns and constraints, foreign keys, table
properties, index terms/collations/order, automatic indexes, and definition SQL.
All definition and PRAGMA reads are pinned to `main`, so temporary objects
with matching names cannot substitute their metadata. Only fixed required names
and compiled known index names become metadata query parameters. Unexpected index names remain visible to form comparison without
becoming new queries. SQL/default text is limited to 32,768 bytes, identifiers
to 128 bytes, and each metadata projection to 256 rows; an oversized or invalid
projection is incomplete, and truncated prefixes are never fingerprinted.

Known forms come from fresh and three historical fixture initialization paths.
Matching individual forms does not establish a coherent supported whole schema,
current migration eligibility or SQLite integrity. Other SQLite versions and
platforms need separate evidence. The separate `unexpected_automation_schema_objects` count covers objects with
case-insensitive `automation_` or `idx_automation_` names and objects attached
to Automations tables, excluding SQLite automatic indexes. It does not count
unrelated application objects or establish cross-object compatibility. Transitive
missing-table dependencies still need separate handling. A query failure
refuses the report rather than inventing zero findings. The broader store gap
remains explicit, and doctor does not recreate or repair objects.

The store observation reads the existing `coven.automations.v1` contract-migration
ledger record. Its six counters describe definitions, occurrences and runs
migrated or left unverifiable **at migration time**, not current row totals or
current integrity. An existing ledger with no profile record reports presence
zero and omits the counters and timestamp. A missing ledger table or required ledger column instead reports
`contract_migration_store_unavailable` and omits even the record-presence flag;
independent schema findings and other observations remain available. Doctor does
not create the ledger. Invalid numeric types/ranges or a malformed bounded timestamp
produce `invalid_persisted_metadata` while preserving independent observations.
The doctor does not rerun migration, repair the ledger, read historical definition
bodies, or perform an implicit SQLite integrity scan. The
`integrity_and_migrations_not_verified` gap remains explicit even with a valid
record; event-baseline, timezone and other migration evidence remain separate.

The scheduler retains one successful-pass summary in the same fence-guarded
transaction as its latest-pass record. A later failure does not erase that
summary. Startup migration may backfill from a latest pass that actually
succeeded; a failed latest pass cannot supply an earlier success time. Doctor
only reads these records. The completion fact is Unix time in milliseconds.

If the successful-pass history table or a required projection column is missing, `last_successful_pass` reports
`successful_pass_store_unavailable` and omits the completion timestamp. Any
independently readable latest-pass status remains available, as do the store's
missing-object finding and queue observations. This differs from an existing
history table with no recorded success. Doctor does not create the table.
Other missing-table dependencies still need complete preflight and isolation.

A missing latest-pass `duration_ms` column reports
`latest_pass_duration_unavailable` and omits the duration while preserving a
readable latest status and independently valid start lag. Status, duration and
lag reads use `main`, so TEMP shadows cannot supply substitute values. If lag
metadata is also invalid, the planning observation carries both the duration
unavailable gap and `invalid_persisted_metadata`, with no planning facts. This handles duration-column loss independently of lag availability.

Missing `scheduled_at` or `started_at` columns report
`latest_pass_lag_unavailable` and omit lag while retaining valid duration. If
duration is also missing or invalid, both gaps remain explicit and the planning
observation has no numeric facts. No duration or lag is fabricated from a TEMP
shadow.

Missing latest-pass `status`, `id`, or the table itself reports
`latest_pass_status_unavailable` and omits the success flag. Independently valid
success history remains available; unavailable or malformed history adds its
own gap. This differs from an available latest-pass table with no row: an
unavailable status cannot contradict retained success history. Missing `id` or
the table also makes duration and lag unavailable, while a missing `status`
column alone leaves valid duration and lag visible. No TEMP shadow supplies
substitute evidence. Other transitive query dependencies still need isolation.

SQL NULL and non-text values in retained-success, scheduled or started pass
timestamps are malformed cells, distinct from an absent row. The doctor
classifies them within the affected success/lag category and preserves
independently readable queue/store observations; no row-decoding error may
turn them into a whole-report refusal. Even a BLOB containing valid timestamp
text is not accepted as a persisted TEXT timestamp.

If a retained successful timestamp is malformed, `last_successful_pass` reports
`invalid_persisted_metadata` and omits that timestamp. The valid latest-pass
status and independent categories remain available. The report never echoes
the malformed text or substitutes a healthy empty history. Isolation also covers pass start-lag timestamps, retry-exhaustion counters, and
selected eligible-queue timestamps. Invalid latest-pass status reports
`invalid_latest_pass_status` and withholds the entire `last_successful_pass`
observation, including any retained timestamp. Status must be TEXT and exactly
`succeeded` or `failed`, independent of the column's declared collation.
Invalid duration reports
`invalid_latest_pass_duration` and withholds `planning_start_lag` facts. Each
preserves the independent category when valid, plus queue/store observations;
neither invents a zero or successful pass. Other store or query failures can
still refuse the whole report. Preserve that refusal as an incomplete diagnostic
result; it is not a healthy empty report.

Attempt-state counts require every retained state to be a TEXT value accepted
by the canonical attempt-state enum. An invalid state reports
`invalid_attempt_state` and omits `failed_attempts`, while independently valid
quarantine and retry-exhaustion facts remain available. Missing state columns
retain the distinct `attempt_state_unavailable` gap. These gaps can accompany
independent quarantine/history gaps in their canonical order.

Refusal counts require every non-NULL persisted failure class to be TEXT in the
attempt schema's closed vocabulary. NULL is a valid absent classification.
Invalid classes report `invalid_attempt_failure_class` and omit both refusal
counts; missing columns report `attempt_failure_class_unavailable`. Neither
condition conceals valid failed-attempt counts. State decoding and class/count
comparisons are exact even if a damaged column declares a permissive collation.
No malformed value, private class text, or TEMP substitute enters the report.
This validates retained classifications; it does not infer unrecorded identity
or approval refusals or validate the full state/class transition history.

The retained occurrence-history check separately reports
`retained_occurrence_failure_histories_checked`,
`routines_with_repeated_retained_failures_since_success`, and
`maximum_retained_failures_since_success`. For each retained automation ID,
it counts failed occurrences scheduled strictly after that history's latest
retained successful occurrence. If no success remains, it counts the retained
failures. Other canonical states do not establish success. At least two such
failures qualifies as repeated in this count; this is a counting definition,
not an approved alert threshold. It is not a strict consecutive terminal streak,
retry-exhaustion count, classified failure history or complete lifetime history.

The check decodes only bounded main-schema automation IDs, occurrence states
and scheduled instants, at most 256 retained rows, in the existing snapshot.
Neither IDs nor timestamp values are returned. RFC3339 offsets are compared as
instants. Distinct retained manual/scheduled rows can share an instant through
different timestamp precision; each failed row strictly after the latest success
counts, and a failure at the success instant does not. Missing
prerequisites produce `occurrence_failure_history_store_unavailable`; malformed
metadata produces `occurrence_failure_history_metadata_invalid`; exceeding the
row cap produces `occurrence_failure_history_read_budget_exceeded`. Each omits
all three facts rather than returning a partial or zero failure summary. Valid
attempt, quarantine and retry-exhaustion facts remain independent. A completed
empty scan returns three zeros. Completed DTO groups must fit their history and
cohort bounds; an incomplete scan cannot impose a completed cohort cap.

Retry-exhaustion facts use the retained `automation_retry_state` counters,
including histories outside current quarantine. Zero means no nonzero retained
counter, not proof that no exhaustion ever occurred. These counters describe
exhausted retries, separately from consecutive failed occurrences; `failed_attempts`
is also an all-retained-attempt count, not a failure streak. No private reason
or failure-class string enters these aggregate facts. Invalid exhaustion
counters produce `invalid_persisted_metadata` and omit both exhaustion facts;
valid quarantine/attempt counts and independent categories remain available.

A missing main `automation_retry_state.consecutive_exhaustions` column reports
`retry_exhaustion_history_unavailable` and omits both exhaustion-history facts.
Independent quarantine and failed-attempt counts remain available. Counter
validation, quarantine counts and exhaustion aggregates read main, so TEMP
histories cannot supply replacement counts. Missing history differs from an
available, valid history with no recorded exhaustion. Malformed retry identifiers and other shared-query failures still need
isolation.

Quarantine counting independently requires main retry `quarantined_at`.
Missing metadata reports `quarantine_store_unavailable`, omits the quarantine
count and retains readable attempt and exhaustion facts. Concurrent unavailable
or malformed exhaustion history adds its own gap after the quarantine gap and
withholds only those history facts. A missing retry table omits both quarantine
and history facts; doctor does not recreate it. Eligibility remains explicitly
unavailable because its quarantine dependency cannot be checked. TEMP tables
cannot supply substitute evidence.

Retained scheduled-occurrence planning delay is a separate observation from
scheduler-pass lag. `scheduled_occurrences_planning_checked` counts scheduled
rows checked in the existing snapshot, and
`maximum_scheduled_occurrence_planning_delay_ms` is the largest nonnegative
`created_at - scheduled_for` difference, in milliseconds. Planning before a due
instant contributes zero delay; manual occurrences do not contribute. This is
retained history, not a complete lifetime history or an SLO result. Run-ledger
`started_at` is recorded during adoption before runtime launch, so doctor does
not relabel it actual runtime-start latency.

The check reads at most 256 retained occurrence rows, including manual rows,
using bounded metadata projections from main. More rows produce
`occurrence_planning_read_budget_exceeded` and omit both facts, rather than
reporting a partial maximum. Missing `kind`, `scheduled_for` or `created_at`
metadata produces `occurrence_planning_store_unavailable`. A malformed kind or
selected scheduled-row timestamp produces
`occurrence_planning_metadata_invalid`. Timestamp comparisons parse RFC3339
instants including offsets; NULL, non-TEXT, invalid UTF-8, oversized and
NUL-containing values are invalid. Neither timestamp values nor row identifiers
are returned. Timestamp epochs must also fit the nonnegative safe numeric
range. Independently valid pass facts and other categories remain available. A complete empty cohort has count zero and maximum zero; unavailable
or invalid evidence has neither fact. A completed scan also requires the sum of
observed disjoint queue-state counts, the lease-bearing count and the mismatched
unlaunched-claim count each to fit the 256-row cohort bound; an incomplete scan
does not establish that cap. Full lag history, actual runtime-start
instrumentation and ratified SLO comparisons remain open.

The latest-pass start lag is the nonnegative difference between its recorded
started and scheduled instants, in milliseconds. UTC offsets are parsed before
comparison. Invalid, oversized, undecodable or reversed timestamps produce
`invalid_persisted_metadata` and omit lag while retaining valid duration and
independent observations. This is a scheduler-pass measurement, not occurrence
planning/runtime launch lag or evidence of meeting a ratified SLO.

Failed-attempt counting separately requires main attempt `state`. A missing
column or table reports `attempt_state_unavailable` and omits the failed count,
retaining readable quarantine, exhaustion and failure-class observations. A
missing attempt table also makes refusal classification unavailable and is not
recreated. Repeated-failure gaps use this order: attempt state, quarantine, then
unavailable or invalid exhaustion history. Duplicate, reversed, conflicting or
extra gaps are invalid reports. With all three sources unavailable, the numeric
failure facts are empty and the explicit gaps remain; this is not zero failures.

Refusal aggregates independently require main `automation_attempts.failure_class`.
A missing column reports `attempt_failure_class_unavailable` and omits both
known refusal counts. Independently readable failed-attempt counts still use
main attempt state; TEMP shadows cannot replace either source. When the column
exists, doctor counts only the known launch-refused and unsupported-authority
classes without returning class text. Those counts do not establish complete
identity or approval-service refusal coverage. State and failure-class validation cover the retained closed vocabularies
described above. Cross-record transition history and other shared-query failures
still need complete validation and isolation.

A missing scheduler pass is unobserved. It must not become a measured zero
latency or a failed pass. A zero count of stored refusals cannot prove that
identity and approval services were available. A receipt count cannot prove
receipt validity. SQLite page statistics do not describe free disk space.

## Follow the fixed next-step code

| Next-step code | Safe investigation |
| --- | --- |
| `inspect_scheduler_status` | Run `coven automations status --json`. Distinguish a recorded pass from current process liveness. |
| `inspect_daemon_status` | Run `coven daemon status`. Preserve the report and inspect the owning migration/integrity contract before any repair. |
| `inspect_lease_metadata` | Check `coven automations leases --help` in the installed binary. If supported, page through owner-local lease metadata; otherwise retain the unsupported diagnostic result. |
| `inspect_run_and_attempts` | For a known run ID, use `coven automations run <run-id> --json` and `coven automations attempts <run-id> --json`. Compare fences and terminal state before choosing any action. |
| `review_authority_gate` | Preserve the minimized refusal evidence and check the approved identity/authority policy. Missing permission is not an instruction to widen permissions. |
| `review_changefeed_evidence` | Compare retained cursor/replay evidence with the pinned client contract. A doctor read must not issue a checkpoint to manufacture health evidence. |
| `review_storage_maintenance` | Inspect the daemon's existing storage/maintenance diagnostics and their freshness. Preserve errors and follow the approved retention policy. |

These steps inspect state. They do not authorize retry, recovery, cancellation,
retention changes or production activation. When a run may have caused an
external effect, preserve its automation/occurrence/run/attempt correlation
and require the supported reconciliation path before repeating work.

Do not delete database rows, clear fences, alter receipts, fabricate terminal
success or blindly rerun ambiguous work. Doctor does not recommend raw SQLite
mutation. Keep private logs, prompts, paths, credentials and raw errors out of
shared incident reports; use minimized codes and the relevant object IDs.

## Triage an incident without changing its state

Start with `coven --version`, `coven daemon status`, and
`coven automations doctor --json`. Use the installed binary's `--help` before
following commands from a newer contract. If doctor is unsupported, refused,
over budget or unavailable, retain that outcome; do not replace it with an
empty report or repeated reads until one happens to succeed. A later successful
read is a new observation, not proof that the earlier incident was healthy.

Read the source gaps before interpreting a zero count. A missing prerequisite
means its facts were not observed. Invalid metadata means that source could not
be interpreted safely. A budget gap means the bounded check did not complete.
None is evidence of zero affected work. Counts from independent readable
categories can still help investigate, but cannot fill the missing category.

| Finding | Next read and decision boundary |
| --- | --- |
| Assigned leader, no recent successful pass, or growing eligible work | Run `coven automations status --json` and compare pass timestamps and gaps. Assigned metadata does not prove the process is live; a failed latest pass does not erase a retained earlier success. Do not infer a ratified lag violation from these observations alone. |
| Expired leases, missing/invalid expiry, or mismatched unlaunched claims | Check `coven automations leases --help` and inspect supported pages. For an already known occurrence ID, run `coven automations occurrence <occurrence-id> --json`, then inspect its run and attempts. Expiry or generation mismatch alone does not authorize takeover, settlement or another external effect. |
| Failed attempts, quarantine, or retry exhaustion | Inspect the known occurrence, run and attempts. A retained failure total is not a consecutive failure streak; exhaustion counters are not occurrence-failure counters. Preserve that distinction before assessing repetition or a retry policy. |
| Pending/failed delivery or inconsistent receipt links | Inspect the known run and attempts, preserving their terminal states and correlation. A reciprocal index link is not authenticated receipt content or proof of the external effect. If delivery outcome is ambiguous, require the supported reconciliation path before another dispatch. |
| Authority refusal, unsupported authority, or absent identity/approval evidence | Retain the refusal codes and compare the approved authority contract. No recorded refusal does not prove approval-service availability. Permission or provider changes require their existing approval gate. |
| Missing/unrecognized schema objects or invalid migration metadata | Retain the fixed finding and installed artifact pin. Check the supported migration/integrity contract before maintenance. Historical migration counters do not validate current rows; do not run an implicit repair or integrity scan through doctor. |
| Invalid event sequences, expired checkpoints, or a cursor before retention | Compare the pinned client cursor/replay contract and existing subscriber evidence. An expired or pruned cursor may need the supported client resynchronization path; doctor does not create a replacement checkpoint or prove replay completion. |
| Low filesystem-space estimate, a large WAL sidecar, or unobserved retention pressure | Preserve sample timestamps and the existing storage-maintenance evidence. Main pages and WAL file length are separate measurements, and neither proves reclaimable bytes. Follow the approved maintenance policy; do not delete a sidecar, force a checkpoint or change retention based only on these facts. |

For a run already identified by supported inspection, the read-only correlation
sequence is:

```sh
coven automations occurrence <occurrence-id> --json
coven automations run <run-id> --json
coven automations attempts <run-id> --json
```

Substitute IDs from that incident's authorized inspection; doctor deliberately
does not return private identifiers. These commands are separate reads and can
observe later state than the doctor snapshot. Keep their observation times and
do not combine them into a claim of one atomic cross-command snapshot.

## Record the proof gap

For an incident, retain the report schema, observation time, installed version,
relevant object IDs, fact/gap codes, and the result of each safe inspection.
Record which categories remain unobserved. Preserve source and artifact pins
when comparing behavior across versions.

#1355 remains open until the full doctor requirements have delivery evidence.
The broader #858 profiles, golden scenarios, faults, privacy/security checks,
SLO ratification, platform and consumer canaries, and authenticated exact-release
eligibility remain separate requirements. Passing a limited doctor test or an
`audit_only` inventory does not satisfy those requirements.


The disposable daemon/CLI regression populates terminal occurrence, run, attempt
and receipt ledgers with large private bodies. It checks owner-local action and
both CLI renderings for a 32 KiB output ceiling, absence of fixture values and
home paths, expected failure/refusal/receipt counters, and complete application
table equality while a WAL writer is reserved. This is bounded-output and
application-data nonmutation coverage, not zero-filesystem-write, authenticated receipt, load SLO, or full
platform acceptance evidence.
