//! What decides whether one routine runs next (#1518).
//!
//! Read-only. Each condition is one of the scheduler's own predicates: the
//! stored `status` planning compares, its quarantine predicate,
//! `occurrences::stored_definition` and `occurrences::verified_revision`.
//! Each work item mirrors the claim gate's own clauses or its eligibility
//! statement, and a recovery item's next step mirrors recovery's own
//! refusals. Beside the routine's
//! id, revision and lifecycle state, only codes, canonical timestamps,
//! bounded ids and failure-class tokens leave; prompts, quarantine reasons,
//! validation text, lease owners and authority profiles do not.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::definition::RoutineStatus;
use super::occurrences::{
    iso, latest_due_slot_after, latest_scheduled_slot, recorded_scheduler_authority,
    stored_definition, verified_revision, RecordedSchedulerAuthority, ACTIVE_STATUS,
    ELIGIBLE_OCCURRENCES_SQL,
};

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
    ScheduleHistoryUnreadable,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<String>,
    pub holds_claims: bool,
    pub next_step: NextStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkCode {
    InFlight,
    ClaimStale,
    LeaseExpired,
    RunningRun,
    RetryWaiting,
    RetryExpired,
    RetryHeld,
    SessionFinished,
    TimeoutExpired,
    LaunchAbandoned,
    EligibleNow,
    DueUnplanned,
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
    AwaitRetry,
    AwaitSettlement,
    AwaitSchedulerPass,
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
/// Rank 0 lists claimed and running occurrences, rank 3 recovery-required
/// ones, rank 1 every running run of the routine whose occurrence is not
/// listed by those two. Rank 2 is what a pass claims from: the eligible
/// queue, read through the scheduler's own statement, and the due slot the
/// pass plans first. Misfire supersession then skips every planned, due
/// occurrence of the routine without a running run that is earlier than its
/// latest planned, due one, so the earlier of the two is left out. `?1` is
/// the pass instant, `?2` the routines planning admits, `?3` this routine,
/// `?4` the limit, `?5` the due slot, if any, and `?6` the generation of the
/// scheduler authority on record, if a daemon holds it. Ids and times are read as
/// bounded bytes; lease owners, sessions, events, authority profiles and
/// reasons are never returned.
fn work_select() -> String {
    format!(
        "
    SELECT CASE occurrence.state WHEN 'recovery_required' THEN 3 ELSE 0 END AS rank,
           substr(CAST(occurrence.id AS BLOB), 1, 513) AS occurrence_id,
           occurrence.state AS state,
           substr(CAST(occurrence.scheduled_for AS BLOB), 1, 65) AS scheduled_for,
           substr(CAST(held.id AS BLOB), 1, 513) AS run_id,
           occurrence.state <> 'recovery_required' OR EXISTS (
               SELECT 1 FROM main.automation_runs AS run
               WHERE run.automation_id = ?3
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
           END AS recovery_refusal,
           -- Reconciliation also settles a claimed occurrence's expired retry.
           held.session_id IS NULL
               AND EXISTS (
                   SELECT 1 FROM main.automation_attempts AS attempt
                   WHERE attempt.run_id = held.id AND attempt.state = 'adopted'
               ) AS retry,
           CASE WHEN typeof(held.timeout_at) IN ('text', 'null')
               THEN CAST(held.timeout_at AS BLOB) ELSE X''
           END AS deadline,
           NULL AS not_before,
           -- A running occurrence keeps its lease, which no longer applies.
           CASE WHEN occurrence.state = 'claimed'
               THEN substr(CAST(occurrence.lease_expires_at AS BLOB), 1, 65)
           END AS lease,
           -- The claim releases a fenced pass makes before claiming: stale
           -- claims of another generation, or none, return to planned, then
           -- expired leases of this generation, or of manual claims, fail.
           CASE
               WHEN ?6 IS NULL THEN 0
               WHEN occurrence.lease_owner = 'daemon'
                    AND occurrence.scheduler_generation IS NOT ?6
                    AND NOT EXISTS (
                        SELECT 1 FROM main.automation_runs AS any_run
                        WHERE any_run.occurrence_id = occurrence.id
                    ) THEN 1
               WHEN (
                       (occurrence.lease_owner = 'daemon'
                        AND occurrence.scheduler_generation = ?6)
                       OR (occurrence.kind = 'manual'
                           AND occurrence.scheduler_generation IS NULL)
                    )
                    AND occurrence.lease_expires_at <= ?1
                    AND NOT EXISTS (
                        SELECT 1 FROM main.automation_runs AS live
                        WHERE live.occurrence_id = occurrence.id AND live.status = 'running'
                    ) THEN 2
               ELSE 0
           END AS lease_release,
           -- A failed attempt held for an operator retry, which only
           -- settlement acts on: timeout enforcement and abandoned-launch
           -- recovery stop a live attempt, reconciling a stopping
           -- cancellation can fail the pass, settlement skips a fenced run,
           -- and another running run can settle the occurrence first.
           held.authority_profile IS NULL
               AND COALESCE((
                   SELECT latest.state FROM main.automation_attempts AS latest
                   WHERE latest.run_id = held.id
                   ORDER BY latest.attempt_number DESC
                   LIMIT 1
               ) = 'failed', 0)
               AND NOT EXISTS (
                   SELECT 1 FROM main.automation_attempts AS live
                   WHERE live.run_id = held.id
                     AND live.state IN ('dispatching', 'started', 'observing')
               )
               AND NOT EXISTS (
                   SELECT 1 FROM main.automation_stop_fences AS fence
                   WHERE fence.run_id = held.id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM main.automation_cancellations AS cancellation
                   WHERE cancellation.run_id = held.id AND cancellation.state = 'stopping'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM main.automation_runs AS rival
                   WHERE rival.occurrence_id = occurrence.id
                     AND rival.status = 'running'
                     AND rival.id <> held.id
               ) AS held_retry,
           -- A run whose session has finished, which settlement settles from
           -- that evidence: 1 for a plain run, and 2 for a Runtime Authority
           -- run, which it holds for recovery instead. Expired cancellations
           -- are reconciled first, and another running run can settle the
           -- occurrence first. Settlement skips a fenced plain run, or holds it
           -- for recovery, and skips an ambiguous one whose cancellation is
           -- pending; it fails on a latest attempt it cannot settle.
           CASE
               WHEN session.status IS NULL
                    OR session.status NOT IN ('completed', 'failed', 'cancelled', 'killed', 'idle')
                    OR EXISTS (
                        SELECT 1 FROM main.automation_cancellations AS cancellation
                        WHERE cancellation.run_id = held.id
                          AND cancellation.state IN ('requested', 'stopping')
                          AND cancellation.execution_expires_at <= ?1
                    )
                    OR EXISTS (
                        SELECT 1 FROM main.automation_runs AS rival
                        WHERE rival.occurrence_id = occurrence.id
                          AND rival.status = 'running'
                          AND rival.id <> held.id
                    ) THEN 0
               WHEN held.authority_profile IS NOT NULL THEN
                   CASE WHEN (
                       SELECT COUNT(*) FROM main.automation_attempts AS unresolved
                       WHERE unresolved.run_id = held.id
                         AND unresolved.state IN (
                             'dispatching', 'started', 'observing', 'ambiguous'
                         )
                   ) = 1 THEN 2 ELSE 0 END
               WHEN latest.state NOT IN ('dispatching', 'started', 'observing', 'ambiguous')
                   THEN 0
               WHEN latest.state IS NOT 'ambiguous' AND EXISTS (
                   SELECT 1 FROM main.automation_stop_fences AS fence
                   WHERE fence.run_id = held.id
               ) THEN 0
               WHEN latest.state = 'ambiguous' AND EXISTS (
                   SELECT 1 FROM main.automation_cancellations AS pending
                   WHERE pending.run_id = held.id
                     AND pending.attempt_id = latest.id
                     AND pending.session_id IS held.session_id
                     AND pending.state IN ('requested', 'stopping')
               ) THEN 0
               ELSE 1
           END AS finished,
           -- A run past its deadline whose session is running, which timeout
           -- enforcement stops first: 1 for a plain run, which it settles, or
           -- holds for recovery when the stop is not confirmed, and 2 for a
           -- Runtime Authority run, which it holds for recovery either way. It
           -- leaves a run with a pending cancellation or a stop fence to that
           -- stop, and another running run can settle the occurrence first.
           -- The stop can fail unless exactly one attempt is live on the
           -- session, where a dispatch needs none, and no other running run
           -- shares the session. Settling a plain run binds a dispatch to the
           -- session, which no other attempt may hold; a Runtime Authority
           -- hold needs that attempt to be the only unresolved one.
           CASE
               WHEN session.status IS NOT 'running'
                    OR (
                        SELECT COUNT(*) FROM main.automation_attempts AS live
                        WHERE live.run_id = held.id
                          AND live.state IN ('dispatching', 'started', 'observing')
                          AND (
                              live.session_id = held.session_id
                              OR (live.state = 'dispatching' AND live.session_id IS NULL)
                          )
                    ) <> 1
                    OR EXISTS (
                        SELECT 1 FROM main.automation_runs AS sharer
                        WHERE sharer.session_id = held.session_id
                          AND sharer.status = 'running'
                          AND sharer.id <> held.id
                    )
                    OR EXISTS (
                        SELECT 1 FROM main.automation_cancellations AS cancellation
                        WHERE cancellation.run_id = held.id
                          AND cancellation.state IN ('requested', 'stopping')
                    )
                    OR EXISTS (
                        SELECT 1 FROM main.automation_stop_fences AS fence
                        WHERE fence.run_id = held.id
                    )
                    OR EXISTS (
                        SELECT 1 FROM main.automation_runs AS rival
                        WHERE rival.occurrence_id = occurrence.id
                          AND rival.status = 'running'
                          AND rival.id <> held.id
                    ) THEN 0
               WHEN held.authority_profile IS NULL THEN
                   CASE WHEN EXISTS (
                       SELECT 1 FROM main.automation_attempts AS bound
                       WHERE bound.session_id = held.session_id
                         AND NOT (
                             bound.run_id = held.id
                             AND bound.state IN ('dispatching', 'started', 'observing')
                         )
                   ) THEN 0 ELSE 1 END
               WHEN (
                   SELECT COUNT(*) FROM main.automation_attempts AS unresolved
                   WHERE unresolved.run_id = held.id
                     AND unresolved.state IN (
                         'dispatching', 'started', 'observing', 'ambiguous'
                     )
               ) = 1 THEN 2
               ELSE 0
           END AS timed_out,
           -- A launch whose session has not started when its claim's lease
           -- expires, which abandoned-launch recovery stops, and settlement
           -- then settles in the same pass: 1 for a plain run, and 2 for a
           -- Runtime Authority run, which it holds for recovery. A stop that
           -- is not confirmed holds either for recovery. Recovery compares the
           -- lease and the deadline as stored text, leaving a run past its
           -- deadline to timeout enforcement, and leaves a run with a pending
           -- cancellation or a stop fence to that stop; another running run
           -- can settle the occurrence first. The stop can fail unless
           -- exactly one attempt is live on the session, where a dispatch
           -- needs none, and no other running run shares the session.
           -- Settling a plain run needs its latest attempt live, and holding
           -- a Runtime Authority one needs that attempt to be its only
           -- unresolved one.
           CASE
               WHEN session.status IS NOT 'created'
                    OR occurrence.lease_expires_at IS NULL
                    OR occurrence.lease_expires_at > ?1
                    OR held.timeout_at <= ?1
                    OR (
                        SELECT COUNT(*) FROM main.automation_attempts AS live
                        WHERE live.run_id = held.id
                          AND live.state IN ('dispatching', 'started', 'observing')
                          AND (
                              live.session_id = held.session_id
                              OR (live.state = 'dispatching' AND live.session_id IS NULL)
                          )
                    ) <> 1
                    OR EXISTS (
                        SELECT 1 FROM main.automation_runs AS sharer
                        WHERE sharer.session_id = held.session_id
                          AND sharer.status = 'running'
                          AND sharer.id <> held.id
                    )
                    OR EXISTS (
                        SELECT 1 FROM main.automation_cancellations AS cancellation
                        WHERE cancellation.run_id = held.id
                          AND cancellation.state IN ('requested', 'stopping')
                    )
                    OR EXISTS (
                        SELECT 1 FROM main.automation_stop_fences AS fence
                        WHERE fence.run_id = held.id
                    )
                    OR EXISTS (
                        SELECT 1 FROM main.automation_runs AS rival
                        WHERE rival.occurrence_id = occurrence.id
                          AND rival.status = 'running'
                          AND rival.id <> held.id
                    ) THEN 0
               WHEN held.authority_profile IS NULL THEN
                   CASE WHEN latest.state IN ('dispatching', 'started', 'observing')
                       THEN 1 ELSE 0 END
               WHEN (
                   SELECT COUNT(*) FROM main.automation_attempts AS unresolved
                   WHERE unresolved.run_id = held.id
                     AND unresolved.state IN (
                         'dispatching', 'started', 'observing', 'ambiguous'
                     )
               ) = 1 THEN 2
               ELSE 0
           END AS abandoned,
           -- Whether settlement's scan of running runs decodes this run's
           -- values as their types; recording a plain run's finish also
           -- reads its definition.
           typeof(held.timeout_at) IN ('text', 'null')
               AND (held.authority_profile IS NOT NULL
                    OR typeof(held.definition_json) IN ('text', 'null'))
               AND typeof(session.exit_code) IN ('integer', 'null')
               AND typeof(latest.settled_at) IN ('text', 'null')
               AND typeof(COALESCE(ending.created_at, session.updated_at))
                   IN ('text', 'null') AS typed,
           -- The time settlement parses: an ambiguous attempt's settlement
           -- time, or, with a deadline, the session's last exit or update.
           CASE
               WHEN held.authority_profile IS NOT NULL THEN NULL
               WHEN latest.state = 'ambiguous'
                   THEN COALESCE(CAST(latest.settled_at AS BLOB), X'')
               WHEN held.timeout_at IS NOT NULL
                   THEN CAST(COALESCE(ending.created_at, session.updated_at) AS BLOB)
           END AS evidence,
           -- Every value settlement decodes as text for this run, joined by
           -- line feeds so that one not valid UTF-8 keeps the whole from
           -- decoding. Recording a plain run's finish reads its definition.
           CAST(
               COALESCE(CAST(held.timeout_at AS BLOB), X'') || X'0A'
               || COALESCE(CAST(COALESCE(ending.created_at, session.updated_at) AS BLOB), X'')
               || X'0A' || COALESCE(CAST(latest.settled_at AS BLOB), X'') || X'0A'
               || CASE WHEN held.authority_profile IS NULL
                      THEN COALESCE(CAST(held.definition_json AS BLOB), X'')
                      ELSE X''
                  END
           AS BLOB) AS decoded,
           -- The times a pass parses for a launch: restart containment its
           -- session's creation, and settling a plain run with a deadline the
           -- session's exit, when it has one.
           CASE WHEN typeof(session.created_at) = 'text'
               THEN CAST(session.created_at AS BLOB) ELSE X''
           END AS launched,
           CASE WHEN held.authority_profile IS NULL AND held.timeout_at IS NOT NULL
               THEN CAST(ending.created_at AS BLOB)
           END AS exited,
           -- What restart containment also reads for a launch as text: its
           -- session id, which names its receipt file, and its occurrence's
           -- kind and lease owner, joined by a line feed, the owner when not
           -- null. Each is null when stored as another type.
           CASE WHEN typeof(held.session_id) = 'text'
               THEN CAST(held.session_id AS BLOB)
           END AS receipt,
           CASE WHEN typeof(occurrence.kind) = 'text'
                     AND typeof(occurrence.lease_owner) IN ('text', 'null')
               THEN CAST(
                   CAST(occurrence.kind AS BLOB) || X'0A'
                   || COALESCE(CAST(occurrence.lease_owner AS BLOB), X'')
               AS BLOB)
           END AS contained
    FROM main.automation_occurrences AS occurrence
    LEFT JOIN main.automation_runs AS held
      ON held.id = (
          SELECT first.id FROM main.automation_runs AS first
          WHERE first.occurrence_id = occurrence.id AND first.status = 'running'
          ORDER BY first.id
          LIMIT 1
      )
    LEFT JOIN main.sessions AS session ON session.id = held.session_id
    LEFT JOIN main.events AS ending
      ON ending.id = (
          SELECT exited.id FROM main.events AS exited
          WHERE exited.session_id = session.id AND exited.kind = 'exit'
          ORDER BY exited.created_at DESC, exited.id DESC
          LIMIT 1
      )
    LEFT JOIN main.automation_attempts AS latest
      ON latest.id = (
          SELECT newest.id FROM main.automation_attempts AS newest
          WHERE newest.run_id = held.id
          ORDER BY newest.attempt_number DESC
          LIMIT 1
      )
    WHERE occurrence.automation_id = ?3
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
           0,
           -- The retry reconciliation and the eligible queue time: an adopted
           -- attempt on a run with no session, whose occurrence, of any
           -- routine, is planned.
           run.session_id IS NULL
               AND EXISTS (
                   SELECT 1 FROM main.automation_attempts AS attempt
                   WHERE attempt.run_id = run.id AND attempt.state = 'adopted'
               )
               AND EXISTS (
                   SELECT 1 FROM main.automation_occurrences AS occurrence
                   WHERE occurrence.id = run.occurrence_id
                     AND occurrence.state = 'planned'
               ),
           CASE WHEN typeof(run.timeout_at) IN ('text', 'null')
               THEN CAST(run.timeout_at AS BLOB) ELSE X''
           END,
           (SELECT substr(CAST(MAX(attempt.not_before) AS BLOB), 1, 65)
            FROM main.automation_attempts AS attempt
            WHERE attempt.run_id = run.id
              AND attempt.state = 'adopted'
              AND attempt.not_before > ?1),
           NULL,
           0,
           0,
           0,
           0,
           0,
           0,
           NULL,
           NULL,
           NULL,
           NULL,
           NULL,
           NULL
    FROM main.automation_runs AS run
    WHERE run.automation_id = ?3
      AND run.status = 'running'
      AND NOT EXISTS (
          SELECT 1 FROM main.automation_occurrences AS listed
          WHERE listed.id = run.occurrence_id
            AND listed.automation_id = ?3
            AND listed.state IN ('claimed', 'running', 'recovery_required')
      )
    UNION ALL
    SELECT 2,
           substr(CAST(eligible.id AS BLOB), 1, 513),
           NULL,
           substr(CAST(eligible.scheduled_for AS BLOB), 1, 65),
           NULL,
           0,
           0,
           0,
           NULL,
           NULL,
           NULL,
           0,
           0,
           0,
           0,
           0,
           0,
           NULL,
           NULL,
           NULL,
           NULL,
           NULL,
           NULL
    FROM ({ELIGIBLE_OCCURRENCES_SQL}) AS eligible
    WHERE ?5 IS NULL
       OR eligible.scheduled_for >= ?5
       OR EXISTS (
           SELECT 1 FROM main.automation_runs AS run
           WHERE run.occurrence_id = eligible.id AND run.status = 'running'
       )
    UNION ALL
    SELECT 2, NULL, 'unplanned', CAST(?5 AS BLOB), NULL, 0, 0, 0, NULL, NULL, NULL, 0, 0, 0,
           0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL
    WHERE ?5 IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM main.automation_occurrences AS later
          WHERE later.automation_id = ?3
            AND later.state = 'planned'
            AND later.scheduled_for <= ?1
            AND later.scheduled_for > ?5
      )
    ORDER BY holds_claims DESC, rank, scheduled_for, occurrence_id, run_id
    LIMIT ?4"
    )
}

/// Canonical stored timestamps are at most this long; a longer one is omitted.
const MAX_STORED_TIME: usize = 64;

/// A launch is named only when its session id is at most this long, so its
/// receipt's file name stays well inside every platform's limit.
const MAX_RECEIPT_ID: usize = 200;

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
    let quarantined = quarantined_at.is_some();
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
    // Planning loads an `ACTIVE` routine by its definition's own id, verifies
    // the stored revision, then reads the revision's latest scheduled slot to
    // plan from. An unreadable creation time falls back to the pass instant,
    // so the routine never comes due, but planning still reads the history.
    let revision = definition
        .as_ref()
        .filter(|definition| definition.id == record.id)
        .and_then(|definition| verified_revision(&record, definition).ok());
    let available = definition.is_some()
        && (record.status != ACTIVE_STATUS
            || (revision.is_some() && DateTime::parse_from_rfc3339(&created_at).is_ok()));
    if !available {
        conditions.push(condition(
            Code::DefinitionUnavailable,
            NextStep::ReviseDefinition,
        ));
    }
    // A verified revision requires a stored `ACTIVE` row. A new revision
    // starts an empty history, so revising clears an unreadable one.
    let history = revision.as_ref().map(|(digest, revision_updated_at)| {
        latest_scheduled_slot(conn, &record.id, record.revision, digest)
            .map(|latest| (latest, *revision_updated_at))
    });
    if matches!(history, Some(Err(_))) {
        conditions.push(condition(
            Code::ScheduleHistoryUnreadable,
            NextStep::ReviseDefinition,
        ));
    }
    // A planning pass that reaches the routine admits its occurrences to
    // claiming and first plans the latest slot due since the cursor it keeps,
    // computed here as planning computes it.
    let (admitted, due_slot) = match (&history, &definition) {
        (Some(Ok((latest, revision_updated_at))), Some(definition))
            if !quarantined && definition.status == RoutineStatus::Active =>
        {
            let created = DateTime::parse_from_rfc3339(&created_at)
                .map(|created| created.with_timezone(&Utc))
                .unwrap_or(now);
            let effective = created.max(*revision_updated_at);
            let cursor = latest.map_or(effective, |latest| effective.max(latest));
            let due = latest_due_slot_after(definition, cursor, now)
                .map_err(|_| anyhow!("the routine schedule could not be evaluated"))?;
            match due {
                Some(slot) => planned_slot(conn, &record.id, slot)?,
                None => (true, None),
            }
        }
        _ => (false, None),
    };
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
    // The daemon's pass is fenced by the scheduler authority on record; with
    // none recorded, no pass would release a claim.
    let generation = match recorded_scheduler_authority(conn)? {
        RecordedSchedulerAuthority::Observed(authority) => Some(
            i64::try_from(authority.generation())
                .context("scheduler generation exceeds SQLite range")?,
        ),
        _ => None,
    };
    let (work, work_truncated) = work(conn, &record.id, admitted, due_slot, generation, now)?;
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

/// Planning creates a due slot under an id derived from it. When that id
/// already exists the slot counts as fenced and nothing is created; when
/// another occurrence holds the routine's slot, planning fails the routine,
/// which keeps it from claiming.
fn planned_slot(
    conn: &Connection,
    id: &str,
    slot: DateTime<Utc>,
) -> Result<(bool, Option<DateTime<Utc>>)> {
    let (fenced, taken) = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM main.automation_occurrences WHERE id = ?1),
                    EXISTS (SELECT 1 FROM main.automation_occurrences
                            WHERE automation_id = ?2 AND scheduled_for = ?3)",
            rusqlite::params![format!("{id}-{}", slot.timestamp_millis()), id, iso(slot)],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
        )
        .context("failed to read the routine's planned slots")?;
    Ok(match (fenced, taken) {
        (true, _) => (true, None),
        (false, true) => (false, None),
        (false, false) => (true, Some(slot)),
    })
}

fn work(
    conn: &Connection,
    id: &str,
    admitted: bool,
    due_slot: Option<DateTime<Utc>>,
    generation: Option<i64>,
    now: DateTime<Utc>,
) -> Result<(Vec<WorkItem>, bool)> {
    let admitted: Vec<&str> = if admitted { vec![id] } else { Vec::new() };
    let admitted =
        serde_json::to_string(&admitted).context("failed to encode the admitted routine")?;
    let mut statement = conn
        .prepare(&work_select())
        .context("failed to prepare the routine's work")?;
    let limit = i64::try_from(MAX_WORK + 1).context("work limit exceeds SQLite range")?;
    let rows = statement
        .query_map(
            rusqlite::params![iso(now), admitted, id, limit, due_slot.map(iso), generation],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, Option<Vec<u8>>>(8)?,
                    row.get::<_, Option<Vec<u8>>>(9)?,
                    row.get::<_, Option<Vec<u8>>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, bool>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, bool>(16)?,
                    row.get::<_, Option<Vec<u8>>>(17)?,
                    row.get::<_, Option<Vec<u8>>>(18)?,
                    row.get::<_, Option<Vec<u8>>>(19)?,
                    row.get::<_, Option<Vec<u8>>>(20)?,
                    row.get::<_, Option<Vec<u8>>>(21)?,
                    row.get::<_, Option<Vec<u8>>>(22)?,
                ))
            },
        )
        .context("failed to read the routine's work")?;
    let mut items = Vec::new();
    for row in rows {
        let (
            rank,
            occurrence_id,
            state,
            scheduled_for,
            run_id,
            holds_claims,
            refusal,
            retry,
            deadline,
            not_before,
            lease,
            lease_release,
            held_retry,
            finished,
            timed_out,
            abandoned,
            typed,
            evidence,
            decoded,
            launched,
            exited,
            receipt,
            contained,
        ) = row.context("failed to read the routine's work")?;
        // Reconciliation parses the whole stored deadline and settles the
        // retry once it has passed. The eligible queue compares the deadline
        // and `not_before` as stored text, so a waiting retry needs its
        // deadline open both ways, and `notBefore` is shown only when its text
        // is canonical. A deadline that does not parse leaves a run unnamed.
        let deadline = deadline.map(|stored| {
            String::from_utf8(stored).ok().and_then(|stored| {
                DateTime::parse_from_rfc3339(&stored)
                    .ok()
                    .map(|deadline| (deadline.with_timezone(&Utc), stored))
            })
        });
        let expired = matches!(&deadline, Some(Some((deadline, _))) if *deadline <= now);
        let unreadable = matches!(&deadline, Some(None));
        let dated = deadline.is_some();
        // Expiry is matched first, so an open deadline must also read as
        // open as text.
        let open = match &deadline {
            None => true,
            Some(Some((_, stored))) => stored.as_str() > iso(now).as_str(),
            Some(None) => false,
        };
        let decodes = typed && decoded.is_none_or(|stored| std::str::from_utf8(&stored).is_ok());
        let readable = decodes && parses(evidence);
        let deadline_at = deadline
            .flatten()
            .map(|(deadline, _)| deadline.to_rfc3339_opts(SecondsFormat::Millis, true));
        let mut timing = (None, None);
        let (code, state, next_step) = match (rank, state.as_deref()) {
            // Each pass first settles an adopted retry failed once its
            // deadline has passed, whatever its `not_before`.
            (0 | 1, Some("claimed") | None) if retry && expired => {
                timing.1 = deadline_at;
                (WorkCode::RetryExpired, None, NextStep::AwaitSettlement)
            }
            // Settlement then settles a held failed attempt at the run's
            // deadline, or at once with none; a deadline it cannot parse
            // fails the pass instead. Timeout enforcement takes a dated
            // adopted retry first, and fails on a running occurrence.
            (0, Some(state @ ("claimed" | "running")))
                if held_retry && !unreadable && !(retry && dated && state == "running") =>
            {
                timing.1 = deadline_at;
                (WorkCode::RetryHeld, None, NextStep::AwaitSettlement)
            }
            // Settlement settles a plain run whose session has finished, and
            // holds a Runtime Authority one for recovery, which does not parse
            // its deadline; evidence or a deadline it cannot read fails the
            // pass instead.
            (0, Some("claimed" | "running"))
                if readable && (finished == 2 || (finished == 1 && !unreadable)) =>
            {
                let next_step = if finished == 2 {
                    NextStep::AwaitRuntimeAuthority
                } else {
                    NextStep::AwaitSettlement
                };
                (WorkCode::SessionFinished, None, next_step)
            }
            // Timeout enforcement stops a session still running past the
            // run's deadline, which it parses whole.
            (0, Some("claimed" | "running")) if expired && timed_out != 0 && decodes => {
                timing.1 = deadline_at;
                let next_step = if timed_out == 2 {
                    NextStep::AwaitRuntimeAuthority
                } else {
                    NextStep::AwaitSettlement
                };
                (WorkCode::TimeoutExpired, None, next_step)
            }
            // Abandoned-launch recovery stops a launch past its claim's
            // lease, and settlement then settles a plain run, parsing its
            // deadline, or holds a Runtime Authority one.
            (0, Some("claimed"))
                if abandoned != 0
                    && decodes
                    && parses(launched)
                    && parses(exited)
                    && contains(receipt, contained)
                    && (abandoned == 2 || !unreadable) =>
            {
                let next_step = if abandoned == 2 {
                    NextStep::AwaitRuntimeAuthority
                } else {
                    NextStep::AwaitSettlement
                };
                (WorkCode::LaunchAbandoned, None, next_step)
            }
            (0, Some("claimed")) => match lease_release {
                0 => (
                    WorkCode::InFlight,
                    Some("claimed"),
                    NextStep::InspectOccurrence,
                ),
                1 => (WorkCode::ClaimStale, None, NextStep::AwaitSchedulerPass),
                2 => (WorkCode::LeaseExpired, None, NextStep::AwaitSettlement),
                _ => anyhow::bail!("the routine's work has an unexpected shape"),
            },
            (0, Some("running")) => (
                WorkCode::InFlight,
                Some("running"),
                NextStep::InspectOccurrence,
            ),
            (1, None) if retry && open && not_before.is_some() => {
                let not_before = not_before
                    .and_then(stored_time)
                    .filter(|stored| canonical(stored).as_deref() == Some(stored.as_str()));
                timing = (not_before, deadline_at);
                (WorkCode::RetryWaiting, None, NextStep::AwaitRetry)
            }
            (1, None) => (WorkCode::RunningRun, None, NextStep::InspectRun),
            (2, None) => (WorkCode::EligibleNow, None, NextStep::AwaitSchedulerPass),
            (2, Some("unplanned")) => (WorkCode::DueUnplanned, None, NextStep::AwaitSchedulerPass),
            // The step `occurrence.recover.v1` takes: it refuses a Runtime
            // Authority run, a run whose cancellation is still reconciling,
            // and a latest attempt that may still be live.
            (3, _) => (
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
                .and_then(stored_time)
                .as_deref()
                .and_then(canonical),
            not_before: timing.0,
            deadline_at: timing.1,
            // The pass compares the lease as stored text, so only a canonical
            // one is shown.
            lease_expires_at: lease
                .and_then(stored_time)
                .filter(|stored| canonical(stored).as_deref() == Some(stored.as_str())),
            holds_claims,
            next_step,
        });
    }
    let truncated = items.len() > MAX_WORK;
    items.truncate(MAX_WORK);
    Ok((items, truncated))
}

/// Whether a time the pass parses whole, if there is one, parses.
fn parses(stored: Option<Vec<u8>>) -> bool {
    stored.is_none_or(|stored| {
        String::from_utf8(stored).is_ok_and(|stored| DateTime::parse_from_rfc3339(&stored).is_ok())
    })
}

/// Whether restart containment reads a launch's values: its whole session id
/// names a plain receipt file, and its other values are valid UTF-8.
fn contains(receipt: Option<Vec<u8>>, read: Option<Vec<u8>>) -> bool {
    receipt.is_some_and(|session_id| {
        session_id.len() <= MAX_RECEIPT_ID
            && session_id
                .iter()
                .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }) && read.is_some_and(|read| std::str::from_utf8(&read).is_ok())
}

/// A stored time is read only when it is UTF-8 within the length bound.
fn stored_time(stored: Vec<u8>) -> Option<String> {
    (stored.len() <= MAX_STORED_TIME)
        .then(|| String::from_utf8(stored).ok())
        .flatten()
}

/// An id leaves only as valid UTF-8 within the operator views' id rules.
fn bounded_id(bytes: Vec<u8>) -> Option<String> {
    String::from_utf8(bytes)
        .ok()
        .filter(|id| super::leases::valid_id(id))
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
