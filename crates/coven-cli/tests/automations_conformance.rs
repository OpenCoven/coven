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
                        "command-transaction-rollback",
                        "legacy-import-source-preservation",
                        "command-storage-capacity-recovery",
                        "misfire-latest-planning",
                        "occurrence-lease-recovery",
                        "overlap-forbid-claiming",
                        "retry-backoff-timing",
                        "retry-quarantine-recovery",
                        "scheduler-leadership-fencing",
                        "startup-reconciliation-wake"
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

#[test]
fn native_target_evaluates_checked_in_startup_reconciliation_wake_vectors() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("must-not-be-created");
    let vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS)?;
    let request = json!({
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
    });

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
