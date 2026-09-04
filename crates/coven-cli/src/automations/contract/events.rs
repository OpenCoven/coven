//! Durable Automations v1 event streams.

use std::fmt;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

pub use super::types::EventRef;
use super::types::{EventEnvelope, EventRefStream, SafeInteger, StreamKind};

#[cfg(test)]
use super::types::{EventKind, EventPayload};
#[cfg(test)]
use serde_json::{Map, Value};
#[cfg(test)]
use std::collections::HashSet;

pub const AUTOMATION_EVENTS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_event_stream_heads (
        stream_kind TEXT NOT NULL
            CHECK (stream_kind IN ('automation', 'occurrence', 'run', 'feed')),
        stream_id TEXT NOT NULL,
        next_sequence INTEGER NOT NULL CHECK (next_sequence >= 0),
        earliest_sequence INTEGER NOT NULL DEFAULT 0 CHECK (earliest_sequence >= 0),
        updated_at TEXT NOT NULL,
        PRIMARY KEY(stream_kind, stream_id)
    );

    CREATE TABLE IF NOT EXISTS automation_events (
        feed_position INTEGER PRIMARY KEY AUTOINCREMENT,
        event_id TEXT UNIQUE NOT NULL,
        stream_kind TEXT NOT NULL
            CHECK (stream_kind IN ('automation', 'occurrence', 'run', 'feed')),
        stream_id TEXT NOT NULL,
        sequence INTEGER NOT NULL CHECK (sequence >= 0),
        recorded_at TEXT NOT NULL,
        recorded_at_millis INTEGER NOT NULL,
        observed_at TEXT NOT NULL,
        event_json TEXT NOT NULL,
        UNIQUE(stream_kind, stream_id, sequence)
    );

    CREATE INDEX IF NOT EXISTS idx_automation_events_recorded
        ON automation_events(recorded_at_millis, feed_position);

    CREATE TRIGGER IF NOT EXISTS automation_events_no_update
    BEFORE UPDATE ON automation_events
    BEGIN
        SELECT RAISE(ABORT, 'automation events are append-only');
    END;

    CREATE TABLE IF NOT EXISTS automation_event_checkpoints (
        checkpoint TEXT PRIMARY KEY NOT NULL,
        stream_kind TEXT NOT NULL
            CHECK (stream_kind IN ('automation', 'occurrence', 'run', 'feed')),
        stream_id TEXT NOT NULL,
        after_sequence INTEGER NOT NULL CHECK (after_sequence >= -1),
        issued_at TEXT NOT NULL,
        expires_at TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_automation_event_checkpoints_expiry
        ON automation_event_checkpoints(expires_at);

    CREATE TABLE IF NOT EXISTS automation_event_migrations (
        name TEXT PRIMARY KEY NOT NULL,
        completed_at TEXT NOT NULL
    );

    CREATE TRIGGER IF NOT EXISTS automation_events_no_delete
    BEFORE DELETE ON automation_events
    BEGIN
        SELECT RAISE(ABORT, 'automation events are append-only');
    END;
";

pub struct DefinitionEventInput<'a> {
    pub command: &'a str,
    pub automation_id: &'a str,
    pub revision: u64,
    pub definition_digest: Option<&'a str>,
    pub lifecycle_state: &'a str,
    pub adoption_key: &'a str,
    pub observed_at: &'a str,
}

struct DefinitionLifecycleEventInput<'a> {
    event_id: &'a str,
    event_kind: &'a str,
    automation_id: &'a str,
    revision: u64,
    definition_digest: Option<&'a str>,
    lifecycle_state: &'a str,
    adoption_key: Option<&'a str>,
    imported_from: Option<&'a str>,
    recorded_at: &'a str,
    observed_at: &'a str,
}

pub struct ImportedDefinitionEventInput<'a> {
    pub automation_id: &'a str,
    pub revision: u64,
    pub definition_digest: Option<&'a str>,
    pub lifecycle_state: &'a str,
    pub imported_from: &'a str,
    pub recorded_at: &'a str,
    pub observed_at: &'a str,
}

pub struct MigratedDefinitionEventInput<'a> {
    pub automation_id: &'a str,
    pub revision: u64,
    pub definition_digest: Option<&'a str>,
    pub lifecycle_state: &'a str,
    pub migration: &'a str,
    pub recorded_at: &'a str,
    pub observed_at: &'a str,
}

#[derive(Debug)]
pub enum EventStoreError {
    Sqlite(rusqlite::Error),
    Contract(anyhow::Error),
    DuplicateEventId { event_id: String },
    StreamOutOfOrder { expected: u64, actual: u64 },
    CursorExpired { expired_at: String },
    CheckpointNotFound,
    InvalidRead(String),
}

impl EventStoreError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DuplicateEventId { .. } => "DUPLICATE_EVENT_ID",
            Self::StreamOutOfOrder { .. } => "STREAM_OUT_OF_ORDER",
            Self::CursorExpired { .. } => "CURSOR_EXPIRED",
            Self::CheckpointNotFound => "CHECKPOINT_NOT_FOUND",
            Self::InvalidRead(_) => "VALIDATION_FAILED",
            Self::Sqlite(_) | Self::Contract(_) => "INTERNAL",
        }
    }

    #[must_use]
    pub fn expired_at(&self) -> Option<&str> {
        match self {
            Self::CursorExpired { expired_at } => Some(expired_at),
            _ => None,
        }
    }
}

impl fmt::Display for EventStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "automation event store failed: {error}"),
            Self::Contract(error) => {
                write!(formatter, "automation event violates contract: {error}")
            }
            Self::DuplicateEventId { event_id } => {
                write!(formatter, "automation event id `{event_id}` already exists")
            }
            Self::StreamOutOfOrder { expected, actual } => write!(
                formatter,
                "automation event stream expected sequence {expected}, received {actual}"
            ),
            Self::CursorExpired { expired_at } => {
                write!(
                    formatter,
                    "automation event checkpoint expired at {expired_at}"
                )
            }
            Self::CheckpointNotFound => {
                formatter.write_str("automation event checkpoint not found")
            }
            Self::InvalidRead(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for EventStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Contract(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for EventStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub stream: EventPageStream,
    pub after: Option<u64>,
    pub events: Vec<EventEnvelope>,
    pub next_after: Option<u64>,
    pub checkpoint: String,
    pub checkpoint_expires_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPageStream {
    pub kind: String,
    pub id: String,
}

fn stream_kind(kind: StreamKind) -> &'static str {
    match kind {
        StreamKind::Automation => "automation",
        StreamKind::Occurrence => "occurrence",
        StreamKind::Run => "run",
        StreamKind::Feed => "feed",
    }
}

fn timestamp_millis(value: &str) -> Result<i64, EventStoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.timestamp_millis())
        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))
}

pub fn stream_head(
    conn: &Connection,
    stream_kind: &str,
    stream_id: &str,
) -> Result<Option<u64>, EventStoreError> {
    let next_sequence = conn
        .query_row(
            "SELECT next_sequence
             FROM automation_event_stream_heads
             WHERE stream_kind = ?1 AND stream_id = ?2",
            params![stream_kind, stream_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    next_sequence
        .map(|next| {
            let head = next
                .checked_sub(1)
                .ok_or_else(|| EventStoreError::InvalidRead("invalid stream head".to_owned()))?;
            u64::try_from(head)
                .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))
        })
        .transpose()
}

pub fn append_event(
    conn: &Connection,
    event: &EventEnvelope,
    expected_sequence: u64,
) -> Result<EventRef, EventStoreError> {
    let actual_sequence = event.sequence.get();
    if actual_sequence != expected_sequence {
        return Err(EventStoreError::StreamOutOfOrder {
            expected: expected_sequence,
            actual: actual_sequence,
        });
    }
    let kind = stream_kind(event.stream.kind);
    let stream_id = event.stream.id.as_str();
    let event_id = event.event_id.as_str();
    if conn
        .query_row(
            "SELECT 1 FROM automation_events WHERE event_id = ?1",
            [event_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Err(EventStoreError::DuplicateEventId {
            event_id: event_id.to_owned(),
        });
    }

    conn.execute_batch("SAVEPOINT coven_automation_event_append")?;
    let result = (|| {
        conn.execute(
            "INSERT INTO automation_event_stream_heads (
                stream_kind, stream_id, next_sequence, earliest_sequence, updated_at
             ) VALUES (?1, ?2, 0, 0, ?3)
             ON CONFLICT(stream_kind, stream_id) DO NOTHING",
            params![kind, stream_id, event.recorded_at.as_str()],
        )?;
        let changed = conn.execute(
            "UPDATE automation_event_stream_heads
             SET next_sequence = next_sequence + 1, updated_at = ?4
             WHERE stream_kind = ?1 AND stream_id = ?2 AND next_sequence = ?3",
            params![
                kind,
                stream_id,
                i64::try_from(expected_sequence)
                    .map_err(|error| { EventStoreError::Contract(anyhow::Error::new(error)) })?,
                event.recorded_at.as_str()
            ],
        )?;
        if changed != 1 {
            let actual = conn.query_row(
                "SELECT next_sequence
                 FROM automation_event_stream_heads
                 WHERE stream_kind = ?1 AND stream_id = ?2",
                params![kind, stream_id],
                |row| row.get::<_, i64>(0),
            )?;
            return Err(EventStoreError::StreamOutOfOrder {
                expected: u64::try_from(actual)
                    .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?,
                actual: actual_sequence,
            });
        }
        let event_json = serde_json::to_string(event)
            .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?;
        let recorded_at_millis = timestamp_millis(event.recorded_at.as_str())?;
        conn.execute(
            "INSERT INTO automation_events (
                event_id, stream_kind, stream_id, sequence, recorded_at, recorded_at_millis,
                observed_at, event_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event_id,
                kind,
                stream_id,
                i64::try_from(actual_sequence)
                    .map_err(|error| { EventStoreError::Contract(anyhow::Error::new(error)) })?,
                event.recorded_at.as_str(),
                recorded_at_millis,
                event.observed_at.as_str(),
                event_json
            ],
        )?;
        Ok(EventRef {
            stream: EventRefStream::new(format!("{kind}/{stream_id}"))
                .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?,
            sequence: SafeInteger::new(actual_sequence)
                .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?,
        })
    })();

    match result {
        Ok(event_ref) => {
            conn.execute_batch("RELEASE SAVEPOINT coven_automation_event_append")?;
            Ok(event_ref)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO SAVEPOINT coven_automation_event_append;
                 RELEASE SAVEPOINT coven_automation_event_append;",
            );
            Err(error)
        }
    }
}

pub fn append_definition_event(
    conn: &Connection,
    input: DefinitionEventInput<'_>,
) -> Result<EventRef> {
    let event_kind = match input.command {
        "legacy.definition.create.v1" | "definition.create.v1" => "definition.created",
        "legacy.definition.revise.v1" | "definition.revise.v1" => "definition.revised",
        "legacy.definition.delete.v1" | "definition.tombstone.v1" => "definition.tombstoned",
        _ => anyhow::bail!("unsupported definition event command `{}`", input.command),
    };
    append_definition_lifecycle_event(
        conn,
        DefinitionLifecycleEventInput {
            event_id: &format!("evt{}", Uuid::new_v4().simple()),
            event_kind,
            automation_id: input.automation_id,
            revision: input.revision,
            definition_digest: input.definition_digest,
            lifecycle_state: input.lifecycle_state,
            adoption_key: Some(input.adoption_key),
            imported_from: None,
            recorded_at: input.observed_at,
            observed_at: input.observed_at,
        },
    )
}

pub fn append_imported_definition_event(
    conn: &Connection,
    input: ImportedDefinitionEventInput<'_>,
) -> Result<EventRef> {
    let preimage = format!(
        "definition.imported\0{}\0{}\0{}\0{}",
        input.automation_id,
        input.revision,
        input.definition_digest.unwrap_or(""),
        input.imported_from
    );
    let digest = super::canonical_json::sha256_hex(preimage.as_bytes());
    append_definition_lifecycle_event(
        conn,
        DefinitionLifecycleEventInput {
            event_id: &format!("evt{}", &digest[..32]),
            event_kind: "definition.imported",
            automation_id: input.automation_id,
            revision: input.revision,
            definition_digest: input.definition_digest,
            lifecycle_state: input.lifecycle_state,
            adoption_key: None,
            imported_from: Some(input.imported_from),
            recorded_at: input.recorded_at,
            observed_at: input.observed_at,
        },
    )
}

pub fn append_migrated_definition_event(
    conn: &Connection,
    input: MigratedDefinitionEventInput<'_>,
) -> Result<EventRef> {
    let preimage = format!(
        "definition.revised\0{}\0{}\0{}\0{}",
        input.automation_id,
        input.revision,
        input.definition_digest.unwrap_or(""),
        input.migration
    );
    let digest = super::canonical_json::sha256_hex(preimage.as_bytes());
    append_definition_lifecycle_event(
        conn,
        DefinitionLifecycleEventInput {
            event_id: &format!("evt{}", &digest[..32]),
            event_kind: "definition.revised",
            automation_id: input.automation_id,
            revision: input.revision,
            definition_digest: input.definition_digest,
            lifecycle_state: input.lifecycle_state,
            adoption_key: None,
            imported_from: None,
            recorded_at: input.recorded_at,
            observed_at: input.observed_at,
        },
    )
}

fn append_definition_lifecycle_event(
    conn: &Connection,
    input: DefinitionLifecycleEventInput<'_>,
) -> Result<EventRef> {
    let sequence = stream_head(conn, "automation", input.automation_id)?
        .map_or(0, |head| head.saturating_add(1));
    let mut payload = json!({
        "revision": input.revision,
        "lifecycleState": input.lifecycle_state,
    });
    if let Some(definition_digest) = input.definition_digest {
        payload["definitionDigest"] = json!({
            "algorithm": "sha256",
            "canonicalization": "jcs-rfc8785",
            "value": definition_digest,
        });
    }
    if let Some(imported_from) = input.imported_from {
        payload["importedFrom"] = json!(imported_from);
    }
    let mut event_value = json!({
        "schemaVersion": "coven.automations.v1",
        "eventId": input.event_id,
        "stream": {
            "kind": "automation",
            "id": input.automation_id,
        },
        "sequence": sequence,
        "recordedAt": input.recorded_at,
        "observedAt": input.observed_at,
        "producer": {
            "component": "coven-daemon",
            "instanceId": "local-authority",
        },
        "automationId": input.automation_id,
        "kind": input.event_kind,
        "summary": format!(
            "automation {} {} at revision {}",
            input.automation_id, input.lifecycle_state, input.revision
        ),
        "payload": payload,
        "privacy": {
            "classification": "operational",
            "retention": {
                "classification": "standard",
            },
        },
    });
    if let Some(adoption_key) = input.adoption_key {
        event_value["causation"] = json!({"adoptionKey": adoption_key});
    }
    let event: EventEnvelope =
        serde_json::from_value(event_value).context("definition event violates v1 contract")?;
    append_event(conn, &event, sequence).map_err(anyhow::Error::new)
}

pub fn backfill_definition_event_baselines(conn: &Connection) -> Result<()> {
    const MIGRATION: &str = "definition-import-baseline-v1";
    if conn
        .query_row(
            "SELECT 1 FROM automation_event_migrations WHERE name = ?1",
            [MIGRATION],
            |_| Ok(()),
        )
        .optional()
        .context("failed to inspect automation event migration ledger")?
        .is_some()
    {
        return Ok(());
    }
    let mut statement = conn
        .prepare(
            "SELECT id, revision, definition_digest, lifecycle_state, tombstoned_at, updated_at
             FROM automation_definitions
             ORDER BY updated_at, id",
        )
        .context("failed to prepare automation event baseline query")?;
    let definitions = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .context("failed to enumerate automation event baselines")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to read automation event baselines")?;
    drop(statement);

    let completed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    for (id, revision, digest, lifecycle_state, tombstoned_at, updated_at) in definitions {
        if stream_head(conn, "automation", &id)?.is_some() {
            continue;
        }
        let revision =
            u64::try_from(revision).context("automation baseline revision must be non-negative")?;
        let lifecycle_state = if tombstoned_at.is_some() {
            "tombstoned"
        } else {
            lifecycle_state.as_str()
        };
        append_imported_definition_event(
            conn,
            ImportedDefinitionEventInput {
                automation_id: &id,
                revision,
                definition_digest: digest.as_deref(),
                lifecycle_state,
                imported_from: "legacy-coven-store",
                recorded_at: &completed_at,
                observed_at: &updated_at,
            },
        )?;
    }
    conn.execute(
        "INSERT INTO automation_event_migrations (name, completed_at) VALUES (?1, ?2)",
        params![MIGRATION, completed_at],
    )
    .context("failed to record automation event baseline migration")?;
    Ok(())
}

pub fn read_events(
    conn: &Connection,
    stream_kind: &str,
    stream_id: &str,
    after: Option<u64>,
    from: Option<&str>,
    limit: usize,
    issued_at: &str,
) -> Result<EventPage, EventStoreError> {
    if after.is_some() && from.is_some() {
        return Err(EventStoreError::InvalidRead(
            "automation event read accepts `after` or `from`, not both".to_owned(),
        ));
    }
    if !(1..=1_000).contains(&limit) {
        return Err(EventStoreError::InvalidRead(
            "automation event read limit must be between 1 and 1000".to_owned(),
        ));
    }
    let global_feed = stream_kind == "feed";
    if global_feed && stream_id != "all" {
        return Err(EventStoreError::InvalidRead(
            "the global automation feed uses stream id `all`".to_owned(),
        ));
    }
    let resolved_after = if let Some(after) = after {
        i64::try_from(after)
            .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?
    } else if let Some(from) = from {
        let from_millis = timestamp_millis(from)?;
        let first = if global_feed {
            conn.query_row(
                "SELECT feed_position - 1
                 FROM automation_events
                 WHERE recorded_at_millis >= ?1
                 ORDER BY feed_position
                 LIMIT 1",
                [from_millis],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        } else {
            conn.query_row(
                "SELECT sequence
                 FROM automation_events
                 WHERE stream_kind = ?1 AND stream_id = ?2 AND recorded_at_millis >= ?3
                 ORDER BY sequence
                 LIMIT 1",
                params![stream_kind, stream_id, from_millis],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        };
        first.map_or_else(
            || {
                if global_feed {
                    conn.query_row(
                        "SELECT COALESCE(MAX(feed_position) - 1, -1) FROM automation_events",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(EventStoreError::from)
                } else {
                    stream_head(conn, stream_kind, stream_id).map(|head| {
                        head.map_or(-1, |sequence| i64::try_from(sequence).unwrap_or(i64::MAX))
                    })
                }
            },
            |sequence| Ok(sequence - 1),
        )?
    } else {
        -1
    };

    if let Some(earliest_sequence) = conn
        .query_row(
            "SELECT earliest_sequence
             FROM automation_event_stream_heads
             WHERE stream_kind = ?1 AND stream_id = ?2",
            params![stream_kind, stream_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        if resolved_after + 1 < earliest_sequence {
            return Err(EventStoreError::CursorExpired {
                expired_at: issued_at.to_owned(),
            });
        }
    }

    let limit = i64::try_from(limit)
        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?;
    let query = if global_feed {
        "SELECT feed_position - 1, event_json
         FROM automation_events
         WHERE feed_position - 1 > ?3
         ORDER BY feed_position
         LIMIT ?4"
    } else {
        "SELECT sequence, event_json
         FROM automation_events
         WHERE stream_kind = ?1 AND stream_id = ?2 AND sequence > ?3
         ORDER BY sequence
         LIMIT ?4"
    };
    let mut statement = conn.prepare(query)?;
    let rows = statement.query_map(
        params![stream_kind, stream_id, resolved_after, limit],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut events = Vec::new();
    let mut next_after = resolved_after;
    for row in rows {
        let (cursor, event_json) = row?;
        events.push(
            serde_json::from_str(&event_json)
                .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?,
        );
        next_after = cursor;
    }
    let issued = DateTime::parse_from_rfc3339(issued_at)
        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?
        .with_timezone(&Utc);
    let prune_before = (issued - Duration::days(7)).to_rfc3339_opts(SecondsFormat::Millis, true);
    conn.execute(
        "DELETE FROM automation_event_checkpoints
         WHERE checkpoint IN (
             SELECT checkpoint
             FROM automation_event_checkpoints
             WHERE expires_at < ?1
             ORDER BY expires_at
             LIMIT 100
         )",
        [&prune_before],
    )?;
    let expires_at = (issued + Duration::hours(24)).to_rfc3339_opts(SecondsFormat::Millis, true);
    let checkpoint = format!("ecp{}", Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO automation_event_checkpoints (
            checkpoint, stream_kind, stream_id, after_sequence, issued_at, expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            checkpoint,
            stream_kind,
            stream_id,
            next_after,
            issued_at,
            expires_at
        ],
    )?;

    Ok(EventPage {
        stream: EventPageStream {
            kind: stream_kind.to_owned(),
            id: stream_id.to_owned(),
        },
        after: (resolved_after >= 0).then(|| u64::try_from(resolved_after).unwrap_or(u64::MAX)),
        events,
        next_after: (next_after >= 0).then(|| u64::try_from(next_after).unwrap_or(u64::MAX)),
        checkpoint,
        checkpoint_expires_at: expires_at,
    })
}

pub fn resume_events(
    conn: &Connection,
    checkpoint: &str,
    expected_stream_kind: &str,
    expected_stream_id: &str,
    limit: usize,
    resumed_at: &str,
) -> Result<EventPage, EventStoreError> {
    let stored = conn
        .query_row(
            "SELECT stream_kind, stream_id, after_sequence, expires_at
             FROM automation_event_checkpoints
             WHERE checkpoint = ?1",
            [checkpoint],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or(EventStoreError::CheckpointNotFound)?;
    if stored.0 != expected_stream_kind || stored.1 != expected_stream_id {
        return Err(EventStoreError::InvalidRead(
            "automation event checkpoint belongs to a different stream".to_owned(),
        ));
    }
    let resumed = DateTime::parse_from_rfc3339(resumed_at)
        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?;
    let expires = DateTime::parse_from_rfc3339(&stored.3)
        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?;
    if resumed >= expires {
        return Err(EventStoreError::CursorExpired {
            expired_at: stored.3,
        });
    }
    let after = (stored.2 >= 0).then(|| u64::try_from(stored.2).unwrap_or(u64::MAX));
    read_events(
        conn,
        expected_stream_kind,
        expected_stream_id,
        after,
        None,
        limit,
        resumed_at,
    )
}

#[cfg(test)]
#[derive(Debug, Default)]
pub struct EventReducer {
    stream: Option<(String, String)>,
    cursor: Option<u64>,
    seen_event_ids: HashSet<String>,
    state: Value,
}

#[cfg(test)]
impl EventReducer {
    pub fn apply(&mut self, event: &EventEnvelope) -> Result<(), EventStoreError> {
        if self.seen_event_ids.contains(event.event_id.as_str()) {
            return Ok(());
        }
        let incoming_stream = (
            stream_kind(event.stream.kind).to_owned(),
            event.stream.id.as_str().to_owned(),
        );
        if let Some(stream) = &self.stream {
            if stream != &incoming_stream {
                return Err(EventStoreError::InvalidRead(
                    "event reducer cannot combine different streams".to_owned(),
                ));
            }
        } else {
            self.stream = Some(incoming_stream);
        }

        if let (EventKind::FeedSnapshot, EventPayload::Snapshot(snapshot)) =
            (&event.kind, &event.payload)
        {
            let through = snapshot.through_sequence.get();
            if event.sequence.get() != through {
                return Err(EventStoreError::StreamOutOfOrder {
                    expected: through,
                    actual: event.sequence.get(),
                });
            }
            self.state = Value::Object(
                snapshot
                    .state
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            );
            self.cursor = Some(through);
            self.seen_event_ids
                .insert(event.event_id.as_str().to_owned());
            return Ok(());
        }

        let expected = self.cursor.map_or(0, |cursor| cursor.saturating_add(1));
        if event.sequence.get() != expected {
            return Err(EventStoreError::StreamOutOfOrder {
                expected,
                actual: event.sequence.get(),
            });
        }
        let first_sequence = self
            .state
            .pointer("/eventWindow/firstSequence")
            .and_then(Value::as_u64)
            .unwrap_or(event.sequence.get());
        let mut state = Map::new();
        match &event.payload {
            EventPayload::DefinitionLifecycle(payload) => {
                state.insert("entity".to_owned(), json!("definition"));
                state.insert("revision".to_owned(), json!(payload.revision.get()));
                if let Some(lifecycle_state) = payload.lifecycle_state {
                    state.insert(
                        "state".to_owned(),
                        serde_json::to_value(lifecycle_state).map_err(|error| {
                            EventStoreError::Contract(anyhow::Error::new(error))
                        })?,
                    );
                }
                if let Some(digest) = &payload.definition_digest {
                    state.insert(
                        "definitionDigest".to_owned(),
                        serde_json::to_value(digest).map_err(|error| {
                            EventStoreError::Contract(anyhow::Error::new(error))
                        })?,
                    );
                }
            }
            EventPayload::Transition(payload) => {
                state.insert(
                    "entity".to_owned(),
                    serde_json::to_value(payload.entity)
                        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?,
                );
                state.insert("state".to_owned(), json!(payload.to.as_str()));
            }
            EventPayload::Misfire(payload) => {
                state.insert("entity".to_owned(), json!("occurrence"));
                state.insert(
                    "misfire".to_owned(),
                    serde_json::to_value(payload)
                        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?,
                );
            }
            EventPayload::Receipt(payload) => {
                state.insert("entity".to_owned(), json!("receipt"));
                state.insert(
                    "receipt".to_owned(),
                    serde_json::to_value(payload)
                        .map_err(|error| EventStoreError::Contract(anyhow::Error::new(error)))?,
                );
            }
            EventPayload::Snapshot(_) => unreachable!("snapshot payload handled above"),
        }
        state.insert(
            "eventWindow".to_owned(),
            json!({
                "firstSequence": first_sequence,
                "lastSequence": event.sequence.get(),
            }),
        );
        self.state = Value::Object(state);
        self.cursor = Some(event.sequence.get());
        self.seen_event_ids
            .insert(event.event_id.as_str().to_owned());
        Ok(())
    }

    #[must_use]
    pub const fn state(&self) -> &Value {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn definition_event(
        event_id: &str,
        sequence: u64,
        revision: u64,
        lifecycle_state: &str,
    ) -> EventEnvelope {
        serde_json::from_value(json!({
            "schemaVersion": "coven.automations.v1",
            "eventId": event_id,
            "stream": {"kind": "automation", "id": "daily-notes"},
            "sequence": sequence,
            "recordedAt": format!("2026-09-03T09:00:0{sequence}.000Z"),
            "observedAt": format!("2026-09-03T09:00:0{sequence}.000Z"),
            "producer": {"component": "coven-daemon", "instanceId": "daemon@test"},
            "automationId": "daily-notes",
            "kind": if sequence == 0 { "definition.created" } else { "definition.revised" },
            "summary": format!("definition revision {revision}"),
            "payload": {"revision": revision, "lifecycleState": lifecycle_state},
            "privacy": {
                "classification": "operational",
                "retention": {"classification": "standard"}
            }
        }))
        .unwrap()
    }

    fn occurrence_event(event_id: &str, sequence: u64, from: &str, to: &str) -> EventEnvelope {
        serde_json::from_value(json!({
            "schemaVersion": "coven.automations.v1",
            "eventId": event_id,
            "stream": {"kind": "occurrence", "id": "daily-notes-1756544400000"},
            "sequence": sequence,
            "recordedAt": format!("2026-09-03T09:00:0{sequence}.000Z"),
            "observedAt": format!("2026-09-03T09:00:0{sequence}.000Z"),
            "producer": {"component": "coven-daemon", "instanceId": "daemon@test"},
            "automationId": "daily-notes",
            "occurrenceId": "daily-notes-1756544400000",
            "kind": "occurrence.transitioned",
            "summary": format!("occurrence {from} -> {to}"),
            "payload": {
                "entity": "occurrence",
                "from": from,
                "to": to,
                "reason": "test",
                "fenceGeneration": 1
            },
            "privacy": {
                "classification": "operational",
                "retention": {"classification": "standard"}
            }
        }))
        .unwrap()
    }

    fn snapshot_event(event_id: &str, sequence: u64, through_sequence: u64) -> EventEnvelope {
        serde_json::from_value(json!({
            "schemaVersion": "coven.automations.v1",
            "eventId": event_id,
            "stream": {"kind": "occurrence", "id": "daily-notes-1756544400000"},
            "sequence": sequence,
            "recordedAt": "2026-09-03T09:00:03.000Z",
            "observedAt": "2026-09-03T09:00:03.000Z",
            "producer": {"component": "coven-daemon", "instanceId": "daemon@test"},
            "automationId": "daily-notes",
            "occurrenceId": "daily-notes-1756544400000",
            "kind": "feed.snapshot",
            "summary": "compacted occurrence state",
            "payload": {
                "throughSequence": through_sequence,
                "state": {
                    "entity": "occurrence",
                    "state": "claimed",
                    "eventWindow": {"firstSequence": 0, "lastSequence": through_sequence}
                },
                "reason": "retention_compaction"
            },
            "privacy": {
                "classification": "operational",
                "retention": {"classification": "standard"}
            }
        }))
        .unwrap()
    }

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(AUTOMATION_EVENTS_SCHEMA_SQL).unwrap();
        conn
    }

    #[test]
    fn append_refuses_out_of_order_sequence_and_duplicate_event_id() {
        let conn = connection();
        let first = definition_event("evtdefinition0000000001", 0, 1, "paused");
        append_event(&conn, &first, 0).unwrap();

        let skipped = definition_event("evtdefinition0000000002", 2, 2, "active");
        let error = append_event(&conn, &skipped, 2).unwrap_err();
        assert_eq!(error.code(), "STREAM_OUT_OF_ORDER");
        assert_eq!(
            stream_head(&conn, "automation", "daily-notes").unwrap(),
            Some(0)
        );

        let duplicate = definition_event("evtdefinition0000000001", 1, 2, "active");
        let error = append_event(&conn, &duplicate, 1).unwrap_err();
        assert_eq!(error.code(), "DUPLICATE_EVENT_ID");
        assert_eq!(
            stream_head(&conn, "automation", "daily-notes").unwrap(),
            Some(0)
        );
    }

    #[test]
    fn read_uses_an_exclusive_cursor_and_checkpoint_resume_converges() {
        let conn = connection();
        for event in [
            definition_event("evtdefinition0000000010", 0, 1, "paused"),
            definition_event("evtdefinition0000000011", 1, 2, "active"),
            definition_event("evtdefinition0000000012", 2, 3, "disabled"),
        ] {
            append_event(&conn, &event, event.sequence.get()).unwrap();
        }

        let first = read_events(
            &conn,
            "automation",
            "daily-notes",
            None,
            None,
            2,
            "2026-09-03T09:10:00.000Z",
        )
        .unwrap();
        assert_eq!(
            first
                .events
                .iter()
                .map(|event| event.sequence.get())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        let resumed = resume_events(
            &conn,
            &first.checkpoint,
            "automation",
            "daily-notes",
            100,
            "2026-09-03T09:11:00.000Z",
        )
        .unwrap();
        assert_eq!(
            resumed
                .events
                .iter()
                .map(|event| event.sequence.get())
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(resumed.after, Some(1));
    }

    #[test]
    fn expired_checkpoint_is_refused_without_rewinding() {
        let conn = connection();
        let event = definition_event("evtdefinition0000000020", 0, 1, "paused");
        append_event(&conn, &event, 0).unwrap();
        let page = read_events(
            &conn,
            "automation",
            "daily-notes",
            None,
            None,
            1,
            "2026-09-03T09:10:00.000Z",
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_event_checkpoints
             SET expires_at = '2026-09-03T09:10:30.000Z'
             WHERE checkpoint = ?1",
            [&page.checkpoint],
        )
        .unwrap();

        let error = resume_events(
            &conn,
            &page.checkpoint,
            "automation",
            "daily-notes",
            100,
            "2026-09-03T09:11:00.000Z",
        )
        .unwrap_err();
        assert_eq!(error.code(), "CURSOR_EXPIRED");
        assert_eq!(error.expired_at(), Some("2026-09-03T09:10:30.000Z"));
    }

    #[test]
    fn reducer_deduplicates_delivery_and_rejects_sequence_regression() {
        let events = [
            occurrence_event("evtoccurrence0000000001", 0, "none", "planned"),
            occurrence_event("evtoccurrence0000000002", 1, "planned", "claimed"),
            occurrence_event("evtoccurrence0000000003", 2, "claimed", "running"),
        ];
        let mut reducer = EventReducer::default();
        for event in [&events[0], &events[1], &events[1], &events[2]] {
            reducer.apply(event).unwrap();
        }
        assert_eq!(reducer.state()["state"], "running");
        assert_eq!(reducer.state()["eventWindow"]["lastSequence"], 2);

        let regression = occurrence_event("evtoccurrence0000000004", 1, "planned", "claimed");
        let error = reducer.apply(&regression).unwrap_err();
        assert_eq!(error.code(), "STREAM_OUT_OF_ORDER");
    }

    #[test]
    fn snapshot_then_strictly_later_tail_matches_full_reduction() {
        let events = [
            occurrence_event("evtoccurrence0000000010", 0, "none", "planned"),
            occurrence_event("evtoccurrence0000000011", 1, "planned", "eligible"),
            occurrence_event("evtoccurrence0000000012", 2, "eligible", "claimed"),
            occurrence_event("evtoccurrence0000000013", 3, "claimed", "running"),
            occurrence_event("evtoccurrence0000000014", 4, "running", "succeeded"),
        ];
        let mut full = EventReducer::default();
        for event in &events {
            full.apply(event).unwrap();
        }

        let snapshot = snapshot_event("evtsnapshot00000000001", 2, 2);
        let mut compacted = EventReducer::default();
        compacted.apply(&snapshot).unwrap();
        compacted.apply(&events[3]).unwrap();
        compacted.apply(&events[4]).unwrap();

        assert_eq!(compacted.state(), full.state());
    }

    #[test]
    fn read_from_timestamp_resolves_a_concrete_exclusive_cursor() {
        let conn = connection();
        for event in [
            definition_event("evtdefinition0000000030", 0, 1, "paused"),
            definition_event("evtdefinition0000000031", 1, 2, "active"),
            definition_event("evtdefinition0000000032", 2, 3, "disabled"),
        ] {
            append_event(&conn, &event, event.sequence.get()).unwrap();
        }

        let page = read_events(
            &conn,
            "automation",
            "daily-notes",
            None,
            Some("2026-09-03T09:00:01.000Z"),
            100,
            "2026-09-03T09:10:00.000Z",
        )
        .unwrap();

        assert_eq!(page.after, Some(0));
        assert_eq!(
            page.events
                .iter()
                .map(|event| event.sequence.get())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            serde_json::to_value(&page.events[0]).unwrap()["recordedAt"],
            Value::String("2026-09-03T09:00:01.000Z".to_owned())
        );
    }

    #[test]
    fn checkpoint_creation_prunes_only_entries_beyond_the_expiry_grace() {
        let conn = connection();
        conn.execute(
            "INSERT INTO automation_event_checkpoints (
                checkpoint, stream_kind, stream_id, after_sequence, issued_at, expires_at
             ) VALUES
                ('ecpstale0000000000000000000001', 'automation', 'old', -1,
                 '2026-08-01T00:00:00.000Z', '2026-08-02T00:00:00.000Z'),
                ('ecprecent00000000000000000001', 'automation', 'recent', -1,
                 '2026-09-02T00:00:00.000Z', '2026-09-02T12:00:00.000Z')",
            [],
        )
        .unwrap();

        read_events(
            &conn,
            "automation",
            "daily-notes",
            None,
            None,
            100,
            "2026-09-03T09:10:00.000Z",
        )
        .unwrap();

        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM automation_event_checkpoints
                 WHERE checkpoint = 'ecpstale0000000000000000000001'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM automation_event_checkpoints
                 WHERE checkpoint = 'ecprecent00000000000000000001'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn global_feed_pages_across_domain_streams_with_its_own_cursor() {
        let conn = connection();
        let first = definition_event("evtdefinition0000000040", 0, 1, "paused");
        append_event(&conn, &first, 0).unwrap();
        let mut second_value =
            serde_json::to_value(definition_event("evtdefinition0000000041", 0, 1, "active"))
                .unwrap();
        second_value["stream"]["id"] = json!("weekly-notes");
        second_value["automationId"] = json!("weekly-notes");
        let second: EventEnvelope = serde_json::from_value(second_value).unwrap();
        append_event(&conn, &second, 0).unwrap();

        let page = read_events(
            &conn,
            "feed",
            "all",
            None,
            None,
            1,
            "2026-09-03T09:10:00.000Z",
        )
        .unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(
            page.events[0].automation_id.as_ref().unwrap().as_str(),
            "daily-notes"
        );
        assert_eq!(page.next_after, Some(0));

        let tail = read_events(
            &conn,
            "feed",
            "all",
            page.next_after,
            None,
            10,
            "2026-09-03T09:11:00.000Z",
        )
        .unwrap();
        assert_eq!(tail.events.len(), 1);
        assert_eq!(
            tail.events[0].automation_id.as_ref().unwrap().as_str(),
            "weekly-notes"
        );
        assert_eq!(tail.next_after, Some(1));
    }

    #[test]
    fn read_from_whole_second_includes_later_fractional_event() {
        let conn = connection();
        let mut value =
            serde_json::to_value(definition_event("evtdefinition0000000050", 0, 1, "paused"))
                .unwrap();
        value["recordedAt"] = json!("2026-09-03T09:00:00.500Z");
        value["observedAt"] = json!("2026-09-03T09:00:00.500Z");
        let event: EventEnvelope = serde_json::from_value(value).unwrap();
        append_event(&conn, &event, 0).unwrap();

        let page = read_events(
            &conn,
            "automation",
            "daily-notes",
            None,
            Some("2026-09-03T09:00:00Z"),
            100,
            "2026-09-03T09:10:00.000Z",
        )
        .unwrap();

        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].event_id.as_str(), "evtdefinition0000000050");
    }

    #[test]
    fn upgraded_definition_receives_one_deterministic_import_baseline() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::automations::store::AUTOMATION_DEFINITIONS_SCHEMA_SQL)
            .unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                authority_version, created_at, updated_at
             ) VALUES (
                'upgraded', 'Upgraded', 'PAUSED', '{}', 7,
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                'paused', 0, '2026-09-01T09:00:00.000Z', '2026-09-02T09:00:00.000Z'
             )",
            [],
        )
        .unwrap();
        conn.execute_batch(AUTOMATION_EVENTS_SCHEMA_SQL).unwrap();

        backfill_definition_event_baselines(&conn).unwrap();
        let first: String = conn
            .query_row(
                "SELECT event_json FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'upgraded'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        backfill_definition_event_baselines(&conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM automation_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let event: Value = serde_json::from_str(&first).unwrap();
        assert_eq!(event["kind"], "definition.imported");
        assert_eq!(event["sequence"], 0);
        assert_eq!(event["payload"]["revision"], 7);
        assert_eq!(event["payload"]["importedFrom"], "legacy-coven-store");
        assert_ne!(event["recordedAt"], event["observedAt"]);
        assert_eq!(event["observedAt"], "2026-09-02T09:00:00.000Z");
    }
}
