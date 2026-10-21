# Coven Automations audit runner

This directory contains the first executable Coven Automations v1 conformance
slice. It is deliberately limited to **audit-only** observations:

- it cannot emit `release_eligibility`;
- it cannot claim the aggregate `full` profile;
- an unavailable target produces `not_applicable` results and a nonzero exit;
- malformed target output produces `failed` results with static errors;
- raw target evidence, stdout, stderr, paths, and command lines are not copied
  into the result.

The output is a
[`coven.automations.conformance-result.v1`](../../../spec/coven-automations/v1/conformance-result.schema.json)
envelope. Passing output is not release certification. A verifier must still
pin the source, protocol bundle, runner, vector set, tested subject artifact,
suite inventory, and environment. Release eligibility additionally requires
trusted authentication and a release policy.

## Run

To build and audit every checked-in native suite from one clean, committed
checkout, use Node.js 24 and the repository's Rust build prerequisites:

```sh
node conformance/automations/runner/audit.mjs \
  --output /tmp/coven-automations-audit
```

Choose a new output directory outside the checkout. The audit retains the
source-bound protocol bundle, generated job, result, build provenance, and tested
binary. It builds the binary
with locked dependencies rather than assigning the current Git revision to an
unrelated installed executable. Repository inputs to compilation, protocol packaging, vectors, and
the independent runner all come from a private checkout of the pinned commit.
Editing and restoring your working files during a build cannot replace those
inputs. The reviewed `inventory.json`, not the target's
capability response, selects the required suites. Missing or non-passing suites
fail the audit. Linux CI runs this command and retains its evidence.

Source repository metadata comes from the checkout's configured `origin`,
normalized to a credential-free GitHub HTTPS URL. GitHub HTTPS, scp-style SSH
and `ssh://git@github.com/` origins are supported, including forks; missing,
local-path, credential-bearing or unsupported origins fail before building.
This local audit does not contact GitHub or prove the commit belongs to the
remote. Provenance records `sourceRepositoryInput: origin_configuration` and
`remoteCommitVerified: false`; unpublished local commits remain auditable.

The host build environment is a **caller trust assumption**, not an audited
toolchain. Cargo, rustc, wrappers, flags, Rustup selection, Cargo-home
configuration and external dependency/tool inputs are neither isolated nor
fingerprinted. Provenance explicitly records `hermetic: false`,
`toolchainAttested: false` and `reproducibility: not_assessed`; it does not echo
potentially sensitive environment values. The command is repeatable, but it
does not establish bit-for-bit reproducibility or compiler-origin attestation.
The private snapshot fixes repository inputs, and artifact digests identify
the bytes tested. Neither makes an untrusted host/toolchain trustworthy.
Trusted build and release attestation remains a separate #805 gate.

The clean-source guard also rejects ignored files. Keep build outputs outside
your audited checkout: set `CARGO_TARGET_DIR` to an external cache directory
before building or testing there. The audit treats it as a cache root and uses
a dedicated namespace for the canonical checkout and exact source commit;
it never trusts unscoped build artifacts from another checkout or revision.
The first audit of each source identity therefore requires a cold build.
Without that variable, the audit uses temporary private build storage.
Some workspace tests retain ignored fixture
evidence inside the checkout even with an external build cache, so CI audits
before running those tests. Locally, use a fresh task worktree rather than
deleting unrelated local files to satisfy the guard.

The process watchdog is an audit hang guard, not a runtime SLO. It allows for
host startup of each freshly staged executable; it does not change Coven's
daemon deadlines or certify dispatch latency.

This is debug-build, source-snapshot audit evidence, not a packed npm release,
production Runtime Authority activation, or full-profile certification. The
[readiness map](../../../docs/roadmaps/coven-automations-readiness.md) lists the
remaining gates.

To audit a separately prepared, explicitly bound job:

```sh
node conformance/automations/runner/conformance.mjs \
  --job /absolute/path/to/audit-job.json \
  --target-command /absolute/path/to/coven
```

The target command must be the exact executable represented by
`subjectArtifact.sha256`. Wrappers and extra target arguments are intentionally
unsupported because they would break the binding between the reported subject
and the implementation that actually ran. The runner reads those bytes once
and starts every probe from a fresh private copy, so one invocation cannot
replace the executable used by the next.

Exit status `0` means every requested suite passed. Status `1` means the runner
emitted a valid non-passing audit result. Status `2` means the job itself was
invalid and no result was emitted.

This initial standalone runner supports Linux and macOS. It fails closed on
Windows until target processes can be contained in a kill-on-close Job Object.
The native Coven target commands themselves remain cross-platform.

PE32+ header inspection reports x64/ARM64 metadata only. It does not establish
native execution, full image/signature validity, process containment or release
eligibility. Windows execution remains refused pending native containment evidence.

The planned Windows runner version `0.2.0-windows-job-v1` binds canonical JCS
manifest bytes under `runner.artifactSha256`, using schema
`coven.automations.windows-runner-artifact.v1`. The manifest records the immutable
common source commit, `conformance.mjs` hash, and
`coven-conformance-supervisor.exe` hash plus ABI
`coven.automations.windows-job-supervisor.v1`. These two files belong to the
runner artifact; `subjectArtifact.sha256` still names the independent target.
The metadata constructor does not authenticate the source or execute a helper.
Producer/consumer integration and native containment proof remain unfinished;
constructing this manifest does not enable Windows. Existing POSIX bindings
continue to identify the runner script bytes directly.

The private Rust `coven-conformance-supervisor` package contains creation-time
Job Object assignment, suspended launch, bounded command/header/body framing and
an independent inherited controller-lease watcher. Its common tests and Windows
cross-target checks do not prove native behavior. The helper executable,
locked-image verification, process coordinator and Node producer/consumer wiring
are unfinished. Windows refusal remains in place until actual native containment,
controller-death, blocked-I/O and artifact-binding fixtures pass. These primitives
are not a filesystem/network sandbox or Runtime Authority.

## Audit job

The job is a closed JSON object. Its source, protocol, runner, subject, and
environment fields map directly into the result statement:

```json
{
  "schemaVersion": "coven.automations.conformance-job.v1",
  "resultId": "audit-2026-09-08T120000Z",
  "decisionScope": { "kind": "audit_only" },
  "source": {
    "repository": "https://github.com/OpenCoven/coven",
    "commit": "1111111111111111111111111111111111111111"
  },
  "protocolArtifact": {
    "bundleSchemaVersion": "coven.automations.bundle.v1",
    "sourceCommit": "1111111111111111111111111111111111111111",
    "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "fileCount": 19
  },
  "runner": {
    "name": "coven-automations-conformance-runner",
    "version": "0.1.0",
    "artifactSha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "vectorSetSha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
  },
  "subjectArtifact": {
    "artifactId": "coven-cli",
    "artifactVersion": "0.1.0",
    "platform": { "os": "linux", "arch": "x86_64" },
    "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
  },
  "environment": {
    "os": "linux",
    "arch": "x86_64",
    "runtime": "node-24"
  },
  "observedAt": "2026-09-08T12:00:00.000Z",
  "suites": [
    {
      "profile": "structural",
      "suiteId": "capability-negotiation",
      "vector": {
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": [
          {
            "caseId": "supported-minimal",
            "definition": {
              "schemaVersion": 1,
              "id": "native-supported",
              "name": "Native supported",
              "status": "PAUSED",
              "rrule": "FREQ=DAILY",
              "timezone": "local",
              "prompt": "Run the native conformance probe",
              "misfire": "latest",
              "overlap": "forbid",
              "timeoutMinutes": 30,
              "runtime": "coven-code"
            },
            "expected": {
              "outcome": "supported"
            }
          }
        ]
      }
    }
  ]
}
```

The example values are placeholders, not evidence. Use the exact SHA-256 and
source metadata for the files and binary under test. `source.commit` must equal
`protocolArtifact.sourceCommit`. The runner also recomputes and requires:

- `runner.artifactSha256` from `conformance.mjs`;
- `runner.vectorSetSha256` from the JCS bytes of the job's `suites` array;
- `subjectArtifact.sha256` from `--target-command`.

The checked-in
[`attempt-terminal-immutability.vectors.json`](attempt-terminal-immutability.vectors.json),
[`capability-negotiation.vectors.json`](capability-negotiation.vectors.json),
[`calendar-schedule-resolution.vectors.json`](calendar-schedule-resolution.vectors.json),
[`cancellation-timeout-arbitration.vectors.json`](cancellation-timeout-arbitration.vectors.json),
[`command-adoption-idempotency.vectors.json`](command-adoption-idempotency.vectors.json),
[`legacy-import-source-preservation.vectors.json`](legacy-import-source-preservation.vectors.json),
[`command-envelope-outcomes.vectors.json`](command-envelope-outcomes.vectors.json),
[`command-transaction-rollback.vectors.json`](command-transaction-rollback.vectors.json),
[`definition-lifecycle-transitions.vectors.json`](definition-lifecycle-transitions.vectors.json),
[`definition-validation.vectors.json`](definition-validation.vectors.json),
[`event-reducer-determinism.vectors.json`](event-reducer-determinism.vectors.json),
[`misfire-latest-planning.vectors.json`](misfire-latest-planning.vectors.json),
[`mission-commands.vectors.json`](mission-commands.vectors.json),
[`occurrence-fence-uniqueness.vectors.json`](occurrence-fence-uniqueness.vectors.json),
[`occurrence-lease-recovery.vectors.json`](occurrence-lease-recovery.vectors.json),
[`overlap-forbid-claiming.vectors.json`](overlap-forbid-claiming.vectors.json),
[`retry-backoff-timing.vectors.json`](retry-backoff-timing.vectors.json),
[`retry-quarantine-recovery.vectors.json`](retry-quarantine-recovery.vectors.json),
[`runtime-authority-terminal-recovery.vectors.json`](runtime-authority-terminal-recovery.vectors.json),
[`scheduler-leadership-fencing.vectors.json`](scheduler-leadership-fencing.vectors.json),
[`startup-reconciliation-wake.vectors.json`](startup-reconciliation-wake.vectors.json),
[`result-delivery-recovery.vectors.json`](result-delivery-recovery.vectors.json),
[`result-delivery-process-crash-recovery.vectors.json`](result-delivery-process-crash-recovery.vectors.json),
[`run-settlement-process-crash-recovery.vectors.json`](run-settlement-process-crash-recovery.vectors.json),
[`run-launch-process-crash-recovery.vectors.json`](run-launch-process-crash-recovery.vectors.json),
[`attempt-retry-process-crash-recovery.vectors.json`](attempt-retry-process-crash-recovery.vectors.json),
[`occurrence-recovery-process-crash-recovery.vectors.json`](occurrence-recovery-process-crash-recovery.vectors.json),
[`run-cancellation-process-crash-recovery.vectors.json`](run-cancellation-process-crash-recovery.vectors.json),
[`flat-cancellation-process-crash-recovery.vectors.json`](flat-cancellation-process-crash-recovery.vectors.json),
[`receipt-integrity-validation.vectors.json`](receipt-integrity-validation.vectors.json),
[`rrule-vocabulary.vectors.json`](rrule-vocabulary.vectors.json), and
[`run-terminal-monotonicity.vectors.json`](run-terminal-monotonicity.vectors.json)
files contain the current nonempty vector sets.

## Target protocol

The runner invokes the target directly without a shell.

`<target> automations conformance capability` returns:

```json
{
  "schemaVersion": "coven.automations.conformance-target-capability.v1",
  "profiles": [
    {
      "profile": "structural",
      "suites": [
        "attempt-terminal-immutability",
        "capability-negotiation",
        "command-adoption-idempotency",
        "command-envelope-outcomes",
        "definition-lifecycle-transitions",
        "definition-validation",
        "event-reducer-determinism",
        "mission-commands",
        "occurrence-fence-uniqueness",
        "receipt-integrity-validation",
        "rrule-vocabulary",
        "run-terminal-monotonicity"
      ]
    },
    {
      "profile": "scheduler_reliability",
      "suites": [
        "calendar-schedule-resolution",
        "cancellation-timeout-arbitration",
        "command-storage-lock-recovery",
        "command-transaction-rollback",
        "legacy-import-source-preservation",
        "misfire-latest-planning",
        "occurrence-lease-recovery",
        "overlap-forbid-claiming",
        "retry-backoff-timing",
        "retry-quarantine-recovery",
        "scheduler-leadership-fencing",
        "startup-reconciliation-wake"
      ]
    },
    {
      "profile": "runtime_authority",
      "suites": ["runtime-authority-terminal-recovery"]
    }
  ]
}
```

For each advertised suite, the runner invokes
`<target> automations conformance evaluate`, writes one
`coven.automations.conformance-suite-request.v1` object to standard input, and
expects one `coven.automations.conformance-suite-result.v1` object on standard
output.

The native Coven target implements twelve structural suites, twenty-three
scheduler-reliability suites, and one bounded Runtime Authority recovery suite.
`attempt-terminal-immutability` executes every terminal attempt state against
the production SQLite ledger and proves that later updates and deletion are
refused. `capability-negotiation` executes the checked-in cases against Rust's
real routine-definition parser and capability policy.
`command-adoption-idempotency` executes a valid definition create through the
production transactional adoption path, then proves that an exact replay
returns the committed result without duplicating the definition, adoption, or
event while a changed request under the same adoption key is refused with
`ADOPTION_REPLAY_MISMATCH`.
`command-envelope-outcomes` sends portable `coven.automations.v1` command
envelopes, in order, through the producer's `coven.automations.command.v1`
router to one durable store per case, closing and reopening the store where a
case restarts. Its six cases cover a committed create, revise and read;
refusals; `REVISION_CONFLICT` and `ADOPTION_REPLAY_MISMATCH`; a duplicate
delivery that replays the original result and event without a new revision; and
adoption that survives a restart; and cancellation refusals with duplicate
delivery and changed-target conflicts. Around every rejection the target compares
each automation table. A rejection never changes definitions, events or
execution state. A command the producer does not implement is refused before
adoption and writes nothing; one it implements retains its domain rejection
under the adoption key, so the exact request replays it.
`definition-lifecycle-transitions` executes the complete versioned definition
lifecycle graph through the production command transaction: creation into
paused or active, active/paused revision, disable, tombstone, and fail-closed
refusal of disabled reactivation and every transition out of a tombstone.
`definition-validation`
executes the portable v1 definition parser, integrity verification, and typed
serialization path, accepting a canonical valid definition only when its
normalized JCS digest matches and rejecting a tampered definition.
`event-reducer-determinism` replays a portable event stream through the native
Rust reducer both canonically and with an exact duplicate delivery, requiring
the same normalized final state and expected digest.
`mission-commands` sends `mission.submit.v1`, `mission.get.v1` and
`mission.cancel.v1` to a store holding one familiar whose first revision has
been superseded, on a clock that advances a minute per step. It covers the
vectors M0 can reach (M-01 to M-06, M-09, M-11 and M-15 in
`docs/architecture/coven-automations-missions.md`): a replayed submit, a reused
key, a stale revision, an unenforced envelope, the no-launch settlement, a
cancel after it, another caller, and every bound. It checks what each refusal
leaves behind, as `command-envelope-outcomes` does. Values that exist only at
run time, such as the familiar's ids and `validUntil`, are whole-string tokens
such as `{{revisionId}}` and `{{start:10m}}`.
`calendar-schedule-resolution` executes fixed UTC and IANA-timezone cases
through the production scheduler. It covers same-day and multi-hour daily
schedules, weekly weekday selection, winter and summer offsets, skipped DST
gaps, deterministic selection of the first DST-fold occurrence, leap-day,
month, and year boundaries, and fail-closed refusal of unresolved `local`
timezones.
`cancellation-timeout-arbitration` executes two synchronized races through the
production cancellation and timeout stop-fence paths. It proves that an
in-flight cancellation prevents timeout from issuing a duplicate runtime stop,
that timeout reconciliation rejects a pre-existing requested cancellation
before a paused stale executor can claim stop ownership, that each race issues
exactly one runtime stop, and that replay plus durable run, occurrence, attempt,
and cancellation states preserve the winning authority.
`misfire-latest-planning` executes fixed virtual-time cases through the
production occurrence planner. It proves that downtime collapses daily work to
the latest due slot, exact replay is idempotent, a clock rollback does not add
an older fence, and paused definitions do not plan.
`occurrence-lease-recovery` executes fixed virtual-time cases through the
production expired-lease recovery path. It proves that pre-dispatch claims are
recovered after and exactly at lease expiry, unexpired claims remain owned, and
runtime-owned or runtime-evidenced work is never treated as safe to fail from a
lease timeout alone.
`overlap-forbid-claiming` executes fixed virtual-time cases through the
production atomic occurrence claim path. It proves that claimed or running
occurrences and an independently running run block new work, while succeeded,
failed, and cancelled occurrences release overlap capacity.
`retry-backoff-timing` executes fixed virtual-time cases through the production
retry timing primitive used by rejected-launch and lease-expiry recovery. It
proves immediate and fixed delays are measured from failure observation,
exponential full-jitter ceilings grow by attempt, deterministic replay is
stable for one run and attempt, and the ceiling is capped at one day.
Exponential vectors pin the scheduled delay derived from the first eight bytes
of `BLAKE3("<runId>:<nextAttemptNumber>")`, interpreted as an unsigned
big-endian integer and mapped to `1..=ceiling` with modulo arithmetic.
`retry-quarantine-recovery` executes fixed virtual-time cases through the
production retry-state ledger and scheduler tick. It proves first and repeated
exhaustions quarantine a routine with current failure evidence, quarantine
blocks occurrence planning, explicit release clears the state and restores
scheduling, and release without an existing quarantine is an idempotent no-op.
`scheduler-leadership-fencing` executes fixed virtual-time cases through the
production filesystem leadership lock, durable scheduler generation, and
fenced scheduler tick. It proves a concurrent scheduler process is refused, a
restart advances the generation and invalidates the prior fence, and stale
leaders cannot tick while the current leader can.
`startup-reconciliation-wake` executes fixed virtual-time cases through the
production daemon scheduler loop and control action route. It proves due work
is reconciled and dispatched before the scheduler's first wait, and that an
accepted definition revision wakes the sleeping scheduler for an immediate
pass rather than waiting for the periodic deadline. The definition-change
case binds its mutation timestamp in the vector, so evaluation is independent
of the target host's wall clock.
`runtime-authority-terminal-recovery` covers five terminal observations:
completed with exit zero, failed with a nonzero exit, confirmed cancellation,
confirmed timeout, and idle with unknown exit. It starts from a fixed,
correlated post-launch fixture persisted through the production store APIs,
then invokes production reconciliation, confirmed-stop settlement, and scheduler
dispatch probes. Repeated passes and store reopen must preserve recovery hold
and unresolved run/attempt state, without fabricating receipts, sidecars, or
receipt events, or creating automatic retries or duplicate launches. This
fixture does not exercise an actual authority launch, harness process, trusted
authentication adapter, or evidence consumer.
`occurrence-fence-uniqueness` executes a portable three-case matrix against the
production SQLite occurrence schema, proving that one automation cannot claim
the same scheduled slot twice while different automations may share a slot and
one automation may claim distinct slots.
`receipt-integrity-validation` parses portable receipts through the native
typed receipt contract. It requires a canonical receipt with a matching JCS
SHA-256 integrity value and a minimally tampered receipt whose unchanged
integrity value must be rejected. Accepted receipts are compared by a pinned
digest of their normalized typed representation; receipt contents are not
returned as target evidence.
`rrule-vocabulary` executes a fixed portable matrix through the production
Rust RRULE parser. It covers daily and weekly defaults, hour and weekday
normalization, and fail-closed rejection of unsupported frequencies and keys,
invalid combinations, duplicate parts or values, malformed parts, empty
segments or values, and out-of-range inputs.
`run-terminal-monotonicity` executes the checked-in settlement and replay cases
against the real Rust run ledger in an isolated in-memory store, proving that a
later terminal observation cannot rewrite the first committed terminal state.
Each vector must use distinct first and replay statuses so a passing case
exercises an actual conflict. These suites do not claim the complete occurrence
or attempt state machines.

The runner computes a JCS SHA-256 digest of returned evidence and discards the
raw evidence after building the result envelope. Evidence is restricted to JSON
values that can be canonicalized consistently across the JavaScript runner and
Rust verifier. The native target rejects evaluation requests larger than one
MiB before JSON parsing.
Each target operation has a two-second deadline followed by process-tree
termination; output is capped at one MiB.

## Command storage-lock audit boundary

The `command-storage-lock-recovery` suite uses two connections to a disposable
file-backed SQLite store. A held write lock must produce `SQLITE_BUSY` without
writing a definition, adoption or event. Releasing that lock permits the same
command/key to commit; closing and reopening the store must preserve one copy
and the original replay result, event reference and commit timestamp. An
unlocked control exercises the same command without contention.

This is audit-only evidence for command-adoption lock refusal and recovery.
It does not certify process crashes, disk-full, I/O errors, corruption,
permission loss, runtime dispatch, or release eligibility. The zero busy wait
belongs only to the fixture connection; production deadlines are unchanged.

The `command-storage-readonly-recovery` suite creates a private disposable store,
seeds unrelated definition/adoption/event and authority records, then closes it.
A fresh `SQLITE_OPEN_READ_ONLY` connection must refuse the production definition
command with actual `SQLITE_READONLY`. The complete persisted projections must
remain unchanged and the transaction must close. A fresh writable connection
then retries the same key once; a second reopen must replay the exact result,
event reference, revision, and first commit timestamp without further writes.
A writable control exercises the same command without the refusal. Machine
readable observations are included in the suite evidence.

This is audit-only evidence for SQLite read-only mode. It does not prove OS
permission loss, full-disk or I/O recovery, corruption tolerance, runtime
execution, complete conformance profiles, or release eligibility. The fixture
never opens an existing user store, launches a provider, or changes production
connection permissions.
### Command transaction rollback audit

`command-transaction-rollback` exercises the real definition-command transaction
with rich v1 draft definitions in disposable file-backed stores. Mandatory cases
inject SQLite trigger aborts at event insertion and final adoption insertion,
plus an uninjected control. Each abort must leave zero definition, rich revision,
event, stream-head and adoption rows, including after reopening and repeating the
same request while the fault remains installed. Removing the trigger permits the
same adoption key to commit; another reopen must replay the original result,
revision, event reference and first-commit timestamp with exactly one durable
record of each kind. The stored event must match the returned stream and sequence.

These are deterministic transaction-abort probes. They do not prove process-kill
recovery, disk-full, filesystem I/O/corruption, runtime authority, or release
eligibility. The fixtures remain draft and launch no runtime. Invalid definitions,
missing/duplicate fault cases, and weakened expected invariants are refused with
static errors before executing any case.

## Legacy import audit boundary

The `legacy-import-source-preservation` suite calls existing import actions
through an identity-bound owner-local daemon in a disposable home. Its two
routes distinguish legacy `PAUSED`/`paused` records from v1 `PAUSED`/`draft`
records. Source definitions request ACTIVE status; import must keep them inactive.
The fixture compares source bytes and inventory, preserves a preexisting
definition, exercises duplicate IDs and unsupported schedules, and checks v1
dry run and adoption replay. It compares persisted records across daemon restart
and requires zero occurrences, runs and attempts. Case observations expose only
fixed state/outcome classifications, counts, preservation booleans and case IDs;
they are retained for both passing and failed expectations.

This adds audit-only evidence toward golden scenario 20. It does not certify
runtime launch, principal-scoped disclosure, broad crash boundaries, all
platform containment, complete profiles or release eligibility. The daemon
receives an isolated home and no provider environment. The runner uses a short
owned scratch path on macOS for Unix sockets and a separate fixture hang guard;
production startup and maintenance deadlines are unchanged.

## Command SQLite capacity audit boundary

The `command-storage-capacity-recovery` suite initializes a disposable file-backed
store, seeds an unrelated definition, then caps SQLite `max_page_count` at the
current page count. A bounded synthetic 128 KiB prompt forces page growth through
the production definition-command API. The fault case requires actual
`rusqlite::ErrorCode::DiskFull` (`SQLITE_FULL`); the other case is unconstrained.
The fault source is `sqlite_max_page_count`. This does not cover physical disk
exhaustion, permission loss, I/O errors or corruption. No runtime is launched.

Private snapshots compare every row in definitions, rich revisions, owner
principal/grants, command adoptions/reservations and events after failure. The
fixture lifts its quota, retries the same command/key, checks one added
definition/adoption/event, reopens the store and requires stable replay without
additional writes. Synthetic payloads and database snapshots remain internal.
Unsupported fixture conditions and malformed vectors fail with static errors.
This remains `audit_only`; it supplies no production release eligibility.

## Command process-crash audit boundary

`command-process-crash-recovery` uses three abrupt process-termination cases
and an uninterrupted commit control. Each worker is the exact audited binary,
creates a new private child directory under parent-owned scratch, initializes
a disposable file-backed store, and verifies a preexisting paused definition.
No worker input can select an existing database. No providers or runtimes launch.

Fixture-only TEMP triggers call a local SQLite callback after the definition,
event or adoption INSERT. The callback announces the boundary and parks. The
parent verifies the WAL header, frame salts and rolling checksums, and
requires complete frames with zero commit markers before abruptly killing and
reaping the child. The disposable connection uses a one-page cache and cache
spill after a verified truncating checkpoint. Production cache/durability
settings do not change. SQL abort and panic unwinding are not crash evidence.

Raw reopen checks integrity and complete snapshots before production schema
initialization. Definitions, rich revisions, owner principal/grants, adoptions,
reservations, events and stream heads must retain the baseline without partial
writes. Retrying the same command/key must add one definition/adoption/event
and its stream head, with a correlated event reference. Reopen/replay must
preserve the full response and first commit time without additional writes.

Worker handshakes, private snapshots and synthetic prompts remain internal.
Exported observations contain fixed boundary/outcome classes, counts and
preservation booleans. Child input/output and waits are bounded; error cleanup
kills/reaps the child before removing scratch. The runner's two-minute fixture
hang guard is not a measured recovery SLO. Native-target tests run on Windows;
standalone Windows runner containment remains unsupported.

This adds `audit_only` command transaction crash coverage. It does not cover
external dispatch crashes, physical disk exhaustion, permission/I/O/corruption,
complete profiles, production authority, or release eligibility.

## Command SQLite corruption audit boundary

The `command-storage-corruption-recovery` suite seeds a disposable private
store through production owner-local commands: an active definition, which
records its owner grant, and a richly authored draft. The fixture then inserts
a settled occurrence/run/attempt chain and a pending runNow result reservation
with SQL, so every compared table holds rows before the fault. After a truncating checkpoint it
copies the main database to a path that has never been opened, then sets one
byte to zero: the first header byte (`sqlite_header`), or the first byte of the
B-tree root page of the definition table (`definition_btree`) or of the command
adoption table (`adoption_btree`). A fourth case writes nothing. Vectors cannot
choose a path, offset or bytes.

The production store initializer opens the copy cold, and the production
definition command runs only if that open succeeds. The header case must fail
with `rusqlite::ErrorCode::NotADatabase`, both B-tree cases with
`DatabaseCorrupt`, and the control must commit. Each vector states the stage
that must refuse its fault, and the suite fixes that stage per fault. The cold
initializer reads the header and the definition table, so it refuses those two
(`cold_open`), and the command is never reached. It never reads the adoption
table's root page, so the command itself must refuse that fault (`command`).
The command writes the definition row and its lifecycle event (stream head and
event row) before it inserts the adoption row. The adoption case must show that
the command had completed at least one write before it was refused
(`refusedAfterEarlierWrites`, from SQLite's total-change counter), that the
connection is left outside a transaction, and that none of those writes
persisted. If a future initializer refused the adoption fault at cold open, the
case would no longer exercise the command, so the suite fails rather than
silently losing that coverage. A refusal must
leave the corrupted main file byte-for-byte as written, which proves it wrote
nothing there. The suite then restores only the damaged byte and requires every
other main, WAL, shared-memory and rollback-journal byte to be unchanged. A read-only inspection must find every seeded row
intact and no row for the refused key before any writable reopen. Replacing the
file with a backup or deleting the WAL would hide a refused operation's writes,
so the fixture does neither.

The same command and key then commit once. A reopen must replay the original
result and event reference without further writes. Observations carry fixed
fault, error and stage classes, per-table counts and booleans, never offsets,
paths, SQL, native errors or row values. Restoring the byte is a fixture step,
not operator guidance or a production repair. This is `audit_only` evidence
for three deterministic corruption modes. Corruption on a page that neither the
initializer nor the command reads stays latent, and the command commits around
it; no production integrity scan exists, and this suite does not claim one. It
does not cover other corruption patterns, WAL-frame corruption, I/O faults,
disk exhaustion, process crashes, complete profiles or release eligibility.

## Result delivery audit boundary

The `result-delivery-recovery` suite drives a settled run's `pending`
main-session delivery obligation through the production discharge path,
`deliver_pending_results`. Each of six cases uses its own disposable private
store and Coven home. The fixture stores the vector's definition, which must
target the main session, and settles a run through the production run record,
which writes the obligation. Vectors cannot choose a path, run id or fault
mechanics. The definition they supply must be paused, target the main session
and carry no working directory, and every expected outcome is fixed per fault.

- **Control:** the first pass delivers one notice, and the run's record names
  it.
- **Held gate:** the fixture holds the main-session scope gate. The pass defers
  the run, which stays `pending` with no notice. After release, it delivers
  once.
- **Full inbox:** the scope already holds the most pending notices it keeps.
  The pass defers the run and evicts none of the waiting notices. After the
  fixture marks them delivered, as a turn would, the run delivers once.
- **Discharge rollback:** a fixture TEMP trigger on the delivering connection
  aborts the `delivered` record inside the discharge transaction, and only once
  the notice it names is visible there. The pass fails with that abort, the
  notice rolls back with the record, and the run stays `pending`. With the
  trigger removed, it delivers once. A failed discharge ends the whole pass, so
  runs after it wait for the next pass.
- **Gate unavailable:** a file stands where the gate's lock directory belongs.
  The run is recorded `failed` with the fixed gate reason and no notice. A
  later pass with the directory restored neither retries nor reopens it.
- **Unreadable result:** the run is launched through the production runner with
  a no-op runtime, and its session holds a malformed output event. It alone is
  recorded `failed` with the fixed unreadable reason, while a healthy run that
  settled after it is delivered in the same pass.

Every case then runs a further pass that must change nothing. The settled
record must refuse, through the production immutability trigger, to point at
another notice or to reopen, and must read back unchanged; a failed record
names no notice. Observations carry fixed pass, state, reason and fault
classes, counts and booleans. They never carry run or notice ids, notice text,
paths or output.

This is `audit_only` evidence for deferral, transactional rollback and failure
isolation. A process kill during delivery is the next suite's. This one does
not cover the offer to an idle live session, per-familiar or other principals'
scopes (#857), delivery SLOs, load, complete profiles or release eligibility.

## Result delivery process-crash audit boundary

The `result-delivery-process-crash-recovery` suite kills the process that is
delivering a result. Each of four cases starts the exact audited binary as a
hidden worker, with the same scrubbed environment and parent-owned scratch
root as `command-process-crash-recovery`. The worker creates a private Coven
home and disposable store, stores the fixed synthetic paused definition, settles
one run with a `pending` main-session obligation through the production run
record, and runs `deliver_pending_results`. Vectors cannot choose a path, run,
definition or pause mechanics, and every expected outcome is fixed per boundary.

- **After notice:** a fixture TEMP trigger calls a local callback after the
  result notice insert, inside the delivery transaction.
- **After record:** the same, after the `delivered` update and before commit.
- **After commit:** the delivering connection's WAL hook calls the callback
  once SQLite has committed the delivery, while the worker still holds the
  scope gate.
- **Control:** no pause. The worker delivers once and exits.

The callback announces the boundary and parks. While the worker is paused, the
parent must fail to take the main-session gate and to begin a write
transaction before the commit, and must read the run as `pending` before the
commit and `delivered` after it. The parent then kills and reaps the worker.
SQLite may still hold a killed transaction's pages in memory, so these cases
do not require uncommitted frames in the WAL; `command-process-crash-recovery`
proves recovery from spilled frames.

After the kill, the gate must become free, and the store's raw integrity check
must pass before the production initializer opens it. Before the commit, the
run is `pending` with no notice; after it, the run is `delivered` and its
record names its one notice. A recovery pass in the parent delivers the
pending run once, or changes nothing for a delivered one. A further pass from a
fresh connection must change nothing.

Observations carry boundary, state and pass classes, counts and booleans. They
never carry run or notice ids, notice text, paths, handshakes or output. This
is `audit_only` evidence for a process kill inside one delivery. It does not
cover power loss or unsynced pages, a kill inside a failure record, the idle
session offer, per-familiar scopes (#857), load, complete profiles or release
eligibility.

## Run settlement process-crash audit boundary

The `run-settlement-process-crash-recovery` suite kills the process that is
settling a run whose session has ended. Settlement writes the occurrence's
terminal state, the attempt's terminal state, and the run's terminal status
with its pinned delivery obligation, in one transaction. Each of five cases
starts the exact audited binary as a hidden worker with the same harness as
`result-delivery-process-crash-recovery`. The worker stores the fixed synthetic
paused, main-session-targeted definition, launches one run through
`run_routine_now` with a no-op runtime, ends its session with exit code 0, and
runs `settle_finished_runs`. Vectors cannot choose a path, run, definition or
pause mechanics, and every expected outcome is fixed per boundary.

- **After occurrence, after attempt, after run:** a fixture TEMP trigger
  pauses the worker after that terminal update, inside the settlement
  transaction.
- **After commit:** the settling connection's WAL hook pauses it once SQLite
  has committed the settlement.
- **Control:** no pause. The worker settles the run and exits.

While the worker is paused, the parent must fail to begin a write transaction
before the commit and read the run, attempt and occurrence exactly as the
launch left them. After the commit, it must begin one, and read all three
settled with a `pending` obligation. The parent then kills and reaps the
worker, and the store's raw integrity check must pass.

After the kill, a run killed before the commit must be unsettled with no
obligation, and one killed after it settled with its obligation pending; no
notice exists either way. The parent then runs the settlement and delivery
steps of the daemon's reconciliation, in its order, with no runtime and so no
idle-session offer. The steps before them recover containment, expired
cancellations, abandoned launches and timeouts, none of which these cases
hold. It must settle a killed-before-commit run once, never settle a settled
run again, and deliver each run's result once, with the record naming its one
notice. A store that recovery refuses fails the case. A further
reconciliation from a fresh connection must change nothing.

Observations carry boundary, settlement, state and pass classes, counts and
booleans. They never carry run, attempt, occurrence, session or notice ids,
notice text, paths, handshakes or output. As with delivery, these cases do not
require uncommitted frames in the WAL. This is `audit_only` evidence for a
process kill inside one plain successful settlement. It does not cover kills
during dispatch or launch, held-for-retry, timeout, cancellation or Runtime
Authority settlement, power loss, load, complete profiles or release
eligibility.

## Run launch process-crash audit boundary

The `run-launch-process-crash-recovery` suite kills a daemon worker while it
launches a claimed scheduled occurrence, then restarts the daemon's scheduler
over the store it left. The launch writes the session as `created`, starts the
run and marks its first attempt `dispatching` in one transaction. The runtime
launch then publishes ownership in a second: the session, attempt and
occurrence become `running`, `started` and `running`. Each of six cases starts
the exact audited binary as a hidden worker with the same harness as
`run-settlement-process-crash-recovery`. The worker stores the fixed synthetic
active daily definition, takes scheduler leadership, runs the fenced tick that
plans and claims the 09:00 slot, and runs the fenced dispatch with a no-op
runtime. Vectors cannot choose a path, run, definition, clock or pause
mechanics, and every expected outcome is fixed per boundary.

- **After session, after dispatching:** a fixture TEMP trigger pauses the
  worker after the session insert or the attempt's `dispatching` update, the
  launch transaction's first and last writes.
- **After launch commit:** the dispatching connection's WAL hook pauses it once
  SQLite has committed the launch.
- **After ownership:** a fixture TEMP trigger pauses it after the occurrence's
  `running` update, the ownership transaction's last write.
- **After ownership commit:** the WAL hook pauses it once SQLite has committed
  ownership, its second commit.
- **Control:** no pause. The worker dispatches and exits without the graceful
  shutdown's reconciliation or leadership release.

While the worker is paused, the parent must find the write transaction open
exactly when the pause is inside one. Before the launch commits, it must read
the occurrence claimed with no run, attempt or session. After the launch
commits, and inside ownership, it must read the launch. After ownership
commits, it must read the occurrence, run and session `running`. The parent
then kills and reaps the worker. The store's raw integrity check must pass, and
the store must hold what the parent read while the worker was paused.

The parent then restarts in the daemon's startup order, through production
entry points and without the scheduler thread. Session recovery orphans
`running` sessions; it never fails the launch's `created` session, which a
running run owns. Scheduler leadership follows, with a bounded wait because
Windows documents a delay before it releases a dead holder's lock. The
scheduler's own startup recovery and its startup pass come last.
That pass runs with the startup cutoff, on a runtime that counts launches. A
claim with no launch must be restored and launched exactly once. A launch or
published ownership must never launch again: restart containment kills the
session, and the run is held for an operator retry. The relaunched session then
ends with exit code 0. An ordinary pass before the run's deadline must settle
the relaunched run `succeeded` and keep the held run held. A pass after the
deadline must settle the held run `failed`. Every case ends with one session in
the store, and no pass after the restart's may launch. A store that recovery
refuses fails the case. A further pass must change no occurrence, run, attempt,
event or session row.

Observations carry boundary, launch, state and pass classes, counts and
booleans. They never carry run, attempt, occurrence or session ids, paths,
handshakes or output. As with delivery, these cases do not require uncommitted
frames in the WAL. This is `audit_only` evidence for a daemon killed while it
launches one scheduled occurrence with no no-process containment receipt. It
does not cover a real runtime process, receipt-proven no-process retry, which
relaunches by design, Runtime Authority launches, manual
occurrences, kills during held-for-retry, timeout or cancellation, power loss,
load, complete profiles or release eligibility.

## Operator retry process-crash audit boundary

The `attempt-retry-process-crash-recovery` suite kills a daemon worker while it
handles the operator's `attempt.retry.v1` request for a held run. The parent
then restarts the daemon's scheduler and sends the identical request again,
with the same adoption key, as an operator who never saw an answer would. The
retry is one transaction. It opens attempt 2 as `adopted`, an operator retry
of attempt 1's failure. It returns the occurrence to `planned` and releases the
run's session. Last, it stores the response under the request's adoption key.
Each of six cases starts the exact audited binary as a hidden worker, with the
same harness and daemon restart as `run-launch-process-crash-recovery`. The
worker stores a fixed synthetic active daily definition with no automatic
retry, and claims and launches its 09:00 slot through a no-op runtime. The
session then ends with exit code 1, and an ordinary pass holds the run for an
operator retry. Vectors cannot choose a path, run, definition, adoption key,
clock or pause mechanics, and every expected outcome is fixed per boundary.

- **After attempt, after replan, after release, after adoption:** a fixture
  TEMP trigger pauses the worker after each of the retry transaction's four
  writes, in order.
- **After commit:** the WAL hook pauses it once SQLite has committed the retry.
- **Control:** no pause. The worker answers the request and exits without the
  graceful shutdown's reconciliation or leadership release.

While the worker is paused, the parent must find the write transaction open
exactly when the pause is inside one. Inside it, the parent must read the held
run with no attempt 2 or stored answer. After the commit it must read the
whole retry. The parent then kills and reaps the worker. The store's raw
integrity check must pass, and the store must hold what the parent read while
the worker was paused.

After the restart's startup pass, a committed retry must have dispatched
attempt 2 once, and a held run must be untouched. The re-sent request must then
commit the retry if it was lost. If it had committed, the request must replay
the stored response, which names the attempt 2 the store opened earlier, even
though that attempt has since started. The next pass dispatches a retry that
has not yet launched. Its session ends with exit code 0, and a pass settles the
run `succeeded`. Every case ends with one launch since the restart, two
sessions and the request's one stored answer. A store that recovery refuses,
or a request that errors, fails the case. The same request and a further pass
must then change no occurrence, run, attempt, event, session or adoption row.

Observations carry boundary, retry, answer and state classes, counts and
booleans. They never carry run, attempt, occurrence or session ids, adoption
keys, responses, paths, handshakes or output. This is `audit_only` evidence for
a daemon killed while it handles one operator retry of a failed scheduled run.
It does not cover Runtime Authority runs, which this command refuses,
`occurrence.recover.v1`, cancellation, retries of a timed-out or cancelled
attempt, a graceful daemon restart, a real runtime process, power loss, load,
complete profiles or release eligibility.

## Owner recovery process-crash audit boundary

The `occurrence-recovery-process-crash-recovery` suite kills a daemon worker
while it handles the owner's `occurrence.recover.v1` request for an occurrence
whose stop could not be confirmed. The parent then restarts the daemon's
scheduler and sends the identical request again, with the same adoption key,
as an owner who never saw an answer would. Each of thirteen cases starts the
exact audited binary as a hidden worker, with the same harness and daemon
restart as `run-launch-process-crash-recovery`. The worker stores a fixed
synthetic active daily definition with no automatic retry, and claims and
launches its 09:00 slot through a no-op runtime. The owner's `run.cancel.v1`
then runs through a runtime that never confirms the stop. The daemon's API
fences a stop on the wall clock; the worker fences it on the fixture's fixed
clock, so its times agree with the scheduler's. The attempt becomes
`ambiguous`, the occurrence `recovery_required`, and the cancellation's stop
fence stays on the run. A daemon restart then marks the session `orphaned` and
launches nothing. Vectors cannot choose a path, run, definition, determination
statement, adoption key, clock or pause mechanics, and every expected outcome
is fixed per determination and boundary.

Each determination is one transaction that releases the run's stop fence and
stores the response under the request's adoption key last:

- **`retry_with_new_attempt`** opens attempt 2 as `adopted`, an operator
  recovery of attempt 1's ambiguity. It returns the occurrence to `planned` and
  releases the run's session.
- **`failed_deterministic`** settles the occurrence `failed`, then finishes the
  run `failed`. It launches nothing.

For each determination:

- **After each write:** a fixture TEMP trigger pauses the worker after each
  of the transaction's writes, in order. That is the attempt insert, the
  occurrence replan, the session release, the fence release and the adoption
  insert for a retry. For a failed determination it is the occurrence
  settlement, the run finish, the fence release and the adoption insert.
- **After commit:** the WAL hook pauses it once SQLite has committed the
  determination.
- **Control:** no pause. The worker answers the request and exits without the
  graceful shutdown's reconciliation or leadership release.

While the worker is paused, the parent must find the write transaction open
exactly when the pause is inside one. Inside it, the parent must read the
occurrence still waiting for the owner under the cancellation's fence, with no
attempt 2 or stored answer. After the commit it must read the whole
determination, with no fence. The parent then kills and reaps the worker. The
store's raw integrity check must pass, and the store must hold what the parent
read while the worker was paused.

After the restart's startup pass, a committed retry must have dispatched
attempt 2 once. For a committed failed determination and an unresolved
occurrence, the restart must change no occurrence, run, attempt, event,
session, adoption, cancellation or stop-fence row. The re-sent request must
then commit the determination if
it was lost. If it had committed, the request must replay the stored response,
which names the store's run and, for a retry, the attempt 2 it opened earlier,
even though that attempt has since started. For a retry, the next pass
dispatches an attempt that has not yet launched. Its session ends with exit
code 0, and a pass settles the run `succeeded`, which needs the run's fence
released. A retry case ends with one launch since the restart and two
sessions. A failed case ends with no launch, one session, and a `failed` run
and occurrence. Every case ends with the request's one stored answer. A store
that recovery refuses, or a request that errors, fails the case. The same
request and a further pass must then change no occurrence, run, attempt,
event, session, adoption, cancellation or stop-fence row.

Observations carry determination, boundary, occurrence, answer and state
classes, counts and booleans. They never carry run, attempt, occurrence or
session ids, adoption keys, responses, paths, handshakes or output. This is
`audit_only` evidence for a daemon killed while it handles one owner recovery
after an unconfirmed cancellation of a scheduled run. It does not cover
Runtime Authority runs, which this command refuses, a recovery after a
timeout's or an abandoned launch's unconfirmed stop, a daemon killed inside
the cancellation itself, a graceful daemon restart, a real runtime process,
power loss, load, complete profiles or release eligibility.

## Owner cancellation process-crash audit boundary

The `run-cancellation-process-crash-recovery` suite kills a daemon worker
while it handles the owner's `run.cancel.v1` request for a running scheduled
run. The parent then restarts the daemon's scheduler and sends the identical
request again, with the same adoption key, as an owner who never saw an
answer would. Each of eighteen cases starts the exact audited binary as a
hidden worker, with the same harness and daemon restart as
`run-launch-process-crash-recovery`. The worker stores a fixed synthetic
active daily definition with no automatic retry, and claims and launches its
09:00 slot through a no-op runtime. It then handles the cancellation as the
daemon's API does, through a runtime that confirms every stop and records it
in the worker's private home, so the parent can count the stops of a worker
it killed. The daemon's API fences a stop on the wall clock; the worker fences
it on the fixture's fixed clock, so its times agree with the scheduler's.
Vectors cannot choose a path, run, definition, reason, principal, adoption
key, clock or pause mechanics, and every expected outcome is fixed per
boundary.

The cancellation runs four transactions, with the stop between the second
and the third:

1. **Reservation:** the global adoption-key reservation, then the
   cancellation `requested` with a 30-second execution lease.
2. **Stop ownership:** the run's `cancellation` stop fence, then the
   cancellation `stopping`.
3. **Settlement:** once the runtime confirms the stop, the session
   `cancelled`, then the attempt, the run and the occurrence `cancelled`, then
   the fence released.
4. **Answer:** the cancellation `cancelled`, then the reservation released,
   then the response stored under the request's adoption key.

A fixture TEMP trigger pauses the worker after each of those twelve writes,
and the WAL hook after each of the four commits. The worker's runtime pauses
right after it records the stop. The control has no pause: the worker answers
`cancelled` and exits without the graceful shutdown's reconciliation or the
release of its scheduler authority.

While the worker is paused, the parent must find the write transaction open
exactly when the pause is inside one, and read what the last commit left. The
parent then kills and reaps the worker. The store's raw integrity check must
pass, and the store must still hold what the parent read.

The parent restarts in the daemon's startup order 10 seconds after the
request, inside the cancellation's lease, and the owner re-sends the request
10 seconds later. A pass after the lease reconciles the cancellation, and the
owner sends the request once more. One runtime confirms and counts every stop
and launch that the restart, the passes and the re-sent requests make. What
each case requires depends on how far the cancellation got:

- **Nothing committed:** the restart's containment marks the lost session
  `killed`, and its startup pass settles the attempt `failed`, which holds the
  run for an operator retry. The re-sent request stores an
  `ILLEGAL_TRANSITION` refusal, and later requests replay it.
- **Reserved, without the fence:** the same containment and hold. The request
  inside the lease is answered `CANCEL_PENDING` and writes nothing. The pass
  rejects the cancellation with `ILLEGAL_TRANSITION`, which later requests
  replay.
- **Fenced, stopped, or inside the settlement:** behind the fence, the
  restart's containment marks the session `killed`, and its startup pass
  leaves the attempt to the cancellation. The request inside the lease is
  answered `CANCEL_PENDING`. The pass answers the cancellation
  `recovery_required`, because the stop's outcome was never recorded. The
  same pass then settles the ambiguous run and occurrence `failed` on its
  ended session and releases the cancellation's fence. Later requests replay
  the stored `recovery_required` answer: it records that the stop was never
  confirmed, and the run's history shows how it settled.
- **Settled, without the answer:** the restart changes no row. The request
  inside the lease is answered `CANCEL_PENDING`. The pass answers `cancelled`,
  acknowledged and reconciled when the stop was settled, and later requests
  replay that answer.
- **Answered,** and the control: the restart changes no row, and every
  request replays `cancelled`.

The worker must have recorded exactly one stop once it reached the stop, and
none before it. The restart, the passes and the re-sent requests must issue
no stop and launch nothing. A pending or replayed answer must write nothing,
and a success must name the store's run, attempt and session. A request that
errors fails the case. The same request and a further pass must then change
no occurrence, run, attempt, event, session, reservation, adoption,
cancellation or stop-fence row, and issue no stop or launch.

Observations carry boundary, run, cancellation and answer classes, counts and
booleans. They never carry run, attempt, occurrence or session ids, adoption
keys, responses, paths, handshakes or output. This is `audit_only` evidence
for a daemon killed while it handles one owner cancellation of a running
scheduled run. It does not cover Runtime Authority runs, the flat
`occurrence.cancel.v1` and `attempt.cancel.v1` commands (see
`flat-cancellation-process-crash-recovery`), a cancellation racing a timeout,
a stop the runtime fails to confirm, a graceful daemon restart, a real runtime
process, power loss, load, complete profiles or release eligibility.

## Owner occurrence and attempt cancellation process-crash audit boundary

The `flat-cancellation-process-crash-recovery` suite kills a daemon worker
while it handles the owner's `occurrence.cancel.v1` or `attempt.cancel.v1`
request. The parent then restarts the daemon's scheduler and sends the
identical request again, with the same adoption key, as an owner who never saw
an answer would. Each of twelve cases starts the exact audited binary as a
hidden worker, with the same harness, daemon restart and stop-recording
runtime as `run-cancellation-process-crash-recovery`. The worker stores a
fixed synthetic active daily definition with no automatic retry, establishes
the owner principal, and claims its 09:00 slot. The daemon's API fences a
delegated stop on the wall clock; the worker fences it on the fixture's fixed
clock. Vectors cannot choose a path, occurrence, attempt, definition, reason,
adoption key, clock or pause mechanics, and every expected outcome is fixed
per boundary.

The two commands answer in one of two ways:

- **Nothing dispatched.** `occurrence.cancel.v1` for the claimed slot settles
  the occurrence `cancelled` and stores the answer under the owner's key in one
  transaction. A fixture TEMP trigger pauses the worker after the occurrence
  write and after the answer's write, and the WAL hook after the commit. The
  control has no pause.
- **A running attempt.** The worker launches the slot through a no-op runtime,
  and `attempt.cancel.v1` stores, as its answer, a delegation to a
  `run.cancel.v1` request under a key derived from the owner's. It then sends
  that run cancellation, which keeps its own reservation, stop fence and
  answer. The worker pauses after the delegation's write and after its
  commit, after each of the run cancellation's four commits, and right after
  its runtime records the stop. The control has no pause.

While the worker is paused, the parent must find the write transaction open
exactly when the pause is inside one, and read what the last commit left. The
parent then kills and reaps the worker, and the store's raw integrity check
must pass.

The parent restarts in the daemon's startup order 10 seconds after the
request, inside a run cancellation's lease, and the owner re-sends the request
10 seconds later. A pass after the lease reconciles any run cancellation, and
the owner sends the request once more. One runtime confirms and counts every
stop and launch that the restart, the passes and the re-sent requests make.
What each case requires depends on how far the request got:

- **A claimed slot, nothing committed:** the restart launches the slot. The
  re-sent request is then a new one: it delegates the launched run's stop,
  which settles the run `cancelled`, and later requests replay that answer.
  This is the only case with a launch and a later stop, one of each.
- **A claimed slot, withdrawn,** and its control: the restart changes no row
  and launches nothing, and every request replays the withdrawal.
- **A running attempt, nothing committed:** the restart's containment marks
  the session `killed` and its startup pass fails the attempt, holding the run
  for an operator retry. The re-sent request stores its own
  `ILLEGAL_TRANSITION` refusal, which later requests replay.
- **Delegated, not sent:** the same hold. The re-sent request replays the
  delegation and sends its run cancellation, which refuses the failed attempt
  and stores that refusal under the derived key. Later requests replay it.
- **The run cancellation reserved, fenced, stopped, settled or answered:**
  every re-sent request replays the delegation into the same run cancellation,
  which answers exactly as it does when the owner sends it directly: a
  reservation is rejected; an unconfirmed stop answers `recovery_required`
  and its run settles `failed`; a settled stop answers `cancelled`.

The worker must have recorded exactly one stop once it reached the stop, and
none before it. Except for the claimed slot nobody withdrew, the restart, the
passes and the re-sent requests must issue no stop and launch nothing. A
pending or replayed answer must write nothing. A success must name the store's
occurrence, or the store's run, attempt and session with the time its run
cancellation was requested. A request that errors fails the case. The same
request and a further pass must then change no occurrence, run, attempt,
event, session, reservation, adoption, cancellation or stop-fence row, and
issue no stop or launch.

Observations carry boundary, run, cancellation and answer classes, counts and
booleans. They never carry occurrence, run, attempt or session ids, adoption
keys, responses, paths, handshakes or output. This is `audit_only` evidence for
a daemon killed while it handles one owner occurrence or attempt cancellation.
It does not cover an eligible or planned slot, a waiting retry attempt, a
request first sent to a run already held for an operator retry, a kill inside
an `occurrence.cancel.v1` request that delegates a running attempt's stop
(tracked in #1486), the run-cancellation envelope, a delegation waiting on
runtime ownership, Runtime Authority runs, a stop the runtime fails to confirm,
a graceful daemon restart, a real runtime process, power loss, load, complete
profiles or release eligibility.
