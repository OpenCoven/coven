use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceView {
    Due,
    Eligible,
    Claimed,
    Running,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceRecord {
    pub id: String,
    pub automation_id: String,
    pub automation_revision: u64,
    pub definition_digest: Option<String>,
    pub scheduled_for: String,
    pub kind: String,
    pub state: String,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub scheduler_generation: Option<i64>,
    pub fence_generation: i64,
    pub failure_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInspection {
    pub id: String,
    pub automation_id: String,
    pub automation_revision: u64,
    pub definition_digest: Option<String>,
    pub occurrence_id: Option<String>,
    pub authority_profile: Option<String>,
    pub receipt_id: Option<String>,
    pub session_id: Option<String>,
    pub familiar_id: Option<String>,
    pub runtime: Option<String>,
    pub status: String,
    pub exit_code: Option<i64>,
    pub log_json: Option<String>,
    pub output_commit: Option<String>,
    pub started_at: String,
    pub timeout_at: Option<String>,
    pub finished_at: Option<String>,
    pub attempts: Vec<AttemptInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptInspection {
    pub id: String,
    pub run_id: String,
    pub occurrence_id: String,
    pub attempt_number: i64,
    pub adoption_key: String,
    pub occurrence_fence_generation: i64,
    pub dispatch_generation: i64,
    pub state: String,
    pub failure_class: Option<String>,
    pub prior_attempt_number: Option<i64>,
    pub prior_disposition: Option<String>,
    pub retry_classification: String,
    pub not_before: String,
    pub session_id: Option<String>,
    pub state_reason: Option<String>,
    pub opened_at: String,
    pub settled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceInspection {
    pub occurrence: OccurrenceRecord,
    pub runs: Vec<RunInspection>,
    pub runs_truncated: bool,
}

const OCCURRENCE_RUN_INSPECTION_LIMIT: usize = 20;

/// One bounded operator view, optionally restricted to one automation. The
/// restriction is part of each view's query, so `limit` bounds that
/// automation's rows rather than a global page filtered afterwards.
pub fn list_occurrences(
    conn: &Connection,
    view: OccurrenceView,
    now: DateTime<Utc>,
    limit: usize,
    automation_id: Option<&str>,
) -> Result<Vec<OccurrenceRecord>> {
    let transaction = conn
        .unchecked_transaction()
        .context("failed to begin automation occurrence inspection snapshot")?;
    let bounded = limit.clamp(1, 100);
    let records = match view {
        OccurrenceView::Due => list_by_query(
            &transaction,
            "state = 'planned' AND scheduled_for <= ?1",
            &now.to_rfc3339_opts(SecondsFormat::Millis, true),
            bounded,
            automation_id,
        )?,
        OccurrenceView::Eligible => {
            let eligible = match automation_id {
                Some(automation_id) => super::occurrences::eligible_occurrences_for_automation(
                    &transaction,
                    now,
                    bounded,
                    automation_id,
                )?,
                None => super::occurrences::eligible_occurrences(&transaction, now, bounded)?,
            };
            let mut records = Vec::with_capacity(eligible.len().min(bounded));
            for eligible in eligible {
                let record = occurrence_by_id(&transaction, &eligible.id)?.with_context(|| {
                    format!("eligible occurrence `{}` disappeared", eligible.id)
                })?;
                records.push(record);
            }
            records
        }
        OccurrenceView::Claimed => list_by_state(&transaction, "claimed", bounded, automation_id)?,
        OccurrenceView::Running => list_by_state(&transaction, "running", bounded, automation_id)?,
        OccurrenceView::RecoveryRequired => {
            list_by_state(&transaction, "recovery_required", bounded, automation_id)?
        }
    };
    transaction
        .commit()
        .context("failed to commit automation occurrence inspection snapshot")?;
    Ok(records)
}

/// A keyset position in one automation's occurrence history: the
/// `(scheduled_for, id)` of the last row a previous page returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceHistoryPosition {
    pub scheduled_for: String,
    pub id: String,
}

pub struct OccurrenceHistoryPage {
    pub occurrences: Vec<OccurrenceRecord>,
    /// The position after the last returned row, when more rows exist.
    pub next: Option<OccurrenceHistoryPosition>,
}

/// One automation's occurrences in every state, newest first, from one
/// snapshot. The page starts strictly after `after`, so rows inserted above
/// the cursor while a caller pages never shift a later page. Diagnostic only.
pub fn occurrence_history(
    conn: &Connection,
    automation_id: &str,
    limit: usize,
    after: Option<&OccurrenceHistoryPosition>,
) -> Result<OccurrenceHistoryPage> {
    let transaction = conn
        .unchecked_transaction()
        .context("failed to begin automation occurrence history snapshot")?;
    let bounded = limit.clamp(1, 100);
    // The planning fence UNIQUE(automation_id, scheduled_for) means one
    // automation never has two rows at one time; `id` only keeps the order
    // total if that fence is ever relaxed.
    let mut statement = transaction
        .prepare(
            "SELECT id, automation_id, automation_revision, definition_digest, scheduled_for,
                    kind, state, lease_owner, lease_expires_at, scheduler_generation, attempt,
                    failure_reason, created_at, updated_at
             FROM automation_occurrences
             WHERE automation_id = ?1
               AND (?2 IS NULL OR scheduled_for < ?2 OR (scheduled_for = ?2 AND id < ?3))
             ORDER BY scheduled_for DESC, id DESC
             LIMIT ?4",
        )
        .context("failed to prepare automation occurrence history query")?;
    let rows = statement
        .query_map(
            params![
                automation_id,
                after.map(|position| position.scheduled_for.as_str()),
                after.map(|position| position.id.as_str()),
                i64::try_from(bounded + 1)
                    .context("occurrence history limit exceeds SQLite range")?
            ],
            occurrence_record_from_row,
        )
        .context("failed to read automation occurrence history")?;
    let mut occurrences = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read automation occurrence history")?;
    drop(statement);
    transaction
        .commit()
        .context("failed to commit automation occurrence history snapshot")?;
    let next = if occurrences.len() > bounded {
        occurrences.truncate(bounded);
        occurrences.last().map(|last| OccurrenceHistoryPosition {
            scheduled_for: last.scheduled_for.clone(),
            id: last.id.clone(),
        })
    } else {
        None
    };
    Ok(OccurrenceHistoryPage { occurrences, next })
}

pub fn inspect_occurrence(conn: &Connection, id: &str) -> Result<Option<OccurrenceInspection>> {
    let transaction = conn
        .unchecked_transaction()
        .context("failed to begin automation occurrence detail snapshot")?;
    let Some(occurrence) = occurrence_by_id(&transaction, id)? else {
        transaction
            .commit()
            .context("failed to commit empty automation occurrence detail snapshot")?;
        return Ok(None);
    };
    let mut runs = list_runs_for_occurrence(&transaction, id)?;
    let runs_truncated = runs.len() > OCCURRENCE_RUN_INSPECTION_LIMIT;
    runs.truncate(OCCURRENCE_RUN_INSPECTION_LIMIT);
    for run in &mut runs {
        run.attempts = list_attempts_for_run(&transaction, &run.id)?;
    }
    transaction
        .commit()
        .context("failed to commit automation occurrence detail snapshot")?;
    Ok(Some(OccurrenceInspection {
        occurrence,
        runs,
        runs_truncated,
    }))
}

fn list_by_state(
    conn: &Connection,
    state: &str,
    limit: usize,
    automation_id: Option<&str>,
) -> Result<Vec<OccurrenceRecord>> {
    list_by_query(conn, "state = ?1", state, limit, automation_id)
}

fn list_by_query(
    conn: &Connection,
    predicate: &str,
    parameter: &str,
    limit: usize,
    automation_id: Option<&str>,
) -> Result<Vec<OccurrenceRecord>> {
    let query = format!(
        "SELECT id, automation_id, automation_revision, definition_digest, scheduled_for,
                kind, state, lease_owner, lease_expires_at, scheduler_generation, attempt,
                failure_reason, created_at, updated_at
         FROM automation_occurrences
         WHERE ({predicate})
           AND (?3 IS NULL OR automation_id = ?3)
         ORDER BY scheduled_for ASC, id ASC
         LIMIT ?2"
    );
    let mut statement = conn
        .prepare(&query)
        .context("failed to prepare automation occurrence inspection query")?;
    let rows = statement
        .query_map(
            params![
                parameter,
                i64::try_from(limit).context("occurrence inspection limit exceeds SQLite range")?,
                automation_id
            ],
            occurrence_record_from_row,
        )
        .context("failed to inspect automation occurrences")?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read automation occurrence inspection")
}

fn occurrence_by_id(conn: &Connection, id: &str) -> Result<Option<OccurrenceRecord>> {
    conn.query_row(
        "SELECT id, automation_id, automation_revision, definition_digest, scheduled_for,
                kind, state, lease_owner, lease_expires_at, scheduler_generation, attempt,
                failure_reason, created_at, updated_at
         FROM automation_occurrences
         WHERE id = ?1",
        [id],
        occurrence_record_from_row,
    )
    .optional()
    .context("failed to inspect automation occurrence")
}

fn occurrence_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OccurrenceRecord> {
    Ok(OccurrenceRecord {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        automation_revision: sqlite_u64(row, 2)?,
        definition_digest: row.get(3)?,
        scheduled_for: row.get(4)?,
        kind: row.get(5)?,
        state: row.get(6)?,
        lease_owner: row.get(7)?,
        lease_expires_at: row.get(8)?,
        scheduler_generation: row.get(9)?,
        fence_generation: row.get(10)?,
        failure_reason: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn list_runs_for_occurrence(conn: &Connection, occurrence_id: &str) -> Result<Vec<RunInspection>> {
    let mut statement = conn
        .prepare(
            "SELECT id, automation_id, automation_revision, definition_digest, occurrence_id,
                    authority_profile, receipt_id, session_id, familiar_id, runtime, status,
                    exit_code, log_json, output_commit, started_at, timeout_at, finished_at
             FROM automation_runs
             WHERE occurrence_id = ?1
             ORDER BY started_at ASC, id ASC
             LIMIT ?2",
        )
        .context("failed to prepare automation occurrence run inspection")?;
    let rows = statement
        .query_map(
            params![
                occurrence_id,
                i64::try_from(OCCURRENCE_RUN_INSPECTION_LIMIT + 1)
                    .context("occurrence run inspection limit exceeds SQLite range")?
            ],
            run_record_from_row,
        )
        .context("failed to inspect automation occurrence runs")?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read automation occurrence run inspection")
}

const RUN_COLUMNS: &str =
    "id, automation_id, automation_revision, definition_digest, occurrence_id,
     authority_profile, receipt_id, session_id, familiar_id, runtime, status,
     exit_code, log_json, output_commit, started_at, timeout_at, finished_at";

/// One run and its attempts, read from a single snapshot. Diagnostic only:
/// it neither renews leases nor mutates lifecycle rows.
pub fn inspect_run(conn: &Connection, id: &str) -> Result<Option<RunInspection>> {
    let transaction = conn
        .unchecked_transaction()
        .context("failed to begin automation run detail snapshot")?;
    let run = transaction
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM automation_runs WHERE id = ?1"),
            [id],
            run_record_from_row,
        )
        .optional()
        .context("failed to inspect automation run")?;
    let run = match run {
        Some(mut run) => {
            run.attempts = list_attempts_for_run(&transaction, &run.id)?;
            Some(run)
        }
        None => None,
    };
    transaction
        .commit()
        .context("failed to commit automation run detail snapshot")?;
    Ok(run)
}

fn run_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunInspection> {
    Ok(RunInspection {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        automation_revision: sqlite_u64(row, 2)?,
        definition_digest: row.get(3)?,
        occurrence_id: row.get(4)?,
        authority_profile: row.get(5)?,
        receipt_id: row.get(6)?,
        session_id: row.get(7)?,
        familiar_id: row.get(8)?,
        runtime: row.get(9)?,
        status: row.get(10)?,
        exit_code: row.get(11)?,
        log_json: row.get(12)?,
        output_commit: row.get(13)?,
        started_at: row.get(14)?,
        timeout_at: row.get(15)?,
        finished_at: row.get(16)?,
        attempts: Vec::new(),
    })
}

fn list_attempts_for_run(conn: &Connection, run_id: &str) -> Result<Vec<AttemptInspection>> {
    let mut statement = conn
        .prepare(
            "SELECT id, run_id, occurrence_id, attempt_number, adoption_key,
                    occurrence_fence_generation, dispatch_generation, state, failure_class,
                    prior_attempt_number, prior_disposition, retry_classification, not_before,
                    session_id, state_reason, opened_at, settled_at
             FROM automation_attempts
             WHERE run_id = ?1
             ORDER BY attempt_number ASC",
        )
        .context("failed to prepare automation attempt inspection")?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok(AttemptInspection {
                id: row.get(0)?,
                run_id: row.get(1)?,
                occurrence_id: row.get(2)?,
                attempt_number: row.get(3)?,
                adoption_key: row.get(4)?,
                occurrence_fence_generation: row.get(5)?,
                dispatch_generation: row.get(6)?,
                state: row.get(7)?,
                failure_class: row.get(8)?,
                prior_attempt_number: row.get(9)?,
                prior_disposition: row.get(10)?,
                retry_classification: row.get(11)?,
                not_before: row.get(12)?,
                session_id: row.get(13)?,
                state_reason: row.get(14)?,
                opened_at: row.get(15)?,
                settled_at: row.get(16)?,
            })
        })
        .context("failed to inspect automation attempts")?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read automation attempt inspection")
}

fn sqlite_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(index)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
