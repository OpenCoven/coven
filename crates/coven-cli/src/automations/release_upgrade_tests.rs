//! Upgrade and rollback evidence from stores that released daemons wrote
//! (coven#1054). Each fixture is a `sqlite3 .dump` of a store produced through
//! a release binary's own control actions; `tests/fixtures/
//! automations-release-stores/README.md` records how.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{types::Value as SqlValue, Connection};
use serde_json::Value;

use super::contract::migration::definition_digest;
use super::contract::types::EventEnvelope;

const V0_4_3: &str = include_str!("../../tests/fixtures/automations-release-stores/v0.4.3.sql");
const V0_4_6: &str = include_str!("../../tests/fixtures/automations-release-stores/v0.4.6.sql");
const V0_4_6_ROLLBACK: &str =
    include_str!("../../tests/fixtures/automations-release-stores/v0.4.6-rollback.sql");

fn restore(dump: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("coven.sqlite3");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(dump).unwrap();
    // As in the store the release wrote: one AUTOINCREMENT counter per table.
    assert_eq!(
        rows(
            &conn,
            "SELECT name FROM sqlite_sequence GROUP BY name HAVING COUNT(*) > 1"
        ),
        Vec::<String>::new()
    );
    (dir, path)
}

fn upgrade(path: &Path) -> Connection {
    crate::store::initialize_store(path).unwrap();
    Connection::open(path).unwrap()
}

/// Every row a query returns, rendered for comparison.
fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut statement = conn.prepare(sql).unwrap();
    let width = statement.column_count();
    statement
        .query_map([], |row| {
            (0..width)
                .map(|index| row.get::<_, SqlValue>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map(|values| format!("{values:?}"))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// The automation schema and every automation row, in a stable order.
fn snapshot(path: &Path) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    let mut out = rows(
        &conn,
        "SELECT type, name, sql FROM sqlite_schema
         WHERE name LIKE 'automation%' OR name LIKE 'idx_automation%'
         ORDER BY type, name",
    );
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name LIKE 'automation%'
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for table in tables {
        let mut table_rows = rows(&conn, &format!("SELECT * FROM \"{table}\""));
        table_rows.sort();
        out.push(table);
        out.extend(table_rows);
    }
    out
}

struct Definition {
    status: String,
    lifecycle_state: String,
    revision: i64,
    digest: Option<String>,
    json: String,
}

fn definitions(conn: &Connection) -> BTreeMap<String, Definition> {
    conn.prepare(
        "SELECT id, status, lifecycle_state, revision, definition_digest, definition_json
         FROM automation_definitions",
    )
    .unwrap()
    .query_map([], |row| {
        Ok((
            row.get(0)?,
            Definition {
                status: row.get(1)?,
                lifecycle_state: row.get(2)?,
                revision: row.get(3)?,
                digest: row.get(4)?,
                json: row.get(5)?,
            },
        ))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// `(automation_id -> (revision, digest))` for a table with definition pins.
fn pins(conn: &Connection, table: &str) -> BTreeMap<String, (i64, Option<String>)> {
    conn.prepare(&format!(
        "SELECT automation_id, automation_revision, definition_digest FROM {table}"
    ))
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

struct Event {
    feed_position: i64,
    stream_kind: String,
    stream_id: String,
    sequence: i64,
    body: Value,
}

fn events(conn: &Connection) -> Vec<Event> {
    conn.prepare(
        "SELECT feed_position, stream_kind, stream_id, sequence, event_json
         FROM automation_events
         ORDER BY feed_position",
    )
    .unwrap()
    .query_map([], |row| {
        Ok(Event {
            feed_position: row.get(0)?,
            stream_kind: row.get(1)?,
            stream_id: row.get(2)?,
            sequence: row.get(3)?,
            body: serde_json::from_str(&row.get::<_, String>(4)?).unwrap(),
        })
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// The feed is dense with its AUTOINCREMENT counter at the last position,
/// every stream counts up from zero to its head, and every event decodes as
/// a v1 envelope.
fn assert_feed_is_whole(conn: &Connection) {
    let events = events(conn);
    let positions: Vec<i64> = events.iter().map(|event| event.feed_position).collect();
    assert_eq!(positions, (1..=events.len() as i64).collect::<Vec<_>>());
    assert_eq!(
        rows(
            conn,
            "SELECT seq FROM sqlite_sequence WHERE name = 'automation_events'"
        ),
        [format!("{:?}", [SqlValue::Integer(events.len() as i64)])]
    );
    let mut streams: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
    for event in &events {
        serde_json::from_value::<EventEnvelope>(event.body.clone()).unwrap_or_else(|error| {
            panic!("event {} does not decode: {error}", event.feed_position)
        });
        streams
            .entry((event.stream_kind.clone(), event.stream_id.clone()))
            .or_default()
            .push(event.sequence);
    }
    let heads: BTreeMap<(String, String), i64> = conn
        .prepare("SELECT stream_kind, stream_id, next_sequence FROM automation_event_stream_heads")
        .unwrap()
        .query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        heads.keys().collect::<Vec<_>>(),
        streams.keys().collect::<Vec<_>>()
    );
    for (stream, sequences) in streams {
        assert_eq!(
            sequences,
            (0..sequences.len() as i64).collect::<Vec<_>>(),
            "{stream:?}"
        );
        assert_eq!(heads[&stream], sequences.len() as i64, "{stream:?}");
    }
}

/// The history a release wrote, which no later start may rewrite.
fn history(conn: &Connection) -> Vec<String> {
    [
        "SELECT * FROM automation_events ORDER BY feed_position",
        "SELECT * FROM automation_event_stream_heads ORDER BY stream_kind, stream_id",
        "SELECT id, state, automation_revision, definition_digest
         FROM automation_occurrences ORDER BY id",
        "SELECT id, status, automation_revision, definition_digest
         FROM automation_runs ORDER BY id",
    ]
    .into_iter()
    .flat_map(|sql| rows(conn, sql))
    .collect()
}

#[test]
fn a_v0_4_3_store_upgrades_without_activating_or_reattributing_history() {
    let (_dir, path) = restore(V0_4_3);
    let before: BTreeMap<String, (String, String)> = Connection::open(&path)
        .unwrap()
        .prepare("SELECT id, status, definition_json FROM automation_definitions")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let conn = upgrade(&path);
    let after = definitions(&conn);

    // No implicit activation: every status survives, the Codex import stays
    // paused, and lifecycle state follows the status it had.
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>()
    );
    for (id, (status, _)) in &before {
        assert_eq!(&after[id].status, status, "{id}");
    }
    assert_eq!(after["nightly-review"].lifecycle_state, "active");
    assert_eq!(after["weekly-digest"].lifecycle_state, "paused");
    assert_eq!(after["legacy-standup"].status, "PAUSED");
    assert_eq!(after["legacy-standup"].lifecycle_state, "paused");

    // Bodies are kept byte for byte at revision 1 and digested as stored. The
    // import alone was saved with a `local` timezone, which the durable
    // timezone migration resolves as revision 2, keeping the original body.
    for id in ["nightly-review", "weekly-digest"] {
        assert_eq!(after[id].json, before[id].1, "{id}");
        assert_eq!(after[id].revision, 1, "{id}");
    }
    for definition in after.values() {
        assert_eq!(
            definition.digest.as_deref(),
            Some(definition_digest(&definition.json).unwrap().as_str())
        );
    }
    let import = &after["legacy-standup"];
    assert_eq!(import.revision, 2);
    let mut resolved: Value = serde_json::from_str(&import.json).unwrap();
    let mut original: Value = serde_json::from_str(&before["legacy-standup"].1).unwrap();
    assert_eq!(original["timezone"], "local");
    assert_ne!(resolved["timezone"], "local");
    resolved.as_object_mut().unwrap().remove("timezone");
    original.as_object_mut().unwrap().remove("timezone");
    assert_eq!(resolved, original);
    let retained: String = conn
        .query_row(
            "SELECT previous_definition_json FROM automation_timezone_migrations
             WHERE automation_id = 'legacy-standup'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, before["legacy-standup"].1);

    // weekly-digest ran after its last edit, so its history is pinned to that
    // body. nightly-review was edited after its run, so its history stays
    // unverifiable rather than being attributed to the later body.
    for table in ["automation_occurrences", "automation_runs"] {
        let pins = pins(&conn, table);
        assert_eq!(
            pins["weekly-digest"],
            (1, after["weekly-digest"].digest.clone()),
            "{table}"
        );
        assert_eq!(pins["nightly-review"].1, None, "{table}");
    }
    assert_eq!(
        rows(
            &conn,
            "SELECT definitions_migrated, unverifiable_occurrences, unverifiable_runs
             FROM automation_contract_migrations"
        ),
        rows(&conn, "SELECT 3, 1, 1")
    );

    // One imported baseline per definition, the timezone revision on top, and
    // no transition history invented for the old occurrences and runs.
    let events = events(&conn);
    let mut kinds: Vec<(&str, i64, &str)> = events
        .iter()
        .map(|event| {
            assert_eq!(event.stream_kind, "automation");
            (
                event.stream_id.as_str(),
                event.sequence,
                event.body["kind"].as_str().unwrap(),
            )
        })
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        [
            ("legacy-standup", 0, "definition.imported"),
            ("legacy-standup", 1, "definition.revised"),
            ("nightly-review", 0, "definition.imported"),
            ("weekly-digest", 0, "definition.imported"),
        ]
    );
    for event in events.iter().filter(|event| event.sequence == 0) {
        assert_eq!(event.body["payload"]["importedFrom"], "legacy-coven-store");
        assert_eq!(
            event.body["payload"]["lifecycleState"],
            after[&event.stream_id].lifecycle_state
        );
    }
    assert_feed_is_whole(&conn);
    assert_eq!(
        rows(
            &conn,
            "SELECT (SELECT COUNT(*) FROM automation_occurrences),
                    (SELECT COUNT(*) FROM automation_runs)"
        ),
        rows(&conn, "SELECT 2, 2")
    );

    // A second start changes nothing.
    let first = snapshot(&path);
    crate::store::initialize_store(&path).unwrap();
    assert_eq!(snapshot(&path), first);
}

#[test]
fn a_v0_4_6_store_upgrades_without_rewriting_its_history() {
    let (_dir, path) = restore(V0_4_6);
    let (before_definitions, before_history) = {
        let conn = Connection::open(&path).unwrap();
        (definitions(&conn), history(&conn))
    };
    let conn = upgrade(&path);

    // Definitions, events, stream heads and every occurrence and run pin are
    // exactly what v0.4.6 wrote: the upgrade adds schema, not history.
    let after = definitions(&conn);
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before_definitions.keys().collect::<Vec<_>>()
    );
    for (id, before) in &before_definitions {
        let after = &after[id];
        assert_eq!(
            (
                &after.status,
                &after.lifecycle_state,
                after.revision,
                &after.digest,
                &after.json
            ),
            (
                &before.status,
                &before.lifecycle_state,
                before.revision,
                &before.digest,
                &before.json
            ),
            "{id}"
        );
    }
    assert_eq!(history(&conn), before_history);
    assert_feed_is_whole(&conn);

    // The historical pins v0.4.6 wrote still name the revision each run used.
    let occurrences = pins(&conn, "automation_occurrences");
    assert_eq!(occurrences["nightly-review"].0, 1);
    assert_eq!(after["nightly-review"].revision, 2);
    assert_eq!(occurrences["weekly-digest"].0, 2);
    assert_eq!(
        occurrences["weekly-digest"].1,
        after["weekly-digest"].digest
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'trigger' AND name LIKE 'automation_%_transition_%'"
        ),
        rows(&conn, "SELECT 6")
    );

    let first = snapshot(&path);
    crate::store::initialize_store(&path).unwrap();
    assert_eq!(snapshot(&path), first);
}

#[test]
fn an_upgraded_store_survives_a_v0_4_6_rollback_and_rolls_forward() {
    // The v0.4.6 fixture, upgraded by this producer, given a rich draft through
    // the command envelope, then run by the v0.4.6 binary again: it revised the
    // rich draft, ran a routine with the transition triggers in place, and
    // created a routine.
    let (_dir, path) = restore(V0_4_6_ROLLBACK);
    let before_history = history(&Connection::open(&path).unwrap());
    let conn = upgrade(&path);

    // v0.4.6 kept the feed whole while the triggers fired under it, and
    // rolling forward rewrites none of it.
    assert_feed_is_whole(&conn);
    assert_eq!(history(&conn), before_history);
    let transitions = rows(
        &conn,
        "SELECT json_extract(event_json, '$.kind'), COUNT(*)
         FROM automation_events
         WHERE stream_kind IN ('occurrence', 'run')
         GROUP BY 1 ORDER BY 1",
    );
    assert_eq!(
        transitions,
        rows(
            &conn,
            "SELECT 'attempt.transitioned', 3
             UNION ALL SELECT 'occurrence.transitioned', 2
             UNION ALL SELECT 'run.transitioned', 2"
        )
    );
    let after = definitions(&conn);
    assert_eq!(after["rollback-created"].lifecycle_state, "paused");
    assert_eq!(after["nightly-review"].lifecycle_state, "active");

    // v0.4.6 revised the rich draft without knowing its rich body, which still
    // describes revision 1. It is dropped rather than served as revision 2;
    // the recorded revision 1 stays.
    let rich = &after["rich-briefing"];
    assert_eq!(rich.revision, 2);
    assert!(rich.json.contains("the open questions"));
    assert_eq!(
        rows(
            &conn,
            "SELECT rich_definition_json FROM automation_definitions
             WHERE id = 'rich-briefing'"
        ),
        rows(&conn, "SELECT NULL")
    );
    assert_eq!(
        super::rich_definition::current_view(&conn, "rich-briefing").unwrap(),
        None
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT automation_id, revision FROM automation_rich_definition_revisions"
        ),
        rows(&conn, "SELECT 'rich-briefing', 1")
    );

    let first = snapshot(&path);
    crate::store::initialize_store(&path).unwrap();
    assert_eq!(snapshot(&path), first);
}
