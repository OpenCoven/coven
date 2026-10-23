---
title: "SQLite corruption conformance implementation plan"
summary: "Prove actual private store corruption refusal without hiding failed-operation mutations."
read_when:
  - Implementing Automations SQLite corruption fault coverage
source_adjacent_reason: "Defines executable fault and recovery boundaries for issue 1395 beside the native conformance implementation."
---

# SQLite Corruption Conformance Implementation Plan

> **For agentic workers:** Use executing-plans inline, task by task. Existing
> authorization covers implementation and verification; preserve #858 scope.

**Goal:** Add real SQLite header/B-tree corruption and a healthy control to the
independent portable audit, with cold initialization, no reset/adoption under
fault, exact durable-state recovery and replay evidence.

**Architecture:** Seed one private store through production owner-local commands.
Capture its complete relevant rows, page size and definition root page before
closing/checkpointing it. Copy its clean bytes to a fresh private probe path that
has never entered the initialization cache. Exercise the actual production cold
initializer with its existing phase observer, then the production adoption API
when open succeeds. This proves cold initialization, not an OS process restart.

Restore only the one deliberately damaged byte after closing every owned
connection. Preserve every other main-file byte and every existing WAL/sidecar.
Inspect the resulting database read-only *before* another writable initializer;
all pre-existing rows must match and the refused key must not exist. A whole-file
backup replacement or deleting WAL would hide failed-operation mutations and is
not allowed. Recovery is confined to the fixture, never operator guidance.

**Tech stack:** Rust/rusqlite/serde, current production store and adoption APIs,
existing Node audit runner/inventory. No production repair, new worker process,
new authority adapter, runtime launch or profile/eligibility claim.

## Task 1: Behavioral RED and bounded vector contract

Files:
- Create `conformance/automations/runner/command-storage-corruption-recovery.vectors.json`.
- Modify `crates/coven-cli/src/automations/conformance_target_tests.rs`.

- [x] Add exactly three closed cases: `sqlite_header`, `definition_btree`, `none`.
  Use paused synthetic definitions and valid adoption keys. Expected outcomes are
  respectively `not_a_database`, `database_corrupt`, `committed`; initial added
  rows are zero, zero, one; final added rows are one. Require baseline preservation,
  store-not-reset and replay preservation, all true. No caller path/offset/bytes.
- [x] Add the actual dispatch regression before registering the suite:

```rust
#[test]
fn storage_corruption_suite_executes_real_faults_and_control() {
    let vector: Value = serde_json::from_str(include_str!(
        "../../../../conformance/automations/runner/command-storage-corruption-recovery.vectors.json"
    )).unwrap();
    let result = evaluate(&request_for_profile(
        "scheduler_reliability", "command-storage-corruption-recovery", vector
    )).unwrap();
    assert_eq!(result.status, TargetSuiteStatus::Passed);
    assert!(result.evidence.is_some());
}
```

- [x] Run `cargo test --locked -p coven-cli --bin coven storage_corruption_suite_executes_real_faults_and_control`.
  Observe behavioral refusal of the unknown suite, not a compile error.

## Task 2: Actual fault, cold initializer and exact recovery

Files:
- Create `crates/coven-cli/src/automations/conformance_target/command_storage_corruption_recovery.rs`.
- Modify `crates/coven-cli/src/automations/conformance_target.rs`.

- [x] Use serde closed structs/enums, exact three-case membership, unique case IDs,
  valid AdoptionKey/RoutineDefinition, PAUSED-only status and reserved seed IDs.
  Reject missing controls, duplicate faults, unknown keys and weakened expectations.
- [x] Seed `seed.sqlite` via `store::open_store` and
  `execute_definition_command(..., CommandAuthority::OwnerLocal)`. Initialize the
  lazy owner principal before capturing the snapshot. Read all columns ordered
  by rowid from definitions, rich revisions, owner principal/grants, adoptions,
  reservations, events/heads, occurrences, runs and attempts. Keep SQLite value
  types/bytes private; do not project row contents into public observations.
- [x] Query `PRAGMA page_size` and `sqlite_schema.rootpage` for the definition table.
  Checkpoint TRUNCATE, require its successful empty result, close the seed, and
  copy only its main database to the previously unopened `probe.sqlite`.
  Bound main/WAL/sidecar reads; verify the SQLite header and checked arithmetic.
- [x] Compute the private mutation position as header byte zero or
  `(root_page - 1) * page_size` for a non-page-one populated definition root.
  Save the nonzero original byte and write zero. The healthy control writes none.
  Mutation and restoration must use an existing file without truncation:

```rust
fn replace_fixture_byte(path: &std::path::Path, offset: u64, byte: u8) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&[byte])?;
    file.sync_all()
}
```

- [x] Call `open_store_with_initialization_observer` on the new path and retain
  ConnectionOpened/CommitComplete booleans from its existing phase enum. Execute
  the create command only if the cold opener succeeds. Inspect the real chained
  rusqlite error class; header must be NotADatabase and B-tree DatabaseCorrupt.
  Record the actual fixed refusal stage (`cold_open` or `command`) without
  claiming one before execution proves it. No mocked error is acceptable.
- [x] Close the returned connection, inspect actual main/WAL/sidecar state and
  verify the damaged byte still exists. Restore only that byte. Read all relevant
  rows read-only before any writable recovery. They must equal the seed snapshot
  on fault cases; failed commands must leave no adopted key or added definition.
- [x] Reopen writable and commit the exact original key once (or retain the healthy
  initial result). Assert one added definition/adoption/event/head and no unrelated
  row loss/change; match canonical persisted definition, exact event and result.
  Replay after reopen and require unchanged complete rows/result/event references.
- [x] Emit only case ID, fixed fault/error/stage, counts and booleans. Never emit
  physical offsets, raw errors/SQL, file paths, database bytes or row values.
- [x] Register the constant/module, profile capability and dispatch observations.
  Run the RED test again and inspect actual observations; it must pass by real
  SQLite behavior. Do not relax error classes or recovery invariants to pass it.

## Task 3: Negative coverage, independent inventory and delivery

Files:
- Modify `crates/coven-cli/src/automations/conformance_target_tests.rs`.
- Modify `conformance/automations/runner/inventory.json`.
- Modify the existing runner/audit tests only where exact inventory is asserted.

- [x] Add table-driven mutations for schema/fault/definition/adoption/expected
  fields, unknown path/offset/bytes, missing/duplicate cases and unsupported profile.
  Regress count/evidence minimization and prove both fault cases ran, cold entry
  was observed, actual refusal stages were recorded, and control committed.
- [x] Add the new suite to the scheduler audit-only inventory and exact capability
  lists. Reconcile every hard-coded inventory count/membership assertion; preserve
  all existing suites and audit-only decision scope.
- [x] Run `cargo test --locked -p coven-cli --bin coven automations::conformance_target_tests`
  and `node --test conformance/automations/runner/conformance.test.mjs conformance/automations/runner/audit.test.mjs`.
- [ ] Run all repository local Rust/privacy/secret/policy and committed-range guards;
  obtain independent source review and fix required findings. Sign/DCO, check
  attribution, verify committed tree, run a clean source-bound audit, and publish.
- [ ] Inspect exact-source/native CI and protected queue results, fetch and verify
  landed content/signature/evidence, record #1395 acceptance and archive-backed
  owned retirement. Keep #858 open: this is bounded corruption coverage, not all
  faults, full profiles, 22 goldens, SLOs, canaries or authenticated eligibility.

## Delivery notes

- Both fault cases are refused by the cold initializer (`refusalStage:
  cold_open`): header damage as `NotADatabase` and definition B-tree damage as
  `DatabaseCorrupt`. The definition command is therefore unreachable for them,
  and only the control reaches it. The unit and integration tests pin these
  stages, so a change in SQLite or initializer behavior is reported.
- Independent review found that equality over empty tables proved nothing. Every
  compared table now holds pre-fault rows (`baselineTableRows`):
  - the seed definition is created ACTIVE, which records an owner grant;
  - a richly authored draft goes through `definition.create.v1`;
  - a settled occurrence/run/attempt chain and a pending runNow result
    reservation are inserted as fixture state.
- Vector cases may not use the seed's `adopt:corruption:` key prefix or
  `corruption-preexisting` ID prefix.
- A refusal must leave the corrupted main file byte-for-byte as written
  (`refusalLeftMainFileUnchanged`). This proves the refusal wrote nothing to
  the main file. Without that proof, a whole-file backup copy in place of the
  one-byte restore would be indistinguishable from the restore, because a
  refused cold open writes nothing.
- Mutation checks, each of which fails the suite:
  - skipping the one-byte restore;
  - copying the pristine backup over the probe after the refused open;
  - a refusal that changes one harmless header byte (the file-change counter).
    This one passes when the unchanged-main check is removed.
- `refusalStage` and `initialTransactionClosed` are `null` when there is
  nothing to observe: no refusal for the control, and no connection for a
  cold-open refusal. Sidecar observation includes `-journal`.

## Baseline and limits

Clean main `335ceb57220fef3f44d51d30d638770d54f3c30c`: all 115 existing
conformance-target tests passed. A private Python SQLite mechanics investigation
observed NOTADB for header and CORRUPT for the definition B-tree; this is not
Coven/rusqlite or native portable acceptance evidence. Rust execution must prove
all above invariants. Preserve independent Windows #1378 work and every #857 gate.
