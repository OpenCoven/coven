use chrono::TimeZone;
use rusqlite::Connection;
use serde_json::{json, Value};

use super::*;
use crate::automations::definition::RoutineDefinition;
use crate::automations::store::{insert_definition, insert_definition_at};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 22, 10, 0, 0).unwrap()
}

fn definition(id: &str, status: &str) -> RoutineDefinition {
    RoutineDefinition::from_json(&json!({
        "schemaVersion": 1,
        "id": id,
        "name": id,
        "status": status,
        "rrule": "FREQ=DAILY;BYHOUR=9",
        "timezone": "utc",
        "misfire": "latest",
        "overlap": "forbid",
        "timeoutMinutes": 30,
        "runtime": "coven-code",
        "cwd": "/work/project",
        "prompt": "PRIVATE-PROMPT"
    }))
    .unwrap()
}

fn store() -> (tempfile::TempDir, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store.sqlite");
    crate::store::initialize_store(&path).unwrap();
    let conn = crate::store::open_store(&path).unwrap();
    (temp, conn)
}

fn quarantine(conn: &Connection, id: &str) {
    crate::automations::runs::record_retry_exhaustion(
        conn,
        id,
        "launch_refused",
        "PRIVATE-REASON",
        Utc.with_ymd_and_hms(2026, 10, 21, 9, 0, 0).unwrap(),
    )
    .unwrap();
}

fn found(conn: &Connection, id: &str) -> Explanation {
    match explain(conn, id, now()).unwrap() {
        Lookup::Found(explanation) => explanation,
        Lookup::NotFound => panic!("{id} not found"),
        Lookup::Tombstoned => panic!("{id} tombstoned"),
    }
}

fn codes(explanation: &Explanation) -> Vec<Code> {
    explanation
        .conditions
        .iter()
        .map(|condition| condition.code)
        .collect()
}

/// One routine per shape the scheduler distinguishes, with the conditions
/// each must report. Ids carry no private marker.
fn seed(conn: &Connection) -> Vec<(&'static str, Vec<Code>)> {
    for (id, status) in [
        ("active", "ACTIVE"),
        ("paused", "PAUSED"),
        ("disabled", "DISABLED"),
        ("draft", "PAUSED"),
        ("quarantined", "ACTIVE"),
        ("paused-quarantined", "PAUSED"),
        ("unreadable", "ACTIVE"),
        ("unreadable-paused-quarantined", "PAUSED"),
        ("divergent", "PAUSED"),
        ("lowercase", "ACTIVE"),
        ("no-digest", "ACTIVE"),
        ("stale-digest", "ACTIVE"),
        ("paused-stale-digest", "PAUSED"),
        ("foreign-id", "ACTIVE"),
        ("unreadable-created", "ACTIVE"),
        ("unreadable-updated", "ACTIVE"),
    ] {
        insert_definition_at(conn, &definition(id, status), "2026-10-01T00:00:00.000Z").unwrap();
    }
    conn.execute_batch(
        "UPDATE automation_definitions SET status = 'DRAFT', lifecycle_state = 'draft'
             WHERE id = 'draft';
         UPDATE automation_definitions SET definition_json = '{\"PRIVATE\":1}'
             WHERE id IN ('unreadable', 'unreadable-paused-quarantined');
         -- The stored status says ACTIVE; the definition itself says PAUSED.
         UPDATE automation_definitions SET status = 'ACTIVE', lifecycle_state = 'active'
             WHERE id = 'divergent';
         UPDATE automation_definitions SET status = 'active' WHERE id = 'lowercase';
         -- Revision metadata planning verifies before it fences a slot.
         UPDATE automation_definitions SET definition_digest = NULL WHERE id = 'no-digest';
         UPDATE automation_definitions SET definition_digest = printf('%064d', 0)
             WHERE id IN ('stale-digest', 'paused-stale-digest');
         UPDATE automation_definitions SET (definition_json, definition_digest) =
             (SELECT definition_json, definition_digest FROM automation_definitions
              WHERE id = 'active')
             WHERE id = 'foreign-id';
         UPDATE automation_definitions SET created_at = 'PRIVATE' WHERE id = 'unreadable-created';
         UPDATE automation_definitions SET updated_at = 'PRIVATE' WHERE id = 'unreadable-updated';",
    )
    .unwrap();
    for id in [
        "quarantined",
        "paused-quarantined",
        "unreadable-paused-quarantined",
    ] {
        quarantine(conn, id);
    }
    use Code::*;
    vec![
        ("active", vec![NextSlot]),
        ("paused", vec![NotActive]),
        ("disabled", vec![NotActive]),
        ("draft", vec![NotActive]),
        ("quarantined", vec![Quarantined]),
        ("paused-quarantined", vec![NotActive, Quarantined]),
        ("unreadable", vec![DefinitionUnavailable]),
        (
            "unreadable-paused-quarantined",
            vec![NotActive, Quarantined, DefinitionUnavailable],
        ),
        ("divergent", vec![NotActive]),
        ("lowercase", vec![NotActive]),
        ("no-digest", vec![DefinitionUnavailable]),
        ("stale-digest", vec![DefinitionUnavailable]),
        // Planning never reaches the revision check of a paused routine.
        ("paused-stale-digest", vec![NotActive]),
        ("foreign-id", vec![DefinitionUnavailable]),
        ("unreadable-created", vec![DefinitionUnavailable]),
        ("unreadable-updated", vec![DefinitionUnavailable]),
    ]
}

#[test]
fn an_active_routine_reports_only_its_next_slot() {
    let (_temp, conn) = store();
    insert_definition(&conn, &definition("daily", "ACTIVE")).unwrap();
    let before = conn.total_changes();
    let explanation = found(&conn, "daily");
    assert_eq!(conn.total_changes(), before);
    assert_eq!(
        serde_json::to_value(&explanation).unwrap(),
        json!({
            "automationId": "daily",
            "revision": 1,
            "lifecycleState": "active",
            "observedAt": "2026-10-22T10:00:00.000Z",
            "conditions": [{
                "code": "next_slot",
                "at": "2026-10-23T09:00:00.000Z",
                "nextStep": "none"
            }],
            "work": [],
            "workTruncated": false
        })
    );
}

#[test]
fn every_blocking_condition_is_listed_in_order_and_hides_the_slot() {
    let (_temp, conn) = store();
    for (id, expected) in seed(&conn) {
        let explanation = found(&conn, id);
        assert_eq!(codes(&explanation), expected, "{id}");
        for condition in &explanation.conditions {
            let (step, at) = match condition.code {
                Code::NotActive => (NextStep::ActivateIfIntended, None),
                Code::Quarantined => (
                    NextStep::ReviewFailureThenUnquarantine,
                    Some("2026-10-21T09:00:00.000Z"),
                ),
                Code::DefinitionUnavailable => (NextStep::ReviseDefinition, None),
                Code::NextSlot => (NextStep::None, Some("2026-10-23T09:00:00.000Z")),
            };
            assert_eq!(condition.next_step, step, "{id}");
            assert_eq!(condition.at.as_deref(), at, "{id}");
            assert_eq!(
                condition.failure_class.as_deref(),
                (condition.code == Code::Quarantined).then_some("launch_refused"),
                "{id}"
            );
        }
    }
    assert_eq!(found(&conn, "draft").lifecycle_state, "draft");
    assert_eq!(found(&conn, "paused").lifecycle_state, "paused");
}

#[test]
fn planning_at_each_reported_slot_schedules_exactly_the_explained_routines() {
    let (_temp, conn) = store();
    let cases = seed(&conn);
    let mut slots = Vec::new();
    for (id, _) in &cases {
        if let [Condition {
            code: Code::NextSlot,
            at: Some(at),
            ..
        }] = found(&conn, id).conditions.as_slice()
        {
            slots.push((id.to_string(), at.clone()));
        }
    }
    assert_eq!(slots.len(), 1);
    // `active_definition_ids` admits routines whose revision metadata
    // planning then rejects, so only a planning pass is compared.
    // The production planning page, run at the reported slot.
    let at = DateTime::parse_from_rfc3339(&slots[0].1).unwrap();
    crate::automations::occurrences::tick_planning(&conn, at.with_timezone(&Utc)).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT automation_id, scheduled_for FROM automation_occurrences
             WHERE kind = 'scheduled' ORDER BY automation_id",
        )
        .unwrap();
    let planned = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<(String, String)>>>()
        .unwrap();
    assert_eq!(planned, slots);
}

#[test]
fn private_text_and_malformed_quarantine_metadata_never_leave() {
    let (_temp, conn) = store();
    let cases = seed(&conn);
    // A forged class or timestamp is omitted; the quarantine still counts.
    conn.execute_batch(
        "PRAGMA ignore_check_constraints = ON;
         UPDATE automation_retry_state
            SET failure_class = 'PRIVATE class', quarantined_at = 'PRIVATE'
          WHERE automation_id = 'quarantined';
         PRAGMA ignore_check_constraints = OFF;",
    )
    .unwrap();
    for (id, _) in &cases {
        let text = serde_json::to_string(&found(&conn, id)).unwrap();
        assert!(!text.contains("PRIVATE"), "{id}: {text}");
    }
    let forged = found(&conn, "quarantined");
    assert_eq!(codes(&forged), [Code::Quarantined]);
    assert_eq!(forged.conditions[0].at, None);
    assert_eq!(forged.conditions[0].failure_class, None);
}

#[test]
fn quarantine_failure_classes_are_bounded_tokens() {
    for (class, kept) in [
        ("launch_refused", true),
        ("runtime_authority_unsupported", true),
        ("", false),
        ("Launch_refused", false),
        ("launch refused", false),
        ("launch-refused", false),
        ("launch_refused\n", false),
    ] {
        assert_eq!(bounded_token(class), kept, "{class:?}");
    }
    assert!(bounded_token(&"a".repeat(64)));
    assert!(!bounded_token(&"a".repeat(65)));
    assert_eq!(
        canonical("2026-10-21T11:00:00+02:00").as_deref(),
        Some("2026-10-21T09:00:00.000Z")
    );
    assert_eq!(canonical("2026-10-21 09:00:00"), None);
}

#[test]
fn temporary_tables_cannot_shadow_the_explained_routine() {
    let (_temp, conn) = store();
    insert_definition(&conn, &definition("daily", "ACTIVE")).unwrap();
    conn.execute_batch(
        "CREATE TEMP TABLE automation_definitions AS SELECT * FROM main.automation_definitions;
         UPDATE temp.automation_definitions SET status = 'PAUSED';
         CREATE TEMP TABLE automation_retry_state AS SELECT * FROM main.automation_retry_state;
         INSERT INTO temp.automation_retry_state
             (automation_id, consecutive_exhaustions, quarantined_at, failure_class, reason, updated_at)
             VALUES ('daily', 1, '2026-10-21T09:00:00.000Z', 'launch_refused', 'x', '2026-10-21T09:00:00.000Z');
         CREATE TEMP TABLE automation_missions AS SELECT * FROM main.automation_missions;
         INSERT INTO temp.automation_missions (mission_id, automation_id) VALUES ('m', 'daily');",
    )
    .unwrap();
    assert_eq!(codes(&found(&conn, "daily")), [Code::NextSlot]);
}

#[test]
fn unknown_tombstoned_and_mission_routines_are_not_explained() {
    let (_temp, conn) = store();
    insert_definition(&conn, &definition("gone", "PAUSED")).unwrap();
    insert_definition(&conn, &definition("mission", "PAUSED")).unwrap();
    conn.execute_batch(
        "UPDATE automation_definitions SET tombstoned_at = '2026-10-21T09:00:00.000Z'
             WHERE id = 'gone';
         PRAGMA foreign_keys = OFF;
         INSERT INTO automation_missions
             (mission_id, automation_id, occurrence_id, adoption_key, request_digest, authority,
              principal_id, familiar_root_id, familiar_revision_id, envelope_id,
              envelope_descriptor_digest, project_cwd, statement, timeout_minutes,
              submitted_at, valid_until)
         VALUES ('m', 'mission', 'o', 'k', printf('%064d', 0), 'owner_local', 'p', 'f', 'r',
                 'e', printf('%064d', 0), '/work', 's', 5,
                 '2026-10-21T09:00:00.000Z', '2026-10-22T09:00:00.000Z');
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    assert!(matches!(
        explain(&conn, "gone", now()).unwrap(),
        Lookup::Tombstoned
    ));
    for id in ["mission", "missing"] {
        assert!(
            matches!(explain(&conn, id, now()).unwrap(), Lookup::NotFound),
            "{id}"
        );
    }
}

#[test]
fn the_daemon_action_returns_the_explanation_or_a_typed_refusal() {
    let (_temp, conn) = store();
    insert_definition(&conn, &definition("daily", "PAUSED")).unwrap();
    insert_definition(&conn, &definition("gone", "PAUSED")).unwrap();
    conn.execute(
        "UPDATE automation_definitions SET tombstoned_at = '2026-10-21T09:00:00.000Z'
         WHERE id = 'gone'",
        [],
    )
    .unwrap();
    let route = |body: Value| {
        let (status, response) =
            crate::control_plane::route_action(body, &conn, &crate::api::NoopSessionRuntime);
        (status, serde_json::to_value(response).unwrap())
    };

    let (status, response) = route(json!({"action": ACTION, "id": "daily"}));
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["event"]["action"], ACTION);
    let explanation = &response["event"]["payload"]["explanation"];
    assert_eq!(explanation["automationId"], "daily");
    assert_eq!(
        explanation["conditions"],
        json!([{"code": "not_active", "nextStep": "activate_if_intended"}])
    );
    assert!(!response.to_string().contains("PRIVATE"));

    for (body, expected) in [
        (json!({"action": ACTION, "id": "missing"}), "NOT_FOUND"),
        (json!({"action": ACTION, "id": "gone"}), "GONE_TOMBSTONED"),
        (json!({"action": ACTION}), "VALIDATION_FAILED"),
        (json!({"action": ACTION, "id": "  "}), "VALIDATION_FAILED"),
    ] {
        let (status, response) = route(body.clone());
        assert_ne!(status, 200, "{body}");
        assert_eq!(response["error"]["code"], expected, "{body}: {response}");
    }
}

fn occurrence(conn: &Connection, id: &str, routine: &str, day: u32, state: &str) {
    conn.execute(
        "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, state, attempt, lease_owner, failure_reason,
             created_at, updated_at)
         VALUES (?1, ?2, printf('2026-10-%02dT09:00:00.000Z', ?3), ?4, 1, 'PRIVATE-OWNER',
                 'PRIVATE-REASON', '2026-10-01T00:00:00.000Z', '2026-10-01T00:00:00.000Z')",
        rusqlite::params![id, routine, day, state],
    )
    .unwrap();
}

fn run(conn: &Connection, id: &str, occurrence: &str, status: &str) {
    conn.execute(
        "INSERT INTO automation_runs
            (id, automation_id, occurrence_id, runtime, status, started_at, log_json)
         SELECT ?1, automation_id, id, 'coven-code', ?3, '2026-10-21T09:00:00.000Z', 'PRIVATE-LOG'
         FROM automation_occurrences WHERE id = ?2",
        rusqlite::params![id, occurrence, status],
    )
    .unwrap();
}

fn run_keyed(
    conn: &Connection,
    id: &str,
    routine: &str,
    occurrence: Option<&str>,
    status: &str,
    authority: bool,
) {
    conn.execute(
        "INSERT INTO automation_runs
            (id, automation_id, occurrence_id, authority_profile, runtime, status, started_at,
             log_json)
         VALUES (?1, ?2, ?3, CASE WHEN ?5 THEN 'coven.automations.authority.v1' END,
                 'coven-code', ?4, '2026-10-21T09:00:00.000Z', 'PRIVATE-LOG')",
        rusqlite::params![id, routine, occurrence, status, authority],
    )
    .unwrap();
}

fn session(conn: &Connection, id: &str, status: &str) {
    conn.execute(
        "INSERT INTO sessions (id, project_root, harness, title, status, created_at, updated_at)
         VALUES (?1, '/PRIVATE', 'codex', 'PRIVATE', ?2, '2026-10-21T09:00:00.000Z',
                 '2026-10-21T09:00:00.000Z')",
        rusqlite::params![id, status],
    )
    .unwrap();
}

fn attempt(
    conn: &Connection,
    id: &str,
    run: &str,
    number: i64,
    state: &str,
    session: Option<&str>,
) {
    conn.execute(
        "INSERT INTO automation_attempts
            (id, run_id, occurrence_id, attempt_number, adoption_key,
             occurrence_fence_generation, state, prior_attempt_number, prior_disposition,
             retry_classification, not_before, session_id, state_reason, opened_at)
         SELECT ?1, ?2, occurrence_id, ?3, 'PRIVATE-' || ?1, 1, ?4,
                CASE WHEN ?3 > 1 THEN ?3 - 1 END, CASE WHEN ?3 > 1 THEN 'failed' END,
                CASE WHEN ?3 > 1 THEN 'automatic_retry' ELSE 'initial' END,
                '2026-10-21T09:00:00.000Z', ?5, 'PRIVATE-REASON', '2026-10-21T09:00:00.000Z'
         FROM automation_runs WHERE id = ?2",
        rusqlite::params![id, run, number, state, session],
    )
    .unwrap();
}

fn cancellation(conn: &Connection, run: &str, attempt: &str, state: &str) {
    conn.execute(
        "INSERT INTO automation_cancellations
            (adoption_key, request_digest, automation_id, run_id, attempt_id, session_id, scope,
             requested_by_json, reason, state, requested_at, execution_expires_at)
         SELECT 'PRIVATE-' || ?2, 'PRIVATE', automation_id, ?1, ?2, 'PRIVATE-SESSION', 'run',
                '{}', 'PRIVATE-REASON', ?3, '2026-10-21T09:00:00.000Z',
                '2026-10-21T10:00:00.000Z'
         FROM automation_runs WHERE id = ?1",
        rusqlite::params![run, attempt, state],
    )
    .unwrap();
}

fn work_of(conn: &Connection, id: &str) -> Value {
    serde_json::to_value(&found(conn, id).work).unwrap()
}

#[test]
fn work_lists_the_occurrences_the_claim_gate_counts_and_no_others() {
    let (_temp, conn) = store();
    for id in ["daily", "other"] {
        insert_definition_at(&conn, &definition(id, "ACTIVE"), "2026-10-01T00:00:00.000Z").unwrap();
    }
    for (id, day, state) in [
        ("o-planned", 21, "planned"),
        ("o-claimed", 18, "claimed"),
        ("o-running", 19, "running"),
        ("o-recovery", 16, "recovery_required"),
        ("o-recovery-settled", 17, "recovery_required"),
        ("o-failed", 15, "failed"),
        ("o-succeeded", 14, "succeeded"),
        ("o-skipped", 13, "skipped"),
    ] {
        occurrence(&conn, id, "daily", day, state);
    }
    occurrence(&conn, "o-other", "other", 18, "claimed");
    run(&conn, "r-running", "o-running", "running");
    run(&conn, "r-recovery", "o-recovery", "running");
    run(&conn, "r-settled", "o-recovery-settled", "failed");
    run(&conn, "r-failed", "o-failed", "failed");
    run_keyed(
        &conn,
        "r-retry",
        "daily",
        Some("o-planned"),
        "running",
        false,
    );
    run_keyed(&conn, "r-orphan", "daily", None, "running", false);
    run_keyed(
        &conn,
        "r-foreign",
        "other",
        Some("o-planned"),
        "running",
        false,
    );

    let before = conn.total_changes();
    let explanation = found(&conn, "daily");
    assert_eq!(conn.total_changes(), before);
    assert!(!explanation.work_truncated);
    assert_eq!(
        serde_json::to_value(&explanation.work).unwrap(),
        json!([
            {"code": "in_flight", "occurrenceId": "o-claimed", "state": "claimed",
             "at": "2026-10-18T09:00:00.000Z", "holdsClaims": true, "nextStep": "inspect_occurrence"},
            {"code": "in_flight", "occurrenceId": "o-running", "state": "running",
             "runId": "r-running", "at": "2026-10-19T09:00:00.000Z", "holdsClaims": true,
             "nextStep": "inspect_occurrence"},
            {"code": "running_run", "runId": "r-orphan", "holdsClaims": true,
             "nextStep": "inspect_run"},
            {"code": "running_run", "occurrenceId": "o-planned", "runId": "r-retry",
             "at": "2026-10-21T09:00:00.000Z", "holdsClaims": true, "nextStep": "inspect_run"},
            {"code": "recovery_required", "occurrenceId": "o-recovery", "runId": "r-recovery",
             "at": "2026-10-16T09:00:00.000Z", "holdsClaims": true, "nextStep": "recover_occurrence"},
            {"code": "recovery_required", "occurrenceId": "o-recovery-settled",
             "at": "2026-10-17T09:00:00.000Z", "holdsClaims": false,
             "nextStep": "recover_occurrence"}
        ])
    );
    assert!(!serde_json::to_string(&explanation)
        .unwrap()
        .contains("PRIVATE"));
    // Work and planning conditions are independent: pausing stops neither.
    conn.execute(
        "UPDATE automation_definitions SET status = 'PAUSED' WHERE id = 'daily'",
        [],
    )
    .unwrap();
    let paused = found(&conn, "daily");
    assert_eq!(codes(&paused), [Code::NotActive]);
    assert_eq!(paused.work, explanation.work);
}

/// (id, routine, state), scheduled the day before the due occurrence.
type OccurrenceRow = (&'static str, &'static str, &'static str);
/// (id, routine the run is keyed to, its occurrence, status).
type RunRow = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
);

/// One store shape around `o-due`, a due planned occurrence of `daily`.
struct Shape {
    name: &'static str,
    occurrences: &'static [OccurrenceRow],
    runs: &'static [RunRow],
    /// Each run's adopted attempt waits until this instant.
    adopted_not_before: Option<&'static str>,
    holds: bool,
    /// The occurrence the scheduler claims, from the eligible queue too.
    claimed: Option<&'static str>,
}

const SHAPES: &[Shape] = &[
    Shape {
        name: "idle",
        occurrences: &[],
        runs: &[],
        adopted_not_before: None,
        holds: false,
        claimed: Some("o-due"),
    },
    Shape {
        name: "claimed",
        occurrences: &[("o-held", "daily", "claimed")],
        runs: &[],
        adopted_not_before: None,
        holds: true,
        claimed: None,
    },
    Shape {
        name: "running",
        occurrences: &[("o-held", "daily", "running")],
        runs: &[("r-held", "daily", Some("o-held"), "running")],
        adopted_not_before: None,
        holds: true,
        claimed: None,
    },
    Shape {
        name: "running-without-run",
        occurrences: &[("o-held", "daily", "running")],
        runs: &[],
        adopted_not_before: None,
        holds: true,
        claimed: None,
    },
    Shape {
        name: "recovery-running",
        occurrences: &[("o-held", "daily", "recovery_required")],
        runs: &[("r-held", "daily", Some("o-held"), "running")],
        adopted_not_before: None,
        holds: true,
        claimed: None,
    },
    Shape {
        name: "recovery-settled",
        occurrences: &[("o-held", "daily", "recovery_required")],
        runs: &[("r-held", "daily", Some("o-held"), "failed")],
        adopted_not_before: None,
        holds: false,
        claimed: Some("o-due"),
    },
    Shape {
        name: "recovery-run-keyed-elsewhere",
        occurrences: &[("o-held", "daily", "recovery_required")],
        runs: &[("r-held", "other", Some("o-held"), "running")],
        adopted_not_before: None,
        holds: false,
        claimed: Some("o-due"),
    },
    Shape {
        name: "run-without-occurrence",
        occurrences: &[],
        runs: &[("r-held", "daily", None, "running")],
        adopted_not_before: None,
        holds: true,
        claimed: None,
    },
    Shape {
        name: "run-on-settled-occurrence",
        occurrences: &[("o-held", "daily", "succeeded")],
        runs: &[("r-held", "daily", Some("o-held"), "running")],
        adopted_not_before: None,
        holds: true,
        claimed: None,
    },
    Shape {
        name: "retry-on-planned-occurrence",
        occurrences: &[("o-held", "daily", "planned")],
        runs: &[("r-held", "daily", Some("o-held"), "running")],
        adopted_not_before: None,
        holds: true,
        claimed: Some("o-held"),
    },
    Shape {
        name: "ready-retry-on-due-occurrence",
        occurrences: &[],
        runs: &[("r-held", "daily", Some("o-due"), "running")],
        adopted_not_before: Some("2026-10-22T09:30:00.000Z"),
        holds: true,
        claimed: Some("o-due"),
    },
    Shape {
        name: "waiting-retry-on-due-occurrence",
        occurrences: &[],
        runs: &[("r-held", "daily", Some("o-due"), "running")],
        adopted_not_before: Some("2026-10-22T11:00:00.000Z"),
        holds: true,
        claimed: None,
    },
    Shape {
        name: "waiting-retry-on-planned-occurrence",
        occurrences: &[("o-held", "daily", "planned")],
        runs: &[("r-held", "daily", Some("o-held"), "running")],
        adopted_not_before: Some("2026-10-22T11:00:00.000Z"),
        holds: true,
        claimed: None,
    },
    Shape {
        name: "run-on-another-routines-occurrence",
        occurrences: &[("o-held", "other", "claimed")],
        runs: &[("r-held", "daily", Some("o-held"), "running")],
        adopted_not_before: None,
        holds: true,
        claimed: None,
    },
    Shape {
        name: "another-routine-claimed",
        occurrences: &[("o-held", "other", "claimed")],
        runs: &[],
        adopted_not_before: None,
        holds: false,
        claimed: Some("o-due"),
    },
];

#[test]
fn work_holds_claims_exactly_when_the_scheduler_will_not_claim_a_due_occurrence() {
    for shape in SHAPES {
        let name = shape.name;
        let (_temp, conn) = store();
        for id in ["daily", "other"] {
            insert_definition_at(&conn, &definition(id, "ACTIVE"), "2026-10-01T00:00:00.000Z")
                .unwrap();
        }
        occurrence(&conn, "o-due", "daily", 21, "planned");
        for &(id, routine, state) in shape.occurrences {
            occurrence(&conn, id, routine, 20, state);
        }
        for &(id, routine, occurrence, status) in shape.runs {
            run_keyed(&conn, id, routine, occurrence, status, false);
            if let Some(not_before) = shape.adopted_not_before {
                conn.execute(
                    "INSERT INTO automation_attempts
                        (id, run_id, occurrence_id, attempt_number, adoption_key,
                         occurrence_fence_generation, state, retry_classification,
                         not_before, opened_at)
                     VALUES (?1 || '-attempt', ?1, ?2, 1, ?1 || '-key', 1, 'adopted',
                             'initial', ?3, '2026-10-21T09:00:00.000Z')",
                    rusqlite::params![id, occurrence, not_before],
                )
                .unwrap();
            }
        }
        let work = found(&conn, "daily").work;
        let holds = work.iter().any(|item| item.holds_claims);
        assert_eq!(holds, shape.holds, "{name}");
        // Both production paths: the queue the scheduler claims from, and
        // the claim itself.
        let eligible: Vec<_> =
            crate::automations::occurrences::eligible_occurrences(&conn, now(), 100)
                .unwrap()
                .into_iter()
                .filter(|eligible| eligible.automation_id == "daily")
                .map(|eligible| eligible.id)
                .collect();
        assert_eq!(eligible, Vec::from_iter(shape.claimed), "{name}: eligible");
        let claimed = crate::automations::occurrences::claim_due_occurrence(
            &conn,
            "daily",
            "daemon",
            60,
            now(),
        )
        .unwrap();
        assert_eq!(claimed.as_deref(), shape.claimed, "{name}: claimed");
        // The documented rule: with nothing holding claims the due occurrence
        // is claimed; otherwise only a running run's own occurrence can be.
        match claimed.as_deref() {
            _ if !holds => assert_eq!(claimed.as_deref(), Some("o-due"), "{name}"),
            None => {}
            Some(own) => assert!(
                work.iter().any(|item| item.code == WorkCode::RunningRun
                    && item.occurrence_id.as_deref() == Some(own)),
                "{name}: {own} is no listed run's occurrence"
            ),
        }
    }
}

#[test]
fn each_recovery_item_names_the_step_recovery_takes() {
    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    let recovery = |id: &str, day: u32, authority: bool| {
        occurrence(&conn, &format!("o-{id}"), "daily", day, "recovery_required");
        run_keyed(
            &conn,
            &format!("r-{id}"),
            "daily",
            Some(&format!("o-{id}")),
            "running",
            authority,
        );
    };
    // Each refusal outranks the ones after it, as in recovery.
    recovery("authority", 11, true);
    attempt(&conn, "a-authority", "r-authority", 1, "started", None);
    cancellation(&conn, "r-authority", "a-authority", "requested");
    recovery("cancelling", 12, false);
    attempt(&conn, "a-cancelling", "r-cancelling", 1, "started", None);
    cancellation(&conn, "r-cancelling", "a-cancelling", "stopping");
    recovery("live-attempt", 13, false);
    attempt(
        &conn,
        "a-live-attempt",
        "r-live-attempt",
        1,
        "observing",
        None,
    );
    recovery("live-session", 14, false);
    session(&conn, "PRIVATE-SESSION", "running");
    attempt(
        &conn,
        "a-live-session",
        "r-live-session",
        1,
        "ambiguous",
        Some("PRIVATE-SESSION"),
    );
    // Only the latest attempt and an unresolved cancellation count.
    recovery("settled", 15, false);
    attempt(&conn, "a-settled-1", "r-settled", 1, "started", None);
    attempt(&conn, "a-settled-2", "r-settled", 2, "ambiguous", None);
    cancellation(&conn, "r-settled", "a-settled-1", "cancelled");
    recovery("plain", 16, false);
    run_keyed(&conn, "r-authority-orphan", "daily", None, "running", true);

    let explanation = found(&conn, "daily");
    let rendered = serde_json::to_string(&explanation).unwrap();
    assert!(!rendered.contains("PRIVATE") && !rendered.contains("authority.v1"));
    assert_eq!(explanation.work[0].code, WorkCode::RunningRun);
    assert_eq!(explanation.work[0].next_step, NextStep::InspectRun);
    let recovering: Vec<_> = explanation
        .work
        .iter()
        .filter(|item| item.code == WorkCode::RecoveryRequired)
        .collect();
    assert_eq!(
        recovering
            .iter()
            .map(|item| (item.occurrence_id.as_deref().unwrap(), item.next_step))
            .collect::<Vec<_>>(),
        [
            ("o-authority", NextStep::AwaitRuntimeAuthority),
            ("o-cancelling", NextStep::AwaitCancellation),
            ("o-live-attempt", NextStep::CancelRun),
            ("o-live-session", NextStep::CancelRun),
            ("o-settled", NextStep::RecoverOccurrence),
            ("o-plain", NextStep::RecoverOccurrence),
        ]
    );
    for item in recovering {
        let occurrence_id = item.occurrence_id.as_deref().unwrap();
        let execution = crate::automations::recovery::execute_occurrence_recovery(
            &conn,
            json!({
                "action": crate::automations::recovery::RECOVER_ACTION,
                "adoptionKey": format!("adopt:recover:{occurrence_id}"),
                "occurrenceId": occurrence_id,
                "evidenceDetermination": "failed_deterministic"
            }),
            now(),
        )
        .unwrap();
        let refusal = match execution {
            crate::automations::recovery::RecoveryExecution::Rejected(envelope) => {
                serde_json::to_value(&envelope).unwrap()
            }
            crate::automations::recovery::RecoveryExecution::Success { .. } => Value::Null,
        };
        let refused = |code: &str| refusal["code"] == json!(code);
        assert!(
            match item.next_step {
                NextStep::RecoverOccurrence => refusal.is_null(),
                NextStep::AwaitRuntimeAuthority => refused("AUTHORITY_REQUIRED"),
                NextStep::AwaitCancellation => refused("CANCEL_PENDING"),
                NextStep::CancelRun =>
                    refused("ILLEGAL_TRANSITION")
                        && refusal["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("cancel it before recovering")),
                other => panic!("{occurrence_id}: {other:?}"),
            },
            "{occurrence_id}: {refusal}"
        );
    }
}

#[test]
fn unreadable_scheduled_times_are_omitted_without_failing_the_explanation() {
    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    for (id, day) in [
        ("o-not-utf8", 1),
        ("o-blob", 2),
        ("o-huge", 3),
        ("o-overlong", 4),
        ("o-readable", 5),
        ("o-longest-readable", 6),
    ] {
        occurrence(&conn, id, "daily", day, "claimed");
    }
    // 65 bytes whose first 64 are a readable time, and a readable 64 bytes.
    let overlong = format!("2026-10-18T09:00:00.{}ZX", "0".repeat(43));
    let longest = format!("2026-10-19T09:00:00.{}Z", "0".repeat(43));
    assert_eq!((overlong.len(), longest.len()), (65, 64));
    assert!(canonical(&overlong[..64]).is_some());
    conn.execute_batch(&format!(
        "UPDATE automation_occurrences SET scheduled_for = CAST(x'ff' AS TEXT)
             WHERE id = 'o-not-utf8';
         UPDATE automation_occurrences SET scheduled_for = x'00ff' WHERE id = 'o-blob';
         UPDATE automation_occurrences SET scheduled_for = 'PRIVATE{}' WHERE id = 'o-huge';
         UPDATE automation_occurrences SET scheduled_for = '{overlong}' WHERE id = 'o-overlong';
         UPDATE automation_occurrences SET scheduled_for = '{longest}'
             WHERE id = 'o-longest-readable';",
        "x".repeat(10_000)
    ))
    .unwrap();
    let explanation = found(&conn, "daily");
    let mut times: Vec<_> = explanation
        .work
        .iter()
        .map(|item| (item.occurrence_id.clone().unwrap(), item.at.clone()))
        .collect();
    times.sort();
    assert_eq!(
        times,
        [
            ("o-blob".to_owned(), None),
            ("o-huge".to_owned(), None),
            (
                "o-longest-readable".to_owned(),
                Some("2026-10-19T09:00:00.000Z".to_owned())
            ),
            ("o-not-utf8".to_owned(), None),
            ("o-overlong".to_owned(), None),
            (
                "o-readable".to_owned(),
                Some("2026-10-05T09:00:00.000Z".to_owned())
            ),
        ]
    );
    assert!(!serde_json::to_string(&explanation)
        .unwrap()
        .contains("PRIVATE"));
}

#[test]
fn work_that_holds_claims_comes_first_even_when_truncated() {
    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    for day in 1..=21 {
        occurrence(
            &conn,
            &format!("o-{day:02}"),
            "daily",
            day,
            "recovery_required",
        );
    }
    occurrence(&conn, "o-late", "daily", 28, "recovery_required");
    run_keyed(&conn, "r-late", "daily", Some("o-late"), "running", false);
    // Two running runs on one occurrence: the lowest id is named.
    occurrence(&conn, "o-pair", "daily", 27, "running");
    run_keyed(&conn, "r-pair-b", "daily", Some("o-pair"), "running", false);
    run_keyed(&conn, "r-pair-a", "daily", Some("o-pair"), "running", false);
    let explanation = found(&conn, "daily");
    assert!(explanation.work_truncated);
    assert_eq!(explanation.work.len(), MAX_WORK);
    let named: Vec<_> = explanation.work[..2]
        .iter()
        .map(|item| {
            (
                item.code,
                item.occurrence_id.as_deref(),
                item.run_id.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            (WorkCode::InFlight, Some("o-pair"), Some("r-pair-a")),
            (WorkCode::RecoveryRequired, Some("o-late"), Some("r-late")),
        ]
    );
    assert!(explanation.work[..2].iter().all(|item| item.holds_claims));
    assert!(explanation.work[2..].iter().all(|item| !item.holds_claims));
    assert_eq!(explanation.work[2].occurrence_id.as_deref(), Some("o-01"));
}

#[test]
fn work_is_bounded_and_unreadable_ids_are_omitted() {
    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    for day in 1..=MAX_WORK as u32 {
        occurrence(
            &conn,
            &format!("o-{day:02}"),
            "daily",
            day,
            "recovery_required",
        );
    }
    assert!(!found(&conn, "daily").work_truncated);
    assert_eq!(found(&conn, "daily").work.len(), MAX_WORK);
    occurrence(&conn, "o-extra", "daily", 28, "recovery_required");
    let explanation = found(&conn, "daily");
    assert!(explanation.work_truncated);
    assert_eq!(explanation.work.len(), MAX_WORK);
    assert_eq!(explanation.work[0].occurrence_id.as_deref(), Some("o-01"));

    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    occurrence(&conn, "PRIVATE\u{7}", "daily", 1, "claimed");
    occurrence(
        &conn,
        &format!("PRIVATE{}", "x".repeat(600)),
        "daily",
        2,
        "claimed",
    );
    occurrence(&conn, "o-blob", "daily", 3, "claimed");
    occurrence(&conn, "o-run", "daily", 4, "running");
    run(&conn, "PRIVATE\n", "o-run", "running");
    // The same rules for a running run's own row and its unlisted occurrence.
    occurrence(&conn, "PRIVATE\u{2}", "daily", 5, "succeeded");
    run_keyed(
        &conn,
        "PRIVATE\u{3}",
        "daily",
        Some("PRIVATE\u{2}"),
        "running",
        false,
    );
    run_keyed(
        &conn,
        &format!("PRIVATE{}", "y".repeat(600)),
        "daily",
        None,
        "running",
        false,
    );
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         UPDATE automation_occurrences SET id = CAST(x'ff50524956415445' AS BLOB),
             scheduled_for = 'PRIVATE'
             WHERE id = 'o-blob';
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    let work = found(&conn, "daily").work;
    assert_eq!(work.len(), 6);
    assert!(work.iter().all(|item| item.holds_claims));
    let runs: Vec<_> = work
        .iter()
        .filter(|item| item.code == WorkCode::RunningRun)
        .collect();
    assert_eq!(runs.len(), 2);
    assert!(runs
        .iter()
        .all(|item| item.run_id.is_none() && item.occurrence_id.is_none()));
    let (unnamed, named): (Vec<_>, Vec<_>) =
        work.iter().partition(|item| item.occurrence_id.is_none());
    assert_eq!(unnamed.len(), 5);
    assert_eq!(unnamed.iter().filter(|item| item.at.is_none()).count(), 2);
    assert_eq!(named[0].occurrence_id.as_deref(), Some("o-run"));
    assert_eq!(named[0].run_id, None);
    assert!(!serde_json::to_string(&work).unwrap().contains("PRIVATE"));
}

#[test]
fn temporary_tables_cannot_add_or_hide_work() {
    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    occurrence(&conn, "o-running", "daily", 19, "running");
    run(&conn, "r-running", "o-running", "running");
    // Its claim hold rests only on the run, so a shadowed run read would show.
    occurrence(&conn, "o-recovery", "daily", 18, "recovery_required");
    run(&conn, "r-recovery", "o-recovery", "running");
    run_keyed(&conn, "r-orphan", "daily", None, "running", false);
    // Recovery's refusals are read from three more tables.
    occurrence(&conn, "o-cancelling", "daily", 16, "recovery_required");
    run(&conn, "r-cancelling", "o-cancelling", "running");
    attempt(&conn, "a-cancelling", "r-cancelling", 1, "ambiguous", None);
    cancellation(&conn, "r-cancelling", "a-cancelling", "requested");
    occurrence(&conn, "o-live-session", "daily", 17, "recovery_required");
    run(&conn, "r-live-session", "o-live-session", "running");
    session(&conn, "PRIVATE-SESSION", "created");
    attempt(
        &conn,
        "a-live-session",
        "r-live-session",
        1,
        "ambiguous",
        Some("PRIVATE-SESSION"),
    );
    let expected = work_of(&conn, "daily");
    assert_eq!(expected.as_array().unwrap().len(), 5);
    assert_eq!(expected[4]["holdsClaims"], true);
    assert_eq!(
        [&expected[2]["nextStep"], &expected[3]["nextStep"]],
        [&json!("await_cancellation"), &json!("cancel_run")]
    );
    conn.execute_batch(
        "CREATE TEMP TABLE automation_occurrences AS SELECT * FROM main.automation_occurrences;
         UPDATE temp.automation_occurrences SET state = 'succeeded';
         INSERT INTO temp.automation_occurrences (id, automation_id, scheduled_for, state)
             VALUES ('o-shadow', 'daily', '2026-10-20T09:00:00.000Z', 'claimed');
         CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs;
         UPDATE temp.automation_runs SET status = 'failed', id = 'r-shadow';
         CREATE TEMP TABLE automation_cancellations AS
             SELECT * FROM main.automation_cancellations WHERE 0;
         CREATE TEMP TABLE automation_attempts AS
             SELECT * FROM main.automation_attempts WHERE 0;
         CREATE TEMP TABLE sessions AS SELECT * FROM main.sessions WHERE 0;",
    )
    .unwrap();
    assert_eq!(work_of(&conn, "daily"), expected);
}
