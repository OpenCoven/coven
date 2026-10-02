//! Occurrence, run and attempt transition events (coven#1054).
//!
//! Each state change of an occurrence (`state`), run (`status`) or attempt
//! (`state`), including creation, appends one `*.transitioned` event to the
//! entity's stream. SQLite triggers write it, so the event commits or rolls
//! back with the state change at every write site, and no site can forget it.
//! A state written but not committed never publishes, and a committed state
//! always has its event.
//!
//! The triggers keep the bookkeeping `contract::events::append_event` keeps:
//! the stream head supplies the gapless sequence, and the event id is derived
//! from the row's feed position so the column and the JSON agree. Optional id
//! fields are omitted unless they match the contract's identifier shape, so a
//! legacy row cannot make its stream unreadable.
//!
//! Occurrence events go on the occurrence stream; run and attempt events go on
//! the run stream, where receipts already land. History before these triggers
//! existed is not backfilled: a stream's first event may start mid-lifecycle.

use anyhow::{Context, Result};
use rusqlite::Connection;

struct Entity {
    trigger: &'static str,
    table: &'static str,
    state_column: &'static str,
    entity: &'static str,
    kind: &'static str,
    stream_kind: &'static str,
    stream_id: &'static str,
    automation_id: &'static str,
    occurrence_id: Option<&'static str>,
    run_id: Option<&'static str>,
    attempt_id: Option<&'static str>,
    attempt_number: Option<&'static str>,
    reason: &'static str,
}

const ENTITIES: [Entity; 3] = [
    Entity {
        trigger: "automation_occurrence_transition",
        table: "automation_occurrences",
        state_column: "state",
        entity: "occurrence",
        kind: "occurrence.transitioned",
        stream_kind: "occurrence",
        stream_id: "NEW.id",
        automation_id: "NEW.automation_id",
        occurrence_id: Some("NEW.id"),
        run_id: None,
        attempt_id: None,
        attempt_number: None,
        reason: "NEW.failure_reason",
    },
    Entity {
        trigger: "automation_run_transition",
        table: "automation_runs",
        state_column: "status",
        entity: "run",
        kind: "run.transitioned",
        stream_kind: "run",
        stream_id: "NEW.id",
        automation_id: "NEW.automation_id",
        occurrence_id: Some("NEW.occurrence_id"),
        run_id: Some("NEW.id"),
        attempt_id: None,
        attempt_number: None,
        reason: "NULL",
    },
    Entity {
        trigger: "automation_attempt_transition",
        table: "automation_attempts",
        state_column: "state",
        entity: "attempt",
        kind: "attempt.transitioned",
        stream_kind: "run",
        stream_id: "NEW.run_id",
        automation_id: "(SELECT automation_id FROM automation_runs WHERE id = NEW.run_id)",
        occurrence_id: Some("NEW.occurrence_id"),
        run_id: Some("NEW.run_id"),
        attempt_id: Some("NEW.id"),
        attempt_number: Some("NEW.attempt_number"),
        reason: "COALESCE(NULLIF(trim(NEW.state_reason), ''), NEW.failure_class)",
    },
];

/// SQL true when `value` is a contract identifier: alphanumeric first, then
/// `[A-Za-z0-9._-]`, at most `max` bytes.
fn identifier(value: &str, max: usize) -> String {
    format!(
        "({value} IS NOT NULL AND length(CAST({value} AS BLOB)) BETWEEN 1 AND {max} \
         AND substr({value}, 1, 1) GLOB '[A-Za-z0-9]' \
         AND {value} NOT GLOB '*[^A-Za-z0-9._-]*')"
    )
}

/// The trigger body appending one transition from `from` to the new state.
fn body(entity: &Entity, from: &str) -> String {
    let stream_kind = format!("'{}'", entity.stream_kind);
    let stream_id = entity.stream_id;
    let to = format!("substr(NEW.{}, 1, 32)", entity.state_column);
    let reason = format!(
        "substr(COALESCE(NULLIF(trim({}), ''), 'Coven recorded the {} state'), 1, 500)",
        entity.reason, entity.entity
    );
    let optional = [
        ("automationId", Some(entity.automation_id), 96),
        ("occurrenceId", entity.occurrence_id, 160),
        ("runId", entity.run_id, 160),
        ("attemptId", entity.attempt_id, 160),
    ];
    let ids = optional
        .iter()
        .filter_map(|(key, value, _)| value.map(|value| format!("'{key}', {value}")))
        .collect::<Vec<_>>()
        .join(", ");
    // json_remove ignores a path that does not exist, so each id field is
    // removed when its column is null or not a contract identifier.
    let removals = optional
        .iter()
        .filter_map(|(key, value, max)| {
            value.map(|value| {
                format!(
                    "CASE WHEN {} THEN '$.none' ELSE '$.{key}' END",
                    identifier(value, *max)
                )
            })
        })
        .collect::<Vec<_>>()
        .join(", ");
    let attempt_number = entity
        .attempt_number
        .map(|value| format!(", 'attemptNumber', {value}"))
        .unwrap_or_default();
    format!(
        "INSERT INTO automation_event_stream_heads (
             stream_kind, stream_id, next_sequence, earliest_sequence, updated_at
         ) VALUES ({stream_kind}, {stream_id}, 0, 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(stream_kind, stream_id) DO NOTHING;
         INSERT INTO automation_events (
             feed_position, event_id, stream_kind, stream_id, sequence, recorded_at,
             recorded_at_millis, observed_at, event_json
         )
         SELECT p.position, 'evt' || printf('%032x', p.position), {stream_kind}, {stream_id},
                h.next_sequence, p.at,
                CAST(strftime('%s', p.at) AS INTEGER) * 1000 + CAST(substr(p.at, 21, 3) AS INTEGER),
                p.at,
                json_remove(json_object(
                    'schemaVersion', 'coven.automations.v1',
                    'eventId', 'evt' || printf('%032x', p.position),
                    'stream', json_object('kind', {stream_kind}, 'id', {stream_id}),
                    'sequence', h.next_sequence,
                    'recordedAt', p.at,
                    'observedAt', p.at,
                    'producer', json_object('component', 'coven-daemon', 'instanceId', 'local-authority'),
                    {ids},
                    'kind', '{kind}',
                    'summary', '{entity_name} ' || {to},
                    'payload', json_object(
                        'entity', '{entity_name}', 'from', {from}, 'to', {to}, 'reason', {reason}
                        {attempt_number}
                    ),
                    'privacy', json_object(
                        'classification', 'operational',
                        'retention', json_object('classification', 'standard')
                    )
                ), {removals})
         FROM (
             -- MAX sees rows earlier triggers in this statement inserted;
             -- sqlite_sequence is only written back when the statement ends,
             -- so a multi-row write would reuse one position. Events are
             -- append-only, so MAX + 1 never revisits a position.
             SELECT COALESCE((SELECT MAX(feed_position) FROM automation_events), 0) + 1 AS position,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now') AS at
         ) AS p
         JOIN automation_event_stream_heads AS h
           ON h.stream_kind = {stream_kind} AND h.stream_id = {stream_id};
         UPDATE automation_event_stream_heads
            SET next_sequence = next_sequence + 1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE stream_kind = {stream_kind} AND stream_id = {stream_id};",
        kind = entity.kind,
        entity_name = entity.entity,
    )
}

/// The trigger DDL for every entity: one on insert, one on a state change.
pub(crate) fn transition_trigger_sql() -> String {
    ENTITIES
        .iter()
        .map(|entity| {
            format!(
                "CREATE TRIGGER IF NOT EXISTS {trigger}_created
                 AFTER INSERT ON {table}
                 BEGIN
                 {created}
                 END;
                 CREATE TRIGGER IF NOT EXISTS {trigger}_changed
                 AFTER UPDATE OF {column} ON {table}
                 WHEN OLD.{column} IS NOT NEW.{column}
                 BEGIN
                 {changed}
                 END;",
                trigger = entity.trigger,
                table = entity.table,
                column = entity.state_column,
                created = body(entity, "'none'"),
                changed = body(
                    entity,
                    &format!("substr(OLD.{}, 1, 32)", entity.state_column)
                ),
            )
        })
        .collect()
}

/// Installs the transition triggers. Requires the occurrence, run, attempt and
/// event tables; idempotent.
pub(crate) fn ensure_transition_triggers(conn: &Connection) -> Result<()> {
    conn.execute_batch(&transition_trigger_sql())
        .context("failed to install automation transition event triggers")
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use serde_json::{json, Value};

    const SPEC_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../spec/coven-automations/v1"
    );

    fn store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1, "id": "alpha", "name": "alpha", "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
            "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        (temp, conn)
    }

    /// Validates against the published event envelope, resolving every
    /// sibling v1 schema offline.
    fn envelope_validator() -> jsonschema::Validator {
        let mut registry = jsonschema::Registry::new();
        let mut event_schema = Value::Null;
        for entry in std::fs::read_dir(SPEC_DIR).unwrap() {
            let path = entry.unwrap().path();
            if !path.to_string_lossy().ends_with(".schema.json") {
                continue;
            }
            let schema: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            if path.ends_with("event-envelope.schema.json") {
                event_schema = schema.clone();
            }
            let id = String::from(schema["$id"].as_str().unwrap());
            registry = registry.add(id, schema).unwrap();
        }
        let registry = registry.prepare().unwrap();
        jsonschema::draft202012::options()
            .with_registry(&registry)
            .offline()
            .build(&event_schema)
            .unwrap()
    }

    /// Every event of a stream, read through the typed contract reader and
    /// checked against the published schema.
    fn stream(conn: &Connection, kind: &str, id: &str) -> Vec<Value> {
        let page = crate::automations::contract::events::read_events(
            conn,
            kind,
            id,
            None,
            None,
            100,
            "2026-10-01T00:00:00.000Z",
        )
        .unwrap();
        let validator = envelope_validator();
        page.events
            .iter()
            .map(|event| {
                let value = serde_json::to_value(event).unwrap();
                assert!(validator.is_valid(&value), "{value}");
                value
            })
            .collect()
    }

    fn transitions(events: &[Value]) -> Vec<(String, String, String, u64)> {
        events
            .iter()
            .map(|event| {
                (
                    event["kind"].as_str().unwrap().to_owned(),
                    event["payload"]["from"].as_str().unwrap().to_owned(),
                    event["payload"]["to"].as_str().unwrap().to_owned(),
                    event["sequence"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    fn insert_occurrence(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT ?1, id, revision, definition_digest, '2026-10-01T09:00:00.000Z',
                    'scheduled', 'planned', 0, '2026-10-01T09:00:00.000Z', '2026-10-01T09:00:00.000Z'
             FROM automation_definitions WHERE id = 'alpha'",
            [id],
        )
        .unwrap();
    }

    fn set_state(conn: &Connection, id: &str, state: &str) {
        conn.execute(
            "UPDATE automation_occurrences SET state = ?2 WHERE id = ?1",
            [id, state],
        )
        .unwrap();
    }

    #[test]
    fn occurrence_state_changes_publish_one_gapless_event_each() {
        let (_temp, conn) = store();
        insert_occurrence(&conn, "alpha-1");
        set_state(&conn, "alpha-1", "claimed");
        // A write that leaves the state alone publishes nothing.
        conn.execute(
            "UPDATE automation_occurrences SET lease_owner = 'scheduler-1' WHERE id = 'alpha-1'",
            [],
        )
        .unwrap();
        set_state(&conn, "alpha-1", "claimed");
        set_state(&conn, "alpha-1", "running");
        conn.execute(
            "UPDATE automation_occurrences SET state = 'failed', failure_reason = 'runtime exited 1'
             WHERE id = 'alpha-1'",
            [],
        )
        .unwrap();

        let events = stream(&conn, "occurrence", "alpha-1");
        assert_eq!(
            transitions(&events),
            vec![
                (
                    "occurrence.transitioned".into(),
                    "none".into(),
                    "planned".into(),
                    0
                ),
                (
                    "occurrence.transitioned".into(),
                    "planned".into(),
                    "claimed".into(),
                    1
                ),
                (
                    "occurrence.transitioned".into(),
                    "claimed".into(),
                    "running".into(),
                    2
                ),
                (
                    "occurrence.transitioned".into(),
                    "running".into(),
                    "failed".into(),
                    3
                ),
            ]
        );
        assert_eq!(events[3]["payload"]["reason"], "runtime exited 1");
        assert_eq!(events[0]["occurrenceId"], "alpha-1");
        assert_eq!(events[0]["automationId"], "alpha");
        let ids = events
            .iter()
            .map(|event| event["eventId"].as_str().unwrap().to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 4);
        assert_eq!(
            crate::automations::contract::events::stream_head(&conn, "occurrence", "alpha-1")
                .unwrap(),
            Some(3)
        );
    }

    #[test]
    fn runs_and_attempts_publish_on_the_run_stream_with_rust_appends_interleaved() {
        let (_temp, conn) = store();
        insert_occurrence(&conn, "alpha-1");
        conn.execute(
            "INSERT INTO automation_runs
                (id, automation_id, occurrence_id, runtime, status, started_at)
             VALUES ('run-1', 'alpha', 'alpha-1', 'coven-code', 'running', '2026-10-01T09:00:00.000Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state,
                 retry_classification, not_before, opened_at)
             VALUES ('attempt-run-1-1', 'run-1', 'alpha-1', 1, 'adopt:attempt-run-1-1', 1, 0,
                     'adopted', 'initial', '2026-10-01T09:00:00.000Z', '2026-10-01T09:00:00.000Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_attempts SET state = 'started' WHERE id = 'attempt-run-1-1'",
            [],
        )
        .unwrap();
        // A Rust append reads the head the triggers advanced.
        let head = crate::automations::contract::events::stream_head(&conn, "run", "run-1")
            .unwrap()
            .unwrap();
        assert_eq!(head, 2);
        conn.execute(
            "UPDATE automation_runs SET status = 'succeeded', finished_at = '2026-10-01T09:05:00.000Z'
             WHERE id = 'run-1'",
            [],
        )
        .unwrap();

        let events = stream(&conn, "run", "run-1");
        assert_eq!(
            transitions(&events),
            vec![
                (
                    "run.transitioned".into(),
                    "none".into(),
                    "running".into(),
                    0
                ),
                (
                    "attempt.transitioned".into(),
                    "none".into(),
                    "adopted".into(),
                    1
                ),
                (
                    "attempt.transitioned".into(),
                    "adopted".into(),
                    "started".into(),
                    2
                ),
                (
                    "run.transitioned".into(),
                    "running".into(),
                    "succeeded".into(),
                    3
                ),
            ]
        );
        assert_eq!(events[1]["attemptId"], "attempt-run-1-1");
        assert_eq!(events[1]["automationId"], "alpha");
        assert_eq!(events[1]["payload"]["attemptNumber"], 1);
        assert_eq!(events[0]["occurrenceId"], "alpha-1");
    }

    #[test]
    fn multi_row_statements_publish_one_event_per_row() {
        let (_temp, conn) = store();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES
                ('alpha-1', 'alpha', '2026-10-01T09:00:00.000Z', 'planned', 0,
                 '2026-10-01T09:00:00.000Z', '2026-10-01T09:00:00.000Z'),
                ('alpha-2', 'alpha', '2026-10-02T09:00:00.000Z', 'planned', 0,
                 '2026-10-02T09:00:00.000Z', '2026-10-02T09:00:00.000Z'),
                ('alpha-3', 'alpha', '2026-10-03T09:00:00.000Z', 'planned', 0,
                 '2026-10-03T09:00:00.000Z', '2026-10-03T09:00:00.000Z')",
            [],
        )
        .unwrap();
        // One statement, many rows: a batch supersession.
        let changed = conn
            .execute(
                "UPDATE automation_occurrences SET state = 'superseded' WHERE automation_id = 'alpha'",
                [],
            )
            .unwrap();
        assert_eq!(changed, 3);
        for id in ["alpha-1", "alpha-2", "alpha-3"] {
            assert_eq!(
                transitions(&stream(&conn, "occurrence", id)),
                vec![
                    (
                        "occurrence.transitioned".into(),
                        "none".into(),
                        "planned".into(),
                        0
                    ),
                    (
                        "occurrence.transitioned".into(),
                        "planned".into(),
                        "superseded".into(),
                        1
                    ),
                ],
                "{id}"
            );
        }
        // A later Rust append still takes a fresh feed position.
        let positions: Vec<i64> = conn
            .prepare("SELECT feed_position FROM automation_events ORDER BY feed_position")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let distinct = positions.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(distinct.len(), positions.len());
    }

    #[test]
    fn a_rolled_back_state_change_publishes_nothing() {
        let (_temp, conn) = store();
        insert_occurrence(&conn, "alpha-1");
        let transaction = conn.unchecked_transaction().unwrap();
        set_state(&transaction, "alpha-1", "claimed");
        transaction.rollback().unwrap();
        assert_eq!(
            transitions(&stream(&conn, "occurrence", "alpha-1")).len(),
            1
        );
        assert_eq!(
            crate::automations::contract::events::stream_head(&conn, "occurrence", "alpha-1")
                .unwrap(),
            Some(0)
        );
    }

    #[test]
    fn nullable_and_nonconforming_ids_are_omitted_not_published_invalid() {
        let (_temp, conn) = store();
        conn.execute(
            "INSERT INTO automation_runs (id, automation_id, runtime, status, started_at)
             VALUES ('run-legacy', 'legacy:odd id', 'coven-code', 'running', '2026-10-01T09:00:00.000Z')",
            [],
        )
        .unwrap();
        let events = stream(&conn, "run", "run-legacy");
        assert_eq!(events.len(), 1);
        assert!(events[0].get("occurrenceId").is_none(), "{}", events[0]);
        assert!(events[0].get("automationId").is_none(), "{}", events[0]);
        assert_eq!(events[0]["runId"], "run-legacy");
        assert_eq!(
            events[0]["payload"]["reason"],
            "Coven recorded the run state"
        );
    }
}
