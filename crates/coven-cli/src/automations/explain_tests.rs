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
            }]
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
