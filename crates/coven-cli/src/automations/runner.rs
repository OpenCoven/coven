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
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use super::contract::authority::{
    validate_authority_profile, AuthorityConsumerClass, AuthorityEvidenceVerifier,
    AuthorityProfileDisposition, AuthorityProfileError, AuthorityProfileErrorCode,
    AuthorityValidationPhase, AutomationAuthorityExtension, AUTHORITY_PROFILE,
    RUNTIME_AUTHORITY_CAPABILITY,
};
use super::contract::types::{BackoffPolicy, ExtensionBag, RetryableClass};
use super::definition::{RoutineDefinition, RoutineRetryPolicy};
use super::occurrences::{
    insert_claimed_occurrence, mark_occurrence_running, recover_expired_leases,
    recover_expired_leases_with_scheduler_fence, settle_occurrence,
};
use super::runs::{
    is_retry_quarantined, record_retry_exhaustion, record_run_finish, record_run_start, RunFinish,
    RunStart,
};
use crate::api::{SessionLaunch, SessionRuntime};
use crate::harness::HarnessLaunchMode;

pub(crate) fn containment_receipt_path(coven_home: &Path, session_id: &str) -> PathBuf {
    coven_home
        .join("runtime")
        .join("automation-containment")
        .join(format!("{session_id}.receipt"))
}

fn receipt_proves_containment(
    receipt: Option<&[u8]>,
    previous_daemon_launch: bool,
    windows_job_guarantee: bool,
) -> bool {
    if windows_job_guarantee && previous_daemon_launch {
        return true;
    }
    match receipt {
        Some(receipt) => {
            receipt == crate::pty_runner::CONTAINMENT_QUIESCENT_RECEIPT
                || receipt == crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT
        }
        None => previous_daemon_launch,
    }
}

pub(crate) fn recover_restart_containment(
    coven_home: &Path,
    conn: &Connection,
    now: DateTime<Utc>,
    startup_cutoff: Option<DateTime<Utc>>,
) -> Result<usize, String> {
    let candidates: Vec<(String, String, String, Option<String>)> = {
        let mut statement = conn
            .prepare(
                "SELECT r.session_id, s.created_at, o.kind, o.lease_owner
                 FROM automation_runs AS r
                 JOIN sessions AS s ON s.id = r.session_id
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 WHERE r.status = 'running'
                   AND s.status IN ('created', 'orphaned')",
            )
            .map_err(|error| format!("failed to prepare containment recovery query: {error}"))?;
        let candidates = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|error| format!("failed to query containment recovery candidates: {error}"))?
            .collect::<Result<_, _>>()
            .map_err(|error| format!("failed to read containment recovery candidate: {error}"))?;
        candidates
    };
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut recovered = 0;
    for (session_id, created_at, occurrence_kind, lease_owner) in candidates {
        let created_at = DateTime::parse_from_rfc3339(&created_at)
            .map_err(|error| {
                format!(
                    "automation session `{session_id}` has invalid created_at during containment recovery: {error}"
                )
            })?
            .with_timezone(&Utc);
        let previous_daemon_launch = startup_cutoff.is_some_and(|cutoff| {
            occurrence_kind == "scheduled"
                && lease_owner.as_deref() == Some("daemon")
                && created_at.timestamp_millis() < cutoff.timestamp_millis()
        });
        let path = containment_receipt_path(coven_home, &session_id);
        let receipt = match std::fs::read(&path) {
            Ok(receipt) => Some(receipt),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "failed to read containment receipt `{}`: {error}",
                    path.display()
                ));
            }
        };
        let disposition_proven =
            receipt_proves_containment(receipt.as_deref(), previous_daemon_launch, cfg!(windows));
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
        let removable = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sessions
                    WHERE id = ?1
                      AND status IN ('completed', 'failed', 'cancelled', 'killed', 'idle')
                 ) OR NOT EXISTS(
                     SELECT 1 FROM sessions WHERE id = ?1
                 )",
                [session_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| {
                format!("failed to inspect session `{session_id}` for receipt cleanup: {error}")
            })?;
        if removable {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutomationAuthorityRequest {
    pub automation_id: String,
    pub automation_revision: u64,
    pub definition_digest: String,
    pub occurrence_id: String,
    pub occurrence_key: String,
    pub occurrence_fence_generation: u64,
    pub run_id: String,
    pub attempt_id: String,
    pub attempt_number: u64,
    pub adoption_key: String,
    pub runtime_id: String,
}

pub(crate) trait AutomationDispatchAuthority: AuthorityEvidenceVerifier {
    fn resolve(
        &self,
        request: &AutomationAuthorityRequest,
    ) -> Result<ExtensionBag, AuthorityProfileError>;
}

#[derive(Clone, Copy)]
pub(crate) enum AutomationAuthorityMode<'a> {
    BaseV1,
    // Production construction remains blocked until the trusted-state adapter lands.
    #[allow(dead_code)]
    RuntimeAuthority(&'a dyn AutomationDispatchAuthority),
}

impl AutomationAuthorityMode<'_> {
    fn profile(self) -> Option<&'static str> {
        match self {
            Self::BaseV1 => None,
            Self::RuntimeAuthority(_) => Some(AUTHORITY_PROFILE),
        }
    }
}

#[derive(Clone, Copy)]
struct PersistLaunchContext<'a> {
    not_before: DateTime<Utc>,
    authority: AutomationAuthorityMode<'a>,
    scheduler_fence: Option<&'a super::leadership::SchedulerFence>,
}

fn authority_refusal(error: &AuthorityProfileError) -> String {
    format!(
        "automation authority refused dispatch: {}",
        error.code().as_str()
    )
}

fn authority_binding_matches_request(
    extension: &AutomationAuthorityExtension,
    request: &AutomationAuthorityRequest,
) -> bool {
    let base = &extension.execution_binding.base;
    base.automation_id.as_str() == request.automation_id
        && base.automation_revision.get() == request.automation_revision
        && base.definition_digest.value.as_str() == request.definition_digest
        && base.occurrence_id.as_str() == request.occurrence_id
        && base.occurrence_key.as_str() == request.occurrence_key
        && base.occurrence_fence_generation.get() == request.occurrence_fence_generation
        && base.run_id.as_str() == request.run_id
        && base.attempt_id.as_str() == request.attempt_id
        && base.attempt_number.get() == request.attempt_number
        && base.adoption_key.as_str() == request.adoption_key
        && extension.execution_binding.runtime.runtime_id.as_str() == request.runtime_id
}

fn resolve_authority_extension(
    authority: AutomationAuthorityMode<'_>,
    request: &AutomationAuthorityRequest,
) -> Result<Option<String>, String> {
    let AutomationAuthorityMode::RuntimeAuthority(authority) = authority else {
        return Ok(None);
    };
    let extensions = authority
        .resolve(request)
        .map_err(|error| authority_refusal(&error))?;
    let disposition = validate_authority_profile(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", AUTHORITY_PROFILE],
        &[RUNTIME_AUTHORITY_CAPABILITY],
        AuthorityValidationPhase::PreDispatch,
        Some(authority),
    )
    .map_err(|error| authority_refusal(&error))?;
    let AuthorityProfileDisposition::Validated(extension) = disposition else {
        return Err(authority_refusal(&AuthorityProfileError::new(
            AuthorityProfileErrorCode::ProfileRequired,
            "Runtime Authority validation did not produce a validated extension",
        )));
    };
    if !authority_binding_matches_request(&extension, request) {
        return Err(authority_refusal(&AuthorityProfileError::new(
            AuthorityProfileErrorCode::BindingMismatch,
            "authority binding does not match the claimed automation attempt",
        )));
    }
    serde_json::to_string(extension.as_ref())
        .map(Some)
        .map_err(|_| "failed to serialize validated automation authority binding".to_string())
}

fn persist_launch_with_clock(
    conn: &Connection,
    run_id: &str,
    occurrence_id: &str,
    definition: &RoutineDefinition,
    launch: &SessionLaunch,
    context: PersistLaunchContext<'_>,
    clock: impl FnOnce() -> DateTime<Utc>,
) -> Result<PersistLaunch, String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin durable automation launch: {error}"))?;
    let now = clock().max(context.not_before);
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let existing_run: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM automation_runs
                WHERE id = ?1
                  AND occurrence_id = ?2
                  AND automation_id = ?3
                  AND status = 'running'
            )",
            rusqlite::params![run_id, occurrence_id, definition.id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to inspect durable automation run: {error}"))?;
    let claim_is_current: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM automation_occurrences AS occurrence
                WHERE occurrence.id = ?1
                  AND occurrence.state = 'claimed'
                  AND occurrence.lease_owner IS NOT NULL
                  AND occurrence.lease_expires_at IS NOT NULL
                  AND occurrence.lease_expires_at > ?2
                  AND (
                      ?3 IS NULL
                      OR (
                          occurrence.scheduler_generation = ?3
                          AND EXISTS (
                              SELECT 1
                              FROM automation_scheduler_authority AS authority
                              WHERE authority.id = 1
                                AND authority.owner_id = ?4
                                AND authority.generation = ?3
                          )
                      )
                  )
            )",
            rusqlite::params![
                occurrence_id,
                now_iso,
                context.scheduler_fence.map(|fence| fence.generation()),
                context.scheduler_fence.map(|fence| fence.owner_id()),
            ],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to verify durable automation claim: {error}"))?;
    if !claim_is_current {
        if context.scheduler_fence.is_some() {
            return Err(
                "automations scheduler fence is stale or the occurrence claim changed".to_string(),
            );
        }
        if existing_run {
            let restored = transaction
                .execute(
                    "UPDATE automation_occurrences
                     SET state = 'planned',
                         lease_owner = NULL,
                         lease_expires_at = NULL,
                         updated_at = ?2
                     WHERE id = ?1 AND state = 'claimed'",
                    rusqlite::params![occurrence_id, now_iso],
                )
                .map_err(|error| format!("failed to restore expired retry claim: {error}"))?;
            if restored != 1 {
                return Err("expired retry claim changed before restoration".to_string());
            }
            transaction
                .commit()
                .map_err(|error| format!("failed to commit retry claim restoration: {error}"))?;
            return Ok(PersistLaunch::RetryRestored);
        }
        return Err("occurrence claim expired or changed before durable dispatch".to_string());
    }
    if existing_run {
        let timeout_at: String = transaction
            .query_row(
                "SELECT timeout_at
                 FROM automation_runs
                 WHERE id = ?1 AND status = 'running'",
                [run_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to read retry run timeout: {error}"))?;
        let timeout_at = DateTime::parse_from_rfc3339(&timeout_at)
            .map_err(|error| format!("run `{run_id}` has invalid timeout_at: {error}"))?
            .with_timezone(&Utc);
        if timeout_at <= now {
            settle_waiting_retry_timeout_in(&transaction, run_id, occurrence_id, now)?;
            transaction
                .commit()
                .map_err(|error| format!("failed to commit retry timeout settlement: {error}"))?;
            return Ok(PersistLaunch::RetryTimedOut);
        }
    }
    ensure_dispatch_definition_pin(
        &transaction,
        occurrence_id,
        definition,
        existing_run.then_some(run_id),
    )?;
    let overlapping_run: bool = transaction
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM automation_runs
                 WHERE automation_id = ?1
                   AND status = 'running'
                   AND occurrence_id IS NOT ?2
             )",
            rusqlite::params![definition.id, occurrence_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to enforce automation overlap policy: {error}"))?;
    if overlapping_run {
        return Err("routine already has a nonterminal run; overlap is forbidden".to_string());
    }
    let session =
        crate::session_launch::new_session_record(crate::session_launch::NewSessionParams {
            id: launch.id.clone(),
            project_root: launch.project_root.clone(),
            harness: launch.harness.clone(),
            title: launch.title.clone(),
            status: "created".to_string(),
            now: now_iso.clone(),
            conversation_id: None,
            familiar_id: launch.familiar_id.clone(),
            execution_binding: None,
            labels: Vec::new(),
            visibility: None,
        });
    crate::store::insert_session(&transaction, &session)
        .map_err(|error| format!("failed to persist automation session: {error:#}"))?;
    let attempt_number = if existing_run {
        let attempt_number: i64 = transaction
            .query_row(
                "SELECT a.attempt_number
                 FROM automation_attempts AS a
                 JOIN automation_occurrences AS o ON o.id = a.occurrence_id
                 WHERE a.run_id = ?1
                   AND a.occurrence_id = ?2
                   AND a.state = 'adopted'
                   AND a.not_before <= ?3
                   AND a.occurrence_fence_generation = o.attempt
                 ORDER BY a.attempt_number DESC
                 LIMIT 1",
                rusqlite::params![run_id, occurrence_id, now_iso],
                |row| row.get(0),
            )
            .map_err(|error| format!("retry attempt is not ready for dispatch: {error}"))?;
        u8::try_from(attempt_number)
            .map_err(|_| "retry attempt number exceeds supported range".to_string())?
    } else {
        record_run_start(
            &transaction,
            run_id,
            RunStart {
                automation_id: &definition.id,
                occurrence_id: Some(occurrence_id),
                authority_profile: context.authority.profile(),
                session_id: Some(&launch.id),
                familiar_id: definition.familiar_id.as_deref(),
                runtime: &definition.runtime,
                timeout_at: now + chrono::Duration::minutes(i64::from(definition.timeout_minutes)),
            },
            now,
        )
        .map_err(|error| format!("failed to record run start: {error:#}"))?;
        transaction
            .execute(
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, dispatch_generation, state,
                     retry_classification, not_before, opened_at)
                 SELECT ?1, ?2, ?3, 1, ?4, attempt, 0, 'adopted',
                        'initial', ?5, ?5
                 FROM automation_occurrences
                 WHERE id = ?3 AND state = 'claimed'",
                rusqlite::params![
                    format!("attempt-{run_id}-1"),
                    run_id,
                    occurrence_id,
                    format!("automation:{run_id}:1"),
                    now_iso,
                ],
            )
            .map_err(|error| format!("failed to record initial automation attempt: {error}"))?;
        1
    };
    let stored_profile: Option<String> = transaction
        .query_row(
            "SELECT authority_profile FROM automation_runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read automation authority profile: {error}"))?;
    if stored_profile.as_deref() != context.authority.profile() {
        return Err(
            "automation authority profile changed across attempts; refusing downgrade".to_string(),
        );
    }
    let request = transaction
        .query_row(
            "SELECT r.automation_revision, r.definition_digest, o.scheduled_for, o.kind,
                    a.id, a.adoption_key, a.occurrence_fence_generation
                 FROM automation_runs AS r
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1
                   AND a.attempt_number = ?2
                   AND a.state = 'adopted'",
            rusqlite::params![run_id, i64::from(attempt_number)],
            |row| {
                let revision: i64 = row.get(0)?;
                let scheduled_for: String = row.get(2)?;
                let occurrence_kind: String = row.get(3)?;
                let attempt_id: String = row.get(4)?;
                let adoption_key: String = row.get(5)?;
                let fence: i64 = row.get(6)?;
                let occurrence_key = match occurrence_kind.as_str() {
                    "scheduled" => format!("{}@{scheduled_for}", definition.id),
                    "manual" => format!("{}@manual-{adoption_key}", definition.id),
                    _ => {
                        return Err(rusqlite::Error::InvalidColumnType(
                            3,
                            "kind".to_string(),
                            rusqlite::types::Type::Text,
                        ));
                    }
                };
                Ok(AutomationAuthorityRequest {
                    automation_id: definition.id.clone(),
                    automation_revision: u64::try_from(revision).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    definition_digest: row.get::<_, Option<String>>(1)?.ok_or_else(|| {
                        rusqlite::Error::InvalidColumnType(
                            1,
                            "definition_digest".to_string(),
                            rusqlite::types::Type::Null,
                        )
                    })?,
                    occurrence_id: occurrence_id.to_string(),
                    occurrence_key,
                    occurrence_fence_generation: u64::try_from(fence).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    run_id: run_id.to_string(),
                    attempt_id,
                    attempt_number: u64::from(attempt_number),
                    adoption_key,
                    runtime_id: launch.harness.clone(),
                })
            },
        )
        .map_err(|error| format!("failed to construct automation authority request: {error}"))?;
    let authority_extension_json = resolve_authority_extension(context.authority, &request)?;
    let dispatched = transaction
        .execute(
            "UPDATE automation_attempts
                 SET state = 'dispatching',
                     dispatch_generation = dispatch_generation + 1,
                     authority_extension_json = ?4
                 WHERE run_id = ?1
                   AND attempt_number = ?2
                   AND state = 'adopted'
                   AND not_before <= ?3
                   AND authority_extension_json IS NULL
                   AND occurrence_fence_generation = (
                       SELECT attempt
                       FROM automation_occurrences
                       WHERE id = automation_attempts.occurrence_id
                   )",
            rusqlite::params![
                run_id,
                i64::from(attempt_number),
                now_iso,
                authority_extension_json,
            ],
        )
        .map_err(|error| format!("failed to dispatch automation attempt: {error}"))?;
    if dispatched != 1 {
        return Err("automation attempt changed before dispatch".to_string());
    }
    if existing_run {
        let updated = transaction
            .execute(
                "UPDATE automation_runs
                 SET session_id = ?2
                 WHERE id = ?1 AND status = 'running'",
                rusqlite::params![run_id, launch.id],
            )
            .map_err(|error| format!("failed to bind retry session to run: {error}"))?;
        if updated != 1 {
            return Err("automation run changed before retry dispatch".to_string());
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit durable automation launch: {error}"))?;
    Ok(PersistLaunch::Ready(AttemptDispatch {
        run_id: run_id.to_string(),
        attempt_number,
    }))
}

fn publish_runtime_ownership(
    conn: &Connection,
    occurrence_id: &str,
    run_id: &str,
    attempt_number: u8,
    session_id: &str,
    now: DateTime<Utc>,
    scheduler_fence: Option<&super::leadership::SchedulerFence>,
) -> anyhow::Result<()> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if let Some(fence) = scheduler_fence {
        require_current_scheduler_fence(&transaction, occurrence_id, fence)
            .map_err(anyhow::Error::msg)?;
    }
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
    let attempt_started = transaction.execute(
        "UPDATE automation_attempts
         SET state = 'started',
             session_id = ?3
         WHERE run_id = ?1
           AND attempt_number = ?2
           AND state = 'dispatching'
           AND session_id IS NULL",
        rusqlite::params![run_id, i64::from(attempt_number), session_id],
    )?;
    if attempt_started != 1 {
        let state: Option<String> = transaction
            .query_row(
                "SELECT state FROM automation_attempts
                 WHERE run_id = ?1 AND attempt_number = ?2 AND session_id = ?3",
                rusqlite::params![run_id, i64::from(attempt_number), session_id],
                |row| row.get(0),
            )
            .optional()?;
        if !state
            .as_deref()
            .is_some_and(|state| matches!(state, "started" | "observing" | "succeeded" | "failed"))
        {
            anyhow::bail!("automation attempt changed before runtime ownership was published");
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

struct RejectedLaunch<'a> {
    occurrence_id: &'a str,
    run_id: &'a str,
    attempt_number: u8,
    session_id: &'a str,
    definition: &'a RoutineDefinition,
    failure: PreownershipFailure,
    reason: &'a str,
    scheduler_fence: Option<&'a super::leadership::SchedulerFence>,
}

fn settle_rejected_launch(
    conn: &Connection,
    rejected: RejectedLaunch<'_>,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let RejectedLaunch {
        occurrence_id,
        run_id,
        attempt_number,
        session_id,
        definition,
        failure,
        reason,
        scheduler_fence,
    } = rejected;
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin launch rejection settlement: {error}"))?;
    if let Some(fence) = scheduler_fence {
        require_current_scheduler_fence(&transaction, occurrence_id, fence)?;
    }
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    crate::store::update_session_terminal_if_active(
        &transaction,
        session_id,
        "failed",
        None,
        &now_iso,
    )
    .map_err(|error| format!("failed to settle rejected session: {error:#}"))?;
    let settled_attempt = transaction
        .execute(
            "UPDATE automation_attempts
             SET state = 'failed',
                 failure_class = ?3,
                 state_reason = ?4,
                 settled_at = ?5
             WHERE run_id = ?1
               AND attempt_number = ?2
               AND state = 'dispatching'",
            rusqlite::params![
                run_id,
                i64::from(attempt_number),
                failure_class_name(failure),
                reason,
                now_iso,
            ],
        )
        .map_err(|error| format!("failed to settle rejected attempt: {error}"))?;
    if settled_attempt != 1 {
        return Err("failed to settle rejected attempt".to_string());
    }
    let retryable = match failure {
        PreownershipFailure::Retryable(failure_class) => definition.retry.retries(failure_class),
        PreownershipFailure::LaunchRefused => false,
    };
    let retry_deadline_open: bool = transaction
        .query_row(
            "SELECT timeout_at > ?2
             FROM automation_runs
             WHERE id = ?1 AND status = 'running'",
            rusqlite::params![run_id, now_iso],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to inspect retry run deadline: {error}"))?;
    if retryable && attempt_number < definition.retry.max_attempts && retry_deadline_open {
        let next_attempt_number = attempt_number + 1;
        let retry_at = now
            + chrono::Duration::seconds(i64::from(retry_delay_seconds(
                &definition.retry,
                run_id,
                next_attempt_number,
            )));
        let retry_at_iso = retry_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        transaction
            .execute(
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, dispatch_generation, state,
                     prior_attempt_number, prior_disposition, retry_classification,
                     not_before, opened_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, attempt, 0, 'adopted',
                        ?6, 'failed', 'automatic_retry', ?7, ?8
                 FROM automation_occurrences
                 WHERE id = ?3 AND state = 'claimed'",
                rusqlite::params![
                    format!("attempt-{run_id}-{next_attempt_number}"),
                    run_id,
                    occurrence_id,
                    i64::from(next_attempt_number),
                    format!("automation:{run_id}:{next_attempt_number}"),
                    i64::from(attempt_number),
                    retry_at_iso,
                    now_iso,
                ],
            )
            .map_err(|error| format!("failed to record retry attempt: {error}"))?;
        let replanned = transaction
            .execute(
                "UPDATE automation_occurrences
                 SET state = 'planned',
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     failure_reason = ?2,
                     updated_at = ?3
                 WHERE id = ?1 AND state = 'claimed'",
                rusqlite::params![occurrence_id, reason, now_iso],
            )
            .map_err(|error| format!("failed to schedule retry occurrence: {error}"))?;
        if replanned != 1 {
            return Err("failed to schedule retry occurrence".to_string());
        }
        let released_session = transaction
            .execute(
                "UPDATE automation_runs
                 SET session_id = NULL
                 WHERE id = ?1 AND status = 'running' AND session_id = ?2",
                rusqlite::params![run_id, session_id],
            )
            .map_err(|error| format!("failed to release retry session binding: {error}"))?;
        if released_session != 1 {
            return Err("failed to release retry session binding".to_string());
        }
        transaction
            .commit()
            .map_err(|error| format!("failed to commit retry scheduling: {error}"))?;
        return Ok(true);
    }
    if retryable && attempt_number >= definition.retry.max_attempts {
        record_retry_exhaustion(
            &transaction,
            &definition.id,
            failure_class_name(failure),
            reason,
            now,
        )
        .map_err(|error| format!("failed to record retry exhaustion: {error:#}"))?;
    }
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
        .map_err(|error| format!("failed to commit launch rejection settlement: {error}"))?;
    Ok(false)
}

fn dispatch_occurrence(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    definition: &RoutineDefinition,
    occurrence_id: &str,
    cwd: &str,
    now: DateTime<Utc>,
) -> Result<RunOutcome, String> {
    let mut clock = Utc::now;
    let cancelled = || false;
    let mut control = DispatchControl {
        clock: &mut clock,
        cancelled: &cancelled,
        authority: AutomationAuthorityMode::BaseV1,
        scheduler_fence: None,
    };
    match dispatch_occurrence_with_clock(
        conn,
        runtime,
        definition,
        occurrence_id,
        cwd,
        now,
        &mut control,
    )? {
        DispatchAttempt::Completed(outcome) => Ok(outcome),
        DispatchAttempt::Deferred => {
            Err("manual dispatch was unexpectedly deferred before runtime ownership".to_string())
        }
    }
}

enum DispatchAttempt {
    Completed(RunOutcome),
    Deferred,
}

enum PersistLaunch {
    Ready(AttemptDispatch),
    RetryRestored,
    RetryTimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttemptDispatch {
    run_id: String,
    attempt_number: u8,
}

struct DispatchControl<'a> {
    clock: &'a mut dyn FnMut() -> DateTime<Utc>,
    cancelled: &'a dyn Fn() -> bool,
    authority: AutomationAuthorityMode<'a>,
    scheduler_fence: Option<&'a super::leadership::SchedulerFence>,
}

fn dispatch_occurrence_with_clock(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    definition: &RoutineDefinition,
    occurrence_id: &str,
    cwd: &str,
    now: DateTime<Utc>,
    control: &mut DispatchControl<'_>,
) -> Result<DispatchAttempt, String> {
    let run_id =
        current_run_id_for_occurrence(conn, occurrence_id)?.unwrap_or_else(|| fresh_id("run"));
    let launch = build_session_launch(definition, cwd)?;
    let attempt = match persist_launch_with_clock(
        conn,
        &run_id,
        occurrence_id,
        definition,
        &launch,
        PersistLaunchContext {
            not_before: now,
            authority: control.authority,
            scheduler_fence: control.scheduler_fence,
        },
        &mut control.clock,
    )? {
        PersistLaunch::Ready(attempt) => attempt,
        PersistLaunch::RetryRestored => return Ok(DispatchAttempt::Deferred),
        PersistLaunch::RetryTimedOut => {
            return Ok(DispatchAttempt::Completed(RunOutcome {
                run_id,
                status: "failed".to_string(),
                session_id: None,
                error: Some("automation retry wait exceeded run timeout".to_string()),
            }));
        }
    };

    let ownership_published = Cell::new(false);
    let ownership_publication_error = RefCell::new(None);
    let mut ownership_established = || match publish_runtime_ownership(
        conn,
        occurrence_id,
        &attempt.run_id,
        attempt.attempt_number,
        &launch.id,
        now,
        control.scheduler_fence,
    ) {
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
    let launch_result =
        runtime.launch_contained_adopted_session(&launch, None, &mut ownership_established);
    if let Some(fence) = control.scheduler_fence {
        if !fence
            .is_current(conn)
            .map_err(|error| format!("failed to verify automations scheduler fence: {error:#}"))?
        {
            return Err("automations scheduler fence is stale".to_string());
        }
    }
    match launch_result {
        Ok(()) if ownership_published.get() => Ok(DispatchAttempt::Completed(RunOutcome {
            run_id,
            status: "running".to_string(),
            session_id: Some(launch.id),
            error: None,
        })),
        result if ownership_published.get() => {
            let error = match result {
                Ok(()) => None,
                Err(error) => Some(format!(
                    "runtime ownership was established but launch acknowledgement failed: {error:#}"
                )),
            };
            Ok(DispatchAttempt::Completed(RunOutcome {
                run_id,
                status: "running".to_string(),
                session_id: Some(launch.id),
                error,
            }))
        }
        Ok(()) => {
            let publication_error = publish_runtime_ownership(
                conn,
                occurrence_id,
                &attempt.run_id,
                attempt.attempt_number,
                &launch.id,
                now,
                control.scheduler_fence,
            )
            .err();
            Ok(DispatchAttempt::Completed(RunOutcome {
                run_id,
                status: "running".to_string(),
                session_id: Some(launch.id),
                error: publication_error.map(|error| {
                    format!(
                        "runtime accepted launch without publishing ownership; completion is ambiguous: {error:#}"
                    )
                }),
            }))
        }
        Err(error)
            if error
                .downcast_ref::<crate::daemon::RuntimeOwnershipRetainedError>()
                .is_some()
                || error
                    .downcast_ref::<crate::api::RuntimeOwnershipPublicationError>()
                    .is_some() =>
        {
            let publication_error = publish_runtime_ownership(
                conn,
                occurrence_id,
                &attempt.run_id,
                attempt.attempt_number,
                &launch.id,
                now,
                control.scheduler_fence,
            )
            .err();
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
            Ok(DispatchAttempt::Completed(RunOutcome {
                run_id,
                status: "running".to_string(),
                session_id: Some(launch.id),
                error: Some(error),
            }))
        }
        Err(error)
            if (control.cancelled)()
                && error
                    .downcast_ref::<crate::api::RuntimeLaunchAdmissionClosedError>()
                    .is_some() =>
        {
            if attempt_has_authority(conn, &run_id, attempt.attempt_number)? {
                let reason =
                    "runtime admission closed after authority consumption; no process started";
                let retry_scheduled = settle_rejected_launch(
                    conn,
                    RejectedLaunch {
                        occurrence_id,
                        run_id: &run_id,
                        attempt_number: attempt.attempt_number,
                        session_id: &launch.id,
                        definition,
                        failure: PreownershipFailure::Retryable(RetryableClass::TransientDispatch),
                        reason,
                        scheduler_fence: control.scheduler_fence,
                    },
                    now,
                )?;
                return Ok(DispatchAttempt::Completed(RunOutcome {
                    run_id,
                    status: if retry_scheduled {
                        "retry_scheduled".to_string()
                    } else {
                        "failed".to_string()
                    },
                    session_id: None,
                    error: Some(reason.to_string()),
                }));
            }
            restore_preownership_launch_for_retry(
                conn,
                occurrence_id,
                &run_id,
                &launch.id,
                now,
                control.scheduler_fence,
            )?;
            Ok(DispatchAttempt::Deferred)
        }
        Err(error) => {
            let reason = format!("{error:#}");
            let failure_class = classify_preownership_failure(&error);
            let failure_at = (control.clock)().max(now);
            let retry_scheduled = settle_rejected_launch(
                conn,
                RejectedLaunch {
                    occurrence_id,
                    run_id: &run_id,
                    attempt_number: attempt.attempt_number,
                    session_id: &launch.id,
                    definition,
                    failure: failure_class,
                    reason: &reason,
                    scheduler_fence: control.scheduler_fence,
                },
                failure_at,
            )?;
            Ok(DispatchAttempt::Completed(RunOutcome {
                run_id,
                status: if retry_scheduled {
                    "retry_scheduled".to_string()
                } else {
                    "failed".to_string()
                },
                session_id: None,
                error: Some(reason),
            }))
        }
    }
}

fn current_run_id_for_occurrence(
    conn: &Connection,
    occurrence_id: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id
         FROM automation_runs
         WHERE occurrence_id = ?1 AND status = 'running'
         ORDER BY started_at DESC
         LIMIT 1",
        [occurrence_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| format!("failed to read current automation run: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreownershipFailure {
    Retryable(RetryableClass),
    LaunchRefused,
}

fn classify_preownership_failure(error: &anyhow::Error) -> PreownershipFailure {
    let io_kind = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind)
    });
    match io_kind {
        Some(
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::AddrNotAvailable
            | std::io::ErrorKind::BrokenPipe,
        ) => PreownershipFailure::Retryable(RetryableClass::RuntimeUnavailable),
        Some(
            std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted,
        ) => PreownershipFailure::Retryable(RetryableClass::TransientDispatch),
        _ => PreownershipFailure::LaunchRefused,
    }
}

fn failure_class_name(failure: PreownershipFailure) -> &'static str {
    let PreownershipFailure::Retryable(failure_class) = failure else {
        return "launch_refused";
    };
    match failure_class {
        RetryableClass::TransientDispatch => "transient_dispatch",
        RetryableClass::LeaseExpired => "lease_expired",
        RetryableClass::RuntimeUnavailable => "runtime_unavailable",
    }
}

fn attempt_has_authority(
    conn: &Connection,
    run_id: &str,
    attempt_number: u8,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT authority_extension_json IS NOT NULL
         FROM automation_attempts
         WHERE run_id = ?1 AND attempt_number = ?2",
        rusqlite::params![run_id, i64::from(attempt_number)],
        |row| row.get(0),
    )
    .map_err(|error| format!("failed to inspect automation attempt authority: {error}"))
}

fn retry_delay_seconds(policy: &RoutineRetryPolicy, run_id: &str, next_attempt_number: u8) -> u32 {
    match policy.backoff_policy {
        BackoffPolicy::None => 0,
        BackoffPolicy::Fixed => policy.backoff_seconds.unwrap_or(1),
        BackoffPolicy::Exponential => {
            let base = policy.backoff_seconds.unwrap_or(1);
            let exponent = u32::from(next_attempt_number.saturating_sub(2)).min(16);
            let ceiling = base.saturating_mul(1_u32 << exponent).min(86_400);
            let digest = blake3::hash(format!("{run_id}:{next_attempt_number}").as_bytes());
            let sample = u64::from_be_bytes(
                digest.as_bytes()[..8]
                    .try_into()
                    .expect("blake3 digest prefix is eight bytes"),
            );
            u32::try_from(sample % u64::from(ceiling) + 1)
                .expect("retry delay is bounded to one day")
        }
    }
}

fn restore_preownership_launch_for_retry(
    conn: &Connection,
    occurrence_id: &str,
    run_id: &str,
    session_id: &str,
    now: DateTime<Utc>,
    scheduler_fence: Option<&super::leadership::SchedulerFence>,
) -> Result<(), String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin cancelled launch restoration: {error}"))?;
    if let Some(fence) = scheduler_fence {
        require_current_scheduler_fence(&transaction, occurrence_id, fence)?;
    }
    restore_preownership_launch_for_retry_in(&transaction, occurrence_id, run_id, session_id, now)?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit cancelled launch restoration: {error}"))
}

fn restore_preownership_launch_for_retry_in(
    conn: &Connection,
    occurrence_id: &str,
    run_id: &str,
    session_id: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let preserves_run: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM automation_attempts AS current_attempt
                WHERE current_attempt.run_id = ?1
                  AND current_attempt.state = 'dispatching'
                  AND EXISTS (
                      SELECT 1
                      FROM automation_attempts AS prior_attempt
                      WHERE prior_attempt.run_id = current_attempt.run_id
                        AND prior_attempt.attempt_number < current_attempt.attempt_number
                        AND prior_attempt.state IN (
                            'succeeded', 'failed', 'cancelled', 'timed_out', 'ambiguous'
                        )
                  )
            )",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to inspect cancelled automation attempt: {error}"))?;
    if !preserves_run {
        let deleted_run = conn
            .execute(
                "DELETE FROM automation_runs
                 WHERE id = ?1 AND occurrence_id = ?2 AND session_id = ?3 AND status = 'running'",
                rusqlite::params![run_id, occurrence_id, session_id],
            )
            .map_err(|error| format!("failed to remove cancelled automation run: {error}"))?;
        if deleted_run != 1 {
            return Err("cancelled automation run changed before restoration".to_string());
        }
    }
    let deleted_session = conn
        .execute(
            "DELETE FROM sessions WHERE id = ?1 AND status = 'created'",
            [session_id],
        )
        .map_err(|error| format!("failed to remove cancelled automation session: {error}"))?;
    if deleted_session != 1 {
        return Err("cancelled automation session changed before restoration".to_string());
    }
    if preserves_run {
        let restored_attempt = conn
            .execute(
                "UPDATE automation_attempts
                 SET state = 'adopted'
                 WHERE run_id = ?1 AND state = 'dispatching'",
                [run_id],
            )
            .map_err(|error| format!("failed to restore cancelled retry attempt: {error}"))?;
        if restored_attempt != 1 {
            return Err("cancelled retry attempt changed before restoration".to_string());
        }
        let restored_run = conn
            .execute(
                "UPDATE automation_runs
                 SET session_id = NULL
                 WHERE id = ?1 AND occurrence_id = ?2 AND status = 'running'",
                rusqlite::params![run_id, occurrence_id],
            )
            .map_err(|error| format!("failed to restore cancelled retry run: {error}"))?;
        if restored_run != 1 {
            return Err("cancelled retry run changed before restoration".to_string());
        }
    }
    let restored_occurrence = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'planned',
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 failure_reason = NULL,
                 updated_at = ?2
             WHERE id = ?1 AND state = 'claimed' AND lease_owner = 'daemon'",
            rusqlite::params![
                occurrence_id,
                now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .map_err(|error| format!("failed to restore cancelled automation occurrence: {error}"))?;
    if restored_occurrence != 1 {
        return Err("cancelled automation occurrence changed before restoration".to_string());
    }
    Ok(())
}

pub(crate) fn recover_no_process_preownership_launches(
    coven_home: &Path,
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<usize, String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin pre-ownership launch recovery: {error}"))?;
    let candidates: Vec<(String, String, String, String)> = {
        let mut statement = transaction
            .prepare(
                "SELECT r.id, r.session_id, r.occurrence_id, r.automation_id
                 FROM automation_runs AS r
                 JOIN sessions AS s ON s.id = r.session_id
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 WHERE r.status = 'running'
                   AND s.status = 'created'
                   AND o.state = 'claimed'
                   AND o.lease_owner = 'daemon'",
            )
            .map_err(|error| format!("failed to prepare pre-ownership launch recovery: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|error| format!("failed to query pre-ownership launches: {error}"))?;
        let mut candidates = Vec::new();
        for row in rows {
            candidates.push(
                row.map_err(|error| format!("failed to read pre-ownership launch: {error}"))?,
            );
        }
        candidates
    };
    let retryable = candidates
        .into_iter()
        .filter(|(_, session_id, _, _)| {
            std::fs::read(containment_receipt_path(coven_home, session_id))
                .is_ok_and(|receipt| receipt == crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT)
        })
        .collect::<Vec<_>>();
    for (run_id, session_id, occurrence_id, automation_id) in &retryable {
        if !retry_proven_preownership_lease_expiry_in(
            &transaction,
            automation_id,
            occurrence_id,
            run_id,
            session_id,
            now,
        )? {
            restore_preownership_launch_for_retry_in(
                &transaction,
                occurrence_id,
                run_id,
                session_id,
                now,
            )?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit pre-ownership launch recovery: {error}"))?;
    Ok(retryable.len())
}

fn retry_proven_preownership_lease_expiry_in(
    conn: &Connection,
    automation_id: &str,
    occurrence_id: &str,
    run_id: &str,
    session_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let legacy_without_snapshot_or_attempts: bool = conn
        .query_row(
            "SELECT definition_json IS NULL
                    AND NOT EXISTS (
                        SELECT 1 FROM automation_attempts WHERE run_id = automation_runs.id
                    )
             FROM automation_runs
             WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to inspect legacy automation run: {error}"))?;
    if legacy_without_snapshot_or_attempts {
        return Ok(false);
    }
    let Some(definition) =
        load_definition_for_occurrence_dispatch(conn, automation_id, occurrence_id)?
    else {
        return Ok(false);
    };
    let retries_lease_expiry = definition.retry.retries(RetryableClass::LeaseExpired);
    let authority_bound: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM automation_attempts
                WHERE run_id = ?1
                  AND state = 'dispatching'
                  AND authority_extension_json IS NOT NULL
            )",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to inspect recovered attempt authority: {error}"))?;
    if !retries_lease_expiry && !authority_bound {
        return Ok(false);
    }
    let attempt_number: i64 = conn
        .query_row(
            "SELECT attempt_number
             FROM automation_attempts
             WHERE run_id = ?1 AND state = 'dispatching'
             ORDER BY attempt_number DESC
             LIMIT 1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read expired automation attempt: {error}"))?;
    let attempt_number = u8::try_from(attempt_number)
        .map_err(|_| "expired attempt number exceeds supported range".to_string())?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    crate::store::update_session_terminal_if_active(conn, session_id, "failed", None, &now_iso)
        .map_err(|error| format!("failed to settle expired automation session: {error:#}"))?;
    let settled = conn
        .execute(
            "UPDATE automation_attempts
             SET state = 'failed',
                 failure_class = 'lease_expired',
                 state_reason = 'claim lease expired with proof that no process started',
                 settled_at = ?3
             WHERE run_id = ?1
               AND attempt_number = ?2
               AND state = 'dispatching'",
            rusqlite::params![run_id, i64::from(attempt_number), now_iso],
        )
        .map_err(|error| format!("failed to settle expired automation attempt: {error}"))?;
    if settled != 1 {
        return Err("expired automation attempt changed during recovery".to_string());
    }
    if retries_lease_expiry && attempt_number < definition.retry.max_attempts {
        let next_attempt_number = attempt_number + 1;
        let retry_at = now
            + chrono::Duration::seconds(i64::from(retry_delay_seconds(
                &definition.retry,
                run_id,
                next_attempt_number,
            )));
        let retry_at_iso = retry_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state,
                 prior_attempt_number, prior_disposition, retry_classification,
                 not_before, opened_at)
             SELECT ?1, ?2, ?3, ?4, ?5, attempt, 0, 'adopted',
                    ?6, 'failed', 'automatic_retry', ?7, ?8
             FROM automation_occurrences
             WHERE id = ?3 AND state = 'claimed'",
            rusqlite::params![
                format!("attempt-{run_id}-{next_attempt_number}"),
                run_id,
                occurrence_id,
                i64::from(next_attempt_number),
                format!("automation:{run_id}:{next_attempt_number}"),
                i64::from(attempt_number),
                retry_at_iso,
                now_iso,
            ],
        )
        .map_err(|error| format!("failed to open lease-expiry retry attempt: {error}"))?;
        let replanned = conn
            .execute(
                "UPDATE automation_occurrences
                 SET state = 'planned',
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     failure_reason = 'claim lease expired with proof that no process started',
                     updated_at = ?2
                 WHERE id = ?1 AND state = 'claimed'",
                rusqlite::params![occurrence_id, now_iso],
            )
            .map_err(|error| format!("failed to replan expired automation occurrence: {error}"))?;
        if replanned != 1 {
            return Err("expired automation occurrence changed during recovery".to_string());
        }
        conn.execute(
            "UPDATE automation_runs SET session_id = NULL WHERE id = ?1 AND status = 'running'",
            [run_id],
        )
        .map_err(|error| format!("failed to release expired run session binding: {error}"))?;
        return Ok(true);
    }

    let reason = if retries_lease_expiry {
        "claim lease expired after all retry attempts were exhausted"
    } else {
        "claim lease expired after authority consumption with proof that no process started"
    };
    if retries_lease_expiry {
        record_retry_exhaustion(conn, automation_id, "lease_expired", reason, now)
            .map_err(|error| format!("failed to record lease retry exhaustion: {error:#}"))?;
    }
    if !settle_occurrence(conn, occurrence_id, "failed", Some(reason), now)? {
        return Err("failed to settle lease-exhausted occurrence".to_string());
    }
    if !record_run_finish(
        conn,
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
    .map_err(|error| format!("failed to settle lease-exhausted run: {error:#}"))?
    {
        return Err("failed to settle lease-exhausted run".to_string());
    }
    Ok(true)
}

fn ensure_dispatch_definition_pin(
    conn: &Connection,
    occurrence_id: &str,
    definition: &RoutineDefinition,
    existing_run_id: Option<&str>,
) -> Result<(), String> {
    let (automation_id, automation_revision, occurrence_digest): (String, i64, Option<String>) =
        conn.query_row(
            "SELECT automation_id, automation_revision, definition_digest
             FROM automation_occurrences
             WHERE id = ?1",
            [occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| format!("failed to read occurrence definition pin: {error}"))?;
    if automation_id != definition.id {
        return Err(format!(
            "occurrence `{occurrence_id}` belongs to `{automation_id}`, not `{}`",
            definition.id
        ));
    }
    let Some(occurrence_digest) = occurrence_digest else {
        return Err(format!(
            "occurrence `{occurrence_id}` has unverifiable legacy definition history"
        ));
    };
    if let Some(run_id) = existing_run_id {
        let (run_automation_id, run_revision, run_digest, run_definition_json): (
            String,
            i64,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT automation_id, automation_revision, definition_digest, definition_json
                 FROM automation_runs
                 WHERE id = ?1 AND occurrence_id = ?2 AND status = 'running'",
                rusqlite::params![run_id, occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|error| format!("failed to read run definition pin: {error}"))?;
        if run_automation_id != automation_id
            || run_revision != automation_revision
            || run_digest.as_deref() != Some(occurrence_digest.as_str())
        {
            return Err(format!(
                "run `{run_id}` does not match occurrence `{occurrence_id}` definition pin"
            ));
        }
        let Some(run_definition_json) = run_definition_json else {
            return Err(format!(
                "run `{run_id}` has no durable definition snapshot for retry"
            ));
        };
        let snapshot_digest = super::contract::migration::definition_digest(&run_definition_json)
            .map_err(|error| {
            format!("failed to digest run `{run_id}` definition snapshot: {error:#}")
        })?;
        if snapshot_digest != occurrence_digest {
            return Err(format!(
                "run `{run_id}` definition snapshot does not match its pinned digest"
            ));
        }
        let snapshot: RoutineDefinition =
            serde_json::from_str(&run_definition_json).map_err(|error| {
                format!("run `{run_id}` definition snapshot is unreadable: {error}")
            })?;
        if snapshot != *definition {
            return Err(format!(
                "retry definition does not match run `{run_id}` snapshot"
            ));
        }
        return Ok(());
    }
    let Some(current) =
        super::store::get_definition(conn, &automation_id).map_err(|error| format!("{error:#}"))?
    else {
        return Err(format!(
            "routine `{automation_id}` vanished after occurrence fencing"
        ));
    };
    let current_revision =
        i64::try_from(current.revision).map_err(|_| "definition revision exceeds SQLite range")?;
    if current_revision != automation_revision
        || current.definition_digest.as_deref() != Some(occurrence_digest.as_str())
    {
        return Err(format!(
            "definition revision changed after occurrence fencing for `{automation_id}`"
        ));
    }
    let persisted_digest = super::contract::migration::definition_digest(&current.definition_json)
        .map_err(|error| format!("failed to digest stored routine `{automation_id}`: {error:#}"))?;
    if persisted_digest != occurrence_digest {
        return Err(format!(
            "stored definition digest changed after occurrence fencing for `{automation_id}`"
        ));
    }
    let persisted_definition: RoutineDefinition = serde_json::from_str(&current.definition_json)
        .map_err(|error| format!("stored routine `{automation_id}` is unreadable: {error}"))?;
    if persisted_definition != *definition {
        return Err(format!(
            "definition body changed after occurrence fencing for `{automation_id}`"
        ));
    }
    Ok(())
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
    if is_retry_quarantined(conn, &definition.id)
        .map_err(|error| format!("failed to inspect routine quarantine: {error:#}"))?
    {
        return Ok(RunOutcome {
            run_id: String::new(),
            status: "failed".to_string(),
            session_id: None,
            error: Some(
                "routine is quarantined after retry exhaustion; explicitly unquarantine it before running"
                    .to_string(),
            ),
        });
    }
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

    let dispatch_now = Utc::now().max(now);
    match dispatch_occurrence(conn, runtime, definition, &occurrence_id, cwd, dispatch_now) {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            if !settle_occurrence(conn, &occurrence_id, "failed", Some(&error), dispatch_now)? {
                return Err(format!(
                    "{error}; manual occurrence changed before rejection settlement"
                ));
            }
            Err(error)
        }
    }
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
#[cfg(test)]
fn dispatch_claimed_occurrences(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<DispatchReport, String> {
    dispatch_claimed_occurrences_with_clock(conn, runtime, now, Utc::now)
}

#[cfg(test)]
pub(crate) fn dispatch_claimed_occurrences_with_clock(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
    clock: impl FnMut() -> DateTime<Utc>,
) -> Result<DispatchReport, String> {
    dispatch_claimed_occurrences_with_clock_and_cancel(conn, runtime, now, clock, || false)
}

pub(crate) fn dispatch_claimed_occurrences_with_clock_and_cancel(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
    clock: impl FnMut() -> DateTime<Utc>,
    cancelled: impl Fn() -> bool,
) -> Result<DispatchReport, String> {
    dispatch_claimed_occurrences_inner(conn, runtime, now, clock, cancelled, None)
}

pub(crate) fn dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
    clock: impl FnMut() -> DateTime<Utc>,
    cancelled: impl Fn() -> bool,
    fence: &super::leadership::SchedulerFence,
) -> Result<DispatchReport, String> {
    if !fence
        .is_current(conn)
        .map_err(|error| format!("failed to verify automations scheduler fence: {error:#}"))?
    {
        return Err("automations scheduler fence is stale".to_string());
    }
    dispatch_claimed_occurrences_inner(conn, runtime, now, clock, cancelled, Some(fence))
}

fn dispatch_claimed_occurrences_inner(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
    mut clock: impl FnMut() -> DateTime<Utc>,
    cancelled: impl Fn() -> bool,
    scheduler_fence: Option<&super::leadership::SchedulerFence>,
) -> Result<DispatchReport, String> {
    let mut report = DispatchReport::default();
    match scheduler_fence {
        Some(fence) => {
            recover_expired_leases_with_scheduler_fence(conn, now, fence)?;
        }
        None => {
            recover_expired_leases(conn, now)?;
        }
    }
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let claimed: Vec<(String, String)> = {
        let mut statement = conn
            .prepare(
                "SELECT o.id, o.automation_id FROM automation_occurrences AS o
                 WHERE o.state = 'claimed'
                   AND o.lease_owner = 'daemon'
                   AND o.lease_expires_at IS NOT NULL
                   AND o.lease_expires_at > ?1
                   AND (?2 IS NULL OR o.scheduler_generation = ?2)
                   AND (
                       NOT EXISTS (
                           SELECT 1 FROM automation_runs AS r
                           WHERE r.occurrence_id = o.id
                       )
                       OR EXISTS (
                           SELECT 1
                           FROM automation_runs AS r
                           JOIN automation_attempts AS a ON a.run_id = r.id
                           WHERE r.occurrence_id = o.id
                             AND r.status = 'running'
                             AND a.state = 'adopted'
                             AND a.not_before <= ?1
                       )
                   )
                 ORDER BY o.scheduled_for ASC",
            )
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let rows = statement
            .query_map(
                rusqlite::params![now_iso, scheduler_fence.map(|fence| fence.generation())],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|error| format!("failed to read claim: {error}"))?);
        }
        out
    };

    for (occurrence_id, automation_id) in claimed {
        if cancelled() {
            restore_unlaunched_daemon_claims_for_retry_inner(
                conn,
                clock().max(now),
                scheduler_fence,
            )?;
            break;
        }
        let check_now = clock().max(now);
        match scheduler_fence {
            Some(fence) => {
                recover_expired_leases_with_scheduler_fence(conn, check_now, fence)?;
            }
            None => {
                recover_expired_leases(conn, check_now)?;
            }
        }
        let dispatchable: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM automation_occurrences AS occurrence
                    WHERE occurrence.id = ?1
                      AND occurrence.state = 'claimed'
                      AND occurrence.lease_owner = 'daemon'
                      AND occurrence.lease_expires_at IS NOT NULL
                      AND occurrence.lease_expires_at > ?2
                      AND (
                          ?3 IS NULL
                          OR (
                              occurrence.scheduler_generation = ?3
                              AND EXISTS (
                                  SELECT 1
                                  FROM automation_scheduler_authority
                                  WHERE id = 1
                                    AND owner_id = ?4
                                    AND generation = ?3
                              )
                          )
                      )
                )",
                rusqlite::params![
                    occurrence_id,
                    check_now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    scheduler_fence.map(|fence| fence.generation()),
                    scheduler_fence.map(|fence| fence.owner_id()),
                ],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to verify occurrence claim: {error}"))?;
        if !dispatchable {
            continue;
        }
        let definition =
            match load_definition_for_occurrence_dispatch(conn, &automation_id, &occurrence_id) {
                Ok(Some(definition)) => definition,
                Ok(None) => {
                    let reason = format!("routine `{automation_id}` vanished during dispatch");
                    settle_dispatch_occurrence(
                        conn,
                        &occurrence_id,
                        Some(&reason),
                        check_now,
                        scheduler_fence,
                    )?;
                    report.failed.push(reason);
                    continue;
                }
                Err(reason) => {
                    settle_dispatch_occurrence(
                        conn,
                        &occurrence_id,
                        Some(&reason),
                        check_now,
                        scheduler_fence,
                    )?;
                    report.failed.push(reason);
                    continue;
                }
            };

        let Some(cwd) = definition
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
        else {
            let reason = format!("{automation_id}: routine has no cwd; add a cwd before running");
            settle_dispatch_occurrence(
                conn,
                &occurrence_id,
                Some(&reason),
                check_now,
                scheduler_fence,
            )?;
            report.failed.push(reason);
            continue;
        };

        let dispatch_now = clock().max(check_now);
        if let Some(run_id) = expired_waiting_retry_run(conn, &occurrence_id, dispatch_now)? {
            settle_waiting_retry_timeout(
                conn,
                &run_id,
                &occurrence_id,
                dispatch_now,
                scheduler_fence,
            )?;
            report.failed.push(format!(
                "{automation_id}: automation retry wait exceeded run timeout"
            ));
            continue;
        }
        let mut control = DispatchControl {
            clock: &mut clock,
            cancelled: &cancelled,
            authority: AutomationAuthorityMode::BaseV1,
            scheduler_fence,
        };
        match dispatch_occurrence_with_clock(
            conn,
            runtime,
            &definition,
            &occurrence_id,
            cwd,
            dispatch_now,
            &mut control,
        ) {
            Ok(DispatchAttempt::Completed(outcome)) if outcome.status == "running" => {
                report.dispatched.push(outcome.run_id);
            }
            Ok(DispatchAttempt::Completed(outcome)) if outcome.status == "retry_scheduled" => {}
            Ok(DispatchAttempt::Completed(outcome)) => {
                report.failed.push(format!(
                    "{automation_id}: {}",
                    outcome
                        .error
                        .unwrap_or_else(|| "launch did not enter running state".to_string())
                ));
            }
            Ok(DispatchAttempt::Deferred) => {
                restore_unlaunched_daemon_claims_for_retry_inner(
                    conn,
                    clock().max(dispatch_now),
                    scheduler_fence,
                )?;
                break;
            }
            Err(reason) => {
                settle_dispatch_occurrence(
                    conn,
                    &occurrence_id,
                    Some("dispatch failed before runtime ownership"),
                    dispatch_now,
                    scheduler_fence,
                )?;
                report.failed.push(format!("{automation_id}: {reason}"));
            }
        }
    }

    Ok(report)
}

fn settle_dispatch_occurrence(
    conn: &Connection,
    occurrence_id: &str,
    failure_reason: Option<&str>,
    now: DateTime<Utc>,
    scheduler_fence: Option<&super::leadership::SchedulerFence>,
) -> Result<bool, String> {
    let Some(fence) = scheduler_fence else {
        return settle_occurrence(conn, occurrence_id, "failed", failure_reason, now);
    };
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin fenced occurrence settlement: {error}"))?;
    require_current_scheduler_fence(&transaction, occurrence_id, fence)?;
    let settled = settle_occurrence(&transaction, occurrence_id, "failed", failure_reason, now)?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit fenced occurrence settlement: {error}"))?;
    Ok(settled)
}

fn require_current_scheduler_fence(
    conn: &Connection,
    occurrence_id: &str,
    fence: &super::leadership::SchedulerFence,
) -> Result<(), String> {
    let current: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM automation_scheduler_authority AS authority
                JOIN automation_occurrences AS occurrence
                  ON occurrence.id = ?3
                WHERE authority.id = 1
                  AND authority.owner_id = ?1
                  AND authority.generation = ?2
                  AND occurrence.scheduler_generation = ?2
            )",
            rusqlite::params![fence.owner_id(), fence.generation(), occurrence_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to verify automations scheduler fence: {error}"))?;
    if !current {
        return Err("automations scheduler fence is stale".to_string());
    }
    Ok(())
}

pub(crate) fn restore_unlaunched_daemon_claims_for_retry(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<usize, String> {
    restore_unlaunched_daemon_claims_for_retry_inner(conn, now, None)
}

fn restore_unlaunched_daemon_claims_for_retry_inner(
    conn: &Connection,
    now: DateTime<Utc>,
    scheduler_fence: Option<&super::leadership::SchedulerFence>,
) -> Result<usize, String> {
    let restored = conn
        .execute(
            "UPDATE automation_occurrences
         SET state = 'planned',
             lease_owner = NULL,
             lease_expires_at = NULL,
             failure_reason = NULL,
             updated_at = ?1
         WHERE state = 'claimed'
           AND lease_owner = 'daemon'
           AND (
               ?2 IS NULL
               OR (
                   scheduler_generation = ?2
                   AND EXISTS (
                       SELECT 1
                       FROM automation_scheduler_authority
                       WHERE id = 1 AND owner_id = ?3 AND generation = ?2
                   )
               )
           )
           AND (
               NOT EXISTS (
                   SELECT 1 FROM automation_runs
                   WHERE automation_runs.occurrence_id = automation_occurrences.id
               )
               OR EXISTS (
                   SELECT 1
                   FROM automation_runs AS retry_run
                   JOIN automation_attempts AS retry_attempt
                     ON retry_attempt.run_id = retry_run.id
                   WHERE retry_run.occurrence_id = automation_occurrences.id
                     AND retry_run.status = 'running'
                     AND retry_run.session_id IS NULL
                     AND retry_attempt.state = 'adopted'
               )
           )",
            rusqlite::params![
                now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                scheduler_fence.map(|fence| fence.generation()),
                scheduler_fence.map(|fence| fence.owner_id()),
            ],
        )
        .map_err(|error| format!("failed to restore unlaunched daemon claims: {error}"))?;
    if restored == 0 {
        if let Some(fence) = scheduler_fence {
            if !fence.is_current(conn).map_err(|error| {
                format!("failed to verify automations scheduler fence: {error:#}")
            })? {
                return Err("automations scheduler fence is stale".to_string());
            }
        }
    }
    Ok(restored)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SettlementReport {
    pub succeeded: usize,
    pub failed: usize,
    pub cancelled: usize,
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
                   AND o.lease_expires_at <= ?1
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
        match claim_stop_fence(conn, &run_id, &session_id, "recovery", None, None, now)? {
            StopFenceClaim::Acquired => {}
            StopFenceClaim::InProgress | StopFenceClaim::Conflict => continue,
            StopFenceClaim::UnknownOutcome => {
                mark_unconfirmed_stop_for_recovery(
                    conn,
                    &run_id,
                    "abandoned launch stop outcome was not durably recorded",
                    now,
                )?;
                failures.push(format!(
                    "abandoned automation run `{run_id}` has an unknown prior stop outcome for session `{session_id}`"
                ));
                continue;
            }
        }
        if let Err(error) = runtime.kill_session(&session_id) {
            mark_unconfirmed_stop_for_recovery(
                conn,
                &run_id,
                "abandoned launch stop was not confirmed",
                now,
            )?;
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
        conn.execute(
            "DELETE FROM automation_stop_fences
             WHERE run_id = ?1 AND session_id = ?2 AND owner = 'recovery'",
            rusqlite::params![run_id, session_id],
        )
        .map_err(|error| format!("failed to release abandoned launch stop ownership: {error}"))?;
    }
    Ok(failures)
}

/// Requests termination for runs that exceeded their definition's wall-clock
/// budget. A successful strict kill is persisted as terminal session evidence;
/// an unproven kill remains running and is returned for daemon diagnostics.
fn expired_waiting_retry_run(
    conn: &Connection,
    occurrence_id: &str,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    let candidate = conn
        .query_row(
            "SELECT r.id, r.timeout_at
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id
             WHERE r.occurrence_id = ?1
               AND r.status = 'running'
               AND r.session_id IS NULL
               AND r.timeout_at IS NOT NULL
               AND a.state = 'adopted'",
            [occurrence_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to inspect waiting retry deadline: {error}"))?;
    let Some((run_id, timeout_at)) = candidate else {
        return Ok(None);
    };
    let timeout_at = DateTime::parse_from_rfc3339(&timeout_at)
        .map_err(|error| format!("run `{run_id}` has invalid timeout_at: {error}"))?
        .with_timezone(&Utc);
    Ok((timeout_at <= now).then_some(run_id))
}

fn settle_waiting_retry_timeout(
    conn: &Connection,
    run_id: &str,
    occurrence_id: &str,
    now: DateTime<Utc>,
    scheduler_fence: Option<&super::leadership::SchedulerFence>,
) -> Result<(), String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin retry timeout settlement: {error}"))?;
    if let Some(fence) = scheduler_fence {
        require_current_scheduler_fence(&transaction, occurrence_id, fence)?;
    }
    settle_waiting_retry_timeout_in(&transaction, run_id, occurrence_id, now)?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit retry timeout settlement: {error}"))
}

fn settle_waiting_retry_timeout_in(
    conn: &Connection,
    run_id: &str,
    occurrence_id: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let reason = "automation retry wait exceeded run timeout";
    let settled_attempt = conn
        .execute(
            "UPDATE automation_attempts
             SET state = 'timed_out',
                 failure_class = 'timeout',
                 state_reason = ?2,
                 settled_at = ?3
             WHERE run_id = ?1 AND state = 'adopted'",
            rusqlite::params![run_id, reason, now_iso],
        )
        .map_err(|error| format!("failed to settle timed-out retry attempt: {error}"))?;
    if settled_attempt != 1 {
        return Err(format!(
            "automation run `{run_id}` has no single waiting retry attempt to time out"
        ));
    }
    let settled_occurrence = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'failed',
                 failure_reason = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?3
             WHERE id = ?1 AND state IN ('planned', 'claimed')",
            rusqlite::params![occurrence_id, reason, now_iso],
        )
        .map_err(|error| format!("failed to settle timed-out retry occurrence: {error}"))?;
    if settled_occurrence != 1 {
        return Err(format!(
            "automation occurrence `{occurrence_id}` changed during retry timeout settlement"
        ));
    }
    if !record_run_finish(
        conn,
        run_id,
        RunFinish {
            status: "failed",
            exit_code: None,
            session_id: None,
            log_json: None,
            output_commit: None,
        },
        now,
    )
    .map_err(|error| format!("failed to settle timed-out retry run: {error:#}"))?
    {
        return Err(format!(
            "automation run `{run_id}` changed during retry timeout settlement"
        ));
    }
    Ok(())
}

pub fn enforce_run_timeouts(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<Vec<String>, String> {
    let waiting_candidates: Vec<(String, String)> = {
        let mut statement = conn
            .prepare(
                "SELECT r.id, r.occurrence_id, r.timeout_at
                 FROM automation_runs AS r
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.status = 'running'
                   AND r.session_id IS NULL
                   AND r.timeout_at IS NOT NULL
                   AND a.state = 'adopted'",
            )
            .map_err(|error| format!("failed to prepare retry timeout query: {error}"))?;
        let mapped = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| format!("failed to query retry timeouts: {error}"))?;
        let mut candidates = Vec::new();
        for row in mapped {
            let (run_id, occurrence_id, timeout_at) =
                row.map_err(|error| format!("failed to read retry timeout row: {error}"))?;
            let timeout_at = DateTime::parse_from_rfc3339(&timeout_at)
                .map_err(|error| format!("run `{run_id}` has invalid timeout_at: {error}"))?
                .with_timezone(&Utc);
            if timeout_at <= now {
                candidates.push((run_id, occurrence_id));
            }
        }
        candidates
    };
    for (run_id, occurrence_id) in waiting_candidates {
        settle_waiting_retry_timeout(conn, &run_id, &occurrence_id, now, None)?;
    }

    let candidates: Vec<(String, String, String)> = {
        let mut statement = conn
            .prepare(
                "SELECT r.id, r.session_id, r.timeout_at, r.automation_id
                 FROM automation_runs AS r
                 JOIN sessions AS s ON s.id = r.session_id
                 JOIN automation_attempts AS a
                   ON a.run_id = r.id
                  AND (
                      a.session_id = r.session_id
                      OR (a.state = 'dispatching' AND a.session_id IS NULL)
                  )
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 WHERE r.status = 'running'
                   AND s.status IN ('created', 'running', 'orphaned')
                   AND a.state IN ('dispatching', 'started', 'observing')
                   AND o.state IN ('claimed', 'running')
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
        if let Some(failure) =
            enforce_timeout_candidate(conn, runtime, &run_id, &session_id, &definition_name, now)?
        {
            failures.push(failure);
        }
    }
    Ok(failures)
}

pub(crate) fn enforce_run_timeout(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    run_id: &str,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    let candidate = conn
        .query_row(
            "SELECT r.session_id, r.timeout_at, r.automation_id
             FROM automation_runs AS r
             JOIN sessions AS s ON s.id = r.session_id
             JOIN automation_attempts AS a
               ON a.run_id = r.id
              AND (
                  a.session_id = r.session_id
                  OR (a.state = 'dispatching' AND a.session_id IS NULL)
              )
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id
             WHERE r.id = ?1
               AND r.status = 'running'
               AND s.status IN ('created', 'running', 'orphaned')
               AND a.state IN ('dispatching', 'started', 'observing')
               AND o.state IN ('claimed', 'running')
               AND r.timeout_at IS NOT NULL",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to query exact automation timeout: {error}"))?;
    let Some((session_id, timeout_at, automation_id)) = candidate else {
        return Ok(None);
    };
    let timeout_at = DateTime::parse_from_rfc3339(&timeout_at)
        .map_err(|error| format!("run `{run_id}` has invalid timeout_at: {error}"))?
        .with_timezone(&Utc);
    if timeout_at > now {
        return Ok(None);
    }
    enforce_timeout_candidate(conn, runtime, run_id, &session_id, &automation_id, now)
}

fn enforce_timeout_candidate(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    run_id: &str,
    session_id: &str,
    automation_id: &str,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    match claim_stop_fence(conn, run_id, session_id, "timeout", None, None, now)? {
        StopFenceClaim::Acquired => {}
        StopFenceClaim::InProgress | StopFenceClaim::Conflict => return Ok(None),
        StopFenceClaim::UnknownOutcome => {
            mark_unconfirmed_stop_for_recovery(
                conn,
                run_id,
                "prior timeout stop outcome was not durably recorded",
                now,
            )?;
            return Ok(Some(format!(
                "automation `{automation_id}` run `{run_id}` has an unknown prior timeout stop outcome for session `{session_id}`"
            )));
        }
    }
    if let Err(error) = runtime.kill_session(session_id) {
        mark_unconfirmed_stop_for_recovery(conn, run_id, "timeout stop was not confirmed", now)?;
        return Ok(Some(format!(
            "automation `{automation_id}` run `{run_id}` exceeded its timeout, but session `{session_id}` termination is unproven: {error:#}"
        )));
    }
    settle_confirmed_stop(conn, run_id, session_id, ConfirmedStop::TimedOut, now)?;
    Ok(None)
}

pub(crate) enum ConfirmedStop {
    Cancelled,
    TimedOut,
}

pub(crate) enum StopFenceClaim {
    Acquired,
    InProgress,
    UnknownOutcome,
    Conflict,
}

pub(crate) fn claim_stop_fence(
    conn: &Connection,
    run_id: &str,
    session_id: &str,
    owner: &str,
    operation_key: Option<&str>,
    cancellation_execution_expires_at: Option<&str>,
    now: DateTime<Utc>,
) -> Result<StopFenceClaim, String> {
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin automation stop reservation: {error}"))?;
    let acquired_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let execution_expires_at =
        (now + chrono::Duration::seconds(30)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let inserted = transaction
        .execute(
            "INSERT INTO automation_stop_fences (
                run_id, session_id, owner, operation_key, acquired_at, execution_expires_at
             )
             SELECT r.id, r.session_id, ?3, ?4, ?5, ?6
             FROM automation_runs AS r
             JOIN sessions AS s ON s.id = r.session_id
             JOIN automation_attempts AS a
               ON a.run_id = r.id
              AND (
                  a.session_id = r.session_id
                  OR (a.state = 'dispatching' AND a.session_id IS NULL)
              )
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id
             WHERE r.id = ?1
               AND r.session_id = ?2
               AND r.status = 'running'
               AND s.status IN ('created', 'running', 'orphaned')
               AND a.state IN ('dispatching', 'started', 'observing')
               AND o.state IN ('claimed', 'running')
               AND (
                   (
                       ?3 = 'cancellation'
                       AND EXISTS (
                           SELECT 1
                           FROM automation_cancellations AS c
                           WHERE c.run_id = r.id
                             AND c.session_id = r.session_id
                             AND c.adoption_key = ?4
                             AND c.state = 'requested'
                             AND c.execution_expires_at = ?7
                             AND c.execution_expires_at > ?8
                             AND (r.timeout_at IS NULL OR r.timeout_at > ?8)
                       )
                   )
                   OR (
                       ?3 != 'cancellation'
                       AND NOT EXISTS (
                           SELECT 1
                           FROM automation_cancellations AS c
                           WHERE c.run_id = r.id
                             AND c.state IN ('requested', 'stopping')
                       )
                   )
               )
             ON CONFLICT(run_id) DO NOTHING",
            rusqlite::params![
                run_id,
                session_id,
                owner,
                operation_key,
                acquired_at,
                execution_expires_at,
                cancellation_execution_expires_at,
                acquired_at,
            ],
        )
        .map_err(|error| format!("failed to reserve automation stop ownership: {error}"))?;
    if inserted == 1 {
        if owner == "cancellation" {
            let transitioned = transaction
                .execute(
                    "UPDATE automation_cancellations
                     SET state = 'stopping'
                     WHERE adoption_key = ?1
                       AND run_id = ?2
                       AND session_id = ?3
                       AND state = 'requested'
                       AND execution_expires_at = ?4
                       AND execution_expires_at > ?5",
                    rusqlite::params![
                        operation_key,
                        run_id,
                        session_id,
                        cancellation_execution_expires_at,
                        acquired_at
                    ],
                )
                .map_err(|error| {
                    format!("failed to persist cancellation stop ownership: {error}")
                })?;
            if transitioned != 1 {
                transaction.rollback().map_err(|error| {
                    format!("failed to roll back stale cancellation stop ownership: {error}")
                })?;
                return Ok(StopFenceClaim::Conflict);
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("failed to commit automation stop ownership: {error}"))?;
        return Ok(StopFenceClaim::Acquired);
    }

    let existing = transaction
        .query_row(
            "SELECT session_id, owner, operation_key, execution_expires_at
             FROM automation_stop_fences WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to inspect automation stop ownership: {error}"))?;
    let Some((existing_session, existing_owner, existing_key, expires_at)) = existing else {
        transaction.commit().map_err(|error| {
            format!("failed to close automation stop ownership lookup: {error}")
        })?;
        return Ok(StopFenceClaim::Conflict);
    };
    if existing_session != session_id
        || existing_owner != owner
        || existing_key.as_deref() != operation_key
    {
        transaction.commit().map_err(|error| {
            format!("failed to close automation stop ownership conflict: {error}")
        })?;
        return Ok(StopFenceClaim::Conflict);
    }
    let parsed_expires_at = DateTime::parse_from_rfc3339(&expires_at)
        .map_err(|error| format!("stored automation stop lease is invalid: {error}"))?
        .with_timezone(&Utc);
    if parsed_expires_at <= now {
        let reclaimed = transaction
            .execute(
                "UPDATE automation_stop_fences
                 SET execution_expires_at = ?2
                 WHERE run_id = ?1 AND execution_expires_at = ?3",
                rusqlite::params![run_id, execution_expires_at, expires_at],
            )
            .map_err(|error| format!("failed to reclaim automation stop recovery: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit automation stop recovery: {error}"))?;
        if reclaimed == 1 {
            Ok(StopFenceClaim::UnknownOutcome)
        } else {
            Ok(StopFenceClaim::InProgress)
        }
    } else {
        transaction.commit().map_err(|error| {
            format!("failed to close automation stop ownership lookup: {error}")
        })?;
        Ok(StopFenceClaim::InProgress)
    }
}

pub(crate) fn settle_confirmed_stop(
    conn: &Connection,
    run_id: &str,
    session_id: &str,
    disposition: ConfirmedStop,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let (session_status, aggregate_state, attempt_state, failure_class, state_reason, exit_code) =
        match disposition {
            ConfirmedStop::Cancelled => (
                "cancelled",
                "cancelled",
                "cancelled",
                "cancelled",
                "automation run cancelled by operator",
                None,
            ),
            ConfirmedStop::TimedOut => (
                "killed",
                "failed",
                "timed_out",
                "timeout",
                "automation run timed out",
                None,
            ),
        };
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin automation stop settlement: {error}"))?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    if !crate::store::update_session_terminal_if_active(
        &transaction,
        session_id,
        session_status,
        exit_code.and_then(|code| i32::try_from(code).ok()),
        &now_iso,
    )
    .map_err(|error| format!("failed to settle automation session `{session_id}`: {error:#}"))?
    {
        transaction
            .rollback()
            .map_err(|error| format!("failed to roll back losing stop settlement: {error}"))?;
        return Ok(false);
    }

    let attempt_changed = transaction
        .execute(
            "UPDATE automation_attempts
             SET state = ?2,
                 failure_class = ?3,
                 state_reason = ?4,
                 settled_at = ?5,
                 session_id = COALESCE(session_id, ?6)
             WHERE run_id = ?1
               AND state IN ('dispatching', 'started', 'observing')
               AND (
                   session_id = ?6
                   OR (state = 'dispatching' AND session_id IS NULL)
               )",
            rusqlite::params![
                run_id,
                attempt_state,
                failure_class,
                state_reason,
                now_iso,
                session_id
            ],
        )
        .map_err(|error| format!("failed to settle stopped automation attempt: {error}"))?;
    if attempt_changed != 1 {
        transaction
            .rollback()
            .map_err(|error| format!("failed to roll back incomplete stop settlement: {error}"))?;
        return Err(format!(
            "automation run `{run_id}` no longer has exactly one active attempt for session `{session_id}`"
        ));
    }

    if !record_run_finish(
        &transaction,
        run_id,
        RunFinish {
            status: aggregate_state,
            exit_code,
            session_id: Some(session_id.to_string()),
            log_json: None,
            output_commit: None,
        },
        now,
    )
    .map_err(|error| format!("failed to settle stopped automation run: {error:#}"))?
    {
        transaction
            .rollback()
            .map_err(|error| format!("failed to roll back incomplete run stop: {error}"))?;
        return Ok(false);
    }
    let occurrence_id: String = transaction
        .query_row(
            "SELECT occurrence_id FROM automation_runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to resolve stopped run occurrence: {error}"))?;
    if !settle_occurrence(
        &transaction,
        &occurrence_id,
        aggregate_state,
        Some(state_reason),
        now,
    )? {
        transaction
            .rollback()
            .map_err(|error| format!("failed to roll back incomplete occurrence stop: {error}"))?;
        return Err(format!(
            "automation occurrence `{occurrence_id}` was no longer active during stop settlement"
        ));
    }
    transaction
        .execute(
            "DELETE FROM automation_stop_fences
             WHERE run_id = ?1 AND session_id = ?2",
            rusqlite::params![run_id, session_id],
        )
        .map_err(|error| format!("failed to release automation stop ownership: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit automation stop settlement: {error}"))?;
    Ok(true)
}

pub(crate) fn mark_unconfirmed_stop_for_recovery(
    conn: &Connection,
    run_id: &str,
    state_reason: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin ambiguous stop settlement: {error}"))?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let attempt_changed = transaction
        .execute(
            "UPDATE automation_attempts
             SET state = 'ambiguous',
                 failure_class = 'ambiguous_evidence',
                 state_reason = ?2,
                 settled_at = ?3
             WHERE run_id = ?1
               AND state IN ('dispatching', 'started', 'observing')
               AND (
                   session_id = (
                       SELECT r.session_id
                       FROM automation_runs AS r
                       JOIN sessions AS s ON s.id = r.session_id
                       WHERE r.id = ?1
                         AND r.status = 'running'
                         AND s.status IN ('created', 'running', 'orphaned')
                   )
                   OR (
                       state = 'dispatching'
                       AND session_id IS NULL
                       AND EXISTS (
                           SELECT 1
                           FROM automation_runs AS r
                           JOIN sessions AS s ON s.id = r.session_id
                           WHERE r.id = ?1
                             AND r.status = 'running'
                             AND s.status IN ('created', 'running', 'orphaned')
                       )
                   )
               )",
            rusqlite::params![run_id, state_reason, now_iso],
        )
        .map_err(|error| format!("failed to mark ambiguous automation attempt: {error}"))?;
    if attempt_changed != 1 {
        transaction
            .rollback()
            .map_err(|error| format!("failed to roll back ambiguous stop settlement: {error}"))?;
        return Err(format!(
            "automation run `{run_id}` no longer has exactly one active attempt"
        ));
    }

    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = 'recovery_required',
                 failure_reason = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?3
             WHERE id = (
                 SELECT occurrence_id FROM automation_runs WHERE id = ?1
             )
               AND state IN ('claimed', 'running')",
            rusqlite::params![run_id, state_reason, now_iso],
        )
        .map_err(|error| format!("failed to mark recovery-required occurrence: {error}"))?;
    if occurrence_changed != 1 {
        transaction
            .rollback()
            .map_err(|error| format!("failed to roll back ambiguous occurrence stop: {error}"))?;
        return Err(format!(
            "automation run `{run_id}` no longer has an active occurrence"
        ));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit ambiguous stop settlement: {error}"))
}

pub(crate) fn mark_terminal_stop_for_recovery(
    conn: &Connection,
    run_id: &str,
    session_id: &str,
    state_reason: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin terminal stop recovery: {error}"))?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let attempt_changed = transaction
        .execute(
            "UPDATE automation_attempts
             SET state = 'ambiguous',
                 failure_class = 'ambiguous_evidence',
                 state_reason = ?3,
                 settled_at = ?4
             WHERE run_id = ?1
               AND state IN ('dispatching', 'started', 'observing')
               AND (
                   session_id = ?2
                   OR (state = 'dispatching' AND session_id IS NULL)
               )
               AND EXISTS (
                   SELECT 1
                   FROM sessions
                   WHERE id = ?2
                     AND status IN ('completed', 'failed', 'cancelled', 'killed', 'idle')
               )",
            rusqlite::params![run_id, session_id, state_reason, now_iso],
        )
        .map_err(|error| format!("failed to preserve ambiguous terminal attempt: {error}"))?;
    if attempt_changed != 1 {
        transaction
            .rollback()
            .map_err(|error| format!("failed to roll back terminal stop recovery: {error}"))?;
        return Err(format!(
            "automation run `{run_id}` no longer has one active terminally observed attempt"
        ));
    }
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = 'recovery_required',
                 failure_reason = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?3
             WHERE id = (
                 SELECT occurrence_id FROM automation_runs WHERE id = ?1
             )
               AND state IN ('claimed', 'running')",
            rusqlite::params![run_id, state_reason, now_iso],
        )
        .map_err(|error| format!("failed to preserve terminal recovery occurrence: {error}"))?;
    if occurrence_changed != 1 {
        transaction.rollback().map_err(|error| {
            format!("failed to roll back terminal recovery occurrence: {error}")
        })?;
        return Err(format!(
            "automation run `{run_id}` no longer has an active occurrence for terminal recovery"
        ));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit terminal stop recovery: {error}"))
}

pub fn mark_active_attempts_for_restart_reconciliation(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<usize, String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin shutdown reconciliation: {error}"))?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let reason = "daemon shutdown requires restart reconciliation";
    let attempts = transaction
        .execute(
            "UPDATE automation_attempts
             SET state_reason = ?1
             WHERE state IN ('dispatching', 'started', 'observing')
               AND run_id IN (
                   SELECT id FROM automation_runs WHERE status = 'running'
               )",
            rusqlite::params![reason],
        )
        .map_err(|error| format!("failed to mark active attempts for reconciliation: {error}"))?;
    transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = 'recovery_required',
                 failure_reason = ?1,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?2
             WHERE state IN ('claimed', 'running')
               AND id IN (
                   SELECT occurrence_id
                   FROM automation_runs
                   WHERE status = 'running'
               )",
            rusqlite::params![reason, now_iso],
        )
        .map_err(|error| {
            format!("failed to mark active occurrences for restart reconciliation: {error}")
        })?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit shutdown reconciliation: {error}"))?;
    Ok(attempts)
}

/// Reconciles nonterminal automation rows against the authoritative session
/// ledger. A completed session with an explicit zero exit code produces
/// success, acknowledged cancellation remains distinct, and every other
/// terminal session disposition is a failure.
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
                        ),
                        (
                            SELECT a.state
                            FROM automation_attempts AS a
                            WHERE a.run_id = r.id
                            ORDER BY a.attempt_number DESC
                            LIMIT 1
                        ),
                        (
                            SELECT a.settled_at
                            FROM automation_attempts AS a
                            WHERE a.run_id = r.id
                            ORDER BY a.attempt_number DESC
                            LIMIT 1
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
                    row.get(8)?,
                    row.get(9)?,
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
        attempt_state,
        attempt_settled_at,
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

        let ambiguous = attempt_state.as_deref() == Some("ambiguous");
        let timed_out = !ambiguous
            && match (timeout_at.as_deref(), terminal_at.as_deref()) {
                (Some(timeout_at), Some(terminal_at)) => {
                    let timeout_at = DateTime::parse_from_rfc3339(timeout_at).map_err(|error| {
                        format!("run `{run_id}` has invalid timeout_at: {error}")
                    })?;
                    let terminal_at = DateTime::parse_from_rfc3339(terminal_at).map_err(|error| {
                    format!("session for run `{run_id}` has invalid terminal timestamp: {error}")
                })?;
                    terminal_at >= timeout_at
                }
                _ => false,
            };
        let succeeded = !ambiguous
            && session_status.as_deref() == Some("completed")
            && exit_code == Some(0)
            && !timed_out;
        let cancelled = !ambiguous && session_status.as_deref() == Some("cancelled") && !timed_out;
        let status = if succeeded {
            "succeeded"
        } else if cancelled {
            "cancelled"
        } else {
            "failed"
        };
        let attempt_status = if timed_out { "timed_out" } else { status };
        let reason = if ambiguous {
            Some("terminal session evidence observed after an ambiguous stop outcome".to_string())
        } else if succeeded {
            None
        } else if timed_out {
            Some(match session_status.as_deref() {
                Some("completed") => "session completed after automation timeout".to_string(),
                Some(session_status) => {
                    format!("session {session_status} at or after automation timeout")
                }
                None => unreachable!("terminal_session requires a session status"),
            })
        } else if cancelled {
            Some("session cancellation acknowledged".to_string())
        } else {
            Some(match (&session_status, exit_code) {
                (Some(session_status), Some(exit_code)) => {
                    format!("session {session_status} (exit code {exit_code})")
                }
                (Some(session_status), None) => format!("session {session_status}"),
                (None, _) => unreachable!("terminal_session requires a session status"),
            })
        };
        let settlement_time = if ambiguous {
            let settled_at = attempt_settled_at.as_deref().ok_or_else(|| {
                format!("ambiguous attempt for run `{run_id}` has no settlement timestamp")
            })?;
            DateTime::parse_from_rfc3339(settled_at)
                .map_err(|error| {
                    format!("ambiguous attempt for run `{run_id}` has invalid settled_at: {error}")
                })?
                .with_timezone(&Utc)
        } else {
            now
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
            if matches!(
                occurrence_state.as_deref(),
                Some("succeeded" | "failed" | "cancelled")
            ) {
                return Err(format!(
                    "automation occurrence `{occurrence_id}` is already `{}` but terminal session evidence requires `{status}`",
                    occurrence_state.as_deref().unwrap_or("missing")
                ));
            }
            if !settle_occurrence(
                &transaction,
                occurrence_id,
                status,
                reason.as_deref(),
                settlement_time,
            )? {
                return Err(format!(
                    "automation occurrence `{occurrence_id}` changed during settlement"
                ));
            }
        }
        let attempt_settled = if attempt_state.as_deref() == Some("ambiguous") {
            1
        } else {
            transaction
                .execute(
                    "UPDATE automation_attempts
                 SET state = ?2,
                     state_reason = ?3,
                     settled_at = ?4,
                     failure_class = CASE
                         WHEN ?2 = 'cancelled' THEN 'cancelled'
                         WHEN ?2 = 'timed_out' THEN 'timeout'
                         ELSE failure_class
                     END
                 WHERE run_id = ?1
                   AND state IN ('dispatching', 'started', 'observing')",
                    rusqlite::params![
                        run_id,
                        attempt_status,
                        reason,
                        settlement_time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    ],
                )
                .map_err(|error| {
                    format!("failed to settle current attempt for run `{run_id}`: {error}")
                })?
        };
        let attempt_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM automation_attempts WHERE run_id = ?1",
                [&run_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to inspect attempts for run `{run_id}`: {error}"))?;
        if attempt_count > 0 && attempt_settled != 1 {
            return Err(format!(
                "automation run `{run_id}` has no single active attempt to settle"
            ));
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
            settlement_time,
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
        } else if cancelled {
            report.cancelled += 1;
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
    definition
        .validate_durable()
        .map_err(|error| format!("stored routine `{id}` is invalid: {error}"))?;
    Ok(Some(definition))
}

fn load_definition_for_occurrence_dispatch(
    conn: &Connection,
    automation_id: &str,
    occurrence_id: &str,
) -> Result<Option<RoutineDefinition>, String> {
    type RunPin = (Option<String>, i64, Option<String>, i64, Option<String>);
    let snapshot: Option<RunPin> = conn
        .query_row(
            "SELECT r.definition_json, r.automation_revision, r.definition_digest,
                    o.automation_revision, o.definition_digest
             FROM automation_runs AS r
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id
             WHERE r.automation_id = ?1
               AND r.occurrence_id = ?2
               AND r.status = 'running'
             ORDER BY r.started_at DESC
             LIMIT 1",
            rusqlite::params![automation_id, occurrence_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to read run definition snapshot: {error}"))?;
    match snapshot {
        Some((
            Some(definition_json),
            run_revision,
            run_digest,
            occurrence_revision,
            occurrence_digest,
        )) => {
            if run_revision != occurrence_revision || run_digest != occurrence_digest {
                return Err(format!(
                    "running occurrence `{occurrence_id}` does not match its run definition pin"
                ));
            }
            let Some(run_digest) = run_digest else {
                return Err(format!(
                    "running occurrence `{occurrence_id}` has no pinned definition digest"
                ));
            };
            let snapshot_digest = super::contract::migration::definition_digest(&definition_json)
                .map_err(|error| {
                format!(
                    "failed to digest running occurrence `{occurrence_id}` definition: {error:#}"
                )
            })?;
            if snapshot_digest != run_digest {
                return Err(format!(
                    "running occurrence `{occurrence_id}` definition snapshot failed integrity"
                ));
            }
            let definition: RoutineDefinition = serde_json::from_str(&definition_json)
                .map_err(|error| format!("stored run definition is unreadable: {error}"))?;
            definition
                .validate_durable()
                .map_err(|error| format!("stored run definition is invalid: {error}"))?;
            if definition.id != automation_id {
                return Err(format!(
                    "stored run definition belongs to `{}`, not `{automation_id}`",
                    definition.id
                ));
            }
            Ok(Some(definition))
        }
        Some((None, _, _, _, _)) => Err(format!(
            "running occurrence `{occurrence_id}` has no durable definition snapshot"
        )),
        None => load_definition_for_run(conn, automation_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::contract::authority::{
        AuthorityEvidenceVerifier, AuthorityProfileError, AuthorityProfileErrorCode,
        AuthorityValidationPhase, AutomationAuthorityExtension, AUTHORITY_EXTENSION_KEY,
    };
    use crate::automations::contract::types::{BackoffPolicy, RetryableClass};
    use crate::automations::definition::{RoutineDefinition, RoutineRetryPolicy, RoutineStatus};
    use crate::automations::store::insert_definition;
    use crate::store::initialize_store;
    use chrono::TimeZone;
    use serde_json::json;
    use sha2::{Digest as _, Sha256};
    use std::collections::BTreeSet;

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

    struct UnavailableThenContainedRuntime {
        launches: std::sync::atomic::AtomicUsize,
    }

    impl SessionRuntime for UnavailableThenContainedRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            if self
                .launches
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                == 0
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "synthetic runtime unavailable",
                )
                .into());
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

    struct UnavailableRuntime;

    impl SessionRuntime for UnavailableRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "synthetic runtime unavailable",
            )
            .into())
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

    struct TimedOutRuntime;

    impl SessionRuntime for TimedOutRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            Err(
                std::io::Error::new(std::io::ErrorKind::TimedOut, "synthetic dispatch timeout")
                    .into(),
            )
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

    struct AdmissionClosedRuntime;

    impl SessionRuntime for AdmissionClosedRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            Err(anyhow::Error::new(
                crate::api::RuntimeLaunchAdmissionClosedError,
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

    struct UnavailableThenRecordingRuntime {
        launches: std::sync::atomic::AtomicUsize,
        prompts: std::sync::Mutex<Vec<String>>,
    }

    impl SessionRuntime for UnavailableThenRecordingRuntime {
        fn launch_session(&self, launch: &SessionLaunch) -> anyhow::Result<()> {
            self.prompts.lock().unwrap().push(launch.prompt.clone());
            if self
                .launches
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                == 0
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "synthetic runtime unavailable",
                )
                .into());
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

    struct AuthorityObservingRuntime<'a> {
        conn: &'a Connection,
    }

    impl SessionRuntime for AuthorityObservingRuntime<'_> {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch must use strict containment")
        }

        fn launch_contained_adopted_session(
            &self,
            launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            let (profile, extension): (Option<String>, Option<String>) = self.conn.query_row(
                "SELECT r.authority_profile, a.authority_extension_json
                 FROM automation_runs AS r
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.session_id = ?1 AND a.state = 'dispatching'",
                [&launch.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            anyhow::ensure!(
                profile.as_deref() == Some("coven.automations.authority.v1"),
                "runtime launch observed an unpinned authority profile"
            );
            anyhow::ensure!(
                extension.is_some(),
                "runtime launch observed an unpinned authority extension"
            );
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

    struct SupersedingPublishingRuntime<'a> {
        conn: &'a Connection,
    }

    impl SessionRuntime for SupersedingPublishingRuntime<'_> {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch must use strict containment")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            self.conn.execute(
                "UPDATE automation_scheduler_authority
                 SET owner_id = 'replacement-scheduler',
                     generation = generation + 1
                 WHERE id = 1",
                [],
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

    struct SupersedingRejectingRuntime<'a> {
        conn: &'a Connection,
    }

    impl SessionRuntime for SupersedingRejectingRuntime<'_> {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch must use strict containment")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            _ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            self.conn.execute(
                "UPDATE automation_scheduler_authority
                 SET owner_id = 'replacement-scheduler',
                     generation = generation + 1
                 WHERE id = 1",
                [],
            )?;
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "synthetic runtime unavailable",
            )
            .into())
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

    #[derive(Debug)]
    struct VectorAuthority;

    impl AutomationDispatchAuthority for VectorAuthority {
        fn resolve(
            &self,
            request: &AutomationAuthorityRequest,
        ) -> Result<crate::automations::contract::types::ExtensionBag, AuthorityProfileError>
        {
            assert_eq!(
                request.occurrence_key,
                format!("{}@manual-{}", request.automation_id, request.adoption_key)
            );
            const VECTORS: &str =
                include_str!("../../../../spec/coven-automations/authority/v1/test-vectors.json");
            let vectors: serde_json::Value =
                serde_json::from_str(VECTORS).expect("authority vectors");
            let mut binding = vectors["fixtures"]["binding"].clone();
            binding["base"]["automationId"] = json!(request.automation_id);
            binding["base"]["automationRevision"] = json!(request.automation_revision);
            binding["base"]["definitionDigest"]["value"] = json!(request.definition_digest);
            binding["base"]["occurrenceId"] = json!(request.occurrence_id);
            binding["base"]["occurrenceKey"] = json!(request.occurrence_key);
            binding["base"]["occurrenceFenceGeneration"] =
                json!(request.occurrence_fence_generation);
            binding["base"]["runId"] = json!(request.run_id);
            binding["base"]["attemptId"] = json!(request.attempt_id);
            binding["base"]["attemptNumber"] = json!(request.attempt_number);
            binding["base"]["adoptionKey"] = json!(request.adoption_key);
            binding["runtime"]["runtimeId"] = json!(request.runtime_id);
            binding["approval"]["use"]["occurrencePrefix"] = json!("occurrence.");
            binding["approval"]["consumption"]["occurrenceId"] = json!(request.occurrence_id);
            binding["approval"]["consumption"]["runId"] = json!(request.run_id);
            binding["approval"]["consumption"]["attemptNumber"] = json!(request.attempt_number);
            binding["approval"]["consumption"]["fenceGeneration"] =
                json!(request.occurrence_fence_generation);

            let mut body = binding.clone();
            let object = body.as_object_mut().expect("binding object");
            object.remove("integrity");
            object.remove("authentication");
            let canonical = serde_jcs::to_vec(&body).expect("canonical authority binding");
            let mut hasher = Sha256::new();
            hasher.update(b"opencoven:coven-automations-authority-binding:v1");
            hasher.update([0]);
            hasher.update(canonical);
            let digest = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            binding["integrity"]["value"] = json!(digest);
            binding["authentication"]["signedDigest"] = json!(digest);

            serde_json::from_value(json!({
                AUTHORITY_EXTENSION_KEY: {
                    "profile": "coven.automations.authority.v1",
                    "kind": "AutomationAuthorityExtension",
                    "executionBinding": binding,
                    "receiptEvidence": null
                }
            }))
            .map_err(|error| {
                AuthorityProfileError::new(
                    AuthorityProfileErrorCode::SchemaInvalid,
                    error.to_string(),
                )
            })
        }
    }

    impl AuthorityEvidenceVerifier for VectorAuthority {
        fn verify(
            &self,
            _extension: &AutomationAuthorityExtension,
            phase: AuthorityValidationPhase,
        ) -> Result<(), AuthorityProfileError> {
            assert_eq!(phase, AuthorityValidationPhase::PreDispatch);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct StaleAuthority;

    impl AutomationDispatchAuthority for StaleAuthority {
        fn resolve(
            &self,
            _request: &AutomationAuthorityRequest,
        ) -> Result<crate::automations::contract::types::ExtensionBag, AuthorityProfileError>
        {
            Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::Stale,
                "synthetic trusted-state detail that must not escape",
            ))
        }
    }

    impl AuthorityEvidenceVerifier for StaleAuthority {
        fn verify(
            &self,
            _extension: &AutomationAuthorityExtension,
            _phase: AuthorityValidationPhase,
        ) -> Result<(), AuthorityProfileError> {
            unreachable!("stale authority resolution must refuse before verification")
        }
    }

    #[derive(Debug)]
    struct MismatchedAuthority;

    impl AutomationDispatchAuthority for MismatchedAuthority {
        fn resolve(
            &self,
            request: &AutomationAuthorityRequest,
        ) -> Result<crate::automations::contract::types::ExtensionBag, AuthorityProfileError>
        {
            let mut mismatched = request.clone();
            mismatched.runtime_id = "different-runtime".to_string();
            VectorAuthority.resolve(&mismatched)
        }
    }

    impl AuthorityEvidenceVerifier for MismatchedAuthority {
        fn verify(
            &self,
            _extension: &AutomationAuthorityExtension,
            _phase: AuthorityValidationPhase,
        ) -> Result<(), AuthorityProfileError> {
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
    fn stale_scheduler_generation_cannot_dispatch_after_takeover() {
        let (temp, conn) = temp_store();
        let mut routine = definition("stale-scheduler-dispatch");
        routine.status = RoutineStatus::Active;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 9, 30, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?2, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![
                routine.id,
                (now - chrono::Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();
        let first = super::super::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now)
            .unwrap();
        super::super::occurrences::tick_with_scheduler_fence(&conn, now, &first.fence()).unwrap();
        let stale_fence = first.fence();
        drop(first);
        let _second = super::super::leadership::SchedulerLeadership::acquire(
            temp.path(),
            &conn,
            now + chrono::Duration::seconds(1),
        )
        .unwrap();

        let error = dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            &conn,
            &crate::api::NoopSessionRuntime,
            now + chrono::Duration::seconds(1),
            || now + chrono::Duration::seconds(1),
            || false,
            &stale_fence,
        )
        .unwrap_err();

        assert!(error.contains("scheduler fence is stale"), "{error}");
        let session_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(session_count, 0);
    }

    #[test]
    fn superseded_scheduler_cannot_publish_runtime_ownership() {
        let (temp, conn) = temp_store();
        let mut routine = definition("stale-scheduler-publication");
        routine.status = RoutineStatus::Active;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 9, 30, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?2, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![
                routine.id,
                (now - chrono::Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();
        let leadership =
            super::super::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now)
                .unwrap();
        super::super::occurrences::tick_with_scheduler_fence(&conn, now, &leadership.fence())
            .unwrap();

        let error = dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            &conn,
            &SupersedingPublishingRuntime { conn: &conn },
            now,
            || now,
            || false,
            &leadership.fence(),
        )
        .unwrap_err();

        assert!(error.contains("scheduler fence is stale"), "{error}");
        let (occurrence_state, attempt_state, session_status): (String, String, String) = conn
            .query_row(
                "SELECT o.state, a.state, s.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 JOIN sessions AS s ON s.id = r.session_id
                 WHERE o.automation_id = ?1",
                [&routine.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (
                occurrence_state.as_str(),
                attempt_state.as_str(),
                session_status.as_str()
            ),
            ("claimed", "dispatching", "created")
        );
    }

    #[test]
    fn superseded_scheduler_cannot_settle_a_rejected_launch() {
        let (temp, conn) = temp_store();
        let mut routine = definition("stale-scheduler-settlement");
        routine.status = RoutineStatus::Active;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 9, 30, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?2, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![
                routine.id,
                (now - chrono::Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();
        let leadership =
            super::super::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now)
                .unwrap();
        super::super::occurrences::tick_with_scheduler_fence(&conn, now, &leadership.fence())
            .unwrap();

        let error = dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            &conn,
            &SupersedingRejectingRuntime { conn: &conn },
            now,
            || now,
            || false,
            &leadership.fence(),
        )
        .unwrap_err();

        assert!(error.contains("scheduler fence is stale"), "{error}");
        let (occurrence_state, attempt_state, session_status): (String, String, String) = conn
            .query_row(
                "SELECT o.state, a.state, s.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 JOIN sessions AS s ON s.id = r.session_id
                 WHERE o.automation_id = ?1",
                [&routine.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (
                occurrence_state.as_str(),
                attempt_state.as_str(),
                session_status.as_str()
            ),
            ("claimed", "dispatching", "created")
        );
    }

    #[test]
    fn rejected_launch_settlement_rechecks_the_scheduler_fence_in_transaction() {
        let (temp, conn) = temp_store();
        let mut routine = definition("atomic-stale-settlement");
        routine.status = RoutineStatus::Active;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 9, 30, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?2, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![
                routine.id,
                (now - chrono::Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();
        let leadership =
            super::super::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now)
                .unwrap();
        let report =
            super::super::occurrences::tick_with_scheduler_fence(&conn, now, &leadership.fence())
                .unwrap();
        let occurrence_id = report.claimed.first().unwrap();
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        let PersistLaunch::Ready(attempt) = persist_launch_with_clock(
            &conn,
            "atomic-stale-settlement-run",
            occurrence_id,
            &routine,
            &launch,
            PersistLaunchContext {
                not_before: now,
                authority: AutomationAuthorityMode::BaseV1,
                scheduler_fence: Some(&leadership.fence()),
            },
            || now,
        )
        .unwrap() else {
            panic!("scheduler claim should persist a dispatching attempt");
        };
        conn.execute(
            "UPDATE automation_scheduler_authority
             SET owner_id = 'replacement-scheduler',
                 generation = generation + 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let error = settle_rejected_launch(
            &conn,
            RejectedLaunch {
                occurrence_id,
                run_id: &attempt.run_id,
                attempt_number: attempt.attempt_number,
                session_id: &launch.id,
                definition: &routine,
                failure: PreownershipFailure::LaunchRefused,
                reason: "synthetic refusal",
                scheduler_fence: Some(&leadership.fence()),
            },
            now,
        )
        .unwrap_err();

        assert!(error.contains("scheduler fence is stale"), "{error}");
        let restore_error = restore_preownership_launch_for_retry(
            &conn,
            occurrence_id,
            &attempt.run_id,
            &launch.id,
            now,
            Some(&leadership.fence()),
        )
        .unwrap_err();
        assert!(
            restore_error.contains("scheduler fence is stale"),
            "{restore_error}"
        );
        let (occurrence_state, attempt_state, session_status): (String, String, String) = conn
            .query_row(
                "SELECT o.state, a.state, s.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 JOIN sessions AS s ON s.id = r.session_id
                 WHERE r.id = ?1",
                [&attempt.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (
                occurrence_state.as_str(),
                attempt_state.as_str(),
                session_status.as_str()
            ),
            ("claimed", "dispatching", "created")
        );
    }

    fn persisted_timeout_at(conn: &Connection, automation_id: &str) -> DateTime<Utc> {
        let timeout_at = super::super::runs::list_runs(conn, automation_id, 1)
            .unwrap()
            .remove(0)
            .timeout_at
            .expect("running automation has a timeout");
        DateTime::parse_from_rfc3339(&timeout_at)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn persist_launch_at(
        conn: &Connection,
        run_id: &str,
        occurrence_id: &str,
        definition: &RoutineDefinition,
        launch: &SessionLaunch,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        persist_launch_with_clock(
            conn,
            run_id,
            occurrence_id,
            definition,
            launch,
            PersistLaunchContext {
                not_before: now,
                authority: AutomationAuthorityMode::BaseV1,
                scheduler_fence: None,
            },
            || now,
        )
        .map(|_| ())
    }

    #[test]
    fn runtime_authority_is_pinned_before_runtime_launch() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily-notes");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        let occurrence_id = "occurrence.daily-notes-20260903";
        assert!(
            insert_claimed_occurrence(&conn, occurrence_id, &routine.id, "daemon", 60, now,)
                .unwrap()
        );
        let runtime = AuthorityObservingRuntime { conn: &conn };
        let mut clock = || now;
        let cancelled = || false;
        let mut control = DispatchControl {
            clock: &mut clock,
            cancelled: &cancelled,
            authority: AutomationAuthorityMode::RuntimeAuthority(&VectorAuthority),
            scheduler_fence: None,
        };

        let dispatch = dispatch_occurrence_with_clock(
            &conn,
            &runtime,
            &routine,
            occurrence_id,
            routine.cwd.as_deref().unwrap(),
            now,
            &mut control,
        )
        .unwrap();
        let DispatchAttempt::Completed(outcome) = dispatch else {
            panic!("authority-bound dispatch must not be deferred");
        };

        assert_eq!(outcome.status, "running");
        let (profile, extension): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT r.authority_profile, a.authority_extension_json
                 FROM automation_runs AS r
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(profile.as_deref(), Some("coven.automations.authority.v1"));
        assert!(extension.is_some());
    }

    #[test]
    fn invalid_runtime_authority_rolls_back_before_launch() {
        for (authority, expected_code) in [
            (
                &StaleAuthority as &dyn AutomationDispatchAuthority,
                "AUTHORITY_STALE",
            ),
            (
                &MismatchedAuthority as &dyn AutomationDispatchAuthority,
                "AUTHORITY_BINDING_MISMATCH",
            ),
        ] {
            let (_temp, conn) = temp_store();
            let routine = definition("daily-notes");
            insert_definition(&conn, &routine).unwrap();
            let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
            let occurrence_id = "occurrence.daily-notes-refused";
            assert!(insert_claimed_occurrence(
                &conn,
                occurrence_id,
                &routine.id,
                "daemon",
                60,
                now,
            )
            .unwrap());
            let mut clock = || now;
            let cancelled = || false;
            let mut control = DispatchControl {
                clock: &mut clock,
                cancelled: &cancelled,
                authority: AutomationAuthorityMode::RuntimeAuthority(authority),
                scheduler_fence: None,
            };

            let result = dispatch_occurrence_with_clock(
                &conn,
                &ContainedRuntime,
                &routine,
                occurrence_id,
                routine.cwd.as_deref().unwrap(),
                now,
                &mut control,
            );
            let error = match result {
                Ok(_) => panic!("invalid Runtime Authority must fail closed"),
                Err(error) => error,
            };

            assert!(error.contains(expected_code), "{error}");
            assert!(
                !error.contains("synthetic trusted-state detail"),
                "authority errors must not disclose provider details"
            );
            let counts: (i64, i64, i64) = conn
                .query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM automation_runs),
                        (SELECT COUNT(*) FROM automation_attempts),
                        (SELECT COUNT(*) FROM sessions)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(counts, (0, 0, 0));
        }
    }

    #[test]
    fn pinned_runtime_authority_cannot_be_rewritten_or_deleted() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily-notes");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        let occurrence_id = "occurrence.daily-notes-immutable";
        assert!(
            insert_claimed_occurrence(&conn, occurrence_id, &routine.id, "daemon", 60, now,)
                .unwrap()
        );
        let runtime = AuthorityObservingRuntime { conn: &conn };
        let mut clock = || now;
        let cancelled = || false;
        let mut control = DispatchControl {
            clock: &mut clock,
            cancelled: &cancelled,
            authority: AutomationAuthorityMode::RuntimeAuthority(&VectorAuthority),
            scheduler_fence: None,
        };
        let dispatch = dispatch_occurrence_with_clock(
            &conn,
            &runtime,
            &routine,
            occurrence_id,
            routine.cwd.as_deref().unwrap(),
            now,
            &mut control,
        )
        .unwrap();
        let DispatchAttempt::Completed(outcome) = dispatch else {
            panic!("authority-bound dispatch must not be deferred");
        };

        let rewrite = conn.execute(
            "UPDATE automation_attempts
             SET authority_extension_json = '{}'
             WHERE run_id = ?1",
            [&outcome.run_id],
        );
        assert!(rewrite.is_err(), "pinned authority must be immutable");
        let delete = conn.execute(
            "DELETE FROM automation_attempts WHERE run_id = ?1",
            [&outcome.run_id],
        );
        assert!(
            delete.is_err(),
            "authority-bound attempts must remain auditable"
        );
        let downgrade = conn.execute(
            "UPDATE automation_runs SET authority_profile = NULL WHERE id = ?1",
            [&outcome.run_id],
        );
        assert!(
            downgrade.is_err(),
            "a run cannot drop its pinned authority profile"
        );
    }

    #[test]
    fn no_process_recovery_preserves_consumed_runtime_authority() {
        let (temp, conn) = temp_store();
        let routine = definition("daily-notes");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        let occurrence_id = "occurrence.daily-notes-no-process";
        assert!(
            insert_claimed_occurrence(&conn, occurrence_id, &routine.id, "daemon", 60, now,)
                .unwrap()
        );
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_with_clock(
            &conn,
            "run.daily-notes-no-process",
            occurrence_id,
            &routine,
            &launch,
            PersistLaunchContext {
                not_before: now,
                authority: AutomationAuthorityMode::RuntimeAuthority(&VectorAuthority),
                scheduler_fence: None,
            },
            || now,
        )
        .unwrap();
        let receipt = containment_receipt_path(temp.path(), &launch.id);
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        crate::pty_runner::write_containment_receipt(
            &receipt,
            crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT,
        )
        .unwrap();

        assert_eq!(
            recover_no_process_preownership_launches(temp.path(), &conn, now).unwrap(),
            1
        );
        let (run_status, attempt_state, authority, occurrence_state): (
            String,
            String,
            Option<String>,
            String,
        ) = conn
            .query_row(
                "SELECT r.status, a.state, a.authority_extension_json, o.state
                 FROM automation_runs AS r
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 WHERE r.id = 'run.daily-notes-no-process'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(run_status, "failed");
        assert_eq!(attempt_state, "failed");
        assert!(authority.is_some());
        assert_eq!(occurrence_state, "failed");
    }

    #[test]
    fn shutdown_admission_refusal_preserves_consumed_runtime_authority() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily-notes");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        let occurrence_id = "occurrence.daily-notes-shutdown";
        assert!(
            insert_claimed_occurrence(&conn, occurrence_id, &routine.id, "daemon", 60, now,)
                .unwrap()
        );
        let mut clock = || now;
        let cancelled = || true;
        let mut control = DispatchControl {
            clock: &mut clock,
            cancelled: &cancelled,
            authority: AutomationAuthorityMode::RuntimeAuthority(&VectorAuthority),
            scheduler_fence: None,
        };

        let dispatch = dispatch_occurrence_with_clock(
            &conn,
            &AdmissionClosedRuntime,
            &routine,
            occurrence_id,
            routine.cwd.as_deref().unwrap(),
            now,
            &mut control,
        )
        .unwrap();
        let DispatchAttempt::Completed(outcome) = dispatch else {
            panic!("consumed authority cannot restore the same attempt for replay");
        };

        assert_eq!(outcome.status, "failed");
        let (run_status, attempt_state, authority, occurrence_state): (
            String,
            String,
            Option<String>,
            String,
        ) = conn
            .query_row(
                "SELECT r.status, a.state, a.authority_extension_json, o.state
                 FROM automation_runs AS r
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(run_status, "failed");
        assert_eq!(attempt_state, "failed");
        assert!(authority.is_some());
        assert_eq!(occurrence_state, "failed");
    }

    #[test]
    fn shutdown_admission_retry_uses_a_fresh_authority_attempt() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily-notes");
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::TransientDispatch]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        let occurrence_id = "occurrence.daily-notes-shutdown-retry";
        assert!(
            insert_claimed_occurrence(&conn, occurrence_id, &routine.id, "daemon", 60, now,)
                .unwrap()
        );
        let mut clock = || now;
        let cancelled = || true;
        let mut control = DispatchControl {
            clock: &mut clock,
            cancelled: &cancelled,
            authority: AutomationAuthorityMode::RuntimeAuthority(&VectorAuthority),
            scheduler_fence: None,
        };

        let dispatch = dispatch_occurrence_with_clock(
            &conn,
            &AdmissionClosedRuntime,
            &routine,
            occurrence_id,
            routine.cwd.as_deref().unwrap(),
            now,
            &mut control,
        )
        .unwrap();
        let DispatchAttempt::Completed(outcome) = dispatch else {
            panic!("consumed authority must settle before retry");
        };

        assert_eq!(outcome.status, "retry_scheduled");
        let attempts: Vec<(i64, String, String, Option<String>)> = conn
            .prepare(
                "SELECT attempt_number, adoption_key, state, authority_extension_json
                 FROM automation_attempts
                 WHERE run_id = ?1
                 ORDER BY attempt_number",
            )
            .unwrap()
            .query_map([&outcome.run_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].0, 1);
        assert_eq!(attempts[0].2, "failed");
        assert!(attempts[0].3.is_some());
        assert_eq!(attempts[1].0, 2);
        assert_ne!(attempts[0].1, attempts[1].1);
        assert_eq!(attempts[1].2, "adopted");
        assert!(attempts[1].3.is_none());
    }

    #[test]
    fn startup_recovers_crashed_preownership_launch_for_retry() {
        let (temp, conn) = temp_store();
        let routine = definition("crashed-preownership");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "crashed-occurrence",
            &routine.id,
            "daemon",
            60,
            now,
        )
        .unwrap());
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_at(
            &conn,
            "crashed-run",
            "crashed-occurrence",
            &routine,
            &launch,
            now,
        )
        .unwrap();

        assert_eq!(
            recover_no_process_preownership_launches(temp.path(), &conn, now).unwrap(),
            0,
            "created state alone must not authorize replay"
        );
        let receipt = containment_receipt_path(temp.path(), &launch.id);
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        crate::pty_runner::write_containment_receipt(
            &receipt,
            crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT,
        )
        .unwrap();
        assert_eq!(
            recover_no_process_preownership_launches(temp.path(), &conn, now).unwrap(),
            1
        );

        let (state, lease_owner, attempt): (String, Option<String>, i64) = conn
            .query_row(
                "SELECT state, lease_owner, attempt
                 FROM automation_occurrences
                 WHERE id = 'crashed-occurrence'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "planned");
        assert_eq!(lease_owner, None);
        assert_eq!(attempt, 1);
        let run_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap();
        let session_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(run_count, 0);
        assert_eq!(session_count, 0);
    }

    #[test]
    fn startup_restores_legacy_preownership_launch_without_a_definition_snapshot() {
        let (temp, conn) = temp_store();
        let routine = definition("legacy-preownership");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "legacy-occurrence",
            &routine.id,
            "daemon",
            60,
            now,
        )
        .unwrap());
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_at(
            &conn,
            "legacy-run",
            "legacy-occurrence",
            &routine,
            &launch,
            now,
        )
        .unwrap();
        conn.execute(
            "DELETE FROM automation_attempts WHERE run_id = 'legacy-run'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_runs
             SET definition_json = NULL
             WHERE id = 'legacy-run'",
            [],
        )
        .unwrap();
        let receipt = containment_receipt_path(temp.path(), &launch.id);
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        crate::pty_runner::write_containment_receipt(
            &receipt,
            crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT,
        )
        .unwrap();

        assert_eq!(
            recover_no_process_preownership_launches(temp.path(), &conn, now).unwrap(),
            1
        );
        let occurrence_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'legacy-occurrence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_state, "planned");
        let counts: (i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM automation_runs WHERE id = 'legacy-run'),
                    (SELECT COUNT(*) FROM sessions WHERE id = ?1)",
                [&launch.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0));
    }

    #[test]
    fn proven_no_process_recovery_retries_lease_expiry_within_the_same_run() {
        let (temp, conn) = temp_store();
        let mut routine = definition("lease-retry");
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::LeaseExpired]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "lease-occurrence",
            &routine.id,
            "daemon",
            60,
            now,
        )
        .unwrap());
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_at(
            &conn,
            "lease-run",
            "lease-occurrence",
            &routine,
            &launch,
            now,
        )
        .unwrap();
        let mut revised = routine.clone();
        revised.retry = RoutineRetryPolicy::default();
        super::super::store::update_definition(&conn, &revised)
            .unwrap()
            .unwrap();
        let receipt = containment_receipt_path(temp.path(), &launch.id);
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        crate::pty_runner::write_containment_receipt(
            &receipt,
            crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT,
        )
        .unwrap();

        assert_eq!(
            recover_no_process_preownership_launches(
                temp.path(),
                &conn,
                now + chrono::Duration::minutes(61)
            )
            .unwrap(),
            1
        );

        let run_status: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'lease-run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_status, "running");
        let attempts: Vec<(i64, String, Option<String>)> = conn
            .prepare(
                "SELECT attempt_number, state, failure_class
                 FROM automation_attempts
                 WHERE run_id = 'lease-run'
                 ORDER BY attempt_number",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            attempts,
            vec![
                (1, "failed".to_string(), Some("lease_expired".to_string())),
                (2, "adopted".to_string(), None),
            ]
        );
        let occurrence_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'lease-occurrence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_state, "planned");
    }

    #[test]
    fn startup_restores_unlaunched_daemon_claim_without_reusing_attempt() {
        let (_temp, conn) = temp_store();
        let routine = definition("unlaunched-claim");
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "unlaunched-occurrence",
            &routine.id,
            "daemon",
            60,
            now,
        )
        .unwrap());

        assert_eq!(
            restore_unlaunched_daemon_claims_for_retry(&conn, now).unwrap(),
            1
        );

        let (state, lease_owner, attempt): (String, Option<String>, i64) = conn
            .query_row(
                "SELECT state, lease_owner, attempt
                 FROM automation_occurrences
                 WHERE id = 'unlaunched-occurrence'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "planned");
        assert_eq!(lease_owner, None);
        assert_eq!(attempt, 1);
    }

    #[test]
    fn startup_missing_receipt_does_not_contain_manual_launch() {
        let (temp, conn) = temp_store();
        let routine = definition("manual-startup-race");
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "manual-startup-occurrence",
            &routine.id,
            "manual",
            60,
            launched_at,
        )
        .unwrap());
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_at(
            &conn,
            "manual-startup-run",
            "manual-startup-occurrence",
            &routine,
            &launch,
            launched_at,
        )
        .unwrap();

        let recovered = recover_restart_containment(
            temp.path(),
            &conn,
            launched_at + chrono::Duration::seconds(2),
            Some(launched_at + chrono::Duration::seconds(1)),
        )
        .unwrap();

        assert_eq!(recovered, 0);
        assert_eq!(
            crate::store::get_session(&conn, &launch.id)
                .unwrap()
                .unwrap()
                .status,
            "created"
        );
    }

    #[test]
    fn startup_cutoff_excludes_new_daemon_launch_without_receipt() {
        let (temp, conn) = temp_store();
        let mut routine = definition("new-daemon-launch");
        routine.status = RoutineStatus::Active;
        insert_definition(&conn, &routine).unwrap();
        let startup_cutoff = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
            + chrono::Duration::microseconds(400);
        let launched_at = startup_cutoff + chrono::Duration::microseconds(100);
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?2, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![
                routine.id,
                (launched_at - chrono::Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();
        let report = super::super::occurrences::tick(&conn, launched_at).unwrap();
        let occurrence_id = report.claimed.first().unwrap();
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_at(
            &conn,
            "new-daemon-run",
            occurrence_id,
            &routine,
            &launch,
            launched_at,
        )
        .unwrap();

        let recovered = recover_restart_containment(
            temp.path(),
            &conn,
            launched_at + chrono::Duration::seconds(1),
            Some(startup_cutoff),
        )
        .unwrap();

        assert_eq!(recovered, 0);
        assert_eq!(
            crate::store::get_session(&conn, &launch.id)
                .unwrap()
                .unwrap()
                .status,
            "created"
        );
    }

    #[test]
    fn startup_cutoff_does_not_contain_manual_kind_with_daemon_owner() {
        let (temp, conn) = temp_store();
        let routine = definition("manual-daemon-owner");
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "manual-daemon-occurrence",
            &routine.id,
            "daemon",
            60,
            launched_at,
        )
        .unwrap());
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_at(
            &conn,
            "manual-daemon-run",
            "manual-daemon-occurrence",
            &routine,
            &launch,
            launched_at,
        )
        .unwrap();

        let recovered = recover_restart_containment(
            temp.path(),
            &conn,
            launched_at + chrono::Duration::seconds(2),
            Some(launched_at + chrono::Duration::seconds(1)),
        )
        .unwrap();

        assert_eq!(recovered, 0);
        assert_eq!(
            crate::store::get_session(&conn, &launch.id)
                .unwrap()
                .unwrap()
                .status,
            "created"
        );
    }

    #[test]
    fn startup_cutoff_contains_previous_scheduled_daemon_launch_without_receipt() {
        let (temp, conn) = temp_store();
        let mut routine = definition("previous-scheduled-launch");
        routine.status = RoutineStatus::Active;
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?2, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![
                routine.id,
                (launched_at - chrono::Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();
        let report = super::super::occurrences::tick(&conn, launched_at).unwrap();
        let occurrence_id = report.claimed.first().unwrap();
        let launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        persist_launch_at(
            &conn,
            "previous-scheduled-run",
            occurrence_id,
            &routine,
            &launch,
            launched_at,
        )
        .unwrap();

        let recovered = recover_restart_containment(
            temp.path(),
            &conn,
            launched_at + chrono::Duration::seconds(2),
            Some(launched_at + chrono::Duration::seconds(1)),
        )
        .unwrap();

        assert_eq!(recovered, 1);
        assert_eq!(
            crate::store::get_session(&conn, &launch.id)
                .unwrap()
                .unwrap()
                .status,
            "killed"
        );
    }

    #[test]
    fn windows_previous_daemon_job_proves_containment_with_partial_receipt() {
        assert!(receipt_proves_containment(Some(b"partial"), true, true));
        assert!(!receipt_proves_containment(Some(b"partial"), true, false));
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
    fn expired_daemon_claim_is_failed_before_dispatch() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily");
        insert_definition(&conn, &routine).unwrap();
        let claimed_at = Utc::now() - chrono::Duration::hours(2);
        assert!(insert_claimed_occurrence(
            &conn,
            "expired-claim",
            &routine.id,
            "daemon",
            60,
            claimed_at,
        )
        .unwrap());

        let report =
            dispatch_claimed_occurrences(&conn, &crate::api::NoopSessionRuntime, Utc::now())
                .unwrap();

        assert!(report.dispatched.is_empty());
        assert!(report.failed.is_empty());
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'expired-claim'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
        assert!(super::super::runs::list_runs(&conn, &routine.id, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn dispatch_refuses_an_occurrence_after_its_definition_revision_changes() {
        let (_temp, conn) = temp_store();
        let routine = definition("revision-race");
        insert_definition(&conn, &routine).unwrap();
        let claimed_at = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "revision-one-occurrence",
            &routine.id,
            "daemon",
            60,
            claimed_at,
        )
        .unwrap());

        let mut revised = routine.clone();
        revised.prompt = "A different action.".to_string();
        let definition_json = serde_json::to_string(&revised).unwrap();
        let definition_digest =
            crate::automations::contract::migration::definition_digest(&definition_json).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET definition_json = ?2, definition_digest = ?3, revision = 2
             WHERE id = ?1",
            rusqlite::params![routine.id, definition_json, definition_digest],
        )
        .unwrap();

        let report = dispatch_claimed_occurrences(
            &conn,
            &crate::api::NoopSessionRuntime,
            claimed_at + chrono::Duration::seconds(1),
        )
        .unwrap();

        assert!(report.dispatched.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].contains("definition revision changed after occurrence fencing"));
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'revision-one-occurrence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
        assert!(super::super::runs::list_runs(&conn, &routine.id, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn dispatch_accepts_legacy_optional_fields_that_normalize_on_serialize() {
        let (_temp, conn) = temp_store();
        let definition_json = r#"{"schemaVersion":1,"id":"legacy-normalized","name":"Legacy normalized","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","familiarId":null,"cwd":"/tmp/project","outputTarget":null,"prompt":"Do the thing.","model":null,"tags":[]}"#;
        let digest =
            crate::automations::contract::migration::definition_digest(definition_json).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'legacy-normalized', 'Legacy normalized', 'ACTIVE', ?1, 1, ?2, 'active',
                NULL, 0, ?3, ?3
             )",
            rusqlite::params![
                definition_json,
                digest,
                Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();
        let definition: RoutineDefinition = serde_json::from_str(definition_json).unwrap();
        let claimed_at = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "legacy-normalized-occurrence",
            &definition.id,
            "daemon",
            60,
            claimed_at,
        )
        .unwrap());

        let report = dispatch_claimed_occurrences(
            &conn,
            &crate::api::NoopSessionRuntime,
            claimed_at + chrono::Duration::seconds(1),
        )
        .unwrap();

        assert_eq!(report.dispatched.len(), 1);
        assert!(report.failed.is_empty());
    }

    #[test]
    fn manual_dispatch_drift_settles_its_claim_before_returning() {
        let (_temp, conn) = temp_store();
        let stale = definition("manual-revision-race");
        insert_definition(&conn, &stale).unwrap();
        let mut revised = stale.clone();
        revised.prompt = "Revision two.".to_string();
        super::super::store::update_definition(&conn, &revised)
            .unwrap()
            .unwrap();

        let error = run_routine_now(&conn, &crate::api::NoopSessionRuntime, &stale, Utc::now())
            .unwrap_err();

        assert!(error.contains("definition body changed after occurrence fencing"));
        let claim_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_occurrences
                 WHERE automation_id = 'manual-revision-race' AND state = 'claimed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(claim_count, 0);
        let failed_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_occurrences
                 WHERE automation_id = 'manual-revision-race' AND state = 'failed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(failed_count, 1);
    }

    #[test]
    fn claim_expired_after_initial_cutoff_is_not_dispatched() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily");
        insert_definition(&conn, &routine).unwrap();
        let actual_now = Utc::now();
        let claimed_at = actual_now - chrono::Duration::minutes(61);
        assert!(insert_claimed_occurrence(
            &conn,
            "expired-during-pass",
            &routine.id,
            "daemon",
            60,
            claimed_at,
        )
        .unwrap());

        let report = dispatch_claimed_occurrences(
            &conn,
            &crate::api::NoopSessionRuntime,
            actual_now - chrono::Duration::minutes(2),
        )
        .unwrap();

        assert!(report.dispatched.is_empty());
        assert!(report.failed.is_empty());
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'expired-during-pass'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn durable_launch_refuses_a_claim_at_its_exact_deadline() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily");
        insert_definition(&conn, &routine).unwrap();
        let claimed_at = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "deadline-reservation",
            &routine.id,
            "daemon",
            60,
            claimed_at,
        )
        .unwrap());

        let error = dispatch_occurrence(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            "deadline-reservation",
            routine.cwd.as_deref().unwrap(),
            claimed_at + chrono::Duration::minutes(60),
        )
        .unwrap_err();

        assert!(error.contains("claim expired or changed"), "{error}");
        assert!(super::super::runs::list_runs(&conn, &routine.id, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn durable_launch_uses_a_post_reservation_timestamp() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily");
        insert_definition(&conn, &routine).unwrap();
        let sampled_before_lock = Utc::now() - chrono::Duration::seconds(2);
        assert!(insert_claimed_occurrence(
            &conn,
            "stale-sampled-time",
            &routine.id,
            "daemon",
            60,
            sampled_before_lock,
        )
        .unwrap());
        conn.execute(
            "UPDATE automation_occurrences
             SET lease_expires_at = ?2
             WHERE id = ?1",
            rusqlite::params![
                "stale-sampled-time",
                (Utc::now() - chrono::Duration::seconds(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .unwrap();

        let error = dispatch_occurrence(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            "stale-sampled-time",
            routine.cwd.as_deref().unwrap(),
            sampled_before_lock,
        )
        .unwrap_err();

        assert!(error.contains("claim expired or changed"), "{error}");
    }

    #[test]
    fn stored_output_target_definition_is_not_loaded_for_dispatch() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("legacy-output");
        routine.output_target = Some("result.md".to_string());
        insert_definition(&conn, &routine).unwrap();

        let error = load_definition_for_run(&conn, &routine.id).unwrap_err();

        assert!(error.contains("outputTarget is not supported"), "{error}");
    }

    #[test]
    fn invalid_claimed_definition_does_not_block_valid_dispatch() {
        let (_temp, conn) = temp_store();
        let mut invalid = definition("invalid");
        invalid.output_target = Some("result.md".to_string());
        let valid = definition("valid");
        insert_definition(&conn, &invalid).unwrap();
        insert_definition(&conn, &valid).unwrap();
        let now = Utc::now();
        assert!(insert_claimed_occurrence(
            &conn,
            "invalid-occurrence",
            &invalid.id,
            "daemon",
            60,
            now,
        )
        .unwrap());
        assert!(insert_claimed_occurrence(
            &conn,
            "valid-occurrence",
            &valid.id,
            "daemon",
            60,
            now + chrono::Duration::milliseconds(1),
        )
        .unwrap());

        let report =
            dispatch_claimed_occurrences(&conn, &crate::api::NoopSessionRuntime, now).unwrap();

        assert_eq!(report.dispatched.len(), 1);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].contains("stored routine `invalid` is invalid"));
        let invalid_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'invalid-occurrence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(invalid_state, "failed");
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
            None,
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
            recover_restart_containment(temp.path(), &conn, launched_at, Some(launched_at))
                .unwrap(),
            0
        );
        assert_eq!(
            super::super::runs::list_runs(&conn, "daily", 10).unwrap()[0].status,
            "running"
        );
    }

    #[test]
    fn receipt_cleanup_removes_terminal_and_missing_sessions_but_keeps_active_evidence() {
        let (temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        insert_definition(&conn, &definition("active")).unwrap();
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
        let active = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("active"),
            launched_at,
        )
        .unwrap();
        let active_receipt =
            containment_receipt_path(temp.path(), active.session_id.as_deref().unwrap());
        std::fs::write(&active_receipt, b"").unwrap();
        let missing_receipt = containment_receipt_path(temp.path(), "missing-session");
        std::fs::write(&missing_receipt, b"").unwrap();

        assert_eq!(
            cleanup_terminal_containment_receipts(temp.path(), &conn).unwrap(),
            2
        );
        assert!(!terminal_receipt.exists());
        assert!(!missing_receipt.exists());
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
        let completed_at = persisted_timeout_at(&conn, "daily") + chrono::Duration::milliseconds(1);
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
        let (reason, attempt_state, failure_class): (String, String, Option<String>) = conn
            .query_row(
                "SELECT o.failure_reason, a.state, a.failure_class
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE o.automation_id = 'daily'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(reason, "session completed after automation timeout");
        assert_eq!(attempt_state, "timed_out");
        assert_eq!(failure_class.as_deref(), Some("timeout"));
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
    fn cancelled_session_evidence_settles_automation_as_cancelled() {
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
        let cancelled_at = launched_at + chrono::Duration::seconds(5);
        crate::store::update_session_terminal_if_active(
            &conn,
            session_id,
            "cancelled",
            None,
            &cancelled_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap();

        let report = settle_finished_runs(&conn, cancelled_at).unwrap();
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.cancelled, 1);

        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "cancelled");
        assert_eq!(run.exit_code, None);

        let (occurrence_state, attempt_state, failure_class): (String, String, Option<String>) =
            conn.query_row(
                "SELECT o.state, a.state, a.failure_class
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(occurrence_state, "cancelled");
        assert_eq!(attempt_state, "cancelled");
        assert_eq!(failure_class.as_deref(), Some("cancelled"));
    }

    #[test]
    fn completion_recorded_before_cancellation_remains_successful() {
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
        let completed_at = launched_at + chrono::Duration::seconds(4);
        assert!(crate::store::update_session_terminal_if_active(
            &conn,
            session_id,
            "completed",
            Some(0),
            &completed_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap());
        assert!(!crate::store::update_session_terminal_if_active(
            &conn,
            session_id,
            "cancelled",
            None,
            &(completed_at + chrono::Duration::seconds(1))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap());

        let report = settle_finished_runs(&conn, completed_at).unwrap();
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(report.cancelled, 0);

        let (run_status, occurrence_state, attempt_state): (String, String, String) = conn
            .query_row(
                "SELECT r.status, o.state, a.state
                 FROM automation_runs AS r
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(run_status, "succeeded");
        assert_eq!(occurrence_state, "succeeded");
        assert_eq!(attempt_state, "succeeded");
    }

    #[test]
    fn cancellation_after_deadline_preserves_timeout_disposition() {
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
        let cancelled_at = persisted_timeout_at(&conn, "daily") + chrono::Duration::milliseconds(1);
        crate::store::update_session_terminal_if_active(
            &conn,
            outcome.session_id.as_deref().unwrap(),
            "cancelled",
            None,
            &cancelled_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap();

        let report = settle_finished_runs(&conn, cancelled_at).unwrap();
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(report.cancelled, 0);

        let (run_status, occurrence_state, attempt_state, failure_class): (
            String,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT r.status, o.state, a.state, a.failure_class
                 FROM automation_runs AS r
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(run_status, "failed");
        assert_eq!(occurrence_state, "failed");
        assert_eq!(attempt_state, "timed_out");
        assert_eq!(failure_class.as_deref(), Some("timeout"));
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
    fn runtime_unavailable_retries_within_one_run_after_persisted_backoff() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("retryable");
        routine.status = RoutineStatus::Active;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::Exponential,
            backoff_seconds: Some(10),
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 9, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = '2026-09-03T08:00:00.000Z',
                 updated_at = '2026-09-03T08:00:00.000Z'
             WHERE id = 'retryable'",
            [],
        )
        .unwrap();
        let tick = super::super::occurrences::tick(&conn, now).unwrap();
        let occurrence_id = tick.claimed.first().unwrap().clone();
        let runtime = UnavailableThenContainedRuntime {
            launches: std::sync::atomic::AtomicUsize::new(0),
        };

        dispatch_claimed_occurrences_with_clock(&conn, &runtime, now, || now).unwrap();

        assert_eq!(
            settle_finished_runs(&conn, now).unwrap(),
            SettlementReport::default(),
            "a failed provisional session must not terminalize a run waiting to retry"
        );
        let occurrence_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                [&occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_state, "planned");
        let run_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_runs WHERE occurrence_id = ?1",
                [&occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_count, 1);
        let attempts: Vec<(i64, String, Option<String>, String)> = conn
            .prepare(
                "SELECT attempt_number, state, failure_class, not_before
                 FROM automation_attempts
                 ORDER BY attempt_number",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!([attempts[0].0, attempts[1].0], [1, 2]);
        assert_eq!(attempts[0].1, "failed");
        assert_eq!(attempts[0].2.as_deref(), Some("runtime_unavailable"));
        assert_eq!(attempts[1].1, "adopted");
        let retry_at = DateTime::parse_from_rfc3339(&attempts[1].3)
            .unwrap()
            .with_timezone(&Utc);
        assert!(retry_at > now);
        assert!(retry_at <= now + chrono::Duration::seconds(20));

        let early_tick =
            super::super::occurrences::tick(&conn, retry_at - chrono::Duration::milliseconds(1))
                .unwrap();
        assert!(early_tick.claimed.is_empty());
        let due_tick = super::super::occurrences::tick(&conn, retry_at).unwrap();
        assert_eq!(due_tick.claimed, vec![occurrence_id.clone()]);
        let retry_fence: (i64, i64) = conn
            .query_row(
                "SELECT o.attempt, a.occurrence_fence_generation
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE o.id = ?1 AND a.attempt_number = 2",
                [&occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(retry_fence.0, retry_fence.1);
        dispatch_claimed_occurrences_with_clock(&conn, &runtime, retry_at, || retry_at).unwrap();

        let run_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_runs WHERE occurrence_id = ?1",
                [&occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_count, 1);
        let second_state: String = conn
            .query_row(
                "SELECT state FROM automation_attempts
                 WHERE run_id = (
                     SELECT id FROM automation_runs WHERE occurrence_id = ?1
                 ) AND attempt_number = 2",
                [&occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(second_state, "started");
    }

    #[test]
    fn manual_retry_continues_while_the_definition_remains_paused() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("manual-retry");
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let runtime = UnavailableThenContainedRuntime {
            launches: std::sync::atomic::AtomicUsize::new(0),
        };
        let now = Utc::now();

        let first = run_routine_now(&conn, &runtime, &routine, now).unwrap();
        assert_eq!(first.status, "retry_scheduled");
        let tick =
            super::super::occurrences::tick(&conn, now + chrono::Duration::seconds(1)).unwrap();
        assert_eq!(tick.claimed.len(), 1);
        dispatch_claimed_occurrences_with_clock(
            &conn,
            &runtime,
            now + chrono::Duration::seconds(1),
            || now + chrono::Duration::seconds(1),
        )
        .unwrap();

        let counts: (i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM automation_runs),
                    (SELECT COUNT(*) FROM automation_attempts)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 2));
    }

    #[test]
    fn retry_uses_the_definition_snapshot_pinned_when_the_run_started() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("pinned-retry");
        routine.status = RoutineStatus::Active;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 9, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = '2026-09-03T08:00:00.000Z',
                 updated_at = '2026-09-03T08:00:00.000Z'
             WHERE id = 'pinned-retry'",
            [],
        )
        .unwrap();
        let runtime = UnavailableThenRecordingRuntime {
            launches: std::sync::atomic::AtomicUsize::new(0),
            prompts: std::sync::Mutex::new(Vec::new()),
        };
        let first = super::super::occurrences::tick(&conn, now).unwrap();
        dispatch_claimed_occurrences_with_clock(&conn, &runtime, now, || now).unwrap();
        let mut revised = routine.clone();
        revised.prompt = "Do the revised thing.".to_string();
        super::super::store::update_definition(&conn, &revised)
            .unwrap()
            .unwrap();
        assert_eq!(
            super::super::occurrences::tick(&conn, now).unwrap().claimed,
            first.claimed
        );

        let report = dispatch_claimed_occurrences_with_clock(&conn, &runtime, now, || now).unwrap();

        assert_eq!(report.dispatched.len(), 1);
        assert_eq!(
            *runtime.prompts.lock().unwrap(),
            vec!["Do the thing.".to_string(), "Do the thing.".to_string()]
        );
    }

    #[test]
    fn retry_backoff_is_deterministic_and_bounded() {
        let policy = RoutineRetryPolicy {
            max_attempts: 4,
            backoff_policy: BackoffPolicy::Exponential,
            backoff_seconds: Some(10),
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };

        let first = retry_delay_seconds(&policy, "run-stable", 2);
        assert_eq!(first, retry_delay_seconds(&policy, "run-stable", 2));
        assert!((1..=10).contains(&first));
        let second = retry_delay_seconds(&policy, "run-stable", 3);
        assert!((1..=20).contains(&second));
    }

    #[test]
    fn retry_backoff_starts_when_the_preownership_failure_is_observed() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("failure-clock");
        routine.status = RoutineStatus::Active;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::Fixed,
            backoff_seconds: Some(60),
            retryable_classes: BTreeSet::from([RetryableClass::TransientDispatch]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 9, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = '2026-09-03T08:00:00.000Z',
                 updated_at = '2026-09-03T08:00:00.000Z'
             WHERE id = 'failure-clock'",
            [],
        )
        .unwrap();
        let tick = super::super::occurrences::tick(&conn, now).unwrap();
        assert_eq!(tick.claimed.len(), 1);
        let failure_at = now + chrono::Duration::seconds(50);
        let calls = Cell::new(0_u8);

        dispatch_claimed_occurrences_with_clock(&conn, &TimedOutRuntime, now, || {
            let call = calls.get();
            calls.set(call + 1);
            if call >= 3 {
                failure_at
            } else {
                now
            }
        })
        .unwrap();

        let not_before: String = conn
            .query_row(
                "SELECT not_before
                 FROM automation_attempts
                 WHERE attempt_number = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            DateTime::parse_from_rfc3339(&not_before)
                .unwrap()
                .with_timezone(&Utc),
            failure_at + chrono::Duration::seconds(60)
        );
    }

    #[test]
    fn preownership_io_failures_are_classified_without_retrying_unknown_errors() {
        let transient = anyhow::Error::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "dispatch queue timed out",
        ));
        assert_eq!(
            classify_preownership_failure(&transient),
            PreownershipFailure::Retryable(RetryableClass::TransientDispatch)
        );
        assert_eq!(
            classify_preownership_failure(&anyhow::anyhow!("unknown launch failure")),
            PreownershipFailure::LaunchRefused
        );
    }

    #[test]
    fn exhausted_retry_quarantines_the_routine_until_explicit_release() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("exhausted");
        routine.status = RoutineStatus::Active;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 9, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = '2026-09-03T08:00:00.000Z',
                 updated_at = '2026-09-03T08:00:00.000Z'
             WHERE id = 'exhausted'",
            [],
        )
        .unwrap();
        let first = super::super::occurrences::tick(&conn, now).unwrap();
        assert_eq!(first.claimed.len(), 1);
        dispatch_claimed_occurrences_with_clock(&conn, &UnavailableRuntime, now, || now).unwrap();
        let retry = super::super::occurrences::tick(&conn, now).unwrap();
        assert_eq!(retry.claimed, first.claimed);

        dispatch_claimed_occurrences_with_clock(&conn, &UnavailableRuntime, now, || now).unwrap();

        let state: (String, String, i64, Option<String>) = conn
            .query_row(
                "SELECT o.state, r.status, q.consecutive_exhaustions, q.quarantined_at
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_retry_state AS q ON q.automation_id = o.automation_id
                 WHERE o.id = ?1",
                [&first.claimed[0]],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(state.0, "failed");
        assert_eq!(state.1, "failed");
        assert_eq!(state.2, 1);
        assert!(state.3.is_some());
        let quarantined_tick =
            super::super::occurrences::tick(&conn, now + chrono::Duration::days(1)).unwrap();
        assert!(quarantined_tick.planned.is_empty());
        assert!(quarantined_tick.claimed.is_empty());
        let blocked = run_routine_now(&conn, &UnavailableRuntime, &routine, now).unwrap();
        assert_eq!(blocked.status, "failed");
        assert!(blocked.error.unwrap().contains("quarantined"));

        assert!(super::super::runs::clear_retry_quarantine(
            &conn,
            "exhausted",
            now + chrono::Duration::minutes(1)
        )
        .unwrap());
        let released =
            super::super::occurrences::tick(&conn, now + chrono::Duration::days(1)).unwrap();
        assert_eq!(released.planned.len(), 1);
        assert_eq!(released.claimed.len(), 1);
    }

    #[test]
    fn retryable_failure_at_max_attempts_one_quarantines_immediately() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("single-attempt-exhausted");
        routine.retry = RoutineRetryPolicy {
            max_attempts: 1,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();

        let outcome = run_routine_now(&conn, &UnavailableRuntime, &routine, now).unwrap();

        assert_eq!(outcome.status, "failed");
        assert!(
            super::super::runs::is_retry_quarantined(&conn, "single-attempt-exhausted").unwrap()
        );
        let exhaustion_count: i64 = conn
            .query_row(
                "SELECT consecutive_exhaustions
                 FROM automation_retry_state
                 WHERE automation_id = 'single-attempt-exhausted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exhaustion_count, 1);
    }

    #[test]
    fn shutdown_before_retry_ownership_preserves_the_run_and_pending_attempt() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("retry-shutdown");
        routine.status = RoutineStatus::Active;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 9, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = '2026-09-03T08:00:00.000Z',
                 updated_at = '2026-09-03T08:00:00.000Z'
             WHERE id = 'retry-shutdown'",
            [],
        )
        .unwrap();
        let first = super::super::occurrences::tick(&conn, now).unwrap();
        dispatch_claimed_occurrences_with_clock(&conn, &UnavailableRuntime, now, || now).unwrap();
        assert_eq!(
            super::super::occurrences::tick(&conn, now).unwrap().claimed,
            first.claimed
        );
        let cancellation_checks = std::sync::atomic::AtomicUsize::new(0);

        dispatch_claimed_occurrences_with_clock_and_cancel(
            &conn,
            &AdmissionClosedRuntime,
            now,
            || now,
            || cancellation_checks.fetch_add(1, std::sync::atomic::Ordering::AcqRel) > 0,
        )
        .unwrap();

        let run_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(run_count, 1);
        let pending: (String, i64) = conn
            .query_row(
                "SELECT state, dispatch_generation
                 FROM automation_attempts
                 WHERE attempt_number = 2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(pending, ("adopted".to_string(), 1));
        let occurrence_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                [&first.claimed[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_state, "planned");
    }

    #[test]
    fn claimed_retry_without_a_new_session_is_restored_after_restart() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("retry-restart");
        routine.status = RoutineStatus::Active;
        routine.timeout_minutes = 120;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 9, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = '2026-09-03T08:00:00.000Z',
                 updated_at = '2026-09-03T08:00:00.000Z'
             WHERE id = 'retry-restart'",
            [],
        )
        .unwrap();
        let first = super::super::occurrences::tick(&conn, now).unwrap();
        dispatch_claimed_occurrences_with_clock(&conn, &UnavailableRuntime, now, || now).unwrap();
        assert_eq!(
            super::super::occurrences::tick(&conn, now).unwrap().claimed,
            first.claimed
        );

        assert_eq!(
            restore_unlaunched_daemon_claims_for_retry(&conn, now + chrono::Duration::minutes(61))
                .unwrap(),
            1
        );

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                [&first.claimed[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "planned");
        assert_eq!(
            super::super::occurrences::tick(&conn, now + chrono::Duration::minutes(61))
                .unwrap()
                .claimed,
            first.claimed
        );
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
    fn stale_definition_cannot_create_a_manual_occurrence_after_tombstone() {
        let (_temp, conn) = temp_store();
        let routine = definition("tombstoned");
        insert_definition(&conn, &routine).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET revision = 2,
                 tombstoned_at = '2026-09-03T09:00:00.000Z',
                 updated_at = '2026-09-03T09:00:00.000Z'
             WHERE id = 'tombstoned'",
            [],
        )
        .unwrap();

        let outcome =
            run_routine_now(&conn, &crate::api::NoopSessionRuntime, &routine, Utc::now()).unwrap();

        assert_eq!(outcome.status, "failed");
        let occurrence_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_occurrences
                 WHERE automation_id = 'tombstoned'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_count, 0);
    }

    #[test]
    fn unproven_preownership_stop_enters_recovery_without_a_second_kill() {
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
        persist_launch_at(&conn, run_id, occurrence_id, &routine, &launch, launched_at).unwrap();

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
        assert_eq!(report.failed, 0);

        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "running");
        let session = crate::store::get_session(&conn, session_id)
            .unwrap()
            .unwrap();
        assert_eq!(session.status, "created");
        let recovery_state: (String, String) = conn
            .query_row(
                "SELECT o.state, a.state
                 FROM automation_occurrences AS o
                 JOIN automation_attempts AS a ON a.occurrence_id = o.id
                 WHERE o.id = ?1",
                rusqlite::params![occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(recovery_state.0, "recovery_required");
        assert_eq!(recovery_state.1, "ambiguous");
    }

    #[test]
    fn abandoned_launch_recovery_defers_to_a_cancellation_stop_owner() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 120;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        let launched_at = now - chrono::Duration::minutes(61);
        assert!(insert_claimed_occurrence(
            &conn,
            "fenced-abandoned-occurrence",
            &routine.id,
            "daemon",
            60,
            launched_at,
        )
        .unwrap());
        let mut launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        launch.id = "fenced-abandoned-session".to_string();
        persist_launch_at(
            &conn,
            "fenced-abandoned-run",
            "fenced-abandoned-occurrence",
            &routine,
            &launch,
            launched_at,
        )
        .unwrap();
        let attempt_id: String = conn
            .query_row(
                "SELECT id FROM automation_attempts WHERE run_id = 'fenced-abandoned-run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let adoption_key = "adopt:cancel:fenced-abandoned:0001";
        let execution_expires_at = (now + chrono::Duration::seconds(30))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_command_reservations (
                 adoption_key, request_digest, command, reserved_at
             ) VALUES (?1, 'digest', 'run.cancel.v1', ?2)",
            rusqlite::params![adoption_key, launched_at.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_cancellations (
                 adoption_key, request_digest, automation_id, run_id, attempt_id,
                 session_id, scope, requested_by_json, reason, state, requested_at,
                 execution_expires_at
             ) VALUES (?1, 'digest', ?2, ?3, ?4, ?5, 'run',
                       '{\"principalId\":\"operator\"}', 'reason', 'requested',
                       ?6, ?7)",
            rusqlite::params![
                adoption_key,
                routine.id,
                "fenced-abandoned-run",
                attempt_id,
                "fenced-abandoned-session",
                launched_at.to_rfc3339(),
                execution_expires_at
            ],
        )
        .unwrap();
        assert!(matches!(
            claim_stop_fence(
                &conn,
                "fenced-abandoned-run",
                "fenced-abandoned-session",
                "cancellation",
                Some(adoption_key),
                Some(&execution_expires_at),
                now,
            )
            .unwrap(),
            StopFenceClaim::Acquired
        ));

        assert!(
            recover_abandoned_launches(&conn, &crate::api::NoopSessionRuntime, now)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            crate::store::get_session(&conn, "fenced-abandoned-session")
                .unwrap()
                .unwrap()
                .status,
            "created"
        );
    }

    #[test]
    fn preownership_launch_is_expired_at_its_exact_deadline() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("daily");
        routine.timeout_minutes = 120;
        insert_definition(&conn, &routine).unwrap();
        let now = Utc::now();
        let launched_at = now - chrono::Duration::minutes(60);
        assert!(insert_claimed_occurrence(
            &conn,
            "deadline-occurrence",
            &routine.id,
            "daemon",
            60,
            launched_at,
        )
        .unwrap());
        let mut launch = build_session_launch(&routine, routine.cwd.as_deref().unwrap()).unwrap();
        launch.id = "deadline-session".to_string();
        persist_launch_at(
            &conn,
            "deadline-run",
            "deadline-occurrence",
            &routine,
            &launch,
            launched_at,
        )
        .unwrap();

        assert!(
            recover_abandoned_launches(&conn, &crate::api::NoopSessionRuntime, now)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            crate::store::get_session(&conn, "deadline-session")
                .unwrap()
                .unwrap()
                .status,
            "killed"
        );
    }

    #[test]
    fn timeout_kills_session_and_settles_once() {
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
        let timed_out_at = persisted_timeout_at(&conn, "daily");

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
        assert_eq!(
            report,
            SettlementReport::default(),
            "the timeout path must own the only terminal settlement"
        );
        let run = super::super::runs::list_runs(&conn, "daily", 10)
            .unwrap()
            .remove(0);
        assert_eq!(run.status, "failed");
        let (attempt_state, failure_class): (String, Option<String>) = conn
            .query_row(
                "SELECT state, failure_class
                 FROM automation_attempts
                 WHERE run_id = ?1",
                [run.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempt_state, "timed_out");
        assert_eq!(failure_class.as_deref(), Some("timeout"));
    }

    #[test]
    fn timeout_settles_dispatching_attempt_before_session_ownership_publication() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("dispatching-timeout");
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
        let session_id = outcome.session_id.as_deref().expect("linked session");
        conn.execute(
            "UPDATE automation_attempts
             SET state = 'dispatching', session_id = NULL
             WHERE run_id = ?1",
            [&outcome.run_id],
        )
        .unwrap();
        let timed_out_at = persisted_timeout_at(&conn, &routine.id);

        assert!(
            enforce_run_timeouts(&conn, &crate::api::NoopSessionRuntime, timed_out_at)
                .unwrap()
                .is_empty()
        );
        let state: (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT r.status, o.state, a.state, a.session_id
                 FROM automation_runs AS r
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            (
                "failed".into(),
                "failed".into(),
                "timed_out".into(),
                Some(session_id.to_string())
            )
        );
    }

    #[test]
    fn retry_wait_cannot_outlive_the_run_timeout() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("retry-timeout");
        routine.timeout_minutes = 1;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::Fixed,
            backoff_seconds: Some(120),
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(&conn, &UnavailableRuntime, &routine, launched_at).unwrap();
        assert_eq!(outcome.status, "retry_scheduled");
        let timed_out_at = persisted_timeout_at(&conn, &routine.id);

        assert!(
            enforce_run_timeouts(&conn, &crate::api::NoopSessionRuntime, timed_out_at)
                .unwrap()
                .is_empty()
        );

        let state: (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, a.state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1 AND a.attempt_number = 2",
                [outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("failed".into(), "failed".into(), "timed_out".into())
        );
        let tick =
            super::super::occurrences::tick(&conn, timed_out_at + chrono::Duration::minutes(2))
                .unwrap();
        assert!(tick.claimed.is_empty());
    }

    #[test]
    fn claimed_retry_that_reaches_dispatch_at_timeout_settles_atomically() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("retry-dispatch-timeout");
        routine.timeout_minutes = 1;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(&conn, &UnavailableRuntime, &routine, launched_at).unwrap();
        assert_eq!(outcome.status, "retry_scheduled");
        let timed_out_at = persisted_timeout_at(&conn, &routine.id);
        let claimed_at = timed_out_at - chrono::Duration::milliseconds(1);
        let tick = super::super::occurrences::tick(&conn, claimed_at).unwrap();
        assert_eq!(tick.claimed.len(), 1);

        let report = dispatch_claimed_occurrences_with_clock(
            &conn,
            &crate::api::NoopSessionRuntime,
            claimed_at,
            || timed_out_at,
        )
        .unwrap();

        assert!(report.dispatched.is_empty());
        let state: (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, a.state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1 AND a.attempt_number = 2",
                [outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("failed".into(), "failed".into(), "timed_out".into())
        );
    }

    #[test]
    fn retry_crossing_timeout_inside_persistence_settles_atomically() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("retry-persist-timeout");
        routine.timeout_minutes = 1;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(&conn, &UnavailableRuntime, &routine, launched_at).unwrap();
        assert_eq!(outcome.status, "retry_scheduled");
        let timed_out_at = persisted_timeout_at(&conn, &routine.id);
        let before_timeout = timed_out_at - chrono::Duration::milliseconds(1);
        let tick = super::super::occurrences::tick(&conn, before_timeout).unwrap();
        assert_eq!(tick.claimed.len(), 1);
        let calls = Cell::new(0_u8);

        let report = dispatch_claimed_occurrences_with_clock(
            &conn,
            &crate::api::NoopSessionRuntime,
            before_timeout,
            || {
                let call = calls.get();
                calls.set(call + 1);
                if call >= 2 {
                    timed_out_at
                } else {
                    before_timeout
                }
            },
        )
        .unwrap();

        assert!(report.dispatched.is_empty());
        let state: (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, a.state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1 AND a.attempt_number = 2",
                [outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("failed".into(), "failed".into(), "timed_out".into())
        );
    }

    #[test]
    fn retry_crossing_claim_expiry_inside_persistence_is_restored() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("retry-persist-lease");
        routine.timeout_minutes = 120;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(&conn, &UnavailableRuntime, &routine, launched_at).unwrap();
        assert_eq!(outcome.status, "retry_scheduled");
        let claimed_at = launched_at + chrono::Duration::seconds(1);
        let tick = super::super::occurrences::tick(&conn, claimed_at).unwrap();
        assert_eq!(tick.claimed.len(), 1);
        let after_lease = claimed_at + chrono::Duration::minutes(61);
        let calls = Cell::new(0_u8);

        dispatch_claimed_occurrences_with_clock(
            &conn,
            &crate::api::NoopSessionRuntime,
            claimed_at,
            || {
                let call = calls.get();
                calls.set(call + 1);
                if call >= 2 {
                    after_lease
                } else {
                    claimed_at
                }
            },
        )
        .unwrap();

        let state: (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, a.state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1 AND a.attempt_number = 2",
                [outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("planned".into(), "running".into(), "adopted".into())
        );
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
        assert!(super::super::store::remove_definition_for_test(&conn, "daily").unwrap());

        let timed_out_at = persisted_timeout_at(&conn, "daily");
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
        let timed_out_at = persisted_timeout_at(&conn, "daily");

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
    fn unproven_timeout_stop_enters_recovery_without_automatic_retry() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("timeout-recovery");
        routine.timeout_minutes = 1;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        let timed_out_at = persisted_timeout_at(&conn, &routine.id);

        let failures = enforce_run_timeouts(&conn, &FailedKillRuntime, timed_out_at).unwrap();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("termination is unproven"));

        let lifecycle: (String, String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT o.state, a.state, a.failure_class, a.state_reason
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            lifecycle,
            (
                "recovery_required".to_string(),
                "ambiguous".to_string(),
                Some("ambiguous_evidence".to_string()),
                Some("timeout stop was not confirmed".to_string()),
            )
        );
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_attempts WHERE run_id = ?1",
                [&outcome.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            attempts, 1,
            "an unconfirmed timeout stop must not become an automatic retry"
        );
        assert_eq!(
            crate::store::get_session(
                &conn,
                outcome.session_id.as_deref().expect("linked session"),
            )
            .unwrap()
            .expect("persisted session")
            .status,
            "running",
            "an unconfirmed stop must not be represented as a stopped session"
        );
    }

    #[test]
    fn timeout_wins_over_late_completion_without_opening_a_retry() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("timeout-race");
        routine.timeout_minutes = 1;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        let timed_out_at = persisted_timeout_at(&conn, &routine.id);

        assert!(
            enforce_run_timeouts(&conn, &crate::api::NoopSessionRuntime, timed_out_at)
                .unwrap()
                .is_empty()
        );
        assert!(
            !crate::store::update_session_terminal_if_active(
                &conn,
                outcome.session_id.as_deref().expect("linked session"),
                "completed",
                Some(0),
                &(timed_out_at + chrono::Duration::milliseconds(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            )
            .unwrap(),
            "a late completion must lose to the already-recorded timeout stop"
        );
        assert_eq!(
            settle_finished_runs(&conn, timed_out_at).unwrap(),
            SettlementReport::default(),
            "a losing completion observation must not produce a second settlement"
        );

        let lifecycle: (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT r.status, o.state, a.state, a.failure_class
                 FROM automation_runs AS r
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            lifecycle,
            (
                "failed".to_string(),
                "failed".to_string(),
                "timed_out".to_string(),
                Some("timeout".to_string()),
            )
        );
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_attempts WHERE run_id = ?1",
                [&outcome.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            attempts, 1,
            "a timeout must remain a timeout unless persisted policy explicitly permits retry"
        );
    }

    #[test]
    fn completion_wins_over_late_timeout_without_rewriting_terminal_state() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("completion-timeout-race");
        routine.timeout_minutes = 1;
        routine.retry = RoutineRetryPolicy {
            max_attempts: 2,
            backoff_policy: BackoffPolicy::None,
            backoff_seconds: None,
            retryable_classes: BTreeSet::from([RetryableClass::RuntimeUnavailable]),
        };
        insert_definition(&conn, &routine).unwrap();
        let launched_at = Utc::now();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &routine,
            launched_at,
        )
        .unwrap();
        let completed_at = launched_at + chrono::Duration::seconds(30);
        let timed_out_at = persisted_timeout_at(&conn, &routine.id);
        assert!(
            completed_at < timed_out_at,
            "the completion observation must win before the later timeout pass"
        );
        assert!(crate::store::update_session_terminal_if_active(
            &conn,
            outcome.session_id.as_deref().expect("linked session"),
            "completed",
            Some(0),
            &completed_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .unwrap());
        let settlement = settle_finished_runs(&conn, completed_at).unwrap();
        assert_eq!(settlement.succeeded, 1);
        assert_eq!(settlement.failed, 0);

        assert!(
            enforce_run_timeouts(&conn, &crate::api::NoopSessionRuntime, timed_out_at)
                .unwrap()
                .is_empty(),
            "a later timeout observation must lose to the completed terminal state"
        );
        assert_eq!(
            settle_finished_runs(&conn, timed_out_at).unwrap(),
            SettlementReport::default(),
            "the losing timeout observation must not settle a second terminal result"
        );
        let lifecycle: (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT r.status, o.state, a.state, a.failure_class
                 FROM automation_runs AS r
                 JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                 JOIN automation_attempts AS a ON a.run_id = r.id
                 WHERE r.id = ?1",
                [&outcome.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            lifecycle,
            (
                "succeeded".to_string(),
                "succeeded".to_string(),
                "succeeded".to_string(),
                None,
            )
        );
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_attempts WHERE run_id = ?1",
                [&outcome.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            attempts, 1,
            "the losing timeout must not open an automatic retry after completion"
        );
        assert!(
            !crate::store::update_session_terminal_if_active(
                &conn,
                outcome.session_id.as_deref().expect("linked session"),
                "killed",
                None,
                &timed_out_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            )
            .unwrap(),
            "a timeout observed after completion must not rewrite the session terminal state"
        );
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
