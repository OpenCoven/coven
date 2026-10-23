use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/attempt-terminal-immutability.vectors.json"
);
const COMMAND_ADOPTION_IDEMPOTENCY_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/command-adoption-idempotency.vectors.json"
);
const COMMAND_ENVELOPE_OUTCOMES_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/command-envelope-outcomes.vectors.json");
const MISSION_COMMANDS_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/mission-commands.vectors.json");
const CALENDAR_SCHEDULE_RESOLUTION_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/calendar-schedule-resolution.vectors.json"
);
const CANCELLATION_TIMEOUT_ARBITRATION_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/cancellation-timeout-arbitration.vectors.json"
);
const DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/definition-lifecycle-transitions.vectors.json"
);
const CAPABILITY_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/capability-negotiation.vectors.json");
const DEFINITION_VALIDATION_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/definition-validation.vectors.json");
const EVENT_REDUCER_DETERMINISM_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/event-reducer-determinism.vectors.json");
const MISFIRE_LATEST_PLANNING_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/misfire-latest-planning.vectors.json");
const OCCURRENCE_LEASE_RECOVERY_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/occurrence-lease-recovery.vectors.json");
const OVERLAP_FORBID_CLAIMING_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/overlap-forbid-claiming.vectors.json");
const RETRY_BACKOFF_TIMING_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/retry-backoff-timing.vectors.json");
const RETRY_QUARANTINE_RECOVERY_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/retry-quarantine-recovery.vectors.json");
const SCHEDULER_LEADERSHIP_FENCING_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/scheduler-leadership-fencing.vectors.json"
);
const STARTUP_RECONCILIATION_WAKE_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/startup-reconciliation-wake.vectors.json"
);
const OCCURRENCE_FENCE_UNIQUENESS_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/occurrence-fence-uniqueness.vectors.json"
);
const RECEIPT_INTEGRITY_VALIDATION_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/receipt-integrity-validation.vectors.json"
);
const RRULE_VOCABULARY_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/rrule-vocabulary.vectors.json");
const RUN_TERMINAL_MONOTONICITY_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/run-terminal-monotonicity.vectors.json");
const RUNTIME_AUTHORITY_TERMINAL_RECOVERY_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/runtime-authority-terminal-recovery.vectors.json"
);
const MAX_CONFORMANCE_REQUEST_BYTES: usize = 1024 * 1024;

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn run_target(coven_home: &Path, operation: &str, input: Option<&Value>) -> anyhow::Result<Output> {
    let mut command = Command::new(coven_bin());
    command
        .args(["automations", "conformance", operation])
        .env("COVEN_HOME", coven_home)
        .env(
            "COVEN_AUTOMATIONS_CONFORMANCE_SCRATCH",
            coven_home
                .parent()
                .ok_or_else(|| anyhow::anyhow!("conformance home requires a parent directory"))?,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(input.to_string().as_bytes())?;
    }
    child.wait_with_output().map_err(Into::into)
}

fn run_target_bytes(coven_home: &Path, operation: &str, input: &[u8]) -> anyhow::Result<Output> {
    let mut child = Command::new(coven_bin())
        .args(["automations", "conformance", operation])
        .env("COVEN_HOME", coven_home)
        .env(
            "COVEN_AUTOMATIONS_CONFORMANCE_SCRATCH",
            coven_home
                .parent()
                .ok_or_else(|| anyhow::anyhow!("conformance home requires a parent directory"))?,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input)?;
    child.wait_with_output().map_err(Into::into)
}

#[test]
fn native_target_capability_is_stateless_and_machine_readable() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");

    let output = run_target(&coven_home, "capability", None)?;

    assert!(
        output.status.success(),
        "capability command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?,
        json!({
            "schemaVersion": "coven.automations.conformance-target-capability.v1",
            "profiles": [
                {
                    "profile": "structural",
                    "suites": [
                        "attempt-terminal-immutability",
                        "capability-negotiation",
                        "command-adoption-idempotency",
                        "command-envelope-outcomes",
                        "definition-lifecycle-transitions",
                        "definition-validation",
                        "event-reducer-determinism",
                        "mission-commands",
                        "occurrence-fence-uniqueness",
                        "receipt-integrity-validation",
                        "rrule-vocabulary",
                        "run-terminal-monotonicity"
                    ]
                },
                {
                    "profile": "scheduler_reliability",
                    "suites": [
                        "calendar-schedule-resolution",
                        "cancellation-timeout-arbitration",
                        "command-storage-lock-recovery",
                        "command-storage-readonly-recovery",
                        "command-storage-corruption-recovery",
                        "command-transaction-rollback",
                        "legacy-import-source-preservation",
                        "command-storage-capacity-recovery",
                        "command-process-crash-recovery",
                        "misfire-latest-planning",
                        "occurrence-lease-recovery",
                        "overlap-forbid-claiming",
                        "retry-backoff-timing",
                        "retry-quarantine-recovery",
                        "scheduler-leadership-fencing",
                        "startup-reconciliation-wake",
                        "result-delivery-recovery",
                        "result-delivery-process-crash-recovery",
                        "run-settlement-process-crash-recovery",
                        "run-launch-process-crash-recovery",
                        "attempt-retry-process-crash-recovery",
                        "occurrence-recovery-process-crash-recovery",
                        "run-cancellation-process-crash-recovery",
                        "flat-cancellation-process-crash-recovery",
                        "timeout-enforcement-process-crash-recovery",
                        "held-settlement-process-crash-recovery",
                        "automatic-retry-process-crash-recovery",
                        "retry-timeout-process-crash-recovery",
                        "waiting-retry-cancellation-process-crash-recovery",
                        "run-now-process-crash-recovery",
                        "run-now-envelope-process-crash-recovery",
                        "tick-process-crash-recovery",
                        "no-process-receipt-process-crash-recovery",
                        "event-feed-process-crash-recovery",
                        "cancellation-timeout-process-crash-recovery"
                    ]
                },
                {
                    "profile": "runtime_authority",
                    "suites": ["runtime-authority-terminal-recovery"]
                }
            ]
        })
    );
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_runtime_authority_terminal_recovery_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let mut request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "runtime_authority",
        "suiteId": "runtime-authority-terminal-recovery",
        "protocolArtifact": {},
        "subjectArtifact": {},
        "vector": serde_json::from_str::<Value>(RUNTIME_AUTHORITY_TERMINAL_RECOVERY_VECTORS)?
    });
    let output = run_target(&coven_home, "evaluate", Some(&request))?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "runtime-authority-terminal-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 5);
    assert_eq!(response["evidence"]["passedCases"], 5);
    assert_eq!(response["evidence"].as_object().unwrap().len(), 3);
    assert!(!coven_home.exists());
    assert_eq!(std::fs::read_dir(temp_dir.path())?.count(), 0);

    request["vector"]["cases"][0]["expected"]["baseReceiptRows"] = json!(1);
    let output = run_target(&coven_home, "evaluate", Some(&request))?;
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["status"], "failed");
    assert!(response.get("evidence").is_none());
    assert_eq!(std::fs::read_dir(temp_dir.path())?.count(), 0);

    request["vector"]["cases"][0]["unexpected"] = json!("must-not-appear-in-errors");
    let output = run_target(&coven_home, "evaluate", Some(&request))?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "Error: conformance vector is invalid\n"
    );
    assert_eq!(std::fs::read_dir(temp_dir.path())?.count(), 0);
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_cancellation_timeout_arbitration_vectors(
) -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(CANCELLATION_TIMEOUT_ARBITRATION_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "cancellation-timeout-arbitration",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 20
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "cancellation-timeout-arbitration");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_calendar_schedule_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "calendar-schedule-resolution",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "calendar-schedule-resolution");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 11);
    assert_eq!(response["evidence"]["passedCases"], 11);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_misfire_latest_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "misfire-latest-planning",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "misfire-latest-planning");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 4);
    assert_eq!(response["evidence"]["passedCases"], 4);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_occurrence_lease_recovery_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "occurrence-lease-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "occurrence-lease-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 5);
    assert_eq!(response["evidence"]["passedCases"], 5);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_overlap_forbid_claiming_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "overlap-forbid-claiming",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "overlap-forbid-claiming");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 7);
    assert_eq!(response["evidence"]["passedCases"], 7);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_retry_backoff_timing_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(RETRY_BACKOFF_TIMING_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "retry-backoff-timing",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "retry-backoff-timing");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 5);
    assert_eq!(response["evidence"]["passedCases"], 5);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_retry_quarantine_recovery_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(RETRY_QUARANTINE_RECOVERY_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "retry-quarantine-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "retry-quarantine-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 4);
    assert_eq!(response["evidence"]["passedCases"], 4);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_scheduler_leadership_fencing_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(SCHEDULER_LEADERSHIP_FENCING_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "scheduler-leadership-fencing",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "scheduler-leadership-fencing");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 3);
    assert_eq!(response["evidence"]["passedCases"], 3);
    assert!(!coven_home.exists());
    Ok(())
}

fn startup_reconciliation_request(vectors: Value) -> Value {
    json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "startup-reconciliation-wake",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    })
}

// File-backed I/O and an owned root keep diagnostic fixtures bounded and
// retain their files if child exit cannot be proved during teardown.
fn run_startup_diagnostic_target(
    request: &Value,
    invalid_scratch: bool,
) -> anyhow::Result<(Output, tempfile::TempDir)> {
    use std::time::{Duration, Instant};

    struct OwnedChild {
        child: std::process::Child,
        root: Option<tempfile::TempDir>,
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            if self.child.wait().is_err() {
                if let Some(root) = self.root.take() {
                    let _ = root.keep();
                }
                eprintln!("conformance diagnostic child exit unproved; owned root retained");
            }
        }
    }

    let root = tempfile::tempdir()?;
    let coven_home = root.path().join("PRIVATE_CONFORMANCE_HOME_SENTINEL");
    let scratch = root.path().join("PRIVATE_CONFORMANCE_SCRATCH_SENTINEL");
    if invalid_scratch {
        std::fs::write(&scratch, b"owned scratch fixture")?;
    } else {
        std::fs::create_dir(&scratch)?;
    }
    let input = root.path().join("request.json");
    let stdout = root.path().join("child.stdout");
    let stderr = root.path().join("child.stderr");
    std::fs::write(&input, serde_json::to_vec(request)?)?;
    let child = Command::new(coven_bin())
        .args(["automations", "conformance", "evaluate"])
        .env("COVEN_HOME", &coven_home)
        .env("COVEN_AUTOMATIONS_CONFORMANCE_SCRATCH", &scratch)
        .stdin(Stdio::from(std::fs::File::open(input)?))
        .stdout(Stdio::from(std::fs::File::create(&stdout)?))
        .stderr(Stdio::from(std::fs::File::create(&stderr)?))
        .spawn()?;
    let mut owned = OwnedChild {
        child,
        root: Some(root),
    };
    let started = Instant::now();
    // This outer hang guard catches a wedged fixture; it does not assert or
    // extend any scheduler or production lifecycle deadline.
    let deadline = started + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = owned.child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "conformance diagnostic child exceeded hang guard; elapsed={:?}",
                started.elapsed()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = Output {
        status,
        stdout: std::fs::read(stdout)?,
        stderr: std::fs::read(stderr)?,
    };
    let root = owned
        .root
        .take()
        .expect("owned root remains until child exit");
    Ok((output, root))
}

#[test]
fn native_target_evaluates_checked_in_startup_reconciliation_wake_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS)?;
    let request = startup_reconciliation_request(vectors);

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "startup-reconciliation-wake");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    assert!(!coven_home.exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("startup-reconciliation-trace"));
    Ok(())
}

#[test]
fn native_target_reports_startup_reconciliation_execution_phase_without_private_paths(
) -> anyhow::Result<()> {
    let vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS)?;
    let request = startup_reconciliation_request(vectors);
    let (output, root) = run_startup_diagnostic_target(&request, true)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("conformance suite execution failed"));
    assert!(
        stderr.contains("startup-reconciliation-trace scenario=startup overflow=false")
            && stderr.contains("phase=temp-home outcome=begin")
            && stderr.contains("phase=temp-home outcome=error"),
        "missing real CLI owning-operation trace: {stderr}"
    );
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("phase="))
            .count(),
        2
    );
    assert!(!stderr.contains("phase=scheduler-start"));
    assert!(
        stderr.len() <= 32_768,
        "bounded trace exceeded diagnostic size guard"
    );
    assert!(
        !stderr.contains("PRIVATE_CONFORMANCE_"),
        "diagnostic leaked private sentinel"
    );
    assert!(
        !stderr.contains(root.path().to_string_lossy().as_ref()),
        "diagnostic leaked owned fixture path"
    );
    assert!(!root
        .path()
        .join("PRIVATE_CONFORMANCE_HOME_SENTINEL")
        .exists());
    assert_eq!(
        std::fs::read(root.path().join("PRIVATE_CONFORMANCE_SCRATCH_SENTINEL"))?,
        b"owned scratch fixture"
    );
    Ok(())
}

#[test]
fn native_target_keeps_startup_expectation_mismatch_separate_from_execution_error(
) -> anyhow::Result<()> {
    let mut vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS)?;
    vectors["cases"][0]["expected"]["launchCount"] = json!(2);
    let request = startup_reconciliation_request(vectors);
    let (output, root) = run_startup_diagnostic_target(&request, false)?;
    assert!(
        output.status.success(),
        "valid mismatch failed execution: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["status"], "failed");
    assert!(response["evidence"].is_null());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("startup-reconciliation-trace"));
    assert!(!root
        .path()
        .join("PRIVATE_CONFORMANCE_HOME_SENTINEL")
        .exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_definition_lifecycle_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "definition-lifecycle-transitions",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "definition-lifecycle-transitions");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 17);
    assert_eq!(response["evidence"]["passedCases"], 17);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_rrule_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "rrule-vocabulary",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "rrule-vocabulary");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 22);
    assert_eq!(response["evidence"]["passedCases"], 22);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_command_adoption_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(COMMAND_ADOPTION_IDEMPOTENCY_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "command-adoption-idempotency",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-adoption-idempotency");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 1);
    assert_eq!(response["evidence"]["passedCases"], 1);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_command_envelope_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(COMMAND_ENVELOPE_OUTCOMES_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "command-envelope-outcomes",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-envelope-outcomes");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 6);
    assert_eq!(response["evidence"]["passedCases"], 6);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_mission_command_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(MISSION_COMMANDS_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "mission-commands",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "mission-commands");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 10);
    assert_eq!(response["evidence"]["passedCases"], 10);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_receipt_integrity_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(RECEIPT_INTEGRITY_VALIDATION_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "receipt-integrity-validation",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "receipt-integrity-validation");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_occurrence_fence_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "occurrence-fence-uniqueness",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["schemaVersion"],
        "coven.automations.conformance-suite-result.v1"
    );
    assert_eq!(response["suiteId"], "occurrence-fence-uniqueness");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 3);
    assert_eq!(response["evidence"]["passedCases"], 3);
    assert!(response["evidence"]["vectorDigest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_rejects_reused_occurrence_ids() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS)?;
    vectors["cases"][1]["first"]["occurrenceId"] =
        vectors["cases"][0]["first"]["occurrenceId"].clone();
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "occurrence-fence-uniqueness",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "Error: conformance vector is invalid\n"
    );
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_event_reducer_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "event-reducer-determinism",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["schemaVersion"],
        "coven.automations.conformance-suite-result.v1"
    );
    assert_eq!(response["suiteId"], "event-reducer-determinism");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 1);
    assert_eq!(response["evidence"]["passedCases"], 1);
    assert!(response["evidence"]["vectorDigest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_attempt_terminal_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "attempt-terminal-immutability",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["schemaVersion"],
        "coven.automations.conformance-suite-result.v1"
    );
    assert_eq!(response["suiteId"], "attempt-terminal-immutability");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 5);
    assert_eq!(response["evidence"]["passedCases"], 5);
    assert!(response["evidence"]["vectorDigest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_definition_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "definition-validation",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["schemaVersion"],
        "coven.automations.conformance-suite-result.v1"
    );
    assert_eq!(response["suiteId"], "definition-validation");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    assert!(response["evidence"]["vectorDigest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_run_terminal_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(RUN_TERMINAL_MONOTONICITY_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "run-terminal-monotonicity",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["schemaVersion"],
        "coven.automations.conformance-suite-result.v1"
    );
    assert_eq!(response["suiteId"], "run-terminal-monotonicity");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 4);
    assert_eq!(response["evidence"]["passedCases"], 4);
    assert!(response["evidence"]["vectorDigest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_capability_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(CAPABILITY_VECTORS)?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": "capability-negotiation",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["schemaVersion"],
        "coven.automations.conformance-suite-result.v1"
    );
    assert_eq!(response["suiteId"], "capability-negotiation");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    assert!(response["evidence"]["vectorDigest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_rejects_oversized_request_without_parsing_it() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let mut request = b"{}".to_vec();
    request.resize(MAX_CONFORMANCE_REQUEST_BYTES + 1, b' ');

    let output = run_target_bytes(&coven_home, "evaluate", &request)?;

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "Error: conformance request is invalid\n"
    );
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_storage_lock_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/command-storage-lock-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "command-storage-lock-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-storage-lock-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_transaction_rollback_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/command-transaction-rollback.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "command-transaction-rollback",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-transaction-rollback");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 3);
    assert_eq!(response["evidence"]["passedCases"], 3);
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_storage_readonly_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/command-storage-readonly-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "command-storage-readonly-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-storage-readonly-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    let observations = response["evidence"]["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0]["sqliteErrorCode"], 8);
    assert_eq!(observations[0]["initialOutcome"], "read_only");
    assert_eq!(observations[1]["initialOutcome"], "committed");
    for observation in observations {
        assert_eq!(observation["baselinePreserved"], true);
        assert_eq!(observation["initialTransactionClosed"], true);
        assert_eq!(observation["finalAddedDefinitions"], 1);
        assert_eq!(observation["replayPreserved"], true);
        assert_eq!(observation["replayWritesUnchanged"], true);
    }
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_imports_legacy_sources_through_real_daemon() -> anyhow::Result<()> {
    #[cfg(unix)]
    let temp = tempfile::tempdir_in("/tmp")?;
    #[cfg(not(unix))]
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("must-not-be-created");
    let vector: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/legacy-import-source-preservation.vectors.json"
    ))?;
    let mut request = json!({
        "schemaVersion":"coven.automations.conformance-suite-request.v1",
        "profile":"scheduler_reliability", "suiteId":"legacy-import-source-preservation",
        "protocolArtifact":{}, "subjectArtifact":{}, "vector":vector
    });
    let output = run_target(&home, "evaluate", Some(&request))?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    let observations = response["evidence"]["caseObservations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    for (index, lifecycle, outcome) in
        [(0, "paused", "legacy_completed"), (1, "draft", "committed")]
    {
        let observed = &observations[index];
        assert_eq!(observed["definitionStatus"], "PAUSED");
        assert_eq!(observed["lifecycleState"], lifecycle);
        assert_eq!(observed["commandOutcome"], outcome);
        for field in [
            "sourcePreserved",
            "preexistingPreserved",
            "replayPreserved",
            "restartPreserved",
            "passed",
        ] {
            assert_eq!(observed[field], true, "{field}: {observed}");
        }
        for table in [
            "automation_occurrences",
            "automation_runs",
            "automation_attempts",
        ] {
            assert_eq!(observed["workCounts"][table], 0);
        }
    }
    assert_eq!(observations[0]["dryRunPreserved"], Value::Null);
    assert_eq!(observations[1]["dryRunPreserved"], true);
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains("synthetic sentinel"));
    assert!(!text.contains("synthetic import sentinel"));
    assert!(!text.contains(temp.path().to_string_lossy().as_ref()));
    assert!(!home.exists());

    request["vector"]["cases"][0]["expected"]["skipped"] = json!(0);
    let output = run_target(&home, "evaluate", Some(&request))?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["status"], "failed");
    assert_eq!(
        response["evidence"]["caseObservations"][0]["skippedCount"],
        1
    );
    assert_eq!(response["evidence"]["caseObservations"][0]["passed"], false);
    assert!(!home.exists());

    request["vector"]["cases"][0]["expected"]["sourcePreserved"] = json!(false);
    let output = run_target(&home, "evaluate", Some(&request))?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "Error: conformance vector is invalid\n"
    );
    assert!(!home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_storage_capacity_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/command-storage-capacity-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "command-storage-capacity-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-storage-capacity-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 2);
    assert_eq!(response["evidence"]["passedCases"], 2);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("capacity observations");
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0]["faultSource"], "sqlite_max_page_count");
    assert_eq!(observations[0]["sqliteErrorCode"], 13);
    assert_eq!(observations[0]["initialAddedDefinitions"], 0);
    assert_eq!(observations[1]["initialAddedDefinitions"], 1);
    for observation in observations {
        assert_eq!(observation["baselinePreserved"], true);
        assert_eq!(observation["finalAddedDefinitions"], 1);
        assert_eq!(observation["replayPreserved"], true);
        assert_eq!(observation["replayWritesUnchanged"], true);
    }
    for (field, value) in [
        ("promptBytes", json!(131073)),
        ("fault", json!("physical_disk_full")),
        ("unexpected", json!("private-invalid-value")),
    ] {
        let mut malformed = request.clone();
        malformed["vector"]["cases"][0][field] = value;
        let rejected = run_target(&coven_home, "evaluate", Some(&malformed))?;
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        let error = String::from_utf8_lossy(&rejected.stderr);
        assert!(error.contains("conformance vector is invalid"));
        assert!(!error.contains("private-invalid-value"));
    }
    let mut wrong_expectation = request.clone();
    wrong_expectation["vector"]["cases"][0]["expected"]["initialOutcome"] = json!("committed");
    let rejected = run_target(&coven_home, "evaluate", Some(&wrong_expectation))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/command-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "command-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-process-crash-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 4);
    assert_eq!(response["evidence"]["passedCases"], 4);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("crash observations");
    assert_eq!(observations.len(), 4);
    for (index, observed) in observations.iter().enumerate() {
        let crash = index < 3;
        assert_eq!(
            observed["boundary"],
            request["vector"]["cases"][index]["boundary"]
        );
        assert_eq!(observed["abruptlyTerminated"], crash);
        assert_eq!(observed["uncommittedWalFramesObserved"], crash);
        for field in [
            "baselinePreserved",
            "integrityOk",
            "preexistingPreserved",
            "replayPreserved",
            "replayWritesUnchanged",
        ] {
            assert_eq!(observed[field], true, "case {index}: {field}");
        }
        for field in [
            "finalAddedDefinitions",
            "finalAddedAdoptions",
            "finalAddedEvents",
        ] {
            assert_eq!(observed[field], 1, "case {index}: {field}");
        }
    }
    assert!(!String::from_utf8_lossy(&output.stdout).contains("adopt:process-crash"));
    for field in ["baselinePreserved", "integrityOk", "replayPreserved"] {
        let mut invalid = request.clone();
        invalid["vector"]["cases"][0]["expected"][field] = json!(false);
        let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["databasePath"] = json!("private-must-not-be-read");
    let rejected = run_target(
        &coven_home,
        "command-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("private-must-not-be-read"));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_result_delivery_process_crash_vectors() -> anyhow::Result<()>
{
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/result-delivery-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "result-delivery-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "result-delivery-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 4);
    assert_eq!(response["evidence"]["passedCases"], 4);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("delivery crash observations");
    assert_eq!(observations.len(), 4);
    for (observation, (boundary, after_exit, recovery)) in observations.iter().zip([
        ("after_notice", "pending", "delivered"),
        ("after_record", "pending", "delivered"),
        ("after_commit", "delivered", "unchanged"),
        ("none", "delivered", "unchanged"),
    ]) {
        let killed = boundary != "none";
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["gateHeldAtPause"],
            if killed { json!(true) } else { json!(null) }
        );
        assert_eq!(observation["stateAfterExit"], after_exit);
        assert_eq!(observation["recoveryPass"], recovery);
        assert_eq!(observation["finalState"], "delivered");
        assert_eq!(observation["finalNotices"], 1);
        let before_commit = matches!(boundary, "after_notice" | "after_record");
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed {
                json!(before_commit)
            } else {
                json!(null)
            }
        );
        assert_eq!(
            observation["stateAtPause"],
            match (killed, before_commit) {
                (false, _) => json!(null),
                (true, true) => json!("pending"),
                (true, false) => json!("delivered"),
            }
        );
        assert_eq!(observation["exitConsistent"], true);
        assert_eq!(observation["gateReleased"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["recordNamesNotice"], true);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "result-delivery-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_run_settlement_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/run-settlement-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "run-settlement-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "run-settlement-process-crash-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 5);
    assert_eq!(response["evidence"]["passedCases"], 5);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("settlement crash observations");
    assert_eq!(observations.len(), 5);
    for (observation, boundary) in observations.iter().zip([
        "after_occurrence",
        "after_attempt",
        "after_run",
        "after_commit",
        "none",
    ]) {
        let killed = boundary != "none";
        let before_commit = killed && boundary != "after_commit";
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed {
                json!(before_commit)
            } else {
                json!(null)
            }
        );
        assert_eq!(
            observation["settlementAtPause"],
            match (killed, before_commit) {
                (false, _) => json!(null),
                (true, true) => json!("unsettled"),
                (true, false) => json!("settled"),
            }
        );
        assert_eq!(
            observation["settlementAfterExit"],
            if before_commit {
                "unsettled"
            } else {
                "settled"
            }
        );
        assert_eq!(
            observation["deliveryAfterExit"],
            if before_commit {
                json!(null)
            } else {
                json!("pending")
            }
        );
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["recoverySettled"], u64::from(before_commit));
        assert_eq!(observation["noticesAfterExit"], 0);
        assert_eq!(observation["exitConsistent"], true);
        assert_eq!(observation["recoveryConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["finalSettlement"], "settled");
        assert_eq!(observation["finalDelivery"], "delivered");
        assert_eq!(observation["finalNotices"], 1);
        assert_eq!(observation["recordNamesNotice"], true);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "run-settlement-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_run_launch_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/run-launch-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "run-launch-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "run-launch-process-crash-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 6);
    assert_eq!(response["evidence"]["passedCases"], 6);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("launch crash observations");
    assert_eq!(observations.len(), 6);
    for (observation, (boundary, committed, inside_transaction)) in observations.iter().zip([
        ("after_session", "claimed", true),
        ("after_dispatching", "claimed", true),
        ("after_launch_commit", "launching", false),
        ("after_ownership", "launching", true),
        ("after_ownership_commit", "owned", false),
        ("none", "owned", false),
    ]) {
        let killed = boundary != "none";
        let relaunch = committed == "claimed";
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed {
                json!(inside_transaction)
            } else {
                json!(null)
            }
        );
        assert_eq!(
            observation["launchAtPause"],
            if killed {
                json!(committed)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["launchAfterExit"], committed);
        assert_eq!(observation["sessionsAfterExit"], u64::from(!relaunch));
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["restartLaunches"], u64::from(relaunch));
        assert_eq!(
            observation["launchAfterRestart"],
            if relaunch { "owned" } else { "held" }
        );
        assert_eq!(
            observation["launchAfterPass"],
            if relaunch { "succeeded" } else { "held" }
        );
        assert_eq!(
            observation["finalLaunch"],
            if relaunch { "succeeded" } else { "failed" }
        );
        assert_eq!(observation["finalSessions"], 1);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "run-launch-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_attempt_retry_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/attempt-retry-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "attempt-retry-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "attempt-retry-process-crash-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 6);
    assert_eq!(response["evidence"]["passedCases"], 6);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("retry crash observations");
    assert_eq!(observations.len(), 6);
    for (observation, (boundary, committed)) in observations.iter().zip([
        ("after_attempt", false),
        ("after_replan", false),
        ("after_release", false),
        ("after_adoption", false),
        ("after_commit", true),
        ("none", true),
    ]) {
        let killed = boundary != "none";
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed {
                json!(!committed)
            } else {
                json!(null)
            }
        );
        let at_exit = if committed { "retried" } else { "held" };
        assert_eq!(
            observation["retryAtPause"],
            if killed { json!(at_exit) } else { json!(null) }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["retryAfterExit"], at_exit);
        assert_eq!(observation["sessionsAfterExit"], 1);
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["restartLaunches"], u64::from(committed));
        assert_eq!(
            observation["retryAfterRestart"],
            if committed { "relaunched" } else { "held" }
        );
        assert_eq!(
            observation["reissue"],
            if committed { "replayed" } else { "committed" }
        );
        assert_eq!(
            observation["retryAfterReissue"],
            if committed { "relaunched" } else { "retried" }
        );
        assert_eq!(observation["launches"], 1);
        assert_eq!(observation["finalRetry"], "succeeded");
        assert_eq!(observation["finalSessions"], 2);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "attempt-retry-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_occurrence_recovery_process_crash_vectors(
) -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/occurrence-recovery-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "occurrence-recovery-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "occurrence-recovery-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 13);
    assert_eq!(response["evidence"]["passedCases"], 13);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("recovery crash observations");
    assert_eq!(observations.len(), 13);
    let retry = "retry_with_new_attempt";
    let failed = "failed_deterministic";
    for (observation, (determination, boundary, committed)) in observations.iter().zip([
        (retry, "after_attempt", false),
        (retry, "after_replan", false),
        (retry, "after_release", false),
        (retry, "after_fence_release", false),
        (retry, "after_adoption", false),
        (retry, "after_commit", true),
        (retry, "none", true),
        (failed, "after_settle", false),
        (failed, "after_finish", false),
        (failed, "after_fence_release", false),
        (failed, "after_adoption", false),
        (failed, "after_commit", true),
        (failed, "none", true),
    ]) {
        let killed = boundary != "none";
        let retries = determination == retry;
        assert_eq!(observation["determination"], determination);
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed {
                json!(!committed)
            } else {
                json!(null)
            }
        );
        let determined = if retries { "retried" } else { "failed" };
        let at_exit = if committed { determined } else { "unresolved" };
        assert_eq!(
            observation["occurrenceAtPause"],
            if killed { json!(at_exit) } else { json!(null) }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["occurrenceAfterExit"], at_exit);
        assert_eq!(observation["sessionsAfterExit"], 1);
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(
            observation["restartLaunches"],
            u64::from(retries && committed)
        );
        let relaunched = if retries { "relaunched" } else { "failed" };
        assert_eq!(
            observation["occurrenceAfterRestart"],
            if committed { relaunched } else { "unresolved" }
        );
        assert_eq!(observation["restartIsNoop"], !(retries && committed));
        assert_eq!(
            observation["reissue"],
            if committed { "replayed" } else { "committed" }
        );
        assert_eq!(
            observation["occurrenceAfterReissue"],
            if committed { relaunched } else { determined }
        );
        assert_eq!(observation["launches"], u64::from(retries));
        assert_eq!(
            observation["finalOccurrence"],
            if retries { "succeeded" } else { "failed" }
        );
        assert_eq!(observation["finalSessions"], if retries { 2 } else { 1 });
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "occurrence-recovery-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_run_cancellation_process_crash_vectors() -> anyhow::Result<()>
{
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/run-cancellation-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "run-cancellation-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "run-cancellation-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 18);
    assert_eq!(response["evidence"]["passedCases"], 18);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("cancellation crash observations");
    assert_eq!(observations.len(), 18);
    // How far the cancellation got before the worker died, and whether a
    // write was open there.
    for (observation, (boundary, stage, open)) in observations.iter().zip([
        ("after_reservation_insert", "untouched", true),
        ("after_cancellation_insert", "untouched", true),
        ("after_reservation_commit", "reserved", false),
        ("after_fence_insert", "reserved", true),
        ("after_stopping", "reserved", true),
        ("after_fence_commit", "fenced", false),
        ("after_stop", "stopped", false),
        ("after_session_end", "stopped", true),
        ("after_attempt_settle", "stopped", true),
        ("after_run_finish", "stopped", true),
        ("after_occurrence_settle", "stopped", true),
        ("after_fence_release", "stopped", true),
        ("after_settle_commit", "settled", false),
        ("after_cancelled", "settled", true),
        ("after_reservation_release", "settled", true),
        ("after_adoption", "settled", true),
        ("after_answer_commit", "answered", false),
        ("none", "answered", false),
    ]) {
        let killed = boundary != "none";
        let (run_at_exit, cancellation_at_exit) = match stage {
            "untouched" => ("running", "none"),
            "reserved" => ("running", "requested"),
            "fenced" | "stopped" => ("fenced", "stopping"),
            "settled" => ("cancelled", "stopping"),
            _ => ("cancelled", "cancelled"),
        };
        let run_after_restart = match stage {
            "untouched" | "reserved" => "held",
            "fenced" | "stopped" => "contained",
            _ => "cancelled",
        };
        let reissue = match stage {
            "untouched" => "refused",
            "answered" => "cancelled",
            _ => "pending",
        };
        let (run_after_pass, cancellation_after_pass, resend) = match stage {
            "untouched" => ("held", "refused", "rejected"),
            "reserved" => ("held", "rejected", "rejected"),
            "fenced" | "stopped" => ("failed", "recovery_required", "recovery_required"),
            _ => ("cancelled", "cancelled", "cancelled"),
        };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed { json!(open) } else { json!(null) }
        );
        assert_eq!(
            observation["runAtPause"],
            if killed {
                json!(run_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(
            observation["cancellationAtPause"],
            if killed {
                json!(cancellation_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["runAfterExit"], run_at_exit);
        assert_eq!(observation["cancellationAfterExit"], cancellation_at_exit);
        assert_eq!(
            observation["workerStops"],
            u64::from(matches!(stage, "stopped" | "settled" | "answered"))
        );
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["runAfterRestart"], run_after_restart);
        assert_eq!(
            observation["cancellationAfterRestart"],
            cancellation_at_exit
        );
        assert_eq!(
            observation["restartIsNoop"],
            matches!(stage, "settled" | "answered")
        );
        assert_eq!(observation["reissue"], reissue);
        assert_eq!(observation["runAfterPass"], run_after_pass);
        assert_eq!(
            observation["cancellationAfterPass"],
            cancellation_after_pass
        );
        assert_eq!(observation["resend"], resend);
        assert_eq!(observation["laterStops"], 0);
        assert_eq!(observation["launches"], 0);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "run-cancellation-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_flat_cancellation_process_crash_vectors() -> anyhow::Result<()>
{
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/flat-cancellation-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "flat-cancellation-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "flat-cancellation-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 12);
    assert_eq!(response["evidence"]["passedCases"], 12);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("flat cancellation crash observations");
    assert_eq!(observations.len(), 12);
    // How far the cancellation got before the worker died, and whether a
    // write was open there.
    for (observation, (boundary, stage, open)) in observations.iter().zip([
        ("after_occurrence_cancel", "claimed", true),
        ("after_withdrawal_adoption", "claimed", true),
        ("after_withdrawal_commit", "withdrawn", false),
        ("claimed_control", "withdrawn", false),
        ("after_delegation_adoption", "untouched", true),
        ("after_delegation_commit", "delegated", false),
        ("after_reservation_commit", "reserved", false),
        ("after_fence_commit", "fenced", false),
        ("after_stop", "stopped", false),
        ("after_settle_commit", "settled", false),
        ("after_answer_commit", "answered", false),
        ("launched_control", "answered", false),
    ]) {
        let killed = !boundary.ends_with("_control");
        let (run_at_exit, cancellation_at_exit) = match stage {
            "claimed" => ("claimed", "none"),
            "withdrawn" => ("withdrawn", "withdrawn"),
            "untouched" => ("running", "none"),
            "delegated" => ("running", "delegated"),
            "reserved" => ("running", "requested"),
            "fenced" | "stopped" => ("fenced", "stopping"),
            "settled" => ("cancelled", "stopping"),
            _ => ("cancelled", "cancelled"),
        };
        let run_after_restart = match stage {
            "claimed" => "running",
            "withdrawn" => "withdrawn",
            "untouched" | "delegated" | "reserved" => "held",
            "fenced" | "stopped" => "contained",
            _ => "cancelled",
        };
        let reissue = match stage {
            "claimed" => "stopped",
            "withdrawn" => "withdrawn",
            "untouched" | "delegated" => "refused",
            "answered" => "cancelled",
            _ => "pending",
        };
        let (run_after_pass, cancellation_after_pass, resend) = match stage {
            "withdrawn" => ("withdrawn", "withdrawn", "withdrawn"),
            "untouched" => ("held", "refused", "rejected"),
            "delegated" => ("held", "delegation_refused", "rejected"),
            "reserved" => ("held", "rejected", "rejected"),
            "fenced" | "stopped" => ("failed", "recovery_required", "recovery_required"),
            _ => ("cancelled", "cancelled", "cancelled"),
        };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed { json!(open) } else { json!(null) }
        );
        assert_eq!(
            observation["runAtPause"],
            if killed {
                json!(run_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(
            observation["cancellationAtPause"],
            if killed {
                json!(cancellation_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["runAfterExit"], run_at_exit);
        assert_eq!(observation["cancellationAfterExit"], cancellation_at_exit);
        assert_eq!(
            observation["workerStops"],
            u64::from(matches!(stage, "stopped" | "settled" | "answered"))
        );
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["runAfterRestart"], run_after_restart);
        assert_eq!(
            observation["cancellationAfterRestart"],
            cancellation_at_exit
        );
        assert_eq!(
            observation["restartIsNoop"],
            matches!(stage, "withdrawn" | "settled" | "answered")
        );
        assert_eq!(observation["reissue"], reissue);
        assert_eq!(observation["runAfterPass"], run_after_pass);
        assert_eq!(
            observation["cancellationAfterPass"],
            cancellation_after_pass
        );
        assert_eq!(observation["resend"], resend);
        // Only a slot nobody withdrew launches, and the re-sent request stops
        // it.
        assert_eq!(observation["laterStops"], u64::from(stage == "claimed"));
        assert_eq!(observation["launches"], u64::from(stage == "claimed"));
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "flat-cancellation-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_timeout_enforcement_process_crash_vectors(
) -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/timeout-enforcement-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "timeout-enforcement-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "timeout-enforcement-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 10);
    assert_eq!(response["evidence"]["passedCases"], 10);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("timeout crash observations");
    assert_eq!(observations.len(), 10);
    // How far the enforcement got before the worker died, and whether a write
    // was open there.
    for (observation, (boundary, stage, open)) in observations.iter().zip([
        ("after_fence_insert", "due", true),
        ("after_fence_commit", "fenced", false),
        ("after_stop", "stopped", false),
        ("after_session_end", "stopped", true),
        ("after_attempt_settle", "stopped", true),
        ("after_run_finish", "stopped", true),
        ("after_occurrence_settle", "stopped", true),
        ("after_fence_release", "stopped", true),
        ("after_settle_commit", "settled", false),
        ("none", "settled", false),
    ]) {
        let killed = boundary != "none";
        let run_at_exit = match stage {
            "due" => "running",
            "fenced" | "stopped" => "fenced",
            _ => "timed_out",
        };
        // Behind the fence, the restart contains the session; the pass after
        // the fence's lease needs recovery, and the next settles the run.
        let (run_after_restart, run_after_pass, run_after_settle) = match stage {
            "fenced" | "stopped" => ("contained", "ambiguous", "failed"),
            _ => ("timed_out", "timed_out", "timed_out"),
        };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed { json!(open) } else { json!(null) }
        );
        assert_eq!(
            observation["runAtPause"],
            if killed {
                json!(run_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["runAfterExit"], run_at_exit);
        assert_eq!(
            observation["workerStops"],
            u64::from(matches!(stage, "stopped" | "settled"))
        );
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["runAfterRestart"], run_after_restart);
        assert_eq!(observation["restartIsNoop"], stage == "settled");
        assert_eq!(observation["runAfterPass"], run_after_pass);
        assert_eq!(observation["runAfterSettle"], run_after_settle);
        assert_eq!(observation["laterStops"], 0);
        assert_eq!(observation["launches"], 0);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "timeout-enforcement-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_held_settlement_process_crash_vectors() -> anyhow::Result<()>
{
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/held-settlement-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "held-settlement-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "held-settlement-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 7);
    assert_eq!(response["evidence"]["passedCases"], 7);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("held settlement crash observations");
    assert_eq!(observations.len(), 7);
    // How far the run got before the worker died, and whether a write was
    // open there.
    for (observation, (boundary, stage, open)) in observations.iter().zip([
        ("after_session_end", "ended", false),
        ("after_hold_write", "ended", true),
        ("after_hold_commit", "held", false),
        ("after_occurrence_settle", "overdue", true),
        ("after_run_finish", "overdue", true),
        ("after_settle_commit", "settled", false),
        ("none", "settled", false),
    ]) {
        let killed = boundary != "none";
        let run_at_exit = match stage {
            "ended" => "ended",
            "held" | "overdue" => "held",
            _ => "failed",
        };
        // Inside the hold window the restart holds the run; after the
        // deadline it settles it.
        let run_after_restart = match stage {
            "ended" | "held" => "held",
            _ => "failed",
        };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed { json!(open) } else { json!(null) }
        );
        assert_eq!(
            observation["runAtPause"],
            if killed {
                json!(run_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["runAfterExit"], run_at_exit);
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["runAfterRestart"], run_after_restart);
        assert_eq!(
            observation["restartIsNoop"],
            matches!(stage, "held" | "settled")
        );
        assert_eq!(observation["runAfterPass"], "failed");
        assert_eq!(
            observation["passIsNoop"],
            matches!(stage, "overdue" | "settled")
        );
        assert_eq!(observation["stops"], 0);
        assert_eq!(observation["launches"], 0);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "held-settlement-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_automatic_retry_process_crash_vectors() -> anyhow::Result<()>
{
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/automatic-retry-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "automatic-retry-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "automatic-retry-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 11);
    assert_eq!(response["evidence"]["passedCases"], 11);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("automatic retry crash observations");
    assert_eq!(observations.len(), 11);
    // How far the occurrence got before the worker died, and whether a write
    // was open there.
    for (observation, (boundary, retry_at_exit, open)) in observations.iter().zip([
        ("after_rejected_session", "launching", true),
        ("after_retry_recorded", "launching", true),
        ("after_session_released", "launching", true),
        ("after_retry_commit", "scheduled", false),
        ("after_reclaim", "scheduled", true),
        ("after_reclaim_commit", "reclaimed", false),
        ("after_relaunch_dispatching", "reclaimed", true),
        ("after_relaunch_commit", "relaunching", false),
        ("after_relaunch_ownership", "relaunching", true),
        ("after_ownership_commit", "retried", false),
        ("none", "retried", false),
    ]) {
        let killed = boundary != "none";
        // Only a retry scheduled when the worker died at the slot waits
        // through the restart, which comes before its backoff ends.
        let waits = boundary == "after_retry_commit";
        let relaunched = matches!(retry_at_exit, "scheduled" | "reclaimed");
        // A restart holds a committed launch, the slot's or the retry's.
        let retry_after_restart = match retry_at_exit {
            "launching" => "held",
            _ if waits => "scheduled",
            "scheduled" | "reclaimed" => "retried",
            _ => "relaunch_held",
        };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed { json!(open) } else { json!(null) }
        );
        assert_eq!(
            observation["retryAtPause"],
            if killed {
                json!(retry_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["retryAfterExit"], retry_at_exit);
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["retryAfterRestart"], retry_after_restart);
        assert_eq!(observation["restartIsNoop"], waits);
        assert_eq!(
            observation["restartLaunches"],
            u64::from(relaunched && !waits)
        );
        assert_eq!(
            observation["retryAfterPass"],
            if waits {
                "retried"
            } else {
                retry_after_restart
            }
        );
        assert_eq!(observation["passIsNoop"], !waits);
        assert_eq!(observation["launches"], u64::from(relaunched));
        assert_eq!(observation["stops"], 0);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "automatic-retry-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_retry_timeout_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/retry-timeout-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "retry-timeout-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "retry-timeout-process-crash-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 6);
    assert_eq!(response["evidence"]["passedCases"], 6);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("retry timeout crash observations");
    assert_eq!(observations.len(), 6);
    // Whether the timeout settlement had committed before the worker died,
    // and whether a write was open there.
    for (observation, (boundary, settled, open)) in observations.iter().zip([
        ("after_retry_commit", false, false),
        ("after_retry_timed_out", false, true),
        ("after_occurrence_failed", false, true),
        ("after_run_failed", false, true),
        ("after_settlement_commit", true, false),
        ("none", true, false),
    ]) {
        let killed = boundary != "none";
        let retry_at_exit = if settled { "timed_out" } else { "scheduled" };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed { json!(open) } else { json!(null) }
        );
        assert_eq!(
            observation["retryAtPause"],
            if killed {
                json!(retry_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["retryAfterExit"], retry_at_exit);
        assert_eq!(observation["recoveryRefused"], false);
        // A restart after the deadline and the backoff settles a waiting
        // retry instead of launching it.
        assert_eq!(observation["retryAfterRestart"], "timed_out");
        assert_eq!(observation["restartIsNoop"], settled);
        assert_eq!(observation["retryAfterPass"], "timed_out");
        assert_eq!(observation["passIsNoop"], true);
        assert_eq!(observation["launches"], 0);
        assert_eq!(observation["stops"], 0);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "retry-timeout-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_waiting_retry_cancellation_process_crash_vectors(
) -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/waiting-retry-cancellation-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "waiting-retry-cancellation-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "waiting-retry-cancellation-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 12);
    assert_eq!(response["evidence"]["passedCases"], 12);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("waiting retry cancellation crash observations");
    assert_eq!(observations.len(), 12);
    // Whether the command's transaction had committed before the worker was
    // gone, and whether the owner cancelled before the run's deadline.
    for (observation, (boundary, committed, waiting)) in observations.iter().zip([
        ("after_retry_cancelled", false, true),
        ("after_run_cancelled", false, true),
        ("after_occurrence_cancelled", false, true),
        ("after_withdrawal_adoption", false, true),
        ("after_withdrawal_commit", true, true),
        ("waiting_control", true, true),
        ("after_retry_timed_out", false, false),
        ("after_occurrence_failed", false, false),
        ("after_run_failed", false, false),
        ("after_refusal_adoption", false, false),
        ("after_refusal_commit", true, false),
        ("expired_control", true, false),
    ]) {
        let killed = !boundary.ends_with("_control");
        let (retry_at_exit, stored_at_exit) = match (committed, waiting) {
            (false, _) => ("scheduled", "none"),
            (true, true) => ("cancelled", "withdrawn"),
            (true, false) => ("timed_out", "timeout_refusal"),
        };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed {
                json!(!committed)
            } else {
                json!(null)
            }
        );
        assert_eq!(
            observation["retryAtPause"],
            if killed {
                json!(retry_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(
            observation["storedAtPause"],
            if killed {
                json!(stored_at_exit)
            } else {
                json!(null)
            }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["retryAfterExit"], retry_at_exit);
        assert_eq!(observation["storedAfterExit"], stored_at_exit);
        assert_eq!(observation["recoveryRefused"], false);
        if waiting {
            // Before the deadline, the re-sent request withdraws the retry
            // once, or replays the withdrawal the worker committed.
            assert_eq!(observation["retryAfterRestart"], retry_at_exit);
            assert_eq!(observation["restartIsNoop"], true);
            assert_eq!(
                observation["reissue"],
                if committed { "replayed" } else { "withdrawn" }
            );
            assert_eq!(observation["retryAfterReissue"], "cancelled");
            assert_eq!(observation["storedAfterReissue"], "withdrawn");
            assert_eq!(observation["resend"], "replayed");
        } else {
            // After it, the restart settles a waiting retry timed out, and the
            // re-sent request replays the stored refusal or is refused afresh.
            assert_eq!(observation["retryAfterRestart"], "timed_out");
            assert_eq!(observation["restartIsNoop"], committed);
            assert_eq!(
                observation["reissue"],
                if committed {
                    "refusal_replayed"
                } else {
                    "refused"
                }
            );
            assert_eq!(observation["retryAfterReissue"], "timed_out");
            assert_eq!(
                observation["storedAfterReissue"],
                if committed {
                    "timeout_refusal"
                } else {
                    "attempt_refusal"
                }
            );
            assert_eq!(observation["resend"], "refusal_replayed");
        }
        assert_eq!(observation["passIsNoop"], true);
        assert_eq!(observation["launches"], 0);
        assert_eq!(observation["stops"], 0);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "waiting-retry-cancellation-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_run_now_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/run-now-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "run-now-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "run-now-process-crash-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 9);
    assert_eq!(response["evidence"]["passedCases"], 9);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("run-now crash observations");
    assert_eq!(observations.len(), 9);
    // What the store had committed when the worker was gone, and whether a
    // write was open there.
    for (observation, (boundary, committed, open)) in observations.iter().zip([
        ("after_claim", "absent", true),
        ("after_adoption", "absent", true),
        ("after_claim_commit", "claimed", false),
        ("after_session", "claimed", true),
        ("after_dispatching", "claimed", true),
        ("after_launch_commit", "launching", false),
        ("after_ownership", "launching", true),
        ("after_ownership_commit", "owned", false),
        ("none", "owned", false),
    ]) {
        let killed = boundary != "none";
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(
            observation["transactionOpenAtPause"],
            if killed { json!(open) } else { json!(null) }
        );
        assert_eq!(
            observation["launchAtPause"],
            if killed {
                json!(committed)
            } else {
                json!(null)
            }
        );
        let stored = if committed == "absent" {
            "none"
        } else {
            "claimed"
        };
        assert_eq!(
            observation["storedAtPause"],
            if killed { json!(stored) } else { json!(null) }
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["launchAfterExit"], committed);
        assert_eq!(observation["storedAfterExit"], stored);
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["resend"], "replayed");
        assert_eq!(observation["replayIsNoop"], true);
        // Only a request that found nothing launches, once; every stored
        // claim replays.
        assert_eq!(
            observation["reissue"],
            if committed == "absent" {
                "launched"
            } else {
                "replayed"
            }
        );
        assert_eq!(
            observation["launches"],
            if committed == "absent" { 1 } else { 0 }
        );
        let (after_restart, after_pass, after_deadline, after_lease, stops) = match committed {
            "absent" => ("absent", "succeeded", "succeeded", "succeeded", 0),
            // The restart fails a claim that never launched at once.
            "claimed" => ("abandoned", "abandoned", "abandoned", "abandoned", 0),
            // It contains the previous daemon's launch, as it does a
            // scheduled one, and holds the run until its deadline (#1502).
            _ => ("held", "held", "held_expired", "held_expired", 0),
        };
        // Every restart that finds the worker's claim writes.
        assert_eq!(observation["restartIsNoop"], committed == "absent");
        assert_eq!(observation["launchAfterRestart"], after_restart);
        assert_eq!(observation["launchAfterPass"], after_pass);
        assert_eq!(observation["launchAfterDeadline"], after_deadline);
        assert_eq!(observation["launchAfterLease"], after_lease);
        assert_eq!(observation["stops"], stops);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "run-now-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!text.contains("occ-manual-"));
    assert!(!text.contains("adopt:"));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_run_now_envelope_process_crash_vectors() -> anyhow::Result<()>
{
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/run-now-envelope-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "run-now-envelope-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "run-now-envelope-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 9);
    assert_eq!(response["evidence"]["passedCases"], 9);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("run-now envelope crash observations");
    assert_eq!(observations.len(), 9);
    // What the store had committed when the worker was gone, and whether a
    // write was open there.
    for (observation, (boundary, committed, stored, open)) in observations.iter().zip([
        ("after_reservation", "absent", "none", true),
        ("after_adoption", "absent", "none", true),
        ("after_claim_commit", "claimed", "pending", false),
        ("after_launch_commit", "launching", "pending", false),
        ("after_ownership_commit", "owned", "pending", false),
        ("after_consumed", "owned", "pending", true),
        ("after_result", "owned", "pending", true),
        ("after_result_commit", "owned", "retained", false),
        ("none", "owned", "retained", false),
    ]) {
        let killed = boundary != "none";
        let at_pause = |value: Value| if killed { value } else { json!(null) };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(observation["transactionOpenAtPause"], at_pause(json!(open)));
        assert_eq!(observation["launchAtPause"], at_pause(json!(committed)));
        assert_eq!(observation["storedAtPause"], at_pause(json!(stored)));
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["launchAfterExit"], committed);
        assert_eq!(observation["storedAfterExit"], stored);
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["replayIsNoop"], true);
        // Only a request that found nothing launches, once, and retains its
        // result. A claim with no retained result is refused as ambiguous
        // every time; a retained result is replayed.
        let (reissue, resend, stored_after) = match stored {
            "none" => ("launched", "replayed", "retained"),
            "pending" => ("ambiguous", "ambiguous", "pending"),
            _ => ("replayed", "replayed", "retained"),
        };
        assert_eq!(observation["reissue"], reissue);
        assert_eq!(observation["storedAfterReissue"], stored_after);
        assert_eq!(observation["resend"], resend);
        assert_eq!(observation["storedAfterResend"], stored_after);
        assert_eq!(
            observation["launches"],
            if committed == "absent" { 1 } else { 0 }
        );
        let (after_restart, after_pass, after_deadline, after_lease, stops) = match committed {
            "absent" => ("absent", "succeeded", "succeeded", "succeeded", 0),
            // The restart fails a claim that never launched at once.
            "claimed" => ("abandoned", "abandoned", "abandoned", "abandoned", 0),
            // It contains the previous daemon's launch, as it does a
            // scheduled one, and holds the run until its deadline (#1502).
            _ => ("held", "held", "held_expired", "held_expired", 0),
        };
        // Every restart that finds the worker's claim writes.
        assert_eq!(observation["restartIsNoop"], committed == "absent");
        assert_eq!(observation["launchAfterRestart"], after_restart);
        assert_eq!(observation["launchAfterPass"], after_pass);
        assert_eq!(observation["launchAfterDeadline"], after_deadline);
        assert_eq!(observation["launchAfterLease"], after_lease);
        assert_eq!(observation["stops"], stops);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "run-now-envelope-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!text.contains("occ-manual-"));
    assert!(!text.contains("adopt:"));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_tick_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/tick-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "tick-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "tick-process-crash-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 14);
    assert_eq!(response["evidence"]["passedCases"], 14);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("tick crash observations");
    assert_eq!(observations.len(), 14);
    // What the store had committed when the worker was gone, and whether a
    // write was open there.
    let boundaries = [
        ("after_plan", "absent", 0, true),
        ("after_plan_commit", "planned", 0, false),
        ("after_cursor", "planned", 0, true),
        ("after_cursor_commit", "planned", 1, false),
        ("after_claim", "planned", 1, true),
        ("after_claim_commit", "claimed", 1, false),
        ("none", "claimed", 1, false),
    ];
    let cases = ["prompt", "late"]
        .into_iter()
        .flat_map(|scene| boundaries.map(|boundary| (scene, boundary)));
    for (observation, (scene, (boundary, committed, cursor, open))) in
        observations.iter().zip(cases)
    {
        let killed = boundary != "none";
        let at_pause = |value: Value| if killed { value } else { json!(null) };
        assert_eq!(observation["scene"], scene);
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(observation["transactionOpenAtPause"], at_pause(json!(open)));
        assert_eq!(observation["slotAtPause"], at_pause(json!(committed)));
        assert_eq!(observation["cursorAtPause"], at_pause(json!(cursor)));
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["slotAfterExit"], committed);
        assert_eq!(observation["cursorAfterExit"], cursor);
        assert_eq!(observation["recoveryRefused"], false);
        // Exactly one slot launches, once, and nothing stops.
        assert_eq!(observation["restartLaunches"], 1);
        assert_eq!(observation["launches"], 1);
        assert_eq!(observation["stops"], 0);
        assert_eq!(observation["replayIsNoop"], true);
        let (slot, next_slot, claims) = if scene == "prompt" {
            // The slot itself launches; a committed claim with no run is
            // restored to planned and claimed a second time.
            (
                "succeeded",
                "absent",
                if committed == "claimed" { 2 } else { 1 },
            )
        } else {
            // The next day's slot launches instead. The missed slot is
            // superseded if it was planned, its restored claim included.
            let missed = match committed {
                "absent" => "absent",
                "planned" => "skipped",
                _ => "skipped_claim",
            };
            (missed, "succeeded", 1)
        };
        let launched = |settled: &'static str| {
            if settled == "succeeded" {
                "owned"
            } else {
                settled
            }
        };
        assert_eq!(observation["slotAfterRestart"], launched(slot));
        assert_eq!(observation["nextSlotAfterRestart"], launched(next_slot));
        assert_eq!(observation["claims"], claims);
        assert_eq!(observation["slotAfterPass"], slot);
        assert_eq!(observation["nextSlotAfterPass"], next_slot);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "tick-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    // No routine or occurrence id: occurrence ids extend the routine's.
    assert!(!text.contains("conformance-tick-crash"));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_no_process_receipt_process_crash_vectors(
) -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/no-process-receipt-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "no-process-receipt-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "no-process-receipt-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 10);
    assert_eq!(response["evidence"]["passedCases"], 10);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("no-process receipt crash observations");
    assert_eq!(observations.len(), 10);
    // What the store had committed when the worker was gone, and whether a
    // write was open there. The receipt is always recorded first.
    let boundaries = [
        ("after_receipt", "launching", false),
        ("after_run_delete", "launching", true),
        ("after_restore", "launching", true),
        ("after_restore_commit", "restored", false),
        ("none", "restored", false),
    ];
    let cases = ["no_retry", "lease_retry"]
        .into_iter()
        .flat_map(|scene| boundaries.map(|boundary| (scene, boundary)));
    for (observation, (scene, (boundary, committed, open))) in observations.iter().zip(cases) {
        let killed = boundary != "none";
        let at_pause = |value: Value| if killed { value } else { json!(null) };
        assert_eq!(observation["scene"], scene);
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(observation["transactionOpenAtPause"], at_pause(json!(open)));
        assert_eq!(observation["launchAtPause"], at_pause(json!(committed)));
        assert_eq!(observation["receiptAtPause"], at_pause(json!(true)));
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["launchAfterExit"], committed);
        assert_eq!(observation["receiptAfterExit"], true);
        assert_eq!(observation["recoveryRefused"], false);
        // Exactly one launch, after the restart; nothing stops; the slot is
        // claimed twice in all.
        assert_eq!(observation["launches"], 1);
        assert_eq!(observation["stops"], 0);
        assert_eq!(observation["claims"], 2);
        assert_eq!(observation["replayIsNoop"], true);
        // The receipt recovers a launch the dispatcher did not restore. Under
        // a lease-expiry retry policy it settles the attempt and retries it
        // after the policy's delay; otherwise it restores the launch as the
        // dispatcher would have, and the restart launches it again.
        let (after_restart, restart_launches, after_retry, after_settle) =
            if scene == "lease_retry" && committed == "launching" {
                ("retry_waiting", 0, "retry_owned", "retry_succeeded")
            } else {
                ("owned", 1, "owned", "succeeded")
            };
        assert_eq!(observation["launchAfterRestart"], after_restart);
        assert_eq!(observation["restartLaunches"], restart_launches);
        assert_eq!(observation["launchAfterRetryPass"], after_retry);
        assert_eq!(observation["launchAfterSettlePass"], after_settle);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "no-process-receipt-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    // No routine, occurrence, run or session id: occurrence ids extend the
    // routine's, and session ids name receipts.
    assert!(!text.contains("conformance-receipt-crash"));
    assert!(!text.contains("session-"));
    assert!(!text.contains(".receipt"));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_event_feed_process_crash_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/event-feed-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "event-feed-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "event-feed-process-crash-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 5);
    assert_eq!(response["evidence"]["passedCases"], 5);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("event feed crash observations");
    assert_eq!(observations.len(), 5);
    // Whether a write was open at the pause, whether the second routine's
    // claim and the second checkpoint had committed, and whether the client
    // received the second page.
    let cases = [
        ("inside_claim", true, false, false),
        ("after_claim_commit", false, true, false),
        ("inside_checkpoint", true, true, false),
        ("after_checkpoint_commit", false, true, true),
        ("none", false, true, true),
    ];
    for (observation, (boundary, open, claimed, second_checkpoint)) in
        observations.iter().zip(cases)
    {
        let killed = boundary != "none";
        let at_pause = |value: Value| if killed { value } else { json!(null) };
        let mut feed = vec![
            "first:occurrence:planned",
            "first:occurrence:claimed",
            "second:occurrence:planned",
        ];
        if claimed {
            feed.push("second:occurrence:claimed");
        }
        let checkpoints = if second_checkpoint { 2 } else { 1 };
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(observation["transactionOpenAtPause"], at_pause(json!(open)));
        assert_eq!(observation["feedAtPause"], at_pause(json!(feed)));
        assert_eq!(
            observation["checkpointsAtPause"],
            at_pause(json!(checkpoints))
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        // The killed claim's event never commits; a lost page's checkpoint
        // may.
        assert_eq!(observation["feedAfterExit"], json!(feed));
        assert_eq!(observation["checkpointsAfterExit"], checkpoints);
        // The client received the first page, with two events, and the
        // second, with two more, only when the worker was not killed.
        assert_eq!(observation["deliveredPages"], if killed { 1 } else { 2 });
        assert_eq!(observation["deliveredEvents"], if killed { 2 } else { 4 });
        assert_eq!(observation["deliveredCheckpointsDurable"], true);
        assert_eq!(observation["recoveryRefused"], false);
        // The restart restores a committed claim and launches both slots.
        let second_path = if claimed {
            json!(["planned", "claimed", "planned", "claimed", "running"])
        } else {
            json!(["planned", "claimed", "running"])
        };
        assert_eq!(
            observation["firstOccurrencePath"],
            json!(["planned", "claimed", "planned", "claimed", "running"])
        );
        assert_eq!(observation["secondOccurrencePath"], second_path);
        let run_path = json!([
            "run:running",
            "attempt:adopted",
            "attempt:dispatching",
            "attempt:started"
        ]);
        assert_eq!(observation["firstRunPath"], run_path);
        assert_eq!(observation["secondRunPath"], run_path);
        let feed_events = if claimed { 18 } else { 16 };
        assert_eq!(observation["feedEvents"], feed_events);
        assert_eq!(
            observation["resumedEvents"],
            feed_events - if killed { 2 } else { 4 }
        );
        assert_eq!(observation["launches"], 2);
        // Every committed event exactly once, in order, by checkpoint and by
        // cursor alike, and nothing after the last page.
        assert_eq!(observation["exactlyOnce"], true);
        assert_eq!(observation["resumePathsAgree"], true);
        assert_eq!(observation["resumeRefused"], false);
        assert_eq!(observation["streamsContiguous"], true);
        assert_eq!(observation["finalResumeEmpty"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "event-feed-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    // No routine, occurrence, run, event or checkpoint id.
    assert!(!text.contains("conformance-feed-crash"));
    assert!(!text.contains("run-"));
    assert!(!text.contains("\"evt"));
    assert!(!text.contains("ecp"));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_cancellation_timeout_process_crash_vectors(
) -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/cancellation-timeout-process-crash-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "cancellation-timeout-process-crash-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response["suiteId"],
        "cancellation-timeout-process-crash-recovery"
    );
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 16);
    assert_eq!(response["evidence"]["passedCases"], 16);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("cancellation timeout crash observations");
    assert_eq!(observations.len(), 16);
    // Scene and boundary; a write open at the pause; the run and the
    // cancellation the kill left; the worker's stops; the run after the
    // restart; the owner's first request; the run, the cancellation and the
    // request after the pass; and the run after a second pass.
    let cases = [
        (
            "cancellation",
            "inside_reservation",
            true,
            "running",
            "none",
            0,
            "ended_after_deadline",
            "refused",
            "ended_after_deadline",
            "refused",
            "rejected",
            "ended_after_deadline",
        ),
        (
            "cancellation",
            "after_reservation_commit",
            false,
            "running",
            "requested",
            0,
            "ended_after_deadline",
            "pending",
            "ended_after_deadline",
            "rejected",
            "rejected",
            "ended_after_deadline",
        ),
        (
            "cancellation",
            "inside_fence",
            true,
            "running",
            "requested",
            0,
            "ended_after_deadline",
            "pending",
            "ended_after_deadline",
            "rejected",
            "rejected",
            "ended_after_deadline",
        ),
        (
            "cancellation",
            "after_fence_commit",
            false,
            "cancellation_fenced",
            "stopping",
            0,
            "cancellation_contained",
            "pending",
            "failed_after_cancellation",
            "recovery_required",
            "recovery_required",
            "failed_after_cancellation",
        ),
        (
            "cancellation",
            "after_stop",
            false,
            "cancellation_fenced",
            "stopping",
            1,
            "cancellation_contained",
            "pending",
            "failed_after_cancellation",
            "recovery_required",
            "recovery_required",
            "failed_after_cancellation",
        ),
        (
            "cancellation",
            "inside_settlement",
            true,
            "cancellation_fenced",
            "stopping",
            1,
            "cancellation_contained",
            "pending",
            "failed_after_cancellation",
            "recovery_required",
            "recovery_required",
            "failed_after_cancellation",
        ),
        (
            "cancellation",
            "after_settle_commit",
            false,
            "cancelled",
            "stopping",
            1,
            "cancelled",
            "pending",
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
        ),
        (
            "cancellation",
            "inside_answer",
            true,
            "cancelled",
            "stopping",
            1,
            "cancelled",
            "pending",
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
        ),
        (
            "cancellation",
            "after_answer_commit",
            false,
            "cancelled",
            "cancelled",
            1,
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
        ),
        (
            "cancellation",
            "none",
            false,
            "cancelled",
            "cancelled",
            1,
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
            "cancelled",
        ),
        (
            "timeout",
            "inside_fence",
            true,
            "running",
            "none",
            0,
            "ended_after_deadline",
            "refused",
            "ended_after_deadline",
            "refused",
            "rejected",
            "ended_after_deadline",
        ),
        (
            "timeout",
            "after_fence_commit",
            false,
            "timeout_fenced",
            "none",
            0,
            "timeout_contained",
            "refused",
            "ambiguous",
            "refused",
            "rejected",
            "failed_after_timeout",
        ),
        (
            "timeout",
            "after_stop",
            false,
            "timeout_fenced",
            "none",
            1,
            "timeout_contained",
            "refused",
            "ambiguous",
            "refused",
            "rejected",
            "failed_after_timeout",
        ),
        (
            "timeout",
            "inside_settlement",
            true,
            "timeout_fenced",
            "none",
            1,
            "timeout_contained",
            "refused",
            "ambiguous",
            "refused",
            "rejected",
            "failed_after_timeout",
        ),
        (
            "timeout",
            "after_settle_commit",
            false,
            "timed_out",
            "none",
            1,
            "timed_out",
            "refused",
            "timed_out",
            "refused",
            "rejected",
            "timed_out",
        ),
        (
            "timeout",
            "none",
            false,
            "timed_out",
            "none",
            1,
            "timed_out",
            "refused",
            "timed_out",
            "refused",
            "rejected",
            "timed_out",
        ),
    ];
    for (
        observation,
        (
            scene,
            boundary,
            open,
            run_exit,
            cancellation_exit,
            stops,
            run_restart,
            request_answer,
            run_pass,
            cancellation_pass,
            resend,
            run_settle,
        ),
    ) in observations.iter().zip(cases)
    {
        let killed = boundary != "none";
        let at_pause = |value: Value| if killed { value } else { json!(null) };
        assert_eq!(observation["scene"], scene);
        assert_eq!(observation["boundary"], boundary);
        assert_eq!(observation["abruptlyTerminated"], killed);
        assert_eq!(observation["transactionOpenAtPause"], at_pause(json!(open)));
        assert_eq!(observation["runAtPause"], at_pause(json!(run_exit)));
        assert_eq!(
            observation["cancellationAtPause"],
            at_pause(json!(cancellation_exit))
        );
        assert_eq!(observation["pauseConsistent"], true);
        assert_eq!(observation["integrityOk"], true);
        assert_eq!(observation["runAfterExit"], run_exit);
        assert_eq!(observation["cancellationAfterExit"], cancellation_exit);
        // At most one stop, the worker's.
        assert_eq!(observation["workerStops"], stops);
        assert_eq!(observation["recoveryRefused"], false);
        assert_eq!(observation["runAfterRestart"], run_restart);
        assert_eq!(observation["cancellationAfterRestart"], cancellation_exit);
        // A settled run gives the restart nothing to do.
        assert_eq!(
            observation["restartIsNoop"],
            matches!(run_exit, "cancelled" | "timed_out")
        );
        assert_eq!(observation["request"], request_answer);
        assert_eq!(observation["runAfterPass"], run_pass);
        assert_eq!(observation["cancellationAfterPass"], cancellation_pass);
        assert_eq!(observation["resend"], resend);
        assert_eq!(observation["runAfterSettle"], run_settle);
        // The restart, the passes and the owner's requests stop nothing,
        // launch nothing, and a final request and pass change nothing.
        assert_eq!(observation["laterStops"], 0);
        assert_eq!(observation["launches"], 0);
        assert_eq!(observation["replayIsNoop"], true);
    }
    let mut invalid = request.clone();
    invalid["vector"]["cases"][0]["storePath"] = json!("private-must-not-be-read");
    let rejected = run_target(&coven_home, "evaluate", Some(&invalid))?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("conformance vector is invalid"));
    let rejected = run_target(
        &coven_home,
        "cancellation-timeout-crash-worker",
        Some(&invalid["vector"]["cases"][0]),
    )?;
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("conformance vector is invalid"));
    assert!(!error.contains("private-must-not-be-read"));
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    // No routine, run or session id, adoption key or reason.
    assert!(!text.contains("conformance-race-crash"));
    assert!(!text.contains("run-"));
    assert!(!text.contains("adopt:"));
    assert!(!text.contains("not needed today"));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_storage_corruption_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/command-storage-corruption-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "command-storage-corruption-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "command-storage-corruption-recovery");
    assert_eq!(response["status"], "passed");
    assert_eq!(response["evidence"]["executedCases"], 4);
    assert_eq!(response["evidence"]["passedCases"], 4);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("corruption observations");
    assert_eq!(observations.len(), 4);
    for (observation, (fault, outcome, stage)) in observations.iter().zip([
        ("sqlite_header", "not_a_database", json!("cold_open")),
        ("definition_btree", "database_corrupt", json!("cold_open")),
        ("none", "committed", Value::Null),
        ("adoption_btree", "database_corrupt", json!("command")),
    ]) {
        assert_eq!(observation["faultSource"], fault);
        assert_eq!(observation["initialOutcome"], outcome);
        assert_eq!(observation["refusalStage"], stage);
        assert_eq!(observation["baselinePreserved"], true);
        assert_eq!(observation["storeNotReset"], true);
        assert_eq!(observation["finalAddedDefinitions"], 1);
        assert_eq!(observation["replayPreserved"], true);
    }
    let adoption = &observations[3];
    assert_eq!(adoption["refusedAfterEarlierWrites"], true);
    assert_eq!(adoption["initialTransactionClosed"], true);
    assert_eq!(adoption["refusalLeftMainFileUnchanged"], true);
    assert_eq!(adoption["walObservedAfterInitial"], false);
    for (field, value) in [
        ("offset", json!(0)),
        ("path", json!("private-invalid-value")),
        ("fault", json!("wal_checksum")),
    ] {
        let mut malformed = request.clone();
        malformed["vector"]["cases"][0][field] = value;
        let rejected = run_target(&coven_home, "evaluate", Some(&malformed))?;
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        let error = String::from_utf8_lossy(&rejected.stderr);
        assert!(error.contains("conformance vector is invalid"));
        assert!(!error.contains("private-invalid-value"));
    }
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}

#[test]
fn native_target_evaluates_checked_in_result_delivery_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../conformance/automations/runner/result-delivery-recovery.vectors.json"
    ))?;
    let request = json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "scheduler_reliability",
        "suiteId": "result-delivery-recovery",
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {"os": "linux", "arch": "x86_64"},
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vectors
    });

    let output = run_target(&coven_home, "evaluate", Some(&request))?;

    assert!(
        output.status.success(),
        "evaluate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["suiteId"], "result-delivery-recovery");
    assert_eq!(response["status"], "passed", "{response}");
    assert_eq!(response["evidence"]["executedCases"], 6);
    assert_eq!(response["evidence"]["passedCases"], 6);
    let observations = response["evidence"]["observations"]
        .as_array()
        .expect("delivery observations");
    for (observation, (fault, first, final_state)) in observations.iter().zip([
        ("none", "delivered", "delivered"),
        ("gate_held", "deferred", "delivered"),
        ("inbox_full", "deferred", "delivered"),
        ("discharge_rollback", "pass_failed", "delivered"),
        ("gate_unavailable", "failed", "failed"),
        ("unreadable_result", "failed", "failed"),
    ]) {
        assert_eq!(observation["faultSource"], fault);
        assert_eq!(observation["firstPass"], first);
        assert_eq!(observation["finalState"], final_state);
        assert_eq!(observation["replayIsNoop"], true);
        assert_eq!(observation["settledRecordImmutable"], true);
    }
    assert_eq!(observations[3]["passFailureFromFixture"], true);
    assert_eq!(observations[5]["healthyRunDelivered"], true);
    for (field, value) in [
        ("runId", json!("private-invalid-value")),
        ("path", json!("private-invalid-value")),
        ("fault", json!("process_kill")),
    ] {
        let mut malformed = request.clone();
        malformed["vector"]["cases"][0][field] = value;
        let rejected = run_target(&coven_home, "evaluate", Some(&malformed))?;
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        let error = String::from_utf8_lossy(&rejected.stderr);
        assert!(error.contains("conformance vector is invalid"));
        assert!(!error.contains("private-invalid-value"));
    }
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(temp_dir.path().to_string_lossy().as_ref()));
    assert!(!coven_home.exists());
    Ok(())
}
