//! What decides whether one routine runs next (#1518).
//!
//! Read-only. Each condition is one of the scheduler's own predicates: the
//! stored `status` planning compares, its quarantine predicate,
//! `occurrences::stored_definition` and `occurrences::verified_revision`.
//! Each work item mirrors the claim gate's own clauses, and a recovery item's
//! next step mirrors recovery's own refusals. Beside the routine's
//! id, revision and lifecycle state, only codes, canonical timestamps,
//! bounded ids and failure-class tokens leave; prompts, quarantine reasons,
//! validation text, lease owners and authority profiles do not.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::definition::{RoutineDefinition, RoutineStatus};
use super::occurrences::{stored_definition, verified_revision, ACTIVE_STATUS};
use super::store::RoutineRecord;

pub(crate) const ACTION: &str = "coven.automations.explain.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Explanation {
    pub automation_id: String,
    pub revision: u64,
    pub lifecycle_state: String,
    pub observed_at: String,
    pub conditions: Vec<Condition>,
    pub work: Vec<WorkItem>,
    pub work_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Condition {
    pub code: Code,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    pub next_step: NextStep,
}

/// Every blocking condition that holds is listed, in this order. Each one
/// alone keeps planning from scheduling the routine, so clearing one does
/// not clear the others. `NextSlot` appears only when none holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Code {
    NotActive,
    Quarantined,
    DefinitionUnavailable,
    NextSlot,
}

/// An occurrence already planned that holds or needs work. `conditions` say
/// whether planning creates the next occurrence; `work` says what holds the
/// ones that exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkItem {
    pub code: WorkCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    pub holds_claims: bool,
    pub next_step: NextStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkCode {
    InFlight,
    RunningRun,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NextStep {
    ActivateIfIntended,
    ReviewFailureThenUnquarantine,
    ReviseDefinition,
    InspectOccurrence,
    InspectRun,
    RecoverOccurrence,
    AwaitRuntimeAuthority,
    AwaitCancellation,
    CancelRun,
    None,
}

pub(crate) enum Lookup {
    Found(Explanation),
    NotFound,
    Tombstoned,
}

/// One statement, so the definition and its quarantine come from a single
/// snapshot. Missions are not scheduled routines and stay not found, as in
/// `store::get_definition_with_tombstone`.
const ROUTINE_SELECT: &str = "
    SELECT definition.id, definition.name, definition.status, definition.definition_json,
           definition.revision, definition.definition_digest, definition.lifecycle_state,
           definition.tombstoned_at, definition.updated_at,
           retry_state.quarantined_at, retry_state.failure_class, definition.created_at
    FROM main.automation_definitions AS definition
    LEFT JOIN main.automation_retry_state AS retry_state
      ON retry_state.automation_id = definition.id
     AND retry_state.quarantined_at IS NOT NULL
    WHERE definition.id = ?1
      AND NOT EXISTS (
          SELECT 1 FROM main.automation_missions WHERE automation_id = ?1
      )";

/// At most this many work items; `work_truncated` says when more exist.
const MAX_WORK: usize = 20;

/// What the claim gate (`ELIGIBLE_OCCURRENCES_SQL` and `claim_due_occurrence`)
/// counts against the routine: a claimed or running occurrence of it, or a
/// running run keyed to it, keeps every other occurrence from being claimed.
/// Rank 0 lists claimed and running occurrences, rank 2 recovery-required
/// ones, and rank 1 every running run of the routine whose occurrence is not
/// listed by those two. Ids and times are read as bounded bytes; lease
/// owners, sessions, authority profiles and reasons are never returned.
const WORK_SELECT: &str = "
    SELECT CASE occurrence.state WHEN 'recovery_required' THEN 2 ELSE 0 END AS rank,
           substr(CAST(occurrence.id AS BLOB), 1, 513) AS occurrence_id,
           occurrence.state AS state,
           substr(CAST(occurrence.scheduled_for AS BLOB), 1, 65) AS scheduled_for,
           (SELECT substr(CAST(run.id AS BLOB), 1, 513)
            FROM main.automation_runs AS run
            WHERE run.occurrence_id = occurrence.id AND run.status = 'running'
            ORDER BY run.id
            LIMIT 1) AS run_id,
           occurrence.state <> 'recovery_required' OR EXISTS (
               SELECT 1 FROM main.automation_runs AS run
               WHERE run.automation_id = ?1
                 AND run.status = 'running'
                 AND run.occurrence_id = occurrence.id
           ) AS holds_claims,
           -- Recovery's own refusals, in the order it checks them.
           CASE
               WHEN occurrence.state <> 'recovery_required' THEN 0
               WHEN EXISTS (
                   SELECT 1 FROM main.automation_runs AS run
                   WHERE run.occurrence_id = occurrence.id
                     AND run.status = 'running'
                     AND run.authority_profile IS NOT NULL
               ) THEN 1
               WHEN EXISTS (
                   SELECT 1
                   FROM main.automation_runs AS run
                   JOIN main.automation_cancellations AS cancellation
                     ON cancellation.run_id = run.id
                   WHERE run.occurrence_id = occurrence.id
                     AND run.status = 'running'
                     AND cancellation.state IN ('requested', 'stopping')
               ) THEN 2
               WHEN EXISTS (
                   SELECT 1
                   FROM main.automation_runs AS run
                   JOIN main.automation_attempts AS attempt ON attempt.run_id = run.id
                   LEFT JOIN main.sessions AS session ON session.id = attempt.session_id
                   WHERE run.occurrence_id = occurrence.id
                     AND run.status = 'running'
                     AND attempt.attempt_number = (
                         SELECT MAX(latest.attempt_number)
                         FROM main.automation_attempts AS latest
                         WHERE latest.run_id = run.id
                     )
                     AND (
                         attempt.state IN ('dispatching', 'started', 'observing')
                         OR session.status IN ('created', 'running')
                     )
               ) THEN 3
               ELSE 0
           END AS recovery_refusal
    FROM main.automation_occurrences AS occurrence
    WHERE occurrence.automation_id = ?1
      AND occurrence.state IN ('claimed', 'running', 'recovery_required')
    UNION ALL
    SELECT 1,
           substr(CAST(run.occurrence_id AS BLOB), 1, 513),
           NULL,
           (SELECT substr(CAST(occurrence.scheduled_for AS BLOB), 1, 65)
            FROM main.automation_occurrences AS occurrence
            WHERE occurrence.id = run.occurrence_id),
           substr(CAST(run.id AS BLOB), 1, 513),
           1,
           0
    FROM main.automation_runs AS run
    WHERE run.automation_id = ?1
      AND run.status = 'running'
      AND NOT EXISTS (
          SELECT 1 FROM main.automation_occurrences AS listed
          WHERE listed.id = run.occurrence_id
            AND listed.automation_id = ?1
            AND listed.state IN ('claimed', 'running', 'recovery_required')
      )
    ORDER BY holds_claims DESC, rank, scheduled_for, occurrence_id, run_id
    LIMIT ?2";

/// Canonical stored timestamps are at most this long; a longer one is omitted.
const MAX_STORED_TIME: usize = 64;

pub(crate) fn explain(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<Lookup> {
    // A deferred transaction: every read below comes from one snapshot, and
    // dropping it writes nothing.
    let snapshot = conn
        .unchecked_transaction()
        .context("failed to begin the explanation read")?;
    let conn: &Connection = &snapshot;
    let row = conn
        .query_row(ROUTINE_SELECT, [id], |row| {
            Ok((
                super::store::routine_record_from_row(row)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
            ))
        })
        .optional()
        .context("failed to read the routine to explain")?;
    let Some((record, quarantined_at, failure_class, created_at)) = row else {
        return Ok(Lookup::NotFound);
    };
    if record.tombstoned_at.is_some() {
        return Ok(Lookup::Tombstoned);
    }

    let definition = stored_definition(&record.id, &record.definition_json).ok();
    let mut conditions = Vec::new();
    if record.status != ACTIVE_STATUS
        || definition
            .as_ref()
            .is_some_and(|definition| definition.status != RoutineStatus::Active)
    {
        conditions.push(condition(Code::NotActive, NextStep::ActivateIfIntended));
    }
    if let Some(quarantined_at) = quarantined_at {
        conditions.push(Condition {
            at: canonical(&quarantined_at),
            failure_class: failure_class.filter(|class| bounded_token(class)),
            ..condition(Code::Quarantined, NextStep::ReviewFailureThenUnquarantine)
        });
    }
    let available = definition.as_ref().is_some_and(|definition| {
        record.status != ACTIVE_STATUS || revision_verified(&record, definition, &created_at)
    });
    if !available {
        conditions.push(condition(
            Code::DefinitionUnavailable,
            NextStep::ReviseDefinition,
        ));
    }
    if let (true, Some(definition)) = (conditions.is_empty(), definition) {
        // Validation admits only DAILY or WEEKLY rules with an hour and an
        // exact timezone, which always have a slot within the search window.
        let slot = super::schedule::next_due(&definition.rrule, definition.timezone, now)
            .map_err(|_| anyhow!("the routine schedule could not be evaluated"))?
            .ok_or_else(|| anyhow!("the routine schedule has no next slot"))?;
        conditions.push(Condition {
            at: Some(slot.to_rfc3339_opts(SecondsFormat::Millis, true)),
            ..condition(Code::NextSlot, NextStep::None)
        });
    }
    let (work, work_truncated) = work(conn, &record.id)?;
    Ok(Lookup::Found(Explanation {
        automation_id: record.id,
        revision: record.revision,
        lifecycle_state: record.lifecycle_state,
        observed_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        conditions,
        work,
        work_truncated,
    }))
}

fn work(conn: &Connection, id: &str) -> Result<(Vec<WorkItem>, bool)> {
    let mut statement = conn
        .prepare(WORK_SELECT)
        .context("failed to prepare the routine's work")?;
    let limit = i64::try_from(MAX_WORK + 1).context("work limit exceeds SQLite range")?;
    let rows = statement
        .query_map(rusqlite::params![id, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .context("failed to read the routine's work")?;
    let mut items = Vec::new();
    for row in rows {
        let (rank, occurrence_id, state, scheduled_for, run_id, holds_claims, refusal) =
            row.context("failed to read the routine's work")?;
        let (code, state, next_step) = match (rank, state.as_deref()) {
            (0, Some("claimed")) => (
                WorkCode::InFlight,
                Some("claimed"),
                NextStep::InspectOccurrence,
            ),
            (0, Some("running")) => (
                WorkCode::InFlight,
                Some("running"),
                NextStep::InspectOccurrence,
            ),
            (1, None) => (WorkCode::RunningRun, None, NextStep::InspectRun),
            // The step `occurrence.recover.v1` takes: it refuses a Runtime
            // Authority run, a run whose cancellation is still reconciling,
            // and a latest attempt that may still be live.
            (2, _) => (
                WorkCode::RecoveryRequired,
                None,
                match refusal {
                    0 => NextStep::RecoverOccurrence,
                    1 => NextStep::AwaitRuntimeAuthority,
                    2 => NextStep::AwaitCancellation,
                    3 => NextStep::CancelRun,
                    _ => anyhow::bail!("the routine's work has an unexpected shape"),
                },
            ),
            _ => anyhow::bail!("the routine's work has an unexpected shape"),
        };
        items.push(WorkItem {
            code,
            occurrence_id: occurrence_id.and_then(bounded_id),
            state,
            run_id: run_id.and_then(bounded_id),
            at: scheduled_for
                .filter(|stored| stored.len() <= MAX_STORED_TIME)
                .and_then(|stored| String::from_utf8(stored).ok())
                .as_deref()
                .and_then(canonical),
            holds_claims,
            next_step,
        });
    }
    let truncated = items.len() > MAX_WORK;
    items.truncate(MAX_WORK);
    Ok((items, truncated))
}

/// An id leaves only as valid UTF-8 within the operator views' id rules.
fn bounded_id(bytes: Vec<u8>) -> Option<String> {
    String::from_utf8(bytes)
        .ok()
        .filter(|id| super::leases::valid_id(id))
}

/// Planning loads an `ACTIVE` routine by its definition's own id, verifies
/// the stored revision, and plans from its creation time. An unreadable
/// creation time falls back to the pass instant, so the routine never comes
/// due and no failure is recorded.
fn revision_verified(
    record: &RoutineRecord,
    definition: &RoutineDefinition,
    created_at: &str,
) -> bool {
    definition.id == record.id
        && DateTime::parse_from_rfc3339(created_at).is_ok()
        && verified_revision(record, definition).is_ok()
}

fn condition(code: Code, next_step: NextStep) -> Condition {
    Condition {
        code,
        at: None,
        failure_class: None,
        next_step,
    }
}

/// A stored timestamp leaves only in canonical form; anything else is omitted.
fn canonical(stored: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(stored).ok().map(|instant| {
        instant
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    })
}

/// Failure classes are snake_case tokens; the free-form reason beside them is
/// never read.
fn bounded_token(class: &str) -> bool {
    !class.is_empty()
        && class.len() <= 64
        && class
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

#[cfg(test)]
#[path = "explain_tests.rs"]
mod tests;
