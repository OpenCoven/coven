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
const CAPABILITY_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/capability-negotiation.vectors.json");
const DEFINITION_VALIDATION_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/definition-validation.vectors.json");
const EVENT_REDUCER_DETERMINISM_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/event-reducer-determinism.vectors.json");
const OCCURRENCE_FENCE_UNIQUENESS_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/occurrence-fence-uniqueness.vectors.json"
);
const RECEIPT_INTEGRITY_VALIDATION_VECTORS: &str = include_str!(
    "../../../conformance/automations/runner/receipt-integrity-validation.vectors.json"
);
const RUN_TERMINAL_MONOTONICITY_VECTORS: &str =
    include_str!("../../../conformance/automations/runner/run-terminal-monotonicity.vectors.json");
const MAX_CONFORMANCE_REQUEST_BYTES: usize = 1024 * 1024;

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn run_target(coven_home: &Path, operation: &str, input: Option<&Value>) -> anyhow::Result<Output> {
    let mut command = Command::new(coven_bin());
    command
        .args(["automations", "conformance", operation])
        .env("COVEN_HOME", coven_home)
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
            "profiles": [{
                "profile": "structural",
                "suites": [
                    "attempt-terminal-immutability",
                    "capability-negotiation",
                    "command-adoption-idempotency",
                    "definition-validation",
                    "event-reducer-determinism",
                    "occurrence-fence-uniqueness",
                    "receipt-integrity-validation",
                    "run-terminal-monotonicity"
                ]
            }]
        })
    );
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
