use serde_json::{json, Value};

use super::conformance_target::{
    capability, evaluate, evaluate_run_terminal_monotonicity_case_counts, TargetSuiteStatus,
    CALENDAR_SCHEDULE_RESOLUTION_SUITE, CAPABILITY_NEGOTIATION_SUITE,
    MISFIRE_LATEST_PLANNING_SUITE, OCCURRENCE_LEASE_RECOVERY_SUITE, OVERLAP_FORBID_CLAIMING_SUITE,
    RUN_TERMINAL_MONOTONICITY_SUITE,
};

const ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/attempt-terminal-immutability.vectors.json"
);
const COMMAND_ADOPTION_IDEMPOTENCY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/command-adoption-idempotency.vectors.json"
);
const CALENDAR_SCHEDULE_RESOLUTION_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/calendar-schedule-resolution.vectors.json"
);
const DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/definition-lifecycle-transitions.vectors.json"
);
const DEFINITION_VALIDATION_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/definition-validation.vectors.json");
const EVENT_REDUCER_DETERMINISM_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/event-reducer-determinism.vectors.json"
);
const MISFIRE_LATEST_PLANNING_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/misfire-latest-planning.vectors.json");
const OCCURRENCE_LEASE_RECOVERY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/occurrence-lease-recovery.vectors.json"
);
const OVERLAP_FORBID_CLAIMING_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/overlap-forbid-claiming.vectors.json");
const OCCURRENCE_FENCE_UNIQUENESS_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/occurrence-fence-uniqueness.vectors.json"
);
const RECEIPT_INTEGRITY_VALIDATION_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/receipt-integrity-validation.vectors.json"
);
const RRULE_VOCABULARY_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/rrule-vocabulary.vectors.json");

fn request_for_profile(profile: &str, suite_id: &str, vector: Value) -> Value {
    json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": profile,
        "suiteId": suite_id,
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

fn request_for(suite_id: &str, vector: Value) -> Value {
    request_for_profile("structural", suite_id, vector)
}

fn request(vector: Value) -> Value {
    request_for(CAPABILITY_NEGOTIATION_SUITE, vector)
}

#[test]
fn capability_advertises_the_native_structural_suites() {
    assert_eq!(
        serde_json::to_value(capability()).unwrap(),
        json!({
            "schemaVersion": "coven.automations.conformance-target-capability.v1",
            "profiles": [
                {
                    "profile": "structural",
                    "suites": [
                        "attempt-terminal-immutability",
                        CAPABILITY_NEGOTIATION_SUITE,
                        "command-adoption-idempotency",
                        "definition-lifecycle-transitions",
                        "definition-validation",
                        "event-reducer-determinism",
                        "occurrence-fence-uniqueness",
                        "receipt-integrity-validation",
                        "rrule-vocabulary",
                        RUN_TERMINAL_MONOTONICITY_SUITE
                    ]
                },
                {
                    "profile": "scheduler_reliability",
                    "suites": [
                        CALENDAR_SCHEDULE_RESOLUTION_SUITE,
                        MISFIRE_LATEST_PLANNING_SUITE,
                        OCCURRENCE_LEASE_RECOVERY_SUITE,
                        OVERLAP_FORBID_CLAIMING_SUITE
                    ]
                }
            ]
        })
    );
}

#[test]
fn calendar_schedule_resolution_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        CALENDAR_SCHEDULE_RESOLUTION_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 11);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 11);
}

#[test]
fn calendar_schedule_resolution_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
    vectors["cases"][5]["expected"]["nextDueAt"] = json!("2026-03-08T07:00:00.000Z");

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        CALENDAR_SCHEDULE_RESOLUTION_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn calendar_schedule_resolution_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 7] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["from"] = json!("not-a-time"),
        |vectors| vectors["cases"][5]["timezone"] = json!("utc"),
        |vectors| {
            vectors["cases"][10]["expected"] = json!({
                "outcome": "scheduled",
                "nextDueAt": "2026-08-28T09:00:00.000Z"
            })
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                CALENDAR_SCHEDULE_RESOLUTION_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn calendar_schedule_resolution_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(CALENDAR_SCHEDULE_RESOLUTION_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn occurrence_lease_recovery_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OCCURRENCE_LEASE_RECOVERY_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 5);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 5);
}

#[test]
fn occurrence_lease_recovery_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["recoveredCount"] = json!(0);

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OCCURRENCE_LEASE_RECOVERY_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn occurrence_lease_recovery_suite_accepts_portable_case_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    vectors["cases"][0]["caseId"] = json!(format!("case:{}", "a".repeat(120)));

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OCCURRENCE_LEASE_RECOVERY_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
}

#[test]
fn occurrence_lease_recovery_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["now"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["initial"]["leaseExpiresAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["initial"]["runningRun"] = json!(true),
        |vectors| vectors["cases"][4]["expected"]["state"] = json!("failed"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                OCCURRENCE_LEASE_RECOVERY_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn occurrence_lease_recovery_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(OCCURRENCE_LEASE_RECOVERY_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn overlap_forbid_claiming_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OVERLAP_FORBID_CLAIMING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 7);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 7);
}

#[test]
fn overlap_forbid_claiming_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
    vectors["cases"][1]["expected"] = json!({
        "claimed": true,
        "targetState": "claimed",
        "targetAttempt": 1,
        "leaseOwner": "daemon-a",
        "leaseExpiresAt": "2026-09-01T11:00:00.000Z"
    });

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OVERLAP_FORBID_CLAIMING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn overlap_forbid_claiming_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["now"] = json!("not-a-time"),
        |vectors| vectors["cases"][2]["blocker"] = json!("claimed_occurrence"),
        |vectors| vectors["cases"][0]["expected"]["leaseExpiresAt"] = Value::Null,
        |vectors| vectors["cases"][1]["expected"]["targetAttempt"] = json!(1),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                OVERLAP_FORBID_CLAIMING_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn overlap_forbid_claiming_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(OVERLAP_FORBID_CLAIMING_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn misfire_latest_planning_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        MISFIRE_LATEST_PLANNING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 4);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 4);
}

#[test]
fn misfire_latest_planning_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["scheduledSlots"] = json!(["2026-09-03T09:00:00.000Z"]);

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        MISFIRE_LATEST_PLANNING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn misfire_latest_planning_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["createdAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["definition"]["status"] = json!("PAUSED"),
        |vectors| vectors["cases"][1]["existingScheduledFor"] = Value::Null,
        |vectors| vectors["cases"][2]["expected"]["firstOutcome"] = json!("planned"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                MISFIRE_LATEST_PLANNING_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn misfire_latest_planning_suite_rejects_vacuous_timing_scenarios() {
    let invalid_mutations: [fn(&mut Value); 5] = [
        |vectors| vectors["cases"][0]["createdAt"] = json!("2026-09-03T10:00:00.000Z"),
        |vectors| vectors["cases"][1]["observedAt"] = json!("2026-09-04T08:00:00.000Z"),
        |vectors| vectors["cases"][2]["createdAt"] = json!("2026-09-03T13:00:00.000Z"),
        |vectors| {
            vectors["cases"][2]["createdAt"] = json!("2026-09-03T08:00:00.000Z");
            vectors["cases"][2]["observedAt"] = json!("2026-09-03T08:30:00.000Z");
            vectors["cases"][2]["existingScheduledFor"] = json!("2026-09-03T09:00:00.000Z");
            vectors["cases"][2]["expected"]["scheduledSlots"] = json!(["2026-09-03T09:00:00.000Z"]);
        },
        |vectors| vectors["cases"][3]["observedAt"] = json!("2026-09-01T08:30:00.000Z"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                MISFIRE_LATEST_PLANNING_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn misfire_latest_planning_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(MISFIRE_LATEST_PLANNING_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn definition_lifecycle_transitions_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS).unwrap();
    let response = evaluate(&request_for("definition-lifecycle-transitions", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 17);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 17);
}

#[test]
fn definition_lifecycle_transitions_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value =
        serde_json::from_str(DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["finalState"] = json!("active");

    let response = evaluate(&request_for("definition-lifecycle-transitions", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn definition_lifecycle_transitions_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 6] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][3]["initialState"] = json!("active"),
        |vectors| vectors["cases"][3]["operation"] = json!("revise_paused"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("definition-lifecycle-transitions", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn rrule_vocabulary_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
    let response = evaluate(&request_for("rrule-vocabulary", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 22);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 22);
}

#[test]
fn rrule_vocabulary_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["byHour"] = json!([10]);

    let response = evaluate(&request_for("rrule-vocabulary", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn rrule_vocabulary_suite_reports_outcome_mismatches() {
    let mismatches: [fn(&mut Value); 2] = [
        |vectors| vectors["cases"][0]["expected"] = json!({"outcome": "rejected"}),
        |vectors| {
            vectors["cases"][4]["expected"] = json!({
                "outcome": "accepted",
                "frequency": "daily",
                "byHour": [9],
                "byDay": []
            })
        },
    ];

    for mismatch in mismatches {
        let mut vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
        mismatch(&mut vectors);
        let response = evaluate(&request_for("rrule-vocabulary", vectors)).unwrap();
        assert_eq!(response.status, TargetSuiteStatus::Failed);
        assert_eq!(response.evidence, None);
    }
}

#[test]
fn rrule_vocabulary_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 10] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][0]["rrule"] = json!(""),
        |vectors| vectors["cases"][0]["rrule"] = json!("x".repeat(1025)),
        |vectors| {
            vectors["cases"][2]["rrule"] = json!("FREQ=DAILY;BYHOUR=10");
            vectors["cases"][2]["expected"] = json!({
                "outcome": "accepted",
                "frequency": "daily",
                "byHour": [10],
                "byDay": []
            });
        },
        |vectors| vectors["cases"][4]["rrule"] = json!("FREQ=DAILY;COUNT=4"),
        |vectors| vectors["cases"][0]["expected"]["byHour"] = json!([17, 9]),
        |vectors| vectors["cases"][2]["expected"]["byDay"] = json!([]),
        |vectors| {
            vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();
        },
        |vectors| {
            vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone();
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("rrule-vocabulary", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn command_adoption_idempotency_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(COMMAND_ADOPTION_IDEMPOTENCY_VECTORS).unwrap();
    let response = evaluate(&request_for("command-adoption-idempotency", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 1);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 1);
}

#[test]
fn command_adoption_idempotency_suite_rejects_impossible_expectations() {
    let mut vectors: Value = serde_json::from_str(COMMAND_ADOPTION_IDEMPOTENCY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["eventRows"] = json!(2);

    assert_eq!(
        evaluate(&request_for("command-adoption-idempotency", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn command_adoption_idempotency_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][0]["adoptionKey"] = json!("bad key"),
        |vectors| vectors["cases"][0]["definition"] = json!("not-an-object"),
        |vectors| {
            vectors["cases"][0]["conflictingDefinition"]["id"] = json!("different-automation")
        },
        |vectors| {
            vectors["cases"][0]["conflictingDefinition"] = vectors["cases"][0]["definition"].clone()
        },
        |vectors| {
            let mut duplicate = vectors["cases"][0].clone();
            duplicate["adoptionKey"] = json!("adopt:create:conformance:0002");
            vectors["cases"].as_array_mut().unwrap().push(duplicate);
        },
        |vectors| {
            let mut duplicate = vectors["cases"][0].clone();
            duplicate["caseId"] = json!("second-case");
            vectors["cases"].as_array_mut().unwrap().push(duplicate);
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(COMMAND_ADOPTION_IDEMPOTENCY_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("command-adoption-idempotency", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn receipt_integrity_validation_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(RECEIPT_INTEGRITY_VALIDATION_VECTORS).unwrap();
    let response = evaluate(&request_for("receipt-integrity-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn receipt_integrity_validation_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(RECEIPT_INTEGRITY_VALIDATION_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["normalizedDigest"] =
        json!("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    let response = evaluate(&request_for("receipt-integrity-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn receipt_integrity_validation_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 9] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| {
            vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();
        },
        |vectors| vectors["cases"][0]["receipt"] = json!("not-an-object"),
        |vectors| {
            vectors["cases"][1]["receipt"] = json!({ "schemaVersion": "coven.automations.v1" })
        },
        |vectors| {
            vectors["cases"][1]["receipt"]["integrity"]["value"] = json!("not-a-sha256-digest")
        },
        |vectors| {
            vectors["cases"][0]["expected"]["normalizedDigest"] = json!("sha256:not-a-digest");
        },
        |vectors| vectors["cases"][0]["expected"] = json!({"outcome": "rejected"}),
        |vectors| {
            vectors["cases"][1]["expected"] = json!({
                "outcome": "accepted",
                "normalizedDigest":
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            });
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(RECEIPT_INTEGRITY_VALIDATION_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("receipt-integrity-validation", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn occurrence_fence_uniqueness_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    let response = evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 3);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 3);
}

#[test]
fn occurrence_fence_uniqueness_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["secondCommitted"] = json!(true);

    let response = evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn occurrence_fence_uniqueness_suite_requires_all_scenarios() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"].as_array_mut().unwrap().pop();

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_reused_occurrence_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][1]["first"]["occurrenceId"] =
        vectors["cases"][0]["first"]["occurrenceId"].clone();

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_mismatched_scenarios() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["second"]["scheduledFor"] = json!("2026-08-31T09:00:00.000Z");

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_checks_first_row_preservation() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["firstPreserved"] = json!(false);

    let response = evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_noncanonical_timestamps() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["first"]["scheduledFor"] = json!("2026-08-30T09:00:00Z");

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_contract_invalid_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["first"]["automationId"] = json!("daily:notes");

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_impossible_zero_row_expectations() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["rowCount"] = json!(0);

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    let response = evaluate(&request_for("event-reducer-determinism", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 1);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 1);
}

#[test]
fn event_reducer_determinism_suite_fails_closed_on_a_digest_mismatch() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["expectedStateDigest"] = json!(format!("sha256:{}", "0".repeat(64)));

    let response = evaluate(&request_for("event-reducer-determinism", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn event_reducer_determinism_suite_rejects_an_invalid_duplicate_index() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["duplicateIndex"] = json!(3);

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    let duplicate = vectors["cases"][0].clone();
    vectors["cases"].as_array_mut().unwrap().push(duplicate);

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_rejects_duplicate_canonical_event_ids() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    let duplicate = vectors["cases"][0]["events"][1].clone();
    vectors["cases"][0]["events"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_rejects_malformed_expected_digests() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["expectedStateDigest"] = json!("sha256:not-a-digest");

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_fails_closed_on_out_of_order_events() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["events"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);

    let response = evaluate(&request_for("event-reducer-determinism", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn attempt_terminal_immutability_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    let response = evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 5);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 5);
}

#[test]
fn attempt_terminal_immutability_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["updateCommitted"] = json!(true);

    let response = evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn attempt_terminal_immutability_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn attempt_terminal_immutability_suite_rejects_nonterminal_start_states() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][0]["firstState"] = json!("observing");

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn attempt_terminal_immutability_suite_rejects_noop_updates() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][0]["attemptedState"] = vectors["cases"][0]["firstState"].clone();

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn attempt_terminal_immutability_suite_requires_every_terminal_state() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"].as_array_mut().unwrap().pop();

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn definition_validation_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    let response = evaluate(&request_for("definition-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn definition_validation_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    vectors["cases"][0]["expected"] = json!({"outcome": "rejected"});

    let response = evaluate(&request_for("definition-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn definition_validation_suite_rejects_malformed_expected_digests() {
    let mut vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["normalizedDigest"] = json!("sha256:not-a-digest");

    assert_eq!(
        evaluate(&request_for("definition-validation", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn definition_validation_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();

    assert_eq!(
        evaluate(&request_for("definition-validation", vectors)).unwrap_err(),
        "conformance vector is invalid"
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
fn run_terminal_monotonicity_suite_executes_the_real_run_ledger() {
    let response = evaluate(&request_for(
        RUN_TERMINAL_MONOTONICITY_SUITE,
        json!({
            "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
            "cases": [
                {
                    "caseId": "succeeded-cannot-be-rewritten",
                    "firstStatus": "succeeded",
                    "replayStatus": "failed",
                    "expected": {
                        "firstCommitted": true,
                        "replayCommitted": false,
                        "finalStatus": "succeeded"
                    }
                },
                {
                    "caseId": "failed-cannot-be-rewritten",
                    "firstStatus": "failed",
                    "replayStatus": "succeeded",
                    "expected": {
                        "firstCommitted": true,
                        "replayCommitted": false,
                        "finalStatus": "failed"
                    }
                }
            ]
        }),
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn run_terminal_monotonicity_suite_fails_closed_on_an_expectation_mismatch() {
    let response = evaluate(&request_for(
        RUN_TERMINAL_MONOTONICITY_SUITE,
        json!({
            "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
            "cases": [{
                "caseId": "rewrite-wrongly-expected",
                "firstStatus": "failed",
                "replayStatus": "succeeded",
                "expected": {
                    "firstCommitted": true,
                    "replayCommitted": true,
                    "finalStatus": "succeeded"
                }
            }]
        }),
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn run_terminal_monotonicity_suite_rejects_non_conflicting_replay_vectors() {
    assert_eq!(
        evaluate(&request_for(
            RUN_TERMINAL_MONOTONICITY_SUITE,
            json!({
                "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
                "cases": [{
                    "caseId": "duplicate-succeeded-replay",
                    "firstStatus": "succeeded",
                    "replayStatus": "succeeded",
                    "expected": {
                        "firstCommitted": true,
                        "replayCommitted": false,
                        "finalStatus": "succeeded"
                    }
                }]
            }),
        ))
        .unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn run_terminal_monotonicity_suite_continues_executing_after_a_mismatch() {
    let vector = json!({
        "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
        "cases": [
            {
                "caseId": "rewrite-wrongly-expected",
                "firstStatus": "failed",
                "replayStatus": "succeeded",
                "expected": {
                    "firstCommitted": true,
                    "replayCommitted": true,
                    "finalStatus": "succeeded"
                }
            },
            {
                "caseId": "cancelled-cannot-be-rewritten",
                "firstStatus": "cancelled",
                "replayStatus": "timed_out",
                "expected": {
                    "firstCommitted": true,
                    "replayCommitted": false,
                    "finalStatus": "cancelled"
                }
            }
        ]
    });

    assert_eq!(
        evaluate_run_terminal_monotonicity_case_counts(&vector).unwrap(),
        (2, 1)
    );
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
