//! Routine run dispatch (coven#816).
//!
//! A run is a claimed occurrence dispatched through the shared session-launch
//! seam. The occurrence, run, and session correlation is persisted before
//! runtime ownership begins; terminal settlement happens only after the
//! session ledger contains completion evidence.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use super::definition::RoutineDefinition;
use super::occurrences::{insert_claimed_occurrence, mark_occurrence_running, settle_occurrence};
use super::runs::{record_run_finish, record_run_start, RunFinish, RunStart};
use crate::api::{SessionLaunch, SessionRuntime};
use crate::harness::HarnessLaunchMode;

pub(crate) fn containment_receipt_path(coven_home: &Path, session_id: &str) -> PathBuf {
    coven_home
        .join("runtime")
        .join("automation-containment")
        .join(format!("{session_id}.receipt"))
}

pub(crate) fn recover_restart_containment(
    coven_home: &Path,
    conn: &Connection,
    now: DateTime<Utc>,
    startup: bool,
) -> Result<usize, String> {
    let candidates: Vec<String> = {
        let mut statement = conn
            .prepare(
                "SELECT r.session_id
                 FROM automation_runs AS r
                 JOIN sessions AS s ON s.id = r.session_id
                 WHERE r.status = 'running'
                   AND s.status IN ('created', 'orphaned')",
            )
            .map_err(|error| format!("failed to prepare containment recovery query: {error}"))?;
        let candidates = statement
            .query_map([], |row| row.get(0))
            .map_err(|error| format!("failed to query containment recovery candidates: {error}"))?
            .collect::<Result<_, _>>()
            .map_err(|error| format!("failed to read containment recovery candidate: {error}"))?;
        candidates
    };
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut recovered = 0;
    for session_id in candidates {
        let path = containment_receipt_path(coven_home, &session_id);
        let disposition_proven = if cfg!(windows) {
            startup
        } else {
            match std::fs::read(&path) {
                Ok(receipt) => {
                    receipt == crate::pty_runner::CONTAINMENT_QUIESCENT_RECEIPT
                        || receipt == crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => startup,
                Err(error) => {
                    return Err(format!(
                        "failed to read containment receipt `{}`: {error}",
                        path.display()
                    ));
                }
            }
        };
        if !disposition_proven {
            continue;
        }
        if crate::store::update_session_terminal_if_active(
            conn,
            &session_id,
            "killed",
            None,
            &now_iso,
        )
        .map_err(|error| {
            format!("failed to record restart containment for session `{session_id}`: {error}")
        })? {
            recovered += 1;
        }
    }
    Ok(recovered)
}

pub(crate) fn cleanup_terminal_containment_receipts(
    coven_home: &Path,
    conn: &Connection,
) -> Result<usize, String> {
    let directory = coven_home.join("runtime").join("automation-containment");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(0);
        }
        Err(error) => {
            return Err(format!(
                "failed to list containment receipts in `{}`: {error}",
                directory.display()
            ));
        }
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read containment receipt entry in `{}`: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("receipt") {
            continue;
        }
        let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let terminal = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sessions
                    WHERE id = ?1
                      AND status IN ('completed', 'failed', 'cancelled', 'killed', 'idle')
                 )",
                [session_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| {
                format!("failed to inspect session `{session_id}` for receipt cleanup: {error}")
            })?;
        if terminal {
            std::fs::remove_file(&path).map_err(|error| {
                format!(
                    "failed to remove terminal containment receipt `{}`: {error}",
                    path.display()
                )
            })?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub run_id: String,
    pub status: String,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

fn fresh_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn persist_launch(
    conn: &Connection,
    run_id: &str,
    occurrence_id: &str,
    definition: &RoutineDefinition,
    launch: &SessionLaunch,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin durable automation launch: {error}"))?;
    let overlapping_run: bool = transaction
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM automation_runs
                 WHERE automation_id = ?1 AND status = 'running'
             )",
            rusqlite::params![definition.id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to enforce automation overlap policy: {error}"))?;
    if overlapping_run {
        return Err("routine already has a nonterminal run; overlap is forbidden".to_string());
    }
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let session =
        crate::session_launch::new_session_record(crate::session_launch::NewSessionParams {
            id: launch.id.clone(),
            project_root: launch.project_root.clone(),
            harness: launch.harness.clone(),
            title: launch.title.clone(),
            status: "created".to_string(),
            now: now_iso,
            conversation_id: None,
            familiar_id: launch.familiar_id.clone(),
            execution_binding: None,
            labels: Vec::new(),
            visibility: None,
        });
    crate::store::insert_session(&transaction, &session)
        .map_err(|error| format!("failed to persist automation session: {error:#}"))?;
    record_run_start(
        &transaction,
        run_id,
        RunStart {
            automation_id: &definition.id,
            occurrence_id: Some(occurrence_id),
            session_id: Some(&launch.id),
            familiar_id: definition.familiar_id.as_deref(),
            runtime: &definition.runtime,
            timeout_at: now + chrono::Duration::minutes(i64::from(definition.timeout_minutes)),
        },
        now,
    )
    .map_err(|error| format!("failed to record run start: {error:#}"))?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit durable automation launch: {error}"))
}

fn publish_runtime_ownership(
    conn: &Connection,
    occurrence_id: &str,
    session_id: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let transaction = conn.unchecked_transaction()?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let ownership_published = crate::store::update_session_status_if_current(
        &transaction,
        session_id,
        "created",
        "running",
        None,
        &now_iso,
    )?;
    if !ownership_published {
        let status = crate::store::get_session(&transaction, session_id)?
            .map(|session| session.status)
            .ok_or_else(|| {
                anyhow::anyhow!("automation session vanished before ownership publication")
            })?;
        if !matches!(
            status.as_str(),
            "running" | "completed" | "failed" | "cancelled" | "killed" | "idle" | "orphaned"
        ) {
            anyhow::bail!(
                "automation session changed to `{status}` before runtime ownership was published"
            );
        }
    }
    if !mark_occurrence_running(&transaction, occurrence_id, now).map_err(anyhow::Error::msg)? {
        let state: Option<String> = transaction
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                rusqlite::params![occurrence_id],
                |row| row.get(0),
            )
            .optional()?;
        if !state
            .as_deref()
            .is_some_and(|state| matches!(state, "running" | "succeeded" | "failed"))
        {
            anyhow::bail!("automation occurrence changed before runtime ownership was published");
        }
    }
    transaction.commit()?;
    Ok(())
}

fn settle_rejected_launch(
    conn: &Connection,
    occurrence_id: &str,
    run_id: &str,
    session_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin launch rejection settlement: {error}"))?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    crate::store::update_session_terminal_if_active(
        &transaction,
        session_id,
        "failed",
        None,
        &now_iso,
    )
    .map_err(|error| format!("failed to settle rejected session: {error:#}"))?;
    if !settle_occurrence(&transaction, occurrence_id, "failed", Some(reason), now)? {
        return Err("failed to settle rejected occurrence".to_string());
    }
    if !record_run_finish(
        &transaction,
        run_id,
        RunFinish {
            status: "failed",
            exit_code: None,
            session_id: Some(session_id.to_string()),
            log_json: None,
            output_commit: None,
        },
        now,
    )
    .map_err(|error| format!("failed to settle rejected run: {error:#}"))?
    {
        return Err("failed to settle rejected run".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit launch rejection settlement: {error}"))
}

fn dispatch_occurrence(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    definition: &RoutineDefinition,
    occurrence_id: &str,
    cwd: &str,
    now: DateTime<Utc>,
) -> Result<RunOutcome, String> {
    let run_id = fresh_id("run");
    let launch = build_session_launch(definition, cwd)?;
    persist_launch(conn, &run_id, occurrence_id, definition, &launch, now)?;

    let ownership_published = Cell::new(false);
    let ownership_publication_error = RefCell::new(None);
    let mut ownership_established =
        || match publish_runtime_ownership(conn, occurrence_id, &launch.id, now) {
            Ok(()) => {
                ownership_published.set(true);
                Ok(())
            }
            Err(error) => {
                *ownership_publication_error.borrow_mut() = Some(format!("{error:#}"));
                Err(anyhow::Error::new(
                    crate::api::RuntimeOwnershipPublicationError,
                ))
            }
        };
    match runtime.launch_contained_adopted_session(&launch, None, &mut ownership_established) {
        Ok(()) if ownership_published.get() => Ok(RunOutcome {
            run_id,
            status: "running".to_string(),
            session_id: Some(launch.id),
            error: None,
        }),
        result if ownership_published.get() => {
            let error = match result {
                Ok(()) => None,
                Err(error) => Some(format!(
                    "runtime ownership was established but launch acknowledgement failed: {error:#}"
                )),
            };
            Ok(RunOutcome {
                run_id,
                status: "running".to_string(),
                session_id: Some(launch.id),
                error,
            })
        }
        Ok(()) => {
            let publication_error =
                publish_runtime_ownership(conn, occurrence_id, &launch.id, now).err();
            Ok(RunOutcome {
                run_id,
                status: "running".to_string(),
                session_id: Some(launch.id),
                error: publication_error.map(|error| {
                    format!(
                        "runtime accepted launch without publishing ownership; completion is ambiguous: {error:#}"
                    )
                }),
            })
        }
        Err(error)
            if error
                .downcast_ref::<crate::daemon::RuntimeOwnershipRetainedError>()
                .is_some()
                || error
                    .downcast_ref::<crate::api::RuntimeOwnershipPublicationError>()
                    .is_some() =>
        {
            let publication_error =
                publish_runtime_ownership(conn, occurrence_id, &launch.id, now).err();
            let callback_error = ownership_publication_error.borrow().clone();
            let error = match (callback_error, publication_error) {
                (Some(callback_error), Some(retry_error)) => {
                    format!("{error:#}: {callback_error}; retry failed: {retry_error:#}")
                }
                (Some(callback_error), None) => format!("{error:#}: {callback_error}"),
                (None, Some(retry_error)) => format!(
                    "{error:#}; failed to publish retained runtime ownership: {retry_error:#}"
                ),
                (None, None) => format!("{error:#}"),
            };
            Ok(RunOutcome {
                run_id,
                status: "running".to_string(),
                session_id: Some(launch.id),
                error: Some(error),
            })
        }
        Err(error) => {
            let reason = format!("{error:#}");
            settle_rejected_launch(conn, occurrence_id, &run_id, &launch.id, &reason, now)?;
            Ok(RunOutcome {
                run_id,
                status: "failed".to_string(),
                session_id: None,
                error: Some(reason),
            })
        }
    }
}

/// Runs a routine once, now: fences and claims an immediate occurrence,
/// durably links its session, and dispatches through the shared session-launch
/// path. A launch acknowledgement leaves the run in flight; a later
/// reconciliation pass settles terminal session evidence. A missing cwd fails
/// without guessing a project.
pub fn run_routine_now(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    definition: &RoutineDefinition,
    now: DateTime<Utc>,
) -> Result<RunOutcome, String> {
    let Some(cwd) = definition
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
    else {
        return Ok(RunOutcome {
            run_id: String::new(),
            status: "failed".to_string(),
            session_id: None,
            error: Some("routine has no cwd; add a cwd before running".to_string()),
        });
    };

    let occurrence_id = fresh_id("occ");
    if !insert_claimed_occurrence(conn, &occurrence_id, &definition.id, "manual", 60, now)? {
        return Ok(RunOutcome {
            run_id: String::new(),
            status: "failed".to_string(),
            session_id: None,
            error: Some("routine already has a nonterminal run; overlap is forbidden".to_string()),
        });
    }

    dispatch_occurrence(conn, runtime, definition, &occurrence_id, cwd, now)
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
    pub failed: Vec<String>,
}

/// Dispatches every claimed occurrence through the same durable launch
/// primitive as manual runs. Successful launch acknowledgements remain
/// nonterminal until session evidence is reconciled.
pub fn dispatch_claimed_occurrences(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    _now: DateTime<Utc>,
) -> Result<DispatchReport, String> {
    let mut report = DispatchReport::default();

    let claimed: Vec<(String, String)> = {
        let mut statement = conn
            .prepare(
                "SELECT o.id, o.automation_id FROM automation_occurrences AS o
                 WHERE o.state = 'claimed'
                   AND o.lease_owner = 'daemon'
                   AND NOT EXISTS (
                       SELECT 1 FROM automation_runs AS r WHERE r.occurrence_id = o.id
                   )
                 ORDER BY o.scheduled_for ASC",
            )
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|error| format!("failed to read claim: {error}"))?);
        }
        out
    };

    for (occurrence_id, automation_id) in claimed {
        let now = Utc::now();
        let Some(definition) = load_definition_for_run(conn, &automation_id)? else {
            let reason = format!("routine `{automation_id}` vanished during dispatch");
            settle_occurrence(conn, &occurrence_id, "failed", Some(&reason), now)?;
            report.failed.push(reason);
            continue;
        };

        let Some(cwd) = definition
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
        else {
            let reason = format!("{automation_id}: routine has no cwd; add a cwd before running");
            settle_occurrence(conn, &occurrence_id, "failed", Some(&reason), now)?;
            report.failed.push(reason);
            continue;
        };

        match dispatch_occurrence(conn, runtime, &definition, &occurrence_id, cwd, now) {
            Ok(outcome) if outcome.status == "running" => {
                report.dispatched.push(outcome.run_id);
            }
            Ok(outcome) => {
                report.failed.push(format!(
                    "{automation_id}: {}",
                    outcome
                        .error
                        .unwrap_or_else(|| "launch did not enter running state".to_string())
                ));
            }
            Err(reason) => {
                settle_occurrence(
                    conn,
                    &occurrence_id,
                    "failed",
                    Some("dispatch failed before runtime ownership"),
                    now,
                )?;
                report.failed.push(format!("{automation_id}: {reason}"));
            }
        }
    }

    Ok(report)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SettlementReport {
    pub succeeded: usize,
    pub failed: usize,
}

/// Requests strict termination for an abandoned pre-publication launch once
/// its claim lease expires. Lease age selects work for recovery but never
/// proves process death; failed termination therefore remains nonterminal.
pub fn recover_abandoned_launches(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<Vec<String>, String> {
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let candidates: Vec<(String, String)> = {
        let mut statement = conn
            .prepare(
                "SELECT r.id, r.session_id
                 FROM automation_runs AS r
                 JOIN sessions AS s ON s.id = r.session_id
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 WHERE r.status = 'running'
                   AND s.status = 'created'
                   AND o.state = 'claimed'
                   AND o.lease_expires_at IS NOT NULL
                   AND o.lease_expires_at < ?1
                   AND (r.timeout_at IS NULL OR r.timeout_at > ?1)",
            )
            .map_err(|error| format!("failed to prepare abandoned automation recovery: {error}"))?;
        let rows = statement
            .query_map(rusqlite::params![now_iso], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|error| format!("failed to query abandoned automation launches: {error}"))?;
        let mut candidates = Vec::new();
        for row in rows {
            candidates
                .push(row.map_err(|error| format!("failed to read abandoned launch: {error}"))?);
        }
        candidates
    };

    let mut failures = Vec::new();
    for (run_id, session_id) in candidates {
        if let Err(error) = runtime.kill_session(&session_id) {
            failures.push(format!(
                "abandoned automation run `{run_id}` has unproven session `{session_id}` termination: {error:#}"
            ));
            continue;
        }
        crate::store::update_session_terminal_if_active(
            conn,
            &session_id,
            "killed",
            None,
            &now_iso,
        )
        .map_err(|error| {
            format!("failed to persist abandoned session `{session_id}` termination: {error:#}")
        })?;
    }
    Ok(failures)
}

/// Requests termination for runs that exceeded their definition's wall-clock
/// budget. A successful strict kill is persisted as terminal session evidence;
/// an unproven kill remains running and is returned for daemon diagnostics.
pub fn enforce_run_timeouts(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<Vec<String>, String> {
    let candidates: Vec<(String, String, String)> = {
        let mut statement = conn
            .prepare(
                "SELECT r.id, r.session_id, r.timeout_at, r.automation_id
                 FROM automation_runs AS r
                 JOIN sessions AS s ON s.id = r.session_id
                 WHERE r.status = 'running'
                   AND s.status IN ('created', 'running', 'orphaned')
                   AND r.timeout_at IS NOT NULL",
            )
            .map_err(|error| format!("failed to prepare automation timeout query: {error}"))?;
        let mapped = statement
            .query_map([], |row| {
                let run_id: String = row.get(0)?;
                let session_id: String = row.get(1)?;
                let timeout_at: String = row.get(2)?;
                let automation_id: String = row.get(3)?;
                Ok((run_id, session_id, timeout_at, automation_id))
            })
            .map_err(|error| format!("failed to query automation timeouts: {error}"))?;
        let mut candidates = Vec::new();
        for row in mapped {
            let (run_id, session_id, timeout_at, automation_id) =
                row.map_err(|error| format!("failed to read automation timeout row: {error}"))?;
            let timeout_at = DateTime::parse_from_rfc3339(&timeout_at)
                .map_err(|error| format!("run `{run_id}` has invalid timeout_at: {error}"))?
                .with_timezone(&Utc);
            if timeout_at <= now {
                candidates.push((run_id, session_id, automation_id));
            }
        }
        candidates
    };

    let mut failures = Vec::new();
    for (run_id, session_id, definition_name) in candidates {
        if let Err(error) = runtime.kill_session(&session_id) {
            failures.push(format!(
                "automation `{definition_name}` run `{run_id}` exceeded its timeout, but session `{session_id}` termination is unproven: {error:#}"
            ));
            continue;
        }
        let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        crate::store::update_session_terminal_if_active(
            conn,
            &session_id,
            "killed",
            None,
            &now_iso,
        )
        .map_err(|error| {
            format!("failed to persist timeout for session `{session_id}`: {error:#}")
        })?;
    }
    Ok(failures)
}

/// Reconciles nonterminal automation rows against the authoritative session
/// ledger. Only a completed session with an explicit zero exit code can
/// produce success; every other terminal session disposition is a failure.
pub fn settle_finished_runs(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<SettlementReport, String> {
    type RunningRow = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    );

    let rows: Vec<RunningRow> = {
        let mut statement = conn
            .prepare(
                "SELECT r.id, r.occurrence_id, r.session_id, s.status, s.exit_code,
                        o.state, r.timeout_at,
                        COALESCE(
                            (
                                SELECT e.created_at
                                FROM events AS e
                                WHERE e.session_id = s.id AND e.kind = 'exit'
                                ORDER BY e.created_at DESC, e.id DESC
                                LIMIT 1
                            ),
                            s.updated_at
                        )
                 FROM automation_runs AS r
                 LEFT JOIN sessions AS s ON s.id = r.session_id
                 LEFT JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 WHERE r.status = 'running'
                 ORDER BY r.started_at ASC",
            )
            .map_err(|error| format!("failed to prepare automation reconciliation: {error}"))?;
        let mapped = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            })
            .map_err(|error| format!("failed to query automation reconciliation: {error}"))?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(
                row.map_err(|error| format!("failed to read automation reconciliation: {error}"))?,
            );
        }
        rows
    };

    let mut report = SettlementReport::default();
    for (
        run_id,
        occurrence_id,
        session_id,
        session_status,
        exit_code,
        occurrence_state,
        timeout_at,
        terminal_at,
    ) in rows
    {
        let terminal_session = session_status.as_deref().is_some_and(|status| {
            matches!(
                status,
                "completed" | "failed" | "cancelled" | "killed" | "idle"
            )
        });
        if !terminal_session {
            continue;
        }

        let completed_after_timeout = match (timeout_at.as_deref(), terminal_at.as_deref()) {
            (Some(timeout_at), Some(terminal_at)) => {
                let timeout_at = DateTime::parse_from_rfc3339(timeout_at)
                    .map_err(|error| format!("run `{run_id}` has invalid timeout_at: {error}"))?;
                let terminal_at = DateTime::parse_from_rfc3339(terminal_at).map_err(|error| {
                    format!("session for run `{run_id}` has invalid terminal timestamp: {error}")
                })?;
                terminal_at > timeout_at
            }
            _ => false,
        };
        let succeeded = session_status.as_deref() == Some("completed")
            && exit_code == Some(0)
            && !completed_after_timeout;
        let status = if succeeded { "succeeded" } else { "failed" };
        let reason = if succeeded {
            None
        } else if completed_after_timeout {
            Some("session completed after automation timeout".to_string())
        } else {
            Some(match (&session_status, exit_code) {
                (Some(session_status), Some(exit_code)) => {
                    format!("session {session_status} (exit code {exit_code})")
                }
                (Some(session_status), None) => format!("session {session_status}"),
                (None, _) => unreachable!("terminal_session requires a session status"),
            })
        };

        let transaction = conn
            .unchecked_transaction()
            .map_err(|error| format!("failed to begin automation settlement: {error}"))?;
        let Some(occurrence_id) = occurrence_id.as_deref() else {
            return Err(format!(
                "running automation run `{run_id}` has no occurrence"
            ));
        };
        if occurrence_state.as_deref() != Some(status) {
            if matches!(occurrence_state.as_deref(), Some("succeeded" | "failed")) {
                return Err(format!(
                    "automation occurrence `{occurrence_id}` is already `{}` but terminal session evidence requires `{status}`",
                    occurrence_state.as_deref().unwrap_or("missing")
                ));
            }
            if !settle_occurrence(&transaction, occurrence_id, status, reason.as_deref(), now)? {
                return Err(format!(
                    "automation occurrence `{occurrence_id}` changed during settlement"
                ));
            }
        }
        if !record_run_finish(
            &transaction,
            &run_id,
            RunFinish {
                status,
                exit_code,
                session_id,
                log_json: None,
                output_commit: None,
            },
            now,
        )
        .map_err(|error| format!("failed to settle automation run `{run_id}`: {error:#}"))?
        {
            return Err(format!(
                "automation run `{run_id}` changed during settlement"
            ));
        }
        transaction
            .commit()
            .map_err(|error| format!("failed to commit automation settlement: {error}"))?;
        if succeeded {
            report.succeeded += 1;
        } else {
            report.failed += 1;
        }
    }

    Ok(report)
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

    struct ContainedRuntime;

    impl SessionRuntime for ContainedRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch must use strict containment")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            ownership_established()
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

    struct DelayedContainedRuntime {
        launches: std::sync::atomic::AtomicUsize,
    }

    impl SessionRuntime for DelayedContainedRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch must use strict containment")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            ownership_established()?;
            if self
                .launches
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                == 0
            {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
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

    struct OwnershipThenFailureRuntime;

    impl SessionRuntime for OwnershipThenFailureRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("launch_adopted_session is overridden")
        }

        fn launch_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            ownership_established()?;
            anyhow::bail!("synthetic failure after ownership")
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

    struct RetainedOwnershipWithoutCallbackRuntime;

    impl SessionRuntime for RetainedOwnershipWithoutCallbackRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("launch_adopted_session is overridden")
        }

        fn launch_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            _ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            Err(crate::daemon::RuntimeOwnershipRetainedError.into())
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

    struct TerminalBeforeOwnershipRuntime<'a> {
        conn: &'a Connection,
    }

    impl SessionRuntime for TerminalBeforeOwnershipRuntime<'_> {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("launch_adopted_session is overridden")
        }

        fn launch_adopted_session(
            &self,
            launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            self.conn.execute(
                "UPDATE sessions SET status = 'completed', exit_code = 0 WHERE id = ?1",
                rusqlite::params![launch.id],
            )?;
            ownership_established()
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

    struct FailedKillRuntime;

    impl SessionRuntime for FailedKillRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("timeout test does not launch")
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            anyhow::bail!("synthetic unproven termination")
        }
    }

    struct RetainedPublicationErrorRuntime;

    impl SessionRuntime for RetainedPublicationErrorRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("launch_adopted_session is overridden")
        }

        fn launch_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            _ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            Err(anyhow::Error::new(
                crate::api::RuntimeOwnershipPublicationError,
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
    fn accepted_launch_keeps_occurrence_and_run_running() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let outcome =
            run_routine_now(&conn, &ContainedRuntime, &definition("daily"), launched_at).unwrap();
        assert_eq!(outcome.status, "running");
        assert!(outcome.session_id.is_some());

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert_eq!(runs[0].session_id, outcome.session_id);
        assert_eq!(runs[0].exit_code, None);
        assert_eq!(runs[0].finished_at, None);
        assert_eq!(runs[0].familiar_id.as_deref(), Some("charm"));

        let session =
            crate::store::get_session(&conn, outcome.session_id.as_deref().unwrap()).unwrap();
        assert_eq!(
            session.as_ref().map(|row| row.status.as_str()),
            Some("running")
        );

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }

    #[test]
    fn completed_session_evidence_settles_run_successfully() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            launched_at,
        )
        .unwrap();
        let session_id = outcome.session_id.as_deref().unwrap();
        let finished_at = launched_at + chrono::Duration::seconds(5);
        crate::store::update_session_terminal_if_active(
            &conn,
            session_id,
            "completed",
            Some(0),
            &finished_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap();

        let report = settle_finished_runs(&conn, finished_at).unwrap();
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 0);

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "succeeded");
        assert_eq!(runs[0].exit_code, Some(0));
        assert!(runs[0].finished_at.is_some());

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "succeeded");
    }

    #[test]
    fn orphaned_session_is_not_terminal_automation_evidence() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            launched_at,
        )
        .unwrap();
        conn.execute(
            "UPDATE sessions SET status = 'orphaned' WHERE id = ?1",
            rusqlite::params![outcome.session_id.as_deref().unwrap()],
        )
        .unwrap();

        assert_eq!(
            settle_finished_runs(&conn, launched_at + chrono::Duration::seconds(1)).unwrap(),
            SettlementReport::default()
        );
        assert_eq!(
            super::super::runs::list_runs(&conn, "daily", 10).unwrap()[0].status,
            "running"
        );
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }

    #[cfg(unix)]
    #[test]
    fn durable_containment_receipt_recovers_an_orphaned_run() {
        let (temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            launched_at,
        )
        .unwrap();
        let session_id = outcome.session_id.as_deref().unwrap();
        conn.execute(
            "UPDATE sessions SET status = 'orphaned' WHERE id = ?1",
            rusqlite::params![session_id],
        )
        .unwrap();
        let receipt = containment_receipt_path(temp.path(), session_id);
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        std::fs::write(&receipt, crate::pty_runner::CONTAINMENT_QUIESCENT_RECEIPT).unwrap();

        let recovered = recover_restart_containment(
            temp.path(),
            &conn,
            launched_at + chrono::Duration::seconds(1),
            false,
        )
        .unwrap();
        assert_eq!(recovered, 1);
        let report =
            settle_finished_runs(&conn, launched_at + chrono::Duration::seconds(1)).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(
            super::super::runs::list_runs(&conn, "daily", 10).unwrap()[0].status,
            "failed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn empty_containment_receipt_preserves_unknown_disposition() {
        let (temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            launched_at,
        )
        .unwrap();
        let session_id = outcome.session_id.as_deref().unwrap();
        conn.execute(
            "UPDATE sessions SET status = 'orphaned' WHERE id = ?1",
            rusqlite::params![session_id],
        )
        .unwrap();
        let receipt = containment_receipt_path(temp.path(), session_id);
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        std::fs::write(receipt, b"").unwrap();

        assert_eq!(
            recover_restart_containment(temp.path(), &conn, launched_at, true).unwrap(),
            0
        );
        assert_eq!(
            super::super::runs::list_runs(&conn, "daily", 10).unwrap()[0].status,
            "running"
        );
    }

    #[test]
    fn receipt_cleanup_removes_only_terminal_session_evidence() {
        let (temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let terminal = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            launched_at,
        )
        .unwrap();
        let terminal_id = terminal.session_id.as_deref().unwrap();
        crate::store::update_session_terminal_if_active(
            &conn,
            terminal_id,
            "completed",
            Some(0),
            &launched_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap();
        let terminal_receipt = containment_receipt_path(temp.path(), terminal_id);
        std::fs::create_dir_all(terminal_receipt.parent().unwrap()).unwrap();
        std::fs::write(
            &terminal_receipt,
            crate::pty_runner::CONTAINMENT_QUIESCENT_RECEIPT,
        )
        .unwrap();
        let active_receipt = containment_receipt_path(temp.path(), "active-session");
        std::fs::write(&active_receipt, b"").unwrap();

        assert_eq!(
            cleanup_terminal_containment_receipts(temp.path(), &conn).unwrap(),
            1
        );
        assert!(!terminal_receipt.exists());
        assert!(active_receipt.exists());
    }

    #[test]
    fn completion_after_immutable_deadline_settles_as_failed() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 1;
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        let completed_at =
            launched_at + chrono::Duration::minutes(1) + chrono::Duration::milliseconds(1);
        crate::store::update_session_terminal_if_active(
            &conn,
            outcome.session_id.as_deref().unwrap(),
            "completed",
            Some(0),
            &completed_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap();

        let report = settle_finished_runs(&conn, completed_at).unwrap();
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failed, 1);
        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "failed");
        let reason: String = conn
            .query_row(
                "SELECT failure_reason FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reason, "session completed after automation timeout");
    }

    #[test]
    fn expired_claim_lease_does_not_preempt_later_session_success() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 120;
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_occurrences
             SET lease_expires_at = '2020-01-01T00:00:00.000Z'
             WHERE automation_id = 'daily'",
            [],
        )
        .unwrap();

        let after_lease = launched_at + chrono::Duration::minutes(61);
        assert_eq!(
            super::super::occurrences::recover_expired_leases(&conn, after_lease).unwrap(),
            0
        );
        assert_eq!(
            settle_finished_runs(&conn, after_lease).unwrap(),
            SettlementReport::default()
        );

        let session_id = outcome.session_id.as_deref().unwrap();
        let finished_at = after_lease + chrono::Duration::seconds(5);
        crate::store::update_session_terminal_if_active(
            &conn,
            session_id,
            "completed",
            Some(0),
            &finished_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap();
        let report = settle_finished_runs(&conn, finished_at).unwrap();
        assert_eq!(report.succeeded, 1);
        assert_eq!(
            super::super::runs::list_runs(&conn, "daily", 10).unwrap()[0].status,
            "succeeded"
        );
    }

    #[test]
    fn failed_session_evidence_settles_run_as_failed() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            launched_at,
        )
        .unwrap();
        let session_id = outcome.session_id.as_deref().unwrap();
        let finished_at = launched_at + chrono::Duration::seconds(5);
        crate::store::update_session_terminal_if_active(
            &conn,
            session_id,
            "failed",
            Some(17),
            &finished_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap();

        let report = settle_finished_runs(&conn, finished_at).unwrap();
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failed, 1);

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "failed");
        assert_eq!(runs[0].exit_code, Some(17));

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
    fn failed_launch_records_a_failed_run() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let outcome =
            run_routine_now(&conn, &RejectingRuntime, &definition("daily"), Utc::now()).unwrap();
        assert_eq!(outcome.status, "failed");
        assert!(outcome.error.as_deref().unwrap().contains("synthetic"));

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "failed");

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
    fn failure_after_runtime_ownership_remains_nonterminal() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let outcome = run_routine_now(
            &conn,
            &OwnershipThenFailureRuntime,
            &definition("daily"),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(outcome.status, "running");
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("after ownership")));

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "running");
        assert!(runs[0].finished_at.is_none());

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }

    #[test]
    fn retained_runtime_ownership_without_callback_remains_nonterminal() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let outcome = run_routine_now(
            &conn,
            &RetainedOwnershipWithoutCallbackRuntime,
            &definition("daily"),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(outcome.status, "running");
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("runtime ownership")));

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "running");
        assert!(runs[0].finished_at.is_none());
        let session = crate::store::get_session(
            &conn,
            runs[0].session_id.as_deref().expect("linked session"),
        )
        .unwrap()
        .expect("persisted session");
        assert_eq!(session.status, "running");

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }

    #[test]
    fn retained_ownership_after_publication_error_remains_nonterminal() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let outcome = run_routine_now(
            &conn,
            &RetainedPublicationErrorRuntime,
            &definition("daily"),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(outcome.status, "running");
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("could not be published")));

        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "running");
        let session = crate::store::get_session(&conn, run.session_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(session.status, "running");
    }

    #[test]
    fn terminal_session_before_ownership_publication_reconciles_successfully() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &TerminalBeforeOwnershipRuntime { conn: &conn },
            &definition("daily"),
            launched_at,
        )
        .unwrap();
        assert_eq!(outcome.status, "running");

        let report =
            settle_finished_runs(&conn, launched_at + chrono::Duration::seconds(1)).unwrap();
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 0);

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "succeeded");
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "succeeded");
    }

    #[test]
    fn manual_run_claims_the_occurrence_it_just_created() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let now = Utc::now();
        let old_scheduled_for =
            (now - chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('scheduled-earlier', 'daily', ?1, 'planned', 0, ?1, ?1)",
            rusqlite::params![old_scheduled_for],
        )
        .unwrap();

        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            now,
        )
        .unwrap();
        assert_eq!(outcome.status, "running");
        let old_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'scheduled-earlier'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_state, "planned");
    }

    #[test]
    fn daemon_dispatch_never_touches_a_manual_claim() {
        let (temp, conn) = temp_store();
        let routine = definition("daily");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "manual-occurrence",
            &routine.id,
            "manual",
            60,
            now,
        )
        .unwrap());
        let daemon_conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();

        let report =
            dispatch_claimed_occurrences(&daemon_conn, &crate::api::NoopSessionRuntime, now)
                .unwrap();
        assert!(report.dispatched.is_empty());
        assert!(report.failed.is_empty());
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'manual-occurrence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "claimed");
    }

    #[test]
    fn batch_dispatch_assigns_each_run_its_own_launch_timestamp() {
        let (_temp, conn) = temp_store();
        let first = definition("first");
        let second = definition("second");
        insert_definition(&conn, &first).unwrap();
        insert_definition(&conn, &second).unwrap();
        let now = Utc::now();
        for routine in [&first, &second] {
            assert!(insert_claimed_occurrence(
                &conn,
                &format!("{}-occurrence", routine.id),
                &routine.id,
                "daemon",
                60,
                now,
            )
            .unwrap());
        }

        let runtime = DelayedContainedRuntime {
            launches: std::sync::atomic::AtomicUsize::new(0),
        };
        let report = dispatch_claimed_occurrences(&conn, &runtime, now).unwrap();
        assert_eq!(report.dispatched.len(), 2);
        let mut statement = conn
            .prepare("SELECT started_at FROM automation_runs ORDER BY started_at ASC")
            .unwrap();
        let started: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let first_started = DateTime::parse_from_rfc3339(&started[0]).unwrap();
        let second_started = DateTime::parse_from_rfc3339(&started[1]).unwrap();
        assert!(
            second_started - first_started >= chrono::Duration::milliseconds(20),
            "serial launches shared or truncated their dispatch timestamp: {started:?}"
        );
    }

    #[test]
    fn overlap_forbid_rejects_a_second_manual_run_atomically() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        let first = run_routine_now(&conn, &crate::api::NoopSessionRuntime, &routine, now).unwrap();
        assert_eq!(first.status, "running");

        let second = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            now + chrono::Duration::seconds(1),
        )
        .unwrap();
        assert_eq!(second.status, "failed");
        assert!(second
            .error
            .as_deref()
            .is_some_and(|error| error.contains("overlap is forbidden")));

        let occurrence_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_count, 1);
        assert_eq!(
            super::super::runs::list_runs(&conn, "daily", 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn expired_preownership_launch_recovers_run_and_occurrence_together() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 120;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        let launched_at = now - chrono::Duration::minutes(61);
        let occurrence_id = "abandoned-occurrence";
        let session_id = "abandoned-session";
        let run_id = "abandoned-run";
        assert!(insert_claimed_occurrence(
            &conn,
            occurrence_id,
            &routine.id,
            "daemon",
            60,
            launched_at,
        )
        .unwrap());
        let mut launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        launch.id = session_id.to_string();
        persist_launch(&conn, run_id, occurrence_id, &routine, &launch, launched_at).unwrap();

        assert_eq!(
            crate::store::mark_stale_created_sessions_failed(
                &conn,
                &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            )
            .unwrap(),
            0,
            "generic stale recovery must defer correlated automation sessions"
        );
        let failures = recover_abandoned_launches(&conn, &FailedKillRuntime, now).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(
            super::super::occurrences::recover_expired_leases(&conn, now).unwrap(),
            0,
            "lease age cannot settle a launch with unproven process disposition"
        );
        assert_eq!(
            crate::store::get_session(&conn, session_id)
                .unwrap()
                .unwrap()
                .status,
            "created"
        );
        assert!(
            recover_abandoned_launches(&conn, &crate::api::NoopSessionRuntime, now)
                .unwrap()
                .is_empty()
        );
        let report = settle_finished_runs(&conn, now).unwrap();
        assert_eq!(report.failed, 1);

        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "failed");
        let session = crate::store::get_session(&conn, session_id)
            .unwrap()
            .unwrap();
        assert_eq!(session.status, "killed");
        let occurrence_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                rusqlite::params![occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_state, "failed");
    }

    #[test]
    fn timeout_kills_session_before_settling_failure() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 1;
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        let timed_out_at = launched_at + chrono::Duration::minutes(1);

        let failures =
            enforce_run_timeouts(&conn, &crate::api::NoopSessionRuntime, timed_out_at).unwrap();
        assert!(failures.is_empty());
        let session = crate::store::get_session(
            &conn,
            outcome.session_id.as_deref().expect("linked session"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(session.status, "killed");

        let report = settle_finished_runs(&conn, timed_out_at).unwrap();
        assert_eq!(report.failed, 1);
        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "failed");
    }

    #[test]
    fn deleting_definition_does_not_disable_running_deadline() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 1;
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        assert!(super::super::store::delete_definition(&conn, "daily").unwrap());

        let timed_out_at = launched_at + chrono::Duration::minutes(1);
        assert!(
            enforce_run_timeouts(&conn, &crate::api::NoopSessionRuntime, timed_out_at)
                .unwrap()
                .is_empty()
        );
        let session = crate::store::get_session(
            &conn,
            outcome.session_id.as_deref().expect("linked session"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(session.status, "killed");
    }

    #[test]
    fn unproven_timeout_termination_remains_running() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 1;
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        let timed_out_at = launched_at + chrono::Duration::minutes(1);

        let failures = enforce_run_timeouts(&conn, &FailedKillRuntime, timed_out_at).unwrap();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("termination is unproven"));

        let session = crate::store::get_session(
            &conn,
            outcome.session_id.as_deref().expect("linked session"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(session.status, "running");
        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "running");
    }

    #[test]
    fn missing_cwd_fails_without_launching() {
        let (_temp, conn) = temp_store();
        let mut definition = definition("nocwd");
        definition.cwd = None;
        insert_definition(&conn, &definition).unwrap();

        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(outcome.status, "failed");
        assert!(outcome.error.as_deref().unwrap().contains("no cwd"));
    }
}
