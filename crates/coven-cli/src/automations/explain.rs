//! What decides whether one routine runs next (#1518).
//!
//! Read-only. Each condition is one of the scheduler's own predicates: the
//! stored `status` planning compares, its quarantine predicate,
//! `occurrences::stored_definition` and `occurrences::verified_revision`.
//! Beside the routine's id, revision and lifecycle state, only condition
//! codes, canonical timestamps and bounded failure-class tokens leave;
//! prompts, quarantine reasons and validation text do not.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NextStep {
    ActivateIfIntended,
    ReviewFailureThenUnquarantine,
    ReviseDefinition,
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

pub(crate) fn explain(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<Lookup> {
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
    Ok(Lookup::Found(explanation(record, now, conditions)))
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

fn explanation(
    record: RoutineRecord,
    now: DateTime<Utc>,
    conditions: Vec<Condition>,
) -> Explanation {
    Explanation {
        automation_id: record.id,
        revision: record.revision,
        lifecycle_state: record.lifecycle_state,
        observed_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        conditions,
    }
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
