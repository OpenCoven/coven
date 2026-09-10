use serde_json::{json, Value};

use super::conformance_target::{
    capability, evaluate, TargetSuiteStatus, CAPABILITY_NEGOTIATION_SUITE,
};

fn request(vector: Value) -> Value {
    json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": CAPABILITY_NEGOTIATION_SUITE,
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
            "platform": {
                "os": "linux",
                "arch": "x86_64"
            },
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vector
    })
}

#[test]
fn capability_advertises_only_the_native_structural_suite() {
    assert_eq!(
        serde_json::to_value(capability()).unwrap(),
        json!({
            "schemaVersion": "coven.automations.conformance-target-capability.v1",
            "profiles": [{
                "profile": "structural",
                "suites": [CAPABILITY_NEGOTIATION_SUITE]
            }]
        })
    );
}

#[test]
fn capability_negotiation_suite_executes_supported_and_refused_cases() {
    let response = evaluate(&request(json!({
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": [
            {
                "caseId": "supported-minimal",
                "definition": {
                    "schemaVersion": 1,
                    "id": "native-supported",
                    "name": "Native supported",
                    "status": "PAUSED",
                    "rrule": "FREQ=DAILY",
                    "timezone": "local",
                    "prompt": "Run the native conformance probe",
                    "misfire": "latest",
                    "overlap": "forbid",
                    "timeoutMinutes": 30,
                    "runtime": "coven-code"
                },
                "expected": {
                    "outcome": "supported"
                }
            },
            {
                "caseId": "refused-trigger",
                "definition": {
                    "schemaVersion": 1,
                    "id": "native-refused",
                    "name": "Native refused",
                    "status": "PAUSED",
                    "rrule": "FREQ=DAILY",
                    "timezone": "local",
                    "prompt": "Run the native conformance probe",
                    "misfire": "latest",
                    "overlap": "forbid",
                    "timeoutMinutes": 30,
                    "runtime": "coven-code",
                    "trigger": {
                        "variant": "webhook",
                        "version": 1,
                        "webhook": {}
                    }
                },
                "expected": {
                    "outcome": "unsupported",
                    "variant": "trigger.webhook"
                }
            }
        ]
    })))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(
        response.evidence,
        Some(json!({
            "executedCases": 2,
            "passedCases": 2,
            "vectorDigest": "sha256:fc716602d581388f683fbf9c81b588a7510b4eed308999dd89ea13f76ad5304b"
        }))
    );
}

#[test]
fn capability_negotiation_suite_fails_closed_on_an_expectation_mismatch() {
    let response = evaluate(&request(json!({
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": [{
            "caseId": "wrong-expectation",
            "definition": {
                "schemaVersion": 1,
                "id": "native-supported",
                "name": "Native supported",
                "status": "PAUSED",
                "rrule": "FREQ=DAILY",
                "timezone": "local",
                "prompt": "Run the native conformance probe",
                "misfire": "latest",
                "overlap": "forbid",
                "timeoutMinutes": 30,
                "runtime": "coven-code"
            },
            "expected": {
                "outcome": "unsupported",
                "variant": "webhook"
            }
        }]
    })))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn evaluator_rejects_unadvertised_suites_with_a_static_error() {
    let mut request = request(json!({
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": []
    }));
    request["suiteId"] = json!("scheduler-secret-output");

    assert_eq!(
        evaluate(&request).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn evaluator_rejects_malformed_vectors_with_a_static_error() {
    assert_eq!(
        evaluate(&request(json!({
            "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
            "cases": [{
                "caseId": "contains-secret",
                "definition": "SECRET-DEFINITION",
                "expected": {"outcome": "supported"}
            }]
        })))
        .unwrap_err(),
        "conformance vector is invalid"
    );
}
