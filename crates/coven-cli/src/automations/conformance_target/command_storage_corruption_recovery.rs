//! Actual private SQLite corruption refusal and one-byte fixture recovery.
//! No runtime launch, production repair or release-eligibility claim.
use std::{
    collections::BTreeSet,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{scheduler_conformance_tempdir, valid_case_id};
use crate::automations::command_adoption::{
    execute_definition_command, DefinitionCommand, DefinitionCommandOutcome,
    DefinitionCommandResponse,
};
use crate::automations::contract::types::AdoptionKey;
use crate::automations::definition::RoutineDefinition;
use crate::automations::owner_grants::CommandAuthority;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const FIRST_AT: &str = "2026-08-30T09:00:00.000Z";
const SETTLED_AT: &str = "2026-08-30T09:00:30.000Z";
// Seed definition IDs and adoption keys start with these. Vector cases may use
// neither, so a case can never collide with the baseline it must preserve.
const SEED_ID: &str = "corruption-preexisting";
const SEED_KEY_PREFIX: &str = "adopt:corruption:";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VectorSet {
    schema_version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    fault: Fault,
    adoption_key: String,
    definition: Value,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    baseline_preserved: bool,
    initial_rows: u64,
    final_rows: u64,
    replay_preserved: bool,
    store_not_reset: bool,
    refusal_stage: Option<RefusalStage>,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum RefusalStage {
    ColdOpen,
    Command,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    NotADatabase,
    DatabaseCorrupt,
    Committed,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Fault {
    SqliteHeader,
    DefinitionBtree,
    AdoptionBtree,
    None,
}

pub(super) fn evaluate(vector: &Value) -> Result<(bool, Vec<Value>), &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != "coven.automations.command-storage-corruption-recovery-vectors.v1"
        || vectors.cases.len() != 4
    {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut faults = BTreeSet::new();
    for case in &vectors.cases {
        RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
        if !valid_case_id(&case.case_id)
            || !ids.insert(&case.case_id)
            || !faults.insert(&case.fault)
            || AdoptionKey::new(case.adoption_key.clone()).is_err()
            || case.adoption_key.starts_with(SEED_KEY_PREFIX)
            || case.definition["id"]
                .as_str()
                .is_none_or(|id| id.starts_with(SEED_ID))
            || case.definition["status"] != "PAUSED"
            || !case.expected.baseline_preserved
            || case.expected.final_rows != 1
            || !case.expected.replay_preserved
            || !case.expected.store_not_reset
            || case.expected.initial_rows != u64::from(case.fault == Fault::None)
            || case.expected.initial_outcome != expected_outcome(case.fault)
            || case.expected.refusal_stage != expected_stage(case.fault)
        {
            return Err(INVALID);
        }
    }
    let mut passed = true;
    let mut observations = Vec::new();
    for case in &vectors.cases {
        let (matches, observation) = case_matches(case)?;
        passed &= matches;
        observations.push(observation);
    }
    Ok((passed, observations))
}

fn execute(conn: &Connection, case: &Case, at: &str) -> anyhow::Result<DefinitionCommandResponse> {
    execute_definition_command(
        conn,
        &case.adoption_key,
        DefinitionCommand::Create {
            definition: case.definition.clone(),
        },
        at,
        CommandAuthority::OwnerLocal,
    )
}

type Snapshot = Vec<Vec<Vec<rusqlite::types::Value>>>;
fn snapshot(conn: &Connection) -> Result<Snapshot, &'static str> {
    let mut tables = Vec::new();
    for table in [
        "automation_definitions",
        "automation_rich_definition_revisions",
        "automation_owner_principal",
        "automation_owner_grants",
        "automation_command_adoptions",
        "automation_command_reservations",
        "automation_events",
        "automation_event_stream_heads",
        "automation_occurrences",
        "automation_runs",
        "automation_attempts",
    ] {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .map_err(|_| FAILED)?;
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get(index))
                    .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
            })
            .map_err(|_| FAILED)?;
        tables.push(
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| FAILED)?,
        );
    }
    Ok(tables)
}

const MAX_STORE_BYTES: u64 = 16 * 1024 * 1024;

fn expected_outcome(fault: Fault) -> InitialOutcome {
    match fault {
        Fault::SqliteHeader => InitialOutcome::NotADatabase,
        Fault::DefinitionBtree | Fault::AdoptionBtree => InitialOutcome::DatabaseCorrupt,
        Fault::None => InitialOutcome::Committed,
    }
}
/// The stage each fault must be refused at, which every vector states. The cold
/// initializer reads the header and the definition B-tree, so it refuses those.
/// It never reads the adoption table's root page, so only the command can reach
/// that fault. If a future initializer refused it at cold open, the case would
/// stop exercising the command path, so it fails rather than silently losing
/// that coverage; the vectors would then need to change visibly.
fn expected_stage(fault: Fault) -> Option<RefusalStage> {
    match fault {
        Fault::SqliteHeader | Fault::DefinitionBtree => Some(RefusalStage::ColdOpen),
        Fault::AdoptionBtree => Some(RefusalStage::Command),
        Fault::None => None,
    }
}
/// The table whose B-tree root page a fault damages, if any.
fn fault_table(fault: Fault) -> Option<&'static str> {
    match fault {
        Fault::DefinitionBtree => Some("automation_definitions"),
        Fault::AdoptionBtree => Some("automation_command_adoptions"),
        Fault::SqliteHeader | Fault::None => None,
    }
}
fn error_outcome(error: &anyhow::Error) -> Result<InitialOutcome, &'static str> {
    match error
        .downcast_ref::<rusqlite::Error>()
        .and_then(rusqlite::Error::sqlite_error_code)
    {
        Some(ErrorCode::NotADatabase) => Ok(InitialOutcome::NotADatabase),
        Some(ErrorCode::DatabaseCorrupt) => Ok(InitialOutcome::DatabaseCorrupt),
        _ => Err(FAILED),
    }
}
fn bounded_bytes(path: &Path) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| FAILED)?
        .take(MAX_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FAILED)?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(FAILED);
    }
    Ok(bytes)
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}
fn sidecar_bytes(path: &Path) -> Result<Vec<Option<Vec<u8>>>, &'static str> {
    ["-wal", "-shm", "-journal"]
        .into_iter()
        .map(|suffix| {
            let path = sidecar(path, suffix);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => bounded_bytes(&path).map(Some),
                Ok(_) => Err(FAILED),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(FAILED),
            }
        })
        .collect()
}
fn replace_fixture_byte(path: &Path, offset: u64, byte: u8) -> Result<(), &'static str> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| FAILED)?;
    file.seek(SeekFrom::Start(offset)).map_err(|_| FAILED)?;
    file.write_all(&[byte]).map_err(|_| FAILED)?;
    file.sync_all().map_err(|_| FAILED)
}
/// A richly authored draft committed through `definition.create.v1`, so the
/// rich revision table holds a production-recorded row before the fault.
fn seed_rich_definition(conn: &Connection) -> Result<(), &'static str> {
    use crate::automations::contract::canonical_json::{
        canonicalize_without_integrity, sha256_hex,
    };
    let mut definition = json!({
        "schemaVersion": "coven.automations.v1", "automationId": format!("{SEED_ID}-rich"),
        "revision": 1, "lifecycleState": "draft",
        "display": {"name": "Corruption baseline", "description": "Synthetic rich baseline.", "tags": []},
        "trigger": {"variant": "schedule", "version": 1,
            "schedule": {"rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc"}},
        "conditions": [],
        "action": {"variant": "familiarInvocation", "version": 1,
            "prompt": "Synthetic rich baseline.", "cwd": "~/projects/notes"},
        "binding": {"familiarBindingPolicy": "exact", "familiarId": "charm",
            "authority": {"approvalPolicyRef": "policy://authority/familiars/charm"}},
        "runtimeRequirements": {"runtimeId": "coven-code", "capabilities": ["sessions.launch"]},
        "policies": {"timeout": {"perRunMinutes": 30},
            "retry": {"maxAttempts": 3, "backoffPolicy": "exponential",
                "retryableClasses": ["transient_dispatch"]},
            "concurrency": {"overlap": "forbid"}, "misfire": {"disposition": "latest"},
            "retention": {"occurrenceHistory": {"classification": "standard"},
                "runLogs": {"classification": "standard"},
                "receipts": {"classification": "standard"}}},
        "provenance": {"createdBy": {"principalId": "principal:tim"}, "createdAt": FIRST_AT},
        "extensions": {}
    });
    let digest = sha256_hex(&canonicalize_without_integrity(&definition).map_err(|_| FAILED)?);
    definition["integrity"] =
        json!({"algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": digest});
    let created = execute_definition_command(
        conn,
        &format!("{SEED_KEY_PREFIX}rich-baseline"),
        DefinitionCommand::RichCreate { definition },
        FIRST_AT,
        CommandAuthority::OwnerLocal,
    )
    .map_err(|_| FAILED)?;
    if created.outcome != DefinitionCommandOutcome::Committed {
        return Err(FAILED);
    }
    Ok(())
}

/// Settled work and a pending runNow result reservation for the seed, so no
/// compared table is empty and equality proves preservation in every one. The
/// states are terminal and nothing here schedules or dispatches, so these rows
/// stay inert unless a refused operation or recovery step changes them.
fn seed_settled_work(conn: &Connection) -> Result<(), &'static str> {
    let occurrence = format!("{SEED_ID}-occurrence");
    let run = format!("{SEED_ID}-run");
    conn.execute(
        "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'failed', 1, ?3, ?4)",
        rusqlite::params![occurrence, SEED_ID, FIRST_AT, SETTLED_AT],
    )
    .map_err(|_| FAILED)?;
    conn.execute(
        "INSERT INTO automation_runs
            (id, automation_id, occurrence_id, runtime, status, started_at, finished_at)
         VALUES (?1, ?2, ?3, 'coven-code', 'failed', ?4, ?5)",
        rusqlite::params![run, SEED_ID, occurrence, FIRST_AT, SETTLED_AT],
    )
    .map_err(|_| FAILED)?;
    conn.execute(
        "INSERT INTO automation_attempts (
            id, run_id, occurrence_id, attempt_number, adoption_key,
            occurrence_fence_generation, dispatch_generation, state,
            retry_classification, not_before, opened_at, settled_at
         ) VALUES (?1, ?2, ?3, 1, ?4, 1, 1, 'failed', 'initial', ?5, ?5, ?6)",
        rusqlite::params![
            format!("{SEED_ID}-attempt"),
            run,
            occurrence,
            format!("{SEED_KEY_PREFIX}attempt-1"),
            FIRST_AT,
            SETTLED_AT
        ],
    )
    .map_err(|_| FAILED)?;
    conn.execute(
        "INSERT INTO automation_command_reservations
            (adoption_key, request_digest, command, reserved_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            format!("{SEED_KEY_PREFIX}pending-result"),
            "0".repeat(64),
            crate::automations::run_now::RESULT_COMMAND,
            FIRST_AT
        ],
    )
    .map_err(|_| FAILED)?;
    Ok(())
}

fn preserves_seed(before: &Snapshot, after: &Snapshot, added: usize) -> bool {
    before
        .iter()
        .zip(after)
        .enumerate()
        .all(|(index, (old, new))| {
            old.iter().all(|row| new.contains(row))
                && if [0, 4, 6, 7].contains(&index) {
                    new.len() == old.len() + added
                } else {
                    new == old
                }
        })
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let home = scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(home.path()).map_err(|_| FAILED)?;
    // The seed and probe have distinct paths. The probe never enters the
    // initialization cache before the actual observed cold initializer below.
    let seed_path = home.path().join("seed.sqlite");
    let path = home.path().join("probe.sqlite");
    let conn = crate::store::open_store(&seed_path).map_err(|_| FAILED)?;
    let mut seed = case.definition.clone();
    seed["id"] = json!(SEED_ID);
    // An owner-local create that activates records the owner grant, so the
    // grants table is populated by the production path rather than by SQL.
    seed["status"] = json!("ACTIVE");
    let created = execute_definition_command(
        &conn,
        &format!("{SEED_KEY_PREFIX}baseline"),
        DefinitionCommand::Create {
            definition: seed.clone(),
        },
        FIRST_AT,
        CommandAuthority::OwnerLocal,
    )
    .map_err(|_| FAILED)?;
    if created.outcome != DefinitionCommandOutcome::Committed {
        return Err(FAILED);
    }
    let stored = crate::automations::store::get_definition(&conn, SEED_ID)
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let persisted: RoutineDefinition =
        serde_json::from_str(&stored.definition_json).map_err(|_| FAILED)?;
    if persisted != RoutineDefinition::from_json(&seed).map_err(|_| INVALID)? {
        return Err(FAILED);
    }
    crate::automations::owner_grants::owner_principal_id(&conn, FIRST_AT).map_err(|_| FAILED)?;
    seed_rich_definition(&conn)?;
    seed_settled_work(&conn)?;
    let baseline = snapshot(&conn)?;
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|_| FAILED)?;
    let page_size = u64::try_from(page_size).map_err(|_| FAILED)?;
    if !page_size.is_power_of_two() || !(512..=65536).contains(&page_size) {
        return Err(FAILED);
    }
    // A B-tree fault zeroes the page-type byte that starts its table's root page.
    let root_offset = fault_table(case.fault)
        .map(|table| {
            let root_page: i64 = conn
                .query_row(
                    "SELECT rootpage FROM sqlite_schema WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .map_err(|_| FAILED)?;
            u64::try_from(root_page)
                .ok()
                .filter(|page| *page >= 2)
                .and_then(|page| (page - 1).checked_mul(page_size))
                .ok_or(FAILED)
        })
        .transpose()?;
    let checkpoint: (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|_| FAILED)?;
    if checkpoint != (0, 0, 0) {
        return Err(FAILED);
    }
    drop(conn);
    let pristine = bounded_bytes(&seed_path)?;
    if !pristine.starts_with(b"SQLite format 3\0") {
        return Err(FAILED);
    }
    let position = match case.fault {
        Fault::SqliteHeader => Some(0),
        Fault::DefinitionBtree | Fault::AdoptionBtree => Some(root_offset.ok_or(FAILED)?),
        Fault::None => None,
    };
    let original = position
        .map(|p| {
            pristine
                .get(usize::try_from(p).map_err(|_| FAILED)?)
                .copied()
                .filter(|b| *b != 0)
                .ok_or(FAILED)
        })
        .transpose()?;
    std::fs::copy(&seed_path, &path).map_err(|_| FAILED)?;
    if sidecar_bytes(&path)?.iter().any(Option::is_some) {
        return Err(FAILED);
    }
    if let Some(position) = position {
        replace_fixture_byte(&path, position, 0)?;
    }

    let mut cold_entered = false;
    let mut cold_completed = false;
    let opened = crate::store::open_store_with_initialization_observer(&path, |phase| {
        cold_entered |= matches!(
            phase,
            crate::store::StoreInitializationPhase::ConnectionOpened
        );
        cold_completed |= matches!(
            phase,
            crate::store::StoreInitializationPhase::CommitComplete
        );
    });
    let mut committed = None;
    // These stay `None` when the cold open refuses: no connection or command
    // existed, so there is no transaction to observe and the stage is the open.
    let mut initial_transaction_closed = None;
    // Only a command refusal sets this. SQLite's total-change counter keeps
    // statements that completed inside a transaction later rolled back, so a
    // rise shows the command had already written before it was refused.
    let mut refused_after_earlier_writes = None;
    let (initial_outcome, stage) = match opened {
        Err(error) => (error_outcome(&error)?, Some(RefusalStage::ColdOpen)),
        Ok(conn) => {
            let changes_before = conn.total_changes();
            let result = execute(&conn, case, FIRST_AT);
            initial_transaction_closed = Some(conn.is_autocommit());
            let (outcome, stage) = match result {
                Ok(response) if response.outcome == DefinitionCommandOutcome::Committed => {
                    committed = Some(response);
                    (InitialOutcome::Committed, None)
                }
                Err(error) => {
                    refused_after_earlier_writes = Some(conn.total_changes() > changes_before);
                    (error_outcome(&error)?, Some(RefusalStage::Command))
                }
                _ => return Err(FAILED),
            };
            drop(conn);
            (outcome, stage)
        }
    };
    let after_initial = bounded_bytes(&path)?;
    // A refusal must leave the corrupted main file exactly as written. That
    // proves it wrote nothing there, so restoring one byte below cannot hide a
    // write the way a whole-file backup copy could.
    let refusal_left_main_unchanged = match (position, original) {
        (Some(position), Some(_)) => {
            let mut corrupted = pristine.clone();
            *corrupted
                .get_mut(usize::try_from(position).map_err(|_| FAILED)?)
                .ok_or(FAILED)? = 0;
            Some(after_initial == corrupted)
        }
        _ => None,
    };
    let sidecars_before_restore = sidecar_bytes(&path)?;
    let mut expected_restored = after_initial.clone();
    if let (Some(position), Some(original)) = (position, original) {
        let position_usize = usize::try_from(position).map_err(|_| FAILED)?;
        // If the actual failure reset/replaced/repaired this byte, refuse rather
        // than masking it by installing a clean database backup.
        if after_initial.get(position_usize) != Some(&0) {
            return Err(FAILED);
        }
        expected_restored[position_usize] = original;
        replace_fixture_byte(&path, position, original)?;
    }
    let restoration_exact = bounded_bytes(&path)? == expected_restored
        && sidecar_bytes(&path)? == sidecars_before_restore;
    // Do not open writable or initialize again before inspecting retained state.
    // This read-only view includes retained committed WAL; none is deleted.
    let inspected = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| FAILED)?;
    let initial_snapshot = snapshot(&inspected)?;
    drop(inspected);
    let faulted = case.fault != Fault::None;
    let initial_rows = initial_snapshot[0]
        .len()
        .checked_sub(baseline[0].len())
        .ok_or(FAILED)?;
    let baseline_preserved = preserves_seed(&baseline, &initial_snapshot, initial_rows)
        && (!faulted || initial_snapshot == baseline);
    let store_not_reset =
        baseline_preserved && restoration_exact && refusal_left_main_unchanged != Some(false);
    let mut passed = cold_entered
        && stage == case.expected.refusal_stage
        && refused_after_earlier_writes == (case.fault == Fault::AdoptionBtree).then_some(true)
        && initial_transaction_closed != Some(false)
        && initial_outcome == case.expected.initial_outcome
        && initial_rows as u64 == case.expected.initial_rows
        && baseline_preserved == case.expected.baseline_preserved
        && store_not_reset == case.expected.store_not_reset;
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    passed &= snapshot(&conn)? == initial_snapshot;
    let committed = match committed {
        Some(response) if !faulted => response,
        None if faulted => execute(&conn, case, FIRST_AT).map_err(|_| FAILED)?,
        _ => return Err(FAILED),
    };
    passed &= committed.outcome == DefinitionCommandOutcome::Committed
        && committed.result.is_some()
        && committed.event_ref.is_some()
        && committed.error.is_none();
    let committed_snapshot = snapshot(&conn)?;
    passed &= preserves_seed(
        &baseline,
        &committed_snapshot,
        case.expected.final_rows as usize,
    );
    let event =
        serde_json::to_value(committed.event_ref.as_ref().ok_or(FAILED)?).map_err(|_| FAILED)?;
    let (kind, stream) = event["stream"]
        .as_str()
        .and_then(|v| v.split_once('/'))
        .ok_or(FAILED)?;
    let sequence = event["sequence"].as_i64().ok_or(FAILED)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM automation_events WHERE stream_kind=?1 AND stream_id=?2 AND sequence=?3", rusqlite::params![kind,stream,sequence], |row| row.get(0)).map_err(|_| FAILED)?;
    passed &= kind == "automation" && Some(stream) == case.definition["id"].as_str() && count == 1;
    drop(conn);
    let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let replay = execute(&reopened, case, "2026-08-30T09:01:00.000Z").map_err(|_| FAILED)?;
    let replay_preserved = replay.outcome == DefinitionCommandOutcome::Replayed
        && replay.error.is_none()
        && replay.revision == committed.revision
        && replay.result == committed.result
        && replay.event_ref == committed.event_ref
        && replay.replay_first_committed_at.as_deref() == Some(FIRST_AT)
        && snapshot(&reopened)? == committed_snapshot;
    let definition = RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
    let stored = crate::automations::store::get_definition(&reopened, &definition.id)
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let persisted: RoutineDefinition =
        serde_json::from_str(&stored.definition_json).map_err(|_| FAILED)?;
    passed &= persisted == definition && replay_preserved == case.expected.replay_preserved;
    Ok((
        passed,
        json!({"caseId":case.case_id,"faultSource":case.fault,
        "initialOutcome":initial_outcome,"refusalStage":stage,
        "sqliteErrorCode":match initial_outcome {InitialOutcome::NotADatabase=>Some(26),InitialOutcome::DatabaseCorrupt=>Some(11),InitialOutcome::Committed=>None},
        "coldInitializationEntered":cold_entered,"coldInitializationCompleted":cold_completed,
        "initialTransactionClosed":initial_transaction_closed,
        "refusedAfterEarlierWrites":refused_after_earlier_writes,"baselinePreserved":baseline_preserved,
        "storeNotReset":store_not_reset,"oneByteRestorePreservedOtherBytes":restoration_exact,
        "refusalLeftMainFileUnchanged":refusal_left_main_unchanged,
        "walObservedAfterInitial":sidecars_before_restore[0].is_some(),
        "shmObservedAfterInitial":sidecars_before_restore[1].is_some(),
        "journalObservedAfterInitial":sidecars_before_restore[2].is_some(),
        "baselineTableRows":baseline.iter().map(Vec::len).collect::<Vec<_>>(),
        "initialAddedDefinitions":initial_rows,
        "finalAddedDefinitions":committed_snapshot[0].len().checked_sub(baseline[0].len()),
        "replayPreserved":replay_preserved}),
    ))
}
