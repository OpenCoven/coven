use super::*;

fn fixture() -> (tempfile::TempDir, Connection) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("coven.sqlite3");
    crate::store::initialize_store(&path).unwrap();
    let conn = crate::store::open_store(&path).unwrap();
    (root, conn)
}

#[test]
fn missing_pass_is_not_measured_zero_duration_or_failure() {
    let (_root, conn) = fixture();
    let report = inspect(&conn, Utc::now()).unwrap();
    for category in [Category::LastSuccessfulPass, Category::PlanningStartLag] {
        let observation = report
            .observations
            .iter()
            .find(|v| v.category == category)
            .unwrap();
        for fact in [
            Fact::LatestPassSucceeded,
            Fact::LatestPassDurationMs,
            Fact::LatestPassStartLagMs,
            Fact::LastSuccessfulPassFinishedAtMillis,
        ] {
            assert!(
                !observation.facts.contains_key(&fact),
                "absent pass is unobserved, not zero-valued evidence"
            );
        }
    }
}

#[test]
fn report_rejects_a_gap_from_another_category() {
    let (_root, conn) = fixture();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    report.observations[0].gaps = vec![Gap::ReceiptIntegrityNotVerified];
    assert!(
        report.validate().is_err(),
        "category-mismatched gap was accepted"
    );
}

#[test]
fn report_rejects_a_next_step_from_another_category() {
    let (_root, conn) = fixture();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    report.observations[0].next_step = NextStep::ReviewAuthorityGate;
    assert!(
        report.validate().is_err(),
        "category-mismatched next step was accepted"
    );
}

#[test]
fn report_rejects_missing_required_queue_facts() {
    let (_root, conn) = fixture();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    report
        .observations
        .iter_mut()
        .find(|v| v.category == Category::Queue)
        .unwrap()
        .facts
        .clear();
    assert!(
        report.validate().is_err(),
        "missing queue facts were accepted"
    );
}

#[test]
fn report_rejects_a_non_boolean_leader_presence() {
    let (_root, conn) = fixture();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    report.observations[0].facts.insert(Fact::LeaderAssigned, 2);
    assert!(
        report.validate().is_err(),
        "non-boolean presence was accepted"
    );
}

#[test]
fn last_successful_pass_survives_a_later_failed_pass() {
    use crate::automations::diagnostics::{record_scheduler_pass, SchedulerPassRecord};
    let (root, conn) = fixture();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let leadership =
        crate::automations::leadership::SchedulerLeadership::acquire(root.path(), &conn, now)
            .unwrap();
    let succeeded_at = now + chrono::Duration::milliseconds(25);
    for (offset, status, error_class) in [
        (0, "succeeded", None),
        (1, "failed", Some("store_unavailable")),
    ] {
        let scheduled = now + chrono::Duration::seconds(offset);
        record_scheduler_pass(
            &conn,
            &leadership.fence(),
            &SchedulerPassRecord {
                trigger: "wake",
                scheduled_at: scheduled,
                started_at: scheduled + chrono::Duration::milliseconds(10),
                finished_at: scheduled + chrono::Duration::milliseconds(25),
                duration_ms: 15,
                status,
                error_class,
                planned: Some(0),
                recovered: Some(0),
                claimed: Some(0),
                dispatched: Some(0),
                failures: Some(0),
            },
        )
        .unwrap();
    }
    let report =
        serde_json::to_value(inspect(&conn, now + chrono::Duration::seconds(2)).unwrap()).unwrap();
    let observation = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["category"] == "last_successful_pass")
        .unwrap();
    assert_eq!(observation["facts"]["latest_pass_succeeded"], 0);
    assert_eq!(
        observation["facts"]["last_successful_pass_finished_at_millis"],
        succeeded_at.timestamp_millis(),
        "the latest failed pass must not erase the last successful pass observation"
    );
}

fn record_fixture_pass(
    conn: &Connection,
    fence: &crate::automations::leadership::SchedulerFence,
    at: DateTime<Utc>,
    succeeded: bool,
) -> Result<()> {
    crate::automations::diagnostics::record_scheduler_pass(
        conn,
        fence,
        &crate::automations::diagnostics::SchedulerPassRecord {
            trigger: "wake",
            scheduled_at: at,
            started_at: at,
            finished_at: at + chrono::Duration::milliseconds(25),
            duration_ms: 25,
            status: if succeeded { "succeeded" } else { "failed" },
            error_class: if succeeded {
                None
            } else {
                Some("store_unavailable")
            },
            planned: Some(0),
            recovered: Some(0),
            claimed: Some(0),
            dispatched: Some(0),
            failures: Some(0),
        },
    )
}

fn success_observation(conn: &Connection, at: DateTime<Utc>) -> serde_json::Value {
    let report = serde_json::to_value(inspect(conn, at).unwrap()).unwrap();
    report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["category"] == "last_successful_pass")
        .unwrap()
        .clone()
}

#[test]
fn successful_pass_is_durable_on_a_fresh_connection() {
    let (root, conn) = fixture();
    let now = Utc::now();
    let leadership =
        crate::automations::leadership::SchedulerLeadership::acquire(root.path(), &conn, now)
            .unwrap();
    record_fixture_pass(&conn, &leadership.fence(), now, true).unwrap();
    record_fixture_pass(
        &conn,
        &leadership.fence(),
        now + chrono::Duration::seconds(1),
        false,
    )
    .unwrap();
    let reopened = crate::store::open_store(&root.path().join("coven.sqlite3")).unwrap();
    let observation = success_observation(&reopened, now + chrono::Duration::seconds(2));
    assert_eq!(
        observation["facts"]["last_successful_pass_finished_at_millis"],
        (now + chrono::Duration::milliseconds(25)).timestamp_millis()
    );
    assert_eq!(observation["facts"]["latest_pass_succeeded"], 0);
    assert!(observation["gaps"].as_array().unwrap().is_empty());
}

#[test]
fn initialization_backfills_only_a_known_success_and_is_idempotent() {
    let (root, conn) = fixture();
    let now = Utc::now();
    let leadership =
        crate::automations::leadership::SchedulerLeadership::acquire(root.path(), &conn, now)
            .unwrap();
    record_fixture_pass(&conn, &leadership.fence(), now, true).unwrap();
    conn.execute_batch("DROP TABLE automation_scheduler_last_successful_pass")
        .unwrap();
    let path = root.path().join("coven.sqlite3");
    crate::store::initialize_store(&path).unwrap();
    crate::store::initialize_store(&path).unwrap();
    let reopened = crate::store::open_store(&path).unwrap();
    let observation = success_observation(&reopened, now + chrono::Duration::seconds(1));
    assert_eq!(
        observation["facts"]["last_successful_pass_finished_at_millis"],
        (now + chrono::Duration::milliseconds(25)).timestamp_millis()
    );
    let rows: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM automation_scheduler_last_successful_pass",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn initialization_does_not_invent_success_from_a_failed_latest_pass() {
    let (root, conn) = fixture();
    let now = Utc::now();
    let leadership =
        crate::automations::leadership::SchedulerLeadership::acquire(root.path(), &conn, now)
            .unwrap();
    record_fixture_pass(&conn, &leadership.fence(), now, false).unwrap();
    conn.execute_batch("DROP TABLE automation_scheduler_last_successful_pass")
        .unwrap();
    crate::store::initialize_store(&root.path().join("coven.sqlite3")).unwrap();
    let observation = success_observation(&conn, now + chrono::Duration::seconds(1));
    assert!(observation["facts"]
        .get("last_successful_pass_finished_at_millis")
        .is_none());
    assert_eq!(
        observation["gaps"],
        serde_json::json!(["prior_successful_pass_not_recorded"])
    );
}

#[test]
fn doctor_does_not_migrate_a_missing_success_projection() {
    let (_root, conn) = fixture();
    conn.execute_batch("DROP TABLE automation_scheduler_last_successful_pass")
        .unwrap();
    let before = conn.total_changes();
    inspect(&conn, Utc::now()).unwrap().validate().unwrap();
    assert_eq!(conn.total_changes(), before);
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name = 'automation_scheduler_last_successful_pass'", [], |row| row.get(0)).unwrap();
    assert_eq!(tables, 0);
    assert!(conn.is_autocommit());
}

#[test]
fn stale_scheduler_cannot_overwrite_success_history() {
    let (root, conn) = fixture();
    let now = Utc::now();
    let first =
        crate::automations::leadership::SchedulerLeadership::acquire(root.path(), &conn, now)
            .unwrap();
    let stale = first.fence();
    record_fixture_pass(&conn, &stale, now, true).unwrap();
    drop(first);
    let second = crate::automations::leadership::SchedulerLeadership::acquire(
        root.path(),
        &conn,
        now + chrono::Duration::seconds(1),
    )
    .unwrap();
    let at = now + chrono::Duration::seconds(2);
    record_fixture_pass(&conn, &second.fence(), at, true).unwrap();
    assert!(record_fixture_pass(&conn, &stale, now + chrono::Duration::seconds(3), true).is_err());
    let observation = success_observation(&conn, now + chrono::Duration::seconds(4));
    assert_eq!(
        observation["facts"]["last_successful_pass_finished_at_millis"],
        (at + chrono::Duration::milliseconds(25)).timestamp_millis()
    );
}

#[test]
fn malformed_success_metadata_preserves_independent_observations() {
    let (root, conn) = fixture();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let leadership =
        crate::automations::leadership::SchedulerLeadership::acquire(root.path(), &conn, now)
            .unwrap();
    record_fixture_pass(&conn, &leadership.fence(), now, true).unwrap();
    for invalid in [
        rusqlite::types::Value::Text("PRIVATE-DOCTOR-INVALID-SUCCESS".to_owned()),
        rusqlite::types::Value::Text("é".repeat(33)),
        rusqlite::types::Value::Blob(vec![0xff]),
        rusqlite::types::Value::Text("2026-10-18T00:00:00Z\0PRIVATE-DOCTOR".to_owned()),
    ] {
        conn.execute(
            "UPDATE automation_scheduler_last_successful_pass SET finished_at = ?1 WHERE id = 1",
            [&invalid],
        )
        .unwrap();
        let before = conn.total_changes();
        let report = inspect(&conn, now)
            .expect("one invalid timestamp must not hide independent queue observations");
        report.validate().unwrap();
        let value = serde_json::to_value(report).unwrap();
        let observations = value["observations"].as_array().unwrap();
        let success = observations
            .iter()
            .find(|item| item["category"] == "last_successful_pass")
            .unwrap();
        assert_eq!(
            success["gaps"],
            serde_json::json!(["invalid_persisted_metadata"])
        );
        assert_eq!(success["facts"]["latest_pass_succeeded"], 1);
        assert!(success["facts"]["last_successful_pass_finished_at_millis"].is_null());
        let queue = observations
            .iter()
            .find(|item| item["category"] == "queue")
            .unwrap();
        assert_eq!(queue["facts"]["planned_occurrences"], 0);
        assert!(!value.to_string().contains("PRIVATE-DOCTOR"));
        assert_eq!(conn.total_changes(), before);
        assert!(conn.is_autocommit());
    }
}

#[test]
fn retry_exhaustion_summary_counts_histories_and_preserves_private_details() {
    let (_root, conn) = fixture();
    for (id, streak) in [("doctor-zero", 0), ("doctor-one", 1), ("doctor-four", 4)] {
        let definition =
            super::super::definition::RoutineDefinition::from_json(&serde_json::json!({
                "schemaVersion": 1, "id": id, "name": id, "status": "PAUSED",
                "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
                "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code",
                "cwd": "/work/project", "prompt": "PRIVATE-DOCTOR-RETRY-PROMPT"
            }))
            .unwrap();
        super::super::store::insert_definition(&conn, &definition).unwrap();
        conn.execute(
            "INSERT INTO automation_retry_state
             (automation_id, consecutive_exhaustions, quarantined_at, failure_class, reason, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, '2026-10-18T00:00:00Z')",
            rusqlite::params![id, streak, (streak == 4).then_some("2026-10-18T00:00:00Z"),
                (streak == 4).then_some("PRIVATE-DOCTOR-RETRY-CLASS"),
                (streak == 4).then_some("PRIVATE-DOCTOR-RETRY-REASON")],
        ).unwrap();
    }
    let before = conn.total_changes();
    let report = serde_json::to_value(inspect(&conn, Utc::now()).unwrap()).unwrap();
    let failure = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["category"] == "repeated_failures")
        .unwrap();
    assert_eq!(failure["facts"]["retry_exhaustion_histories"], 2);
    assert_eq!(failure["facts"]["maximum_consecutive_retry_exhaustions"], 4);
    assert_eq!(failure["facts"]["quarantined_routines"], 1);
    assert!(!report.to_string().contains("PRIVATE-DOCTOR"));
    assert_eq!(conn.total_changes(), before);
    assert!(conn.is_autocommit());
    // A valid larger MAX must not conceal a corrupt smaller retained counter.
    conn.execute_batch(
        "PRAGMA ignore_check_constraints = ON;
         UPDATE automation_retry_state SET consecutive_exhaustions = -1
         WHERE automation_id = 'doctor-zero';
         PRAGMA ignore_check_constraints = OFF;",
    )
    .unwrap();
    let before_invalid = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    report.validate().unwrap();
    let mut forged = report.clone();
    forged
        .observations
        .iter_mut()
        .find(|item| item.category == Category::RepeatedFailures)
        .unwrap()
        .facts
        .insert(Fact::MaximumConsecutiveRetryExhaustions, 4);
    assert!(
        forged.validate().is_err(),
        "an invalid counter observation must not carry an invented maximum"
    );
    let report = serde_json::to_value(report).unwrap();
    let failure = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["category"] == "repeated_failures")
        .unwrap();
    assert_eq!(
        failure["gaps"],
        serde_json::json!(["invalid_persisted_metadata"])
    );
    assert_eq!(failure["facts"]["quarantined_routines"], 1);
    assert!(failure["facts"].get("retry_exhaustion_histories").is_none());
    assert!(failure["facts"]
        .get("maximum_consecutive_retry_exhaustions")
        .is_none());
    let queue = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["category"] == "queue")
        .unwrap();
    assert_eq!(queue["facts"]["planned_occurrences"], 0);
    assert!(!report.to_string().contains("PRIVATE-DOCTOR"));
    assert_eq!(conn.total_changes(), before_invalid);
    assert!(conn.is_autocommit());
}

#[test]
fn pass_start_lag_uses_instants_and_isolates_invalid_timestamps() {
    let (root, conn) = fixture();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let observed_at = now + chrono::Duration::seconds(2);
    let leadership =
        crate::automations::leadership::SchedulerLeadership::acquire(root.path(), &conn, now)
            .unwrap();
    crate::automations::diagnostics::record_scheduler_pass(
        &conn,
        &leadership.fence(),
        &crate::automations::diagnostics::SchedulerPassRecord {
            trigger: "wake",
            scheduled_at: now,
            started_at: now + chrono::Duration::milliseconds(1250),
            finished_at: now + chrono::Duration::milliseconds(1275),
            duration_ms: 25,
            status: "succeeded",
            error_class: None,
            planned: Some(0),
            recovered: Some(0),
            claimed: Some(0),
            dispatched: Some(0),
            failures: Some(0),
        },
    )
    .unwrap();
    // Equivalent instant in a different offset, not a sortable UTC string.
    conn.execute("UPDATE automation_scheduler_last_pass SET scheduled_at = '2026-10-18T01:00:00+01:00' WHERE id = 1", []).unwrap();
    let before = conn.total_changes();
    let report = serde_json::to_value(inspect(&conn, observed_at).unwrap()).unwrap();
    let lag = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["category"] == "planning_start_lag")
        .unwrap();
    assert_eq!(lag["facts"]["latest_pass_start_lag_ms"], 1250);
    assert_eq!(lag["facts"]["latest_pass_duration_ms"], 25);
    assert_eq!(lag["gaps"], serde_json::json!(["lag_history_not_measured"]));
    assert_eq!(conn.total_changes(), before);
    for invalid in [
        rusqlite::types::Value::Text("PRIVATE-DOCTOR-START".to_owned()),
        rusqlite::types::Value::Text("é".repeat(33)),
        rusqlite::types::Value::Blob(vec![0xff]),
        rusqlite::types::Value::Text("2026-10-18T00:00:00Z\0PRIVATE-DOCTOR".to_owned()),
        rusqlite::types::Value::Text("2026-10-17T00:00:00Z".to_owned()),
    ] {
        conn.execute(
            "UPDATE automation_scheduler_last_pass SET started_at = ?1 WHERE id = 1",
            [&invalid],
        )
        .unwrap();
        let before = conn.total_changes();
        let report = inspect(&conn, observed_at).unwrap();
        report.validate().unwrap();
        let mut forged = report.clone();
        forged
            .observations
            .iter_mut()
            .find(|item| item.category == Category::PlanningStartLag)
            .unwrap()
            .facts
            .insert(Fact::LatestPassStartLagMs, 1250);
        assert!(
            forged.validate().is_err(),
            "an invalid lag observation must not carry an invented lag"
        );
        let mut missing_duration = report.clone();
        missing_duration
            .observations
            .iter_mut()
            .find(|item| item.category == Category::PlanningStartLag)
            .unwrap()
            .facts
            .clear();
        assert!(
            missing_duration.validate().is_err(),
            "invalid lag scope must retain its independently observed duration"
        );
        let report = serde_json::to_value(report).unwrap();
        let lag = report["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["category"] == "planning_start_lag")
            .unwrap();
        assert_eq!(
            lag["gaps"],
            serde_json::json!(["invalid_persisted_metadata"])
        );
        assert_eq!(lag["facts"]["latest_pass_duration_ms"], 25);
        assert!(lag["facts"].get("latest_pass_start_lag_ms").is_none());
        let queue = report["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["category"] == "queue")
            .unwrap();
        assert_eq!(queue["facts"]["planned_occurrences"], 0);
        assert!(!report.to_string().contains("PRIVATE-DOCTOR"));
        assert_eq!(conn.total_changes(), before);
        assert!(conn.is_autocommit());
    }
}

#[test]
fn due_planned_count_includes_unclaimable_history_but_excludes_future_and_claimed() {
    let (_root, conn) = fixture();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
        .unwrap()
        .with_timezone(&Utc);
    for (id, automation, at, state) in [
        ("older", "same", "2026-10-18T00:00:00.000Z", "planned"),
        ("latest", "same", "2026-10-18T00:00:01.000Z", "planned"),
        ("equal", "equal", "2026-10-18T00:00:02.000Z", "planned"),
        (
            "inactive",
            "inactive",
            "2026-10-18T00:00:00.000Z",
            "planned",
        ),
        ("future", "future", "2026-10-18T00:00:02.001Z", "planned"),
        ("claimed", "claimed", "2026-10-18T00:00:00.000Z", "claimed"),
    ] {
        conn.execute(
            "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, state, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?3, ?3)",
            rusqlite::params![id, automation, at, state],
        )
        .unwrap();
    }
    let before = conn.total_changes();
    let report = inspect(&conn, now).unwrap();
    let queue = report
        .observations
        .iter()
        .find(|o| o.category == Category::Queue)
        .unwrap();
    assert_eq!(
        queue.facts[&Fact::DuePlannedOccurrences],
        4,
        "due is a stored schedule cutoff, not latest-only or active-definition eligibility"
    );
    assert_eq!(
        queue.facts[&Fact::EligibleOccurrences],
        0,
        "unbound definitions are not eligible"
    );
    assert_eq!(queue.facts[&Fact::PlannedOccurrences], 5);
    assert_eq!(queue.facts[&Fact::ClaimedOccurrences], 1);
    assert_eq!(queue.gaps, vec![Gap::RuntimeAdmissionNotProven]);
    assert_eq!(conn.total_changes(), before);
    assert!(conn.is_autocommit());
}

#[test]
fn invalid_eligible_timestamp_preserves_independent_report_categories() {
    let (_root, conn) = fixture();
    let definition =
        crate::automations::definition::RoutineDefinition::from_json(&serde_json::json!({
            "schemaVersion": 1, "id": "doctor-invalid-queue", "name": "doctor-invalid-queue",
            "status": "ACTIVE", "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc",
            "misfire": "latest", "overlap": "forbid", "timeoutMinutes": 30,
            "runtime": "coven-code", "prompt": "PRIVATE-DOCTOR-PROMPT"
        }))
        .unwrap();
    crate::automations::store::insert_definition(&conn, &definition).unwrap();
    conn.execute(
        "INSERT INTO automation_occurrences
        (id, automation_id, scheduled_for, created_at, updated_at)
        VALUES ('doctor-invalid-queue-occurrence', 'doctor-invalid-queue', ?1,
            '2026-10-18T00:00:00.000Z', '2026-10-18T00:00:00.000Z')",
        ["2026-10-18T00:00:01.000Z\0PRIVATE-DOCTOR-SCHEDULE"],
    )
    .unwrap();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
        .unwrap()
        .with_timezone(&Utc);
    let before = conn.total_changes();
    let report =
        inspect(&conn, now).expect("invalid queue metadata must not hide independent categories");
    let queue = report
        .observations
        .iter()
        .find(|o| o.category == Category::Queue)
        .unwrap();
    assert_eq!(queue.gaps, vec![Gap::InvalidPersistedMetadata]);
    assert!(!queue.facts.contains_key(&Fact::EligibleOccurrences));
    assert_eq!(queue.facts[&Fact::PlannedOccurrences], 1);
    assert!(report
        .observations
        .iter()
        .any(|o| o.category == Category::Store && !o.facts.is_empty()));
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    assert_eq!(conn.total_changes(), before);
    assert!(conn.is_autocommit());
    let mut forged = report.clone();
    forged
        .observations
        .iter_mut()
        .find(|o| o.category == Category::Queue)
        .unwrap()
        .facts
        .clear();
    assert!(forged.validate().is_err());
}

#[test]
fn lease_expiry_summary_partitions_exact_deadline_offsets_and_invalid_metadata() {
    let (_root, conn) = fixture();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
        .unwrap()
        .with_timezone(&Utc);
    for (id, state, expiry) in [
        ("active", "claimed", Some("2026-10-18T00:00:03.000Z")),
        ("equal", "running", Some("2026-10-18T00:00:02.000Z")),
        ("past-offset", "claimed", Some("2026-10-18T01:00:01+01:00")),
        (
            "invalid",
            "claimed",
            Some("2026-10-18T00:00:01.000Z\0PRIVATE-EXPIRY"),
        ),
        ("missing", "claimed", None),
        ("outside", "planned", Some("2026-10-18T00:00:02.000Z")),
    ] {
        conn.execute("INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at, created_at, updated_at)
            VALUES (?1, ?1, '2026-10-18T00:00:00.000Z', ?2, 'PRIVATE-OWNER', ?3,
                '2026-10-18T00:00:00.000Z', '2026-10-18T00:00:00.000Z')", rusqlite::params![id, state, expiry]).unwrap();
    }
    let before = conn.total_changes();
    let report = inspect(&conn, now).unwrap();
    let lease = report
        .observations
        .iter()
        .find(|o| o.category == Category::LeasesFences)
        .unwrap();
    for (fact, expected) in [
        (Fact::LeaseBearingOccurrences, 6),
        (Fact::ActiveLeaseOccurrences, 1),
        (Fact::ExpiredLeaseOccurrences, 3),
        (Fact::MissingLeaseExpiryOccurrences, 1),
        (Fact::InvalidLeaseExpiryOccurrences, 1),
        (Fact::LeaseOutsideActiveStateOccurrences, 1),
    ] {
        assert_eq!(lease.facts[&fact], expected);
    }
    for fact in [
        Fact::LeaseBearingOccurrences,
        Fact::LeaseOutsideActiveStateOccurrences,
    ] {
        let mut forged = report.clone();
        forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::LeasesFences)
            .unwrap()
            .facts
            .insert(fact, 7);
        assert!(
            forged.validate().is_err(),
            "inconsistent lease partition was accepted"
        );
    }
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    assert_eq!(conn.total_changes(), before);
    assert!(conn.is_autocommit());
    for suffix in [
        b"\0PRIVATE".to_vec(),
        b"PRIVATE".to_vec(),
        vec![b'x'; 80],
        "é".repeat(33).into_bytes(),
        vec![0xff],
    ] {
        let mut expiry = b"2026-10-18T00:00:01.000Z".to_vec();
        expiry.extend(suffix);
        conn.execute(
            "UPDATE automation_occurrences SET lease_expires_at=CAST(?1 AS TEXT)
            WHERE id='invalid'",
            [expiry],
        )
        .unwrap();
        let before = conn.total_changes();
        let report = inspect(&conn, now).unwrap();
        let lease = report
            .observations
            .iter()
            .find(|o| o.category == Category::LeasesFences)
            .unwrap();
        assert_eq!(lease.facts[&Fact::InvalidLeaseExpiryOccurrences], 1);
        assert_eq!(lease.facts[&Fact::ExpiredLeaseOccurrences], 3);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        assert_eq!(conn.total_changes(), before);
        assert!(conn.is_autocommit());
    }
}

#[test]
fn recorded_fence_counts_mismatches_without_granting_authority_or_hiding_unknowns() {
    let (_root, conn) = fixture();
    conn.execute("UPDATE automation_scheduler_authority SET owner_id='PRIVATE-RECORDED-OWNER', generation=2 WHERE id=1", []).unwrap();
    for (id, state, owner, generation, run) in [
        ("older", "claimed", "daemon", Some(1), None),
        ("null", "claimed", "daemon", None, None),
        ("future", "claimed", "daemon", Some(3), None),
        ("equal", "claimed", "daemon", Some(2), None),
        ("manual", "claimed", "manual", Some(1), None),
        ("running", "running", "daemon", Some(1), None),
        ("running-run", "claimed", "daemon", Some(1), Some("running")),
        ("terminal-run", "claimed", "daemon", Some(1), Some("failed")),
    ] {
        conn.execute("INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, state, lease_owner, scheduler_generation, lease_expires_at, created_at, updated_at)
            VALUES (?1, ?1, '2026-10-18T00:00:00.000Z', ?2, ?3, ?4,
                '2026-10-18T00:00:30.000Z', '2026-10-18T00:00:00.000Z', '2026-10-18T00:00:00.000Z')",
            rusqlite::params![id, state, owner, generation]).unwrap();
        if let Some(status) = run {
            conn.execute(
                "INSERT INTO automation_runs (id, automation_id, occurrence_id, status, started_at)
                VALUES (?1, ?1, ?1, ?2, '2026-10-18T00:00:00.000Z')",
                rusqlite::params![id, status],
            )
            .unwrap();
        }
    }
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
        .unwrap()
        .with_timezone(&Utc);
    let before = conn.total_changes();
    let report = serde_json::to_value(inspect(&conn, now).unwrap()).unwrap();
    let lease = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["category"] == "leases_fences")
        .unwrap();
    assert_eq!(lease["facts"]["recorded_scheduler_fence_present"], 1);
    assert_eq!(lease["facts"]["mismatched_unlaunched_scheduler_claims"], 3);
    assert!(!report.to_string().contains("PRIVATE"));
    assert_eq!(conn.total_changes(), before);
    assert!(conn.is_autocommit());
    conn.execute(
        "UPDATE automation_scheduler_authority SET owner_id=NULL WHERE id=1",
        [],
    )
    .unwrap();
    let report = serde_json::to_value(inspect(&conn, now).unwrap()).unwrap();
    let lease = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["category"] == "leases_fences")
        .unwrap();
    assert_eq!(lease["facts"]["recorded_scheduler_fence_present"], 0);
    assert!(lease["facts"]
        .get("mismatched_unlaunched_scheduler_claims")
        .is_none());
    conn.execute(
        "UPDATE automation_scheduler_authority SET owner_id='' WHERE id=1",
        [],
    )
    .unwrap();
    let report = serde_json::to_value(inspect(&conn, now).unwrap()).unwrap();
    for category in ["leadership", "leases_fences"] {
        let observation = report["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["category"] == category)
            .unwrap();
        assert_eq!(
            observation["gaps"],
            serde_json::json!(["invalid_persisted_metadata"])
        );
    }
    assert!(report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["category"] == "queue" && o["facts"].get("planned_occurrences").is_some()));
}

#[test]
fn recorded_authority_rejects_malformed_owners_and_fabricated_zero() {
    let (_root, conn) = fixture();
    for owner in [
        vec![],
        vec![b'x'; 513],
        b" PRIVATE".to_vec(),
        b"PRIVATE\0".to_vec(),
        vec![255],
        {
            let mut bytes = vec![b'x'; 511];
            bytes.extend([0xc3, 0xa9]);
            bytes
        },
    ] {
        conn.execute("UPDATE automation_scheduler_authority SET owner_id=CAST(?1 AS TEXT), generation=2 WHERE id=1", rusqlite::params![owner]).unwrap();
        let before = conn.total_changes();
        let report = inspect(&conn, Utc::now()).unwrap();
        report.validate().unwrap();
        for category in [Category::Leadership, Category::LeasesFences] {
            let observation = report
                .observations
                .iter()
                .find(|o| o.category == category)
                .unwrap();
            assert_eq!(observation.gaps, vec![Gap::InvalidPersistedMetadata]);
            assert!(!observation
                .facts
                .contains_key(&Fact::RecordedSchedulerFencePresent));
        }
        assert_eq!(conn.total_changes(), before);
        assert!(conn.is_autocommit());
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    }
    conn.execute(
        "UPDATE automation_scheduler_authority SET owner_id=?1, generation=2 WHERE id=1",
        rusqlite::params!["x".repeat(512)],
    )
    .unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let observation = report
        .observations
        .iter()
        .find(|o| o.category == Category::LeasesFences)
        .unwrap();
    assert_eq!(observation.facts[&Fact::RecordedSchedulerFencePresent], 1);
    assert_eq!(
        observation.facts[&Fact::MismatchedUnlaunchedSchedulerClaims],
        0
    );
    conn.execute(
        "UPDATE automation_scheduler_authority SET owner_id=NULL WHERE id=1",
        [],
    )
    .unwrap();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    report.validate().unwrap();
    let observation = report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::LeasesFences)
        .unwrap();
    observation
        .facts
        .insert(Fact::MismatchedUnlaunchedSchedulerClaims, 0);
    assert!(
        report.validate().is_err(),
        "fabricated zero without a recorded fence accepted"
    );
    conn.execute("DELETE FROM automation_scheduler_authority WHERE id=1", [])
        .unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let observation = report
        .observations
        .iter()
        .find(|o| o.category == Category::Leadership)
        .unwrap();
    assert!(observation.facts.is_empty());
    assert_eq!(observation.gaps, vec![Gap::RecordedAuthorityNotObserved]);
}

#[test]
fn recorded_fence_mismatches_cannot_exceed_lease_bearing_total() {
    let (_root, conn) = fixture();
    conn.execute(
        "UPDATE automation_scheduler_authority SET owner_id='test-owner', generation=2 WHERE id=1",
        [],
    )
    .unwrap();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    let observation = report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::LeasesFences)
        .unwrap();
    assert_eq!(observation.facts[&Fact::LeaseBearingOccurrences], 0);
    observation
        .facts
        .insert(Fact::MismatchedUnlaunchedSchedulerClaims, 1);
    assert!(
        report.validate().is_err(),
        "mismatch count exceeds lease-bearing total"
    );
}

#[test]
fn recorded_authority_invalid_generations_preserve_other_observations() {
    let (_root, conn) = fixture();
    for value in [
        rusqlite::types::Value::Text("PRIVATE-GENERATION".into()),
        rusqlite::types::Value::Real(1.5),
        rusqlite::types::Value::Integer(9_007_199_254_740_992),
        rusqlite::types::Value::Blob(vec![255]),
        rusqlite::types::Value::Integer(0),
    ] {
        conn.execute("UPDATE automation_scheduler_authority SET owner_id='PRIVATE-OWNER', generation=?1 WHERE id=1", rusqlite::params![value]).unwrap();
        let before = conn.total_changes();
        let report = inspect(&conn, Utc::now()).unwrap();
        report.validate().unwrap();
        for category in [Category::Leadership, Category::LeasesFences] {
            let observation = report
                .observations
                .iter()
                .find(|o| o.category == category)
                .unwrap();
            assert_eq!(observation.gaps, vec![Gap::InvalidPersistedMetadata]);
        }
        let queue = report
            .observations
            .iter()
            .find(|o| o.category == Category::Queue)
            .unwrap();
        assert!(queue.facts.contains_key(&Fact::PlannedOccurrences));
        assert_eq!(conn.total_changes(), before);
        assert!(conn.is_autocommit());
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    }
}

#[test]
fn recorded_contract_migration_is_historical_bounded_and_read_only() {
    let (_root, conn) = fixture();
    let before = conn.total_changes();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    report.validate().unwrap();
    let store = report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::Store)
        .unwrap();
    assert_eq!(store.facts[&Fact::RecordedContractMigrationPresent], 1);
    assert_eq!(store.facts[&Fact::ContractMigrationDefinitionsMigrated], 0);
    assert_eq!(store.gaps, vec![Gap::IntegrityAndMigrationsNotVerified]);
    assert_eq!(conn.total_changes(), before);
    assert!(conn.is_autocommit());
    store.facts.remove(&Fact::ContractMigrationRecordedAtMillis);
    assert!(
        report.validate().is_err(),
        "partial observed migration accepted"
    );
    for column in [
        "definitions_migrated",
        "occurrences_migrated",
        "runs_migrated",
        "unverifiable_definitions",
        "unverifiable_occurrences",
        "unverifiable_runs",
    ] {
        for value in [
            rusqlite::types::Value::Text("PRIVATE-COUNTER".into()),
            rusqlite::types::Value::Real(1.5),
            rusqlite::types::Value::Integer(-1),
            rusqlite::types::Value::Integer(9_007_199_254_740_992),
        ] {
            conn.execute(
                &format!("UPDATE automation_contract_migrations SET {column}=?1"),
                rusqlite::params![value],
            )
            .unwrap();
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now()).unwrap();
            report.validate().unwrap();
            let store = report
                .observations
                .iter()
                .find(|o| o.category == Category::Store)
                .unwrap();
            assert_eq!(store.gaps, vec![Gap::InvalidPersistedMetadata]);
            assert_eq!(store.facts.len(), 6);
            assert!(report
                .observations
                .iter()
                .find(|o| o.category == Category::Queue)
                .unwrap()
                .facts
                .contains_key(&Fact::PlannedOccurrences));
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            assert_eq!(conn.total_changes(), before);
            assert!(conn.is_autocommit());
        }
        conn.execute(
            &format!("UPDATE automation_contract_migrations SET {column}=0"),
            [],
        )
        .unwrap();
    }
    for value in [
        b"PRIVATE-TIMESTAMP".to_vec(),
        b"2026-10-18T00:00:00Z\0PRIVATE".to_vec(),
        vec![b'x'; 65],
        vec![255],
    ] {
        conn.execute(
            "UPDATE automation_contract_migrations SET migrated_at=CAST(?1 AS TEXT)",
            rusqlite::params![value],
        )
        .unwrap();
        let report = inspect(&conn, Utc::now()).unwrap();
        let store = report
            .observations
            .iter()
            .find(|o| o.category == Category::Store)
            .unwrap();
        assert_eq!(store.gaps, vec![Gap::InvalidPersistedMetadata]);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    }
    conn.execute("DELETE FROM automation_contract_migrations", [])
        .unwrap();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    let store = report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::Store)
        .unwrap();
    assert_eq!(store.facts[&Fact::RecordedContractMigrationPresent], 0);
    assert_eq!(store.facts.len(), 7);
    store
        .facts
        .insert(Fact::ContractMigrationDefinitionsMigrated, 0);
    assert!(
        report.validate().is_err(),
        "fabricated migration count accepted"
    );
}

#[test]
fn invalid_latest_pass_fields_do_not_hide_independent_categories() {
    let (_root, conn) = fixture();
    conn.execute("INSERT INTO automation_scheduler_last_pass
        (id,scheduler_generation,trigger,scheduled_at,started_at,finished_at,duration_ms,status,planned,recovered,claimed,dispatched,failures)
        VALUES (1,1,'wake','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z','2026-10-18T00:00:02Z',1000,'succeeded',0,0,0,0,0)", []).unwrap();
    for (column, value, category, gap) in [
        (
            "duration_ms",
            rusqlite::types::Value::Text("PRIVATE-DURATION".into()),
            Category::PlanningStartLag,
            Gap::InvalidLatestPassDuration,
        ),
        (
            "duration_ms",
            rusqlite::types::Value::Real(1.5),
            Category::PlanningStartLag,
            Gap::InvalidLatestPassDuration,
        ),
        (
            "duration_ms",
            rusqlite::types::Value::Integer(-1),
            Category::PlanningStartLag,
            Gap::InvalidLatestPassDuration,
        ),
        (
            "duration_ms",
            rusqlite::types::Value::Integer(9_007_199_254_740_992),
            Category::PlanningStartLag,
            Gap::InvalidLatestPassDuration,
        ),
        (
            "status",
            rusqlite::types::Value::Text("PRIVATE-STATUS".into()),
            Category::LastSuccessfulPass,
            Gap::InvalidLatestPassStatus,
        ),
        (
            "status",
            rusqlite::types::Value::Blob(vec![255]),
            Category::LastSuccessfulPass,
            Gap::InvalidLatestPassStatus,
        ),
    ] {
        // Deliberately model corrupt persisted metadata, then restore normal
        // SQLite constraint checking before exercising the read-only observer.
        conn.execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        conn.execute(
            &format!("UPDATE automation_scheduler_last_pass SET {column}=?1"),
            rusqlite::params![value],
        )
        .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints=OFF")
            .unwrap();
        let before = conn.total_changes();
        let mut report = inspect(&conn, Utc::now()).unwrap();
        report.validate().unwrap();
        let observation = report
            .observations
            .iter_mut()
            .find(|o| o.category == category)
            .unwrap();
        assert_eq!(observation.gaps, vec![gap]);
        if category == Category::PlanningStartLag {
            assert!(!observation.facts.contains_key(&Fact::LatestPassDurationMs));
            assert!(!observation.facts.contains_key(&Fact::LatestPassStartLagMs));
            assert_eq!(
                observation.facts[&Fact::ScheduledOccurrencesPlanningChecked],
                0
            );
        } else {
            assert!(observation.facts.is_empty());
        }
        observation.facts.insert(
            if category == Category::LastSuccessfulPass {
                Fact::LatestPassSucceeded
            } else {
                Fact::LatestPassDurationMs
            },
            0,
        );
        assert!(
            report.validate().is_err(),
            "invalid pass metadata admitted a fabricated zero"
        );
        let report = inspect(&conn, Utc::now()).unwrap();
        assert!(report
            .observations
            .iter()
            .find(|o| o.category == Category::Queue)
            .unwrap()
            .facts
            .contains_key(&Fact::PlannedOccurrences));
        let unaffected = if category == Category::LastSuccessfulPass {
            Category::PlanningStartLag
        } else {
            Category::LastSuccessfulPass
        };
        let observation = report
            .observations
            .iter()
            .find(|o| o.category == unaffected)
            .unwrap();
        assert!(!observation.facts.is_empty());
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        assert_eq!(conn.total_changes(), before);
        assert!(conn.is_autocommit());
        conn.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE automation_scheduler_last_pass SET status='succeeded',duration_ms=1000; PRAGMA ignore_check_constraints=OFF").unwrap();
    }
}

#[test]
fn schema_presence_reports_missing_index_without_claiming_semantic_integrity() {
    let (_root, conn) = fixture();
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let store = report
        .observations
        .iter()
        .find(|o| o.category == Category::Store)
        .unwrap();
    assert_eq!(store.facts[&Fact::MissingRequiredSchemaObjects], 0);
    assert_eq!(store.facts[&Fact::RequiredSchemaObjectPresenceChecks], 73);
    assert_eq!(store.gaps, vec![Gap::IntegrityAndMigrationsNotVerified]);
    assert_eq!(conn.total_changes(), before);
    conn.execute("DROP INDEX idx_automation_definitions_updated_at", [])
        .unwrap();
    let before = conn.total_changes();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    let store = report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::Store)
        .unwrap();
    assert_eq!(store.facts[&Fact::MissingRequiredSchemaObjects], 1);
    assert_eq!(store.gaps, vec![Gap::IntegrityAndMigrationsNotVerified]);
    store.facts.insert(Fact::MissingRequiredSchemaObjects, 74);
    assert!(
        report.validate().is_err(),
        "missing count exceeds fixed presence checks"
    );
    assert_eq!(conn.total_changes(), before);
    assert!(conn.is_autocommit());
    // The name is restored with a deliberately different definition. Presence
    // alone must keep the semantic/integrity gap; it does not certify this index.
    conn.execute(
        "CREATE INDEX idx_automation_definitions_updated_at ON automation_definitions(status)",
        [],
    )
    .unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let store = report
        .observations
        .iter()
        .find(|o| o.category == Category::Store)
        .unwrap();
    assert_eq!(store.facts[&Fact::MissingRequiredSchemaObjects], 0);
    assert_eq!(store.gaps, vec![Gap::IntegrityAndMigrationsNotVerified]);
}

#[test]
fn schema_presence_contract_covers_supported_release_store_upgrades() {
    for dump in [
        include_str!("../../tests/fixtures/automations-release-stores/v0.4.3.sql"),
        include_str!("../../tests/fixtures/automations-release-stores/v0.4.6.sql"),
        include_str!("../../tests/fixtures/automations-release-stores/v0.4.6-rollback.sql"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("coven.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(dump).unwrap();
        drop(conn);
        // Initialization is fixture preparation, outside the observer. No
        // daemon or dispatcher runs any of the historical definitions.
        crate::store::initialize_store(&path).unwrap();
        let conn =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(missing_required_schema_objects(&conn).unwrap(), 0);
        assert_eq!(conn.total_changes(), 0);
        assert!(conn.is_autocommit());
    }
}

fn capture_schema_metadata(conn: &Connection) -> serde_json::Value {
    fn rows(conn: &Connection, sql: &str, name: &str) -> serde_json::Value {
        let mut query = conn.prepare(sql).unwrap();
        let width = query.column_count();
        let values = query
            .query_map([name], |row| {
                (0..width)
                    .map(|column| {
                        Ok(match row.get_ref(column)? {
                            rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                            rusqlite::types::ValueRef::Integer(value) => serde_json::json!(value),
                            rusqlite::types::ValueRef::Real(value) => serde_json::json!(value),
                            rusqlite::types::ValueRef::Text(value) => {
                                serde_json::json!(std::str::from_utf8(value).unwrap())
                            }
                            rusqlite::types::ValueRef::Blob(value) => serde_json::json!(value),
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        serde_json::json!(values)
    }
    let mut objects = BTreeMap::new();
    for (kind, name) in REQUIRED_SCHEMA_OBJECTS {
        let mut object = serde_json::Map::new();
        object.insert(
            "definition".into(),
            rows(
                conn,
                "SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE name=?1 ORDER BY type",
                name,
            ),
        );
        if *kind == "table" {
            object.insert(
                "columns".into(),
                rows(conn, "SELECT cid,name,type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo(?1) ORDER BY cid", name),
            );
            object.insert(
                "foreignKeys".into(),
                rows(conn, "SELECT id,seq,\"table\",\"from\",\"to\",on_update,on_delete,\"match\" FROM pragma_foreign_key_list(?1) ORDER BY id,seq", name),
            );
            object.insert(
                "properties".into(),
                rows(conn, "SELECT schema,name,type,ncol,wr,strict FROM pragma_table_list(?1) WHERE schema='main' ORDER BY schema,name", name),
            );
            object.insert(
                "indexes".into(),
                rows(
                    conn,
                    "SELECT seq,name,\"unique\",origin,partial FROM pragma_index_list(?1) ORDER BY name",
                    name,
                ),
            );
            let mut index_details = BTreeMap::new();
            let mut indexes = conn
                .prepare("SELECT name FROM pragma_index_list(?1) ORDER BY name")
                .unwrap();
            for index in indexes
                .query_map([name], |row| row.get::<_, String>(0))
                .unwrap()
            {
                let index = index.unwrap();
                // All names here come from trusted fixture preparation, never
                // from the production observer or an untrusted remote report.
                index_details.insert(
                    index.clone(),
                    rows(conn, "SELECT seqno,cid,name,\"desc\",coll,key FROM pragma_index_xinfo(?1) ORDER BY seqno", &index),
                );
            }
            object.insert("indexDetails".into(), serde_json::json!(index_details));
        }
        if *kind == "index" {
            object.insert(
                "columns".into(),
                rows(conn, "SELECT seqno,cid,name,\"desc\",coll,key FROM pragma_index_xinfo(?1) ORDER BY seqno", name),
            );
        }
        objects.insert(format!("{kind}:{name}"), serde_json::Value::Object(object));
    }
    serde_json::json!(objects)
}

#[test]
fn capture_supported_schema_forms_from_production_initialization() {
    let mut forms = BTreeMap::new();
    let (_root, conn) = fixture();
    forms.insert("fresh", capture_schema_metadata(&conn));
    for (name, dump) in [
        (
            "v0.4.3",
            include_str!("../../tests/fixtures/automations-release-stores/v0.4.3.sql"),
        ),
        (
            "v0.4.6",
            include_str!("../../tests/fixtures/automations-release-stores/v0.4.6.sql"),
        ),
        (
            "v0.4.6-rollback",
            include_str!("../../tests/fixtures/automations-release-stores/v0.4.6-rollback.sql"),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("coven.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(dump).unwrap();
        drop(conn);
        crate::store::initialize_store(&path).unwrap();
        let conn =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(missing_required_schema_objects(&conn).unwrap(), 0);
        let before = conn.total_changes();
        let observation = schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap();
        assert_eq!(
            (
                observation.missing,
                observation.unrecognized,
                observation.incomplete
            ),
            (0, 0, 0),
            "historical store: {name}"
        );
        forms.insert(name, capture_schema_metadata(&conn));
        assert_eq!(conn.total_changes(), before);
    }
    let review: serde_json::Value =
        serde_json::from_str(include_str!("doctor-schema-form-review.json")).unwrap();
    let mut observed_digests = BTreeSet::new();
    for (fixture_name, captured) in &forms {
        let observation = if *fixture_name == "fresh" {
            Some(schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap())
        } else {
            None
        };
        if let Some(observation) = observation {
            assert_eq!(
                (
                    observation.missing,
                    observation.unrecognized,
                    observation.incomplete
                ),
                (0, 0, 0)
            );
        }
        let approved = review["fixtureObjects"][fixture_name].as_object().unwrap();
        assert_eq!(approved.len(), REQUIRED_SCHEMA_OBJECTS.len());
        for (key, descriptor) in captured.as_object().unwrap() {
            let digest = super::super::contract::canonical_json::sha256_digest(descriptor).unwrap();
            assert_eq!(
                approved[key]["sha256"], digest,
                "fixture form drift: {fixture_name}/{key}"
            );
            assert_eq!(
                review["descriptors"][&digest], *descriptor,
                "readable descriptor mismatch"
            );
            observed_digests.insert(digest);
        }
    }
    assert_eq!(
        review["descriptors"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        observed_digests
    );
    // This optional output is an observation for reviewing supported SQL forms,
    // not automatically the production compatibility contract. No artifact is
    // written by default, and no daemon/dispatcher executes fixture definitions.
    if let Some(path) = std::env::var_os("COVEN_DOCTOR_SCHEMA_CAPTURE_PATH") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "kind":"production_initialized_schema_forms_for_review",
                "presenceChecks":REQUIRED_SCHEMA_OBJECTS.len(), "forms":forms
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn schema_form_capture_preserves_index_definitions_collations_and_order() {
    let (_root, conn) = fixture();
    let before = capture_schema_metadata(&conn);
    let mission = &before["table:automation_missions"];
    assert!(
        mission["indexDetails"]
            .as_object()
            .unwrap()
            .keys()
            .any(|name| name.starts_with("sqlite_autoindex_automation_missions_")),
        "automatic uniqueness/key metadata must not be discarded"
    );
    conn.execute("DROP INDEX idx_automation_definitions_updated_at", [])
        .unwrap();
    conn.execute("CREATE INDEX idx_automation_definitions_updated_at ON automation_definitions(status COLLATE NOCASE DESC, lower(name))", []).unwrap();
    let total_changes = conn.total_changes();
    let after = capture_schema_metadata(&conn);
    assert_ne!(
        before["index:idx_automation_definitions_updated_at"],
        after["index:idx_automation_definitions_updated_at"]
    );
    let columns = after["index:idx_automation_definitions_updated_at"]["columns"]
        .as_array()
        .unwrap();
    assert!(
        columns
            .iter()
            .any(|column| column[2] == "status" && column[3] == 1 && column[4] == "NOCASE"),
        "index expression/ordering/collation metadata was lost"
    );
    assert!(
        columns.iter().any(|column| column[1] == -2),
        "expression-index term marker was lost"
    );
    assert!(
        after["index:idx_automation_definitions_updated_at"]["definition"][0][3]
            .as_str()
            .unwrap()
            .contains("lower(name)"),
        "expression SQL was lost"
    );
    assert_eq!(conn.total_changes(), total_changes);
    assert!(conn.is_autocommit());
}

#[test]
fn bounded_schema_forms_partition_missing_changed_and_oversized_objects() {
    let (_root, conn) = fixture();
    let initial = schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap();
    assert_eq!(
        (initial.missing, initial.unrecognized, initial.incomplete),
        (0, 0, 0)
    );
    let before = conn.total_changes();
    conn.execute_batch(
        "DROP INDEX idx_automation_definitions_updated_at;
        CREATE INDEX idx_automation_definitions_updated_at ON automation_definitions(status);",
    )
    .unwrap();
    let changed = schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap();
    assert!(changed.unrecognized > 0);
    assert_eq!((changed.missing, changed.incomplete), (0, 0));
    conn.execute_batch("DROP INDEX idx_automation_definitions_updated_at;")
        .unwrap();
    let missing = schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap();
    assert_eq!(missing.missing, 1);
    assert_eq!(missing.incomplete, 0);
    conn.execute_batch(&format!("CREATE INDEX idx_automation_definitions_updated_at ON automation_definitions(status) /*{}*/;", "x".repeat(32_769))).unwrap();
    let oversized = schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap();
    assert_eq!(oversized.missing, 0);
    assert!(oversized.incomplete > 0);
    assert!(
        oversized.missing + oversized.unrecognized + oversized.incomplete
            <= REQUIRED_SCHEMA_OBJECTS.len() as u64
    );
    assert_eq!(
        conn.total_changes(),
        before,
        "metadata inspection and DDL do not change application rows"
    );
}

#[test]
fn schema_form_findings_reach_report_without_exposing_definitions() {
    let (_root, conn) = fixture();
    conn.execute_batch("DROP INDEX idx_automation_definitions_updated_at;
        CREATE INDEX idx_automation_definitions_updated_at ON automation_definitions(status) /*PRIVATE_SCHEMA_TEXT*/;").unwrap();
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let store = report
        .observations
        .iter()
        .find(|o| o.category == Category::Store)
        .unwrap();
    let facts = serde_json::to_value(&store.facts).unwrap();
    assert!(
        facts["unrecognized_required_schema_object_forms"]
            .as_u64()
            .is_some_and(|n| n > 0),
        "same-name changed definition must be diagnosed"
    );
    assert_eq!(facts["incomplete_required_schema_object_checks"], 0);
    assert_eq!(store.gaps, vec![Gap::IntegrityAndMigrationsNotVerified]);
    assert!(!serde_json::to_string(&report)
        .unwrap()
        .contains("PRIVATE_SCHEMA_TEXT"));
    assert_eq!(conn.total_changes(), before);
    report.validate().unwrap();
}

#[test]
fn schema_forms_ignore_temp_objects_shadowing_main_names() {
    let (_root, conn) = fixture();
    let original = schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap();
    assert_eq!(
        (original.missing, original.unrecognized, original.incomplete),
        (0, 0, 0)
    );
    conn.execute_batch("CREATE TEMP TABLE automation_definitions(private_shadow TEXT);
        CREATE INDEX temp.idx_automation_definitions_updated_at ON automation_definitions(private_shadow);").unwrap();
    let before = conn.total_changes();
    let observed = schema::inspect(&conn, REQUIRED_SCHEMA_OBJECTS).unwrap();
    assert_eq!(
        (observed.missing, observed.unrecognized, observed.incomplete),
        (0, 0, 0),
        "main schema checks must not read same-named temporary objects"
    );
    assert_eq!(conn.total_changes(), before);
}

#[test]
fn schema_additions_are_counted_without_querying_private_names() {
    let (_root, conn) = fixture();
    conn.execute_batch("CREATE TABLE automation_PRIVATE_extra(value TEXT);
        CREATE VIEW automation_PRIVATE_view AS SELECT 1;
        CREATE INDEX PRIVATE_extra_index ON automation_definitions(status);
        CREATE TRIGGER PRIVATE_extra_trigger AFTER INSERT ON automation_definitions BEGIN SELECT 1; END;").unwrap();
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let store = report
        .observations
        .iter()
        .find(|o| o.category == Category::Store)
        .unwrap();
    let facts = serde_json::to_value(&store.facts).unwrap();
    assert_eq!(
        facts["unexpected_automation_schema_objects"], 4,
        "unknown tables/views and objects attached to owned tables need separate evidence"
    );
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    assert_eq!(conn.total_changes(), before);
}

#[test]
fn schema_report_rejects_overlapping_failure_counts() {
    let (_root, conn) = fixture();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    let store = report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::Store)
        .unwrap();
    store.facts.insert(Fact::MissingRequiredSchemaObjects, 30);
    store
        .facts
        .insert(Fact::UnrecognizedRequiredSchemaObjectForms, 30);
    store
        .facts
        .insert(Fact::IncompleteRequiredSchemaObjectChecks, 30);
    assert!(
        report.validate().is_err(),
        "each count fits but their partition exceeds 73"
    );
}

#[test]
fn missing_success_history_table_keeps_independent_doctor_observations() {
    let (_root, conn) = fixture();
    conn.execute_batch("DROP TABLE automation_scheduler_last_successful_pass")
        .unwrap();
    let before = conn.total_changes();
    let report =
        inspect(&conn, Utc::now()).expect("missing history must not erase the schema finding");
    let history = report
        .observations
        .iter()
        .find(|o| o.category == Category::LastSuccessfulPass)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&history.gaps).unwrap(),
        serde_json::json!(["successful_pass_store_unavailable"])
    );
    assert!(!history
        .facts
        .contains_key(&Fact::LastSuccessfulPassFinishedAtMillis));
    let store = report
        .observations
        .iter()
        .find(|o| o.category == Category::Store)
        .unwrap();
    assert!(store.facts[&Fact::MissingRequiredSchemaObjects] > 0);
    let queue = report
        .observations
        .iter()
        .find(|o| o.category == Category::Queue)
        .unwrap();
    assert_eq!(queue.facts[&Fact::PlannedOccurrences], 0);
    assert_eq!(conn.total_changes(), before);
    report.validate().unwrap();
}

#[test]
fn missing_migration_ledger_is_unavailable_not_absent_or_zero() {
    let (_root, conn) = fixture();
    conn.execute_batch("DROP TABLE automation_contract_migrations")
        .unwrap();
    let before = conn.total_changes();
    let report =
        inspect(&conn, Utc::now()).expect("missing ledger must preserve independent findings");
    let store = report
        .observations
        .iter()
        .find(|o| o.category == Category::Store)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&store.gaps).unwrap(),
        serde_json::json!(["contract_migration_store_unavailable"])
    );
    assert!(!store
        .facts
        .contains_key(&Fact::RecordedContractMigrationPresent));
    assert!(!store
        .facts
        .contains_key(&Fact::ContractMigrationDefinitionsMigrated));
    assert!(store.facts[&Fact::MissingRequiredSchemaObjects] > 0);
    assert!(report
        .observations
        .iter()
        .find(|o| o.category == Category::Queue)
        .unwrap()
        .facts
        .contains_key(&Fact::EligibleOccurrences));
    assert_eq!(conn.total_changes(), before);
    let recreated: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='automation_contract_migrations')", [], |r| r.get(0)).unwrap();
    assert!(!recreated);
    report.validate().unwrap();
}

#[test]
fn missing_projection_columns_preserve_independent_observations() {
    for (table, replacement, category, gap) in [
        (
            "automation_scheduler_last_successful_pass",
            "id INTEGER PRIMARY KEY",
            Category::LastSuccessfulPass,
            "successful_pass_store_unavailable",
        ),
        (
            "automation_contract_migrations",
            "profile TEXT PRIMARY KEY",
            Category::Store,
            "contract_migration_store_unavailable",
        ),
    ] {
        let (_root, conn) = fixture();
        conn.execute_batch(&format!(
            "DROP TABLE {table}; CREATE TABLE {table}({replacement});"
        ))
        .unwrap();
        let before = conn.total_changes();
        let report = inspect(&conn, Utc::now())
            .expect("missing columns must not erase independent findings");
        let unavailable = report
            .observations
            .iter()
            .find(|o| o.category == category)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&unavailable.gaps).unwrap(),
            serde_json::json!([gap])
        );
        let store = report
            .observations
            .iter()
            .find(|o| o.category == Category::Store)
            .unwrap();
        assert_eq!(store.facts[&Fact::MissingRequiredSchemaObjects], 0);
        assert!(store.facts[&Fact::UnrecognizedRequiredSchemaObjectForms] > 0);
        assert_eq!(conn.total_changes(), before);
        report.validate().unwrap();
    }
}

#[test]
fn table_only_prerequisites_follow_main_tables_without_reading_rows() {
    let (_root, conn) = fixture();
    conn.execute_batch("CREATE TEMP TABLE automation_receipts(PRIVATE_shadow TEXT);")
        .unwrap();
    let before = conn.total_changes();
    assert!(
        projection_available(&conn, "automation_receipts", &[]).unwrap(),
        "COUNT(*) requires table existence, not an invented projected column"
    );
    assert!(
        !projection_available(&conn, "automation_receipts", &["PRIVATE_shadow"]).unwrap(),
        "temporary columns cannot satisfy main-table prerequisites"
    );
    conn.execute_batch("DROP TABLE main.automation_receipts;")
        .unwrap();
    assert!(
        !projection_available(&conn, "automation_receipts", &[]).unwrap(),
        "a temporary shadow cannot replace a missing main table"
    );
    conn.execute_batch("CREATE VIEW main.automation_receipts AS SELECT 1 AS PRIVATE_value;")
        .unwrap();
    assert!(
        !projection_available(&conn, "automation_receipts", &[]).unwrap(),
        "an unexpected view must not become an execution prerequisite"
    );
    assert_eq!(conn.total_changes(), before);
}

#[test]
fn event_counts_preserve_each_available_main_relation() {
    for missing in 0..4 {
        let (_root, conn) = fixture();
        for (bit, table) in [
            (1, "automation_events"),
            (2, "automation_event_stream_heads"),
        ] {
            if missing & bit != 0 {
                conn.execute_batch(&format!("DROP TABLE main.{table};"))
                    .unwrap();
            }
            conn.execute_batch(&format!("CREATE TEMP TABLE {table}(PRIVATE_text TEXT); INSERT INTO temp.{table} VALUES('PRIVATE_EVENT_SHADOW');")).unwrap();
        }
        let before = conn.total_changes();
        let report = inspect(&conn, Utc::now())
            .expect("missing event relation must preserve independent facts");
        let event = report
            .observations
            .iter()
            .find(|o| o.category == Category::EventChangefeed)
            .unwrap();
        for (bit, fact) in [(1, Fact::StoredEvents), (2, Fact::EventStreams)] {
            if missing & bit == 0 {
                assert_eq!(
                    event.facts.get(&fact),
                    Some(&0),
                    "must count main, not a TEMP shadow"
                );
            } else {
                assert!(
                    !event.facts.contains_key(&fact),
                    "unavailable is not measured zero"
                );
            }
        }
        let mut expected = vec![if missing == 0 {
            "subscriber_replay_not_observed"
        } else {
            "event_store_unavailable"
        }];
        if missing & 2 != 0 {
            expected.push("checkpoint_retention_store_unavailable");
        }
        if missing != 0 {
            expected.push("event_sequence_store_unavailable");
        }
        assert_eq!(
            serde_json::to_value(&event.gaps).unwrap(),
            serde_json::json!(expected)
        );
        assert_eq!(conn.total_changes(), before);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        assert!(report
            .observations
            .iter()
            .find(|o| o.category == Category::Queue)
            .unwrap()
            .facts
            .contains_key(&Fact::EligibleOccurrences));
        report.validate().unwrap();
        let mut forged = report;
        let event = forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::EventChangefeed)
            .unwrap();
        event.gaps = vec![Gap::EventStoreUnavailable];
        event.facts.insert(Fact::EventStreams, 0);
        event.facts.insert(Fact::StoredEvents, 0);
        assert!(
            forged.validate().is_err(),
            "unavailable gap cannot accompany both measured facts"
        );
    }
}

#[test]
fn unavailable_receipts_do_not_hide_delivery_counts_or_read_temp_shadows() {
    for missing in [true, false] {
        let (_root, conn) = fixture();
        for (id, state) in [
            ("delivery-failed-a", "failed"),
            ("delivery-failed-b", "failed"),
            ("delivery-pending", "pending"),
        ] {
            conn.execute("INSERT INTO main.automation_runs (id,automation_id,status,started_at,delivery_state)
                VALUES (?1,?1,'failed','2026-10-18T00:00:00.000Z',?2)", rusqlite::params![id,state]).unwrap();
        }

        if missing {
            conn.execute_batch("DROP TABLE main.automation_receipts")
                .unwrap();
        }
        conn.execute_batch("CREATE TEMP TABLE automation_receipts(PRIVATE_value TEXT); INSERT INTO temp.automation_receipts VALUES('PRIVATE_RECEIPT_SHADOW');").unwrap();
        let before = conn.total_changes();
        let report =
            inspect(&conn, Utc::now()).expect("missing receipts must retain delivery observations");
        let delivery = report
            .observations
            .iter()
            .find(|o| o.category == Category::DeliveryReceipts)
            .unwrap();
        assert_eq!(delivery.facts[&Fact::FailedDeliveries], 2);
        assert_eq!(delivery.facts[&Fact::PendingDeliveries], 1);
        if missing {
            assert!(
                !delivery.facts.contains_key(&Fact::StoredReceipts),
                "unavailable is not a measured count"
            );
            assert_eq!(
                serde_json::to_value(&delivery.gaps).unwrap(),
                serde_json::json!([
                    "receipt_store_unavailable",
                    "receipt_link_store_unavailable"
                ])
            );
        } else {
            assert_eq!(
                delivery.facts[&Fact::StoredReceipts],
                0,
                "must count main, not a TEMP shadow"
            );
            assert_eq!(delivery.gaps, vec![Gap::ReceiptAuthorityNotVerified]);
        }
        assert_eq!(conn.total_changes(), before);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        report.validate().unwrap();
        let mut forged = report;
        let delivery = forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DeliveryReceipts)
            .unwrap();
        delivery.gaps = vec![Gap::ReceiptStoreUnavailable];
        delivery.facts.insert(Fact::StoredReceipts, 0);
        assert!(
            forged.validate().is_err(),
            "unavailable receipt store cannot have a measured receipt count"
        );
    }
}

#[test]
fn missing_pass_duration_preserves_status_and_independent_lag() {
    for shadow in [false, true] {
        for invalid_lag in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_scheduler_last_pass;
                CREATE TABLE main.automation_scheduler_last_pass(id INTEGER PRIMARY KEY,status TEXT,scheduled_at TEXT,started_at TEXT);
                INSERT INTO main.automation_scheduler_last_pass VALUES(1,'succeeded','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.010Z');").unwrap();
            if invalid_lag {
                conn.execute("UPDATE main.automation_scheduler_last_pass SET started_at='PRIVATE_INVALID_LAG'", []).unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_scheduler_last_pass AS SELECT * FROM main.automation_scheduler_last_pass;
                ALTER TABLE temp.automation_scheduler_last_pass ADD COLUMN duration_ms INTEGER;
                UPDATE temp.automation_scheduler_last_pass SET status='failed',duration_ms=999,started_at='2026-10-18T00:00:01.000Z';").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now())
                .expect("missing duration column must not erase status or lag");
            let status = report
                .observations
                .iter()
                .find(|o| o.category == Category::LastSuccessfulPass)
                .unwrap();
            assert_eq!(status.facts[&Fact::LatestPassSucceeded], 1);
            let planning = report
                .observations
                .iter()
                .find(|o| o.category == Category::PlanningStartLag)
                .unwrap();
            assert!(!planning.facts.contains_key(&Fact::LatestPassDurationMs));
            if invalid_lag {
                assert!(!planning.facts.contains_key(&Fact::LatestPassDurationMs));
                assert!(!planning.facts.contains_key(&Fact::LatestPassStartLagMs));
                assert_eq!(
                    serde_json::to_value(&planning.gaps).unwrap(),
                    serde_json::json!([
                        "latest_pass_duration_unavailable",
                        "invalid_persisted_metadata"
                    ])
                );
            } else {
                assert_eq!(planning.facts[&Fact::LatestPassStartLagMs], 10);
                assert_eq!(
                    serde_json::to_value(&planning.gaps).unwrap(),
                    serde_json::json!(["latest_pass_duration_unavailable"])
                );
            }
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
        }
    }
}

#[test]
fn missing_lag_columns_preserve_duration_and_all_concurrent_gaps() {
    for missing in ["scheduled_at", "started_at", "both"] {
        for duration in ["valid", "invalid", "missing"] {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                let mut columns = vec!["id INTEGER PRIMARY KEY", "status TEXT"];
                if duration != "missing" {
                    columns.push("duration_ms");
                }
                if missing != "scheduled_at" && missing != "both" {
                    columns.push("scheduled_at TEXT");
                }
                if missing != "started_at" && missing != "both" {
                    columns.push("started_at TEXT");
                }
                conn.execute_batch(&format!("DROP TABLE main.automation_scheduler_last_pass;
                    CREATE TABLE main.automation_scheduler_last_pass({});
                    INSERT INTO main.automation_scheduler_last_pass(id,status) VALUES(1,'succeeded');", columns.join(","))).unwrap();
                if duration == "valid" {
                    conn.execute_batch(
                        "UPDATE main.automation_scheduler_last_pass SET duration_ms=15",
                    )
                    .unwrap();
                }
                if duration == "invalid" {
                    conn.execute_batch("UPDATE main.automation_scheduler_last_pass SET duration_ms='PRIVATE_INVALID_DURATION'").unwrap();
                }
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_scheduler_last_pass(id INTEGER,status TEXT,duration_ms INTEGER,scheduled_at TEXT,started_at TEXT);
                    INSERT INTO temp.automation_scheduler_last_pass VALUES(1,'failed',999,'2026-10-18T00:00:00.000Z','2026-10-18T00:00:01.000Z');").unwrap();
                }
                let before = conn.total_changes();
                let report = inspect(&conn, Utc::now())
                    .expect("missing lag columns must retain independent observations");
                let status = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::LastSuccessfulPass)
                    .unwrap();
                assert_eq!(status.facts[&Fact::LatestPassSucceeded], 1);
                let planning = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::PlanningStartLag)
                    .unwrap();
                assert!(!planning.facts.contains_key(&Fact::LatestPassStartLagMs));
                let expected = match duration {
                    "valid" => {
                        assert_eq!(planning.facts[&Fact::LatestPassDurationMs], 15);
                        serde_json::json!(["latest_pass_lag_unavailable"])
                    }
                    "invalid" => {
                        assert!(!planning.facts.contains_key(&Fact::LatestPassDurationMs));
                        assert!(!planning.facts.contains_key(&Fact::LatestPassStartLagMs));
                        serde_json::json!([
                            "invalid_latest_pass_duration",
                            "latest_pass_lag_unavailable"
                        ])
                    }
                    _ => {
                        assert!(!planning.facts.contains_key(&Fact::LatestPassDurationMs));
                        assert!(!planning.facts.contains_key(&Fact::LatestPassStartLagMs));
                        serde_json::json!([
                            "latest_pass_duration_unavailable",
                            "latest_pass_lag_unavailable"
                        ])
                    }
                };
                assert_eq!(serde_json::to_value(&planning.gaps).unwrap(), expected);
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                report.validate().unwrap();
            }
        }
    }
}

#[test]
fn unavailable_pass_status_preserves_independent_history_and_planning() {
    for missing in ["status", "id", "table"] {
        for history in ["valid", "invalid", "missing"] {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                conn.execute_batch("DROP TABLE main.automation_scheduler_last_pass")
                    .unwrap();
                if missing == "status" {
                    conn.execute_batch("CREATE TABLE main.automation_scheduler_last_pass(id INTEGER PRIMARY KEY,duration_ms INTEGER,scheduled_at TEXT,started_at TEXT);
                        INSERT INTO main.automation_scheduler_last_pass VALUES(1,15,'2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.010Z');").unwrap();
                } else if missing == "id" {
                    conn.execute_batch("CREATE TABLE main.automation_scheduler_last_pass(status TEXT,duration_ms INTEGER,scheduled_at TEXT,started_at TEXT);
                        INSERT INTO main.automation_scheduler_last_pass VALUES('succeeded',15,'2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.010Z');").unwrap();
                }
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_scheduler_last_pass(id INTEGER,status TEXT,duration_ms INTEGER,scheduled_at TEXT,started_at TEXT);
                        INSERT INTO temp.automation_scheduler_last_pass VALUES(1,'failed',999,'2026-10-18T00:00:00.000Z','2026-10-18T00:00:01.000Z');").unwrap();
                }
                if history == "missing" {
                    conn.execute_batch("DROP TABLE main.automation_scheduler_last_successful_pass")
                        .unwrap();
                } else {
                    conn.execute("INSERT INTO main.automation_scheduler_last_successful_pass(id,scheduler_generation,finished_at,duration_ms) VALUES(1,1,?1,15)",
                        [if history == "valid" { "2026-10-18T00:00:00.025Z" } else { "PRIVATE_INVALID_HISTORY" }]).unwrap();
                }
                let before = conn.total_changes();
                let report = inspect(&conn, Utc::now())
                    .expect("missing status prerequisites must preserve independent diagnostics");
                let status = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::LastSuccessfulPass)
                    .unwrap();
                assert!(!status.facts.contains_key(&Fact::LatestPassSucceeded));
                let expected = match history {
                    "valid" => {
                        assert_eq!(
                            status.facts[&Fact::LastSuccessfulPassFinishedAtMillis],
                            1792281600025
                        );
                        serde_json::json!(["latest_pass_status_unavailable"])
                    }
                    "invalid" => {
                        assert!(status.facts.is_empty());
                        serde_json::json!([
                            "latest_pass_status_unavailable",
                            "invalid_persisted_metadata"
                        ])
                    }
                    _ => {
                        assert!(status.facts.is_empty());
                        serde_json::json!([
                            "latest_pass_status_unavailable",
                            "successful_pass_store_unavailable"
                        ])
                    }
                };
                assert_eq!(serde_json::to_value(&status.gaps).unwrap(), expected);
                let planning = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::PlanningStartLag)
                    .unwrap();
                if missing == "status" {
                    assert_eq!(planning.facts[&Fact::LatestPassDurationMs], 15);
                    assert_eq!(planning.facts[&Fact::LatestPassStartLagMs], 10);
                } else {
                    assert!(!planning.facts.contains_key(&Fact::LatestPassDurationMs));
                    assert!(!planning.facts.contains_key(&Fact::LatestPassStartLagMs));
                    assert_eq!(
                        serde_json::to_value(&planning.gaps).unwrap(),
                        serde_json::json!([
                            "latest_pass_duration_unavailable",
                            "latest_pass_lag_unavailable"
                        ])
                    );
                }
                report.validate().unwrap();
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                let mut forged = report.clone();
                let status = forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::LastSuccessfulPass)
                    .unwrap();
                status.facts.insert(Fact::LatestPassSucceeded, 0);
                assert!(
                    forged.validate().is_err(),
                    "unavailable status cannot claim measured failure"
                );
            }
        }
    }
}

#[test]
fn unavailable_authority_projection_preserves_independent_lease_counts() {
    for missing in ["id", "generation", "owner_id", "table"] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_scheduler_authority")
                .unwrap();
            if missing != "table" {
                let columns = ["id INTEGER", "generation INTEGER", "owner_id TEXT"]
                    .into_iter()
                    .filter(|column| !column.starts_with(missing))
                    .collect::<Vec<_>>()
                    .join(",");
                conn.execute_batch(&format!(
                    "CREATE TABLE main.automation_scheduler_authority({columns})"
                ))
                .unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_scheduler_authority(id INTEGER,generation INTEGER,owner_id TEXT);
                    INSERT INTO temp.automation_scheduler_authority VALUES(1,999,'PRIVATE_SHADOW_OWNER');").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now())
                .expect("unavailable authority must not abort independent categories");
            let leadership = report
                .observations
                .iter()
                .find(|o| o.category == Category::Leadership)
                .unwrap();
            assert!(leadership.facts.is_empty());
            assert_eq!(
                serde_json::to_value(&leadership.gaps).unwrap(),
                serde_json::json!(["recorded_authority_unavailable"])
            );
            let leases = report
                .observations
                .iter()
                .find(|o| o.category == Category::LeasesFences)
                .unwrap();
            assert_eq!(leases.facts[&Fact::LeaseBearingOccurrences], 0);
            assert!(!leases
                .facts
                .contains_key(&Fact::RecordedSchedulerFencePresent));
            assert!(!leases
                .facts
                .contains_key(&Fact::MismatchedUnlaunchedSchedulerClaims));
            assert_eq!(
                serde_json::to_value(&leases.gaps).unwrap(),
                serde_json::json!(["recorded_authority_unavailable"])
            );
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
            let mut forged = report.clone();
            forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::LeasesFences)
                .unwrap()
                .facts
                .insert(Fact::RecordedSchedulerFencePresent, 0);
            assert!(forged.validate().is_err());
        }
    }
}

#[test]
fn missing_claim_generation_keeps_recorded_leadership_and_lease_evidence() {
    let (_root, conn) = fixture();
    conn.execute("UPDATE main.automation_scheduler_authority SET generation=2,owner_id='PRIVATE_RECORDED_OWNER' WHERE id=1",[]).unwrap();
    conn.execute_batch("DROP TABLE main.automation_occurrences;
        CREATE TABLE main.automation_occurrences(id TEXT,automation_id TEXT,state TEXT,scheduled_for TEXT,lease_owner TEXT,lease_expires_at TEXT);
        INSERT INTO main.automation_occurrences VALUES('one','one','claimed','2026-10-18T00:00:00.000Z','daemon','2026-10-18T00:00:30.000Z');").unwrap();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
        .unwrap()
        .with_timezone(&Utc);
    let before = conn.total_changes();
    let report =
        inspect(&conn, now).expect("claim query prerequisites must not erase authority metadata");
    let leadership = report
        .observations
        .iter()
        .find(|o| o.category == Category::Leadership)
        .unwrap();
    assert_eq!(leadership.facts[&Fact::LeaderAssigned], 1);
    assert_eq!(leadership.facts[&Fact::SchedulerGeneration], 2);
    let leases = report
        .observations
        .iter()
        .find(|o| o.category == Category::LeasesFences)
        .unwrap();
    assert_eq!(leases.facts[&Fact::RecordedSchedulerFencePresent], 1);
    assert_eq!(leases.facts[&Fact::LeaseBearingOccurrences], 1);
    assert_eq!(leases.facts[&Fact::ActiveLeaseOccurrences], 1);
    assert!(!leases
        .facts
        .contains_key(&Fact::MismatchedUnlaunchedSchedulerClaims));
    assert_eq!(
        serde_json::to_value(&leases.gaps).unwrap(),
        serde_json::json!(["recorded_claim_store_unavailable"])
    );
    assert_eq!(conn.total_changes(), before);
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    report.validate().unwrap();
    let mut forged = report.clone();
    forged
        .observations
        .iter_mut()
        .find(|o| o.category == Category::LeasesFences)
        .unwrap()
        .facts
        .insert(Fact::MismatchedUnlaunchedSchedulerClaims, 0);
    assert!(forged.validate().is_err());
}

#[test]
fn unavailable_delivery_state_preserves_receipt_count_and_concurrent_gaps() {
    for missing_receipts in [false, true] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_runs;
                CREATE TABLE main.automation_runs(id TEXT,automation_id TEXT,occurrence_id TEXT,status TEXT,timeout_at TEXT);").unwrap();
            conn.execute_batch("DROP TABLE main.automation_receipts")
                .unwrap();
            if !missing_receipts {
                // A table-only count needs no receipt payload contract. Nonzero
                // main rows distinguish preserved evidence from a default zero.
                conn.execute_batch("CREATE TABLE main.automation_receipts(id TEXT PRIMARY KEY);
                    INSERT INTO main.automation_receipts VALUES('PRIVATE_MAIN_RECEIPT_A'),('PRIVATE_MAIN_RECEIPT_B');").unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_runs(id TEXT,automation_id TEXT,occurrence_id TEXT,status TEXT,timeout_at TEXT,delivery_state TEXT);
                    INSERT INTO temp.automation_runs VALUES('PRIVATE_SHADOW_RUN','shadow',NULL,'failed',NULL,'failed');
                    CREATE TEMP TABLE automation_receipts(value TEXT);
                    INSERT INTO temp.automation_receipts VALUES('PRIVATE_SHADOW_RECEIPT');").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now())
                .expect("unavailable delivery state must retain independent receipt evidence");
            let delivery = report
                .observations
                .iter()
                .find(|o| o.category == Category::DeliveryReceipts)
                .unwrap();
            assert!(!delivery.facts.contains_key(&Fact::FailedDeliveries));
            assert!(!delivery.facts.contains_key(&Fact::PendingDeliveries));
            if missing_receipts {
                assert!(delivery.facts.is_empty());
                assert_eq!(
                    serde_json::to_value(&delivery.gaps).unwrap(),
                    serde_json::json!([
                        "delivery_state_unavailable",
                        "receipt_store_unavailable",
                        "receipt_link_store_unavailable"
                    ])
                );
            } else {
                assert_eq!(delivery.facts[&Fact::StoredReceipts], 2);
                assert_eq!(
                    serde_json::to_value(&delivery.gaps).unwrap(),
                    serde_json::json!([
                        "delivery_state_unavailable",
                        "receipt_link_store_unavailable",
                        "receipt_commitments_unavailable",
                        "receipt_authority_not_verified"
                    ])
                );
            }
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
            let mut forged = report.clone();
            forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DeliveryReceipts)
                .unwrap()
                .facts
                .insert(Fact::FailedDeliveries, 0);
            assert!(
                forged.validate().is_err(),
                "unavailable delivery state is not measured zero failures"
            );
            if missing_receipts {
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::DeliveryReceipts)
                    .unwrap()
                    .gaps
                    .reverse();
                assert!(
                    forged.validate().is_err(),
                    "concurrent gaps have one canonical order"
                );
            }
        }
    }
}

#[test]
fn failure_class_prerequisites_are_independent_of_failed_attempt_counts() {
    for missing in [false, true] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_attempts;
                CREATE TABLE main.automation_attempts(id TEXT PRIMARY KEY,run_id TEXT,state TEXT,not_before TEXT);
                INSERT INTO main.automation_attempts VALUES('one','one','failed',NULL),('two','two','failed',NULL),('three','three','succeeded',NULL);").unwrap();
            if !missing {
                conn.execute_batch("ALTER TABLE main.automation_attempts ADD COLUMN failure_class TEXT;
                    UPDATE main.automation_attempts SET failure_class=CASE id WHEN 'one' THEN 'launch_refused' WHEN 'two' THEN 'runtime_authority_unsupported' ELSE NULL END;").unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_attempts(id TEXT,run_id TEXT,state TEXT,not_before TEXT,failure_class TEXT);
                    INSERT INTO temp.automation_attempts VALUES('PRIVATE_SHADOW','shadow','failed',NULL,'launch_refused');").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now())
                .expect("missing failure class must preserve failed-attempt observations");
            let failures = report
                .observations
                .iter()
                .find(|o| o.category == Category::RepeatedFailures)
                .unwrap();
            assert_eq!(
                failures.facts[&Fact::FailedAttempts],
                2,
                "failed attempts must come from main"
            );
            let refusals = report
                .observations
                .iter()
                .find(|o| o.category == Category::AuthorityRefusals)
                .unwrap();
            if missing {
                assert!(refusals.facts.is_empty());
                assert_eq!(
                    serde_json::to_value(&refusals.gaps).unwrap(),
                    serde_json::json!(["attempt_failure_class_unavailable"])
                );
            } else {
                assert_eq!(refusals.facts[&Fact::LaunchRefusedAttempts], 1);
                assert_eq!(refusals.facts[&Fact::UnsupportedAuthorityAttempts], 1);
                assert_eq!(
                    refusals.gaps,
                    vec![Gap::IdentityAndApprovalRefusalsNotRecorded]
                );
            }
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
            if missing {
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::AuthorityRefusals)
                    .unwrap()
                    .facts
                    .insert(Fact::LaunchRefusedAttempts, 0);
                assert!(
                    forged.validate().is_err(),
                    "unavailable classification is not zero refusals"
                );
            }
        }
    }
}

#[test]
fn missing_retry_counters_preserve_quarantine_and_ignore_temp_histories() {
    for missing in [true, false] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_retry_state;
                CREATE TABLE main.automation_retry_state(automation_id TEXT PRIMARY KEY,quarantined_at TEXT);
                INSERT INTO main.automation_retry_state VALUES('one','2026-10-18T00:00:00.000Z'),('two','2026-10-18T00:00:00.000Z'),('three',NULL);").unwrap();
            if !missing {
                conn.execute_batch("ALTER TABLE main.automation_retry_state ADD COLUMN consecutive_exhaustions INTEGER;
                    UPDATE main.automation_retry_state SET consecutive_exhaustions=CASE automation_id WHEN 'one' THEN 1 WHEN 'two' THEN 3 ELSE 0 END;").unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_retry_state(automation_id TEXT,quarantined_at TEXT,consecutive_exhaustions INTEGER,reason TEXT);
                    INSERT INTO temp.automation_retry_state VALUES('shadow','2026-10-18T00:00:00.000Z',999,'PRIVATE_RETRY_SHADOW');").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now())
                .expect("missing retry counters must retain independent quarantine evidence");
            let failures = report
                .observations
                .iter()
                .find(|o| o.category == Category::RepeatedFailures)
                .unwrap();
            assert_eq!(failures.facts[&Fact::QuarantinedRoutines], 2);
            assert_eq!(failures.facts[&Fact::FailedAttempts], 0);
            if missing {
                assert!(!failures.facts.contains_key(&Fact::RetryExhaustionHistories));
                assert!(!failures
                    .facts
                    .contains_key(&Fact::MaximumConsecutiveRetryExhaustions));
                assert_eq!(
                    serde_json::to_value(&failures.gaps).unwrap(),
                    serde_json::json!(["retry_exhaustion_history_unavailable"])
                );
            } else {
                assert_eq!(failures.facts[&Fact::RetryExhaustionHistories], 2);
                assert_eq!(failures.facts[&Fact::MaximumConsecutiveRetryExhaustions], 3);
                assert_eq!(failures.gaps, vec![Gap::FailureHistoryNotSummarized]);
            }
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
            if missing {
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::RepeatedFailures)
                    .unwrap()
                    .facts
                    .insert(Fact::RetryExhaustionHistories, 0);
                assert!(
                    forged.validate().is_err(),
                    "unavailable history is not zero exhaustion"
                );
            }
        }
    }
}

#[test]
fn eligibility_prerequisites_preserve_independent_queue_and_lease_observations() {
    let mut cases = vec![
        ("automation_definitions", None),
        ("automation_missions", None),
        ("automation_runs", None),
        ("automation_missions", Some("automation_id")),
        ("automation_retry_state", Some("automation_id")),
        ("automation_occurrences", Some("id")),
        ("automation_occurrences", Some("automation_id")),
        ("automation_attempts", Some("run_id")),
        ("automation_attempts", Some("not_before")),
    ];
    for column in [
        "id",
        "name",
        "status",
        "definition_json",
        "revision",
        "definition_digest",
        "lifecycle_state",
        "tombstoned_at",
        "updated_at",
    ] {
        cases.push(("automation_definitions", Some(column)));
    }
    for column in [
        "id",
        "automation_id",
        "occurrence_id",
        "status",
        "timeout_at",
    ] {
        cases.push(("automation_runs", Some(column)));
    }
    for (table, missing_column) in cases {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            // Construct intentionally incompatible parent schemas without FK
            // enforcement blocking fixture writes. Restore it before inspection.
            conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            let columns: Vec<(String, String)> = conn
                .prepare(&format!("PRAGMA main.table_info({table})"))
                .unwrap()
                .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            if shadow {
                conn.execute_batch(&format!(
                    "CREATE TEMP TABLE {table} AS SELECT * FROM main.{table}"
                ))
                .unwrap();
            }
            conn.execute_batch(&format!("DROP TABLE main.{table}"))
                .unwrap();
            if let Some(missing) = missing_column {
                let columns = columns
                    .iter()
                    .filter(|(name, _)| name != missing)
                    .map(|(name, kind)| format!("\"{name}\" {kind}"))
                    .collect::<Vec<_>>()
                    .join(",");
                conn.execute_batch(&format!("CREATE TABLE main.{table}({columns})"))
                    .unwrap();
            }
            if table == "automation_occurrences" {
                conn.execute_batch("INSERT INTO main.automation_occurrences(state,scheduled_for,lease_owner,lease_expires_at)
                    VALUES('planned','2026-10-18T00:00:00.000Z',NULL,NULL),('claimed','2026-10-18T00:00:00.000Z','daemon','2026-10-18T00:00:30.000Z');").unwrap();
            } else {
                conn.execute_batch("INSERT INTO main.automation_occurrences(id,automation_id,state,scheduled_for,created_at,updated_at,lease_owner,lease_expires_at)
                    VALUES('planned','one','planned','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.000Z',NULL,NULL),
                          ('claimed','two','claimed','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.000Z','daemon','2026-10-18T00:00:30.000Z');").unwrap();
            }
            conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            let before = conn.total_changes();
            let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
                .unwrap()
                .with_timezone(&Utc);
            let report = inspect(&conn, now).unwrap_or_else(|error| {
                panic!("{table} {missing_column:?} shadow={shadow}: {error}")
            });
            let queue = report
                .observations
                .iter()
                .find(|o| o.category == Category::Queue)
                .unwrap();
            assert_eq!(queue.facts[&Fact::PlannedOccurrences], 1);
            assert_eq!(queue.facts[&Fact::DuePlannedOccurrences], 1);
            assert_eq!(queue.facts[&Fact::ClaimedOccurrences], 1);
            assert!(!queue.facts.contains_key(&Fact::EligibleOccurrences));
            assert_eq!(
                serde_json::to_value(&queue.gaps).unwrap(),
                serde_json::json!(["eligibility_store_unavailable"])
            );
            let leases = report
                .observations
                .iter()
                .find(|o| o.category == Category::LeasesFences)
                .unwrap();
            assert_eq!(leases.facts[&Fact::LeaseBearingOccurrences], 1);
            assert_eq!(leases.facts[&Fact::ActiveLeaseOccurrences], 1);
            report.validate().unwrap();
            assert_eq!(conn.total_changes(), before);
            assert!(conn
                .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, bool>(0))
                .unwrap());
            let mut forged = report.clone();
            forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::Queue)
                .unwrap()
                .facts
                .insert(Fact::EligibleOccurrences, 0);
            assert!(forged.validate().is_err());
        }
    }
}

#[test]
fn eligibility_uses_main_for_every_transitive_relation() {
    for table in [
        "automation_definitions",
        "automation_missions",
        "automation_retry_state",
        "automation_occurrences",
        "automation_runs",
        "automation_attempts",
    ] {
        let (_root, conn) = fixture();
        let definition=crate::automations::definition::RoutineDefinition::from_json(&serde_json::json!({
            "schemaVersion":1,"id":"main-routine","name":"main-routine","status":"ACTIVE",
            "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid",
            "timeoutMinutes":30,"runtime":"coven-code","prompt":"PRIVATE_MAIN_PROMPT"
        })).unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        conn.execute_batch("INSERT INTO main.automation_occurrences(id,automation_id,scheduled_for,created_at,updated_at)
            VALUES('main-occurrence','main-routine','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.000Z');
            INSERT INTO main.automation_runs(id,automation_id,occurrence_id,status,started_at,timeout_at)
            VALUES('main-run','main-routine','main-occurrence','running','2026-10-18T00:00:00.000Z','2026-10-18T00:01:00.000Z');
").unwrap();
        if table == "automation_attempts" {
            conn.execute_batch("INSERT INTO main.automation_attempts(id,run_id,occurrence_id,attempt_number,adoption_key,occurrence_fence_generation,state,retry_classification,not_before,opened_at)
                VALUES('main-attempt','main-run','main-occurrence',1,'PRIVATE_MAIN_KEY',1,'adopted','initial','2026-10-18T00:00:00.000Z','2026-10-18T00:00:00.000Z');").unwrap();
        }
        conn.execute_batch(&format!(
            "CREATE TEMP TABLE {table} AS SELECT * FROM main.{table}"
        ))
        .unwrap();
        match table {
            "automation_definitions" | "automation_occurrences" => conn
                .execute_batch(&format!("DELETE FROM temp.{table}"))
                .unwrap(),
            "automation_missions" => {
                conn.execute(
                    "INSERT INTO temp.automation_missions(automation_id) VALUES('main-routine')",
                    [],
                )
                .unwrap();
            }
            "automation_retry_state" => {
                conn.execute("INSERT INTO temp.automation_retry_state(automation_id,quarantined_at,reason) VALUES('main-routine','2026-10-18T00:00:00.000Z','PRIVATE_SHADOW')",[]).unwrap();
            }
            "automation_runs" => {
                conn.execute(
                    "UPDATE temp.automation_runs SET occurrence_id='other-occurrence'",
                    [],
                )
                .unwrap();
            }
            "automation_attempts" => {
                conn.execute(
                    "UPDATE temp.automation_attempts SET not_before='2026-10-18T00:00:30.000Z'",
                    [],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let before = conn.total_changes();
        let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
            .unwrap()
            .with_timezone(&Utc);
        let report = inspect(&conn, now).unwrap_or_else(|error| panic!("{table}: {error}"));
        let queue = report
            .observations
            .iter()
            .find(|o| o.category == Category::Queue)
            .unwrap();
        assert_eq!(
            queue.facts[&Fact::EligibleOccurrences],
            1,
            "{table} must not shadow main"
        );
        assert_eq!(queue.facts[&Fact::PlannedOccurrences], 1);
        assert_eq!(queue.facts[&Fact::DuePlannedOccurrences], 1);
        report.validate().unwrap();
        assert_eq!(conn.total_changes(), before);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    }
}

#[test]
fn eligibility_prerequisite_projection_executes_the_canonical_reader() {
    // Declare only the advertised prerequisites, then compile and execute the
    // real reader, including its transitive definition/mission/quarantine SQL.
    // A newly required but undeclared column must fail this contract test.
    let conn = Connection::open_in_memory().unwrap();
    for &(table, columns) in super::super::occurrences::ELIGIBILITY_QUERY_PREREQUISITES {
        let columns = columns
            .iter()
            .map(|name| format!("\"{name}\" TEXT"))
            .collect::<Vec<_>>()
            .join(",");
        conn.execute_batch(&format!("CREATE TABLE main.{table}({columns})"))
            .unwrap();
    }
    let before = conn.total_changes();
    assert_eq!(
        super::super::occurrences::eligible_occurrence_count(&conn, Utc::now()).unwrap(),
        0
    );
    assert_eq!(conn.total_changes(), before);
}

#[test]
fn missing_schedule_keeps_queue_states_and_independent_lease_evidence() {
    for shadow in [false, true] {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TABLE main.automation_occurrences;
            CREATE TABLE main.automation_occurrences(id TEXT PRIMARY KEY,automation_id TEXT,state TEXT,lease_owner TEXT,lease_expires_at TEXT);
            INSERT INTO main.automation_occurrences VALUES('one','one','planned',NULL,NULL),('two','two','planned',NULL,NULL),
                ('three','three','claimed','daemon','2026-10-18T00:00:30.000Z');").unwrap();
        if shadow {
            conn.execute_batch("CREATE TEMP TABLE automation_occurrences AS SELECT * FROM main.automation_occurrences;
                ALTER TABLE temp.automation_occurrences ADD COLUMN scheduled_for TEXT;
                UPDATE temp.automation_occurrences SET scheduled_for='2026-10-18T00:00:00.000Z';").unwrap();
        }
        let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
            .unwrap()
            .with_timezone(&Utc);
        let before = conn.total_changes();
        let report = inspect(&conn, now)
            .expect("missing schedule must retain independent state and lease observations");
        let queue = report
            .observations
            .iter()
            .find(|o| o.category == Category::Queue)
            .unwrap();
        assert_eq!(queue.facts[&Fact::PlannedOccurrences], 2);
        assert_eq!(queue.facts[&Fact::ClaimedOccurrences], 1);
        assert_eq!(queue.facts[&Fact::RunningOccurrences], 0);
        assert_eq!(queue.facts[&Fact::RecoveryRequiredOccurrences], 0);
        assert!(!queue.facts.contains_key(&Fact::DuePlannedOccurrences));
        assert!(!queue.facts.contains_key(&Fact::EligibleOccurrences));
        assert_eq!(
            serde_json::to_value(&queue.gaps).unwrap(),
            serde_json::json!(["due_queue_unavailable", "eligibility_store_unavailable"])
        );
        let leases = report
            .observations
            .iter()
            .find(|o| o.category == Category::LeasesFences)
            .unwrap();
        assert_eq!(leases.facts[&Fact::LeaseBearingOccurrences], 1);
        assert_eq!(leases.facts[&Fact::ActiveLeaseOccurrences], 1);
        assert_eq!(conn.total_changes(), before);
        report.validate().unwrap();
        let mut forged = report.clone();
        forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::Queue)
            .unwrap()
            .facts
            .insert(Fact::DuePlannedOccurrences, 0);
        assert!(
            forged.validate().is_err(),
            "unavailable due count cannot be measured zero"
        );
        let mut forged = report.clone();
        forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::Queue)
            .unwrap()
            .gaps
            .reverse();
        assert!(
            forged.validate().is_err(),
            "unavailable queue gaps have one canonical order"
        );
    }
}

#[test]
fn queue_count_subsets_are_validated_without_inventing_missing_counts() {
    let (_root, conn) = fixture();
    let report = inspect(&conn, Utc::now()).unwrap();
    for (planned, due, eligible) in [(0, 1, 0), (0, 0, 1), (2, 0, 1)] {
        let mut forged = report.clone();
        let queue = forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::Queue)
            .unwrap();
        queue.facts.insert(Fact::PlannedOccurrences, planned);
        queue.facts.insert(Fact::DuePlannedOccurrences, due);
        queue.facts.insert(Fact::EligibleOccurrences, eligible);
        assert!(
            forged.validate().is_err(),
            "queue subsets cannot contradict: planned={planned},due={due},eligible={eligible}"
        );
    }
    let mut forged = report.clone();
    let queue = forged
        .observations
        .iter_mut()
        .find(|o| o.category == Category::Queue)
        .unwrap();
    queue.gaps = vec![Gap::EligibilityStoreUnavailable];
    queue.facts.remove(&Fact::EligibleOccurrences);
    queue.facts.insert(Fact::DuePlannedOccurrences, 1);
    assert!(
        forged.validate().is_err(),
        "unavailable eligibility cannot excuse due exceeding planned"
    );
    let mut valid = report;
    let queue = valid
        .observations
        .iter_mut()
        .find(|o| o.category == Category::Queue)
        .unwrap();
    queue.facts.insert(Fact::PlannedOccurrences, 2);
    queue.facts.insert(Fact::DuePlannedOccurrences, 1);
    queue.facts.insert(Fact::EligibleOccurrences, 1);
    // The nonempty queue cannot retain the fixture's measured empty history.
    valid
        .observations
        .iter_mut()
        .find(|o| o.category == Category::RepeatedFailures)
        .unwrap()
        .facts
        .insert(Fact::RetainedOccurrenceFailureHistoriesChecked, 1);
    valid
        .validate()
        .expect("consistent nonzero subsets remain valid");
}

#[test]
fn unavailable_quarantine_preserves_attempts_and_exact_concurrent_history_gaps() {
    for history in ["valid", "invalid", "missing", "table"] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_retry_state;
                DROP TABLE main.automation_attempts;
                CREATE TABLE main.automation_attempts(id TEXT PRIMARY KEY,run_id TEXT,state TEXT,not_before TEXT,failure_class TEXT);
                INSERT INTO main.automation_attempts VALUES('attempt-one','run-one','failed',NULL,NULL),('attempt-two','run-two','failed',NULL,NULL);").unwrap();
            if history != "table" {
                let counter = if history == "missing" {
                    ""
                } else {
                    ",consecutive_exhaustions INTEGER"
                };
                conn.execute_batch(&format!("CREATE TABLE main.automation_retry_state(automation_id TEXT PRIMARY KEY{counter});
                    INSERT INTO main.automation_retry_state(automation_id) VALUES('one'),('two'),('three');")).unwrap();
                if history != "missing" {
                    conn.execute_batch("UPDATE main.automation_retry_state SET consecutive_exhaustions=CASE automation_id WHEN 'one' THEN 1 WHEN 'two' THEN 3 ELSE 0 END;").unwrap();
                    if history == "invalid" {
                        conn.execute("UPDATE main.automation_retry_state SET consecutive_exhaustions='PRIVATE_INVALID_COUNTER' WHERE automation_id='one'",[]).unwrap();
                    }
                }
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_retry_state(automation_id TEXT,quarantined_at TEXT,consecutive_exhaustions INTEGER,reason TEXT);
                    INSERT INTO temp.automation_retry_state VALUES('shadow','2026-10-18T00:00:00.000Z',999,'PRIVATE_SHADOW_QUARANTINE');").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now())
                .expect("unavailable quarantine must preserve independent attempt/history facts");
            let failures = report
                .observations
                .iter()
                .find(|o| o.category == Category::RepeatedFailures)
                .unwrap();
            assert_eq!(failures.facts[&Fact::FailedAttempts], 2);
            assert!(!failures.facts.contains_key(&Fact::QuarantinedRoutines));
            let gaps = match history {
                "valid" => {
                    assert_eq!(failures.facts[&Fact::RetryExhaustionHistories], 2);
                    assert_eq!(failures.facts[&Fact::MaximumConsecutiveRetryExhaustions], 3);
                    serde_json::json!(["quarantine_store_unavailable"])
                }
                "invalid" => serde_json::json!([
                    "quarantine_store_unavailable",
                    "invalid_persisted_metadata"
                ]),
                _ => serde_json::json!([
                    "quarantine_store_unavailable",
                    "retry_exhaustion_history_unavailable"
                ]),
            };
            assert_eq!(serde_json::to_value(&failures.gaps).unwrap(), gaps);
            if history != "valid" {
                assert_eq!(failures.facts[&Fact::FailedAttempts], 2);
                assert!(!failures.facts.contains_key(&Fact::QuarantinedRoutines));
                assert!(!failures.facts.contains_key(&Fact::RetryExhaustionHistories));
            }
            let queue = report
                .observations
                .iter()
                .find(|o| o.category == Category::Queue)
                .unwrap();
            assert!(!queue.facts.contains_key(&Fact::EligibleOccurrences));
            assert_eq!(queue.gaps, vec![Gap::EligibilityStoreUnavailable]);
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
            if history == "table" {
                assert_eq!(conn.query_row("SELECT COUNT(*) FROM main.sqlite_schema WHERE type='table' AND name='automation_retry_state'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
            }
            let mut forged = report.clone();
            forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::RepeatedFailures)
                .unwrap()
                .facts
                .insert(Fact::QuarantinedRoutines, 0);
            assert!(
                forged.validate().is_err(),
                "unavailable quarantine cannot be measured zero"
            );
            if history != "valid" {
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::RepeatedFailures)
                    .unwrap()
                    .gaps
                    .reverse();
                assert!(
                    forged.validate().is_err(),
                    "concurrent history gaps must retain canonical order"
                );
            }
        }
    }
}

#[test]
fn unavailable_attempt_state_preserves_independent_retry_and_refusal_facts() {
    for attempts in ["state", "table"] {
        for quarantine in [true, false] {
            for history in ["valid", "invalid", "missing"] {
                for shadow in [false, true] {
                    let (_root, conn) = fixture();
                    conn.execute_batch("DROP TABLE main.automation_attempts; DROP TABLE main.automation_retry_state;").unwrap();
                    if attempts == "state" {
                        conn.execute_batch("CREATE TABLE main.automation_attempts(id TEXT PRIMARY KEY,run_id TEXT,not_before TEXT,failure_class TEXT);
                            INSERT INTO main.automation_attempts VALUES('one','one',NULL,'launch_refused'),('two','two',NULL,'runtime_authority_unsupported');").unwrap();
                    }
                    let q = if quarantine {
                        ",quarantined_at TEXT"
                    } else {
                        ""
                    };
                    let h = if history != "missing" {
                        ",consecutive_exhaustions INTEGER"
                    } else {
                        ""
                    };
                    conn.execute_batch(&format!("CREATE TABLE main.automation_retry_state(automation_id TEXT PRIMARY KEY{q}{h});
                        INSERT INTO main.automation_retry_state(automation_id) VALUES('one'),('two'),('three');")).unwrap();
                    if quarantine {
                        conn.execute_batch("UPDATE main.automation_retry_state SET quarantined_at='2026-10-18T00:00:00.000Z' WHERE automation_id IN ('one','two');").unwrap();
                    }
                    if history != "missing" {
                        conn.execute_batch("UPDATE main.automation_retry_state SET consecutive_exhaustions=CASE automation_id WHEN 'one' THEN 1 WHEN 'two' THEN 3 ELSE 0 END;").unwrap();
                        if history == "invalid" {
                            conn.execute("UPDATE main.automation_retry_state SET consecutive_exhaustions='PRIVATE_COUNTER' WHERE automation_id='one'",[]).unwrap();
                        }
                    }
                    if shadow {
                        conn.execute_batch("CREATE TEMP TABLE automation_attempts(id TEXT,run_id TEXT,state TEXT,not_before TEXT,failure_class TEXT);
                            INSERT INTO temp.automation_attempts VALUES('PRIVATE_SHADOW','shadow','failed',NULL,'launch_refused');
                            CREATE TEMP TABLE automation_retry_state(automation_id TEXT,quarantined_at TEXT,consecutive_exhaustions INTEGER);
                            INSERT INTO temp.automation_retry_state VALUES('shadow','2026-10-18T00:00:00.000Z',999);").unwrap();
                    }
                    let before = conn.total_changes();
                    let report = inspect(&conn, Utc::now()).expect(
                        "missing attempt state must preserve independent retry/refusal evidence",
                    );
                    let failures = report
                        .observations
                        .iter()
                        .find(|o| o.category == Category::RepeatedFailures)
                        .unwrap();
                    assert!(!failures.facts.contains_key(&Fact::FailedAttempts));
                    if quarantine {
                        assert_eq!(failures.facts[&Fact::QuarantinedRoutines], 2);
                    } else {
                        assert!(!failures.facts.contains_key(&Fact::QuarantinedRoutines));
                    }
                    if history == "valid" {
                        assert_eq!(failures.facts[&Fact::RetryExhaustionHistories], 2);
                        assert_eq!(failures.facts[&Fact::MaximumConsecutiveRetryExhaustions], 3);
                    } else {
                        assert!(!failures.facts.contains_key(&Fact::RetryExhaustionHistories));
                        assert!(!failures
                            .facts
                            .contains_key(&Fact::MaximumConsecutiveRetryExhaustions));
                    }
                    let mut expected = vec!["attempt_state_unavailable"];
                    if !quarantine {
                        expected.push("quarantine_store_unavailable");
                    }
                    if history == "missing" {
                        expected.push("retry_exhaustion_history_unavailable");
                    } else if history == "invalid" {
                        expected.push("invalid_persisted_metadata");
                    }
                    assert_eq!(
                        serde_json::to_value(&failures.gaps).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                    let refusals = report
                        .observations
                        .iter()
                        .find(|o| o.category == Category::AuthorityRefusals)
                        .unwrap();
                    if attempts == "state" {
                        assert_eq!(refusals.facts[&Fact::LaunchRefusedAttempts], 1);
                        assert_eq!(refusals.facts[&Fact::UnsupportedAuthorityAttempts], 1);
                    } else {
                        assert!(refusals.facts.is_empty());
                        assert_eq!(refusals.gaps, vec![Gap::AttemptFailureClassUnavailable]);
                        assert_eq!(conn.query_row("SELECT COUNT(*) FROM main.sqlite_schema WHERE type='table' AND name='automation_attempts'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
                    }
                    let queue = report
                        .observations
                        .iter()
                        .find(|o| o.category == Category::Queue)
                        .unwrap();
                    assert_eq!(queue.gaps, vec![Gap::EligibilityStoreUnavailable]);
                    assert_eq!(conn.total_changes(), before);
                    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                    report.validate().unwrap();
                    let mut forged = report.clone();
                    forged
                        .observations
                        .iter_mut()
                        .find(|o| o.category == Category::RepeatedFailures)
                        .unwrap()
                        .facts
                        .insert(Fact::FailedAttempts, 0);
                    assert!(
                        forged.validate().is_err(),
                        "unavailable attempts cannot be measured zero"
                    );
                    let mut forged = report.clone();
                    forged
                        .observations
                        .iter_mut()
                        .find(|o| o.category == Category::RepeatedFailures)
                        .unwrap()
                        .gaps
                        .push(Gap::FailureHistoryNotSummarized);
                    assert!(
                        forged.validate().is_err(),
                        "extra normal gap cannot expand unavailable evidence scope"
                    );
                    let mut forged = report.clone();
                    forged
                        .observations
                        .iter_mut()
                        .find(|o| o.category == Category::RepeatedFailures)
                        .unwrap()
                        .gaps
                        .push(Gap::AttemptStateUnavailable);
                    assert!(
                        forged.validate().is_err(),
                        "duplicate attempt gap cannot expand evidence scope"
                    );
                    if failures.gaps.len() > 1 {
                        let mut forged = report.clone();
                        forged
                            .observations
                            .iter_mut()
                            .find(|o| o.category == Category::RepeatedFailures)
                            .unwrap()
                            .gaps
                            .reverse();
                        assert!(
                            forged.validate().is_err(),
                            "combined failure gaps have canonical order"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn lease_prerequisites_preserve_independent_expiry_state_and_authority_evidence() {
    for missing in ["lease_expires_at", "lease_owner", "state", "table"] {
        for authority in ["assigned", "released", "invalid", "unavailable"] {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                match authority {
                    "assigned" => {
                        conn.execute("UPDATE main.automation_scheduler_authority SET generation=2,owner_id='PRIVATE_MAIN_OWNER' WHERE id=1",[]).unwrap();
                    }
                    "released" => {
                        conn.execute("UPDATE main.automation_scheduler_authority SET generation=2,owner_id=NULL WHERE id=1",[]).unwrap();
                    }
                    "invalid" => {
                        conn.execute("UPDATE main.automation_scheduler_authority SET generation=2,owner_id='' WHERE id=1",[]).unwrap();
                    }
                    _ => conn
                        .execute_batch("DROP TABLE main.automation_scheduler_authority")
                        .unwrap(),
                }
                conn.execute_batch("DROP TABLE main.automation_occurrences")
                    .unwrap();
                if missing != "table" {
                    let fields = [
                        ("id", "TEXT PRIMARY KEY"),
                        ("automation_id", "TEXT"),
                        ("scheduled_for", "TEXT"),
                        ("state", "TEXT"),
                        ("lease_owner", "TEXT"),
                        ("lease_expires_at", "TEXT"),
                        ("scheduler_generation", "INTEGER"),
                    ];
                    let definitions = fields
                        .iter()
                        .filter(|(name, _)| *name != missing)
                        .map(|(name, kind)| format!("{name} {kind}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    let names = fields
                        .iter()
                        .filter(|(name, _)| *name != missing)
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(",");
                    conn.execute_batch(&format!(
                        "CREATE TABLE main.automation_occurrences({definitions})"
                    ))
                    .unwrap();
                    for values in [
                        [
                            "'one'",
                            "'one'",
                            "'2026-10-18T00:00:00.000Z'",
                            "'claimed'",
                            "'daemon'",
                            "'2026-10-18T00:00:30.000Z'",
                            "1",
                        ],
                        [
                            "'two'",
                            "'two'",
                            "'2026-10-18T00:00:00.000Z'",
                            "'planned'",
                            "NULL",
                            "'PRIVATE_INVALID_EXPIRY'",
                            "NULL",
                        ],
                    ] {
                        let values = fields
                            .iter()
                            .zip(values)
                            .filter(|((name, _), _)| *name != missing)
                            .map(|(_, value)| value)
                            .collect::<Vec<_>>()
                            .join(",");
                        conn.execute_batch(&format!(
                            "INSERT INTO main.automation_occurrences({names}) VALUES({values})"
                        ))
                        .unwrap();
                    }
                }
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_occurrences(id TEXT,automation_id TEXT,scheduled_for TEXT,state TEXT,lease_owner TEXT,lease_expires_at TEXT,scheduler_generation INTEGER);
                        INSERT INTO temp.automation_occurrences VALUES('PRIVATE_SHADOW','shadow','2026-10-18T00:00:00.000Z','planned','daemon','2026-10-18T00:00:30.000Z',999);
                        CREATE TEMP TABLE automation_scheduler_authority(id INTEGER,generation INTEGER,owner_id TEXT);
                        INSERT INTO temp.automation_scheduler_authority VALUES(1,999,'PRIVATE_SHADOW_OWNER');").unwrap();
                }
                let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02.000Z")
                    .unwrap()
                    .with_timezone(&Utc);
                let before = conn.total_changes();
                let report = inspect(&conn, now).unwrap_or_else(|error| {
                    panic!("missing={missing},authority={authority},shadow={shadow}: {error}")
                });
                let leases = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::LeasesFences)
                    .unwrap();
                let cohort = missing == "state";
                if cohort {
                    assert_eq!(leases.facts[&Fact::LeaseBearingOccurrences], 2);
                    assert_eq!(leases.facts[&Fact::ActiveLeaseOccurrences], 1);
                    assert_eq!(leases.facts[&Fact::InvalidLeaseExpiryOccurrences], 1);
                    assert_eq!(leases.facts[&Fact::ExpiredLeaseOccurrences], 0);
                    assert_eq!(leases.facts[&Fact::MissingLeaseExpiryOccurrences], 0);
                } else {
                    for fact in [
                        Fact::LeaseBearingOccurrences,
                        Fact::ActiveLeaseOccurrences,
                        Fact::ExpiredLeaseOccurrences,
                        Fact::MissingLeaseExpiryOccurrences,
                        Fact::InvalidLeaseExpiryOccurrences,
                    ] {
                        assert!(!leases.facts.contains_key(&fact));
                    }
                }
                assert!(!leases
                    .facts
                    .contains_key(&Fact::LeaseOutsideActiveStateOccurrences));
                let mut expected = vec![if cohort {
                    "lease_state_unavailable"
                } else {
                    "lease_store_unavailable"
                }];
                match authority {
                    "assigned" => {
                        assert_eq!(leases.facts[&Fact::RecordedSchedulerFencePresent], 1);
                        if missing == "lease_expires_at" {
                            assert_eq!(leases.facts[&Fact::MismatchedUnlaunchedSchedulerClaims], 1);
                        } else {
                            assert!(!leases
                                .facts
                                .contains_key(&Fact::MismatchedUnlaunchedSchedulerClaims));
                            expected.push("recorded_claim_store_unavailable");
                        }
                    }
                    "released" => {
                        assert_eq!(leases.facts[&Fact::RecordedSchedulerFencePresent], 0);
                        assert!(!leases
                            .facts
                            .contains_key(&Fact::MismatchedUnlaunchedSchedulerClaims));
                    }
                    "invalid" => {
                        assert!(!leases
                            .facts
                            .contains_key(&Fact::RecordedSchedulerFencePresent));
                        expected.push("invalid_persisted_metadata");
                    }
                    _ => {
                        assert!(!leases
                            .facts
                            .contains_key(&Fact::RecordedSchedulerFencePresent));
                        expected.push("recorded_authority_unavailable");
                    }
                }
                assert_eq!(
                    serde_json::to_value(&leases.gaps).unwrap(),
                    serde_json::to_value(expected).unwrap()
                );
                let queue = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::Queue)
                    .unwrap();
                if missing == "state" || missing == "table" {
                    assert!(queue.facts.is_empty());
                    assert_eq!(
                        serde_json::to_value(&queue.gaps).unwrap(),
                        serde_json::json!([
                            "queue_state_unavailable",
                            "due_queue_unavailable",
                            "eligibility_store_unavailable"
                        ])
                    );
                } else {
                    assert_eq!(queue.facts[&Fact::PlannedOccurrences], 1);
                    assert_eq!(queue.facts[&Fact::ClaimedOccurrences], 1);
                    assert_eq!(queue.facts[&Fact::DuePlannedOccurrences], 1);
                    assert_eq!(queue.facts[&Fact::EligibleOccurrences], 0);
                }
                let leadership = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::Leadership)
                    .unwrap();
                if authority == "assigned" || authority == "released" {
                    assert_eq!(leadership.facts[&Fact::SchedulerGeneration], 2);
                    assert_eq!(
                        leadership.facts[&Fact::LeaderAssigned],
                        u64::from(authority == "assigned")
                    );
                } else {
                    assert!(leadership.facts.is_empty());
                }
                report.validate().unwrap();
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                if missing == "table" {
                    assert_eq!(conn.query_row("SELECT COUNT(*) FROM main.sqlite_schema WHERE type='table' AND name='automation_occurrences'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
                }
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::LeasesFences)
                    .unwrap()
                    .facts
                    .insert(
                        if cohort {
                            Fact::LeaseOutsideActiveStateOccurrences
                        } else {
                            Fact::LeaseBearingOccurrences
                        },
                        0,
                    );
                assert!(
                    forged.validate().is_err(),
                    "unavailable lease fact cannot become measured zero"
                );
                if missing == "state" && authority == "assigned" {
                    let mut forged = report.clone();
                    let observation = forged
                        .observations
                        .iter_mut()
                        .find(|o| o.category == Category::LeasesFences)
                        .unwrap();
                    observation
                        .gaps
                        .retain(|gap| *gap != Gap::RecordedClaimStoreUnavailable);
                    observation
                        .facts
                        .insert(Fact::MismatchedUnlaunchedSchedulerClaims, 0);
                    assert!(
                        forged.validate().is_err(),
                        "missing occurrence state also prevents measuring scheduler claims"
                    );
                }
                if leases.gaps.len() > 1 {
                    let mut forged = report.clone();
                    forged
                        .observations
                        .iter_mut()
                        .find(|o| o.category == Category::LeasesFences)
                        .unwrap()
                        .gaps
                        .reverse();
                    assert!(
                        forged.validate().is_err(),
                        "lease gaps have one canonical order"
                    );
                }
            }
        }
    }
}

#[test]
fn eligibility_read_budget_preserves_independent_counts_without_materializing_inputs() {
    for mode in [
        "definition_json",
        "name",
        "quarantine_id",
        "definition_rows",
        "quarantine_rows",
    ] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("PRAGMA foreign_keys=OFF;
                DROP TABLE main.automation_definitions;
                CREATE TABLE main.automation_definitions(id TEXT,name TEXT,status TEXT,definition_json TEXT,
                    revision INTEGER,definition_digest TEXT,lifecycle_state TEXT,tombstoned_at TEXT,updated_at TEXT);
                DROP TABLE main.automation_retry_state;
                CREATE TABLE main.automation_retry_state(automation_id TEXT,quarantined_at TEXT,consecutive_exhaustions INTEGER);
                INSERT INTO main.automation_definitions VALUES('one','one','INACTIVE','{}',1,'digest','ready',NULL,'date');
                INSERT INTO main.automation_occurrences(id,automation_id,scheduled_for,state,created_at,updated_at)
                    VALUES('planned','one','2026-10-18T00:00:00.000Z','planned','date','date');").unwrap();
            if mode == "definition_json" || mode == "name" {
                conn.execute(
                    &format!("UPDATE main.automation_definitions SET {mode}=?1"),
                    [format!("PRIVATE_BUDGET_{}", "x".repeat(1_048_577))],
                )
                .unwrap();
            } else if mode == "quarantine_id" {
                conn.execute(
                    "INSERT INTO main.automation_retry_state VALUES(?1,'date',0)",
                    [format!("PRIVATE_BUDGET_{}", "x".repeat(1_048_577))],
                )
                .unwrap();
            } else if mode == "definition_rows" {
                conn.execute_batch("WITH RECURSIVE n(i) AS (VALUES(2) UNION ALL SELECT i+1 FROM n WHERE i<257)
                    INSERT INTO main.automation_definitions SELECT 'id-'||i,'name','INACTIVE','{}',1,'digest','ready',NULL,'date' FROM n;").unwrap();
            } else {
                conn.execute_batch(
                    "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<257)
                    INSERT INTO main.automation_retry_state SELECT 'id-'||i,'date',0 FROM n;",
                )
                .unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_definitions AS SELECT * FROM main.automation_definitions WHERE 0;
                    CREATE TEMP TABLE automation_retry_state AS SELECT * FROM main.automation_retry_state WHERE 0;").unwrap();
            }
            conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            let before = conn.total_changes();
            let now = DateTime::parse_from_rfc3339("2026-10-18T00:01:00Z")
                .unwrap()
                .with_timezone(&Utc);
            let report = inspect(&conn, now)
                .expect("oversized eligibility inputs must preserve independent observations");
            let queue = report
                .observations
                .iter()
                .find(|o| o.category == Category::Queue)
                .unwrap();
            assert_eq!(queue.facts[&Fact::PlannedOccurrences], 1);
            assert_eq!(queue.facts[&Fact::DuePlannedOccurrences], 1);
            assert!(
                !queue.facts.contains_key(&Fact::EligibleOccurrences),
                "over-budget eligibility must stay unknown: {mode}"
            );
            assert_eq!(
                serde_json::to_value(&queue.gaps).unwrap(),
                serde_json::json!(["eligibility_read_budget_exceeded"])
            );
            assert!(!serde_json::to_string(&report)
                .unwrap()
                .contains("PRIVATE_BUDGET"));
            assert_eq!(conn.total_changes(), before);
            report.validate().unwrap();
            let mut forged = report.clone();
            forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::Queue)
                .unwrap()
                .facts
                .insert(Fact::EligibleOccurrences, 0);
            assert!(
                forged.validate().is_err(),
                "budget refusal cannot become a measured zero"
            );
        }
    }
}

#[test]
fn eligibility_input_budget_enforces_exact_byte_and_row_boundaries_in_main() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE main.automation_definitions(id,name,status,definition_json,definition_digest,lifecycle_state,tombstoned_at,updated_at);
        CREATE TABLE main.automation_missions(automation_id);
        CREATE TABLE main.automation_retry_state(automation_id,quarantined_at);
        INSERT INTO main.automation_definitions VALUES('i','n','s','j','d','l',NULL,'u');
        CREATE TEMP TABLE automation_definitions AS SELECT * FROM main.automation_definitions WHERE 0;
        CREATE TEMP TABLE automation_retry_state AS SELECT * FROM main.automation_retry_state WHERE 0;").unwrap();
    // Six one-byte non-prompt fields plus the prompt reach the exact combined
    // input budget. Character counts would miss multibyte and embedded-NUL data.
    let bytes = ELIGIBILITY_INPUT_BYTES as usize;
    for (value, expected) in [
        ("x".repeat(bytes - 6), true),
        ("x".repeat(bytes - 5), false),
        ("é".repeat(bytes / 2), false),
        (format!("\0{}", "x".repeat(bytes)), false),
    ] {
        conn.execute(
            "UPDATE main.automation_definitions SET definition_json=?1",
            [value],
        )
        .unwrap();
        let before = conn.total_changes();
        assert_eq!(eligibility_inputs_within_budget(&conn).unwrap(), expected);
        assert_eq!(conn.total_changes(), before);
    }
    conn.execute(
        "UPDATE main.automation_definitions SET definition_json='j'",
        [],
    )
    .unwrap();
    // A huge TEMP shadow must not consume the main reader's budget.
    conn.execute(
        "INSERT INTO temp.automation_definitions(id) VALUES(?1)",
        ["x".repeat(bytes + 1)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO temp.automation_retry_state VALUES(?1,'q')",
        ["x".repeat(bytes + 1)],
    )
    .unwrap();
    assert!(eligibility_inputs_within_budget(&conn).unwrap());
    for table in ["automation_definitions", "automation_retry_state"] {
        conn.execute_batch(
            "DELETE FROM main.automation_definitions; DELETE FROM main.automation_retry_state;",
        )
        .unwrap();
        let insert = if table == "automation_definitions" {
            "INSERT INTO main.automation_definitions(id) VALUES(?1)"
        } else {
            "INSERT INTO main.automation_retry_state VALUES(?1,'q')"
        };
        for index in 0..ELIGIBILITY_INPUT_ROWS {
            conn.execute(insert, [format!("id-{index}")]).unwrap();
        }
        assert!(
            eligibility_inputs_within_budget(&conn).unwrap(),
            "exact row budget: {table}"
        );
        conn.execute(insert, ["overflow"]).unwrap();
        assert!(
            !eligibility_inputs_within_budget(&conn).unwrap(),
            "overflow row: {table}"
        );
    }
    conn.execute_batch(
        "DELETE FROM main.automation_definitions; DELETE FROM main.automation_retry_state;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO main.automation_definitions(id,tombstoned_at) VALUES(?1,'gone')",
        ["x".repeat(bytes + 1)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO main.automation_definitions(id,name) VALUES('mission',?1)",
        ["x".repeat(bytes + 1)],
    )
    .unwrap();
    conn.execute_batch("INSERT INTO main.automation_missions VALUES('mission');")
        .unwrap();
    assert!(
        eligibility_inputs_within_budget(&conn).unwrap(),
        "canonical reader excludes tombstones and missions"
    );
}

#[test]
fn malformed_active_definitions_cannot_be_reported_as_an_empty_or_partial_eligible_queue() {
    for has_valid in [false, true] {
        for invalid in ["json", "validation", "durable"] {
            for scope in ["active", "inactive", "quarantined"] {
                for shadow in [false, true] {
                    let (_root, conn) = fixture();
                    let mut bad_json = serde_json::Value::Null;
                    for id in ["valid-routine", "PRIVATE_BAD_ROUTINE"] {
                        if id == "valid-routine" && !has_valid {
                            continue;
                        }
                        let value = serde_json::json!({"schemaVersion":1,"id":id,"name":id,"status":"ACTIVE",
                        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid",
                        "timeoutMinutes":30,"runtime":"coven-code","prompt":"PRIVATE_DEFINITION_PROMPT"});
                        let definition =
                            crate::automations::definition::RoutineDefinition::from_json(&value)
                                .unwrap();
                        crate::automations::store::insert_definition(&conn, &definition).unwrap();
                        conn.execute("INSERT INTO main.automation_occurrences(id,automation_id,scheduled_for,created_at,updated_at)
                        VALUES(?1,?1,'2026-10-18T00:00:00.000Z','date','date')",[id]).unwrap();
                        if id == "PRIVATE_BAD_ROUTINE" {
                            bad_json = value;
                        }
                    }
                    let bad_text = match invalid {
                        "json" => "PRIVATE_MALFORMED_JSON".to_string(),
                        "validation" => {
                            bad_json["prompt"] = serde_json::json!("");
                            serde_json::to_string(&bad_json).unwrap()
                        }
                        "durable" => {
                            bad_json["timezone"] = serde_json::json!("local");
                            serde_json::to_string(&bad_json).unwrap()
                        }
                        _ => unreachable!(),
                    };
                    conn.execute("UPDATE main.automation_definitions SET definition_json=?1 WHERE id='PRIVATE_BAD_ROUTINE'",[bad_text]).unwrap();
                    match scope {
                        "inactive" => {
                            conn.execute("UPDATE main.automation_definitions SET status='PAUSED' WHERE id='PRIVATE_BAD_ROUTINE'",[]).unwrap();
                        }
                        "quarantined" => {
                            conn.execute("INSERT INTO main.automation_retry_state(automation_id,quarantined_at,updated_at,failure_class,reason) VALUES('PRIVATE_BAD_ROUTINE','date','date','failed','PRIVATE_QUARANTINE_REASON')",[]).unwrap();
                        }
                        _ => {}
                    }
                    if shadow {
                        conn.execute_batch("CREATE TEMP TABLE automation_definitions AS SELECT * FROM main.automation_definitions;
                        DELETE FROM temp.automation_definitions WHERE id='PRIVATE_BAD_ROUTINE';
                        CREATE TEMP TABLE automation_retry_state AS SELECT * FROM main.automation_retry_state;
                        DELETE FROM temp.automation_retry_state;").unwrap();
                    }
                    let before = conn.total_changes();
                    let now = DateTime::parse_from_rfc3339("2026-10-18T00:01:00Z")
                        .unwrap()
                        .with_timezone(&Utc);
                    let report = inspect(&conn, now)
                        .expect("invalid definition must retain independently readable categories");
                    let queue = report
                        .observations
                        .iter()
                        .find(|o| o.category == Category::Queue)
                        .unwrap();
                    assert_eq!(
                        queue.facts[&Fact::PlannedOccurrences],
                        1 + u64::from(has_valid)
                    );
                    assert_eq!(
                        queue.facts[&Fact::DuePlannedOccurrences],
                        1 + u64::from(has_valid)
                    );
                    if scope == "active" {
                        assert!(
                            !queue.facts.contains_key(&Fact::EligibleOccurrences),
                            "invalid active definition cannot become a partial count: {invalid}"
                        );
                        assert_eq!(queue.gaps, vec![Gap::InvalidPersistedMetadata]);
                        let mut forged = report.clone();
                        forged
                            .observations
                            .iter_mut()
                            .find(|o| o.category == Category::Queue)
                            .unwrap()
                            .facts
                            .insert(Fact::EligibleOccurrences, 1);
                        assert!(forged.validate().is_err());
                    } else {
                        assert_eq!(
                            queue.facts[&Fact::EligibleOccurrences],
                            u64::from(has_valid)
                        );
                        assert_eq!(queue.gaps, vec![Gap::RuntimeAdmissionNotProven]);
                    }
                    assert_eq!(conn.total_changes(), before);
                    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                    report.validate().unwrap();
                }
            }
        }
    }
}

#[test]
fn invalid_occurrence_states_cannot_be_reported_as_empty_or_partial_queue_counts() {
    use rusqlite::types::Value;
    for bad in [
        Value::Null,
        Value::Integer(1),
        Value::Real(1.5),
        Value::Blob(b"planned".to_vec()),
        Value::Text("PLANNED".into()),
        Value::Text("planned\0PRIVATE_STATE".into()),
        Value::Text(format!("PRIVATE_STATE{}", "x".repeat(1024))),
        Value::Text("PRIVATE_UNKNOWN_STATE".into()),
        Value::Blob(vec![0xff]),
    ] {
        for companion in [false, true] {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE main.automation_occurrences;
                    CREATE TABLE main.automation_occurrences(id TEXT PRIMARY KEY,automation_id TEXT,scheduled_for TEXT,state,
                        lease_owner TEXT,lease_expires_at TEXT,scheduler_generation INTEGER);
                    INSERT INTO main.automation_occurrences VALUES('bad','routine','2026-10-18T00:00:00.000Z',NULL,
                        'PRIVATE_OWNER','2026-10-18T00:02:00.000Z',1);").unwrap();
                conn.execute(
                    if bad == Value::Blob(vec![0xff]) {
                        "UPDATE main.automation_occurrences SET state=CAST(?1 AS TEXT)"
                    } else {
                        "UPDATE main.automation_occurrences SET state=?1"
                    },
                    [bad.clone()],
                )
                .unwrap();
                if companion {
                    conn.execute_batch("INSERT INTO main.automation_occurrences VALUES('valid','routine','2026-10-18T00:00:00.000Z','planned',NULL,NULL,NULL);").unwrap();
                }
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_occurrences AS SELECT * FROM main.automation_occurrences;
                    UPDATE temp.automation_occurrences SET state='planned';").unwrap();
                }
                conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
                let before = conn.total_changes();
                let now = DateTime::parse_from_rfc3339("2026-10-18T00:01:00Z")
                    .unwrap()
                    .with_timezone(&Utc);
                let report = inspect(&conn, now)
                    .expect("invalid occurrence state must preserve independent categories");
                let queue = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::Queue)
                    .unwrap();
                assert!(
                    queue.facts.is_empty(),
                    "malformed state cannot become a measured empty/partial queue"
                );
                assert_eq!(
                    serde_json::to_value(&queue.gaps).unwrap(),
                    serde_json::json!(["invalid_occurrence_state"])
                );
                let leases = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::LeasesFences)
                    .unwrap();
                assert_eq!(leases.facts[&Fact::LeaseBearingOccurrences], 1);
                assert_eq!(leases.facts[&Fact::ActiveLeaseOccurrences], 1);
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                report.validate().unwrap();
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::Queue)
                    .unwrap()
                    .facts
                    .insert(Fact::PlannedOccurrences, 0);
                assert!(
                    forged.validate().is_err(),
                    "invalid queue state cannot acquire a fabricated zero"
                );
            }
        }
    }
}

#[test]
fn all_canonical_occurrence_states_remain_readable_despite_invalid_temp_shadows() {
    use crate::automations::contract::types::OccurrenceState::*;
    let (_root, conn) = fixture();
    for (index, state) in [
        Planned,
        Eligible,
        Claimed,
        Dispatching,
        Running,
        Recovering,
        RecoveryRequired,
        Succeeded,
        Failed,
        Cancelled,
        TimedOut,
        Skipped,
        Superseded,
    ]
    .into_iter()
    .enumerate()
    {
        let state_name = serde_json::to_value(state)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        conn.execute("INSERT INTO main.automation_occurrences(id,automation_id,scheduled_for,state,created_at,updated_at)
            VALUES(?1,?1,'2026-10-18T00:00:00.000Z',?2,'date','date')",rusqlite::params![format!("state-{index}"),state_name]).unwrap();
    }
    conn.execute_batch(
        "CREATE TEMP TABLE automation_occurrences AS SELECT * FROM main.automation_occurrences;
        UPDATE temp.automation_occurrences SET state='PRIVATE_INVALID_SHADOW';",
    )
    .unwrap();
    let before = conn.total_changes();
    let now = DateTime::parse_from_rfc3339("2026-10-18T00:01:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let report = inspect(&conn, now).unwrap();
    let queue = report
        .observations
        .iter()
        .find(|o| o.category == Category::Queue)
        .unwrap();
    for fact in [
        Fact::PlannedOccurrences,
        Fact::ClaimedOccurrences,
        Fact::RunningOccurrences,
        Fact::RecoveryRequiredOccurrences,
        Fact::DuePlannedOccurrences,
    ] {
        assert_eq!(queue.facts[&fact], 1);
    }
    assert_eq!(queue.facts[&Fact::EligibleOccurrences], 0);
    assert_eq!(queue.gaps, vec![Gap::RuntimeAdmissionNotProven]);
    assert_eq!(conn.total_changes(), before);
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    report.validate().unwrap();
}

#[test]
fn checkpoint_expiry_observations_are_bounded_private_partitioned_and_read_only() {
    use rusqlite::types::Value;
    for shadow in [false, true] {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TABLE main.automation_event_checkpoints;
            CREATE TABLE main.automation_event_checkpoints(checkpoint TEXT PRIMARY KEY,expires_at);").unwrap();
        for (index, expiry) in [
            Value::Text("2026-10-18T00:00:03Z".into()),
            Value::Text("2026-10-18T00:00:02Z".into()),
            Value::Text("2026-10-18T01:00:01+01:00".into()),
            Value::Text("PRIVATE_INVALID_EXPIRY".into()),
            Value::Text("2026-10-18T00:00:03Z\0PRIVATE".into()),
            Value::Text("x".repeat(1024)),
            Value::Null,
            Value::Integer(1),
            Value::Blob(vec![0xff]),
        ]
        .into_iter()
        .enumerate()
        {
            conn.execute(
                if expiry == Value::Blob(vec![0xff]) {
                    "INSERT INTO main.automation_event_checkpoints VALUES(?1,CAST(?2 AS TEXT))"
                } else {
                    "INSERT INTO main.automation_event_checkpoints VALUES(?1,?2)"
                },
                rusqlite::params![format!("PRIVATE_CHECKPOINT_{index}"), expiry],
            )
            .unwrap();
        }
        if shadow {
            conn.execute_batch("CREATE TEMP TABLE automation_event_checkpoints(checkpoint TEXT,expires_at TEXT);
            INSERT INTO temp.automation_event_checkpoints VALUES('PRIVATE_SHADOW','2099-01-01T00:00:00Z');").unwrap();
        }
        let before = conn.total_changes();
        let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02Z")
            .unwrap()
            .with_timezone(&Utc);
        let report = inspect(&conn, now).unwrap();
        let event = report
            .observations
            .iter()
            .find(|o| o.category == Category::EventChangefeed)
            .unwrap();
        let facts = serde_json::to_value(&event.facts).unwrap();
        for (key, value) in [
            ("stored_event_checkpoints", 9),
            ("unexpired_event_checkpoints", 1),
            ("expired_event_checkpoints", 2),
            ("invalid_event_checkpoint_expiries", 6),
        ] {
            assert_eq!(
                facts[key],
                serde_json::json!(value),
                "checkpoint expiry fact: {key}"
            );
        }
        assert_eq!(event.gaps, vec![Gap::CheckpointCursorMetadataUnavailable]);
        assert_eq!(conn.total_changes(), before);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        report.validate().unwrap();
        let mut forged = serde_json::to_value(&report).unwrap();
        let event = forged["observations"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|o| o["category"] == "event_changefeed")
            .unwrap();
        event["facts"]["expired_event_checkpoints"] = serde_json::json!(3);
        assert!(
            serde_json::from_value::<Report>(forged)
                .unwrap()
                .validate()
                .is_err(),
            "checkpoint partitions must sum to total"
        );
    }
}

#[test]
fn checkpoint_sources_compose_with_each_missing_event_relation_without_inventing_facts() {
    for mode in ["present", "expiry_missing", "table_missing"] {
        for missing in 0..4 {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                conn.execute_batch("DROP TABLE main.automation_events; DROP TABLE main.automation_event_stream_heads;
                    DROP TABLE main.automation_event_checkpoints;").unwrap();
                for (bit, table, rows) in [
                    (1, "automation_events", 2),
                    (2, "automation_event_stream_heads", 1),
                ] {
                    if missing & bit == 0 {
                        conn.execute_batch(&format!(
                            "CREATE TABLE main.{table}(private_text TEXT);"
                        ))
                        .unwrap();
                        for _ in 0..rows {
                            conn.execute(
                                &format!("INSERT INTO main.{table} VALUES('PRIVATE_MAIN')"),
                                [],
                            )
                            .unwrap();
                        }
                    }
                    if shadow {
                        conn.execute_batch(&format!("CREATE TEMP TABLE {table}(private_text TEXT);
                        INSERT INTO temp.{table} VALUES('PRIVATE_SHADOW'),('PRIVATE_SHADOW'),('PRIVATE_SHADOW');")).unwrap();
                    }
                }
                if mode != "table_missing" {
                    conn.execute_batch("CREATE TABLE main.automation_event_checkpoints(checkpoint TEXT PRIMARY KEY);
                        INSERT INTO main.automation_event_checkpoints VALUES('PRIVATE_ONE'),('PRIVATE_TWO');").unwrap();
                    if mode == "present" {
                        conn.execute_batch("ALTER TABLE main.automation_event_checkpoints ADD COLUMN expires_at TEXT;
                        UPDATE main.automation_event_checkpoints SET expires_at=CASE checkpoint WHEN 'PRIVATE_ONE' THEN '2026-10-18T00:00:03Z' ELSE '2026-10-18T00:00:01Z' END;").unwrap();
                    }
                }
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_event_checkpoints(checkpoint TEXT,expires_at TEXT);
                    INSERT INTO temp.automation_event_checkpoints VALUES('PRIVATE_SHADOW','2099-01-01T00:00:00Z');").unwrap();
                }
                let before = conn.total_changes();
                let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02Z")
                    .unwrap()
                    .with_timezone(&Utc);
                let report = inspect(&conn, now).unwrap();
                let event = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::EventChangefeed)
                    .unwrap();
                for (bit, fact, total) in [(1, Fact::StoredEvents, 2), (2, Fact::EventStreams, 1)] {
                    assert_eq!(
                        event.facts.get(&fact).copied(),
                        if missing & bit == 0 {
                            Some(total)
                        } else {
                            None
                        }
                    );
                }
                assert_eq!(
                    event.facts.get(&Fact::StoredEventCheckpoints).copied(),
                    if mode == "table_missing" {
                        None
                    } else {
                        Some(2)
                    }
                );
                for (fact, total) in [
                    (Fact::UnexpiredEventCheckpoints, 1),
                    (Fact::ExpiredEventCheckpoints, 1),
                    (Fact::InvalidEventCheckpointExpiries, 0),
                ] {
                    assert_eq!(
                        event.facts.get(&fact).copied(),
                        if mode == "present" { Some(total) } else { None }
                    );
                }
                let mut expected = Vec::new();
                if missing != 0 {
                    expected.push(Gap::EventStoreUnavailable);
                }
                if mode == "table_missing" {
                    expected.push(Gap::CheckpointStoreUnavailable);
                } else if mode == "expiry_missing" {
                    expected.push(Gap::CheckpointExpiryUnavailable);
                }
                if mode != "table_missing" {
                    expected.push(Gap::CheckpointCursorMetadataUnavailable);
                }
                expected.push(Gap::EventSequenceStoreUnavailable);
                if expected.is_empty() {
                    expected.push(Gap::SubscriberReplayNotObserved);
                }
                assert_eq!(event.gaps, expected);
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                report.validate().unwrap();
                let mut forged = report.clone();
                let observation = forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::EventChangefeed)
                    .unwrap();
                observation.facts.insert(
                    if mode == "table_missing" {
                        Fact::StoredEventCheckpoints
                    } else {
                        Fact::UnexpiredEventCheckpoints
                    },
                    0,
                );
                assert!(
                    forged.validate().is_err(),
                    "missing metadata cannot become zero, and partitions cannot disagree"
                );
                if event.gaps.len() > 1 {
                    let mut forged = report.clone();
                    forged
                        .observations
                        .iter_mut()
                        .find(|o| o.category == Category::EventChangefeed)
                        .unwrap()
                        .gaps
                        .reverse();
                    assert!(
                        forged.validate().is_err(),
                        "event gaps have one canonical order"
                    );
                }
            }
        }
    }
}

#[test]
fn checkpoint_retention_observations_follow_canonical_cursor_boundaries_privately() {
    for shadow in [false, true] {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TABLE main.automation_event_checkpoints; DROP TABLE main.automation_event_stream_heads;
            CREATE TABLE main.automation_event_stream_heads(stream_kind TEXT,stream_id TEXT,earliest_sequence,next_sequence);
            INSERT INTO main.automation_event_stream_heads VALUES('run','PRIVATE_STREAM',2,4),('run','PRIVATE_BAD_HEAD',5,4);
            CREATE TABLE main.automation_event_checkpoints(checkpoint TEXT,stream_kind,stream_id,after_sequence,expires_at TEXT);
            INSERT INTO main.automation_event_checkpoints VALUES
                ('PRIVATE_BEFORE','run','PRIVATE_STREAM',0,'2099-01-01T00:00:00Z'),
                ('PRIVATE_EDGE','run','PRIVATE_STREAM',1,'2099-01-01T00:00:00Z'),
                ('PRIVATE_FUTURE','run','PRIVATE_STREAM',4,'2099-01-01T00:00:00Z'),
                ('PRIVATE_NO_HEAD','run','PRIVATE_UNOBSERVED',0,'2099-01-01T00:00:00Z'),
                ('PRIVATE_BAD_HEAD','run','PRIVATE_BAD_HEAD',0,'2099-01-01T00:00:00Z'),
                ('PRIVATE_BAD_AFTER','run','PRIVATE_STREAM',-2,'2099-01-01T00:00:00Z'),
                ('PRIVATE_NULL_AFTER','run','PRIVATE_STREAM',NULL,'2099-01-01T00:00:00Z'),
                ('PRIVATE_KIND','PRIVATE_KIND','PRIVATE_STREAM',0,'2099-01-01T00:00:00Z'),
                ('PRIVATE_FEED','feed','PRIVATE_WRONG_FEED',0,'2099-01-01T00:00:00Z'),
                ('PRIVATE_GLOBAL','feed','all',-1,'2099-01-01T00:00:00Z'),
                ('PRIVATE_TEXT_AFTER','run','PRIVATE_STREAM','1','2099-01-01T00:00:00Z');").unwrap();
        if shadow {
            conn.execute_batch("CREATE TEMP TABLE automation_event_stream_heads AS SELECT * FROM main.automation_event_stream_heads;
            UPDATE temp.automation_event_stream_heads SET earliest_sequence=0,next_sequence=100;
            CREATE TEMP TABLE automation_event_checkpoints AS SELECT * FROM main.automation_event_checkpoints WHERE 0;").unwrap();
        }
        let before = conn.total_changes();
        let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:02Z")
            .unwrap()
            .with_timezone(&Utc);
        let report = inspect(&conn, now).unwrap();
        let event = report
            .observations
            .iter()
            .find(|o| o.category == Category::EventChangefeed)
            .unwrap();
        let facts = serde_json::to_value(&event.facts).unwrap();
        for (key, value) in [
            ("invalid_checkpoint_cursor_metadata", 5),
            ("checkpoints_without_retention_boundary", 2),
            ("checkpoints_with_invalid_retention_boundary", 1),
            ("checkpoint_retention_comparisons", 3),
            ("checkpoints_before_retention", 1),
        ] {
            assert_eq!(
                facts[key],
                serde_json::json!(value),
                "retention observation {key}"
            );
        }
        assert_eq!(event.gaps, vec![Gap::EventSequenceMetadataInvalid]);
        assert_eq!(conn.total_changes(), before);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        report.validate().unwrap();
        let mut forged = serde_json::to_value(&report).unwrap();
        let event = forged["observations"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|o| o["category"] == "event_changefeed")
            .unwrap();
        event["facts"]["checkpoints_before_retention"] = serde_json::json!(4);
        assert!(serde_json::from_value::<Report>(forged)
            .unwrap()
            .validate()
            .is_err());
    }
}

#[test]
fn checkpoint_cursor_identity_validation_is_bounded_and_canonical() {
    use rusqlite::types::Value;
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE main.automation_event_checkpoints(stream_kind,stream_id,after_sequence);
        CREATE TABLE main.automation_event_stream_heads(stream_kind,stream_id,earliest_sequence,next_sequence);").unwrap();
    for (value, invalid) in [
        (Value::Blob(vec![0xff]), true),
        (Value::Null, true),
        (Value::Integer(1), true),
        (Value::Blob(b"PRIVATE".to_vec()), true),
        (Value::Text("x".repeat(321)), true),
        (Value::Text("🦉".repeat(321)), true),
        (Value::Text("🦉".repeat(320)), false),
        (Value::Text("x".repeat(320)), false),
        (Value::Text("PRIVATE\0ID".into()), false),
    ] {
        conn.execute("DELETE FROM main.automation_event_checkpoints", [])
            .unwrap();
        conn.execute(
            if value == Value::Blob(vec![0xff]) {
                "INSERT INTO main.automation_event_checkpoints VALUES('run',CAST(?1 AS TEXT),0)"
            } else {
                "INSERT INTO main.automation_event_checkpoints VALUES('run',?1,0)"
            },
            [value],
        )
        .unwrap();
        let before = conn.total_changes();
        let counts = checkpoint_retention_counts(&conn, true).unwrap();
        assert_eq!(
            counts[0],
            u64::from(invalid),
            "identity validity must match canonical bounded string metadata"
        );
        assert_eq!(counts[1], u64::from(!invalid));
        assert_eq!(&counts[2..], &[0, 0, 0]);
        assert_eq!(conn.total_changes(), before);
    }
}

#[test]
fn checkpoint_cursor_and_retention_prerequisites_preserve_independent_evidence() {
    use rusqlite::types::Value;
    for (source, missing) in [
        ("none", "none"),
        ("checkpoint", "stream_kind"),
        ("checkpoint", "stream_id"),
        ("checkpoint", "after_sequence"),
        ("checkpoint", "table"),
        ("head", "stream_kind"),
        ("head", "stream_id"),
        ("head", "earliest_sequence"),
        ("head", "next_sequence"),
        ("head", "table"),
    ] {
        for expiry_missing in [false, true] {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                conn.execute_batch("DROP TABLE main.automation_event_checkpoints; DROP TABLE main.automation_event_stream_heads;").unwrap();
                if !(source == "head" && missing == "table") {
                    let columns: Vec<_> = [
                        "stream_kind",
                        "stream_id",
                        "earliest_sequence",
                        "next_sequence",
                    ]
                    .into_iter()
                    .filter(|name| !(source == "head" && *name == missing))
                    .collect();
                    conn.execute_batch(&format!(
                        "CREATE TABLE main.automation_event_stream_heads({});",
                        columns.join(",")
                    ))
                    .unwrap();
                    let values: Vec<Value> = columns
                        .iter()
                        .map(|name| match *name {
                            "stream_kind" => Value::Text("run".into()),
                            "stream_id" => Value::Text("PRIVATE_STREAM".into()),
                            "earliest_sequence" => Value::Integer(2),
                            _ => Value::Integer(4),
                        })
                        .collect();
                    conn.execute(
                        &format!(
                            "INSERT INTO main.automation_event_stream_heads VALUES({})",
                            vec!["?"; values.len()].join(",")
                        ),
                        rusqlite::params_from_iter(values),
                    )
                    .unwrap();
                }
                let table_available = !(source == "checkpoint" && missing == "table");
                let cursor_available = table_available && source != "checkpoint";
                let retention_available = cursor_available && source != "head";
                if table_available {
                    let columns: Vec<_> = [
                        "checkpoint",
                        "stream_kind",
                        "stream_id",
                        "after_sequence",
                        "expires_at",
                    ]
                    .into_iter()
                    .filter(|name| {
                        !(source == "checkpoint" && *name == missing)
                            && !(expiry_missing && *name == "expires_at")
                    })
                    .collect();
                    conn.execute_batch(&format!(
                        "CREATE TABLE main.automation_event_checkpoints({});",
                        columns.join(",")
                    ))
                    .unwrap();
                    for after in [0, -2] {
                        let values: Vec<Value> = columns
                            .iter()
                            .map(|name| match *name {
                                "checkpoint" => Value::Text(format!("PRIVATE_TOKEN_{after}")),
                                "stream_kind" => Value::Text("run".into()),
                                "stream_id" => Value::Text("PRIVATE_STREAM".into()),
                                "after_sequence" => Value::Integer(after),
                                _ => Value::Text("2099-01-01T00:00:00Z".into()),
                            })
                            .collect();
                        conn.execute(
                            &format!(
                                "INSERT INTO main.automation_event_checkpoints VALUES({})",
                                vec!["?"; values.len()].join(",")
                            ),
                            rusqlite::params_from_iter(values),
                        )
                        .unwrap();
                    }
                }
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_event_checkpoints(stream_kind,stream_id,after_sequence,expires_at);
                    INSERT INTO temp.automation_event_checkpoints VALUES('run','PRIVATE_STREAM',0,'2099-01-01T00:00:00Z');
                    CREATE TEMP TABLE automation_event_stream_heads(stream_kind,stream_id,earliest_sequence,next_sequence);
                    INSERT INTO temp.automation_event_stream_heads VALUES('run','PRIVATE_STREAM',0,1);").unwrap();
                }
                let before = conn.total_changes();
                let report = inspect(&conn, Utc::now()).unwrap();
                let event = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::EventChangefeed)
                    .unwrap();
                assert_eq!(
                    event.facts.get(&Fact::StoredEventCheckpoints).copied(),
                    table_available.then_some(2)
                );
                assert_eq!(
                    event.facts.get(&Fact::UnexpiredEventCheckpoints).copied(),
                    (table_available && !expiry_missing).then_some(2)
                );
                assert_eq!(
                    event
                        .facts
                        .get(&Fact::InvalidCheckpointCursorMetadata)
                        .copied(),
                    cursor_available.then_some(1)
                );
                for (fact, count) in [
                    (Fact::CheckpointsWithoutRetentionBoundary, 0),
                    (Fact::CheckpointsWithInvalidRetentionBoundary, 0),
                    (Fact::CheckpointRetentionComparisons, 1),
                    (Fact::CheckpointsBeforeRetention, 1),
                ] {
                    assert_eq!(
                        event.facts.get(&fact).copied(),
                        retention_available.then_some(count)
                    );
                }
                let mut gaps = Vec::new();
                if source == "head" && missing == "table" {
                    gaps.push(Gap::EventStoreUnavailable);
                }
                if !table_available {
                    gaps.push(Gap::CheckpointStoreUnavailable);
                } else {
                    if expiry_missing {
                        gaps.push(Gap::CheckpointExpiryUnavailable);
                    }
                    if !cursor_available {
                        gaps.push(Gap::CheckpointCursorMetadataUnavailable);
                    } else if !retention_available {
                        gaps.push(Gap::CheckpointRetentionStoreUnavailable);
                    }
                }
                if source == "head" {
                    gaps.push(Gap::EventSequenceStoreUnavailable);
                }
                if gaps.is_empty() {
                    gaps.push(Gap::SubscriberReplayNotObserved);
                }
                assert_eq!(event.gaps, gaps);
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                report.validate().unwrap();
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::EventChangefeed)
                    .unwrap()
                    .facts
                    .insert(
                        if cursor_available {
                            Fact::CheckpointRetentionComparisons
                        } else {
                            Fact::InvalidCheckpointCursorMetadata
                        },
                        0,
                    );
                assert!(
                    forged.validate().is_err(),
                    "unknown evidence cannot become zero or an inconsistent partition"
                );
                if event.gaps.len() > 1 {
                    let mut forged = report.clone();
                    forged
                        .observations
                        .iter_mut()
                        .find(|o| o.category == Category::EventChangefeed)
                        .unwrap()
                        .gaps
                        .reverse();
                    assert!(forged.validate().is_err());
                }
            }
        }
    }
}

#[test]
fn ambiguous_stream_heads_do_not_multiply_checkpoints_or_hide_independent_observations() {
    for mode in [
        "identical",
        "conflicting",
        "invalid_first",
        "invalid_second",
        "many",
    ] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_event_stream_heads; DROP TABLE main.automation_event_checkpoints;
                CREATE TABLE main.automation_event_stream_heads(stream_kind TEXT,stream_id TEXT,earliest_sequence,next_sequence);
                CREATE TABLE main.automation_event_checkpoints(checkpoint TEXT,stream_kind TEXT,stream_id TEXT,after_sequence,expires_at TEXT);
                INSERT INTO main.automation_event_checkpoints VALUES
                    ('PRIVATE_VALID','run','PRIVATE_DUP',0,'2099-01-01T00:00:00Z'),
                    ('PRIVATE_INVALID','run','PRIVATE_DUP',-2,'2099-01-01T00:00:00Z'),
                    ('PRIVATE_UNIQUE','run','PRIVATE_UNIQUE',0,'2099-01-01T00:00:00Z'),
                    ('PRIVATE_MISSING','run','PRIVATE_MISSING',0,'2099-01-01T00:00:00Z');
                INSERT INTO main.automation_event_stream_heads VALUES('run','PRIVATE_UNIQUE',0,1);").unwrap();
            let boundaries = match mode {
                "identical" => vec![rusqlite::types::Value::Integer(2); 2],
                "conflicting" => vec![
                    rusqlite::types::Value::Integer(2),
                    rusqlite::types::Value::Integer(0),
                ],
                "invalid_first" => vec![
                    rusqlite::types::Value::Text("PRIVATE_BAD_BOUNDARY".into()),
                    rusqlite::types::Value::Integer(2),
                ],
                "invalid_second" => vec![
                    rusqlite::types::Value::Integer(2),
                    rusqlite::types::Value::Text("PRIVATE_BAD_BOUNDARY".into()),
                ],
                _ => vec![rusqlite::types::Value::Integer(2); 4],
            };
            for earliest in boundaries {
                conn.execute("INSERT INTO main.automation_event_stream_heads VALUES('run','PRIVATE_DUP',?1,4)",[earliest]).unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_event_stream_heads(stream_kind TEXT,stream_id TEXT,earliest_sequence,next_sequence);
                INSERT INTO temp.automation_event_stream_heads VALUES('run','PRIVATE_DUP',0,100);
                CREATE TEMP TABLE automation_event_checkpoints AS SELECT * FROM main.automation_event_checkpoints WHERE 0;").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now())
                .expect("ambiguous head metadata must preserve independent observations");
            let event = report
                .observations
                .iter()
                .find(|o| o.category == Category::EventChangefeed)
                .unwrap();
            for (fact, count) in [
                (Fact::StoredEventCheckpoints, 4),
                (Fact::UnexpiredEventCheckpoints, 4),
                (Fact::InvalidCheckpointCursorMetadata, 1),
                (Fact::CheckpointsWithoutRetentionBoundary, 1),
                (Fact::CheckpointsWithInvalidRetentionBoundary, 1),
                (Fact::CheckpointRetentionComparisons, 1),
                (Fact::CheckpointsBeforeRetention, 0),
            ] {
                assert_eq!(
                    event.facts[&fact], count,
                    "one observation per checkpoint despite duplicate heads: {mode}"
                );
            }
            assert!(report
                .observations
                .iter()
                .find(|o| o.category == Category::Queue)
                .unwrap()
                .facts
                .contains_key(&Fact::EligibleOccurrences));
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
        }
    }
}

#[test]
fn bounded_event_sequence_checks_detect_gaps_future_rows_and_orphans_without_payloads() {
    for shadow in [false, true] {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TABLE main.automation_events; DROP TABLE main.automation_event_stream_heads;
            CREATE TABLE main.automation_event_stream_heads(stream_kind,stream_id,earliest_sequence,next_sequence);
            CREATE TABLE main.automation_events(stream_kind,stream_id,sequence);
            INSERT INTO main.automation_event_stream_heads VALUES
                ('run','PRIVATE_COMPLETE',0,3),('run','PRIVATE_RETAINED',1,3),
                ('run','PRIVATE_GAP',0,3),('run','PRIVATE_FUTURE',0,1),('run','PRIVATE_EMPTY',0,0);
            INSERT INTO main.automation_events VALUES
                ('run','PRIVATE_COMPLETE',0),('run','PRIVATE_COMPLETE',1),('run','PRIVATE_COMPLETE',2),
                ('run','PRIVATE_RETAINED',0),('run','PRIVATE_RETAINED',1),('run','PRIVATE_RETAINED',2),
                ('run','PRIVATE_GAP',0),('run','PRIVATE_GAP',2),
                ('run','PRIVATE_FUTURE',0),('run','PRIVATE_FUTURE',1),('run','PRIVATE_ORPHAN',0);").unwrap();
        if shadow {
            conn.execute_batch("CREATE TEMP TABLE automation_events AS SELECT * FROM main.automation_events WHERE 0;
            CREATE TEMP TABLE automation_event_stream_heads AS SELECT * FROM main.automation_event_stream_heads;
            UPDATE temp.automation_event_stream_heads SET next_sequence=100;").unwrap();
        }
        let before = conn.total_changes();
        let report = inspect(&conn, Utc::now()).unwrap();
        let event = report
            .observations
            .iter()
            .find(|o| o.category == Category::EventChangefeed)
            .unwrap();
        let facts = serde_json::to_value(&event.facts).unwrap();
        for (key, count) in [
            ("event_sequence_streams_checked", 5),
            ("event_sequence_stream_mismatches", 2),
            ("event_sequence_rows_without_head", 1),
            ("event_sequence_rows_before_retention", 1),
            ("event_sequence_rows_beyond_head", 1),
        ] {
            assert_eq!(
                facts[key],
                serde_json::json!(count),
                "complete bounded sequence observation: {key}"
            );
        }
        assert_eq!(event.gaps, vec![Gap::SubscriberReplayNotObserved]);
        assert_eq!(conn.total_changes(), before);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        report.validate().unwrap();
        let mut forged = serde_json::to_value(&report).unwrap();
        let event = forged["observations"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|o| o["category"] == "event_changefeed")
            .unwrap();
        event["facts"]["event_sequence_stream_mismatches"] = serde_json::json!(6);
        assert!(serde_json::from_value::<Report>(forged)
            .unwrap()
            .validate()
            .is_err());
    }
}

#[test]
fn sequence_report_rejects_future_rows_without_a_mismatched_stream() {
    let (_root, conn) = fixture();
    conn.execute_batch("DROP TABLE main.automation_events; DROP TABLE main.automation_event_stream_heads;
        CREATE TABLE main.automation_event_stream_heads(stream_kind,stream_id,earliest_sequence,next_sequence);
        CREATE TABLE main.automation_events(stream_kind,stream_id,sequence);
        INSERT INTO main.automation_event_stream_heads VALUES('run','PRIVATE_STREAM',0,0);
        INSERT INTO main.automation_events VALUES('run','PRIVATE_STREAM',0);").unwrap();
    let mut report = inspect(&conn, Utc::now()).unwrap();
    let event = report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::EventChangefeed)
        .unwrap();
    assert_eq!(event.facts[&Fact::EventSequenceRowsBeyondHead], 1);
    assert_eq!(event.facts[&Fact::EventSequenceStreamMismatches], 1);
    event.facts.insert(Fact::EventSequenceStreamMismatches, 0);
    assert!(
        report.validate().is_err(),
        "future event rows necessarily imply a mismatched stream"
    );
}

#[test]
fn sequence_report_rejects_complete_observations_beyond_scan_caps() {
    for (fact, value) in [
        (Fact::EventStreams, sequences::MAX_HEADS as u64 + 1),
        (Fact::StoredEvents, sequences::MAX_EVENTS as u64 + 1),
    ] {
        let (_root, conn) = fixture();
        let mut report = inspect(&conn, Utc::now()).unwrap();
        let event = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::EventChangefeed)
            .unwrap();
        event.facts.insert(fact, value);
        if fact == Fact::EventStreams {
            event.facts.insert(Fact::EventSequenceStreamsChecked, value);
        }
        assert!(
            report.validate().is_err(),
            "complete sequence observations must respect scan caps: {fact:?}"
        );
    }
}

fn sequence_metadata_fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE main.automation_event_stream_heads(stream_kind,stream_id,earliest_sequence,next_sequence);
        CREATE TABLE main.automation_events(stream_kind,stream_id,sequence);").unwrap();
    conn
}

#[test]
fn sequence_scan_refuses_malformed_and_duplicate_metadata_without_partial_counts() {
    for mutation in [
        "UPDATE main.automation_event_stream_heads SET earliest_sequence=NULL",
        "UPDATE main.automation_event_stream_heads SET next_sequence=-1",
        "UPDATE main.automation_event_stream_heads SET next_sequence=9007199254740992",
        "UPDATE main.automation_event_stream_heads SET next_sequence=1.5",
        "UPDATE main.automation_event_stream_heads SET next_sequence='1'",
        "UPDATE main.automation_event_stream_heads SET earliest_sequence=2",
        "UPDATE main.automation_event_stream_heads SET stream_kind=NULL",
        "UPDATE main.automation_event_stream_heads SET stream_kind='PRIVATE_INVALID'",
        "UPDATE main.automation_event_stream_heads SET stream_kind='feed'",
        "UPDATE main.automation_event_stream_heads SET stream_id=CAST(x'ff' AS TEXT)",
        "UPDATE main.automation_event_stream_heads SET stream_id=printf('%0321d',1)",
        "INSERT INTO main.automation_event_stream_heads SELECT * FROM main.automation_event_stream_heads",
        "UPDATE main.automation_events SET sequence=NULL",
        "UPDATE main.automation_events SET sequence=-1",
        "UPDATE main.automation_events SET sequence=1.5",
        "UPDATE main.automation_events SET sequence='0'",
        "UPDATE main.automation_events SET sequence=9007199254740992",
        "UPDATE main.automation_events SET stream_kind='PRIVATE_INVALID'",
        "UPDATE main.automation_events SET stream_kind='feed'",
        "UPDATE main.automation_events SET stream_id=CAST(x'ff' AS TEXT)",
        "UPDATE main.automation_events SET stream_id=printf('%0321d',1)",
        "INSERT INTO main.automation_events SELECT * FROM main.automation_events",
    ] {
        let conn=sequence_metadata_fixture();
        conn.execute_batch("INSERT INTO main.automation_event_stream_heads VALUES('run','PRIVATE_STREAM',0,1);
            INSERT INTO main.automation_events VALUES('run','PRIVATE_STREAM',0);").unwrap();
        conn.execute_batch(mutation).unwrap();
        let before=conn.total_changes();
        assert_eq!(sequences::inspect(&conn).unwrap(),sequences::Observation::InvalidMetadata);
        assert_eq!(conn.total_changes(),before);
    }
    let conn = sequence_metadata_fixture();
    let id = "🦉".repeat(320);
    conn.execute(
        "INSERT INTO main.automation_event_stream_heads VALUES('run',?1,0,1)",
        [&id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO main.automation_events VALUES('run',?1,0)",
        [&id],
    )
    .unwrap();
    assert!(
        matches!(sequences::inspect(&conn).unwrap(),sequences::Observation::Complete(stats) if stats.mismatches==0)
    );
}

#[test]
fn sequence_scan_enforces_exact_row_and_combined_byte_budgets() {
    let conn = sequence_metadata_fixture();
    conn.execute_batch(&format!(
        "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<{})
        INSERT INTO main.automation_event_stream_heads SELECT 'run','head-'||i,0,0 FROM n;",
        sequences::MAX_HEADS - 1
    ))
    .unwrap();
    assert!(
        matches!(sequences::inspect(&conn).unwrap(),sequences::Observation::Complete(stats) if stats.streams==sequences::MAX_HEADS as u64 && stats.mismatches==0)
    );
    conn.execute_batch(
        "INSERT INTO main.automation_event_stream_heads VALUES('run','overflow',0,0);",
    )
    .unwrap();
    assert_eq!(
        sequences::inspect(&conn).unwrap(),
        sequences::Observation::BudgetExceeded
    );
    conn.execute_batch(&format!(
        "DELETE FROM main.automation_event_stream_heads;
        INSERT INTO main.automation_event_stream_heads VALUES('run','events',0,{});
        WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<{})
        INSERT INTO main.automation_events SELECT 'run','events',i FROM n;",
        sequences::MAX_EVENTS,
        sequences::MAX_EVENTS - 1
    ))
    .unwrap();
    assert!(
        matches!(sequences::inspect(&conn).unwrap(),sequences::Observation::Complete(stats) if stats.mismatches==0)
    );
    conn.execute(
        "INSERT INTO main.automation_events VALUES('run','events',?1)",
        [sequences::MAX_EVENTS as i64],
    )
    .unwrap();
    conn.execute_batch(
        "UPDATE main.automation_event_stream_heads SET next_sequence=next_sequence+1;",
    )
    .unwrap();
    assert_eq!(
        sequences::inspect(&conn).unwrap(),
        sequences::Observation::BudgetExceeded
    );
    conn.execute_batch(
        "DELETE FROM main.automation_event_stream_heads; DELETE FROM main.automation_events;",
    )
    .unwrap();
    let width = 323; // three kind bytes and 320 legal ASCII ID bytes.
    let full = sequences::MAX_INPUT_BYTES / width;
    let remaining = sequences::MAX_INPUT_BYTES - full * width;
    conn.execute_batch(&format!(
        "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<{})
        INSERT INTO main.automation_events SELECT 'run',printf('%0320d',1),i FROM n;",
        full - 1
    ))
    .unwrap();
    conn.execute(
        "INSERT INTO main.automation_events VALUES('run',?1,0)",
        ["x".repeat(remaining - 3)],
    )
    .unwrap();
    assert!(
        matches!(sequences::inspect(&conn).unwrap(),sequences::Observation::Complete(stats) if stats.orphans==(full+1) as u64)
    );
    conn.execute_batch("INSERT INTO main.automation_events VALUES('run','x',0);")
        .unwrap();
    let before = conn.total_changes();
    assert_eq!(
        sequences::inspect(&conn).unwrap(),
        sequences::Observation::BudgetExceeded
    );
    assert_eq!(conn.total_changes(), before);
}

#[test]
fn sequence_prerequisites_cannot_be_supplied_by_temp_and_preserve_checkpoint_facts() {
    use rusqlite::types::Value;
    let mut cases = vec![("none", "none")];
    for &(table, columns) in sequences::PREREQUISITES {
        cases.push((table, "table"));
        for &column in columns {
            cases.push((table, column));
        }
    }
    for (missing_table, missing_column) in cases {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch(
                "DROP TABLE main.automation_events; DROP TABLE main.automation_event_stream_heads;",
            )
            .unwrap();
            for &(table, columns) in sequences::PREREQUISITES {
                if missing_table == table && missing_column == "table" {
                    continue;
                }
                let selected: Vec<_> = columns
                    .iter()
                    .copied()
                    .filter(|name| !(table == missing_table && *name == missing_column))
                    .collect();
                conn.execute_batch(&format!(
                    "CREATE TABLE main.{table}({});",
                    selected.join(",")
                ))
                .unwrap();
                let values: Vec<Value> = selected
                    .iter()
                    .map(|name| match *name {
                        "stream_kind" => Value::Text("run".into()),
                        "stream_id" => Value::Text("PRIVATE_STREAM".into()),
                        "next_sequence" => Value::Integer(1),
                        _ => Value::Integer(0),
                    })
                    .collect();
                conn.execute(
                    &format!(
                        "INSERT INTO main.{table} VALUES({})",
                        vec!["?"; values.len()].join(",")
                    ),
                    rusqlite::params_from_iter(values),
                )
                .unwrap();
            }
            if shadow {
                conn.execute_batch("CREATE TEMP TABLE automation_event_stream_heads(stream_kind,stream_id,earliest_sequence,next_sequence);
                INSERT INTO temp.automation_event_stream_heads VALUES('run','PRIVATE_STREAM',0,100);
                CREATE TEMP TABLE automation_events(stream_kind,stream_id,sequence);").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now()).unwrap();
            let event = report
                .observations
                .iter()
                .find(|o| o.category == Category::EventChangefeed)
                .unwrap();
            assert_eq!(event.facts[&Fact::StoredEventCheckpoints], 0);
            if missing_table == "none" {
                assert_eq!(event.facts[&Fact::EventSequenceStreamsChecked], 1);
                assert_eq!(event.facts[&Fact::EventSequenceStreamMismatches], 0);
            } else {
                for fact in [
                    Fact::EventSequenceStreamsChecked,
                    Fact::EventSequenceStreamMismatches,
                    Fact::EventSequenceRowsWithoutHead,
                    Fact::EventSequenceRowsBeforeRetention,
                    Fact::EventSequenceRowsBeyondHead,
                ] {
                    assert!(!event.facts.contains_key(&fact));
                }
                assert_eq!(event.gaps.last(), Some(&Gap::EventSequenceStoreUnavailable));
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::EventChangefeed)
                    .unwrap()
                    .facts
                    .insert(Fact::EventSequenceStreamsChecked, 0);
                assert!(forged.validate().is_err());
            }
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
        }
    }
}

#[test]
fn sequence_invalid_and_budget_reports_preserve_counts_and_checkpoint_scope() {
    for mode in ["invalid", "budget"] {
        for checkpoint_missing in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch("DROP TABLE main.automation_events; DROP TABLE main.automation_event_stream_heads;
                CREATE TABLE main.automation_events(stream_kind,stream_id,sequence);
                CREATE TABLE main.automation_event_stream_heads(stream_kind,stream_id,earliest_sequence,next_sequence);").unwrap();
            if mode == "invalid" {
                conn.execute_batch(
                    "INSERT INTO main.automation_event_stream_heads VALUES('run','head-0',2,1);",
                )
                .unwrap();
            } else {
                conn.execute_batch(&format!(
                    "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<{})
                INSERT INTO main.automation_event_stream_heads SELECT 'run','head-'||i,0,0 FROM n;",
                    sequences::MAX_HEADS
                ))
                .unwrap();
            }
            if checkpoint_missing {
                conn.execute_batch("DROP TABLE main.automation_event_checkpoints;")
                    .unwrap();
            } else {
                conn.execute_batch("INSERT INTO main.automation_event_checkpoints(checkpoint,stream_kind,stream_id,after_sequence,issued_at,expires_at)
                VALUES('PRIVATE_TOKEN','run','head-0',-1,'2026-10-18T00:00:00Z','2099-01-01T00:00:00Z');").unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now()).unwrap();
            let event = report
                .observations
                .iter()
                .find(|o| o.category == Category::EventChangefeed)
                .unwrap();
            assert_eq!(event.facts[&Fact::StoredEvents], 0);
            assert_eq!(
                event.facts[&Fact::EventStreams],
                if mode == "invalid" {
                    1
                } else {
                    (sequences::MAX_HEADS + 1) as u64
                }
            );
            if !checkpoint_missing {
                assert_eq!(event.facts[&Fact::UnexpiredEventCheckpoints], 1);
            }
            let gap = if mode == "invalid" {
                Gap::EventSequenceMetadataInvalid
            } else {
                Gap::EventSequenceReadBudgetExceeded
            };
            assert_eq!(
                event.gaps,
                if checkpoint_missing {
                    vec![Gap::CheckpointStoreUnavailable, gap]
                } else {
                    vec![gap]
                }
            );
            let mut forged = report.clone();
            forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::EventChangefeed)
                .unwrap()
                .facts
                .insert(Fact::EventSequenceStreamsChecked, 0);
            assert!(forged.validate().is_err());
            assert_eq!(conn.total_changes(), before);
            assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
            report.validate().unwrap();
        }
    }
}

#[test]
fn malformed_delivery_states_preserve_receipts_without_partial_delivery_counts() {
    use rusqlite::types::Value;
    for bad in [
        Value::Integer(1),
        Value::Real(1.5),
        Value::Blob(b"pending".to_vec()),
        Value::Text("PENDING".into()),
        Value::Text("pending\0PRIVATE_STATE".into()),
        Value::Text("PRIVATE_UNKNOWN".into()),
        Value::Text("x".repeat(4096)),
        Value::Blob(vec![0xff]),
    ] {
        for receipts_available in [false, true] {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE main.automation_runs;
                    CREATE TABLE main.automation_runs(delivery_state);
                    INSERT INTO main.automation_runs VALUES('pending'),('failed'),('delivered'),(NULL);").unwrap();
                conn.execute("INSERT INTO main.automation_runs VALUES(?1)", [bad.clone()])
                    .unwrap();
                if !receipts_available {
                    conn.execute_batch("DROP TABLE main.automation_receipts")
                        .unwrap();
                }
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_runs(delivery_state); INSERT INTO temp.automation_runs VALUES('delivered');
                    CREATE TEMP TABLE automation_receipts(id); INSERT INTO temp.automation_receipts VALUES('PRIVATE_RECEIPT');").unwrap();
                }
                let before = conn.total_changes();
                let report = inspect(&conn, Utc::now()).unwrap();
                let delivery = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::DeliveryReceipts)
                    .unwrap();
                assert!(
                    !delivery.facts.contains_key(&Fact::FailedDeliveries),
                    "invalid delivery state must not produce partial counts"
                );
                assert!(!delivery.facts.contains_key(&Fact::PendingDeliveries));
                assert_eq!(
                    delivery.facts.contains_key(&Fact::StoredReceipts),
                    receipts_available
                );
                let gaps = serde_json::to_value(&delivery.gaps).unwrap();
                assert_eq!(
                    gaps,
                    if receipts_available {
                        serde_json::json!([
                            "invalid_delivery_state",
                            "receipt_link_store_unavailable",
                            "receipt_commitments_unavailable",
                            "receipt_authority_not_verified"
                        ])
                    } else {
                        serde_json::json!([
                            "invalid_delivery_state",
                            "receipt_store_unavailable",
                            "receipt_link_store_unavailable"
                        ])
                    }
                );
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
                report.validate().unwrap();
                let mut forged = report.clone();
                forged
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::DeliveryReceipts)
                    .unwrap()
                    .facts
                    .insert(Fact::FailedDeliveries, 0);
                assert!(forged.validate().is_err());
            }
        }
    }
}

#[test]
fn valid_delivery_states_include_null_and_ignore_temp_substitutes() {
    let (_root, conn) = fixture();
    conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE main.automation_runs;
        CREATE TABLE main.automation_runs(delivery_state COLLATE NOCASE);
        INSERT INTO main.automation_runs VALUES(NULL),('pending'),('pending'),('failed'),('delivered');
        CREATE TEMP TABLE automation_runs(delivery_state); INSERT INTO temp.automation_runs VALUES('PRIVATE_INVALID');").unwrap();
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let delivery = report
        .observations
        .iter()
        .find(|o| o.category == Category::DeliveryReceipts)
        .unwrap();
    assert_eq!(delivery.facts[&Fact::PendingDeliveries], 2);
    assert_eq!(delivery.facts[&Fact::FailedDeliveries], 1);
    assert_eq!(
        delivery.gaps,
        vec![
            Gap::ReceiptLinkStoreUnavailable,
            Gap::ReceiptCommitmentsUnavailable,
            Gap::ReceiptAuthorityNotVerified
        ]
    );
    assert_eq!(conn.total_changes(), before);
    report.validate().unwrap();
    conn.execute_batch("INSERT INTO main.automation_runs VALUES('PENDING')")
        .unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let delivery = report
        .observations
        .iter()
        .find(|o| o.category == Category::DeliveryReceipts)
        .unwrap();
    assert_eq!(
        delivery.gaps,
        vec![
            Gap::InvalidDeliveryState,
            Gap::ReceiptLinkStoreUnavailable,
            Gap::ReceiptCommitmentsUnavailable,
            Gap::ReceiptAuthorityNotVerified
        ]
    );
    assert!(!delivery.facts.contains_key(&Fact::PendingDeliveries));
}

#[test]
fn receipt_link_findings_detect_nonreciprocal_metadata_without_payload_reads() {
    for (runs, receipts, expected) in [
        (
            "('run-a','receipt-a')",
            "('receipt-a','run-a')",
            "receipt_integrity_not_verified",
        ),
        (
            "('run-a',NULL)",
            "('receipt-a','run-a')",
            "receipt_links_inconsistent",
        ),
        (
            "('run-a','receipt-missing')",
            "('receipt-a','run-a')",
            "receipt_links_inconsistent",
        ),
        (
            "('run-a','receipt-a')",
            "('receipt-a','run-missing')",
            "receipt_links_inconsistent",
        ),
    ] {
        let (_root, conn) = fixture();
        conn.execute_batch(&format!("PRAGMA foreign_keys=OFF; DROP TABLE main.automation_runs; DROP TABLE main.automation_receipts;
            CREATE TABLE main.automation_runs(id,receipt_id,delivery_state); CREATE TABLE main.automation_receipts(id,run_id);
            INSERT INTO main.automation_runs(id,receipt_id) VALUES {runs};
            INSERT INTO main.automation_receipts VALUES {receipts};
            CREATE TEMP TABLE automation_runs(id,receipt_id,delivery_state); CREATE TEMP TABLE automation_receipts(id,run_id);
            INSERT INTO temp.automation_runs VALUES('PRIVATE_RUN','PRIVATE_RECEIPT','failed');")).unwrap();
        let before = conn.total_changes();
        let report = inspect(&conn, Utc::now()).unwrap();
        let delivery = report
            .observations
            .iter()
            .find(|o| o.category == Category::DeliveryReceipts)
            .unwrap();
        let mut expected_gaps = Vec::new();
        if expected != "receipt_integrity_not_verified" {
            expected_gaps.push(expected);
        }
        expected_gaps.extend([
            "receipt_commitments_unavailable",
            "receipt_authority_not_verified",
        ]);
        assert_eq!(
            serde_json::to_value(&delivery.gaps).unwrap(),
            serde_json::json!(expected_gaps)
        );
        assert_eq!(delivery.facts[&Fact::StoredReceipts], 1);
        assert_eq!(conn.total_changes(), before);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
        report.validate().unwrap();
    }
}

#[test]
fn receipt_link_report_rejects_observed_links_when_receipt_store_is_missing() {
    for suffix in [
        None,
        Some(Gap::ReceiptLinksInconsistent),
        Some(Gap::ReceiptLinkMetadataInvalid),
        Some(Gap::ReceiptLinkReadBudgetExceeded),
    ] {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TABLE main.automation_receipts")
            .unwrap();
        let mut report = inspect(&conn, Utc::now()).unwrap();
        let delivery = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DeliveryReceipts)
            .unwrap();
        delivery.gaps = vec![Gap::ReceiptStoreUnavailable];
        if let Some(suffix) = suffix {
            delivery.gaps.push(suffix);
        }
        assert!(
            report.validate().is_err(),
            "missing receipt store cannot support observed links: {suffix:?}"
        );
    }
}

fn receipt_link_fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE main.automation_runs(id,receipt_id); CREATE TABLE main.automation_receipts(id,run_id);").unwrap();
    conn
}

#[test]
fn receipt_link_scan_rejects_malformed_or_duplicate_keys_without_writes() {
    for mutation in [
        "UPDATE main.automation_runs SET id=NULL",
        "UPDATE main.automation_runs SET id=1",
        "UPDATE main.automation_runs SET id=x'ff'",
        "UPDATE main.automation_runs SET id=CAST(x'ff' AS TEXT)",
        "UPDATE main.automation_runs SET id='bad key'",
        "UPDATE main.automation_runs SET receipt_id=1",
        "UPDATE main.automation_runs SET receipt_id=printf('%0161d',1)",
        "INSERT INTO main.automation_runs SELECT * FROM main.automation_runs",
        "UPDATE main.automation_receipts SET id=NULL",
        "UPDATE main.automation_receipts SET id=CAST(x'ff' AS TEXT)",
        "UPDATE main.automation_receipts SET run_id=NULL",
        "UPDATE main.automation_receipts SET run_id=1.5",
        "UPDATE main.automation_receipts SET run_id='bad key'",
        "INSERT INTO main.automation_receipts SELECT * FROM main.automation_receipts",
        "INSERT INTO main.automation_receipts VALUES('receipt-b','run-a')",
    ] {
        let conn = receipt_link_fixture();
        conn.execute_batch("INSERT INTO main.automation_runs VALUES('run-a','receipt-a'); INSERT INTO main.automation_receipts VALUES('receipt-a','run-a');").unwrap();
        conn.execute_batch(mutation).unwrap();
        let before = conn.total_changes();
        assert_eq!(
            receipt_links::inspect(&conn).unwrap(),
            receipt_links::Observation::InvalidMetadata,
            "{mutation}"
        );
        assert_eq!(conn.total_changes(), before);
    }
}

#[test]
fn receipt_link_scan_observes_exact_row_and_combined_key_byte_limits() {
    let conn = receipt_link_fixture();
    conn.execute_batch(
        "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<255)
        INSERT INTO main.automation_runs SELECT 'run-'||i,NULL FROM n;",
    )
    .unwrap();
    assert_eq!(
        receipt_links::inspect(&conn).unwrap(),
        receipt_links::Observation::Consistent
    );
    conn.execute_batch("INSERT INTO main.automation_runs VALUES('overflow',NULL)")
        .unwrap();
    assert_eq!(
        receipt_links::inspect(&conn).unwrap(),
        receipt_links::Observation::BudgetExceeded
    );
    conn.execute_batch("DELETE FROM main.automation_runs;
        WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<255)
        INSERT INTO main.automation_runs SELECT 'r'||printf('%0127d',i),'p'||printf('%0127d',i) FROM n;
        INSERT INTO main.automation_receipts SELECT receipt_id,id FROM main.automation_runs;").unwrap();
    assert_eq!(receipt_links::MAX_INPUT_BYTES, 256 * 4 * 128);
    let before = conn.total_changes();
    assert_eq!(
        receipt_links::inspect(&conn).unwrap(),
        receipt_links::Observation::Consistent
    );
    assert_eq!(conn.total_changes(), before);
    conn.execute_batch(
        "UPDATE main.automation_runs SET id=id||'x' WHERE rowid=1;
        UPDATE main.automation_receipts SET run_id=run_id||'x' WHERE rowid=1;",
    )
    .unwrap();
    assert_eq!(
        receipt_links::inspect(&conn).unwrap(),
        receipt_links::Observation::BudgetExceeded
    );
    conn.execute_batch(
        "DELETE FROM main.automation_runs; DELETE FROM main.automation_receipts;
        WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<256)
        INSERT INTO main.automation_receipts SELECT 'receipt-'||i,'run-'||i FROM n;",
    )
    .unwrap();
    assert_eq!(
        receipt_links::inspect(&conn).unwrap(),
        receipt_links::Observation::BudgetExceeded
    );
}

#[test]
fn receipt_link_report_rejects_observed_links_beyond_receipt_scan_cap() {
    for gaps in [
        vec![Gap::ReceiptIntegrityNotVerified],
        vec![Gap::ReceiptLinksInconsistent],
    ] {
        let (_root, conn) = fixture();
        let mut report = inspect(&conn, Utc::now()).unwrap();
        let delivery = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DeliveryReceipts)
            .unwrap();
        delivery.gaps = gaps;
        delivery
            .facts
            .insert(Fact::StoredReceipts, receipt_links::MAX_ROWS as u64 + 1);
        assert!(
            report.validate().is_err(),
            "observed link conclusion cannot exceed receipt scan cap"
        );
    }
}

#[test]
fn receipt_link_report_rejects_delivery_subsets_beyond_run_scan_cap() {
    for gaps in [
        vec![Gap::ReceiptIntegrityNotVerified],
        vec![Gap::ReceiptLinksInconsistent],
    ] {
        for (failed, pending) in [(257, 0), (0, 257), (128, 129)] {
            let (_root, conn) = fixture();
            let mut report = inspect(&conn, Utc::now()).unwrap();
            let delivery = report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DeliveryReceipts)
                .unwrap();
            delivery.gaps = gaps.clone();
            delivery.facts.insert(Fact::FailedDeliveries, failed);
            delivery.facts.insert(Fact::PendingDeliveries, pending);
            assert!(
                report.validate().is_err(),
                "completed links cannot have delivery subsets above run cap: {failed}+{pending}"
            );
        }
    }
}

#[test]
fn database_geometry_observes_main_freelist_without_claiming_disk_capacity() {
    let (_root, conn) = fixture();
    conn.execute_batch("CREATE TABLE doctor_pressure_fixture(payload BLOB);
        INSERT INTO doctor_pressure_fixture VALUES(zeroblob(262144)),(zeroblob(262144)),(zeroblob(262144));
        DELETE FROM doctor_pressure_fixture;
        CREATE TEMP TABLE doctor_temp_pressure(payload BLOB); INSERT INTO temp.doctor_temp_pressure VALUES(zeroblob(2097152));").unwrap();
    let pages = conn
        .query_row("PRAGMA main.page_count", [], |row| row.get::<_, i64>(0))
        .unwrap();
    let page_size = conn
        .query_row("PRAGMA main.page_size", [], |row| row.get::<_, i64>(0))
        .unwrap();
    let free = conn
        .query_row("PRAGMA main.freelist_count", [], |row| row.get::<_, i64>(0))
        .unwrap();
    assert!(free > 0, "fixture must contain reusable pages");
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let storage = report
        .observations
        .iter()
        .find(|o| o.category == Category::DiskRetention)
        .unwrap();
    let facts = serde_json::to_value(&storage.facts).unwrap();
    assert_eq!(facts["database_freelist_pages"], serde_json::json!(free));
    assert_eq!(
        facts["database_logical_bytes"],
        serde_json::json!(pages * page_size)
    );
    assert_eq!(
        facts["database_non_freelist_bytes"],
        serde_json::json!((pages - free) * page_size)
    );
    assert_eq!(storage.gaps, vec![Gap::RetentionPressureNotObserved]);
    assert_eq!(conn.total_changes(), before);
    report.validate().unwrap();
}

#[test]
fn database_geometry_rejects_incompatible_dto_sizes_partitions_and_derived_bytes() {
    let (_root, conn) = fixture();
    let report = inspect(&conn, Utc::now()).unwrap();
    for (fact, value) in [
        (Fact::PageSizeBytes, 0),
        (Fact::PageSizeBytes, 511),
        (Fact::PageSizeBytes, 513),
        (Fact::PageSizeBytes, 131072),
        (Fact::DatabaseFreelistPages, MAX_SAFE),
        (Fact::DatabaseLogicalBytes, 0),
        (Fact::DatabaseNonFreelistBytes, MAX_SAFE),
    ] {
        let mut forged = report.clone();
        forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DiskRetention)
            .unwrap()
            .facts
            .insert(fact, value);
        assert!(
            forged.validate().is_err(),
            "invalid geometry: {fact:?}={value}"
        );
    }
    let mut forged = report.clone();
    let storage = forged
        .observations
        .iter_mut()
        .find(|o| o.category == Category::DiskRetention)
        .unwrap();
    storage.facts.insert(
        Fact::DatabaseFreelistPages,
        storage.facts[&Fact::DatabasePages],
    );
    storage.facts.insert(Fact::DatabaseNonFreelistBytes, 0);
    assert!(
        forged.validate().is_err(),
        "page one cannot be on the freelist"
    );
    for size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        assert_eq!(database_geometry(0, size, 0).unwrap(), (0, 0));
        assert_eq!(database_geometry(4, size, 3).unwrap(), (4 * size, size));
    }
    assert!(database_geometry(MAX_SAFE, 65536, 0).is_err());
    assert!(database_geometry(u64::MAX, 65536, 0).is_err());
    assert!(database_geometry(0, 4096, 1).is_err());
}

#[test]
fn filesystem_capacity_has_a_separate_sample_without_paths_or_capacity_guarantees() {
    let (root, conn) = fixture();
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let storage = report
        .observations
        .iter()
        .find(|o| o.category == Category::DiskRetention)
        .unwrap();
    let facts = serde_json::to_value(&storage.facts).unwrap();
    assert!(
        facts["filesystem_available_bytes_estimate"].is_u64(),
        "file-backed capacity probe must be observed rather than defaulted"
    );
    assert!(facts["filesystem_capacity_sampled_at_millis"].is_u64());
    assert_eq!(
        serde_json::to_value(&storage.gaps).unwrap(),
        serde_json::json!(["retention_pressure_not_observed"])
    );
    assert_eq!(conn.total_changes(), before);
    assert!(!serde_json::to_string(&report)
        .unwrap()
        .contains(root.path().to_str().unwrap()));
    report.validate().unwrap();
}

#[test]
fn filesystem_capacity_gap_requires_the_omitted_pair_and_preserves_geometry() {
    let (_root, conn) = fixture();
    let report = inspect(&conn, Utc::now()).unwrap();
    for gap in [
        Gap::FilesystemCapacityUnavailable,
        Gap::FilesystemCapacityMetadataInvalid,
    ] {
        let mut forged = report.clone();
        let storage = forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DiskRetention)
            .unwrap();
        storage.gaps = vec![gap, Gap::RetentionPressureNotObserved];
        assert!(
            forged.validate().is_err(),
            "failed probe cannot retain measured values"
        );
        for fact in [
            Fact::FilesystemAvailableBytesEstimate,
            Fact::FilesystemCapacitySampledAtMillis,
        ] {
            forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DiskRetention)
                .unwrap()
                .facts
                .remove(&fact);
        }
        forged.validate().unwrap();
        assert_eq!(
            forged
                .observations
                .iter()
                .find(|o| o.category == Category::DiskRetention)
                .unwrap()
                .facts
                .len(),
            8
        );
        let storage = forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DiskRetention)
            .unwrap();
        storage.gaps.reverse();
        assert!(forged.validate().is_err());
    }
    let mut zero = report.clone();
    zero.observations
        .iter_mut()
        .find(|o| o.category == Category::DiskRetention)
        .unwrap()
        .facts
        .insert(Fact::FilesystemAvailableBytesEstimate, 0);
    zero.validate().unwrap();
    zero.observations
        .iter_mut()
        .find(|o| o.category == Category::DiskRetention)
        .unwrap()
        .facts
        .remove(&Fact::FilesystemCapacitySampledAtMillis);
    assert!(
        zero.validate().is_err(),
        "a value without a sample timestamp is incomplete"
    );
}

#[test]
fn in_memory_capacity_is_unknown_without_hiding_database_geometry() {
    let conn = Connection::open_in_memory().unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let storage = report
        .observations
        .iter()
        .find(|o| o.category == Category::DiskRetention)
        .unwrap();
    assert_eq!(
        storage.gaps,
        vec![
            Gap::FilesystemCapacityUnavailable,
            Gap::WalSidecarUnavailable,
            Gap::RetentionPressureNotObserved
        ]
    );
    assert_eq!(storage.facts.len(), 5);
    assert!(!storage
        .facts
        .contains_key(&Fact::FilesystemAvailableBytesEstimate));
    report.validate().unwrap();
}

#[test]
fn null_and_nontext_pass_timestamps_preserve_independent_categories() {
    use rusqlite::types::Value;
    for (table, column) in [
        ("automation_scheduler_last_pass", "scheduled_at"),
        ("automation_scheduler_last_pass", "started_at"),
        ("automation_scheduler_last_successful_pass", "finished_at"),
    ] {
        for bad in [
            Value::Null,
            Value::Integer(1),
            Value::Blob(b"2026-10-18T00:00:00Z".to_vec()),
            Value::Blob(vec![0xff]),
        ] {
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                conn.execute_batch("DROP TABLE main.automation_scheduler_last_pass;
                    DROP TABLE main.automation_scheduler_last_successful_pass;
                    CREATE TABLE main.automation_scheduler_last_pass(id,status,duration_ms,scheduled_at,started_at);
                    CREATE TABLE main.automation_scheduler_last_successful_pass(id,finished_at);
                    INSERT INTO main.automation_scheduler_last_pass VALUES(1,'succeeded',25,'2026-10-18T00:00:00Z','2026-10-18T00:00:01Z');
                    INSERT INTO main.automation_scheduler_last_successful_pass VALUES(1,'2026-10-18T00:00:02Z');").unwrap();
                conn.execute(
                    &format!("UPDATE main.{table} SET {column}=?1"),
                    [bad.clone()],
                )
                .unwrap();
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_scheduler_last_pass AS SELECT * FROM main.automation_scheduler_last_pass;
                    UPDATE temp.automation_scheduler_last_pass SET scheduled_at='2026-10-18T00:00:00Z',started_at='2026-10-18T00:00:01Z';
                    CREATE TEMP TABLE automation_scheduler_last_successful_pass(id,finished_at);
                    INSERT INTO temp.automation_scheduler_last_successful_pass VALUES(1,'2026-10-18T00:00:02Z');").unwrap();
                }
                let before = conn.total_changes();
                let report = inspect(&conn, Utc::now())
                    .expect("malformed pass timestamp must remain a category finding");
                let category = if table == "automation_scheduler_last_pass" {
                    Category::PlanningStartLag
                } else {
                    Category::LastSuccessfulPass
                };
                let pass = report
                    .observations
                    .iter()
                    .find(|o| o.category == category)
                    .unwrap();
                assert!(pass.gaps.contains(&Gap::InvalidPersistedMetadata));
                let omitted = if category == Category::PlanningStartLag {
                    Fact::LatestPassStartLagMs
                } else {
                    Fact::LastSuccessfulPassFinishedAtMillis
                };
                assert!(!pass.facts.contains_key(&omitted));
                assert!(report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::Queue)
                    .unwrap()
                    .facts
                    .contains_key(&Fact::PlannedOccurrences));
                assert!(report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::Store)
                    .unwrap()
                    .facts
                    .contains_key(&Fact::PresentAutomationTables));
                assert_eq!(conn.total_changes(), before);
                report.validate().unwrap();
            }
        }
    }
}

#[test]
fn latest_pass_status_requires_exact_text_despite_column_collation() {
    for (collation, status) in [
        ("NOCASE", "SUCCEEDED"),
        ("NOCASE", "FAILED"),
        ("RTRIM", "succeeded "),
        ("RTRIM", "failed "),
    ] {
        for shadow in [false, true] {
            let (_root, conn) = fixture();
            conn.execute_batch(&format!("DROP TABLE main.automation_scheduler_last_pass;
                CREATE TABLE main.automation_scheduler_last_pass(id,status COLLATE {collation},duration_ms,scheduled_at,started_at);
                INSERT INTO main.automation_scheduler_last_pass VALUES(1,'succeeded',25,'2026-10-18T00:00:00Z','2026-10-18T00:00:01Z');")).unwrap();
            conn.execute(
                "UPDATE main.automation_scheduler_last_pass SET status=?1",
                [status],
            )
            .unwrap();
            if shadow {
                conn.execute_batch(
                    "CREATE TEMP TABLE automation_scheduler_last_pass(id,status);
                    INSERT INTO temp.automation_scheduler_last_pass VALUES(1,'succeeded');",
                )
                .unwrap();
            }
            let before = conn.total_changes();
            let report = inspect(&conn, Utc::now()).unwrap();
            let pass = report
                .observations
                .iter()
                .find(|o| o.category == Category::LastSuccessfulPass)
                .unwrap();
            assert_eq!(
                pass.gaps,
                vec![Gap::InvalidLatestPassStatus],
                "{collation}: {status:?}"
            );
            assert!(pass.facts.is_empty());
            assert!(report
                .observations
                .iter()
                .find(|o| o.category == Category::Queue)
                .unwrap()
                .facts
                .contains_key(&Fact::PlannedOccurrences));
            assert_eq!(conn.total_changes(), before);
            report.validate().unwrap();
        }
    }
}

#[test]
fn malformed_attempt_metadata_withholds_only_affected_counts() {
    use rusqlite::types::Value;
    for column in ["state", "failure_class"] {
        for bad in [
            Value::Null,
            Value::Integer(1),
            Value::Blob(b"failed".to_vec()),
            Value::Text("PRIVATE_UNKNOWN".into()),
            Value::Text("FAILED".into()),
            Value::Text("failed\0".into()),
            Value::Text("launch_refused ".into()),
            Value::Text("x".repeat(65)),
        ] {
            if column == "failure_class" && bad == Value::Null {
                continue;
            }
            for shadow in [false, true] {
                let (_root, conn) = fixture();
                conn.execute_batch("DROP TABLE main.automation_attempts;
                    CREATE TABLE main.automation_attempts(id,run_id,state COLLATE NOCASE,not_before,failure_class COLLATE NOCASE);
                    INSERT INTO main.automation_attempts VALUES('one','one','failed',NULL,'launch_refused');").unwrap();
                conn.execute(
                    &format!("UPDATE main.automation_attempts SET {column}=?1"),
                    [bad.clone()],
                )
                .unwrap();
                if shadow {
                    conn.execute_batch("CREATE TEMP TABLE automation_attempts(id,run_id,state,not_before,failure_class);
                        INSERT INTO temp.automation_attempts VALUES('shadow','shadow','failed',NULL,'launch_refused');").unwrap();
                }
                let before = conn.total_changes();
                let report = inspect(&conn, Utc::now()).unwrap();
                let failures = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::RepeatedFailures)
                    .unwrap();
                let refusals = report
                    .observations
                    .iter()
                    .find(|o| o.category == Category::AuthorityRefusals)
                    .unwrap();
                if column == "state" {
                    assert!(
                        !failures.facts.contains_key(&Fact::FailedAttempts),
                        "bad state: {bad:?}"
                    );
                    assert_eq!(
                        serde_json::to_value(&failures.gaps).unwrap(),
                        serde_json::json!(["invalid_attempt_state"])
                    );
                    assert_eq!(refusals.facts[&Fact::LaunchRefusedAttempts], 1);
                } else {
                    assert!(refusals.facts.is_empty(), "bad class: {bad:?}");
                    assert_eq!(
                        serde_json::to_value(&refusals.gaps).unwrap(),
                        serde_json::json!(["invalid_attempt_failure_class"])
                    );
                    assert_eq!(failures.facts[&Fact::FailedAttempts], 1);
                }
                assert!(failures.facts.contains_key(&Fact::QuarantinedRoutines));
                assert_eq!(conn.total_changes(), before);
                assert!(!serde_json::to_string(&report)
                    .unwrap()
                    .contains("PRIVATE_UNKNOWN"));
                report.validate().unwrap();
            }
        }
    }
}

#[test]
fn canonical_attempt_metadata_and_concurrent_invalid_gaps_are_closed() {
    use super::super::contract::types::AttemptState;
    let (_root, conn) = fixture();
    conn.execute_batch("DROP TABLE main.automation_attempts;
        CREATE TABLE main.automation_attempts(id,run_id,state COLLATE RTRIM,not_before,failure_class COLLATE RTRIM);").unwrap();
    let states = [
        AttemptState::Adopted,
        AttemptState::Dispatching,
        AttemptState::Started,
        AttemptState::Observing,
        AttemptState::Succeeded,
        AttemptState::Failed,
        AttemptState::Cancelled,
        AttemptState::TimedOut,
        AttemptState::Ambiguous,
    ];
    let classes = [
        "transient_dispatch",
        "lease_expired",
        "runtime_unavailable",
        "launch_refused",
        "runtime_error",
        "timeout",
        "cancelled",
        "ambiguous_evidence",
        "runtime_authority_unsupported",
    ];
    for (index, state) in states.into_iter().enumerate() {
        let state_name = serde_json::to_value(state).unwrap();
        conn.execute(
            "INSERT INTO main.automation_attempts VALUES(?1,?1,?2,NULL,?3)",
            rusqlite::params![index as i64, state_name.as_str().unwrap(), classes[index]],
        )
        .unwrap();
    }
    conn.execute_batch("INSERT INTO main.automation_attempts VALUES(9,9,'succeeded',NULL,NULL);")
        .unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let failures = report
        .observations
        .iter()
        .find(|o| o.category == Category::RepeatedFailures)
        .unwrap();
    assert_eq!(failures.facts[&Fact::FailedAttempts], 1);
    let refusals = report
        .observations
        .iter()
        .find(|o| o.category == Category::AuthorityRefusals)
        .unwrap();
    assert_eq!(refusals.facts[&Fact::LaunchRefusedAttempts], 1);
    assert_eq!(refusals.facts[&Fact::UnsupportedAuthorityAttempts], 1);
    report.validate().unwrap();
    conn.execute_batch("UPDATE main.automation_attempts SET state=CAST(x'ff' AS TEXT),failure_class=CAST(x'ff' AS TEXT) WHERE id=0;
        DROP TABLE main.automation_retry_state;
        CREATE TABLE main.automation_retry_state(consecutive_exhaustions);
        INSERT INTO main.automation_retry_state VALUES(-1);").unwrap();
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let failures = report
        .observations
        .iter()
        .find(|o| o.category == Category::RepeatedFailures)
        .unwrap();
    for fact in [
        Fact::FailedAttempts,
        Fact::QuarantinedRoutines,
        Fact::RetryExhaustionHistories,
        Fact::MaximumConsecutiveRetryExhaustions,
    ] {
        assert!(!failures.facts.contains_key(&fact));
    }
    assert_eq!(
        failures.facts[&Fact::RetainedOccurrenceFailureHistoriesChecked],
        0
    );
    assert_eq!(
        failures.gaps,
        vec![
            Gap::InvalidAttemptState,
            Gap::QuarantineStoreUnavailable,
            Gap::InvalidPersistedMetadata
        ]
    );
    report.validate().unwrap();
    for (category, fact) in [
        (Category::RepeatedFailures, Fact::FailedAttempts),
        (Category::AuthorityRefusals, Fact::LaunchRefusedAttempts),
        (
            Category::AuthorityRefusals,
            Fact::UnsupportedAuthorityAttempts,
        ),
    ] {
        let mut forged = report.clone();
        forged
            .observations
            .iter_mut()
            .find(|o| o.category == category)
            .unwrap()
            .facts
            .insert(fact, 0);
        assert!(
            forged.validate().is_err(),
            "invalid metadata cannot claim measured zero"
        );
    }
    let mut forged = report.clone();
    forged
        .observations
        .iter_mut()
        .find(|o| o.category == Category::RepeatedFailures)
        .unwrap()
        .gaps
        .reverse();
    assert!(forged.validate().is_err());
    let mut forged = report.clone();
    forged
        .observations
        .iter_mut()
        .find(|o| o.category == Category::RepeatedFailures)
        .unwrap()
        .gaps
        .insert(0, Gap::AttemptStateUnavailable);
    assert!(forged.validate().is_err());
    assert_eq!(conn.total_changes(), before);
}

#[test]
fn wal_sidecar_metadata_is_observed_without_reading_or_changing_it() {
    let (_root, conn) = fixture();
    let mut sidecar = std::ffi::OsString::from(conn.path().unwrap());
    sidecar.push("-wal");
    let path = std::path::PathBuf::from(sidecar);
    let before = std::fs::metadata(&path).unwrap().len();
    let changes = conn.total_changes();
    let report = serde_json::to_value(inspect(&conn, Utc::now()).unwrap()).unwrap();
    let storage = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["category"] == "disk_retention")
        .unwrap();
    assert_eq!(storage["facts"]["database_wal_sidecar_present"], 1);
    assert_eq!(
        storage["facts"]["database_wal_sidecar_bytes_estimate"],
        before
    );
    assert!(storage["facts"]["database_wal_sidecar_sampled_at_millis"]
        .as_u64()
        .is_some());
    assert_eq!(
        storage["gaps"],
        serde_json::json!(["retention_pressure_not_observed"])
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
    assert_eq!(conn.total_changes(), changes);
}

#[test]
fn absent_wal_sidecar_report_is_sampled_zero_without_store_initialization() {
    let root = tempfile::tempdir().unwrap();
    let conn = Connection::open(root.path().join("PRIVATE_empty.sqlite3")).unwrap();
    let before = conn.total_changes();
    let report = inspect(&conn, Utc::now()).unwrap();
    let storage = report
        .observations
        .iter()
        .find(|o| o.category == Category::DiskRetention)
        .unwrap();
    assert_eq!(storage.facts[&Fact::DatabaseWalSidecarPresent], 0);
    assert_eq!(storage.facts[&Fact::DatabaseWalSidecarBytesEstimate], 0);
    assert!(storage
        .facts
        .contains_key(&Fact::DatabaseWalSidecarSampledAtMillis));
    assert_eq!(conn.total_changes(), before);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE"));
    report.validate().unwrap();
}

#[test]
fn wal_sidecar_fact_group_and_concurrent_gaps_are_closed() {
    let (_root, conn) = fixture();
    let report = inspect(&conn, Utc::now()).unwrap();
    let fields = [
        Fact::DatabaseWalSidecarPresent,
        Fact::DatabaseWalSidecarBytesEstimate,
        Fact::DatabaseWalSidecarSampledAtMillis,
    ];
    for fact in fields {
        let mut forged = report.clone();
        forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DiskRetention)
            .unwrap()
            .facts
            .remove(&fact);
        assert!(
            forged.validate().is_err(),
            "sidecar observations require the entire group"
        );
    }
    for (present, bytes, valid) in [(0, 0, true), (0, 1, false), (1, 0, true), (2, 0, false)] {
        let mut forged = report.clone();
        let storage = forged
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DiskRetention)
            .unwrap();
        storage
            .facts
            .insert(Fact::DatabaseWalSidecarPresent, present);
        storage
            .facts
            .insert(Fact::DatabaseWalSidecarBytesEstimate, bytes);
        assert_eq!(forged.validate().is_ok(), valid);
    }
    for gap in [Gap::WalSidecarUnavailable, Gap::WalSidecarMetadataInvalid] {
        for capacity in [
            None,
            Some(Gap::FilesystemCapacityUnavailable),
            Some(Gap::FilesystemCapacityMetadataInvalid),
        ] {
            let mut forged = report.clone();
            let storage = forged
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DiskRetention)
                .unwrap();
            storage.gaps = capacity
                .into_iter()
                .chain([gap, Gap::RetentionPressureNotObserved])
                .collect();
            for fact in fields {
                storage.facts.remove(&fact);
            }
            if capacity.is_some() {
                storage
                    .facts
                    .remove(&Fact::FilesystemAvailableBytesEstimate);
                storage
                    .facts
                    .remove(&Fact::FilesystemCapacitySampledAtMillis);
            }
            assert!(storage.facts.contains_key(&Fact::DatabasePages));
            forged.validate().unwrap();
            let mut fake_zero = forged.clone();
            fake_zero
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DiskRetention)
                .unwrap()
                .facts
                .insert(Fact::DatabaseWalSidecarBytesEstimate, 0);
            assert!(
                fake_zero.validate().is_err(),
                "unknown sidecar cannot claim measured zero"
            );
            let mut reversed = forged.clone();
            reversed
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DiskRetention)
                .unwrap()
                .gaps
                .reverse();
            assert!(
                reversed.validate().is_err(),
                "independent storage gaps have one canonical order"
            );
            let mut duplicate = forged;
            duplicate
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DiskRetention)
                .unwrap()
                .gaps
                .insert(0, gap);
            assert!(duplicate.validate().is_err());
        }
    }
}

#[test]
fn retained_scheduled_planning_delay_is_separate_from_pass_and_runtime_start() {
    let (_root, conn) = fixture();
    conn.execute_batch("INSERT INTO automation_occurrences
        (id, automation_id, scheduled_for, kind, created_at, updated_at) VALUES
        ('early', 'a', '2026-10-18T01:00:00Z', 'scheduled', '2026-10-18T00:59:59Z', '2026-10-18T01:00:00Z'),
        ('late', 'b', '2026-10-18T02:00:00+01:00', 'scheduled', '2026-10-18T01:00:00.250Z', '2026-10-18T01:00:01Z'),
        ('manual', 'c', '2026-10-18T00:00:00Z', 'manual', '2026-10-18T02:00:00Z', '2026-10-18T02:00:00Z');").unwrap();
    let report = serde_json::to_value(inspect(&conn, Utc::now()).unwrap()).unwrap();
    let planning = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["category"] == "planning_start_lag")
        .unwrap();
    assert_eq!(
        planning["facts"]["scheduled_occurrences_planning_checked"],
        2
    );
    assert_eq!(
        planning["facts"]["maximum_scheduled_occurrence_planning_delay_ms"],
        250
    );
    assert!(planning["facts"].get("latest_pass_start_lag_ms").is_none());
    assert_eq!(
        planning["gaps"],
        serde_json::json!(["lag_history_not_measured"])
    );
}

#[test]
fn planning_measurement_dto_is_atomic_bounded_and_independent_of_pass_scope() {
    let (_root, conn) = fixture();
    let original = inspect(&conn, Utc::now()).unwrap();
    let category = |report: &mut Report| {
        report
            .observations
            .iter()
            .position(|o| o.category == Category::PlanningStartLag)
            .unwrap()
    };
    for mutation in 0..8 {
        let mut forged = original.clone();
        let index = category(&mut forged);
        let observation = &mut forged.observations[index];
        match mutation {
            0 => {
                observation
                    .facts
                    .remove(&Fact::ScheduledOccurrencesPlanningChecked);
            }
            1 => {
                observation
                    .facts
                    .remove(&Fact::MaximumScheduledOccurrencePlanningDelayMs);
            }
            2 => {
                observation
                    .facts
                    .insert(Fact::MaximumScheduledOccurrencePlanningDelayMs, 1);
            }
            3 => {
                observation
                    .facts
                    .insert(Fact::ScheduledOccurrencesPlanningChecked, 257);
            }
            4 => observation
                .gaps
                .push(Gap::OccurrencePlanningStoreUnavailable),
            5 => {
                observation.facts.clear();
            }
            6 => {
                observation.facts.clear();
                observation.gaps.extend([
                    Gap::OccurrencePlanningStoreUnavailable,
                    Gap::OccurrencePlanningMetadataInvalid,
                ]);
            }
            _ => {
                observation.facts.clear();
                observation
                    .gaps
                    .insert(0, Gap::OccurrencePlanningStoreUnavailable);
            }
        }
        assert!(
            forged.validate().is_err(),
            "forged planning group {mutation}"
        );
    }
    conn.execute_batch(
        "ALTER TABLE automation_occurrences RENAME COLUMN created_at TO missing_created_at",
    )
    .unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let planning = report
        .observations
        .iter()
        .find(|o| o.category == Category::PlanningStartLag)
        .unwrap();
    assert_eq!(
        planning.gaps,
        vec![
            Gap::LagHistoryNotMeasured,
            Gap::OccurrencePlanningStoreUnavailable
        ]
    );
    assert!(!planning.facts.contains_key(&Fact::LatestPassDurationMs));
    assert!(!planning.facts.contains_key(&Fact::LatestPassStartLagMs));
    assert!(report
        .observations
        .iter()
        .find(|o| o.category == Category::Queue)
        .unwrap()
        .facts
        .contains_key(&Fact::PlannedOccurrences));
}

#[test]
fn completed_planning_scan_bounds_independent_occurrence_subsets() {
    let (_root, conn) = fixture();
    let original = inspect(&conn, Utc::now()).unwrap();
    for variant in 0..3 {
        let mut report = original.clone();
        if variant == 0 {
            let queue = report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::Queue)
                .unwrap();
            queue.facts.insert(Fact::PlannedOccurrences, 129);
            queue.facts.insert(Fact::ClaimedOccurrences, 128);
        } else {
            let leases = report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::LeasesFences)
                .unwrap();
            if variant == 1 {
                leases.facts.insert(Fact::LeaseBearingOccurrences, 257);
                leases.facts.insert(Fact::ActiveLeaseOccurrences, 257);
            } else {
                for fact in [
                    Fact::LeaseBearingOccurrences,
                    Fact::ActiveLeaseOccurrences,
                    Fact::ExpiredLeaseOccurrences,
                    Fact::MissingLeaseExpiryOccurrences,
                    Fact::InvalidLeaseExpiryOccurrences,
                    Fact::LeaseOutsideActiveStateOccurrences,
                ] {
                    leases.facts.remove(&fact);
                }
                leases.gaps = vec![Gap::LeaseStoreUnavailable];
                leases.facts.insert(Fact::RecordedSchedulerFencePresent, 1);
                leases
                    .facts
                    .insert(Fact::MismatchedUnlaunchedSchedulerClaims, 257);
            }
        }
        assert!(
            report.validate().is_err(),
            "completed planning scan accepted inconsistent cohort {variant}"
        );
        let planning = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::PlanningStartLag)
            .unwrap();
        planning
            .facts
            .remove(&Fact::ScheduledOccurrencesPlanningChecked);
        planning
            .facts
            .remove(&Fact::MaximumScheduledOccurrencePlanningDelayMs);
        planning
            .gaps
            .push(Gap::OccurrencePlanningReadBudgetExceeded);
        let failures = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::RepeatedFailures)
            .unwrap();
        for fact in [
            Fact::RetainedOccurrenceFailureHistoriesChecked,
            Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess,
            Fact::MaximumRetainedFailuresSinceSuccess,
        ] {
            failures.facts.remove(&fact);
        }
        failures
            .gaps
            .push(Gap::OccurrenceFailureHistoryReadBudgetExceeded);
        assert!(
            report.validate().is_ok(),
            "incomplete scan must not impose a completed cohort cap {variant}"
        );
    }
}

#[test]
fn retained_failure_history_counts_after_success_without_claiming_a_strict_streak() {
    let (_root, conn) = fixture();
    conn.execute_batch("INSERT INTO automation_occurrences
        (id, automation_id, scheduled_for, state, created_at, updated_at) VALUES
        ('a0','PRIVATE-HISTORY-A','2026-10-17T23:59:59Z','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('a1','PRIVATE-HISTORY-A','2026-10-18T01:00:00+01:00','succeeded','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('a2','PRIVATE-HISTORY-A','2026-10-18T00:00:01Z','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('a3','PRIVATE-HISTORY-A','2026-10-18T00:00:02Z','cancelled','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('a4','PRIVATE-HISTORY-A','2026-10-18T00:00:03Z','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('b0','PRIVATE-HISTORY-B','2026-10-18T00:00:00Z','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('b1','PRIVATE-HISTORY-B','2026-10-18T00:00:01Z','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('b2','PRIVATE-HISTORY-B','2026-10-18T00:00:02Z','planned','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('c0','PRIVATE-HISTORY-C','2026-10-18T00:00:00Z','succeeded','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z'),
        ('c1','PRIVATE-HISTORY-C','2026-10-18T00:00:01Z','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z');").unwrap();
    let before = conn.total_changes();
    let report = serde_json::to_value(inspect(&conn, Utc::now()).unwrap()).unwrap();
    let failures = report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["category"] == "repeated_failures")
        .unwrap();
    assert_eq!(
        failures["facts"]["retained_occurrence_failure_histories_checked"],
        3
    );
    assert_eq!(
        failures["facts"]["routines_with_repeated_retained_failures_since_success"],
        2
    );
    assert_eq!(
        failures["facts"]["maximum_retained_failures_since_success"],
        2
    );
    assert!(!report.to_string().contains("PRIVATE-HISTORY"));
    assert_eq!(conn.total_changes(), before);
}

#[test]
fn completed_failure_history_rejects_disjoint_and_empty_cohort_contradictions() {
    let (_root, conn) = fixture();
    let original = inspect(&conn, Utc::now()).unwrap();
    for variant in 0..5 {
        let mut report = original.clone();
        if variant <= 1 {
            report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::Queue)
                .unwrap()
                .facts
                .insert(Fact::PlannedOccurrences, 1);
            if variant == 0 {
                let history = report
                    .observations
                    .iter_mut()
                    .find(|o| o.category == Category::RepeatedFailures)
                    .unwrap();
                history
                    .facts
                    .insert(Fact::RetainedOccurrenceFailureHistoriesChecked, 1);
                history
                    .facts
                    .insert(Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess, 1);
                history
                    .facts
                    .insert(Fact::MaximumRetainedFailuresSinceSuccess, 256);
            }
        } else if variant == 2 {
            let leases = report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::LeasesFences)
                .unwrap();
            leases.facts.insert(Fact::LeaseBearingOccurrences, 1);
            leases.facts.insert(Fact::ActiveLeaseOccurrences, 1);
            leases
                .facts
                .insert(Fact::LeaseOutsideActiveStateOccurrences, 1);
        } else if variant == 3 {
            let leases = report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::LeasesFences)
                .unwrap();
            for fact in [
                Fact::LeaseBearingOccurrences,
                Fact::ActiveLeaseOccurrences,
                Fact::ExpiredLeaseOccurrences,
                Fact::MissingLeaseExpiryOccurrences,
                Fact::InvalidLeaseExpiryOccurrences,
                Fact::LeaseOutsideActiveStateOccurrences,
            ] {
                leases.facts.remove(&fact);
            }
            leases.gaps = vec![Gap::LeaseStoreUnavailable];
            leases.facts.insert(Fact::RecordedSchedulerFencePresent, 1);
            leases
                .facts
                .insert(Fact::MismatchedUnlaunchedSchedulerClaims, 1);
        } else {
            report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::PlanningStartLag)
                .unwrap()
                .facts
                .insert(Fact::ScheduledOccurrencesPlanningChecked, 1);
        }
        assert!(
            report.validate().is_err(),
            "history cohort contradiction {variant} was accepted"
        );
        let history = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::RepeatedFailures)
            .unwrap();
        for fact in [
            Fact::RetainedOccurrenceFailureHistoriesChecked,
            Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess,
            Fact::MaximumRetainedFailuresSinceSuccess,
        ] {
            history.facts.remove(&fact);
        }
        history
            .gaps
            .push(Gap::OccurrenceFailureHistoryMetadataInvalid);
        assert!(
            report.validate().is_ok(),
            "unobserved history falsely proves empty cohort {variant}"
        );
    }
}

#[test]
fn failure_history_dto_groups_and_missing_source_preserve_independent_facts() {
    let (_root, conn) = fixture();
    let original = inspect(&conn, Utc::now()).unwrap();
    for variant in 0..9 {
        let mut report = original.clone();
        let history = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::RepeatedFailures)
            .unwrap();
        match variant {
            0 => {
                history
                    .facts
                    .remove(&Fact::RetainedOccurrenceFailureHistoriesChecked);
            }
            1 => {
                history
                    .facts
                    .remove(&Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess);
            }
            2 => {
                history
                    .facts
                    .remove(&Fact::MaximumRetainedFailuresSinceSuccess);
            }
            3 => {
                history
                    .facts
                    .insert(Fact::MaximumRetainedFailuresSinceSuccess, 1);
            }
            4 => {
                history
                    .facts
                    .insert(Fact::RetainedOccurrenceFailureHistoriesChecked, 257);
            }
            5 => {
                history
                    .gaps
                    .push(Gap::OccurrenceFailureHistoryStoreUnavailable);
            }
            6 => {
                history
                    .facts
                    .insert(Fact::RetainedOccurrenceFailureHistoriesChecked, 1);
                history
                    .facts
                    .insert(Fact::MaximumRetainedFailuresSinceSuccess, 2);
            }
            7 => {
                history
                    .facts
                    .insert(Fact::RetainedOccurrenceFailureHistoriesChecked, 256);
                history
                    .facts
                    .insert(Fact::MaximumRetainedFailuresSinceSuccess, 2);
                history
                    .facts
                    .insert(Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess, 1);
            }
            _ => {
                history
                    .facts
                    .remove(&Fact::RetainedOccurrenceFailureHistoriesChecked);
                history
                    .facts
                    .remove(&Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess);
                history
                    .facts
                    .remove(&Fact::MaximumRetainedFailuresSinceSuccess);
                history
                    .gaps
                    .insert(0, Gap::OccurrenceFailureHistoryStoreUnavailable);
            }
        }
        assert!(report.validate().is_err(), "forged history group {variant}");
    }
    conn.execute_batch(
        "ALTER TABLE automation_occurrences RENAME COLUMN automation_id TO missing_automation_id",
    )
    .unwrap();
    let report = inspect(&conn, Utc::now()).unwrap();
    let history = report
        .observations
        .iter()
        .find(|o| o.category == Category::RepeatedFailures)
        .unwrap();
    assert_eq!(history.facts[&Fact::FailedAttempts], 0);
    assert_eq!(history.facts[&Fact::QuarantinedRoutines], 0);
    assert!(!history
        .facts
        .contains_key(&Fact::RetainedOccurrenceFailureHistoriesChecked));
    assert_eq!(
        history.gaps,
        vec![
            Gap::FailureHistoryNotSummarized,
            Gap::OccurrenceFailureHistoryStoreUnavailable
        ]
    );
}

#[test]
fn failed_history_and_claimed_subsets_share_the_total_occurrence_cap() {
    let (_root, conn) = fixture();
    let original = inspect(&conn, Utc::now()).unwrap();
    for lease_available in [false, true] {
        let mut report = original.clone();
        let queue = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::Queue)
            .unwrap();
        queue.facts.clear();
        queue.gaps = vec![Gap::InvalidOccurrenceState];
        let history = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::RepeatedFailures)
            .unwrap();
        history
            .facts
            .insert(Fact::RetainedOccurrenceFailureHistoriesChecked, 1);
        history
            .facts
            .insert(Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess, 1);
        history
            .facts
            .insert(Fact::MaximumRetainedFailuresSinceSuccess, 256);
        let leases = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::LeasesFences)
            .unwrap();
        if lease_available {
            leases.facts.insert(Fact::LeaseBearingOccurrences, 1);
            leases.facts.insert(Fact::ActiveLeaseOccurrences, 1);
        } else {
            for fact in [
                Fact::LeaseBearingOccurrences,
                Fact::ActiveLeaseOccurrences,
                Fact::ExpiredLeaseOccurrences,
                Fact::MissingLeaseExpiryOccurrences,
                Fact::InvalidLeaseExpiryOccurrences,
                Fact::LeaseOutsideActiveStateOccurrences,
            ] {
                leases.facts.remove(&fact);
            }
            leases.gaps = vec![Gap::LeaseStoreUnavailable];
            leases.facts.insert(Fact::RecordedSchedulerFencePresent, 1);
            leases
                .facts
                .insert(Fact::MismatchedUnlaunchedSchedulerClaims, 1);
        }
        assert!(
            report.validate().is_err(),
            "claimed subset contradicts full failed cohort; lease available={lease_available}"
        );
        let history = report
            .observations
            .iter_mut()
            .find(|o| o.category == Category::RepeatedFailures)
            .unwrap();
        for fact in [
            Fact::RetainedOccurrenceFailureHistoriesChecked,
            Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess,
            Fact::MaximumRetainedFailuresSinceSuccess,
        ] {
            history.facts.remove(&fact);
        }
        history
            .gaps
            .push(Gap::OccurrenceFailureHistoryReadBudgetExceeded);
        report
            .validate()
            .expect("incomplete failure scan cannot invent a disjoint failed cohort");
    }
}

fn leases_fences(report: &mut Report) -> &mut Observation {
    report
        .observations
        .iter_mut()
        .find(|o| o.category == Category::LeasesFences)
        .unwrap()
}

#[test]
fn stop_fences_are_counted_by_lease_without_their_identities() {
    let (_root, conn) = fixture();
    let now = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 10, 22, 9, 0, 0).unwrap();
    conn.execute_batch(
        "INSERT INTO automation_stop_fences
            (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at) VALUES
         ('PRIVATE-RUN-A', 'PRIVATE-SESSION-A', 'cancellation', 'PRIVATE-KEY',
          '2026-10-22T08:59:00Z', '2026-10-22T09:00:30Z'),
         ('PRIVATE-RUN-B', 'PRIVATE-SESSION-B', 'timeout', NULL,
          '2026-10-22T08:59:00Z', '2026-10-22T09:00:00Z'),
         ('PRIVATE-RUN-C', 'PRIVATE-SESSION-C', 'recovery', NULL,
          '2026-10-22T08:59:00Z', 'PRIVATE-LEASE'),
         ('PRIVATE-RUN-D', 'PRIVATE-SESSION-D', 'timeout', NULL,
          '2026-10-22T08:59:00Z', CAST('2026-10-22T09:00:30Z' AS BLOB))",
    )
    .unwrap();
    let before = conn.total_changes();
    let mut report = inspect(&conn, now).unwrap();
    let text = serde_json::to_string(&report).unwrap();
    assert!(!text.contains("PRIVATE"));
    assert_eq!(conn.total_changes(), before);
    let leases = leases_fences(&mut report);
    // Held at the instant its lease ends, a fence has expired.
    for (fact, count) in [
        (Fact::StopFences, 4),
        (Fact::ActiveStopFences, 1),
        (Fact::ExpiredStopFences, 1),
        (Fact::InvalidStopFenceExpiries, 2),
    ] {
        assert_eq!(leases.facts[&fact], count, "{fact:?}");
    }
    assert_eq!(leases.gaps, [Gap::FenceConsistencyNotMeasured]);

    // Without the main table, the gap stands in for every stop-fence fact,
    // and the occurrence leases are still read.
    conn.execute_batch("DROP TABLE automation_stop_fences")
        .unwrap();
    let mut report = inspect(&conn, now).unwrap();
    let leases = leases_fences(&mut report);
    assert!(!leases.facts.contains_key(&Fact::StopFences));
    assert_eq!(leases.facts[&Fact::LeaseBearingOccurrences], 0);
    assert_eq!(
        leases.gaps,
        [
            Gap::FenceConsistencyNotMeasured,
            Gap::StopFenceStoreUnavailable
        ]
    );
}

#[test]
fn stop_fence_facts_are_whole_or_replaced_by_their_gap() {
    let (_root, conn) = fixture();
    let original = inspect(&conn, Utc::now()).unwrap();
    let stop_facts = [
        Fact::StopFences,
        Fact::ActiveStopFences,
        Fact::ExpiredStopFences,
        Fact::InvalidStopFenceExpiries,
    ];
    for variant in 0..7 {
        let mut report = original.clone();
        let leases = leases_fences(&mut report);
        match variant {
            0 => {
                leases.facts.remove(&Fact::StopFences);
            }
            1 => {
                leases.facts.insert(Fact::ActiveStopFences, 1);
            }
            2 => {
                leases.facts.insert(Fact::StopFences, 1);
            }
            3 => leases.gaps.push(Gap::StopFenceStoreUnavailable),
            // Facts beyond the safe-integer range that still add up.
            6 => {
                leases.facts.insert(Fact::StopFences, MAX_SAFE + 1);
                leases.facts.insert(Fact::ActiveStopFences, MAX_SAFE + 1);
            }
            // No report omits them silently: no release predates them.
            4 => {
                for fact in stop_facts {
                    leases.facts.remove(&fact);
                }
            }
            _ => {
                for fact in stop_facts {
                    leases.facts.remove(&fact);
                }
                leases.gaps.insert(0, Gap::StopFenceStoreUnavailable);
            }
        }
        assert!(report.validate().is_err(), "forged stop fences {variant}");
    }
    // The gap alone, after the lease gaps, is a whole report.
    for lease_gaps in [
        vec![Gap::FenceConsistencyNotMeasured],
        vec![Gap::LeaseStateUnavailable],
    ] {
        let mut report = original.clone();
        let leases = leases_fences(&mut report);
        for fact in stop_facts {
            leases.facts.remove(&fact);
        }
        if lease_gaps == [Gap::LeaseStateUnavailable] {
            leases
                .facts
                .remove(&Fact::LeaseOutsideActiveStateOccurrences);
        }
        leases.gaps = lease_gaps;
        leases.gaps.push(Gap::StopFenceStoreUnavailable);
        report.validate().unwrap();
    }
}
