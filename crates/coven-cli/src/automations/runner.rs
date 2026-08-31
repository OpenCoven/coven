//! Routine run dispatch (coven#816).
//!
//! A run is a claimed occurrence dispatched through the exact session-launch
//! path every other launch uses. Claiming pins the immutable definition
//! revision/digest and delivery inputs; dispatch revalidates the pinned
//! familiar/runtime, durably links the occurrence, run, and session, and only
//! then spawns. Coven (not a harness home) owns the record; terminal status,
//! bounded log, and output delivery land through reconciliation.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::definition::RoutineDefinition;
use super::occurrences::{
    claim_occurrence_by_id_at_revision, fail_occurrence_nonterminal, mark_occurrence_running,
    settle_occurrence,
};
use super::runs::{record_run_finish, record_run_start_pinned, PinnedRunStart, RunFinish};
use super::store::DefinitionSnapshot;
use crate::api::{DurableSessionLaunchError, DurableSessionStore, SessionLaunch, SessionRuntime};
use crate::harness::HarnessLaunchMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub run_id: String,
    pub status: String,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

fn fresh_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

#[cfg(test)]
struct DispatchClaimHook {
    occurrence_id: Option<String>,
    automation_id: Option<String>,
    reached: std::sync::mpsc::SyncSender<String>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
static DISPATCH_CLAIM_HOOKS: std::sync::Mutex<Vec<DispatchClaimHook>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn install_dispatch_claim_hook(hook: DispatchClaimHook) {
    DISPATCH_CLAIM_HOOKS.lock().unwrap().push(hook);
}

#[cfg(test)]
fn pause_dispatch_claim_for_test(occurrence_id: &str, automation_id: Option<&str>) {
    let hook = {
        let mut installed = DISPATCH_CLAIM_HOOKS.lock().unwrap();
        installed
            .iter()
            .position(|hook| {
                hook.occurrence_id.as_deref() == Some(occurrence_id)
                    || hook.automation_id.as_deref().is_some_and(|target| {
                        automation_id.is_some_and(|automation_id| target == automation_id)
                    })
            })
            .map(|position| installed.remove(position))
    };
    if let Some(hook) = hook {
        hook.reached.send(occurrence_id.to_string()).unwrap();
        hook.release.recv().unwrap();
    }
}

#[cfg(not(test))]
fn pause_dispatch_claim_for_test(_occurrence_id: &str, _automation_id: Option<&str>) {}

fn overlap_outcome(definition: &RoutineDefinition) -> RunOutcome {
    RunOutcome {
        run_id: String::new(),
        status: "failed".to_string(),
        session_id: None,
        error: Some(format!(
            "overlap: another occurrence of `{}` is still running",
            definition.id
        )),
    }
}

struct PinnedOccurrence {
    id: String,
    automation_id: String,
    definition: RoutineDefinition,
    snapshot: DefinitionSnapshot,
    deadline_at: String,
}

struct PinnedOccurrenceRow {
    automation_id: String,
    revision: Option<i64>,
    digest: Option<String>,
    definition_json: Option<String>,
    output_target: Option<String>,
    deadline_at: Option<String>,
}

enum ClaimedOccurrenceLoad {
    Ready(Box<PinnedOccurrence>),
    AlreadyHandled,
    Malformed(String),
}

fn load_pinned_occurrence(
    conn: &Connection,
    occurrence_id: &str,
) -> Result<ClaimedOccurrenceLoad, String> {
    let row = conn
        .query_row(
            "SELECT automation_id, definition_revision, definition_digest, definition_json,
                    output_target, deadline_at
             FROM automation_occurrences
             WHERE id = ?1 AND state = 'claimed'",
            params![occurrence_id],
            |row| {
                Ok(PinnedOccurrenceRow {
                    automation_id: row.get(0)?,
                    revision: row.get(1)?,
                    digest: row.get(2)?,
                    definition_json: row.get(3)?,
                    output_target: row.get(4)?,
                    deadline_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| format!("failed to read claimed occurrence: {error}"))?;
    let Some(row) = row else {
        return Ok(ClaimedOccurrenceLoad::AlreadyHandled);
    };
    let validated = (|| -> Result<PinnedOccurrence, String> {
        let revision = row
            .revision
            .ok_or_else(|| "claimed occurrence has no pinned revision".to_string())?;
        let digest = row
            .digest
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "claimed occurrence has no pinned definition digest".to_string())?;
        let definition_json = row
            .definition_json
            .ok_or_else(|| "claimed occurrence has no pinned definition".to_string())?;
        let deadline_at = row
            .deadline_at
            .ok_or_else(|| "claimed occurrence has no pinned deadline".to_string())?;
        let definition: RoutineDefinition =
            serde_json::from_str(&definition_json).map_err(|error| {
                format!(
                    "pinned routine `{}` is unreadable: {error}",
                    row.automation_id
                )
            })?;
        let timeout_minutes = i64::from(definition.timeout_minutes);
        Ok(PinnedOccurrence {
            id: occurrence_id.to_string(),
            automation_id: row.automation_id,
            definition,
            snapshot: DefinitionSnapshot {
                revision,
                digest,
                definition_json,
                output_target: row.output_target,
                timeout_minutes,
            },
            deadline_at,
        })
    })();
    match validated {
        Ok(pinned) => Ok(ClaimedOccurrenceLoad::Ready(Box::new(pinned))),
        Err(reason) => Ok(ClaimedOccurrenceLoad::Malformed(reason)),
    }
}

fn occurrence_state(conn: &Connection, occurrence_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT state FROM automation_occurrences WHERE id = ?1",
        params![occurrence_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| format!("failed to inspect occurrence {occurrence_id} state: {error}"))
}

fn fail_claimed_occurrence(
    conn: &Connection,
    occurrence_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'failed',
                 failure_reason = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?3
             WHERE id = ?1 AND state = 'claimed'",
            params![
                occurrence_id,
                reason,
                now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .map_err(|error| format!("failed to reject malformed claimed occurrence: {error}"))?;
    Ok(changed == 1)
}

fn automation_launch(
    coven_home: &Path,
    pinned: &PinnedOccurrence,
) -> Result<SessionLaunch, String> {
    crate::session_launch::validate_harness(
        &pinned.definition.runtime,
        crate::session_launch::HarnessCheck::Configured,
    )
    .map_err(|error| format!("{error:#}"))?;
    let cwd = pinned
        .definition
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .ok_or_else(|| "routine has no cwd; add a cwd before running".to_string())?;
    let paths = crate::session_launch::resolve_launch_paths(Path::new(cwd), Some(Path::new(cwd)))
        .map_err(|error| match error {
        crate::session_launch::LaunchPathError::ProjectRoot(error)
        | crate::session_launch::LaunchPathError::Cwd(error) => format!("{error:#}"),
    })?;
    let familiar = crate::session_launch::resolve_familiar(
        coven_home,
        pinned.definition.familiar_id.as_deref(),
    )
    .map_err(crate::session_launch::FamiliarError::into_error)
    .map_err(|error| format!("{error:#}"))?;
    let mut launch = build_session_launch(&pinned.definition, &paths.cwd.to_string_lossy())?;
    launch.project_root = paths.project_root.to_string_lossy().into_owned();
    launch.cwd = paths.cwd.to_string_lossy().into_owned();
    launch.familiar_id = familiar.as_ref().map(|context| context.id.clone());
    Ok(launch)
}

fn durable_error_text(error: DurableSessionLaunchError) -> String {
    match error {
        DurableSessionLaunchError::Maintenance(error)
        | DurableSessionLaunchError::Persistence(error)
        | DurableSessionLaunchError::PersistedState(error)
        | DurableSessionLaunchError::Runtime(error)
        | DurableSessionLaunchError::OwnershipRetained(error) => format!("{error:#}"),
        DurableSessionLaunchError::AlreadyDispatched => {
            "occurrence was already dispatched".to_string()
        }
        DurableSessionLaunchError::Rejected(response) => {
            format!("durable automation launch was rejected: {}", response.body)
        }
    }
}

fn settle_launch_failure(
    conn: &Connection,
    pinned: &PinnedOccurrence,
    run_id: &str,
    session_id: Option<&str>,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    match settle_launch_failure_with_run(conn, pinned, run_id, session_id, reason, now) {
        Ok(()) => Ok(()),
        Err(settlement_error) => {
            let failed = fail_occurrence_nonterminal(conn, &pinned.id, reason, now)?;
            if !failed {
                let state: String = conn
                    .query_row(
                        "SELECT state FROM automation_occurrences WHERE id = ?1",
                        params![pinned.id],
                        |row| row.get(0),
                    )
                    .map_err(|error| {
                        format!("failed to verify independently settled occurrence: {error}")
                    })?;
                if state != "failed" {
                    return Err(format!(
                        "{settlement_error}; occurrence remained in state `{state}`"
                    ));
                }
            }
            Err(settlement_error)
        }
    }
}

fn settle_launch_failure_with_run(
    conn: &Connection,
    pinned: &PinnedOccurrence,
    run_id: &str,
    session_id: Option<&str>,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin launch-failure settlement: {error}"))?;
    let existing_session_id: Option<Option<String>> = transaction
        .query_row(
            "SELECT session_id FROM automation_runs WHERE id = ?1",
            params![run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("failed to inspect launch-failure run: {error}"))?;
    if existing_session_id.is_none() {
        record_run_start_pinned(
            &transaction,
            PinnedRunStart {
                run_id,
                automation_id: &pinned.automation_id,
                occurrence_id: &pinned.id,
                session_id,
                familiar_id: pinned.definition.familiar_id.as_deref(),
                runtime: &pinned.definition.runtime,
                snapshot: &pinned.snapshot,
                deadline_at: &pinned.deadline_at,
                now,
            },
        )
        .map_err(|error| format!("failed to record rejected run: {error:#}"))?;
    }
    let session_id = existing_session_id
        .flatten()
        .or_else(|| session_id.map(str::to_string));
    if !fail_occurrence_nonterminal(&transaction, &pinned.id, reason, now)? {
        return Err("failed to terminally settle rejected occurrence".to_string());
    }
    let finished = record_run_finish(
        &transaction,
        run_id,
        RunFinish {
            status: "failed",
            exit_code: None,
            session_id,
            log_json: None,
            output_commit: None,
        },
        now,
    )
    .map_err(|error| format!("failed to terminally settle rejected run: {error:#}"))?;
    if !finished {
        return Err("failed to terminally settle rejected run".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit launch-failure settlement: {error}"))
}

fn dispatch_pinned_occurrence(
    conn: &Connection,
    coven_home: &Path,
    runtime: &dyn SessionRuntime,
    pinned: &PinnedOccurrence,
    owner: &str,
    now: DateTime<Utc>,
) -> Result<RunOutcome, String> {
    let run_id = fresh_id("run");
    let launch = match automation_launch(coven_home, pinned) {
        Ok(launch) => launch,
        Err(reason) => {
            settle_launch_failure(conn, pinned, &run_id, None, &reason, now)?;
            return Ok(RunOutcome {
                run_id,
                status: "failed".to_string(),
                session_id: None,
                error: Some(reason),
            });
        }
    };
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let record =
        crate::session_launch::new_session_record(crate::session_launch::NewSessionParams {
            id: launch.id.clone(),
            project_root: launch.project_root.clone(),
            harness: launch.harness.clone(),
            title: launch.title.clone(),
            status: "running".to_string(),
            now: now_iso,
            conversation_id: None,
            familiar_id: launch.familiar_id.clone(),
            execution_binding: None,
            labels: Vec::new(),
            visibility: None,
        });
    let result = crate::api::launch_durable_session(
        coven_home,
        DurableSessionStore::Existing(conn),
        runtime,
        &launch,
        &record,
        |transaction| {
            let marked =
                mark_occurrence_running(transaction, &pinned.id, owner, &pinned.deadline_at, now)
                    .map_err(anyhow::Error::msg)?;
            if !marked {
                return Err(DurableSessionLaunchError::AlreadyDispatched);
            }
            record_run_start_pinned(
                transaction,
                PinnedRunStart {
                    run_id: &run_id,
                    automation_id: &pinned.automation_id,
                    occurrence_id: &pinned.id,
                    session_id: Some(&launch.id),
                    familiar_id: launch.familiar_id.as_deref(),
                    runtime: &launch.harness,
                    snapshot: &pinned.snapshot,
                    deadline_at: &pinned.deadline_at,
                    now,
                },
            )?;
            Ok(())
        },
    );
    match result {
        Ok(()) => Ok(RunOutcome {
            run_id,
            status: "dispatched".to_string(),
            session_id: Some(launch.id),
            error: None,
        }),
        Err(DurableSessionLaunchError::AlreadyDispatched) => Ok(RunOutcome {
            run_id: String::new(),
            status: "already_dispatched".to_string(),
            session_id: None,
            error: None,
        }),
        Err(DurableSessionLaunchError::OwnershipRetained(error)) => Ok(RunOutcome {
            run_id,
            status: "ambiguous".to_string(),
            session_id: Some(launch.id),
            error: Some(format!("{error:#}")),
        }),
        Err(error) => {
            let persisted_session = matches!(
                &error,
                DurableSessionLaunchError::Runtime(_)
                    | DurableSessionLaunchError::PersistedState(_)
            );
            let reason = durable_error_text(error);
            let session_settlement = if persisted_session {
                ensure_session_terminal_after_launch_failure(conn, &launch.id, &reason, now)
            } else {
                Ok(())
            };
            let linked_settlement = settle_launch_failure(
                conn,
                pinned,
                &run_id,
                persisted_session.then_some(launch.id.as_str()),
                &reason,
                now,
            );
            match (session_settlement, linked_settlement) {
                (Ok(()), Ok(())) => {}
                (Err(session_error), Ok(())) => return Err(session_error),
                (Ok(()), Err(linked_error)) => return Err(linked_error),
                (Err(session_error), Err(linked_error)) => {
                    return Err(format!("{session_error}; {linked_error}"));
                }
            }
            Ok(RunOutcome {
                run_id,
                status: "failed".to_string(),
                session_id: None,
                error: Some(reason),
            })
        }
    }
}

fn ensure_session_terminal_after_launch_failure(
    conn: &Connection,
    session_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let changed = crate::store::update_session_status_if_current(
        conn,
        session_id,
        "running",
        "failed",
        None,
        &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
    .map_err(|error| {
        format!("failed to terminally settle session {session_id} after `{reason}`: {error:#}")
    })?;
    if changed {
        return Ok(());
    }
    let Some(session) = crate::store::get_session(conn, session_id)
        .map_err(|error| format!("failed to verify session {session_id}: {error:#}"))?
    else {
        return Ok(());
    };
    if matches!(
        session.status.as_str(),
        "completed" | "failed" | "cancelled" | "killed" | "idle" | "orphaned"
    ) {
        Ok(())
    } else {
        Err(format!(
            "session {session_id} remained nonterminal in state `{}` after launch failure",
            session.status
        ))
    }
}

/// Runs a routine once, now: fences and claims an immediate occurrence,
/// records a ledger row, dispatches through the shared session-launch path,
/// and leaves the run in flight with a bounded lease. Settlement — terminal
/// status, exit code, bounded log, output delivery — happens through the
/// reconciliation pass once the session finishes. A missing cwd fails the
/// run with a recorded reason instead of guessing a project; a live
/// previous occurrence fails the run (overlap: forbid).
pub fn run_routine_now(
    conn: &Connection,
    coven_home: &Path,
    runtime: &dyn SessionRuntime,
    definition: &RoutineDefinition,
    now: DateTime<Utc>,
) -> Result<RunOutcome, String> {
    let expected_record = super::store::get_definition(conn, &definition.id)
        .map_err(|error| format!("failed to read routine before manual run: {error:#}"))?
        .ok_or_else(|| format!("routine `{}` vanished before manual run", definition.id))?;
    let expected_definition: RoutineDefinition =
        serde_json::from_str(&expected_record.definition_json).map_err(|error| {
            format!("stored routine `{}` is unreadable: {error}", definition.id)
        })?;
    let expected_snapshot = super::store::definition_snapshot(&expected_record)
        .map_err(|error| format!("failed to snapshot manual routine: {error:#}"))?;

    let occurrence_id = fresh_id("occ");
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'planned', 0, ?3, ?3)",
            params![occurrence_id, definition.id, now_iso],
        )
        .map_err(|error| format!("failed to fence immediate occurrence: {error}"))?;
    if inserted == 0 {
        return Err("immediate occurrence fence collided; retry".to_string());
    }

    let claimed = match claim_occurrence_by_id_at_revision(
        conn,
        &occurrence_id,
        "manual",
        expected_record.revision,
        &expected_record.definition_digest,
        now,
    ) {
        Ok(claimed) => claimed,
        Err(reason) => {
            let pinned = PinnedOccurrence {
                id: occurrence_id,
                automation_id: definition.id.clone(),
                definition: expected_definition,
                deadline_at: (now + chrono::Duration::minutes(expected_snapshot.timeout_minutes))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                snapshot: expected_snapshot,
            };
            let run_id = fresh_id("run");
            settle_launch_failure(conn, &pinned, &run_id, None, &reason, now)?;
            return Ok(RunOutcome {
                run_id,
                status: "failed".to_string(),
                session_id: None,
                error: Some(reason),
            });
        }
    };
    if claimed.is_none() {
        // The claim was refused — in practice a live sibling run appeared
        // first (overlap: forbid). Release our fence so the daemon can never
        // dispatch it later, then fail visibly.
        let deleted = conn
            .execute(
                "DELETE FROM automation_occurrences WHERE id = ?1 AND state = 'planned'",
                params![occurrence_id],
            )
            .map_err(|error| format!("failed to release refused occurrence: {error}"))?;
        if deleted != 1 {
            return Err("refused occurrence changed before it could be released".to_string());
        }
        return Ok(overlap_outcome(&expected_definition));
    }
    pause_dispatch_claim_for_test(&occurrence_id, Some(&definition.id));
    let pinned = match load_pinned_occurrence(conn, &occurrence_id)? {
        ClaimedOccurrenceLoad::Ready(pinned) => pinned,
        ClaimedOccurrenceLoad::AlreadyHandled => {
            return Ok(RunOutcome {
                run_id: String::new(),
                status: "already_dispatched".to_string(),
                session_id: None,
                error: None,
            });
        }
        ClaimedOccurrenceLoad::Malformed(reason) => return Err(reason),
    };
    dispatch_pinned_occurrence(conn, coven_home, runtime, &pinned, "manual", now)
}

/// Builds the shared SessionLaunch for a routine run. Every run — manual or
/// scheduled — dispatches through this exact launch shape.
pub fn build_session_launch(
    definition: &RoutineDefinition,
    cwd: &str,
) -> Result<SessionLaunch, String> {
    Ok(SessionLaunch {
        id: fresh_id("session"),
        project_root: cwd.to_string(),
        cwd: cwd.to_string(),
        harness: definition.runtime.clone(),
        model: definition.model.clone(),
        launch_mode: HarnessLaunchMode::NonInteractive,
        launch_policy: None,
        prompt: definition.prompt.clone(),
        title: definition.name.clone(),
        conversation: None,
        conversation_id: None,
        familiar_id: definition.familiar_id.clone(),
        caller_familiar_id: None,
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DispatchReport {
    pub dispatched: Vec<String>,
    pub ambiguous: Vec<String>,
    pub failed: Vec<String>,
}

/// Dispatches every claimed occurrence that the scheduler has fenced: builds
/// the launch, records the ledger row, and launches through the shared
/// runtime path. A dispatched run stays in flight under a bounded lease —
/// the reconciliation pass (`delivery::settle_finished_runs`) settles the
/// occurrence and the ledger once the session finishes. Claimed occurrences
/// whose routine has no cwd fail with a recorded reason instead of guessing
/// a project.
pub fn dispatch_claimed_occurrences(
    conn: &Connection,
    coven_home: &Path,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<DispatchReport, String> {
    let mut report = DispatchReport::default();

    let claimed: Vec<String> = {
        let mut statement = conn
            .prepare(
                "SELECT id FROM automation_occurrences
                 WHERE state = 'claimed' ORDER BY scheduled_for ASC",
            )
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|error| format!("failed to read claim: {error}"))?);
        }
        out
    };

    for occurrence_id in claimed {
        pause_dispatch_claim_for_test(&occurrence_id, None);
        let pinned = match load_pinned_occurrence(conn, &occurrence_id) {
            Ok(ClaimedOccurrenceLoad::Ready(pinned)) => pinned,
            Ok(ClaimedOccurrenceLoad::AlreadyHandled) => continue,
            Ok(ClaimedOccurrenceLoad::Malformed(reason)) => {
                if !fail_claimed_occurrence(conn, &occurrence_id, &reason, now)? {
                    if occurrence_state(conn, &occurrence_id)?.as_deref() != Some("claimed") {
                        continue;
                    }
                    return Err(format!(
                        "{reason}; occurrence {occurrence_id} could not be terminally failed"
                    ));
                }
                report.failed.push(format!("{occurrence_id}: {reason}"));
                continue;
            }
            Err(error) => return Err(error),
        };
        let automation_id = pinned.automation_id.clone();
        if adopt_existing_run(conn, &pinned, now)? {
            continue;
        }
        let outcome =
            dispatch_pinned_occurrence(conn, coven_home, runtime, &pinned, "daemon", now)?;
        if outcome.status == "dispatched" {
            report.dispatched.push(outcome.run_id);
        } else if outcome.status == "ambiguous" {
            report.ambiguous.push(outcome.run_id);
        } else if outcome.status != "already_dispatched" {
            report.failed.push(format!(
                "{automation_id}: {}",
                outcome.error.unwrap_or_else(|| "launch failed".to_string())
            ));
        }
    }

    Ok(report)
}

fn adopt_existing_run(
    conn: &Connection,
    pinned: &PinnedOccurrence,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let run_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_runs WHERE occurrence_id = ?1",
            params![pinned.id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to count existing occurrence runs: {error}"))?;
    if run_count > 1 {
        return Err(format!(
            "occurrence {} has contradictory duplicate run rows",
            pinned.id
        ));
    }
    let existing: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT status, session_id FROM automation_runs
             WHERE occurrence_id = ?1
             ORDER BY started_at ASC
             LIMIT 1",
            params![pinned.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to inspect existing occurrence run: {error}"))?;
    let Some((status, session_id)) = existing else {
        return Ok(false);
    };
    if status == "running" {
        if session_id.is_some() {
            let changed =
                mark_occurrence_running(conn, &pinned.id, "daemon", &pinned.deadline_at, now)?;
            if !changed {
                let state: String = conn
                    .query_row(
                        "SELECT state FROM automation_occurrences WHERE id = ?1",
                        params![pinned.id],
                        |row| row.get(0),
                    )
                    .map_err(|error| format!("failed to verify adopted occurrence: {error}"))?;
                if state != "running" {
                    return Err(format!(
                        "existing run could not adopt occurrence state `{state}`"
                    ));
                }
            }
        }
        return Ok(true);
    }
    let occurrence_status = if status == "succeeded" {
        "succeeded"
    } else {
        "failed"
    };
    if !settle_occurrence(
        conn,
        &pinned.id,
        occurrence_status,
        Some("reconciled from existing terminal run"),
        now,
    )? {
        return Err("failed to reconcile claim from its existing terminal run".to_string());
    }
    Ok(true)
}

/// Reads and validates a stored definition for dispatch.
pub fn load_definition_for_run(
    conn: &Connection,
    id: &str,
) -> Result<Option<RoutineDefinition>, String> {
    let Some(record) =
        super::store::get_definition(conn, id).map_err(|error| format!("{error:#}"))?
    else {
        return Ok(None);
    };
    let definition: RoutineDefinition = serde_json::from_str(&record.definition_json)
        .map_err(|error| format!("stored routine `{id}` is unreadable: {error}"))?;
    Ok(Some(definition))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use crate::store::initialize_store;
    use serde_json::json;
    use std::cell::Cell;
    use std::path::PathBuf;

    struct RejectingRuntime;

    impl SessionRuntime for RejectingRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            anyhow::bail!("synthetic launch failure")
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct RetainedOwnershipRuntime;

    impl SessionRuntime for RetainedOwnershipRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            Err(anyhow::Error::new(
                crate::daemon::RuntimeOwnershipRetainedError,
            ))
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct PersistenceInspectingRuntime {
        store_path: PathBuf,
        observed_durable_state: Cell<bool>,
    }

    #[derive(Default)]
    struct CountingRuntime {
        launches: Cell<usize>,
    }

    impl SessionRuntime for CountingRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            self.launches.set(self.launches.get() + 1);
            Ok(())
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl SessionRuntime for PersistenceInspectingRuntime {
        fn launch_session(&self, launch: &SessionLaunch) -> anyhow::Result<()> {
            let conn = crate::store::open_store(&self.store_path)?;
            let session_exists = crate::store::get_session(&conn, &launch.id)?.is_some();
            let (occurrence_state, lease_expires_at, run_session_id): (
                String,
                Option<String>,
                Option<String>,
            ) = conn.query_row(
                "SELECT o.state, o.lease_expires_at, r.session_id
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.automation_id = 'durable'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            self.observed_durable_state.set(
                session_exists
                    && occurrence_state == "running"
                    && lease_expires_at.is_some()
                    && run_session_id.as_deref() == Some(launch.id.as_str()),
            );
            Ok(())
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn definition(id: &str) -> RoutineDefinition {
        RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "familiarId": "charm",
            "prompt": "Do the thing."
        }))
        .unwrap()
    }

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    #[test]
    fn successful_dispatch_leaves_the_run_in_flight() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("daily");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();
        let outcome = run_routine_now(
            &conn,
            temp.path(),
            &crate::api::NoopSessionRuntime,
            &routine,
            Utc::now(),
        )
        .unwrap();
        // Launch success is not run success: the run is dispatched and the
        // settlement pass reports the terminal status from the Coven session
        // store.
        assert_eq!(outcome.status, "dispatched");
        assert!(outcome.session_id.is_some());

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert_eq!(runs[0].session_id, outcome.session_id);
        assert_eq!(runs[0].familiar_id, None);

        // A live run blocks a second one (overlap: forbid).
        let second = run_routine_now(
            &conn,
            temp.path(),
            &crate::api::NoopSessionRuntime,
            &routine,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(second.status, "failed");
        let second_error = second.error.clone().unwrap_or_default();
        assert!(second_error.contains("overlap"), "{second_error}");
        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1, "the rejected run records no ledger row");
    }

    #[test]
    fn dispatch_persists_linked_session_and_lease_before_runtime_spawn() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut durable = definition("durable");
        durable.cwd = Some(project.to_string_lossy().into_owned());
        durable.familiar_id = None;
        insert_definition(&conn, &durable).unwrap();
        let runtime = PersistenceInspectingRuntime {
            store_path: temp.path().join("store.sqlite"),
            observed_durable_state: Cell::new(false),
        };

        let outcome = run_routine_now(&conn, temp.path(), &runtime, &durable, Utc::now()).unwrap();

        assert_eq!(outcome.status, "dispatched");
        assert!(
            runtime.observed_durable_state.get(),
            "runtime spawn must observe the committed session/run/occurrence lease"
        );
    }

    #[test]
    fn generated_run_and_session_ids_use_uuid_entropy() {
        for prefix in ["run", "session", "occ"] {
            let id = fresh_id(prefix);
            let suffix = id
                .strip_prefix(&format!("{prefix}-"))
                .expect("id keeps its readable prefix");
            uuid::Uuid::parse_str(suffix).expect("id suffix is a UUID");
        }
    }

    #[test]
    fn build_session_launch_carries_familiar_and_runtime() {
        let launch = build_session_launch(&definition("daily"), "/work/project").unwrap();
        assert_eq!(launch.familiar_id.as_deref(), Some("charm"));
        assert_eq!(launch.harness, "coven-code");
        assert_eq!(launch.launch_mode, HarnessLaunchMode::NonInteractive);
        assert_eq!(launch.project_root, "/work/project");
    }

    #[test]
    fn failed_launch_records_a_failed_run() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("daily");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();
        let outcome =
            run_routine_now(&conn, temp.path(), &RejectingRuntime, &routine, Utc::now()).unwrap();
        assert_eq!(outcome.status, "failed");
        assert!(outcome.error.as_deref().unwrap().contains("synthetic"));

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "failed");
        let session =
            crate::store::get_session(&conn, outcome.session_id.as_deref().unwrap_or_default())
                .unwrap();
        assert!(
            session.is_none(),
            "failed outcomes do not expose a live session id"
        );
        let persisted_session: String = conn
            .query_row(
                "SELECT session_id FROM automation_runs WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            crate::store::get_session(&conn, &persisted_session)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn retained_runtime_ownership_keeps_automation_state_live_for_reconciliation() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("retained");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();

        let outcome = run_routine_now(
            &conn,
            temp.path(),
            &RetainedOwnershipRuntime,
            &routine,
            Utc::now(),
        )
        .unwrap();

        assert_eq!(outcome.status, "ambiguous");
        let session_id = outcome.session_id.as_deref().expect("retained session id");
        let states: (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, s.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN sessions AS s ON s.id = r.session_id
                 WHERE o.automation_id = 'retained'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "running".to_string(),
                "running".to_string(),
                "running".to_string()
            )
        );
        assert_eq!(
            crate::store::get_session(&conn, session_id)
                .unwrap()
                .unwrap()
                .status,
            "running"
        );
    }

    #[test]
    fn unknown_familiar_fails_before_runtime_and_settles_the_ledger() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("unknown-familiar");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = Some("ghost".to_string());
        insert_definition(&conn, &routine).unwrap();

        let outcome = run_routine_now(
            &conn,
            temp.path(),
            &crate::api::NoopSessionRuntime,
            &routine,
            Utc::now(),
        )
        .unwrap();

        assert_eq!(outcome.status, "failed");
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unknown familiar")),
            "{outcome:?}"
        );
        assert!(crate::store::list_sessions(&conn).unwrap().is_empty());
        let runs = super::super::runs::list_runs(&conn, "unknown-familiar", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "failed");
        let occurrence_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences
                 WHERE automation_id = 'unknown-familiar'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_state, "failed");
    }

    #[test]
    fn persistence_failure_prevents_spawn_and_is_returned() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("persistence-failure");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_automation_run_insert
             BEFORE INSERT ON automation_runs
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic run persistence failure');
             END;",
        )
        .unwrap();
        let runtime = CountingRuntime::default();

        let error = run_routine_now(&conn, temp.path(), &runtime, &routine, Utc::now())
            .expect_err("persistence failure must be surfaced");

        assert!(
            error.contains("synthetic run persistence failure"),
            "{error}"
        );
        assert_eq!(runtime.launches.get(), 0, "runtime must not spawn");
        let (state, failure): (String, Option<String>) = conn
            .query_row(
                "SELECT state, failure_reason FROM automation_occurrences
                 WHERE automation_id = 'persistence-failure'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "failed");
        assert!(failure
            .as_deref()
            .is_some_and(|reason| reason.contains("synthetic run persistence failure")));
        assert_eq!(
            super::super::runs::list_runs(&conn, "persistence-failure", 10)
                .unwrap()
                .len(),
            0,
            "an impossible run insert must not be retried forever"
        );
        assert!(crate::store::list_sessions(&conn).unwrap().is_empty());
    }

    #[test]
    fn session_insert_failure_still_terminally_settles_occurrence_and_run() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("session-persistence-failure");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_automation_session_insert
             BEFORE INSERT ON sessions
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic session persistence failure');
             END;",
        )
        .unwrap();
        let runtime = CountingRuntime::default();

        let outcome = run_routine_now(&conn, temp.path(), &runtime, &routine, Utc::now()).unwrap();

        assert_eq!(outcome.status, "failed");
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("synthetic session persistence failure")));
        assert_eq!(runtime.launches.get(), 0);
        let (occurrence, run): (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.automation_id = 'session-persistence-failure'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((occurrence.as_str(), run.as_str()), ("failed", "failed"));
        assert!(crate::store::list_sessions(&conn).unwrap().is_empty());
    }

    #[test]
    fn claimed_occurrence_without_proven_snapshot_fails_immediately() {
        let (temp, conn) = temp_store();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("unproven-claim");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, created_at, updated_at)
             VALUES ('unproven-occ', 'unproven-claim', ?1, 'claimed', 'old-daemon',
                     '2099-01-01T00:00:00.000Z', 1, ?1, ?1)",
            params![now],
        )
        .unwrap();
        let runtime = CountingRuntime::default();

        let report =
            dispatch_claimed_occurrences(&conn, temp.path(), &runtime, Utc::now()).unwrap();

        assert_eq!(runtime.launches.get(), 0);
        assert_eq!(report.failed.len(), 1);
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'unproven-occ'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn daemon_dispatch_skips_stale_claim_after_manual_dispatch_wins() {
        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("store.sqlite");
        initialize_store(&store_path).unwrap();
        let conn = crate::store::open_store(&store_path).unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("dispatch-race");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('dispatch-race-occ', 'dispatch-race', ?1, 'planned', 0, ?1, ?1)",
            params![now_iso],
        )
        .unwrap();
        super::super::occurrences::claim_occurrence_by_id(
            &conn,
            "dispatch-race-occ",
            "daemon",
            30,
            now,
        )
        .unwrap()
        .expect("claimed occurrence");
        drop(conn);

        let (reached_tx, reached_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        install_dispatch_claim_hook(DispatchClaimHook {
            occurrence_id: Some("dispatch-race-occ".to_string()),
            automation_id: None,
            reached: reached_tx,
            release: release_rx,
        });
        let loser_path = store_path.clone();
        let loser_home = temp.path().to_path_buf();
        let loser = std::thread::spawn(move || {
            let conn = crate::store::open_store(&loser_path).unwrap();
            dispatch_claimed_occurrences(&conn, &loser_home, &crate::api::NoopSessionRuntime, now)
        });
        assert_eq!(reached_rx.recv().unwrap(), "dispatch-race-occ");

        let winner_conn = crate::store::open_store(&store_path).unwrap();
        let pinned = match load_pinned_occurrence(&winner_conn, "dispatch-race-occ").unwrap() {
            ClaimedOccurrenceLoad::Ready(pinned) => pinned,
            _ => panic!("manual winner must observe the claimed occurrence"),
        };
        let winner = dispatch_pinned_occurrence(
            &winner_conn,
            temp.path(),
            &crate::api::NoopSessionRuntime,
            &pinned,
            "manual",
            now,
        )
        .unwrap();
        assert_eq!(winner.status, "dispatched");
        release_tx.send(()).unwrap();
        let loser = loser.join().unwrap().unwrap();

        assert!(loser.dispatched.is_empty());
        assert!(loser.ambiguous.is_empty());
        assert!(loser.failed.is_empty());
        let state: String = winner_conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'dispatch-race-occ'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
        let run_count: i64 = winner_conn
            .query_row(
                "SELECT COUNT(*) FROM automation_runs WHERE occurrence_id = 'dispatch-race-occ'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_count, 1);
    }

    #[test]
    fn manual_dispatch_treats_daemon_winner_as_already_handled() {
        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("store.sqlite");
        initialize_store(&store_path).unwrap();
        let conn = crate::store::open_store(&store_path).unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut routine = definition("manual-dispatch-race");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        routine.familiar_id = None;
        insert_definition(&conn, &routine).unwrap();
        drop(conn);

        let (reached_tx, reached_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        install_dispatch_claim_hook(DispatchClaimHook {
            occurrence_id: None,
            automation_id: Some("manual-dispatch-race".to_string()),
            reached: reached_tx,
            release: release_rx,
        });
        let manual_path = store_path.clone();
        let manual_home = temp.path().to_path_buf();
        let manual_routine = routine.clone();
        let now = Utc::now();
        let manual = std::thread::spawn(move || {
            let conn = crate::store::open_store(&manual_path).unwrap();
            run_routine_now(
                &conn,
                &manual_home,
                &crate::api::NoopSessionRuntime,
                &manual_routine,
                now,
            )
        });
        let occurrence_id = reached_rx.recv().unwrap();

        let daemon_conn = crate::store::open_store(&store_path).unwrap();
        let daemon = dispatch_claimed_occurrences(
            &daemon_conn,
            temp.path(),
            &crate::api::NoopSessionRuntime,
            now,
        )
        .unwrap();
        assert_eq!(daemon.dispatched.len(), 1);
        release_tx.send(()).unwrap();
        let manual = manual
            .join()
            .unwrap()
            .expect("manual loser should treat the daemon winner as already handled");

        assert_eq!(manual.status, "already_dispatched");
        let state: String = daemon_conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                params![occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
        let run_count: i64 = daemon_conn
            .query_row(
                "SELECT COUNT(*) FROM automation_runs WHERE occurrence_id = ?1",
                params![occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_count, 1);
    }

    #[test]
    fn missing_cwd_fails_without_launching() {
        let (temp, conn) = temp_store();
        let mut definition = definition("nocwd");
        definition.cwd = None;
        insert_definition(&conn, &definition).unwrap();

        let outcome = run_routine_now(
            &conn,
            temp.path(),
            &crate::api::NoopSessionRuntime,
            &definition,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(outcome.status, "failed");
        assert!(outcome.error.as_deref().unwrap().contains("no cwd"));
        assert!(!outcome.run_id.is_empty());
        let (occurrence_state, run_status): (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.automation_id = 'nocwd'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (occurrence_state.as_str(), run_status.as_str()),
            ("failed", "failed")
        );
        assert!(crate::store::list_sessions(&conn).unwrap().is_empty());
    }
}
