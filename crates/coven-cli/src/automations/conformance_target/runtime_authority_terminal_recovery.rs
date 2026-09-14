//! Bounded reconciliation probes, not a trusted-state adapter or dispatch certification.
//! Seed the pinned post-launch ledger; only production code may settle or recover it.

use std::cell::Cell;
use std::collections::BTreeSet;

use anyhow::{anyhow, ensure, Context};
use chrono::Duration;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{canonical_timestamp, scheduler_conformance_tempdir, valid_case_id};
use crate::automations::contract::authority::{
    validate_authority_profile, AuthorityConsumerClass, AuthorityEvidenceVerifier,
    AuthorityProfileDisposition, AuthorityProfileError, AuthorityProfileErrorCode,
    AuthorityValidationPhase, AutomationAuthorityExtension, AUTHORITY_EXTENSION_KEY,
    AUTHORITY_PROFILE, BASE_PROFILE, RUNTIME_AUTHORITY_CAPABILITY,
};
use crate::automations::contract::types::ExtensionBag;
use crate::automations::{definition, occurrences, runner, runs};

const SCHEMA: &str = "coven.automations.runtime-authority-terminal-recovery-vectors.v1";
const STARTED_AT: &str = "2026-09-03T12:00:00.000Z";
const TERMINAL_AT: &str = "2026-09-03T12:00:05.000Z";
const RECONCILE_AT: [&str; 3] = [
    TERMINAL_AT,
    "2026-09-03T12:00:06.000Z",
    "2026-09-03T14:00:00.000Z",
];
const HOLD_REASON: &str =
    "trusted runtime terminal evidence is required before Runtime Authority settlement";
const AUTOMATION_ID: &str = "terminal-recovery";
const OCCURRENCE_ID: &str = "occurrence.terminal-recovery";
const RUN_ID: &str = "run.terminal-recovery";
const ATTEMPT_ID: &str = "attempt.terminal-recovery-1";
const SESSION_ID: &str = "session.terminal-recovery";
const ADOPTION_KEY: &str = "adopt:terminal-recovery-1";
const AUTHORITY_VECTORS: &str =
    include_str!("../../../../../spec/coven-automations/authority/v1/test-vectors.json");

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
    scenario: Scenario,
    started_at: String,
    terminal_at: String,
    reconcile_at: [String; 3],
    session_status: SessionStatus,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    exit_code: Option<i32>,
    evidence_shaped_output: bool,
    expected: Expected,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scenario {
    CompletedZero,
    FailedNonzero,
    ConfirmedCancel,
    ConfirmedTimeout,
    IdleUnknown,
}

impl Scenario {
    const COUNT: usize = 5;

    fn inputs(self) -> (SessionStatus, Option<i32>, bool) {
        match self {
            Self::CompletedZero => (SessionStatus::Completed, Some(0), true),
            Self::FailedNonzero => (SessionStatus::Failed, Some(17), false),
            Self::ConfirmedCancel => (SessionStatus::Cancelled, None, false),
            Self::ConfirmedTimeout => (SessionStatus::Killed, None, false),
            Self::IdleUnknown => (SessionStatus::Idle, None, false),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionStatus {
    Completed,
    Failed,
    Cancelled,
    Killed,
    Idle,
}

impl SessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Killed => "killed",
            Self::Idle => "idle",
        }
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LedgerState {
    Running,
    Started,
    RecoveryRequired,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    occurrence_state: LedgerState,
    run_state: LedgerState,
    attempt_state: LedgerState,
    recovery_reason_preserved: bool,
    run_unresolved: bool,
    attempt_unresolved: bool,
    lease_released: bool,
    binding_preserved: bool,
    session_terminal_preserved: bool,
    run_rows: u32,
    attempt_rows: u32,
    session_rows: u32,
    automatic_retry_rows: u32,
    base_receipt_rows: u32,
    authority_sidecar_rows: u32,
    receipt_event_rows: u32,
    runtime_evidence_rows: u32,
    launch_count: usize,
    settled_run_count: usize,
    reopened_state_preserved: bool,
}

pub(super) fn evaluate(vector: &Value) -> Result<bool, &'static str> {
    let vectors: VectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != SCHEMA || vectors.cases.len() != Scenario::COUNT {
        return Err("conformance vector is invalid");
    }
    let mut ids = BTreeSet::new();
    let mut scenarios = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !ids.insert(&case.case_id)
            || !scenarios.insert(case.scenario)
            || case.started_at != STARTED_AT
            || case.terminal_at != TERMINAL_AT
            || case.reconcile_at != RECONCILE_AT
            || (
                case.session_status,
                case.exit_code,
                case.evidence_shaped_output,
            ) != case.scenario.inputs()
        {
            return Err("conformance vector is invalid");
        }
    }
    let mut passed = 0;
    for case in &vectors.cases {
        passed +=
            usize::from(case_matches(case).map_err(|_| "conformance suite execution failed")?);
    }
    Ok(passed == vectors.cases.len())
}

// This verifier is local to one generated fixture and accepts neither arbitrary
// bindings nor terminal evidence. It makes no cryptographic authentication claim.
struct ExactFixtureAuthority(AutomationAuthorityExtension);

impl AuthorityEvidenceVerifier for ExactFixtureAuthority {
    fn verify(
        &self,
        extension: &AutomationAuthorityExtension,
        phase: AuthorityValidationPhase,
    ) -> Result<(), AuthorityProfileError> {
        if phase != AuthorityValidationPhase::PreDispatch || extension != &self.0 {
            return Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::BindingMismatch,
                "terminal recovery fixture binding or phase mismatch",
            ));
        }
        Ok(())
    }
}

fn fixture_authority(conn: &Connection) -> anyhow::Result<String> {
    let vectors: Value = serde_json::from_str(AUTHORITY_VECTORS)?;
    let mut binding = vectors["fixtures"]["binding"].clone();
    let (revision, digest, fence): (i64, String, i64) = conn.query_row(
        "SELECT r.automation_revision, r.definition_digest, o.attempt
         FROM automation_runs r JOIN automation_occurrences o ON o.id = r.occurrence_id
         WHERE r.id = ?1",
        [RUN_ID],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    binding["base"]["automationId"] = json!(AUTOMATION_ID);
    binding["base"]["automationRevision"] = json!(revision);
    binding["base"]["definitionDigest"]["value"] = json!(digest);
    binding["base"]["occurrenceId"] = json!(OCCURRENCE_ID);
    binding["base"]["occurrenceKey"] = json!(format!("{AUTOMATION_ID}@manual-{ADOPTION_KEY}"));
    binding["base"]["occurrenceFenceGeneration"] = json!(fence);
    binding["base"]["runId"] = json!(RUN_ID);
    binding["base"]["attemptId"] = json!(ATTEMPT_ID);
    binding["base"]["attemptNumber"] = json!(1);
    binding["base"]["adoptionKey"] = json!(ADOPTION_KEY);
    binding["runtime"]["runtimeId"] = json!("coven-code");
    binding["approval"]["use"]["occurrencePrefix"] = json!("occurrence.");
    binding["approval"]["consumption"]["occurrenceId"] = json!(OCCURRENCE_ID);
    binding["approval"]["consumption"]["runId"] = json!(RUN_ID);
    binding["approval"]["consumption"]["attemptNumber"] = json!(1);
    binding["approval"]["consumption"]["fenceGeneration"] = json!(fence);
    let mut body = binding.clone();
    let body_object = body
        .as_object_mut()
        .context("fixture binding is not an object")?;
    body_object.remove("integrity");
    body_object.remove("authentication");
    let mut hasher = Sha256::new();
    hasher.update(b"opencoven:coven-automations-authority-binding:v1\0");
    hasher.update(serde_jcs::to_vec(&body)?);
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    binding["integrity"]["value"] = json!(digest);
    binding["authentication"]["signedDigest"] = json!(digest);
    let extension = json!({
        "profile": AUTHORITY_PROFILE,
        "kind": "AutomationAuthorityExtension",
        "executionBinding": binding,
        "receiptEvidence": null
    });
    let verifier = ExactFixtureAuthority(serde_json::from_value(extension.clone())?);
    let bag: ExtensionBag = serde_json::from_value(json!({AUTHORITY_EXTENSION_KEY: extension}))?;
    let disposition = validate_authority_profile(
        &bag,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &[BASE_PROFILE, AUTHORITY_PROFILE],
        &[RUNTIME_AUTHORITY_CAPABILITY],
        AuthorityValidationPhase::PreDispatch,
        Some(&verifier),
    )?;
    let AuthorityProfileDisposition::Validated(extension) = disposition else {
        anyhow::bail!("fixture authority was not validated");
    };
    Ok(serde_json::to_string(&extension)?)
}

fn seed_started_fixture(conn: &Connection, case: &Case) -> anyhow::Result<String> {
    let now = canonical_timestamp(&case.started_at).context("invalid fixture clock")?;
    let definition = definition::RoutineDefinition::from_json(&json!({
        "schemaVersion": 1, "id": AUTOMATION_ID, "name": "Terminal recovery fixture",
        "status": "ACTIVE", "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc",
        "misfire": "latest", "overlap": "forbid", "timeoutMinutes": 30,
        "runtime": "coven-code", "cwd": "/work/project", "prompt": "Fixture only.",
        "retry": {
            "maxAttempts": 3, "backoffPolicy": "none",
            "retryableClasses": ["runtime_unavailable", "lease_expired", "transient_dispatch"]
        }, "tags": []
    }))
    .map_err(|error| anyhow!(error))?;
    crate::automations::store::insert_definition(conn, &definition)?;
    conn.execute(
        "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![&case.started_at, AUTOMATION_ID],
    )?;
    ensure!(
        occurrences::insert_claimed_occurrence(
            conn,
            OCCURRENCE_ID,
            AUTOMATION_ID,
            "daemon",
            60,
            now,
        )
        .map_err(|error| anyhow!(error))?,
        "fixture occurrence was not claimed"
    );
    let session =
        crate::session_launch::new_session_record(crate::session_launch::NewSessionParams {
            id: SESSION_ID.into(),
            project_root: "/work/project".into(),
            harness: definition.runtime.clone(),
            title: definition.name.clone(),
            status: "running".into(),
            now: case.started_at.clone(),
            conversation_id: None,
            familiar_id: None,
            execution_binding: None,
            labels: Vec::new(),
            visibility: None,
        });
    crate::store::insert_session(conn, &session)?;
    runs::record_run_start(
        conn,
        RUN_ID,
        runs::RunStart {
            automation_id: AUTOMATION_ID,
            occurrence_id: Some(OCCURRENCE_ID),
            authority_profile: Some(AUTHORITY_PROFILE),
            session_id: Some(SESSION_ID),
            familiar_id: None,
            runtime: &definition.runtime,
            timeout_at: now + Duration::minutes(30),
        },
        now,
    )?;
    let extension = fixture_authority(conn)?;
    conn.execute(
        "INSERT INTO automation_attempts
         (id, run_id, occurrence_id, attempt_number, adoption_key,
          occurrence_fence_generation, dispatch_generation, state, retry_classification,
          authority_extension_json, not_before, session_id, opened_at)
         VALUES (?1, ?2, ?3, 1, ?4, 1, 1, 'started', 'initial', ?5, ?6, ?7, ?6)",
        params![
            ATTEMPT_ID,
            RUN_ID,
            OCCURRENCE_ID,
            ADOPTION_KEY,
            &extension,
            &case.started_at,
            SESSION_ID
        ],
    )?;
    ensure!(
        occurrences::mark_occurrence_running(conn, OCCURRENCE_ID, now)
            .map_err(|error| anyhow!(error))?,
        "fixture occurrence was not started"
    );
    if case.evidence_shaped_output {
        conn.execute(
            "INSERT INTO events (id, session_id, kind, payload_json, created_at)
             VALUES ('event.terminal-recovery-output', ?1, 'output', ?2, ?3)",
            params![
                SESSION_ID,
                json!({"text": {
                    "profile": "coven.automations.runtime-terminal-evidence.v1",
                    "sessionId": SESSION_ID, "runId": RUN_ID, "attemptId": ATTEMPT_ID,
                    "disposition": "succeeded"
                }})
                .to_string(),
                &case.terminal_at
            ],
        )?;
    }
    Ok(extension)
}

#[derive(Default)]
struct NoLaunchRuntime {
    launches: Cell<usize>,
}

impl crate::api::SessionRuntime for NoLaunchRuntime {
    fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        self.launches.set(self.launches.get() + 1);
        anyhow::bail!("terminal recovery must not launch a session")
    }

    fn launch_contained_adopted_session(
        &self,
        launch: &crate::api::SessionLaunch,
        _writer: Option<crate::maintenance_gate::WriterLease>,
        _ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.launch_session(launch)
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        anyhow::bail!("terminal recovery must not send input")
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("terminal recovery must not stop a session")
    }
}

fn case_matches(case: &Case) -> anyhow::Result<bool> {
    let home = scheduler_conformance_tempdir().map_err(|error| anyhow!(error))?;
    crate::daemon::ensure_private_coven_home(home.path())?;
    let path = home.path().join("coven.sqlite3");
    let conn = crate::store::open_store(&path)?;
    let extension = seed_started_fixture(&conn, case)?;
    let terminal_at = canonical_timestamp(&case.terminal_at).context("invalid fixture clock")?;
    match case.scenario {
        Scenario::ConfirmedCancel | Scenario::ConfirmedTimeout => {
            let stop = if case.scenario == Scenario::ConfirmedCancel {
                runner::ConfirmedStop::Cancelled
            } else {
                runner::ConfirmedStop::TimedOut
            };
            if runner::settle_confirmed_stop(&conn, RUN_ID, SESSION_ID, stop, terminal_at)
                .map_err(|error| anyhow!(error))?
                != runner::ConfirmedStopSettlement::RecoveryRequired
            {
                return Ok(false);
            }
        }
        _ => ensure!(
            crate::store::update_session_terminal_if_active(
                &conn,
                SESSION_ID,
                case.session_status.as_str(),
                case.exit_code,
                &case.terminal_at,
            )?,
            "fixture terminal transition was not applied"
        ),
    }
    let runtime = NoLaunchRuntime::default();
    let mut previous = None;
    let mut all_passed = true;
    for at in &case.reconcile_at[..2] {
        let snapshot = reconcile(&conn, case, &extension, &runtime, at, previous.as_ref())?;
        all_passed &= snapshot.0 == case.expected;
        previous = Some(snapshot);
    }
    drop(conn);
    let reopened = crate::store::open_initialized_store(&path)?;
    let snapshot = reconcile(
        &reopened,
        case,
        &extension,
        &runtime,
        &case.reconcile_at[2],
        previous.as_ref(),
    )?;
    Ok(all_passed && snapshot.0 == case.expected)
}

fn reconcile(
    conn: &Connection,
    case: &Case,
    extension: &str,
    runtime: &NoLaunchRuntime,
    at: &str,
    previous: Option<&(Expected, String)>,
) -> anyhow::Result<(Expected, String)> {
    let now = canonical_timestamp(at).context("invalid fixture clock")?;
    let report = runner::settle_finished_runs(conn, now).map_err(|error| anyhow!(error))?;
    occurrences::tick(conn, now)?;
    let dispatch = runner::dispatch_claimed_occurrences_with_clock_and_cancel(
        conn,
        runtime,
        now,
        || now,
        || false,
    )
    .map_err(|error| anyhow!(error))?;
    ensure!(
        dispatch.dispatched.is_empty() && dispatch.failed.is_empty(),
        "recovery hold unexpectedly reached dispatch"
    );
    let (mut observed, updated_at) = conn.query_row(
        "SELECT o.state, r.status, a.state,
                o.failure_reason = ?1 AND a.state_reason = ?1,
                r.exit_code IS NULL AND r.finished_at IS NULL AND r.receipt_id IS NULL
                    AND r.log_json IS NULL AND r.output_commit IS NULL,
                a.settled_at IS NULL AND a.failure_class IS NULL,
                o.lease_owner IS NULL AND o.lease_expires_at IS NULL,
                r.authority_profile = ?2 AND a.authority_extension_json = ?3
                    AND a.session_id = r.session_id AND a.occurrence_fence_generation = o.attempt,
                s.status = ?4 AND s.exit_code IS ?5 AND s.updated_at = ?6,
                (SELECT COUNT(*) FROM automation_runs),
                (SELECT COUNT(*) FROM automation_attempts),
                (SELECT COUNT(*) FROM sessions),
                (SELECT COUNT(*) FROM automation_attempts WHERE retry_classification = 'automatic_retry'),
                (SELECT COUNT(*) FROM automation_receipts),
                (SELECT COUNT(*) FROM automation_receipt_authority_extensions),
                (SELECT COUNT(*) FROM automation_events WHERE json_extract(event_json, '$.kind') = 'receipt.recorded'),
                (SELECT COUNT(*) FROM automation_runtime_terminal_evidence),
                o.updated_at
         FROM automation_runs r
         JOIN automation_occurrences o ON o.id = r.occurrence_id
         JOIN automation_attempts a ON a.run_id = r.id
         JOIN sessions s ON s.id = r.session_id
         WHERE r.id = ?7 AND o.id = ?8 AND a.id = ?9 AND s.id = ?10",
        params![HOLD_REASON, AUTHORITY_PROFILE, extension, case.session_status.as_str(),
            case.exit_code, &case.terminal_at, RUN_ID, OCCURRENCE_ID, ATTEMPT_ID, SESSION_ID],
        |row| {
            let state = |index| -> rusqlite::Result<LedgerState> {
                let value: String = row.get(index)?;
                serde_json::from_value(json!(value)).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index, rusqlite::types::Type::Text, Box::new(error),
                    )
                })
            };
            Ok((Expected {
                occurrence_state: state(0)?, run_state: state(1)?, attempt_state: state(2)?,
                recovery_reason_preserved: row.get::<_, Option<bool>>(3)? == Some(true),
                run_unresolved: row.get(4)?, attempt_unresolved: row.get(5)?,
                lease_released: row.get(6)?, binding_preserved: row.get(7)?,
                session_terminal_preserved: row.get(8)?, run_rows: row.get(9)?,
                attempt_rows: row.get(10)?, session_rows: row.get(11)?,
                automatic_retry_rows: row.get(12)?, base_receipt_rows: row.get(13)?,
                authority_sidecar_rows: row.get(14)?, receipt_event_rows: row.get(15)?,
                runtime_evidence_rows: row.get(16)?, launch_count: runtime.launches.get(),
                settled_run_count: report.succeeded + report.failed + report.cancelled,
                reopened_state_preserved: true,
            }, row.get::<_, String>(17)?))
        },
    )?;
    if let Some((previous, previous_updated_at)) = previous {
        observed.reopened_state_preserved =
            &observed == previous && &updated_at == previous_updated_at;
    }
    Ok((observed, updated_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_authority_terminal_recovery_cases_use_production_reconciliation() {
        let vectors: VectorSet = serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/runtime-authority-terminal-recovery.vectors.json"
        )).unwrap();
        for case in vectors.cases {
            assert!(
                case_matches(&case).unwrap_or_else(|error| panic!("{}: {error:#}", case.case_id)),
                "{}",
                case.case_id
            );
        }
    }

    #[test]
    fn runtime_authority_terminal_recovery_fixture_verifier_is_exact_and_predispatch_only() {
        let vectors: Value = serde_json::from_str(AUTHORITY_VECTORS).unwrap();
        let extension: AutomationAuthorityExtension = serde_json::from_value(json!({
            "profile": AUTHORITY_PROFILE, "kind": "AutomationAuthorityExtension",
            "executionBinding": vectors["fixtures"]["binding"], "receiptEvidence": null,
        }))
        .unwrap();
        let verifier = ExactFixtureAuthority(extension.clone());
        assert!(verifier
            .verify(&extension, AuthorityValidationPhase::PreDispatch)
            .is_ok());
        assert!(verifier
            .verify(&extension, AuthorityValidationPhase::Terminal)
            .is_err());
        let mut unrelated = serde_json::to_value(&extension).unwrap();
        unrelated["executionBinding"]["base"]["runId"] = json!("run.unrelated");
        let unrelated = serde_json::from_value(unrelated).unwrap();
        assert!(verifier
            .verify(&unrelated, AuthorityValidationPhase::PreDispatch)
            .is_err());
    }
}
