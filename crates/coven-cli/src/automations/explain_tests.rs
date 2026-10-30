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
        ("unreadable-history", "ACTIVE"),
        ("paused-unreadable-history", "PAUSED"),
        ("unreadable-created-and-history", "ACTIVE"),
        ("quarantined-unreadable-history", "ACTIVE"),
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
         UPDATE automation_definitions SET updated_at = 'PRIVATE' WHERE id = 'unreadable-updated';
         -- The latest scheduled slot of the current revision, which planning
         -- reads before it plans, is unreadable.
         INSERT INTO automation_occurrences
             (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
              state, attempt, created_at, updated_at)
         SELECT id || '-history', id, revision, definition_digest, 'PRIVATE', 'scheduled',
                'skipped', 1, '2026-10-01T00:00:00.000Z', '2026-10-01T00:00:00.000Z'
         FROM automation_definitions
         WHERE id IN ('unreadable-history', 'paused-unreadable-history',
                      'unreadable-created-and-history', 'quarantined-unreadable-history');
         UPDATE automation_definitions SET created_at = 'PRIVATE'
             WHERE id = 'unreadable-created-and-history';",
    )
    .unwrap();
    for id in [
        "quarantined",
        "paused-quarantined",
        "unreadable-paused-quarantined",
        "quarantined-unreadable-history",
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
        ("unreadable-history", vec![ScheduleHistoryUnreadable]),
        // Nor its schedule history.
        ("paused-unreadable-history", vec![NotActive]),
        // An unreadable creation time does not keep planning from the history.
        (
            "unreadable-created-and-history",
            vec![DefinitionUnavailable, ScheduleHistoryUnreadable],
        ),
        (
            "quarantined-unreadable-history",
            vec![Quarantined, ScheduleHistoryUnreadable],
        ),
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
                Code::DefinitionUnavailable | Code::ScheduleHistoryUnreadable => {
                    (NextStep::ReviseDefinition, None)
                }
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
             WHERE kind = 'scheduled' AND state = 'planned' ORDER BY automation_id",
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
    insert_definition(&conn, &definition("broken", "ACTIVE")).unwrap();
    let unreadable_history = |table: &str, id: &str| {
        format!(
            "INSERT INTO {table}
                 (id, automation_id, automation_revision, definition_digest, scheduled_for,
                  kind, state, attempt, created_at, updated_at)
             SELECT id || '-history', id, revision, definition_digest, 'PRIVATE',
                    'scheduled', 'skipped', 1, created_at, created_at
             FROM main.automation_definitions WHERE id = '{id}';"
        )
    };
    conn.execute_batch(&unreadable_history("main.automation_occurrences", "broken"))
        .unwrap();
    conn.execute_batch(&format!(
        "CREATE TEMP TABLE automation_occurrences AS SELECT * FROM main.automation_occurrences;
         DELETE FROM temp.automation_occurrences;
         {}",
        unreadable_history("temp.automation_occurrences", "daily")
    ))
    .unwrap();
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
    assert_eq!(
        codes(&found(&conn, "broken")),
        [Code::ScheduleHistoryUnreadable]
    );
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
        fence_due_slot(&conn, id);
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

/// Records today's slot as already scheduled, so no slot is due unplanned.
fn fence_due_slot(conn: &Connection, routine: &str) {
    conn.execute(
        "INSERT INTO automation_occurrences
            (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
             state, attempt, created_at, updated_at)
         SELECT id || '-fenced', id, revision, definition_digest, '2026-10-22T09:00:00.000Z',
                'scheduled', 'succeeded', 1, created_at, created_at
         FROM automation_definitions WHERE id = ?1",
        [routine],
    )
    .unwrap();
}

fn planned_occurrence(conn: &Connection, routine: &str) -> String {
    conn.query_row(
        "SELECT id FROM automation_occurrences WHERE automation_id = ?1 AND state = 'planned'",
        [routine],
        |row| row.get(0),
    )
    .unwrap()
}

fn adopted_attempt(conn: &Connection, run: &str, not_before: &str) {
    conn.execute(
        "INSERT INTO automation_attempts
            (id, run_id, occurrence_id, attempt_number, adoption_key,
             occurrence_fence_generation, state, retry_classification, not_before, opened_at)
         SELECT ?1 || '-attempt', ?1, occurrence_id, 1, ?1 || '-key', 1, 'adopted', 'initial',
                ?2, '2026-10-21T09:00:00.000Z'
         FROM automation_runs WHERE id = ?1",
        rusqlite::params![run, not_before],
    )
    .unwrap();
}

#[test]
fn a_scheduler_pass_claims_exactly_the_occurrences_explained_as_eligible_now() {
    let (_temp, conn) = store();
    let routines = [
        "ready",
        "paused-after",
        "quarantined-after",
        "stale-after",
        "unreadable-created-after",
        "unreadable-history-after",
        "held",
        "ready-retry-paused",
        "ready-retry",
        "waiting-retry",
        "divergent-after",
        "foreign-after",
        "revised-after",
        "manual-after",
        "future-after",
        "settled-after",
        "fenced-after",
        "taken-after",
    ];
    for id in routines {
        insert_definition_at(&conn, &definition(id, "ACTIVE"), "2026-10-01T00:00:00.000Z").unwrap();
    }
    // The production planner plans each routine's latest due slot a day
    // before the pass, so the pass fences a newer slot; the store then changes
    // under the planned occurrences.
    crate::automations::occurrences::tick_planning(&conn, now() - chrono::Duration::days(1))
        .unwrap();
    for id in routines {
        assert_eq!(found(&conn, id).work.len(), 1, "{id}");
    }
    conn.execute_batch(
        "UPDATE automation_definitions SET status = 'PAUSED'
             WHERE id IN ('paused-after', 'ready-retry-paused');
         UPDATE automation_definitions SET definition_digest = printf('%064d', 0)
             WHERE id = 'stale-after';
         UPDATE automation_definitions SET created_at = 'PRIVATE'
             WHERE id = 'unreadable-created-after';
         -- Planning's cursor starts at the revision time, after today's slot.
         UPDATE automation_definitions SET updated_at = '2026-10-22T09:30:00.000Z'
             WHERE id = 'revised-after';
         INSERT INTO automation_occurrences
             (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
              state, attempt, created_at, updated_at)
         SELECT id || '-history', id, revision, definition_digest, 'PRIVATE', 'scheduled',
                'skipped', 1, '2026-10-01T00:00:00.000Z', '2026-10-01T00:00:00.000Z'
         FROM automation_definitions WHERE id = 'unreadable-history-after';
         -- Planning loads a definition whose own id names another routine.
         UPDATE automation_definitions SET (definition_json, definition_digest) =
             (SELECT definition_json, definition_digest FROM automation_definitions
              WHERE id = 'ready')
             WHERE id = 'foreign-after';
         -- A recovered manual occurrence, due after today's slot, supersedes it.
         INSERT INTO automation_occurrences
             (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
              state, attempt, created_at, updated_at)
         SELECT 'o-manual', id, revision, definition_digest,
                '2026-10-22T09:30:00.000000000Z', 'manual', 'planned', 1,
                '2026-10-22T09:30:00.000Z', '2026-10-22T09:30:00.000Z'
         FROM automation_definitions WHERE id = 'manual-after';
         -- Neither an occurrence not yet due nor a settled one supersedes it.
         INSERT INTO automation_occurrences
             (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
              state, attempt, created_at, updated_at)
         SELECT 'o-' || id, id, revision, definition_digest,
                CASE id WHEN 'future-after' THEN '2026-10-22T11:00:00.000000000Z'
                        ELSE '2026-10-22T09:30:00.000000000Z' END,
                'manual', CASE id WHEN 'future-after' THEN 'planned' ELSE 'failed' END, 1,
                '2026-10-22T09:30:00.000Z', '2026-10-22T09:30:00.000Z'
         FROM automation_definitions WHERE id IN ('future-after', 'settled-after');",
    )
    .unwrap();
    // A row of another digest already holds today's slot: under the id
    // planning derives, which fences it, or under another id, which fails
    // planning for the routine.
    let today_millis = Utc
        .with_ymd_and_hms(2026, 10, 22, 9, 0, 0)
        .unwrap()
        .timestamp_millis();
    for (routine, id) in [
        ("fenced-after", format!("fenced-after-{today_millis}")),
        ("taken-after", "o-taken".to_string()),
    ] {
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             VALUES (?1, ?2, 1, printf('%064d', 1), '2026-10-22T09:00:00.000Z',
                     'scheduled', 'skipped', 1, '2026-10-01T00:00:00.000Z',
                     '2026-10-01T00:00:00.000Z')",
            [id.as_str(), routine],
        )
        .unwrap();
    }
    // The stored status stays ACTIVE; the definition itself says PAUSED.
    let paused = definition("divergent-after", "PAUSED");
    crate::automations::store::update_definition(&conn, &paused).unwrap();
    conn.execute(
        "UPDATE automation_definitions SET status = 'ACTIVE' WHERE id = 'divergent-after'",
        [],
    )
    .unwrap();
    quarantine(&conn, "quarantined-after");
    occurrence(&conn, "o-held", "held", 20, "claimed");
    for (routine, not_before) in [
        ("ready-retry-paused", "2026-10-22T09:30:00.000Z"),
        ("ready-retry", "2026-10-22T09:30:00.000Z"),
        ("waiting-retry", "2026-10-22T11:00:00.000Z"),
    ] {
        let planned = planned_occurrence(&conn, routine);
        let run = format!("r-{routine}");
        run_keyed(&conn, &run, routine, Some(&planned), "running", false);
        adopted_attempt(&conn, &run, not_before);
    }

    // A pass claims each eligible occurrence, and the slot it plans unless
    // something holds the routine's claims.
    let planned = Utc.with_ymd_and_hms(2026, 10, 22, 9, 0, 0).unwrap();
    let (mut predicted, mut listed) = (Vec::new(), Vec::new());
    for id in routines {
        let work = found(&conn, id).work;
        let holds = work.iter().any(|item| item.holds_claims);
        for item in &work {
            match item.code {
                WorkCode::EligibleNow => {
                    predicted.push(item.occurrence_id.clone().unwrap());
                }
                WorkCode::DueUnplanned if !holds => {
                    predicted.push(format!("{id}-{}", planned.timestamp_millis()));
                }
                WorkCode::DueUnplanned => {}
                _ => continue,
            }
            assert!(!item.holds_claims, "{id}");
            assert_eq!(item.next_step, NextStep::AwaitSchedulerPass, "{id}");
            listed.push((id, item.code, item.at.clone().unwrap()));
        }
    }
    // The superseded slot of `ready` is not listed, nor the slot `o-manual`
    // supersedes; a retry's slot is, and its run keeps the planned slot from
    // being claimed.
    let (yesterday, today) = ("2026-10-21T09:00:00.000Z", "2026-10-22T09:00:00.000Z");
    assert_eq!(
        listed,
        [
            ("ready", WorkCode::DueUnplanned, today),
            ("unreadable-created-after", WorkCode::EligibleNow, yesterday),
            ("held", WorkCode::DueUnplanned, today),
            ("ready-retry-paused", WorkCode::EligibleNow, yesterday),
            ("ready-retry", WorkCode::EligibleNow, yesterday),
            ("ready-retry", WorkCode::DueUnplanned, today),
            ("waiting-retry", WorkCode::DueUnplanned, today),
            ("revised-after", WorkCode::EligibleNow, yesterday),
            (
                "manual-after",
                WorkCode::EligibleNow,
                "2026-10-22T09:30:00.000Z"
            ),
            ("future-after", WorkCode::DueUnplanned, today),
            ("settled-after", WorkCode::DueUnplanned, today),
            ("fenced-after", WorkCode::EligibleNow, yesterday),
        ]
        .map(|(id, code, at)| (id, code, at.to_string()))
    );
    assert_eq!(codes(&found(&conn, "divergent-after")), [Code::NotActive]);
    assert_eq!(
        codes(&found(&conn, "unreadable-history-after")),
        [Code::ScheduleHistoryUnreadable]
    );
    predicted.sort();

    let mut claimed = crate::automations::occurrences::tick(&conn, now())
        .unwrap()
        .claimed;
    claimed.sort();
    assert_eq!(claimed, predicted);
}

#[test]
fn an_eligible_occurrence_precedes_recovery_that_holds_nothing() {
    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    fence_due_slot(&conn, "daily");
    occurrence(&conn, "o-recovery", "daily", 1, "recovery_required");
    occurrence(&conn, "o-due", "daily", 21, "planned");
    let work = work_of(&conn, "daily");
    assert_eq!(
        work,
        json!([
            {"code": "eligible_now", "occurrenceId": "o-due", "at": "2026-10-21T09:00:00.000Z",
             "holdsClaims": false, "nextStep": "await_scheduler_pass"},
            {"code": "recovery_required", "occurrenceId": "o-recovery",
             "at": "2026-10-01T09:00:00.000Z", "holdsClaims": false,
             "nextStep": "recover_occurrence"}
        ])
    );
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
    fence_due_slot(&conn, "daily");
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
    fence_due_slot(&conn, "daily");
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
    fence_due_slot(&conn, "daily");
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
    fence_due_slot(&conn, "daily");
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
    fence_due_slot(&conn, "daily");
    occurrence(&conn, "o-running", "daily", 19, "running");
    run(&conn, "r-running", "o-running", "running");
    // Its claim hold rests only on the run, so a shadowed run read would show.
    occurrence(&conn, "o-recovery", "daily", 18, "recovery_required");
    run(&conn, "r-recovery", "o-recovery", "running");
    run_keyed(&conn, "r-orphan", "daily", None, "running", false);
    // A ready retry on an older planned occurrence stays eligible beside the
    // slot a pass fences first, which rests on its running run.
    insert_definition_at(
        &conn,
        &definition("retrying", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    occurrence(&conn, "o-retrying", "retrying", 21, "planned");
    run(&conn, "r-retrying", "o-retrying", "running");
    adopted_attempt(&conn, "r-retrying", "2026-10-22T09:30:00.000Z");
    let retrying = work_of(&conn, "retrying");
    assert_eq!(
        retrying
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["code"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["running_run", "eligible_now", "due_unplanned"]
    );
    // A waiting retry is read from runs, attempts and occurrences.
    insert_definition_at(
        &conn,
        &definition("waiting", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    occurrence(&conn, "o-waiting", "waiting", 21, "planned");
    run(&conn, "r-waiting", "o-waiting", "running");
    adopted_attempt(&conn, "r-waiting", "2026-10-22T11:00:00.000Z");
    let waiting = work_of(&conn, "waiting");
    assert_eq!(waiting[0]["code"], "retry_waiting");
    insert_definition_at(
        &conn,
        &definition("expired-claim", "ACTIVE"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    occurrence(&conn, "o-expired-claim", "expired-claim", 21, "claimed");
    run(&conn, "r-expired-claim", "o-expired-claim", "running");
    conn.execute(
        "UPDATE automation_runs SET timeout_at = '2026-10-22T09:45:00.000Z'
         WHERE id = 'r-expired-claim'",
        [],
    )
    .unwrap();
    adopted_attempt(&conn, "r-expired-claim", "2026-10-22T09:30:00.000Z");
    let expired_claim = work_of(&conn, "expired-claim");
    assert_eq!(expired_claim[0]["code"], "retry_expired");
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
             VALUES ('o-shadow', 'daily', '2026-10-20T09:00:00.000Z', 'claimed'),
                    ('o-shadow-later', 'retrying', '2026-10-22T09:30:00.000Z', 'planned'),
                    ('retrying-1792659600000', 'retrying', '2026-10-22T09:00:00.000Z', 'skipped');
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
    assert_eq!(work_of(&conn, "retrying"), retrying);
    assert_eq!(work_of(&conn, "waiting"), waiting);
    assert_eq!(work_of(&conn, "expired-claim"), expired_claim);
}

#[test]
fn each_retry_item_names_what_the_next_passes_do_with_it() {
    let (_temp, conn) = store();
    let at = |hour: u32, minute: u32| Utc.with_ymd_and_hms(2026, 10, 22, hour, minute, 0).unwrap();
    let time = |hour: u32, minute: u32| format!("2026-10-22T{hour:02}:{minute:02}:00.000Z");
    // (routine, not_before, deadline)
    let shapes = [
        ("waiting", time(11, 0), Some(time(12, 0))),
        ("waiting-no-deadline", time(11, 0), None),
        ("expires-first", time(11, 0), Some(time(10, 30))),
        ("expired", time(9, 30), Some(time(9, 45))),
        ("expired-waiting", time(11, 0), Some(time(9, 45))),
        ("ready", time(9, 30), Some(time(12, 0))),
        ("with-session", time(11, 0), Some(time(9, 45))),
        ("paused-waiting", time(11, 0), Some(time(12, 0))),
        ("quarantined-waiting", time(11, 0), Some(time(12, 0))),
        // Its run retries the occurrence of `foreign-owner`.
        ("foreign", time(9, 30), Some(time(9, 45))),
    ];
    for id in shapes.iter().map(|(id, _, _)| *id).chain(["foreign-owner"]) {
        insert_definition_at(&conn, &definition(id, "ACTIVE"), "2026-10-01T00:00:00.000Z").unwrap();
    }
    crate::automations::occurrences::tick_planning(&conn, now() - chrono::Duration::days(1))
        .unwrap();
    let mut retried = std::collections::HashMap::new();
    for (id, not_before, deadline) in &shapes {
        let owner = if *id == "foreign" {
            "foreign-owner"
        } else {
            id
        };
        let occurrence = planned_occurrence(&conn, owner);
        let run = format!("r-{id}");
        run_keyed(&conn, &run, id, Some(&occurrence), "running", false);
        conn.execute(
            "UPDATE automation_runs SET timeout_at = ?2 WHERE id = ?1",
            rusqlite::params![run, deadline],
        )
        .unwrap();
        adopted_attempt(&conn, &run, not_before);
        retried.insert(*id, occurrence);
    }
    session(&conn, "PRIVATE-SESSION", "running");
    conn.execute_batch(
        "UPDATE automation_runs SET session_id = 'PRIVATE-SESSION' WHERE id = 'r-with-session';
         UPDATE automation_definitions SET status = 'PAUSED' WHERE id = 'paused-waiting';",
    )
    .unwrap();
    quarantine(&conn, "quarantined-waiting");

    let retry_items: Vec<_> = shapes
        .iter()
        .map(|(id, _, _)| {
            let item = found(&conn, id)
                .work
                .into_iter()
                .find(|item| item.run_id.as_deref() == Some(format!("r-{id}").as_str()))
                .unwrap();
            assert!(item.holds_claims, "{id}");
            (
                *id,
                item.code,
                item.next_step,
                item.not_before,
                item.deadline_at,
            )
        })
        .collect();
    let waiting = |deadline: Option<String>| {
        (
            WorkCode::RetryWaiting,
            NextStep::AwaitRetry,
            Some(time(11, 0)),
            deadline,
        )
    };
    let expired = (
        WorkCode::RetryExpired,
        NextStep::AwaitSettlement,
        None,
        Some(time(9, 45)),
    );
    let running = (WorkCode::RunningRun, NextStep::InspectRun, None, None);
    let expected = [
        ("waiting", waiting(Some(time(12, 0)))),
        ("waiting-no-deadline", waiting(None)),
        ("expires-first", waiting(Some(time(10, 30)))),
        ("expired", expired.clone()),
        ("expired-waiting", expired.clone()),
        ("ready", running.clone()),
        ("with-session", running),
        ("paused-waiting", waiting(Some(time(12, 0)))),
        ("quarantined-waiting", waiting(Some(time(12, 0)))),
        ("foreign", expired),
    ]
    .map(|(id, (code, step, not_before, deadline))| (id, code, step, not_before, deadline));
    assert_eq!(retry_items, expected);

    // The daemon's own steps, in its order: reconciliation settles expired
    // retries, then the scheduler pass plans, supersedes and claims.
    let status = |run: &str| -> String {
        conn.query_row(
            "SELECT status FROM automation_runs WHERE id = ?1",
            [run],
            |row| row.get(0),
        )
        .unwrap()
    };
    let pass = |instant: DateTime<Utc>| {
        crate::automations::runner::enforce_run_timeouts(
            &conn,
            &crate::api::NoopSessionRuntime,
            instant,
        )
        .unwrap();
        crate::automations::runner::settle_finished_runs(&conn, instant).unwrap();
        let mut claimed = crate::automations::occurrences::tick(&conn, instant)
            .unwrap()
            .claimed;
        claimed.sort();
        claimed
    };
    let today = |id: &str| format!("{id}-{}", at(9, 0).timestamp_millis());
    let code_at = |id: &str, instant: DateTime<Utc>| {
        let Lookup::Found(explanation) = explain(&conn, id, instant).unwrap() else {
            panic!("{id} not found");
        };
        explanation
            .work
            .into_iter()
            .find(|item| item.run_id.as_deref() == Some(format!("r-{id}").as_str()))
            .map(|item| item.code)
    };
    // An expired retry is settled before claiming, which releases its hold.
    // Settling `foreign` fails the occurrence of `foreign-owner` it retried.
    let mut expected = vec![
        retried["ready"].clone(),
        today("expired"),
        today("expired-waiting"),
        today("foreign"),
        today("foreign-owner"),
    ];
    expected.sort();
    assert_eq!(pass(at(10, 0)), expected);
    for (id, _, _) in &shapes {
        let settled = matches!(*id, "expired" | "expired-waiting" | "foreign");
        assert_eq!(
            status(&format!("r-{id}")),
            if settled { "failed" } else { "running" },
            "{id}"
        );
    }
    // A deadline before `notBefore` settles the retry instead.
    assert_eq!(pass(at(10, 30)), [today("expires-first")]);
    assert_eq!(status("r-expires-first"), "failed");
    // At `notBefore` the queue claims a waiting retry, even for a paused
    // routine, but never for a quarantined one.
    assert_eq!(code_at("waiting", at(11, 0)), Some(WorkCode::RunningRun));
    let mut expected = vec![
        retried["waiting"].clone(),
        retried["waiting-no-deadline"].clone(),
        retried["paused-waiting"].clone(),
    ];
    expected.sort();
    assert_eq!(pass(at(11, 0)), expected);
    assert_eq!(status("r-quarantined-waiting"), "running");
    // Ready but quarantined, it stays out of the queue until its deadline.
    assert_eq!(
        code_at("quarantined-waiting", at(11, 59)),
        Some(WorkCode::RunningRun)
    );
    assert_eq!(
        code_at("quarantined-waiting", at(12, 0)),
        Some(WorkCode::RetryExpired)
    );
    pass(at(12, 0));
    assert_eq!(status("r-quarantined-waiting"), "failed");
}

/// What reconciliation's `enforce_run_timeouts` does with a retry's run.
#[derive(Debug, PartialEq)]
enum Reconciled {
    Settled,
    Untouched,
    FailsThePass,
}

/// (shape, occurrence state, run deadline, attempts as (state, not_before),
/// code, notBefore, deadlineAt, what reconciliation does)
type RetryShape<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    &'a [(&'a str, &'a str)],
    WorkCode,
    Option<&'a str>,
    Option<&'a str>,
    Reconciled,
);

#[test]
fn retry_timing_is_read_only_as_reconciliation_and_the_queue_read_it() {
    use Reconciled::*;
    let (deadline, waiting) = ("2026-10-22T09:45:00.000Z", "2026-10-22T11:00:00.000Z");
    let shapes: [RetryShape<'_>; 13] = [
        // Reconciliation reads the whole stored deadline and any RFC 3339 form.
        (
            "overlong",
            "planned",
            Some("2026-10-22T09:45:00.0000000000000000000000000000000000000000000000000Z"),
            &[("adopted", waiting)],
            WorkCode::RetryExpired,
            None,
            Some(deadline),
            Settled,
        ),
        (
            "offset",
            "planned",
            Some("2026-10-22T11:45:00+02:00"),
            &[("adopted", waiting)],
            WorkCode::RetryExpired,
            None,
            Some(deadline),
            Settled,
        ),
        // It also settles a claimed occurrence's retry.
        (
            "claimed",
            "claimed",
            Some(deadline),
            &[("adopted", "2026-10-22T09:30:00.000Z")],
            WorkCode::RetryExpired,
            None,
            Some(deadline),
            Settled,
        ),
        // A deadline it cannot parse, or a retry whose occurrence it cannot
        // settle, fails every pass.
        (
            "unreadable",
            "planned",
            Some("PRIVATE"),
            &[("adopted", waiting)],
            WorkCode::RunningRun,
            None,
            None,
            FailsThePass,
        ),
        (
            "settled",
            "failed",
            Some(deadline),
            &[("adopted", waiting)],
            WorkCode::RunningRun,
            None,
            None,
            FailsThePass,
        ),
        (
            "running",
            "running",
            Some(deadline),
            &[("adopted", waiting)],
            WorkCode::InFlight,
            None,
            None,
            FailsThePass,
        ),
        // It settles only an adopted attempt's run.
        (
            "dispatching",
            "planned",
            Some(deadline),
            &[("dispatching", waiting)],
            WorkCode::RunningRun,
            None,
            None,
            Untouched,
        ),
        (
            "claimed-dispatching",
            "claimed",
            Some(deadline),
            &[("dispatching", waiting)],
            WorkCode::InFlight,
            None,
            None,
            Untouched,
        ),
        // Nor a run with a session.
        (
            "claimed-session",
            "claimed",
            Some(deadline),
            &[("adopted", "2026-10-22T09:30:00.000Z")],
            WorkCode::InFlight,
            None,
            None,
            Untouched,
        ),
        // Only an adopted attempt's `notBefore` keeps its retry waiting.
        (
            "ready",
            "planned",
            Some("2026-10-22T12:00:00.000Z"),
            &[("failed", waiting), ("adopted", "2026-10-22T09:30:00.000Z")],
            WorkCode::RunningRun,
            None,
            None,
            Untouched,
        ),
        // The queue reads this deadline, as text, as passed; reconciliation
        // settles it at 11:30.
        (
            "offset-open",
            "planned",
            Some("2026-10-22T09:30:00-02:00"),
            &[("adopted", waiting)],
            WorkCode::RunningRun,
            None,
            None,
            Untouched,
        ),
        // The queue compares `notBefore` as stored text, so a time stored in
        // any other form is not shown.
        (
            "offset-not-before",
            "planned",
            Some("2026-10-22T12:00:00.000Z"),
            &[("adopted", "2026-10-22T13:00:00+02:00")],
            WorkCode::RetryWaiting,
            None,
            Some("2026-10-22T12:00:00.000Z"),
            Untouched,
        ),
        (
            "unreadable-not-before",
            "planned",
            Some("2026-10-22T12:00:00.000Z"),
            &[("adopted", "PRIVATE")],
            WorkCode::RetryWaiting,
            None,
            Some("2026-10-22T12:00:00.000Z"),
            Untouched,
        ),
    ];
    for (shape, state, timeout, attempts, code, not_before, deadline_at, reconciled) in shapes {
        let (_temp, conn) = store();
        insert_definition_at(
            &conn,
            &definition("daily", "ACTIVE"),
            "2026-10-01T00:00:00.000Z",
        )
        .unwrap();
        fence_due_slot(&conn, "daily");
        occurrence(&conn, "o-retry", "daily", 21, state);
        run_keyed(&conn, "r-retry", "daily", Some("o-retry"), "running", false);
        if shape == "claimed-session" {
            session(&conn, "PRIVATE-SESSION", "running");
            conn.execute(
                "UPDATE automation_runs SET session_id = 'PRIVATE-SESSION' WHERE id = 'r-retry'",
                [],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE automation_runs SET timeout_at = ?1 WHERE id = 'r-retry'",
            [timeout],
        )
        .unwrap();
        for (index, (attempt_state, attempt_not_before)) in attempts.iter().enumerate() {
            let number = i64::try_from(index).unwrap() + 1;
            conn.execute(
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, state, prior_attempt_number,
                     prior_disposition, retry_classification, not_before, opened_at)
                 VALUES ('a-' || ?1, 'r-retry', 'o-retry', ?1, 'key-' || ?1, 1, ?2,
                         CASE WHEN ?1 > 1 THEN ?1 - 1 END,
                         CASE WHEN ?1 > 1 THEN 'failed' END,
                         CASE WHEN ?1 > 1 THEN 'automatic_retry' ELSE 'initial' END,
                         ?3, '2026-10-21T09:00:00.000Z')",
                rusqlite::params![number, attempt_state, attempt_not_before],
            )
            .unwrap();
        }
        let item = found(&conn, "daily")
            .work
            .into_iter()
            .find(|item| item.run_id.as_deref() == Some("r-retry"))
            .unwrap();
        assert_eq!(
            (
                item.code,
                item.not_before.as_deref(),
                item.deadline_at.as_deref()
            ),
            (code, not_before, deadline_at),
            "{shape}"
        );
        let outcome = crate::automations::runner::enforce_run_timeouts(
            &conn,
            &crate::api::NoopSessionRuntime,
            now(),
        );
        let status: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'r-retry'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let actual = match (outcome, status.as_str()) {
            (Err(_), _) => FailsThePass,
            (Ok(_), "failed") => Settled,
            (Ok(_), "running") => Untouched,
            (Ok(_), other) => panic!("{shape}: run became {other}"),
        };
        assert_eq!(actual, reconciled, "{shape}");
    }
}

/// (routine, lease owner, kind, scheduler generation, lease, its run's status,
/// code, leaseExpiresAt, state after the daemon's fenced pass)
type ClaimShape<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Option<i64>,
    Option<&'a str>,
    Option<&'a str>,
    WorkCode,
    Option<&'a str>,
    &'a str,
);

#[test]
fn each_claim_names_what_the_fenced_pass_releases() {
    let (temp, conn) = store();
    // Two takeovers make generation 1 stale and generation 2 current.
    let first =
        crate::automations::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now())
            .unwrap();
    let old = first.fence().generation();
    drop(first);
    let current =
        crate::automations::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now())
            .unwrap();
    let generation = current.fence().generation();
    assert!(generation > old);
    let (past, future) = ("2026-10-22T09:45:00.000Z", "2026-10-22T10:30:00.000Z");
    let shapes: [ClaimShape<'_>; 16] = [
        (
            "stale",
            "daemon",
            "scheduled",
            Some(old),
            Some(future),
            None,
            WorkCode::ClaimStale,
            Some(future),
            "planned",
        ),
        (
            "stale-expired",
            "daemon",
            "scheduled",
            Some(old),
            Some(past),
            None,
            WorkCode::ClaimStale,
            Some(past),
            "planned",
        ),
        (
            "stale-unfenced",
            "daemon",
            "scheduled",
            None,
            Some(future),
            None,
            WorkCode::ClaimStale,
            Some(future),
            "planned",
        ),
        // Any run at all keeps a stale claim, and only this generation's
        // expired leases fail.
        (
            "stale-with-run",
            "daemon",
            "scheduled",
            Some(old),
            Some(past),
            Some("failed"),
            WorkCode::InFlight,
            Some(past),
            "claimed",
        ),
        (
            "expired",
            "daemon",
            "scheduled",
            Some(generation),
            Some(past),
            None,
            WorkCode::LeaseExpired,
            Some(past),
            "failed",
        ),
        (
            "expired-at-now",
            "daemon",
            "scheduled",
            Some(generation),
            Some("2026-10-22T10:00:00.000Z"),
            None,
            WorkCode::LeaseExpired,
            Some("2026-10-22T10:00:00.000Z"),
            "failed",
        ),
        (
            "expired-settled-run",
            "daemon",
            "scheduled",
            Some(generation),
            Some(past),
            Some("failed"),
            WorkCode::LeaseExpired,
            Some(past),
            "failed",
        ),
        (
            "expired-running-run",
            "daemon",
            "scheduled",
            Some(generation),
            Some(past),
            Some("running"),
            WorkCode::InFlight,
            Some(past),
            "claimed",
        ),
        (
            "manual-expired",
            "operator",
            "manual",
            None,
            Some(past),
            None,
            WorkCode::LeaseExpired,
            Some(past),
            "failed",
        ),
        (
            "manual-live",
            "operator",
            "manual",
            None,
            Some(future),
            None,
            WorkCode::InFlight,
            Some(future),
            "claimed",
        ),
        (
            "foreign-owner",
            "operator",
            "scheduled",
            Some(generation),
            Some(past),
            None,
            WorkCode::InFlight,
            Some(past),
            "claimed",
        ),
        (
            "no-lease",
            "daemon",
            "scheduled",
            Some(generation),
            None,
            None,
            WorkCode::InFlight,
            None,
            "claimed",
        ),
        // Only the daemon's own claims of this generation, or manual claims
        // with none, lose an expired lease; a stale claim returns first.
        (
            "unfenced-operator",
            "operator",
            "scheduled",
            None,
            Some(past),
            None,
            WorkCode::InFlight,
            Some(past),
            "claimed",
        ),
        (
            "manual-generation",
            "operator",
            "manual",
            Some(generation),
            Some(past),
            None,
            WorkCode::InFlight,
            Some(past),
            "claimed",
        ),
        (
            "manual-daemon-expired",
            "daemon",
            "manual",
            None,
            Some(past),
            None,
            WorkCode::ClaimStale,
            Some(past),
            "planned",
        ),
        // The pass compares the lease as stored text: this 09:45Z lease reads
        // as later than 10:00Z, and is not shown.
        (
            "offset-lease",
            "daemon",
            "scheduled",
            Some(generation),
            Some("2026-10-22T11:45:00+02:00"),
            None,
            WorkCode::InFlight,
            None,
            "claimed",
        ),
    ];
    for (id, owner, kind, claim_generation, lease, run_status, _, _, _) in shapes {
        // Paused, so the pass neither plans nor reclaims what it releases.
        insert_definition_at(&conn, &definition(id, "PAUSED"), "2026-10-01T00:00:00.000Z").unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
                 lease_expires_at, scheduler_generation, created_at, updated_at)
             VALUES ('o-' || ?1, ?1, '2026-10-21T09:00:00.000Z', ?2, 'claimed', 1, ?3, ?4,
                     ?5, '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z')",
            rusqlite::params![id, kind, owner, lease, claim_generation],
        )
        .unwrap();
        if let Some(status) = run_status {
            run(&conn, &format!("r-{id}"), &format!("o-{id}"), status);
        }
    }
    for (id, _, _, _, _, _, code, lease, _) in &shapes {
        let item = found(&conn, id).work.into_iter().next().unwrap();
        let (state, next_step) = match code {
            WorkCode::InFlight => (Some("claimed"), NextStep::InspectOccurrence),
            WorkCode::ClaimStale => (None, NextStep::AwaitSchedulerPass),
            WorkCode::LeaseExpired => (None, NextStep::AwaitSettlement),
            other => panic!("{id}: unexpected {other:?}"),
        };
        assert_eq!(
            (
                item.code,
                item.state,
                item.next_step,
                item.lease_expires_at.as_deref(),
                item.holds_claims
            ),
            (*code, state, next_step, *lease, true),
            "{id}"
        );
    }

    // TEMP tables cannot add the runs these releases read.
    let explained: Vec<_> = shapes.iter().map(|(id, ..)| work_of(&conn, id)).collect();
    conn.execute_batch(
        "CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs WHERE 0;
         INSERT INTO temp.automation_runs (id, automation_id, occurrence_id, status)
             SELECT 'r-shadow-' || id, automation_id, id, 'running'
             FROM main.automation_occurrences;",
    )
    .unwrap();
    for ((id, ..), expected) in shapes.iter().zip(&explained) {
        assert_eq!(&work_of(&conn, id), expected, "{id}");
    }
    conn.execute_batch("DROP TABLE temp.automation_runs;")
        .unwrap();

    // The daemon's pass: reconciliation, then the fenced tick.
    crate::automations::daemon_tick::reconcile_automation_runs(
        temp.path(),
        &conn,
        &crate::api::NoopSessionRuntime,
        now(),
        None,
    )
    .unwrap();
    crate::automations::occurrences::tick_with_scheduler_fence(&conn, now(), &current.fence())
        .unwrap();
    for (id, _, _, _, _, _, _, _, after) in &shapes {
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'o-' || ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, *after, "{id}");
    }
}

#[test]
fn without_a_held_scheduler_authority_no_claim_is_named_released() {
    let (temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "PAUSED"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    // An expired manual lease and a claim of no generation, either of which a
    // held authority would release.
    conn.execute_batch(
        "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
             lease_expires_at, scheduler_generation, created_at, updated_at)
         VALUES ('o-manual', 'daily', '2026-10-20T09:00:00.000Z', 'manual', 'claimed', 1,
                 'operator', '2026-10-22T09:45:00.000Z', NULL, '2026-10-20T09:00:00.000Z',
                 '2026-10-20T09:00:00.000Z'),
                ('o-daemon', 'daily', '2026-10-21T09:00:00.000Z', 'scheduled', 'claimed', 1,
                 'daemon', NULL, NULL, '2026-10-21T09:00:00.000Z',
                 '2026-10-21T09:00:00.000Z');",
    )
    .unwrap();
    let in_flight = |conn: &Connection| {
        found(conn, "daily")
            .work
            .into_iter()
            .map(|item| (item.code, item.state, item.lease_expires_at))
            .collect::<Vec<_>>()
    };
    let expected = vec![
        (
            WorkCode::InFlight,
            Some("claimed"),
            Some("2026-10-22T09:45:00.000Z".to_string()),
        ),
        (WorkCode::InFlight, Some("claimed"), None),
    ];
    assert_eq!(in_flight(&conn), expected, "none recorded");
    let mut leadership =
        crate::automations::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now())
            .unwrap();
    assert!(leadership.release(&conn).unwrap());
    assert_eq!(in_flight(&conn), expected, "released");
    conn.execute_batch(
        "UPDATE automation_scheduler_authority SET owner_id = ' padded', generation = 7;",
    )
    .unwrap();
    assert_eq!(in_flight(&conn), expected, "invalid");
    conn.execute_batch("DELETE FROM automation_scheduler_authority;")
        .unwrap();
    assert_eq!(in_flight(&conn), expected, "missing");
    // Claimed, the lease is shown; running, it no longer applies.
    conn.execute_batch(
        "UPDATE automation_occurrences SET state = 'running' WHERE id = 'o-manual';",
    )
    .unwrap();
    assert_eq!(
        in_flight(&conn)[0],
        (WorkCode::InFlight, Some("running"), None)
    );
}

#[test]
fn a_claimed_retry_past_its_deadline_carries_its_lease() {
    let (_temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "PAUSED"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
             lease_expires_at, created_at, updated_at)
         VALUES ('o-held', 'daily', '2026-10-21T09:00:00.000Z', 'manual', 'claimed', 1,
                 'operator', '2026-10-22T09:30:00.000Z', '2026-10-21T09:00:00.000Z',
                 '2026-10-21T09:00:00.000Z');
         INSERT INTO automation_runs
            (id, automation_id, occurrence_id, runtime, status, started_at, timeout_at)
         VALUES ('r-held', 'daily', 'o-held', 'coven-code', 'running',
                 '2026-10-21T09:00:00.000Z', '2026-10-22T09:45:00.000Z');
         INSERT INTO automation_attempts
            (id, run_id, occurrence_id, attempt_number, adoption_key,
             occurrence_fence_generation, state, retry_classification, not_before,
             opened_at)
         VALUES ('a-held', 'r-held', 'o-held', 1, 'r-held-key', 1, 'adopted', 'initial',
                 '2026-10-22T11:00:00.000Z', '2026-10-21T09:00:00.000Z');",
    )
    .unwrap();
    assert_eq!(
        work_of(&conn, "daily"),
        json!([{
            "code": "retry_expired", "occurrenceId": "o-held", "runId": "r-held",
            "at": "2026-10-21T09:00:00.000Z", "deadlineAt": "2026-10-22T09:45:00.000Z",
            "leaseExpiresAt": "2026-10-22T09:30:00.000Z", "holdsClaims": true,
            "nextStep": "await_settlement"
        }])
    );
}

/// A runtime whose kills are never confirmed, as a real one's can be.
struct UnconfirmedKill;

impl crate::api::SessionRuntime for UnconfirmedKill {
    fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("the kill was not confirmed")
    }
}

struct HeldShape {
    name: &'static str,
    occurrence: &'static str,
    deadline: Option<&'static str>,
    session: Option<&'static str>,
    lease: Option<&'static str>,
    /// (number, state, carries the run's session)
    attempts: &'static [(i64, &'static str, bool)],
    authority: bool,
    fenced: bool,
    /// A cancellation: its state, and when its execution expires.
    cancelling: Option<(&'static str, &'static str)>,
    /// Another run on the occurrence, with this status, whose session completed.
    rival: Option<&'static str>,
    /// The deadline is stored as a BLOB, which reconciliation cannot read.
    blob_deadline: bool,
    code: WorkCode,
    deadline_at: Option<&'static str>,
    /// The occurrence after the daemon's reconciliation at 10:00 and then at
    /// 10:30; `None` when that pass fails.
    after: [Option<&'static str>; 2],
}

const HELD: HeldShape = HeldShape {
    name: "held",
    occurrence: "claimed",
    deadline: Some("2026-10-22T10:30:00.000Z"),
    session: None,
    lease: None,
    attempts: &[(1, "failed", false)],
    authority: false,
    fenced: false,
    cancelling: None,
    rival: None,
    blob_deadline: false,
    code: WorkCode::RetryHeld,
    deadline_at: Some("2026-10-22T10:30:00.000Z"),
    after: [Some("claimed"), Some("failed")],
};

/// What the daemon's reconciliation does with a failed attempt held for an
/// operator retry, one store per shape so that a failing pass hides nothing.
#[test]
fn each_held_retry_names_what_reconciliation_does_with_it() {
    let shapes = [
        HELD,
        HeldShape {
            name: "running",
            occurrence: "running",
            session: Some("failed"),
            attempts: &[(1, "failed", true)],
            after: [Some("running"), Some("failed")],
            ..HELD
        },
        HeldShape {
            name: "live-session",
            session: Some("running"),
            attempts: &[(1, "failed", true)],
            ..HELD
        },
        HeldShape {
            name: "no-deadline",
            deadline: None,
            deadline_at: None,
            after: [Some("failed"), Some("failed")],
            ..HELD
        },
        HeldShape {
            name: "expired",
            deadline: Some("2026-10-22T09:45:00.000Z"),
            deadline_at: Some("2026-10-22T09:45:00.000Z"),
            after: [Some("failed"), Some("failed")],
            ..HELD
        },
        HeldShape {
            name: "offset",
            deadline: Some("2026-10-22T12:30:00+02:00"),
            ..HELD
        },
        HeldShape {
            name: "twice",
            attempts: &[(1, "failed", false), (2, "failed", false)],
            ..HELD
        },
        // Timeout enforcement settles an adopted retry past its deadline
        // first.
        HeldShape {
            name: "adopted-expired",
            deadline: Some("2026-10-22T09:45:00.000Z"),
            attempts: &[(1, "adopted", false), (2, "failed", false)],
            code: WorkCode::RetryExpired,
            deadline_at: Some("2026-10-22T09:45:00.000Z"),
            after: [Some("failed"), Some("failed")],
            ..HELD
        },
        // Timeout enforcement settles a claimed occurrence's adopted retry at
        // its deadline, as settlement would, but refuses a running one.
        HeldShape {
            name: "adopted-waiting",
            attempts: &[(1, "adopted", false), (2, "failed", false)],
            ..HELD
        },
        HeldShape {
            name: "running-adopted",
            occurrence: "running",
            attempts: &[(1, "adopted", false), (2, "failed", false)],
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("running"), None],
            ..HELD
        },
        HeldShape {
            name: "running-adopted-undated",
            occurrence: "running",
            deadline: None,
            attempts: &[(1, "adopted", false), (2, "failed", false)],
            deadline_at: None,
            after: [Some("failed"), Some("failed")],
            ..HELD
        },
        // Only the latest attempt counts, and nothing settles one that
        // timed out without a session.
        HeldShape {
            name: "timed-out-after",
            attempts: &[(1, "failed", false), (2, "timed_out", false)],
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), Some("claimed")],
            ..HELD
        },
        HeldShape {
            name: "retrying",
            attempts: &[(1, "failed", false), (2, "started", false)],
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), Some("claimed")],
            ..HELD
        },
        // Abandoned-launch recovery reads this launch, but can stop only a
        // live attempt.
        HeldShape {
            name: "created-session",
            session: Some("created"),
            lease: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "failed", true)],
            ..HELD
        },
        // Timeout enforcement stops a live attempt at the deadline instead,
        // and an unconfirmed stop needs recovery.
        HeldShape {
            name: "live-attempt",
            session: Some("running"),
            attempts: &[(1, "started", true), (2, "failed", false)],
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), Some("recovery_required")],
            ..HELD
        },
        HeldShape {
            name: "authority",
            authority: true,
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), Some("claimed")],
            ..HELD
        },
        HeldShape {
            name: "live-dispatching",
            session: Some("running"),
            attempts: &[(1, "dispatching", true), (2, "failed", false)],
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), Some("recovery_required")],
            ..HELD
        },
        HeldShape {
            name: "live-observing",
            session: Some("running"),
            attempts: &[(1, "observing", true), (2, "failed", false)],
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), Some("recovery_required")],
            ..HELD
        },
        // Settlement skips a fenced run on every pass.
        HeldShape {
            name: "fenced",
            session: Some("failed"),
            attempts: &[(1, "failed", true)],
            fenced: true,
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), Some("claimed")],
            ..HELD
        },
        // Reconciling a requested cancellation leaves the hold to settlement.
        HeldShape {
            name: "cancelling",
            session: Some("failed"),
            attempts: &[(1, "failed", true)],
            cancelling: Some(("requested", "2099-01-01T00:00:00.000Z")),
            ..HELD
        },
        HeldShape {
            name: "cancellation-expired",
            session: Some("failed"),
            attempts: &[(1, "failed", true)],
            cancelling: Some(("requested", "2026-10-22T09:00:00.000Z")),
            ..HELD
        },
        HeldShape {
            name: "cancellation-rejected",
            cancelling: Some(("rejected", "2026-10-22T09:00:00.000Z")),
            ..HELD
        },
        // A stopping cancellation that expires before the deadline fails the
        // pass when its stop cannot be recovered.
        HeldShape {
            name: "stopping",
            session: Some("running"),
            attempts: &[(1, "failed", true)],
            cancelling: Some(("stopping", "2026-10-22T10:15:00.000Z")),
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("claimed"), None],
            ..HELD
        },
        // Settlement of the other run settles the occurrence first, and the
        // hold's settlement then fails the pass.
        HeldShape {
            name: "rival",
            rival: Some("running"),
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [Some("succeeded"), None],
            ..HELD
        },
        // Only a running run settles the occurrence.
        HeldShape {
            name: "settled-rival",
            rival: Some("succeeded"),
            ..HELD
        },
        HeldShape {
            name: "blob-deadline",
            blob_deadline: true,
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [None, None],
            ..HELD
        },
        HeldShape {
            name: "unreadable",
            deadline: Some("2026-10-22 10:30"),
            code: WorkCode::InFlight,
            deadline_at: None,
            after: [None, None],
            ..HELD
        },
        // Recovery is named first; settlement still settles the hold.
        HeldShape {
            name: "recovery",
            occurrence: "recovery_required",
            code: WorkCode::RecoveryRequired,
            deadline_at: None,
            after: [Some("recovery_required"), Some("failed")],
            ..HELD
        },
        // Settlement cannot settle a planned occurrence, so the pass fails.
        HeldShape {
            name: "planned",
            occurrence: "planned",
            code: WorkCode::RunningRun,
            deadline_at: None,
            after: [Some("planned"), None],
            ..HELD
        },
    ];
    let at = |minute: u32| Utc.with_ymd_and_hms(2026, 10, 22, 10, minute, 0).unwrap();
    let mut seen = Vec::new();
    for shape in &shapes {
        let (temp, conn) = store();
        insert_definition_at(
            &conn,
            &definition("daily", "PAUSED"),
            "2026-10-01T00:00:00.000Z",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
                 lease_expires_at, created_at, updated_at)
             VALUES ('o-held', 'daily', '2026-10-21T09:00:00.000Z', 'manual', ?1, 1,
                     'operator', ?2, '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z')",
            rusqlite::params![shape.occurrence, shape.lease],
        )
        .unwrap();
        if let Some(status) = shape.session {
            session(&conn, "s-held", status);
        }
        run_keyed(
            &conn,
            "r-held",
            "daily",
            Some("o-held"),
            "running",
            shape.authority,
        );
        conn.execute(
            "UPDATE automation_runs SET timeout_at = ?1, session_id = ?2 WHERE id = 'r-held'",
            rusqlite::params![shape.deadline, shape.session.map(|_| "s-held")],
        )
        .unwrap();
        for (number, state, with_session) in shape.attempts {
            let id = format!("a-held-{}", 10 - number);
            attempt(
                &conn,
                &id,
                "r-held",
                *number,
                state,
                with_session.then_some("s-held"),
            );
        }
        if shape.blob_deadline {
            conn.execute_batch(
                "UPDATE automation_runs SET timeout_at = CAST(timeout_at AS BLOB)
                 WHERE id = 'r-held';",
            )
            .unwrap();
        }
        if let Some(status) = shape.rival {
            run_keyed(&conn, "r-rival", "daily", Some("o-held"), status, false);
            session(&conn, "s-rival", "completed");
            conn.execute_batch(
                "UPDATE sessions SET exit_code = 0 WHERE id = 's-rival';
                 UPDATE automation_runs SET session_id = 's-rival' WHERE id = 'r-rival';",
            )
            .unwrap();
            attempt(&conn, "a-rival", "r-rival", 1, "started", Some("s-rival"));
        }
        if shape.fenced {
            conn.execute_batch(
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, acquired_at, execution_expires_at)
                 VALUES ('r-held', 's-held', 'timeout', '2026-10-22T09:00:00.000Z',
                         '2099-01-01T00:00:00.000Z');",
            )
            .unwrap();
        }
        if let Some((state, expires)) = shape.cancelling {
            conn.execute(
                "INSERT INTO automation_cancellations
                    (adoption_key, request_digest, automation_id, run_id, attempt_id,
                     session_id, scope, requested_by_json, reason, state, requested_at,
                     execution_expires_at)
                 VALUES ('cancel-held', 'digest', 'daily', 'r-held', 'a-held-9', 's-held',
                         'run', '{\"principalId\":\"operator\"}', 'stop', ?1,
                         '2026-10-22T08:00:00.000Z', ?2)",
                [state, expires],
            )
            .unwrap();
        }
        let item = found(&conn, "daily").work.into_iter().next().unwrap();
        // TEMP tables cannot change what the held run reads as.
        let explained = work_of(&conn, "daily");
        conn.execute_batch(
            "CREATE TEMP TABLE automation_attempts AS SELECT * FROM main.automation_attempts;
             UPDATE temp.automation_attempts
                 SET state = CASE state WHEN 'failed' THEN 'started' ELSE 'failed' END;
             CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs;
             UPDATE temp.automation_runs SET authority_profile =
                 CASE WHEN authority_profile IS NULL THEN 'shadow' END;
             CREATE TEMP TABLE automation_stop_fences AS
                 SELECT * FROM main.automation_stop_fences WHERE 0;
             INSERT INTO temp.automation_stop_fences (run_id) VALUES ('r-held');
             CREATE TEMP TABLE automation_cancellations AS
                 SELECT * FROM main.automation_cancellations WHERE 0;
             INSERT INTO temp.automation_cancellations (run_id, state)
                 VALUES ('r-held', 'requested');",
        )
        .unwrap();
        assert_eq!(work_of(&conn, "daily"), explained, "{}", shape.name);
        conn.execute_batch(
            "DROP TABLE temp.automation_attempts;
             DROP TABLE temp.automation_runs;
             DROP TABLE temp.automation_stop_fences;
             DROP TABLE temp.automation_cancellations;",
        )
        .unwrap();
        let after = [at(0), at(30)].map(|instant| {
            crate::automations::daemon_tick::reconcile_automation_runs(
                temp.path(),
                &conn,
                &UnconfirmedKill,
                instant,
                None,
            )
            .ok()
            .map(|()| {
                conn.query_row(
                    "SELECT state FROM automation_occurrences WHERE id = 'o-held'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
            })
        });
        seen.push((shape.name, item.code, item.deadline_at, after));
    }
    let expected: Vec<_> = shapes
        .iter()
        .map(|shape| {
            (
                shape.name,
                shape.code,
                shape.deadline_at.map(str::to_string),
                shape.after.map(|state| state.map(str::to_string)),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}

#[test]
fn a_deadline_reconciliation_cannot_read_as_text_leaves_a_retry_unnamed() {
    for (stored, code) in [
        ("'2026-10-22T12:00:00.000Z'", WorkCode::RetryWaiting),
        (
            "CAST('2026-10-22T12:00:00.000Z' AS BLOB)",
            WorkCode::RunningRun,
        ),
    ] {
        let (_temp, conn) = store();
        insert_definition_at(
            &conn,
            &definition("daily", "PAUSED"),
            "2026-10-01T00:00:00.000Z",
        )
        .unwrap();
        occurrence(&conn, "o-retry", "daily", 21, "planned");
        run_keyed(&conn, "r-retry", "daily", Some("o-retry"), "running", false);
        conn.execute_batch(&format!(
            "UPDATE automation_runs SET timeout_at = {stored} WHERE id = 'r-retry';"
        ))
        .unwrap();
        adopted_attempt(&conn, "r-retry", "2026-10-22T11:00:00.000Z");
        let item = found(&conn, "daily").work.into_iter().next().unwrap();
        assert_eq!(item.code, code, "{stored}");
        // Timeout enforcement reads the deadline as text, or fails the pass.
        let enforced = crate::automations::runner::enforce_run_timeouts(
            &conn,
            &crate::api::NoopSessionRuntime,
            now(),
        );
        assert_eq!(enforced.is_ok(), code == WorkCode::RetryWaiting, "{stored}");
    }
}

struct FinishedShape {
    name: &'static str,
    occurrence: &'static str,
    deadline: Option<&'static str>,
    /// The session's status and exit code, and when it exited, if it did.
    session: (&'static str, Option<i64>, Option<&'static str>),
    /// (number, state, settled at)
    attempts: &'static [(i64, &'static str, Option<&'static str>)],
    authority: bool,
    fenced: bool,
    /// A cancellation of the latest attempt: its state, and when its
    /// execution expires.
    cancelling: Option<(&'static str, &'static str)>,
    /// Another running run on the occurrence, whose session is live.
    rival: bool,
    /// SQL run against the store before explaining, to add or corrupt rows.
    extra: &'static [&'static str],
    code: WorkCode,
    next_step: NextStep,
    /// The occurrence after the daemon's reconciliation at 10:00 and then at
    /// 10:30; `None` when that pass fails.
    after: [Option<&'static str>; 2],
}

const FINISHED: FinishedShape = FinishedShape {
    name: "completed",
    occurrence: "claimed",
    deadline: Some("2026-10-22T10:30:00.000Z"),
    session: ("completed", Some(0), Some("2026-10-22T09:30:00.000Z")),
    attempts: &[(1, "started", None)],
    authority: false,
    fenced: false,
    cancelling: None,
    rival: false,
    extra: &[],
    code: WorkCode::SessionFinished,
    next_step: NextStep::AwaitSettlement,
    after: [Some("succeeded"), Some("succeeded")],
};

/// Another run of the routine, on its own running occurrence, whose session
/// is live; it started before the run under test, so settlement reaches it
/// first.
const OTHER_RUN: &str = "
    INSERT INTO automation_occurrences
        (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
         lease_expires_at, created_at, updated_at)
    VALUES ('o-other', 'daily', '2026-10-21T10:00:00.000Z', 'manual', 'running', 1,
            'operator', '2099-01-01T00:00:00.000Z', '2026-10-21T10:00:00.000Z',
            '2026-10-21T10:00:00.000Z');
    INSERT INTO sessions (id, project_root, harness, title, status, created_at, updated_at)
    VALUES ('s-other', '/PRIVATE', 'codex', 'PRIVATE', 'running',
            '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
    INSERT INTO automation_runs
        (id, automation_id, occurrence_id, session_id, runtime, status, started_at)
    VALUES ('r-other', 'daily', 'o-other', 's-other', 'coven-code', 'running',
            '2026-10-20T09:00:00.000Z');";

/// That run's live attempt, bound to its session.
const OTHER_ATTEMPT: &str = "
    INSERT INTO automation_attempts
        (id, run_id, occurrence_id, attempt_number, adoption_key,
         occurrence_fence_generation, state, retry_classification, not_before, session_id,
         opened_at)
    VALUES ('a-other', 'r-other', 'o-other', 1, 'key-other', 1, 'started', 'initial',
            '2026-10-21T09:00:00.000Z', 's-other', '2026-10-21T09:00:00.000Z');";

/// What the daemon's reconciliation does with a run whose session has
/// finished, one store per shape so that a failing pass hides nothing.
#[test]
fn each_finished_session_names_what_reconciliation_does_with_it() {
    let in_flight = |name, after| FinishedShape {
        name,
        code: WorkCode::InFlight,
        next_step: NextStep::InspectOccurrence,
        after,
        ..FINISHED
    };
    let shapes = [
        FINISHED,
        FinishedShape {
            name: "running",
            occurrence: "running",
            ..FINISHED
        },
        FinishedShape {
            name: "cancelled",
            session: ("cancelled", None, Some("2026-10-22T09:30:00.000Z")),
            after: [Some("cancelled"), Some("cancelled")],
            ..FINISHED
        },
        // A plain failure before the deadline, with attempts left, is held
        // for an operator retry, and settled at the deadline.
        FinishedShape {
            name: "failed",
            session: ("completed", Some(1), Some("2026-10-22T09:30:00.000Z")),
            after: [Some("claimed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "killed",
            session: ("killed", None, Some("2026-10-22T09:30:00.000Z")),
            after: [Some("claimed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "idle",
            session: ("idle", None, Some("2026-10-22T09:30:00.000Z")),
            after: [Some("claimed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "failed-undated",
            deadline: None,
            session: ("failed", Some(1), Some("2026-10-22T09:30:00.000Z")),
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "failed-last-attempt",
            session: ("failed", Some(1), Some("2026-10-22T09:30:00.000Z")),
            attempts: &[(9, "failed", None), (10, "started", None)],
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "failed-past-deadline",
            deadline: Some("2026-10-22T09:45:00.000Z"),
            session: ("failed", Some(1), Some("2026-10-22T09:30:00.000Z")),
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "timed-out",
            deadline: Some("2026-10-22T09:15:00.000Z"),
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "no-exit",
            session: ("completed", Some(0), None),
            ..FINISHED
        },
        FinishedShape {
            name: "no-attempt",
            attempts: &[],
            ..FINISHED
        },
        FinishedShape {
            name: "observing",
            attempts: &[(1, "observing", None)],
            ..FINISHED
        },
        FinishedShape {
            name: "dispatching",
            attempts: &[(1, "dispatching", None)],
            ..FINISHED
        },
        // An ambiguous stop settles failed at its own time, fence or not.
        FinishedShape {
            name: "ambiguous",
            attempts: &[(1, "ambiguous", Some("2026-10-22T09:20:00.000Z"))],
            fenced: true,
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "ambiguous-unsettled",
            attempts: &[(1, "ambiguous", None)],
            ..in_flight("ambiguous-unsettled", [None, None])
        },
        FinishedShape {
            attempts: &[(1, "ambiguous", Some("2026-10-22T09:20:00.000Z"))],
            cancelling: Some(("requested", "2099-01-01T00:00:00.000Z")),
            ..in_flight("ambiguous-pending", [Some("claimed"), Some("claimed")])
        },
        FinishedShape {
            fenced: true,
            ..in_flight("fenced", [Some("claimed"), Some("claimed")])
        },
        // Only a pending cancellation of the ambiguous attempt and its session
        // holds settlement back.
        FinishedShape {
            name: "ambiguous-other-attempt",
            attempts: &[
                (1, "failed", Some("2026-10-22T09:10:00.000Z")),
                (2, "ambiguous", Some("2026-10-22T09:20:00.000Z")),
            ],
            cancelling: Some(("requested", "2099-01-01T00:00:00.000Z")),
            extra: &["UPDATE automation_cancellations SET attempt_id = 'a-done-19';"],
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "ambiguous-other-session",
            attempts: &[(1, "ambiguous", Some("2026-10-22T09:20:00.000Z"))],
            cancelling: Some(("requested", "2099-01-01T00:00:00.000Z")),
            extra: &["UPDATE automation_cancellations SET session_id = 's-other';"],
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            name: "ambiguous-rejected",
            attempts: &[(1, "ambiguous", Some("2026-10-22T09:20:00.000Z"))],
            cancelling: Some(("rejected", "2099-01-01T00:00:00.000Z")),
            after: [Some("failed"), Some("failed")],
            ..FINISHED
        },
        // Reconciling a cancellation that has not expired is left to its
        // command.
        FinishedShape {
            name: "cancellation-requested",
            cancelling: Some(("requested", "2099-01-01T00:00:00.000Z")),
            ..FINISHED
        },
        FinishedShape {
            cancelling: Some(("requested", "2026-10-22T09:00:00.000Z")),
            ..in_flight(
                "cancellation-expired",
                [Some("succeeded"), Some("succeeded")],
            )
        },
        FinishedShape {
            cancelling: Some(("stopping", "2026-10-22T09:00:00.000Z")),
            ..in_flight("stopping-expired", [Some("succeeded"), Some("succeeded")])
        },
        // Reconciling an expired cancellation of a finished session runs
        // the same settlement first, unless its own stop fence holds the run
        // for recovery.
        FinishedShape {
            session: ("completed", Some(1), Some("2026-10-22T09:30:00.000Z")),
            cancelling: Some(("requested", "2026-10-22T09:00:00.000Z")),
            ..in_flight(
                "cancellation-expired-failed",
                [Some("claimed"), Some("failed")],
            )
        },
        FinishedShape {
            session: ("completed", Some(1), Some("2026-10-22T09:30:00.000Z")),
            cancelling: Some(("stopping", "2026-10-22T09:00:00.000Z")),
            ..in_flight("stopping-expired-failed", [Some("claimed"), Some("failed")])
        },
        FinishedShape {
            attempts: &[(1, "failed", None), (2, "adopted", None)],
            ..in_flight("adopted", [None, None])
        },
        FinishedShape {
            attempts: &[(1, "succeeded", Some("2026-10-22T09:20:00.000Z"))],
            ..in_flight("settled-attempt", [None, None])
        },
        // A failed latest attempt is a held retry whatever its session.
        FinishedShape {
            name: "failed-attempt",
            attempts: &[(1, "failed", Some("2026-10-22T09:20:00.000Z"))],
            code: WorkCode::RetryHeld,
            after: [Some("claimed"), Some("failed")],
            ..FINISHED
        },
        FinishedShape {
            rival: true,
            ..in_flight("rival", [Some("succeeded"), Some("succeeded")])
        },
        FinishedShape {
            name: "settled-rival",
            extra: &["INSERT INTO automation_runs
                          (id, automation_id, occurrence_id, runtime, status, started_at)
                      VALUES ('r-old', 'daily', 'o-done', 'coven-code', 'failed',
                              '2026-10-21T08:00:00.000Z');"],
            ..FINISHED
        },
        FinishedShape {
            deadline: Some("2026-10-22 10:30"),
            ..in_flight("unparseable-deadline", [None, None])
        },
        FinishedShape {
            extra: &["UPDATE automation_runs SET timeout_at = CAST(timeout_at AS BLOB);"],
            ..in_flight("blob-deadline", [None, None])
        },
        FinishedShape {
            session: ("completed", Some(0), Some("yesterday")),
            ..in_flight("unparseable-exit", [None, None])
        },
        // Settlement reads the latest exit as stored text, then the
        // session's update time.
        FinishedShape {
            extra: &[
                "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                      VALUES ('e-late', 's-done', 'exit', '{}', 'yesterday');",
            ],
            ..in_flight("two-exits", [None, None])
        },
        FinishedShape {
            name: "other-event",
            extra: &[
                "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                      VALUES ('e-late', 's-done', 'output', '{}', 'yesterday');",
            ],
            ..FINISHED
        },
        FinishedShape {
            session: ("completed", Some(0), None),
            extra: &["UPDATE sessions SET updated_at = 'never';"],
            ..in_flight("no-exit-bad-update", [None, None])
        },
        FinishedShape {
            deadline: None,
            extra: &["UPDATE events SET created_at = X'00';"],
            ..in_flight("blob-exit-undated", [None, None])
        },
        FinishedShape {
            name: "unparseable-exit-undated",
            deadline: None,
            session: ("completed", Some(0), Some("yesterday")),
            ..FINISHED
        },
        FinishedShape {
            extra: &["UPDATE sessions SET exit_code = 'zero';"],
            ..in_flight("text-exit-code", [None, None])
        },
        FinishedShape {
            extra: &["UPDATE automation_attempts SET settled_at = X'00';"],
            ..in_flight("blob-settled-at", [None, None])
        },
        FinishedShape {
            extra: &["UPDATE automation_runs SET session_id = 's-gone';"],
            ..in_flight("no-session", [Some("claimed"), Some("claimed")])
        },
        // A Runtime Authority run is held for recovery, fenced or not, and
        // its deadline is never parsed.
        FinishedShape {
            name: "authority",
            authority: true,
            fenced: true,
            deadline: Some("2026-10-22 10:30"),
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        FinishedShape {
            name: "authority-ambiguous",
            authority: true,
            attempts: &[(1, "ambiguous", Some("2026-10-22T09:20:00.000Z"))],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        FinishedShape {
            name: "authority-ambiguous-unsettled",
            authority: true,
            attempts: &[(1, "ambiguous", None)],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        FinishedShape {
            authority: true,
            attempts: &[(1, "started", None), (2, "started", None)],
            ..in_flight("authority-unresolved-twice", [None, None])
        },
        FinishedShape {
            authority: true,
            attempts: &[(1, "succeeded", Some("2026-10-22T09:20:00.000Z"))],
            ..in_flight("authority-resolved", [None, None])
        },
        FinishedShape {
            authority: true,
            extra: &["UPDATE automation_runs SET timeout_at = CAST(timeout_at AS BLOB);"],
            ..in_flight("authority-blob-deadline", [None, None])
        },
        // A live session is not finished: timeout enforcement stops it at the
        // deadline.
        FinishedShape {
            session: ("running", None, None),
            ..in_flight("live", [Some("claimed"), Some("recovery_required")])
        },
        // Another run of the routine, on its own occurrence, changes nothing.
        FinishedShape {
            name: "other-run",
            extra: &[OTHER_RUN, OTHER_ATTEMPT],
            ..FINISHED
        },
        FinishedShape {
            name: "other-run-expired-cancellation",
            extra: &[
                OTHER_RUN,
                OTHER_ATTEMPT,
                "INSERT INTO automation_cancellations
                    (adoption_key, request_digest, automation_id, run_id, attempt_id,
                     session_id, scope, requested_by_json, reason, state, requested_at,
                     execution_expires_at)
                 VALUES ('cancel-other', 'digest', 'daily', 'r-other', 'a-other', 's-other',
                         'run', '{\"principalId\":\"operator\"}', 'stop', 'requested',
                         '2026-10-22T08:00:00.000Z', '2026-10-22T09:00:00.000Z');",
            ],
            ..FINISHED
        },
        FinishedShape {
            name: "other-run-fenced",
            extra: &[
                OTHER_RUN,
                OTHER_ATTEMPT,
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, acquired_at, execution_expires_at)
                 VALUES ('r-other', 's-other', 'timeout', '2026-10-22T09:00:00.000Z',
                         '2099-01-01T00:00:00.000Z');",
            ],
            ..FINISHED
        },
        FinishedShape {
            name: "other-run-later-attempt",
            extra: &[
                OTHER_RUN,
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, state, prior_attempt_number,
                     prior_disposition, retry_classification, not_before, opened_at)
                 VALUES ('a-other', 'r-other', 'o-other', 2, 'key-other', 1, 'adopted', 1,
                         'failed', 'automatic_retry', '2026-10-21T09:00:00.000Z',
                         '2026-10-21T09:00:00.000Z');",
            ],
            ..FINISHED
        },
        FinishedShape {
            name: "other-session-exit",
            extra: &["INSERT INTO sessions
                    (id, project_root, harness, title, status, created_at, updated_at)
                 VALUES ('s-other', '/PRIVATE', 'codex', 'PRIVATE', 'completed',
                         '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
                 INSERT INTO events (id, session_id, kind, payload_json, created_at)
                 VALUES ('e-other', 's-other', 'exit', '{}', 'yesterday');"],
            ..FINISHED
        },
        FinishedShape {
            name: "authority-beside-other-run",
            authority: true,
            extra: &[OTHER_RUN, OTHER_ATTEMPT],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        // Another routine's run on the occurrence counts as a rival.
        FinishedShape {
            extra: &["INSERT INTO sessions
                    (id, project_root, harness, title, status, created_at, updated_at)
                 VALUES ('s-stray', '/PRIVATE', 'codex', 'PRIVATE', 'running',
                         '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
                 INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, session_id, runtime, status, started_at)
                 VALUES ('r-stray', 'other', 'o-done', 's-stray', 'coven-code', 'running',
                         '2026-10-20T09:00:00.000Z');"],
            ..in_flight(
                "rival-of-another-routine",
                [Some("succeeded"), Some("succeeded")],
            )
        },
        FinishedShape {
            cancelling: Some(("requested", "2026-10-22T10:00:00.000Z")),
            ..in_flight(
                "cancellation-expires-now",
                [Some("succeeded"), Some("succeeded")],
            )
        },
        FinishedShape {
            name: "authority-dispatching",
            authority: true,
            attempts: &[(1, "dispatching", None)],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        FinishedShape {
            name: "authority-observing",
            authority: true,
            attempts: &[(1, "observing", None)],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        FinishedShape {
            attempts: &[],
            fenced: true,
            ..in_flight("fenced-no-attempt", [Some("claimed"), Some("claimed")])
        },
        FinishedShape {
            attempts: &[(1, "ambiguous", Some("2026-10-22T09:20:00.000Z"))],
            cancelling: Some(("stopping", "2099-01-01T00:00:00.000Z")),
            ..in_flight("ambiguous-stopping", [Some("claimed"), Some("claimed")])
        },
        FinishedShape {
            deadline: None,
            attempts: &[(1, "ambiguous", None)],
            ..in_flight("ambiguous-unsettled-undated", [None, None])
        },
        FinishedShape {
            attempts: &[
                (1, "started", None),
                (2, "failed", Some("2026-10-22T09:20:00.000Z")),
            ],
            ..in_flight(
                "failed-beside-live-attempt",
                [Some("claimed"), Some("failed")],
            )
        },
        FinishedShape {
            extra: &["UPDATE sessions SET exit_code = 0.5;"],
            ..in_flight("real-exit-code", [None, None])
        },
        // The latest exit by time decides, though its id sorts first.
        FinishedShape {
            extra: &[
                "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                      VALUES ('e-before', 's-done', 'exit', '{}', 'yesterday');",
            ],
            ..in_flight("unparseable-exit-sorts-by-time", [None, None])
        },
        // Settlement decodes these as text whether or not it parses them, and
        // recording a plain run's finish reads its pinned definition.
        FinishedShape {
            deadline: None,
            extra: &["UPDATE events SET created_at = CAST(X'FF' AS TEXT);"],
            ..in_flight("invalid-utf8-exit-undated", [None, None])
        },
        FinishedShape {
            extra: &["UPDATE automation_attempts SET settled_at = CAST(X'FF' AS TEXT);"],
            ..in_flight("invalid-utf8-settled-at", [None, None])
        },
        FinishedShape {
            authority: true,
            extra: &["UPDATE automation_runs SET timeout_at = CAST(X'FF' AS TEXT);"],
            ..in_flight("authority-invalid-utf8-deadline", [None, None])
        },
        FinishedShape {
            extra: &["UPDATE automation_runs SET definition_json = CAST('{}' AS BLOB);"],
            ..in_flight("blob-definition", [None, None])
        },
        FinishedShape {
            extra: &["UPDATE automation_runs SET definition_json = CAST(X'FF' AS TEXT);"],
            ..in_flight("invalid-utf8-definition", [None, None])
        },
        // A held failure reads its definition only once its deadline passes.
        FinishedShape {
            session: ("completed", Some(1), Some("2026-10-22T09:30:00.000Z")),
            extra: &["UPDATE automation_runs SET definition_json = CAST(X'FF' AS TEXT);"],
            ..in_flight("held-invalid-utf8-definition", [Some("claimed"), None])
        },
        FinishedShape {
            name: "authority-blob-definition",
            authority: true,
            extra: &["UPDATE automation_runs SET definition_json = CAST('{}' AS BLOB);"],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        FinishedShape {
            name: "authority-invalid-utf8-definition",
            authority: true,
            extra: &["UPDATE automation_runs SET definition_json = CAST(X'FF' AS TEXT);"],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..FINISHED
        },
        // Recovery is named first; settlement still settles the run.
        FinishedShape {
            name: "recovery",
            occurrence: "recovery_required",
            code: WorkCode::RecoveryRequired,
            next_step: NextStep::CancelRun,
            ..FINISHED
        },
    ];
    let at = |minute: u32| Utc.with_ymd_and_hms(2026, 10, 22, 10, minute, 0).unwrap();
    let mut seen = Vec::new();
    for shape in &shapes {
        let (temp, conn) = store();
        insert_definition_at(
            &conn,
            &definition("daily", "PAUSED"),
            "2026-10-01T00:00:00.000Z",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
                 lease_expires_at, created_at, updated_at)
             VALUES ('o-done', 'daily', '2026-10-21T09:00:00.000Z', 'manual', ?1, 1,
                     'operator', '2099-01-01T00:00:00.000Z', '2026-10-21T09:00:00.000Z',
                     '2026-10-21T09:00:00.000Z')",
            [shape.occurrence],
        )
        .unwrap();
        let (status, exit_code, exited) = shape.session;
        session(&conn, "s-done", status);
        conn.execute(
            "UPDATE sessions SET exit_code = ?1 WHERE id = 's-done'",
            [exit_code],
        )
        .unwrap();
        if let Some(exited) = exited {
            conn.execute(
                "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                 VALUES ('e-done', 's-done', 'exit', '{}', ?1)",
                [exited],
            )
            .unwrap();
        }
        run_keyed(
            &conn,
            "r-done",
            "daily",
            Some("o-done"),
            "running",
            shape.authority,
        );
        conn.execute(
            "UPDATE automation_runs SET timeout_at = ?1, session_id = 's-done'
             WHERE id = 'r-done'",
            [shape.deadline],
        )
        .unwrap();
        // Ids that do not sort like attempt numbers; the latest attempt
        // carries the session.
        let latest = shape.attempts.last().map_or(1, |(number, _, _)| *number);
        for (number, state, settled_at) in shape.attempts {
            let id = format!("a-done-{}", 20 - number);
            conn.execute(
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, state, prior_attempt_number,
                     prior_disposition, retry_classification, not_before, session_id,
                     state_reason, opened_at, settled_at)
                 VALUES (?1, 'r-done', 'o-done', ?2, 'PRIVATE-' || ?1, 1, ?3,
                         CASE WHEN ?2 > 1 THEN ?2 - 1 END, CASE WHEN ?2 > 1 THEN 'failed' END,
                         CASE WHEN ?2 > 1 THEN 'automatic_retry' ELSE 'initial' END,
                         '2026-10-21T09:00:00.000Z', CASE WHEN ?5 THEN 's-done' END,
                         'PRIVATE-REASON', '2026-10-21T09:00:00.000Z', ?4)",
                rusqlite::params![id, number, state, settled_at, *number == latest],
            )
            .unwrap();
        }
        if shape.rival {
            run_keyed(&conn, "r-rival", "daily", Some("o-done"), "running", false);
            session(&conn, "s-rival", "running");
            conn.execute_batch(
                "UPDATE automation_runs SET session_id = 's-rival' WHERE id = 'r-rival';",
            )
            .unwrap();
            attempt(&conn, "a-rival", "r-rival", 1, "started", Some("s-rival"));
        }
        if shape.fenced {
            conn.execute_batch(
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, acquired_at, execution_expires_at)
                 VALUES ('r-done', 's-done', 'timeout', '2026-10-22T09:00:00.000Z',
                         '2099-01-01T00:00:00.000Z');",
            )
            .unwrap();
        }
        if let Some((state, expires)) = shape.cancelling {
            conn.execute(
                "INSERT INTO automation_cancellations
                    (adoption_key, request_digest, automation_id, run_id, attempt_id,
                     session_id, scope, requested_by_json, reason, state, requested_at,
                     execution_expires_at)
                 VALUES ('cancel-done', 'digest', 'daily', 'r-done', ?3, 's-done', 'run',
                         '{\"principalId\":\"operator\"}', 'stop', ?1,
                         '2026-10-22T08:00:00.000Z', ?2)",
                rusqlite::params![state, expires, format!("a-done-{}", 20 - latest)],
            )
            .unwrap();
        }
        for sql in shape.extra {
            conn.execute_batch(sql).unwrap();
        }
        let item = found(&conn, "daily").work.into_iter().next().unwrap();
        // TEMP tables cannot change what the finished run reads as.
        let explained = work_of(&conn, "daily");
        conn.execute_batch(
            "CREATE TEMP TABLE sessions AS SELECT * FROM main.sessions;
             UPDATE temp.sessions SET status = CASE status
                 WHEN 'running' THEN 'completed' ELSE 'running' END, exit_code = 'x';
             CREATE TEMP TABLE events AS SELECT * FROM main.events;
             UPDATE temp.events SET id = 'shadow-' || id, created_at = 'never';
             CREATE TEMP TABLE automation_attempts AS SELECT * FROM main.automation_attempts;
             UPDATE temp.automation_attempts SET id = 'shadow-' || id, settled_at = NULL,
                 state = CASE state WHEN 'started' THEN 'adopted' ELSE 'started' END;
             CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs;
             UPDATE temp.automation_runs SET timeout_at = 'never', authority_profile =
                 CASE WHEN authority_profile IS NULL THEN 'shadow' END;
             CREATE TEMP TABLE automation_stop_fences AS
                 SELECT * FROM main.automation_stop_fences WHERE 0;
             INSERT INTO temp.automation_stop_fences (run_id) VALUES ('r-done');
             CREATE TEMP TABLE automation_cancellations AS
                 SELECT * FROM main.automation_cancellations WHERE 0;
             INSERT INTO temp.automation_cancellations
                 (run_id, attempt_id, session_id, state, execution_expires_at)
                 SELECT 'r-done', id, 's-done', 'requested', '2000-01-01T00:00:00.000Z'
                 FROM main.automation_attempts;",
        )
        .unwrap();
        assert_eq!(work_of(&conn, "daily"), explained, "{}", shape.name);
        conn.execute_batch(
            "DROP TABLE temp.sessions;
             DROP TABLE temp.events;
             DROP TABLE temp.automation_attempts;
             DROP TABLE temp.automation_runs;
             DROP TABLE temp.automation_stop_fences;
             DROP TABLE temp.automation_cancellations;",
        )
        .unwrap();
        let after = [at(0), at(30)].map(|instant| {
            crate::automations::daemon_tick::reconcile_automation_runs(
                temp.path(),
                &conn,
                &UnconfirmedKill,
                instant,
                None,
            )
            .ok()
            .map(|()| {
                conn.query_row(
                    "SELECT state FROM automation_occurrences WHERE id = 'o-done'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
            })
        });
        seen.push((shape.name, item.code, item.next_step, after));
    }
    let expected: Vec<_> = shapes
        .iter()
        .map(|shape| {
            (
                shape.name,
                shape.code,
                shape.next_step,
                shape.after.map(|state| state.map(str::to_string)),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}

/// A runtime whose kills are confirmed.
struct ConfirmedKill;

impl crate::api::SessionRuntime for ConfirmedKill {
    fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

struct TimeoutShape {
    name: &'static str,
    occurrence: &'static str,
    lease: Option<&'static str>,
    deadline: Option<&'static str>,
    session: &'static str,
    /// (number, state, carries the run's session)
    attempts: &'static [(i64, &'static str, bool)],
    authority: bool,
    /// SQL run against the store before explaining, to add or corrupt rows.
    extra: &'static [&'static str],
    code: WorkCode,
    next_step: NextStep,
    deadline_at: Option<&'static str>,
    /// The occurrence after the daemon's reconciliation at 10:00 with a
    /// runtime that confirms the kill, and with one that never does, followed
    /// for an abandoned launch by its run's latest attempt; `None` when that
    /// pass fails.
    after: [Option<&'static str>; 2],
}

const TIMED_OUT: TimeoutShape = TimeoutShape {
    name: "timed-out",
    occurrence: "claimed",
    lease: Some("2099-01-01T00:00:00.000Z"),
    deadline: Some("2026-10-22T09:30:00.000Z"),
    session: "running",
    attempts: &[(1, "started", true)],
    authority: false,
    extra: &[],
    code: WorkCode::TimeoutExpired,
    next_step: NextStep::AwaitSettlement,
    deadline_at: Some("2026-10-22T09:30:00.000Z"),
    after: [Some("failed"), Some("recovery_required")],
};

/// A stop fence on the run under test: its owner, and when it expires.
macro_rules! late_fence {
    ($owner:literal, $expires:literal) => {
        concat!(
            "INSERT INTO automation_stop_fences
                (run_id, session_id, owner, acquired_at, execution_expires_at)
             VALUES ('r-late', 's-late', '",
            $owner,
            "', '2026-10-22T09:00:00.000Z', '",
            $expires,
            "');"
        )
    };
}

/// Renames the session of the run under test, given its new id as SQL.
macro_rules! late_session_id {
    ($id:literal) => {
        concat!(
            "PRAGMA foreign_keys = OFF;
             UPDATE sessions SET id = ",
            $id,
            " WHERE id = 's-late';
             UPDATE automation_runs SET session_id = ",
            $id,
            " WHERE session_id = 's-late';
             UPDATE automation_attempts SET session_id = ",
            $id,
            " WHERE session_id = 's-late';
             PRAGMA foreign_keys = ON;"
        )
    };
}

/// A requested cancellation of the run under test's attempt 1.
const LATE_CANCELLATION: &str = "
    INSERT INTO automation_cancellations
        (adoption_key, request_digest, automation_id, run_id, attempt_id, session_id,
         scope, requested_by_json, reason, state, requested_at, execution_expires_at)
    VALUES ('cancel-late', 'digest', 'daily', 'r-late', 'a-late-19', 's-late', 'run',
            '{\"principalId\":\"operator\"}', 'stop', 'requested',
            '2026-10-22T09:00:00.000Z', '2099-01-01T00:00:00.000Z');";

fn timeout_store(shape: &TimeoutShape) -> (tempfile::TempDir, Connection) {
    let (temp, conn) = store();
    insert_definition_at(
        &conn,
        &definition("daily", "PAUSED"),
        "2026-10-01T00:00:00.000Z",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
             lease_expires_at, created_at, updated_at)
         VALUES ('o-late', 'daily', '2026-10-21T09:00:00.000Z', 'manual', ?1, 1,
                 'operator', ?2, '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z')",
        rusqlite::params![shape.occurrence, shape.lease],
    )
    .unwrap();
    session(&conn, "s-late", shape.session);
    run_keyed(
        &conn,
        "r-late",
        "daily",
        Some("o-late"),
        "running",
        shape.authority,
    );
    conn.execute(
        "UPDATE automation_runs SET timeout_at = ?1, session_id = 's-late'
         WHERE id = 'r-late'",
        [shape.deadline],
    )
    .unwrap();
    // Ids that do not sort like attempt numbers.
    for (number, state, carries) in shape.attempts {
        let live = matches!(*state, "dispatching" | "started" | "observing");
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, state, prior_attempt_number,
                 prior_disposition, retry_classification, not_before, session_id,
                 state_reason, opened_at, settled_at)
             VALUES (?1, 'r-late', 'o-late', ?2, 'PRIVATE-' || ?1, 1, ?3,
                     CASE WHEN ?2 > 1 THEN ?2 - 1 END, CASE WHEN ?2 > 1 THEN 'failed' END,
                     CASE WHEN ?2 > 1 THEN 'automatic_retry' ELSE 'initial' END,
                     '2026-10-21T09:00:00.000Z', CASE WHEN ?4 THEN 's-late' END,
                     'PRIVATE-REASON', '2026-10-21T09:00:00.000Z',
                     CASE WHEN NOT ?5 THEN '2026-10-22T09:20:00.000Z' END)",
            rusqlite::params![
                format!("a-late-{}", 20 - number),
                number,
                state,
                carries,
                live
            ],
        )
        .unwrap();
    }
    for sql in shape.extra {
        conn.execute_batch(sql).unwrap();
    }
    (temp, conn)
}

/// What the daemon's reconciliation does with a run whose session is still
/// running past its deadline, one store per shape and runtime so that a
/// failing pass hides nothing.
#[test]
fn each_run_past_its_deadline_names_what_reconciliation_does_with_it() {
    let in_flight = |name, after| TimeoutShape {
        name,
        code: WorkCode::InFlight,
        next_step: NextStep::InspectOccurrence,
        deadline_at: None,
        after,
        ..TIMED_OUT
    };
    let shapes = [
        TIMED_OUT,
        TimeoutShape {
            name: "running",
            occurrence: "running",
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "authority",
            authority: true,
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..TIMED_OUT
        },
        // Enforcement parses the whole deadline, and expires it at its instant.
        TimeoutShape {
            name: "deadline-now",
            deadline: Some("2026-10-22T10:00:00.000Z"),
            deadline_at: Some("2026-10-22T10:00:00.000Z"),
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "deadline-offset",
            deadline: Some("2026-10-22T10:59:00+01:00"),
            deadline_at: Some("2026-10-22T09:59:00.000Z"),
            ..TIMED_OUT
        },
        TimeoutShape {
            deadline: Some("2026-10-22T09:01:00-01:00"),
            ..in_flight(
                "deadline-ahead-sorting-behind",
                [Some("claimed"), Some("claimed")],
            )
        },
        TimeoutShape {
            deadline: Some("2026-10-22T10:00:00.001Z"),
            ..in_flight("deadline-ahead", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            deadline: None,
            ..in_flight("no-deadline", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            deadline: Some("soon"),
            ..in_flight("unparseable-deadline", [None, None])
        },
        TimeoutShape {
            extra: &["UPDATE automation_runs SET timeout_at = CAST(timeout_at AS BLOB);"],
            ..in_flight("blob-deadline", [None, None])
        },
        TimeoutShape {
            extra: &["UPDATE automation_runs SET timeout_at = CAST(X'FF' AS TEXT);"],
            ..in_flight("invalid-utf8-deadline", [None, None])
        },
        // Restart containment can reach these first, from receipts the store
        // does not hold.
        TimeoutShape {
            session: "created",
            ..in_flight("created", [Some("failed"), Some("recovery_required")])
        },
        TimeoutShape {
            session: "orphaned",
            ..in_flight("orphaned", [Some("failed"), Some("recovery_required")])
        },
        TimeoutShape {
            name: "dispatching",
            attempts: &[(1, "dispatching", true)],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "dispatching-unbound",
            attempts: &[(1, "dispatching", false)],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "observing",
            attempts: &[(1, "observing", true)],
            ..TIMED_OUT
        },
        TimeoutShape {
            attempts: &[(1, "started", false)],
            ..in_flight("started-unbound", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            attempts: &[(1, "ambiguous", true)],
            ..in_flight("ambiguous", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            attempts: &[(1, "succeeded", true)],
            ..in_flight("succeeded", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            attempts: &[],
            ..in_flight("no-attempt", [Some("claimed"), Some("claimed")])
        },
        // The stop needs exactly one attempt live on the session, whichever
        // it is.
        TimeoutShape {
            attempts: &[(1, "dispatching", false), (2, "failed", true)],
            ..in_flight(
                "failed-beside-unbound-dispatch",
                [None, Some("recovery_required")],
            )
        },
        TimeoutShape {
            attempts: &[(1, "dispatching", false), (2, "dispatching", false)],
            ..in_flight("two-unbound-dispatches", [None, None])
        },
        TimeoutShape {
            attempts: &[(1, "dispatching", false), (2, "started", true)],
            ..in_flight("started-beside-unbound-dispatch", [None, None])
        },
        TimeoutShape {
            name: "started-after-settled",
            attempts: &[(1, "failed", false), (2, "started", true)],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "dispatch-before-unbound-start",
            attempts: &[(1, "dispatching", false), (2, "started", false)],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "dispatch-before-unbound-failure",
            attempts: &[(1, "dispatching", false), (2, "failed", false)],
            ..TIMED_OUT
        },
        TimeoutShape {
            attempts: &[(1, "dispatching", false), (2, "ambiguous", true)],
            ..in_flight(
                "dispatch-before-bound-ambiguity",
                [None, Some("recovery_required")],
            )
        },
        // Settling a dispatch binds it to the session, which no other
        // attempt may hold.
        TimeoutShape {
            attempts: &[(1, "failed", true), (2, "dispatching", false)],
            ..in_flight("dispatching-after-bound", [None, Some("recovery_required")])
        },
        TimeoutShape {
            attempts: &[(1, "dispatching", false)],
            extra: &["
                INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, runtime, status, started_at)
                VALUES ('r-taken', 'daily', 'o-late', 'coven-code', 'failed',
                        '2026-10-21T08:00:00.000Z');
                INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, state, retry_classification, not_before,
                     session_id, opened_at, settled_at)
                VALUES ('a-taken', 'r-taken', 'o-late', 1, 'key-taken', 1, 'failed',
                        'initial', '2026-10-21T08:00:00.000Z', 's-late',
                        '2026-10-21T08:00:00.000Z', '2026-10-21T08:30:00.000Z');"],
            ..in_flight(
                "dispatching-session-taken",
                [None, Some("recovery_required")],
            )
        },
        TimeoutShape {
            name: "authority-dispatching-session-taken",
            authority: true,
            attempts: &[(1, "failed", true), (2, "dispatching", false)],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..TIMED_OUT
        },
        // Another running run's pass can bind the session first.
        TimeoutShape {
            extra: &[
                OTHER_RUN,
                "UPDATE automation_runs
                 SET session_id = 's-late', timeout_at = '2026-10-22T09:00:00.000Z'
                 WHERE id = 'r-other';
                 INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, state, retry_classification, not_before,
                     opened_at)
                 VALUES ('a-other', 'r-other', 'o-other', 1, 'key-other', 1, 'dispatching',
                         'initial', '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');",
            ],
            ..in_flight("session-shared", [None, Some("recovery_required")])
        },
        TimeoutShape {
            name: "session-shared-by-settled-run",
            extra: &[
                OTHER_RUN,
                "UPDATE automation_runs SET session_id = 's-late', status = 'failed'
                 WHERE id = 'r-other';",
            ],
            ..TIMED_OUT
        },
        TimeoutShape {
            attempts: &[(1, "dispatching", false)],
            extra: &[
                OTHER_RUN,
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, state, retry_classification, not_before,
                     session_id, opened_at)
                 VALUES ('a-other', 'r-other', 'o-other', 1, 'key-other', 1, 'started',
                         'initial', '2026-10-21T09:00:00.000Z', 's-late',
                         '2026-10-21T09:00:00.000Z');",
            ],
            ..in_flight(
                "dispatching-session-live-elsewhere",
                [None, Some("recovery_required")],
            )
        },
        // Enforcement leaves a fenced run to that stop, or holds it for
        // recovery once its own timeout stop has expired.
        TimeoutShape {
            extra: &[late_fence!("timeout", "2026-10-22T10:00:20.000Z")],
            ..in_flight("fenced", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            extra: &[late_fence!("timeout", "2026-10-22T09:59:00.000Z")],
            ..in_flight(
                "fence-expired",
                [Some("recovery_required"), Some("recovery_required")],
            )
        },
        TimeoutShape {
            extra: &[late_fence!("recovery", "2026-10-22T09:59:00.000Z")],
            ..in_flight("recovery-fence", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            extra: &[late_fence!("timeout", "never")],
            ..in_flight("fence-unparseable", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            extra: &[LATE_CANCELLATION],
            ..in_flight("cancellation-requested", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            extra: &[
                LATE_CANCELLATION,
                "UPDATE automation_cancellations SET state = 'stopping';",
            ],
            ..in_flight("cancellation-stopping", [Some("claimed"), Some("claimed")])
        },
        TimeoutShape {
            extra: &[
                LATE_CANCELLATION,
                "UPDATE automation_cancellations
                 SET execution_expires_at = '2026-10-22T09:00:00.000Z';",
            ],
            ..in_flight(
                "cancellation-expired",
                [Some("failed"), Some("recovery_required")],
            )
        },
        // Reconciling an expired stopping cancellation holds the run for
        // recovery before timeout enforcement reaches it.
        TimeoutShape {
            name: "stopping-expired",
            extra: &[
                LATE_CANCELLATION,
                "UPDATE automation_cancellations SET state = 'stopping',
                     execution_expires_at = '2026-10-22T09:00:00.000Z';",
            ],
            code: WorkCode::CancellationExpired,
            next_step: NextStep::AwaitCancellation,
            deadline_at: None,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "cancellation-rejected",
            extra: &[
                LATE_CANCELLATION,
                "UPDATE automation_cancellations SET state = 'rejected';",
            ],
            ..TIMED_OUT
        },
        TimeoutShape {
            extra: &["
                INSERT INTO sessions
                    (id, project_root, harness, title, status, created_at, updated_at)
                VALUES ('s-rival', '/PRIVATE', 'codex', 'PRIVATE', 'running',
                        '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
                INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, session_id, runtime, status, started_at)
                VALUES ('r-rival', 'daily', 'o-late', 's-rival', 'coven-code', 'running',
                        '2026-10-21T09:00:00.000Z');"],
            ..in_flight("rival", [Some("failed"), Some("recovery_required")])
        },
        TimeoutShape {
            name: "other-run",
            extra: &[OTHER_RUN, OTHER_ATTEMPT],
            ..TIMED_OUT
        },
        // Only the run's own attempts, cancellations and running rivals count.
        TimeoutShape {
            name: "other-run-dispatching",
            extra: &[
                OTHER_RUN,
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, state, retry_classification, not_before,
                     opened_at)
                 VALUES ('a-other', 'r-other', 'o-other', 1, 'key-other', 1, 'dispatching',
                         'initial', '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');",
            ],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "other-run-cancelling",
            extra: &[
                OTHER_RUN,
                OTHER_ATTEMPT,
                "INSERT INTO automation_cancellations
                    (adoption_key, request_digest, automation_id, run_id, attempt_id,
                     session_id, scope, requested_by_json, reason, state, requested_at,
                     execution_expires_at)
                 VALUES ('cancel-other', 'digest', 'daily', 'r-other', 'a-other', 's-other',
                         'run', '{\"principalId\":\"operator\"}', 'stop', 'requested',
                         '2026-10-22T09:00:00.000Z', '2099-01-01T00:00:00.000Z');",
            ],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "other-run-fenced",
            extra: &[
                OTHER_RUN,
                OTHER_ATTEMPT,
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, acquired_at, execution_expires_at)
                 VALUES ('r-other', 's-other', 'timeout', '2026-10-22T09:00:00.000Z',
                         '2099-01-01T00:00:00.000Z');",
            ],
            ..TIMED_OUT
        },
        // Another routine's run on the occurrence counts as a rival.
        TimeoutShape {
            extra: &["
                INSERT INTO sessions
                    (id, project_root, harness, title, status, created_at, updated_at)
                VALUES ('s-stray', '/PRIVATE', 'codex', 'PRIVATE', 'running',
                        '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
                INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, session_id, runtime, status, started_at)
                VALUES ('r-stray', 'other', 'o-late', 's-stray', 'coven-code', 'running',
                        '2026-10-21T09:00:00.000Z');"],
            ..in_flight(
                "rival-of-another-routine",
                [Some("failed"), Some("recovery_required")],
            )
        },
        TimeoutShape {
            name: "settled-rival",
            extra: &["
                INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, runtime, status, started_at)
                VALUES ('r-settled', 'daily', 'o-late', 'coven-code', 'failed',
                        '2026-10-21T08:00:00.000Z');"],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "authority-beside-other-run",
            authority: true,
            extra: &[OTHER_RUN, OTHER_ATTEMPT],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "lease-expired",
            extra: &["UPDATE automation_occurrences
                      SET lease_expires_at = '2026-10-22T09:00:00.000Z';"],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "authority-unbound-dispatch",
            authority: true,
            attempts: &[(1, "dispatching", false)],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..TIMED_OUT
        },
        TimeoutShape {
            authority: true,
            attempts: &[(1, "ambiguous", false), (2, "started", true)],
            ..in_flight(
                "authority-beside-ambiguous",
                [None, Some("recovery_required")],
            )
        },
        TimeoutShape {
            authority: true,
            attempts: &[(1, "observing", false), (2, "started", true)],
            ..in_flight(
                "authority-beside-unbound-observing",
                [None, Some("recovery_required")],
            )
        },
        TimeoutShape {
            authority: true,
            attempts: &[(1, "ambiguous", true)],
            ..in_flight("authority-ambiguous", [Some("claimed"), Some("claimed")])
        },
        // Settlement's scan decodes every running run's values, and
        // recording a plain run's finish reads its definition.
        TimeoutShape {
            extra: &["UPDATE automation_runs SET definition_json = CAST('{}' AS BLOB);"],
            ..in_flight("blob-definition", [None, Some("recovery_required")])
        },
        TimeoutShape {
            extra: &["UPDATE automation_runs SET definition_json = CAST(X'FF' AS TEXT);"],
            ..in_flight("invalid-utf8-definition", [None, Some("recovery_required")])
        },
        TimeoutShape {
            name: "authority-blob-definition",
            authority: true,
            extra: &["UPDATE automation_runs SET definition_json = CAST('{}' AS BLOB);"],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..TIMED_OUT
        },
        TimeoutShape {
            extra: &["UPDATE sessions SET exit_code = 'zero';"],
            ..in_flight("text-exit-code", [Some("failed"), None])
        },
        TimeoutShape {
            extra: &["UPDATE sessions SET updated_at = X'00';"],
            ..in_flight("blob-update", [Some("failed"), None])
        },
        TimeoutShape {
            extra: &["UPDATE sessions SET updated_at = CAST(X'FF' AS TEXT);"],
            ..in_flight("invalid-utf8-update", [Some("failed"), None])
        },
        TimeoutShape {
            extra: &["UPDATE automation_attempts SET settled_at = X'00';"],
            ..in_flight(
                "blob-settled-at",
                [Some("failed"), Some("recovery_required")],
            )
        },
        TimeoutShape {
            authority: true,
            extra: &["UPDATE automation_attempts SET settled_at = X'00';"],
            ..in_flight(
                "authority-blob-settled-at",
                [None, Some("recovery_required")],
            )
        },
        TimeoutShape {
            name: "unparseable-update",
            extra: &["UPDATE sessions SET updated_at = 'never';"],
            ..TIMED_OUT
        },
        TimeoutShape {
            name: "recovery",
            occurrence: "recovery_required",
            code: WorkCode::RecoveryRequired,
            next_step: NextStep::CancelRun,
            deadline_at: None,
            after: [Some("recovery_required"), Some("recovery_required")],
            ..TIMED_OUT
        },
    ];
    let mut seen = Vec::new();
    for shape in &shapes {
        let (_temp, conn) = timeout_store(shape);
        let item = found(&conn, "daily").work.into_iter().next().unwrap();
        // TEMP tables cannot change what the late run reads as.
        let explained = work_of(&conn, "daily");
        conn.execute_batch(
            "CREATE TEMP TABLE sessions AS SELECT * FROM main.sessions;
             UPDATE temp.sessions SET status = CASE status
                 WHEN 'running' THEN 'completed' ELSE 'running' END, exit_code = 'x';
             CREATE TEMP TABLE automation_attempts AS SELECT * FROM main.automation_attempts;
             UPDATE temp.automation_attempts SET id = 'shadow-' || id, session_id = NULL,
                 state = CASE state WHEN 'started' THEN 'ambiguous' ELSE 'dispatching' END;
             CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs;
             UPDATE temp.automation_runs SET timeout_at = 'never', authority_profile =
                 CASE WHEN authority_profile IS NULL THEN 'shadow' END;
             INSERT INTO temp.automation_attempts (id, run_id, state)
                 VALUES ('shadow-extra', 'r-late', 'observing');
             INSERT INTO temp.automation_runs (id, occurrence_id, session_id, status)
                 VALUES ('r-shadow', 'o-late', 's-late', 'running');
             CREATE TEMP TABLE automation_stop_fences AS
                 SELECT * FROM main.automation_stop_fences WHERE 0;
             INSERT INTO temp.automation_stop_fences (run_id) VALUES ('r-late');
             CREATE TEMP TABLE automation_cancellations AS
                 SELECT * FROM main.automation_cancellations WHERE 0;
             INSERT INTO temp.automation_cancellations (run_id, state)
                 VALUES ('r-late', 'requested');",
        )
        .unwrap();
        assert_eq!(work_of(&conn, "daily"), explained, "{}", shape.name);
        let after = [
            &ConfirmedKill as &dyn crate::api::SessionRuntime,
            &UnconfirmedKill,
        ]
        .map(|runtime| {
            let (temp, conn) = timeout_store(shape);
            crate::automations::daemon_tick::reconcile_automation_runs(
                temp.path(),
                &conn,
                runtime,
                now(),
                None,
            )
            .ok()
            .map(|()| {
                conn.query_row(
                    "SELECT state FROM automation_occurrences WHERE id = 'o-late'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
            })
        });
        seen.push((
            shape.name,
            item.code,
            item.next_step,
            item.deadline_at,
            after,
        ));
    }
    let expected: Vec<_> = shapes
        .iter()
        .map(|shape| {
            (
                shape.name,
                shape.code,
                shape.next_step,
                shape.deadline_at.map(str::to_string),
                shape.after.map(|state| state.map(str::to_string)),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}

/// The occurrence after the daemon's reconciliation at 10:00 with `runtime`,
/// or `None` when the pass fails.
fn occurrence_after_pass(
    temp: &tempfile::TempDir,
    conn: &Connection,
    runtime: &dyn crate::api::SessionRuntime,
) -> Option<String> {
    crate::automations::daemon_tick::reconcile_automation_runs(
        temp.path(),
        conn,
        runtime,
        now(),
        None,
    )
    .ok()
    .map(|()| {
        conn.query_row(
            "SELECT state FROM automation_occurrences WHERE id = 'o-late'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
    })
}

/// The occurrence and its run's latest attempt after the daemon's
/// reconciliation at 10:00 with `runtime`, or `None` when the pass fails.
fn launch_after_pass(
    temp: &tempfile::TempDir,
    conn: &Connection,
    runtime: &dyn crate::api::SessionRuntime,
) -> Option<String> {
    occurrence_after_pass(temp, conn, runtime).map(|occurrence| {
        let attempt: Option<String> = conn
            .query_row(
                "SELECT state FROM automation_attempts WHERE run_id = 'r-late'
                 ORDER BY attempt_number DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        format!("{occurrence}/{}", attempt.as_deref().unwrap_or("none"))
    })
}

const ABANDONED: TimeoutShape = TimeoutShape {
    name: "abandoned",
    lease: Some("2026-10-22T09:00:00.000Z"),
    deadline: None,
    session: "created",
    attempts: &[(1, "dispatching", true)],
    code: WorkCode::LaunchAbandoned,
    deadline_at: None,
    after: [Some("failed/failed"), Some("recovery_required/ambiguous")],
    ..TIMED_OUT
};

/// What the daemon's reconciliation does with a launch whose session has not
/// started when its claim's lease expires, one store per shape and runtime so
/// that a failing pass hides nothing. Each shape gives the occurrence and the
/// run's latest attempt after each pass, and also after a pass that finds a
/// containment receipt, proving the launch already stopped, with a runtime
/// that never confirms a kill.
#[test]
fn each_abandoned_launch_names_what_reconciliation_does_with_it() {
    let in_flight = |name, after| TimeoutShape {
        name,
        code: WorkCode::InFlight,
        next_step: NextStep::InspectOccurrence,
        after,
        ..ABANDONED
    };
    let shapes = [
        (ABANDONED, Some("failed/failed")),
        (
            TimeoutShape {
                name: "authority",
                authority: true,
                next_step: NextStep::AwaitRuntimeAuthority,
                after: [
                    Some("recovery_required/dispatching"),
                    Some("recovery_required/ambiguous"),
                ],
                ..ABANDONED
            },
            Some("recovery_required/dispatching"),
        ),
        (
            TimeoutShape {
                name: "dispatching-unbound",
                attempts: &[(1, "dispatching", false)],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "started",
                attempts: &[(1, "started", true)],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "observing",
                attempts: &[(1, "observing", true)],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        // Recovery compares the lease as stored text.
        (
            TimeoutShape {
                name: "lease-now",
                lease: Some("2026-10-22T10:00:00.000Z"),
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                lease: Some("2026-10-22T10:00:00.001Z"),
                ..in_flight(
                    "lease-ahead",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                lease: Some("2026-10-22T10:30:00+01:00"),
                ..in_flight(
                    "lease-expired-sorting-ahead",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "lease-ahead-sorting-behind",
                lease: Some("2026-10-22T09:30:00-01:00"),
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                lease: None,
                ..in_flight(
                    "no-lease",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "lease-unparseable-sorting-behind",
                lease: Some("0"),
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        // Recovery compares the deadline as stored text too, and leaves a run
        // past it to timeout enforcement, which parses it.
        (
            TimeoutShape {
                name: "deadline-ahead",
                deadline: Some("2026-10-22T11:00:00.000Z"),
                after: [Some("claimed/failed"), Some("recovery_required/ambiguous")],
                ..ABANDONED
            },
            Some("claimed/failed"),
        ),
        (
            TimeoutShape {
                deadline: Some("2026-10-22T10:00:00.000Z"),
                ..in_flight(
                    "deadline-now",
                    [
                        Some("failed/timed_out"),
                        Some("recovery_required/ambiguous"),
                    ],
                )
            },
            Some("failed/timed_out"),
        ),
        (
            TimeoutShape {
                name: "deadline-expired-sorting-ahead",
                deadline: Some("2026-10-22T10:30:00+01:00"),
                after: [
                    Some("failed/timed_out"),
                    Some("recovery_required/ambiguous"),
                ],
                ..ABANDONED
            },
            Some("failed/timed_out"),
        ),
        (
            TimeoutShape {
                deadline: Some("2026-10-22T09:30:00-01:00"),
                ..in_flight(
                    "deadline-ahead-sorting-behind",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("claimed/failed"),
        ),
        (
            TimeoutShape {
                deadline: Some("never"),
                ..in_flight(
                    "unparseable-deadline",
                    [None, Some("recovery_required/ambiguous")],
                )
            },
            None,
        ),
        (
            TimeoutShape {
                name: "authority-unparseable-deadline",
                authority: true,
                deadline: Some("never"),
                next_step: NextStep::AwaitRuntimeAuthority,
                after: [
                    Some("recovery_required/dispatching"),
                    Some("recovery_required/ambiguous"),
                ],
                ..ABANDONED
            },
            Some("recovery_required/dispatching"),
        ),
        (
            TimeoutShape {
                extra: &["UPDATE automation_runs SET timeout_at = X'00';"],
                ..in_flight("blob-deadline", [None, None])
            },
            None,
        ),
        // Only a session not yet started is a launch, and only on a claim.
        (
            TimeoutShape {
                session: "running",
                ..in_flight(
                    "running-session",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("claimed/dispatching"),
        ),
        (
            TimeoutShape {
                session: "orphaned",
                ..in_flight(
                    "orphaned",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                occurrence: "running",
                ..in_flight(
                    "running-occurrence",
                    [Some("running/dispatching"), Some("running/dispatching")],
                )
            },
            Some("failed/failed"),
        ),
        // The stop needs exactly one attempt live on the session, and
        // settling a plain run its latest attempt live.
        (
            TimeoutShape {
                attempts: &[(1, "started", false)],
                ..in_flight(
                    "started-unbound",
                    [Some("claimed/started"), Some("claimed/started")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                attempts: &[],
                ..in_flight("no-attempt", [Some("claimed/none"), Some("claimed/none")])
            },
            Some("failed/none"),
        ),
        (
            TimeoutShape {
                attempts: &[(1, "started", true), (2, "dispatching", false)],
                ..in_flight("two-live", [Some("failed/failed"), None])
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                attempts: &[(1, "dispatching", true), (2, "failed", false)],
                ..in_flight(
                    "latest-settled",
                    [Some("failed/failed"), Some("recovery_required/failed")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "started-after-settled",
                attempts: &[(1, "failed", false), (2, "started", true)],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "live-elsewhere",
                attempts: &[(1, "started", true), (2, "started", false)],
                after: [Some("failed/failed"), Some("recovery_required/started")],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "dispatching-after-ambiguous",
                attempts: &[(1, "ambiguous", true), (2, "dispatching", false)],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                attempts: &[(1, "dispatching", true), (2, "ambiguous", false)],
                ..in_flight(
                    "latest-ambiguous",
                    [
                        Some("failed/ambiguous"),
                        Some("recovery_required/ambiguous"),
                    ],
                )
            },
            Some("failed/ambiguous"),
        ),
        (
            TimeoutShape {
                attempts: &[(1, "ambiguous", true)],
                ..in_flight(
                    "ambiguous",
                    [Some("claimed/ambiguous"), Some("claimed/ambiguous")],
                )
            },
            Some("failed/ambiguous"),
        ),
        (
            TimeoutShape {
                authority: true,
                attempts: &[(1, "ambiguous", false), (2, "dispatching", true)],
                ..in_flight(
                    "authority-beside-ambiguous",
                    [None, Some("recovery_required/ambiguous")],
                )
            },
            None,
        ),
        (
            TimeoutShape {
                authority: true,
                attempts: &[(1, "observing", false), (2, "dispatching", true)],
                ..in_flight(
                    "authority-beside-unbound-observing",
                    [None, Some("recovery_required/ambiguous")],
                )
            },
            None,
        ),
        (
            TimeoutShape {
                authority: true,
                attempts: &[(1, "started", false), (2, "dispatching", true)],
                ..in_flight(
                    "authority-beside-unbound-started",
                    [None, Some("recovery_required/ambiguous")],
                )
            },
            None,
        ),
        // Recovery leaves a run to a pending cancellation or another stop.
        (
            TimeoutShape {
                extra: &[LATE_CANCELLATION],
                ..in_flight(
                    "cancellation-requested",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                extra: &[
                    LATE_CANCELLATION,
                    "UPDATE automation_cancellations SET state = 'stopping';",
                ],
                ..in_flight(
                    "cancellation-stopping",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "cancellation-rejected",
                extra: &[
                    LATE_CANCELLATION,
                    "UPDATE automation_cancellations SET state = 'rejected';",
                ],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                extra: &[late_fence!("recovery", "2099-01-01T00:00:00.000Z")],
                ..in_flight(
                    "recovery-fence",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("claimed/dispatching"),
        ),
        (
            TimeoutShape {
                extra: &[late_fence!("recovery", "2026-10-22T09:59:00.000Z")],
                ..in_flight(
                    "recovery-fence-expired",
                    [
                        Some("recovery_required/ambiguous"),
                        Some("recovery_required/ambiguous"),
                    ],
                )
            },
            Some("recovery_required/ambiguous"),
        ),
        (
            TimeoutShape {
                extra: &[late_fence!("recovery", "never")],
                ..in_flight(
                    "recovery-fence-unparseable",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("claimed/dispatching"),
        ),
        (
            TimeoutShape {
                attempts: &[(1, "started", true), (2, "dispatching", false)],
                extra: &[late_fence!("recovery", "2026-10-22T09:59:00.000Z")],
                ..in_flight("recovery-fence-expired-two-live", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                attempts: &[],
                extra: &[late_fence!("recovery", "2026-10-22T09:59:00.000Z")],
                ..in_flight("recovery-fence-expired-no-attempt", [None, None])
            },
            Some("claimed/none"),
        ),
        (
            TimeoutShape {
                extra: &[late_fence!("timeout", "2099-01-01T00:00:00.000Z")],
                ..in_flight(
                    "timeout-fence",
                    [Some("claimed/dispatching"), Some("claimed/dispatching")],
                )
            },
            Some("claimed/dispatching"),
        ),
        // Another running run can settle the occurrence first, or share the
        // session.
        (
            TimeoutShape {
                extra: &["
                    INSERT INTO sessions
                        (id, project_root, harness, title, status, created_at, updated_at)
                    VALUES ('s-stray', '/PRIVATE', 'codex', 'PRIVATE', 'running',
                            '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
                    INSERT INTO automation_runs
                        (id, automation_id, occurrence_id, session_id, runtime, status,
                         started_at)
                    VALUES ('r-stray', 'other', 'o-late', 's-stray', 'coven-code', 'running',
                            '2026-10-21T09:00:00.000Z');"],
                ..in_flight(
                    "rival",
                    [Some("failed/failed"), Some("recovery_required/ambiguous")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                extra: &[
                    OTHER_RUN,
                    "UPDATE automation_runs SET session_id = 's-late' WHERE id = 'r-other';",
                ],
                ..in_flight(
                    "session-shared",
                    [Some("failed/failed"), Some("recovery_required/ambiguous")],
                )
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "rival-finished",
                extra: &["
                    INSERT INTO sessions
                        (id, project_root, harness, title, status, created_at, updated_at)
                    VALUES ('s-stray', '/PRIVATE', 'codex', 'PRIVATE', 'failed',
                            '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
                    INSERT INTO automation_runs
                        (id, automation_id, occurrence_id, session_id, runtime, status,
                         started_at, finished_at)
                    VALUES ('r-stray', 'other', 'o-late', 's-stray', 'coven-code', 'failed',
                            '2026-10-21T09:00:00.000Z', '2026-10-21T09:10:00.000Z');"],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "session-shared-by-finished-run",
                extra: &[
                    OTHER_RUN,
                    "UPDATE automation_runs
                        SET session_id = 's-late', status = 'failed',
                            finished_at = '2026-10-21T09:10:00.000Z'
                      WHERE id = 'r-other';",
                ],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "other-run-dispatching-unbound",
                extra: &[
                    OTHER_RUN,
                    OTHER_ATTEMPT,
                    "UPDATE automation_attempts SET state = 'dispatching', session_id = NULL
                      WHERE id = 'a-other';",
                ],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "other-run-cancelling",
                extra: &[
                    OTHER_RUN,
                    OTHER_ATTEMPT,
                    "INSERT INTO automation_cancellations
                        (adoption_key, request_digest, automation_id, run_id, attempt_id,
                         session_id, scope, requested_by_json, reason, state, requested_at,
                         execution_expires_at)
                     VALUES ('cancel-other', 'digest', 'daily', 'r-other', 'a-other',
                             's-other', 'run', '{\"principalId\":\"operator\"}', 'stop',
                             'requested', '2026-10-22T09:00:00.000Z',
                             '2099-01-01T00:00:00.000Z');",
                ],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "authority-beside-other-run",
                authority: true,
                extra: &[OTHER_RUN, OTHER_ATTEMPT],
                next_step: NextStep::AwaitRuntimeAuthority,
                after: [
                    Some("recovery_required/dispatching"),
                    Some("recovery_required/ambiguous"),
                ],
                ..ABANDONED
            },
            Some("recovery_required/dispatching"),
        ),
        (
            TimeoutShape {
                name: "other-run-fenced",
                extra: &[
                    OTHER_RUN,
                    OTHER_ATTEMPT,
                    "INSERT INTO automation_stop_fences
                        (run_id, session_id, owner, acquired_at, execution_expires_at)
                     VALUES ('r-other', 's-other', 'recovery', '2026-10-22T09:00:00.000Z',
                             '2099-01-01T00:00:00.000Z');",
                ],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        // Every pass decodes each running run's values; restart containment
        // parses a launch's creation time, and settling a plain run with a
        // deadline its session's exit time.
        (
            TimeoutShape {
                extra: &["UPDATE automation_runs SET definition_json = CAST('{}' AS BLOB);"],
                ..in_flight(
                    "blob-definition",
                    [None, Some("recovery_required/ambiguous")],
                )
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &["UPDATE sessions SET exit_code = 'zero';"],
                ..in_flight("text-exit-code", [Some("failed/failed"), None])
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                extra: &["UPDATE sessions SET created_at = 'never';"],
                ..in_flight("unparseable-launch-time", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &[
                    "UPDATE sessions SET created_at = CAST('2026-10-21T09:00:00.000Z' AS BLOB);",
                ],
                ..in_flight("blob-launch-time", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                session: "orphaned",
                extra: &["UPDATE sessions SET created_at = 'never';"],
                ..in_flight("orphaned-unparseable-launch-time", [None, None])
            },
            None,
        ),
        // Restart containment also decodes a launch's occurrence kind and
        // lease owner, and reads its receipt file by the session id.
        (
            TimeoutShape {
                name: "no-lease-owner",
                extra: &["UPDATE automation_occurrences SET lease_owner = NULL;"],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                extra: &["UPDATE automation_occurrences SET kind = CAST(kind AS BLOB);"],
                ..in_flight("blob-kind", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &["UPDATE automation_occurrences SET lease_owner = X'00';"],
                ..in_flight("blob-lease-owner", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &["UPDATE automation_occurrences SET lease_owner = CAST(X'FF' AS TEXT);"],
                ..in_flight("invalid-lease-owner", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &["UPDATE automation_occurrences
                             SET kind = kind || CAST(X'C3' AS TEXT),
                                 lease_owner = CAST(X'A9' AS TEXT) || lease_owner;"],
                ..in_flight("character-split-across-kind-and-owner", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                name: "session-id-at-bound",
                extra: &[late_session_id!(
                    "'s-_' || substr(hex(zeroblob(100)), 1, 197)"
                )],
                ..ABANDONED
            },
            Some("recovery_required/ambiguous"),
        ),
        (
            TimeoutShape {
                extra: &[late_session_id!(
                    "'s-_' || substr(hex(zeroblob(100)), 1, 198)"
                )],
                ..in_flight(
                    "session-id-past-bound",
                    [Some("failed/failed"), Some("recovery_required/ambiguous")],
                )
            },
            Some("recovery_required/ambiguous"),
        ),
        (
            TimeoutShape {
                extra: &[late_session_id!("'s late'")],
                ..in_flight(
                    "spaced-session-id",
                    [Some("failed/failed"), Some("recovery_required/ambiguous")],
                )
            },
            Some("recovery_required/ambiguous"),
        ),
        (
            TimeoutShape {
                extra: &[late_session_id!("'s-' || char(0) || 'late'")],
                ..in_flight("nul-session-id", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &[late_session_id!("'s-late' || char(10) || char(0)")],
                ..in_flight("nul-after-line-feed-session-id", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &[late_session_id!("'s-' || CAST(X'FF' AS TEXT)")],
                ..in_flight("invalid-session-id", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                extra: &[late_session_id!("CAST('s-late' AS BLOB)")],
                ..in_flight("blob-session-id", [None, None])
            },
            None,
        ),
        (
            TimeoutShape {
                deadline: Some("2026-10-22T11:00:00.000Z"),
                extra: &[
                    "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                          VALUES ('e-late', 's-late', 'exit', '{}', 'yesterday');",
                ],
                ..in_flight(
                    "unparseable-exit-with-deadline",
                    [None, Some("recovery_required/ambiguous")],
                )
            },
            None,
        ),
        (
            TimeoutShape {
                name: "exit-with-deadline",
                deadline: Some("2026-10-22T11:00:00.000Z"),
                extra: &[
                    "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                          VALUES ('e-late', 's-late', 'exit', '{}',
                                  '2026-10-22T09:10:00.000Z');",
                ],
                after: [Some("claimed/failed"), Some("recovery_required/ambiguous")],
                ..ABANDONED
            },
            Some("claimed/failed"),
        ),
        (
            TimeoutShape {
                name: "authority-unparseable-exit-with-deadline",
                authority: true,
                deadline: Some("2026-10-22T11:00:00.000Z"),
                extra: &[
                    "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                          VALUES ('e-late', 's-late', 'exit', '{}', 'yesterday');",
                ],
                next_step: NextStep::AwaitRuntimeAuthority,
                after: [
                    Some("recovery_required/dispatching"),
                    Some("recovery_required/ambiguous"),
                ],
                ..ABANDONED
            },
            Some("recovery_required/dispatching"),
        ),
        (
            TimeoutShape {
                name: "unparseable-exit-without-deadline",
                extra: &[
                    "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                          VALUES ('e-late', 's-late', 'exit', '{}', 'yesterday');",
                ],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
        (
            TimeoutShape {
                name: "unparseable-update-with-deadline",
                deadline: Some("2026-10-22T11:00:00.000Z"),
                extra: &["UPDATE sessions SET updated_at = 'never';"],
                after: [Some("claimed/failed"), Some("recovery_required/ambiguous")],
                ..ABANDONED
            },
            Some("claimed/failed"),
        ),
        (
            TimeoutShape {
                name: "recovery",
                occurrence: "recovery_required",
                code: WorkCode::RecoveryRequired,
                next_step: NextStep::CancelRun,
                after: [
                    Some("recovery_required/dispatching"),
                    Some("recovery_required/dispatching"),
                ],
                ..ABANDONED
            },
            Some("failed/failed"),
        ),
    ];
    let mut seen = Vec::new();
    for (shape, _) in &shapes {
        let (_temp, conn) = timeout_store(shape);
        let item = found(&conn, "daily").work.into_iter().next().unwrap();
        // TEMP tables cannot change what the launch reads as.
        let explained = work_of(&conn, "daily");
        conn.execute_batch(
            "CREATE TEMP TABLE automation_occurrences AS
                 SELECT * FROM main.automation_occurrences;
             UPDATE temp.automation_occurrences
                 SET lease_expires_at = '2099-01-01T00:00:00.000Z';
             CREATE TEMP TABLE sessions AS SELECT * FROM main.sessions;
             UPDATE temp.sessions SET status = CASE status
                 WHEN 'created' THEN 'running' ELSE 'created' END, created_at = 'never';
             CREATE TEMP TABLE events AS SELECT * FROM main.events;
             INSERT INTO temp.events (id, session_id, kind, payload_json, created_at)
                 VALUES ('e-shadow', 's-late', 'exit', '{}', 'never');
             CREATE TEMP TABLE automation_attempts AS SELECT * FROM main.automation_attempts;
             UPDATE temp.automation_attempts SET id = 'shadow-' || id, session_id = NULL,
                 state = CASE state WHEN 'dispatching' THEN 'failed' ELSE 'dispatching' END;
             CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs;
             UPDATE temp.automation_runs SET timeout_at = '2026-10-22T09:00:00.000Z',
                 authority_profile = CASE WHEN authority_profile IS NULL THEN 'shadow' END;
             INSERT INTO temp.automation_attempts (id, run_id, state)
                 VALUES ('shadow-extra', 'r-late', 'observing'),
                        ('shadow-unresolved', 'r-late', 'ambiguous');
             INSERT INTO temp.automation_runs (id, occurrence_id, session_id, status)
                 VALUES ('r-shadow', 'o-late', 's-late', 'running');
             CREATE TEMP TABLE automation_stop_fences AS
                 SELECT * FROM main.automation_stop_fences WHERE 0;
             INSERT INTO temp.automation_stop_fences (run_id) VALUES ('r-late');
             CREATE TEMP TABLE automation_cancellations AS
                 SELECT * FROM main.automation_cancellations WHERE 0;
             INSERT INTO temp.automation_cancellations (run_id, state)
                 VALUES ('r-late', 'requested');",
        )
        .unwrap();
        assert_eq!(work_of(&conn, "daily"), explained, "{}", shape.name);
        let after = [
            &ConfirmedKill as &dyn crate::api::SessionRuntime,
            &UnconfirmedKill,
        ]
        .map(|runtime| {
            let (temp, conn) = timeout_store(shape);
            launch_after_pass(&temp, &conn, runtime)
        });
        let contained = {
            let (temp, conn) = timeout_store(shape);
            let receipt =
                crate::automations::runner::containment_receipt_path(temp.path(), "s-late");
            std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
            std::fs::write(&receipt, crate::pty_runner::CONTAINMENT_QUIESCENT_RECEIPT).unwrap();
            launch_after_pass(&temp, &conn, &UnconfirmedKill)
        };
        seen.push((
            shape.name,
            item.code,
            item.next_step,
            item.deadline_at,
            after,
            contained,
        ));
    }
    let expected: Vec<_> = shapes
        .iter()
        .map(|(shape, contained)| {
            (
                shape.name,
                shape.code,
                shape.next_step,
                shape.deadline_at.map(str::to_string),
                shape.after.map(|state| state.map(str::to_string)),
                contained.map(str::to_string),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}

/// A cancellation of the run under test's attempt 1, in `$state`, whose
/// execution lease expired at 09:59:30.
macro_rules! expired_cancellation {
    ($state:literal) => {
        concat!(
            "INSERT INTO automation_cancellations
                (adoption_key, request_digest, automation_id, run_id, attempt_id, session_id,
                 scope, requested_by_json, reason, state, requested_at, execution_expires_at)
             VALUES ('cancel-late', 'digest', 'daily', 'r-late', 'a-late-19', 's-late', 'run',
                     '{\"principalId\":\"operator\"}', 'stop', '",
            $state,
            "', '2026-10-22T09:59:00.000Z', '2026-10-22T09:59:30.000Z');"
        )
    };
}

/// The stop fence a stopping cancellation holds, expired with it.
const CANCELLATION_FENCE: &str = "
    INSERT INTO automation_stop_fences
        (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)
    VALUES ('r-late', 's-late', 'cancellation', 'cancel-late', '2026-10-22T09:59:00.000Z',
            '2026-10-22T09:59:30.000Z');";

/// The occurrence, its run's latest attempt and the run's cancellation after
/// the daemon's reconciliation at 10:00 with `runtime`, or `None` when the pass
/// fails. A stop is claimed at the pass's own instant.
fn cancellation_after_pass(
    temp: &tempfile::TempDir,
    conn: &Connection,
    runtime: &dyn crate::api::SessionRuntime,
) -> Option<String> {
    crate::automations::cancellation::set_stop_fence_clock_test_hook(Some(Box::new(now)));
    let launch = launch_after_pass(temp, conn, runtime);
    crate::automations::cancellation::set_stop_fence_clock_test_hook(None);
    launch.map(|launch| {
        let cancellation: Option<String> = conn
            .query_row(
                "SELECT state FROM automation_cancellations WHERE run_id = 'r-late'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        format!("{launch}/{}", cancellation.as_deref().unwrap_or("none"))
    })
}

const CANCELLING: TimeoutShape = TimeoutShape {
    name: "requested",
    lease: Some("2099-01-01T00:00:00.000Z"),
    deadline: None,
    extra: &[expired_cancellation!("requested")],
    code: WorkCode::CancellationExpired,
    next_step: NextStep::AwaitCancellation,
    deadline_at: None,
    after: [
        Some("cancelled/cancelled/cancelled"),
        Some("recovery_required/ambiguous/recovery_required"),
    ],
    ..TIMED_OUT
};

/// A stopping cancellation, whose stop's outcome was never recorded.
const STOPPING: TimeoutShape = TimeoutShape {
    name: "stopping",
    extra: &[expired_cancellation!("stopping"), CANCELLATION_FENCE],
    after: [
        Some("recovery_required/ambiguous/recovery_required"),
        Some("recovery_required/ambiguous/recovery_required"),
    ],
    ..CANCELLING
};

/// What the daemon's reconciliation does with a run whose cancellation's
/// execution lease has expired, one store per shape and runtime so that a
/// failing pass hides nothing. Each shape gives the occurrence, the run's
/// latest attempt and its cancellation after each pass.
#[test]
fn each_expired_cancellation_names_what_reconciliation_does_with_it() {
    let in_flight = |name, after| TimeoutShape {
        name,
        code: WorkCode::InFlight,
        next_step: NextStep::InspectOccurrence,
        after,
        ..CANCELLING
    };
    let held = Some("recovery_required/ambiguous/recovery_required");
    let left = Some("claimed/started/requested");
    let shapes = [
        CANCELLING,
        TimeoutShape {
            name: "running",
            occurrence: "running",
            ..CANCELLING
        },
        TimeoutShape {
            name: "authority",
            authority: true,
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required/started/recovery_required"), held],
            ..CANCELLING
        },
        TimeoutShape {
            name: "dispatching",
            attempts: &[(1, "dispatching", false)],
            ..CANCELLING
        },
        TimeoutShape {
            name: "bound-dispatching",
            attempts: &[(1, "dispatching", true)],
            ..CANCELLING
        },
        TimeoutShape {
            name: "observing",
            attempts: &[(1, "observing", true)],
            ..CANCELLING
        },
        TimeoutShape {
            name: "deadline-ahead",
            deadline: Some("2026-10-22T10:00:00.001Z"),
            ..CANCELLING
        },
        TimeoutShape {
            name: "expiring-now",
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET execution_expires_at = '2026-10-22T10:00:00.000Z';",
            ],
            ..CANCELLING
        },
        // Reconciliation compares the expiry as stored text.
        TimeoutShape {
            name: "expiry-sorting-behind",
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET execution_expires_at = '2026-10-22T09:59:00-01:00';",
            ],
            ..CANCELLING
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET execution_expires_at = '2026-10-22T10:30:00+01:00';",
            ],
            ..in_flight("expiry-sorting-ahead", [left, left])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET execution_expires_at = '2026-10-22T10:00:00.001Z';",
            ],
            ..in_flight("expiry-ahead", [left, left])
        },
        TimeoutShape {
            name: "requester-spaced",
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET requested_by_json = ' { \"principalId\" :\n \"operator\" } ';",
            ],
            ..CANCELLING
        },
        TimeoutShape {
            name: "no-reason",
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET reason = NULL;",
            ],
            ..CANCELLING
        },
        TimeoutShape {
            name: "requester-as-array",
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET requested_by_json = '[\"operator\"]';",
            ],
            ..CANCELLING
        },
        // Only this request's answer, a stop of this run and another running
        // run on this occurrence keep it from being named.
        TimeoutShape {
            name: "other-request-answered",
            extra: &[
                expired_cancellation!("requested"),
                "INSERT INTO automation_command_adoptions
                    (adoption_key, request_digest, command, outcome, response_json,
                     adopted_at)
                 VALUES ('cancel-other', 'digest', 'run.cancel.v1', 'committed',
                         '{\"result\":{}}', '2026-10-22T09:59:10.000Z');",
            ],
            ..CANCELLING
        },
        TimeoutShape {
            name: "other-run-fenced",
            extra: &[
                expired_cancellation!("requested"),
                OTHER_RUN,
                OTHER_ATTEMPT,
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, acquired_at, execution_expires_at)
                 VALUES ('r-other', 's-other', 'timeout', '2026-10-22T09:00:00.000Z',
                         '2099-01-01T00:00:00.000Z');",
            ],
            ..CANCELLING
        },
        TimeoutShape {
            name: "other-run-cancelled-first",
            extra: &[
                OTHER_RUN,
                OTHER_ATTEMPT,
                "INSERT INTO automation_cancellations
                    (adoption_key, request_digest, automation_id, run_id, attempt_id,
                     session_id, scope, requested_by_json, reason, state, requested_at,
                     execution_expires_at)
                 VALUES ('cancel-other', 'digest', 'daily', 'r-other', 'a-other', 's-other',
                         'run', '{\"principalId\":\"operator\"}', 'stop', 'requested',
                         '2026-10-22T09:59:00.000Z', '2026-10-22T09:59:30.000Z');",
                expired_cancellation!("requested"),
            ],
            ..CANCELLING
        },
        TimeoutShape {
            name: "settled-rival",
            extra: &[
                expired_cancellation!("requested"),
                "INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, runtime, status, started_at,
                     finished_at)
                 VALUES ('r-stray', 'other', 'o-late', 'coven-code', 'failed',
                         '2026-10-21T09:00:00.000Z', '2026-10-21T09:10:00.000Z');",
            ],
            ..CANCELLING
        },
        TimeoutShape {
            name: "settled-session-sharer",
            extra: &[
                expired_cancellation!("requested"),
                "INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, session_id, runtime, status,
                     started_at, finished_at)
                 VALUES ('r-stray', 'other', 'o-late', 's-late', 'coven-code', 'failed',
                         '2026-10-21T09:00:00.000Z', '2026-10-21T09:10:00.000Z');",
            ],
            ..CANCELLING
        },
        TimeoutShape {
            name: "authority-dispatching",
            authority: true,
            attempts: &[(1, "dispatching", false)],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required/dispatching/recovery_required"), held],
            ..CANCELLING
        },
        TimeoutShape {
            name: "authority-observing",
            authority: true,
            attempts: &[(1, "observing", true)],
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [Some("recovery_required/observing/recovery_required"), held],
            ..CANCELLING
        },
        STOPPING,
        TimeoutShape {
            name: "stopping-unfenced",
            extra: &[expired_cancellation!("stopping")],
            ..STOPPING
        },
        TimeoutShape {
            name: "authority-stopping",
            authority: true,
            next_step: NextStep::AwaitRuntimeAuthority,
            ..STOPPING
        },
        TimeoutShape {
            name: "stopping-dispatching",
            attempts: &[(1, "dispatching", false)],
            ..STOPPING
        },
        // A stopping cancellation binds no dispatch, and does not claim a
        // stop fence.
        TimeoutShape {
            name: "stopping-dispatch-beside-bound",
            attempts: &[(1, "dispatching", false), (2, "ambiguous", true)],
            ..STOPPING
        },
        TimeoutShape {
            name: "stopping-timeout-fence",
            extra: &[
                expired_cancellation!("stopping"),
                late_fence!("timeout", "2099-01-01T00:00:00.000Z"),
            ],
            ..STOPPING
        },
        // A stopping cancellation does not compare its run's deadline, which
        // reconciliation still parses whole.
        TimeoutShape {
            name: "stopping-past-deadline",
            deadline: Some("2026-10-22T09:30:00.000Z"),
            ..STOPPING
        },
        TimeoutShape {
            name: "stopping-deadline-sorting-behind",
            deadline: Some("2026-10-22T09:01:00-01:00"),
            ..STOPPING
        },
        TimeoutShape {
            deadline: Some("soon"),
            extra: &[expired_cancellation!("stopping"), CANCELLATION_FENCE],
            ..in_flight("stopping-deadline-unparseable", [None, None])
        },
        // A requested cancellation is rejected once its run's deadline has
        // passed, and the stop it claims compares the deadline as text.
        TimeoutShape {
            deadline: Some("2026-10-22T09:30:00.000Z"),
            ..in_flight(
                "past-deadline",
                [
                    Some("failed/timed_out/rejected"),
                    Some("recovery_required/ambiguous/rejected"),
                ],
            )
        },
        TimeoutShape {
            deadline: Some("2026-10-22T10:00:00.000Z"),
            ..in_flight(
                "deadline-now",
                [
                    Some("failed/timed_out/rejected"),
                    Some("recovery_required/ambiguous/rejected"),
                ],
            )
        },
        TimeoutShape {
            deadline: Some("2026-10-22T09:01:00-01:00"),
            ..in_flight("deadline-sorting-behind", [left, left])
        },
        TimeoutShape {
            deadline: Some("2026-10-22T11:00:00+02:00"),
            ..in_flight(
                "deadline-sorting-ahead",
                [
                    Some("failed/timed_out/rejected"),
                    Some("recovery_required/ambiguous/rejected"),
                ],
            )
        },
        TimeoutShape {
            deadline: Some("soon"),
            ..in_flight("deadline-unparseable", [None, None])
        },
        // Restart containment can act on a session that is not running first,
        // and a finished one is settled from its own evidence.
        TimeoutShape {
            session: "created",
            ..in_flight(
                "created",
                [
                    Some("cancelled/cancelled/cancelled"),
                    Some("recovery_required/ambiguous/recovery_required"),
                ],
            )
        },
        TimeoutShape {
            session: "completed",
            ..in_flight(
                "completed",
                [Some("failed/failed/rejected"), Some("failed/failed/rejected")],
            )
        },
        // The stop needs the targeted attempt to be the only one live on the
        // run's session.
        TimeoutShape {
            attempts: &[(1, "ambiguous", true)],
            ..in_flight(
                "ambiguous",
                [
                    Some("claimed/ambiguous/recovery_required"),
                    Some("claimed/ambiguous/recovery_required"),
                ],
            )
        },
        TimeoutShape {
            attempts: &[(1, "started", true), (2, "dispatching", false)],
            ..in_flight(
                "two-live",
                [None, Some("recovery_required/dispatching/recovery_required")],
            )
        },
        TimeoutShape {
            attempts: &[(1, "observing", true), (2, "dispatching", false)],
            ..in_flight(
                "observing-two-live",
                [None, Some("recovery_required/dispatching/recovery_required")],
            )
        },
        // Settling a plain run binds the dispatch to its session, which
        // another attempt already holds.
        TimeoutShape {
            attempts: &[(1, "dispatching", false), (2, "ambiguous", true)],
            ..in_flight("dispatch-beside-bound", [None, held])
        },
        TimeoutShape {
            attempts: &[(1, "failed", true), (2, "dispatching", false)],
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET attempt_id = 'a-late-18';",
            ],
            ..in_flight("dispatch-after-bound", [None, held])
        },
        TimeoutShape {
            attempts: &[(1, "dispatching", false)],
            extra: &[
                expired_cancellation!("requested"),
                OTHER_RUN,
                OTHER_ATTEMPT,
                "UPDATE automation_attempts SET session_id = 's-late' WHERE id = 'a-other';",
            ],
            ..in_flight("dispatch-beside-taken-session", [None, held])
        },
        // A stopping cancellation is held to the same conditions, though its
        // pass stops nothing.
        TimeoutShape {
            attempts: &[(1, "started", true), (2, "dispatching", false)],
            extra: &[expired_cancellation!("stopping"), CANCELLATION_FENCE],
            ..in_flight(
                "stopping-two-live",
                [
                    Some("recovery_required/dispatching/recovery_required"),
                    Some("recovery_required/dispatching/recovery_required"),
                ],
            )
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("stopping"),
                CANCELLATION_FENCE,
                OTHER_RUN,
                "UPDATE automation_runs SET session_id = 's-late' WHERE id = 'r-other';",
            ],
            ..in_flight("stopping-session-shared", [held, held])
        },
        TimeoutShape {
            attempts: &[(1, "failed", false), (2, "started", true)],
            ..in_flight(
                "targets-settled-attempt",
                [Some("cancelled/cancelled/cancelled"), None],
            )
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                OTHER_RUN,
                OTHER_ATTEMPT,
                "UPDATE automation_attempts SET state = 'dispatching', session_id = NULL
                    WHERE id = 'a-other';
                 UPDATE automation_cancellations SET attempt_id = 'a-other';",
            ],
            ..in_flight(
                "targets-other-runs-attempt",
                [Some("cancelled/cancelled/cancelled"), None],
            )
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET session_id = 's-gone';",
            ],
            ..in_flight("other-session", [left, left])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                OTHER_RUN,
                "UPDATE automation_runs SET session_id = 's-late' WHERE id = 'r-other';",
            ],
            ..in_flight(
                "session-shared",
                [Some("cancelled/cancelled/cancelled"), held],
            )
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "INSERT INTO sessions
                    (id, project_root, harness, title, status, created_at, updated_at)
                 VALUES ('s-stray', '/PRIVATE', 'codex', 'PRIVATE', 'running',
                         '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z');
                 INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, session_id, runtime, status,
                     started_at)
                 VALUES ('r-stray', 'other', 'o-late', 's-stray', 'coven-code', 'running',
                         '2026-10-21T09:00:00.000Z');",
            ],
            ..in_flight("rival", [Some("cancelled/cancelled/cancelled"), held])
        },
        TimeoutShape {
            authority: true,
            attempts: &[(1, "ambiguous", false), (2, "started", true)],
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET attempt_id = 'a-late-18';",
            ],
            ..in_flight("authority-unresolved", [None, held])
        },
        // A requested cancellation is left to another stop, and a stored
        // answer is replayed.
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                late_fence!("timeout", "2099-01-01T00:00:00.000Z"),
            ],
            ..in_flight("timeout-fence", [left, left])
        },
        TimeoutShape {
            extra: &[expired_cancellation!("requested"), CANCELLATION_FENCE],
            ..in_flight("own-fence", [held, held])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "INSERT INTO automation_command_adoptions
                    (adoption_key, request_digest, command, outcome, response_json,
                     adopted_at)
                 VALUES ('cancel-late', 'digest', 'run.cancel.v1', 'committed',
                         '{\"result\":{}}', '2026-10-22T09:59:10.000Z');",
            ],
            ..in_flight("answered", [left, left])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("stopping"),
                "INSERT INTO automation_command_adoptions
                    (adoption_key, request_digest, command, outcome, response_json,
                     adopted_at)
                 VALUES ('cancel-late', 'other', 'run.cancel.v1', 'committed',
                         '{\"result\":{}}', '2026-10-22T09:59:10.000Z');",
            ],
            ..in_flight(
                "stopping-answered-differently",
                [
                    Some("claimed/started/stopping"),
                    Some("claimed/started/stopping"),
                ],
            )
        },
        // Reconciliation reads the cancellation's values as text and its
        // requester as a requester.
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET requested_by_json = '{}';",
            ],
            ..in_flight("requester-without-principal", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET requested_by_json = '{\"principalId\":\"operator\",\"role\":\"x\"}';",
            ],
            ..in_flight("requester-with-unknown-field", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET requested_by_json = CAST('{\"principalId\":\"operator\"}' AS BLOB);",
            ],
            ..in_flight("blob-requester", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET reason = CAST('stop' AS BLOB);",
            ],
            ..in_flight("blob-reason", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET reason = CAST(X'FF' AS TEXT);",
            ],
            ..in_flight("invalid-reason", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations
                    SET execution_expires_at = '2026-10-22T09:59:30' || CAST(X'FF' AS TEXT);",
            ],
            ..in_flight("invalid-expiry", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET adoption_key = CAST(adoption_key AS BLOB);",
            ],
            ..in_flight("blob-adoption-key", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET adoption_key = adoption_key || CAST(X'FF' AS TEXT);",
            ],
            ..in_flight("invalid-adoption-key", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET request_digest = CAST(request_digest AS BLOB);",
            ],
            ..in_flight("blob-request-digest", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET request_digest = request_digest || CAST(X'FF' AS TEXT);",
            ],
            ..in_flight("invalid-request-digest", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET automation_id = CAST(automation_id AS BLOB);",
            ],
            ..in_flight("blob-automation-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET automation_id = automation_id || CAST(X'FF' AS TEXT);",
            ],
            ..in_flight("invalid-automation-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET requested_at = CAST(requested_at AS BLOB);",
            ],
            ..in_flight("blob-requested-at", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_cancellations SET requested_at = requested_at || CAST(X'FF' AS TEXT);",
            ],
            ..in_flight("invalid-requested-at", [None, None])
        },
        // The values it compares with the run's own are read as text too.
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                late_session_id!("CAST('s-late' AS BLOB)"),
                "UPDATE automation_cancellations SET session_id = CAST(session_id AS BLOB);",
            ],
            ..in_flight("blob-session-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "PRAGMA foreign_keys = OFF;
                 UPDATE automation_attempts SET id = CAST(id AS BLOB);
                 UPDATE automation_cancellations SET attempt_id = CAST(attempt_id AS BLOB);
                 PRAGMA foreign_keys = ON;",
            ],
            ..in_flight("blob-attempt-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "PRAGMA foreign_keys = OFF;
                 UPDATE automation_runs SET id = CAST(id AS BLOB);
                 UPDATE automation_attempts SET run_id = CAST(run_id AS BLOB);
                 UPDATE automation_cancellations SET run_id = CAST(run_id AS BLOB);
                 PRAGMA foreign_keys = ON;",
            ],
            ..in_flight("blob-run-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                late_session_id!("'s-late' || CAST(X'FF' AS TEXT)"),
                "UPDATE automation_cancellations SET session_id = session_id || CAST(X'FF' AS TEXT);",
            ],
            ..in_flight("invalid-session-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "PRAGMA foreign_keys = OFF;
                 UPDATE automation_attempts SET id = id || CAST(X'FF' AS TEXT);
                 UPDATE automation_cancellations SET attempt_id = attempt_id || CAST(X'FF' AS TEXT);
                 PRAGMA foreign_keys = ON;",
            ],
            ..in_flight("invalid-attempt-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "PRAGMA foreign_keys = OFF;
                 UPDATE automation_runs SET id = id || CAST(X'FF' AS TEXT);
                 UPDATE automation_attempts SET run_id = run_id || CAST(X'FF' AS TEXT);
                 UPDATE automation_cancellations SET run_id = run_id || CAST(X'FF' AS TEXT);
                 PRAGMA foreign_keys = ON;",
            ],
            ..in_flight("invalid-run-id", [None, None])
        },
        TimeoutShape {
            extra: &[
                expired_cancellation!("requested"),
                "UPDATE automation_runs SET definition_json = CAST('{}' AS BLOB);",
            ],
            ..in_flight("blob-definition", [None, held])
        },
        TimeoutShape {
            occurrence: "recovery_required",
            code: WorkCode::RecoveryRequired,
            next_step: NextStep::AwaitCancellation,
            after: [
                Some("recovery_required/started/requested"),
                Some("recovery_required/started/requested"),
            ],
            ..in_flight("recovery", [None, None])
        },
        TimeoutShape {
            occurrence: "recovery_required",
            authority: true,
            code: WorkCode::RecoveryRequired,
            next_step: NextStep::AwaitRuntimeAuthority,
            after: [
                Some("recovery_required/started/requested"),
                Some("recovery_required/started/requested"),
            ],
            ..in_flight("authority-recovery", [None, None])
        },
    ];
    let mut seen = Vec::new();
    for shape in &shapes {
        let (_temp, conn) = timeout_store(shape);
        let item = found(&conn, "daily").work.into_iter().next().unwrap();
        // TEMP tables cannot change what the run reads as.
        let explained = work_of(&conn, "daily");
        conn.execute_batch(
            "CREATE TEMP TABLE automation_cancellations AS
                 SELECT * FROM main.automation_cancellations;
             UPDATE temp.automation_cancellations SET
                 state = CASE state WHEN 'requested' THEN 'stopping' ELSE 'requested' END,
                 execution_expires_at = CASE
                     WHEN execution_expires_at <= '2026-10-22T10:00:00.000Z'
                     THEN '2099-01-01T00:00:00.000Z' ELSE '2026-10-22T09:00:00.000Z' END,
                 requested_by_json = '{}', attempt_id = 'shadow', session_id = 'shadow';
             INSERT INTO temp.automation_cancellations (run_id, state, execution_expires_at)
                 VALUES ('r-late', 'requested', '2026-10-22T09:00:00.000Z');
             CREATE TEMP TABLE automation_command_adoptions AS
                 SELECT * FROM main.automation_command_adoptions WHERE 0;
             INSERT INTO temp.automation_command_adoptions (adoption_key)
                 VALUES ('cancel-late');
             CREATE TEMP TABLE automation_stop_fences AS
                 SELECT * FROM main.automation_stop_fences WHERE 0;
             INSERT INTO temp.automation_stop_fences (run_id) VALUES ('r-late');
             CREATE TEMP TABLE sessions AS SELECT * FROM main.sessions;
             UPDATE temp.sessions SET status = CASE status
                 WHEN 'running' THEN 'created' ELSE 'running' END;
             CREATE TEMP TABLE automation_attempts AS SELECT * FROM main.automation_attempts;
             UPDATE temp.automation_attempts SET id = 'shadow-' || id, session_id = NULL,
                 state = CASE state WHEN 'started' THEN 'failed' ELSE 'started' END;
             INSERT INTO temp.automation_attempts (id, run_id, state)
                 VALUES ('shadow-extra', 'r-late', 'observing');
             CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs;
             UPDATE temp.automation_runs SET timeout_at = 'never', authority_profile =
                 CASE WHEN authority_profile IS NULL THEN 'shadow' END;
             INSERT INTO temp.automation_runs (id, occurrence_id, session_id, status)
                 VALUES ('r-shadow', 'o-late', 's-late', 'running');",
        )
        .unwrap();
        assert_eq!(work_of(&conn, "daily"), explained, "{}", shape.name);
        let after = [
            &ConfirmedKill as &dyn crate::api::SessionRuntime,
            &UnconfirmedKill,
        ]
        .map(|runtime| {
            let (temp, conn) = timeout_store(shape);
            cancellation_after_pass(&temp, &conn, runtime)
        });
        seen.push((shape.name, item.code, item.next_step, after));
    }
    let expected: Vec<_> = shapes
        .iter()
        .map(|shape| {
            (
                shape.name,
                shape.code,
                shape.next_step,
                shape.after.map(|state| state.map(str::to_string)),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}

/// A running run keyed to the routine whose occurrence is not one of the
/// routine's claimed, running or recovery-required ones: another routine's
/// occurrence, one of its own that is planned or settled, or no row at all.
struct StrayShape {
    name: &'static str,
    /// The run's occurrence: its routine and its state.
    occurrence: (&'static str, &'static str),
    lease: Option<&'static str>,
    deadline: Option<&'static str>,
    /// The run's session status; `None` for a run with no session.
    session: Option<&'static str>,
    exit_code: Option<i64>,
    /// (number, state, carries the run's session)
    attempts: &'static [(i64, &'static str, bool)],
    authority: bool,
    /// SQL run against the store before explaining, to add or corrupt rows.
    extra: &'static [&'static str],
    code: WorkCode,
    next_step: NextStep,
    /// The run's status, its occurrence's state and, when it has one, its
    /// cancellation's state after the daemon's reconciliation at 10:00 with a
    /// runtime that confirms the kill, and with one that never does; `None`
    /// when that pass fails.
    after: [Option<&'static str>; 2],
}

/// A run whose session has finished, on another routine's claimed occurrence.
const STRAY: StrayShape = StrayShape {
    name: "finished-on-other-claimed",
    occurrence: ("other", "claimed"),
    lease: Some("2099-01-01T00:00:00.000Z"),
    deadline: None,
    session: Some("failed"),
    exit_code: Some(1),
    attempts: &[(1, "started", true)],
    authority: false,
    extra: &[],
    code: WorkCode::RunningRun,
    next_step: NextStep::InspectRun,
    after: [Some("failed/failed"), Some("failed/failed")],
};

/// Deletes the run's occurrence, leaving its id dangling.
const GONE_OCCURRENCE: &str = "
    PRAGMA foreign_keys = OFF;
    DELETE FROM automation_occurrences WHERE id = 'o-late';
    PRAGMA foreign_keys = ON;";

fn stray_store(shape: &StrayShape) -> (tempfile::TempDir, Connection) {
    let (temp, conn) = store();
    for id in ["daily", "other"] {
        insert_definition_at(&conn, &definition(id, "PAUSED"), "2026-10-01T00:00:00.000Z").unwrap();
    }
    let (routine, state) = shape.occurrence;
    conn.execute(
        "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
             lease_expires_at, created_at, updated_at)
         VALUES ('o-late', ?1, '2026-10-21T09:00:00.000Z', 'manual', ?2, 1,
                 'operator', ?3, '2026-10-21T09:00:00.000Z', '2026-10-21T09:00:00.000Z')",
        rusqlite::params![routine, state, shape.lease],
    )
    .unwrap();
    if let Some(status) = shape.session {
        session(&conn, "s-late", status);
        conn.execute(
            "UPDATE sessions SET exit_code = ?1 WHERE id = 's-late'",
            [shape.exit_code],
        )
        .unwrap();
    }
    run_keyed(
        &conn,
        "r-late",
        "daily",
        Some("o-late"),
        "running",
        shape.authority,
    );
    conn.execute(
        "UPDATE automation_runs
         SET timeout_at = ?1, session_id = CASE WHEN ?2 THEN 's-late' END
         WHERE id = 'r-late'",
        rusqlite::params![shape.deadline, shape.session.is_some()],
    )
    .unwrap();
    // Ids that do not sort like attempt numbers. A settled attempt carries
    // its settlement time from the insert: terminal attempts are immutable.
    for (number, state, carries) in shape.attempts {
        let live = matches!(*state, "dispatching" | "started" | "observing");
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, state, prior_attempt_number,
                 prior_disposition, retry_classification, not_before, session_id,
                 state_reason, opened_at, settled_at)
             VALUES (?1, 'r-late', 'o-late', ?2, 'PRIVATE-' || ?1, 1, ?3,
                     CASE WHEN ?2 > 1 THEN ?2 - 1 END, CASE WHEN ?2 > 1 THEN 'failed' END,
                     CASE WHEN ?2 > 1 THEN 'automatic_retry' ELSE 'initial' END,
                     '2026-10-21T09:00:00.000Z', CASE WHEN ?4 THEN 's-late' END,
                     'PRIVATE-REASON', '2026-10-21T09:00:00.000Z',
                     CASE WHEN NOT ?5 THEN '2026-10-22T09:20:00.000Z' END)",
            rusqlite::params![
                format!("a-late-{}", 20 - number),
                number,
                state,
                carries,
                live
            ],
        )
        .unwrap();
    }
    for sql in shape.extra {
        conn.execute_batch(sql).unwrap();
    }
    (temp, conn)
}

/// The run's status, its occurrence's state and, when it has one, its
/// cancellation's state after the daemon's reconciliation at 10:00 with
/// `runtime`, or `None` when the pass fails. A stop is claimed at the pass's
/// own instant.
fn stray_after_pass(
    temp: &tempfile::TempDir,
    conn: &Connection,
    runtime: &dyn crate::api::SessionRuntime,
) -> Option<String> {
    crate::automations::cancellation::set_stop_fence_clock_test_hook(Some(Box::new(now)));
    let passed = crate::automations::daemon_tick::reconcile_automation_runs(
        temp.path(),
        conn,
        runtime,
        now(),
        None,
    );
    crate::automations::cancellation::set_stop_fence_clock_test_hook(None);
    passed.ok().map(|()| {
        let run: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'r-late'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let occurrence: Option<String> = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'o-late'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let cancellation: Option<String> = conn
            .query_row(
                "SELECT state FROM automation_cancellations WHERE run_id = 'r-late'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let mut after = format!("{run}/{}", occurrence.as_deref().unwrap_or("none"));
        if let Some(cancellation) = cancellation {
            after.push('/');
            after.push_str(&cancellation);
        }
        after
    })
}

/// What the daemon's reconciliation does with a running run whose occurrence
/// is not one the routine's pass settles, one store per shape and runtime so
/// that a failing pass hides nothing. Such a run is listed as `running_run`,
/// with the retry on a planned occurrence named instead; each shape gives
/// the run's status and its occurrence's state after each pass.
#[test]
fn each_running_run_names_what_reconciliation_does_with_it() {
    let fails = [None, None];
    let shapes = [
        // Settlement settles a run on another routine's live occurrence with
        // that occurrence, and a run on a settled occurrence alone when its
        // session gives the occurrence's own state; it fails otherwise.
        STRAY,
        StrayShape {
            name: "finished-on-other-running",
            occurrence: ("other", "running"),
            ..STRAY
        },
        StrayShape {
            name: "finished-on-other-recovery",
            occurrence: ("other", "recovery_required"),
            ..STRAY
        },
        StrayShape {
            name: "finished-on-other-planned",
            occurrence: ("other", "planned"),
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "finished-on-other-failed",
            occurrence: ("other", "failed"),
            ..STRAY
        },
        StrayShape {
            name: "finished-on-own-planned",
            occurrence: ("daily", "planned"),
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "finished-on-own-failed",
            occurrence: ("daily", "failed"),
            ..STRAY
        },
        StrayShape {
            name: "finished-on-own-succeeded",
            occurrence: ("daily", "succeeded"),
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "finished-on-own-cancelled",
            occurrence: ("daily", "cancelled"),
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "succeeded-on-own-succeeded",
            occurrence: ("daily", "succeeded"),
            session: Some("completed"),
            exit_code: Some(0),
            after: [Some("succeeded/succeeded"), Some("succeeded/succeeded")],
            ..STRAY
        },
        StrayShape {
            name: "succeeded-on-own-failed",
            occurrence: ("daily", "failed"),
            session: Some("completed"),
            exit_code: Some(0),
            after: fails,
            ..STRAY
        },
        // A session that ended at or after the run's deadline is timed out,
        // and settles `failed` whatever its exit code.
        StrayShape {
            name: "succeeded-late-on-own-succeeded",
            occurrence: ("daily", "succeeded"),
            deadline: Some("2026-10-21T08:00:00.000Z"),
            session: Some("completed"),
            exit_code: Some(0),
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "succeeded-late-on-own-failed",
            occurrence: ("daily", "failed"),
            deadline: Some("2026-10-21T08:00:00.000Z"),
            session: Some("completed"),
            exit_code: Some(0),
            ..STRAY
        },
        StrayShape {
            name: "cancelled-session-on-own-cancelled",
            occurrence: ("daily", "cancelled"),
            session: Some("cancelled"),
            exit_code: None,
            after: [Some("cancelled/cancelled"), Some("cancelled/cancelled")],
            ..STRAY
        },
        StrayShape {
            name: "finished-on-missing",
            occurrence: ("daily", "failed"),
            extra: &[GONE_OCCURRENCE],
            after: fails,
            ..STRAY
        },
        // A plain failure before the run's deadline is held for an operator
        // retry without touching the occurrence; at the deadline, settling
        // the held run fails the pass.
        StrayShape {
            name: "finished-held-on-own-failed",
            occurrence: ("daily", "failed"),
            deadline: Some("2099-01-01T00:00:00.000Z"),
            after: [Some("running/failed"), Some("running/failed")],
            ..STRAY
        },
        StrayShape {
            name: "finished-held-on-other-planned",
            occurrence: ("other", "planned"),
            deadline: Some("2099-01-01T00:00:00.000Z"),
            after: [Some("running/planned"), Some("running/planned")],
            ..STRAY
        },
        StrayShape {
            name: "held-on-other-claimed",
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "failed", false)],
            ..STRAY
        },
        StrayShape {
            name: "held-on-own-failed",
            occurrence: ("daily", "failed"),
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "failed", false)],
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "held-on-own-planned",
            occurrence: ("daily", "planned"),
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "failed", false)],
            after: fails,
            ..STRAY
        },
        // An ambiguous attempt settles from its own settlement time.
        StrayShape {
            name: "ambiguous-finished-on-own-failed",
            occurrence: ("daily", "failed"),
            attempts: &[(1, "ambiguous", true)],
            ..STRAY
        },
        // A Runtime Authority run is held for recovery, which needs a
        // claimed, running or recovery-required occurrence.
        StrayShape {
            name: "authority-finished-on-other-claimed",
            authority: true,
            after: [
                Some("running/recovery_required"),
                Some("running/recovery_required"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "authority-finished-on-own-failed",
            occurrence: ("daily", "failed"),
            authority: true,
            after: fails,
            ..STRAY
        },
        // Settlement skips a fenced run whose attempt is live; its own
        // expired timeout stop holds the run for recovery, which needs a
        // claimed, running or recovery-required occurrence.
        StrayShape {
            name: "fenced-finished-on-own-failed",
            occurrence: ("daily", "failed"),
            extra: &[late_fence!("timeout", "2099-01-01T00:00:00.000Z")],
            after: [Some("running/failed"), Some("running/failed")],
            ..STRAY
        },
        StrayShape {
            name: "expired-fence-finished-on-own-failed",
            occurrence: ("daily", "failed"),
            extra: &[late_fence!("timeout", "2026-10-22T09:30:00.000Z")],
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "expired-fence-finished-on-other-claimed",
            extra: &[late_fence!("timeout", "2026-10-22T09:30:00.000Z")],
            after: [
                Some("running/recovery_required"),
                Some("running/recovery_required"),
            ],
            ..STRAY
        },
        // Timeout enforcement stops a run past its deadline on another
        // routine's claimed or running occurrence, and no other.
        StrayShape {
            name: "timeout-on-other-claimed",
            session: Some("running"),
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            after: [Some("failed/failed"), Some("running/recovery_required")],
            ..STRAY
        },
        StrayShape {
            name: "timeout-on-other-running",
            occurrence: ("other", "running"),
            session: Some("running"),
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            after: [Some("failed/failed"), Some("running/recovery_required")],
            ..STRAY
        },
        StrayShape {
            name: "timeout-on-other-recovery",
            occurrence: ("other", "recovery_required"),
            session: Some("running"),
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            after: [
                Some("running/recovery_required"),
                Some("running/recovery_required"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "timeout-on-own-planned",
            occurrence: ("daily", "planned"),
            session: Some("running"),
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            after: [Some("running/planned"), Some("running/planned")],
            ..STRAY
        },
        StrayShape {
            name: "timeout-on-own-failed",
            occurrence: ("daily", "failed"),
            session: Some("running"),
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            after: [Some("running/failed"), Some("running/failed")],
            ..STRAY
        },
        StrayShape {
            name: "timeout-on-missing",
            session: Some("running"),
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            extra: &[GONE_OCCURRENCE],
            after: [Some("running/none"), Some("running/none")],
            ..STRAY
        },
        // Abandoned-launch recovery stops a launch past its lease on another
        // routine's claimed occurrence, and no other.
        StrayShape {
            name: "abandoned-on-other-claimed",
            lease: Some("2026-10-22T09:00:00.000Z"),
            session: Some("created"),
            exit_code: None,
            attempts: &[(1, "dispatching", true)],
            after: [Some("failed/failed"), Some("running/recovery_required")],
            ..STRAY
        },
        StrayShape {
            name: "abandoned-on-own-failed",
            occurrence: ("daily", "failed"),
            lease: Some("2026-10-22T09:00:00.000Z"),
            session: Some("created"),
            exit_code: None,
            attempts: &[(1, "dispatching", true)],
            after: [Some("running/failed"), Some("running/failed")],
            ..STRAY
        },
        StrayShape {
            name: "abandoned-on-own-planned",
            occurrence: ("daily", "planned"),
            lease: Some("2026-10-22T09:00:00.000Z"),
            session: Some("created"),
            exit_code: None,
            attempts: &[(1, "dispatching", true)],
            after: [Some("running/planned"), Some("running/planned")],
            ..STRAY
        },
        // Cancellation reconciliation takes an expired cancellation of any
        // run. The stop a requested one claims needs a claimed or running
        // occurrence, so it is refused and the cancellation left requested;
        // holding the run for a stopping one needs a claimed, running or
        // recovery-required occurrence, and fails the pass otherwise.
        StrayShape {
            name: "cancelled-on-other-claimed",
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("requested")],
            after: [
                Some("cancelled/cancelled/cancelled"),
                Some("running/recovery_required/recovery_required"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "cancelled-on-other-recovery",
            occurrence: ("other", "recovery_required"),
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("requested")],
            after: [
                Some("running/recovery_required/requested"),
                Some("running/recovery_required/requested"),
            ],
            ..STRAY
        },
        // A requested cancellation of a run past its deadline is rejected
        // first, and timeout enforcement then stops the run where it can.
        StrayShape {
            name: "cancelled-past-deadline-on-other-claimed",
            deadline: Some("2026-10-22T09:30:00.000Z"),
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("requested")],
            after: [
                Some("failed/failed/rejected"),
                Some("running/recovery_required/rejected"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "cancelled-past-deadline-on-own-failed",
            occurrence: ("daily", "failed"),
            deadline: Some("2026-10-22T09:30:00.000Z"),
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("requested")],
            after: [
                Some("running/failed/rejected"),
                Some("running/failed/rejected"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "cancelled-on-own-planned",
            occurrence: ("daily", "planned"),
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("requested")],
            after: [
                Some("running/planned/requested"),
                Some("running/planned/requested"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "cancelled-on-own-failed",
            occurrence: ("daily", "failed"),
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("requested")],
            after: [
                Some("running/failed/requested"),
                Some("running/failed/requested"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "cancelled-on-missing",
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("requested"), GONE_OCCURRENCE],
            after: [
                Some("running/none/requested"),
                Some("running/none/requested"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "stopping-on-other-claimed",
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("stopping"), CANCELLATION_FENCE],
            after: [
                Some("running/recovery_required/recovery_required"),
                Some("running/recovery_required/recovery_required"),
            ],
            ..STRAY
        },
        StrayShape {
            name: "stopping-on-own-failed",
            occurrence: ("daily", "failed"),
            session: Some("running"),
            exit_code: None,
            extra: &[expired_cancellation!("stopping"), CANCELLATION_FENCE],
            after: fails,
            ..STRAY
        },
        // An expired retry is named on a planned occurrence of any routine.
        // Timeout enforcement settles it on a planned or claimed one, and
        // fails the pass on any other.
        StrayShape {
            name: "retry-on-own-planned",
            occurrence: ("daily", "planned"),
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "adopted", false)],
            code: WorkCode::RetryExpired,
            next_step: NextStep::AwaitSettlement,
            ..STRAY
        },
        StrayShape {
            name: "retry-on-other-planned",
            occurrence: ("other", "planned"),
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "adopted", false)],
            code: WorkCode::RetryExpired,
            next_step: NextStep::AwaitSettlement,
            ..STRAY
        },
        StrayShape {
            name: "retry-on-other-claimed",
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "adopted", false)],
            ..STRAY
        },
        StrayShape {
            name: "retry-on-own-failed",
            occurrence: ("daily", "failed"),
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "adopted", false)],
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "retry-on-other-running",
            occurrence: ("other", "running"),
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "adopted", false)],
            after: fails,
            ..STRAY
        },
        StrayShape {
            name: "retry-on-missing",
            session: None,
            exit_code: None,
            deadline: Some("2026-10-22T09:30:00.000Z"),
            attempts: &[(1, "adopted", false)],
            extra: &[GONE_OCCURRENCE],
            after: fails,
            ..STRAY
        },
        // A run no pass acts on stays as it is.
        StrayShape {
            name: "live-on-own-failed",
            occurrence: ("daily", "failed"),
            session: Some("running"),
            exit_code: None,
            after: [Some("running/failed"), Some("running/failed")],
            ..STRAY
        },
        StrayShape {
            name: "live-on-other-claimed",
            session: Some("running"),
            exit_code: None,
            after: [Some("running/claimed"), Some("running/claimed")],
            ..STRAY
        },
    ];
    let mut seen = Vec::new();
    for shape in &shapes {
        let (temp, conn) = stray_store(shape);
        let item = found(&conn, "daily").work.into_iter().next().unwrap();
        assert!(item.holds_claims, "{}", shape.name);
        // TEMP tables cannot change what the run reads as.
        let explained = work_of(&conn, "daily");
        conn.execute_batch(
            "CREATE TEMP TABLE automation_occurrences AS
                 SELECT * FROM main.automation_occurrences;
             UPDATE temp.automation_occurrences SET automation_id = 'daily',
                 state = CASE state WHEN 'planned' THEN 'claimed' ELSE 'planned' END;
             INSERT INTO temp.automation_occurrences (id, automation_id, state)
                 VALUES ('o-late', 'daily', 'claimed');
             CREATE TEMP TABLE automation_attempts AS SELECT * FROM main.automation_attempts;
             UPDATE temp.automation_attempts SET id = 'shadow-' || id,
                 state = CASE state WHEN 'adopted' THEN 'started' ELSE 'adopted' END;
             CREATE TEMP TABLE automation_runs AS SELECT * FROM main.automation_runs;
             UPDATE temp.automation_runs SET timeout_at = '2026-10-22T09:00:00.000Z',
                 session_id = CASE WHEN session_id IS NULL THEN 's-late' END;
             INSERT INTO temp.automation_runs (id, automation_id, occurrence_id, status)
                 VALUES ('r-shadow', 'daily', 'o-shadow', 'running');",
        )
        .unwrap();
        assert_eq!(work_of(&conn, "daily"), explained, "{}", shape.name);
        // The explained store, its shadows gone, takes the first pass.
        conn.execute_batch(
            "DROP TABLE temp.automation_occurrences;
             DROP TABLE temp.automation_attempts;
             DROP TABLE temp.automation_runs;",
        )
        .unwrap();
        let (never_temp, never_conn) = stray_store(shape);
        let after = [
            stray_after_pass(&temp, &conn, &ConfirmedKill),
            stray_after_pass(&never_temp, &never_conn, &UnconfirmedKill),
        ];
        seen.push((shape.name, item.code, item.next_step, after));
    }
    let expected: Vec<_> = shapes
        .iter()
        .map(|shape| {
            (
                shape.name,
                shape.code,
                shape.next_step,
                shape.after.map(|state| state.map(str::to_string)),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}
