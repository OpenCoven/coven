//! Bounded, payload-free observations of retained per-stream sequence metadata.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use rusqlite::Connection;

use super::super::contract::types::{EventStreamId, StreamKind};

pub(super) const MAX_HEADS: usize = 256;
pub(super) const MAX_EVENTS: usize = 4096;
pub(super) const MAX_INPUT_BYTES: usize = 1_048_576;
pub(super) const PREREQUISITES: &[(&str, &[&str])] = &[
    (
        "automation_event_stream_heads",
        &[
            "stream_kind",
            "stream_id",
            "earliest_sequence",
            "next_sequence",
        ],
    ),
    (
        "automation_events",
        &["stream_kind", "stream_id", "sequence"],
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Stats {
    pub streams: u64,
    pub mismatches: u64,
    pub orphans: u64,
    pub before_retention: u64,
    pub beyond_head: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Observation {
    Complete(Stats),
    InvalidMetadata,
    BudgetExceeded,
}

type Key = (u8, String);
struct Head {
    earliest: u64,
    next: u64,
    inside: u64,
    beyond: bool,
}

fn decode_key(kind: Option<Vec<u8>>, id: Option<Vec<u8>>) -> Option<Key> {
    let kind = String::from_utf8(kind?).ok()?;
    let kind = serde_json::from_value::<StreamKind>(serde_json::Value::String(kind)).ok()?;
    let kind = match kind {
        StreamKind::Automation => 0,
        StreamKind::Occurrence => 1,
        StreamKind::Run => 2,
        StreamKind::Feed => 3,
    };
    let id = id.filter(|bytes| bytes.len() <= 1280)?;
    let id = EventStreamId::new(String::from_utf8(id).ok()?).ok()?;
    if kind == 3 && id.as_str() != "all" {
        return None;
    }
    Some((kind, id.as_str().to_owned()))
}

fn consume(remaining: &mut usize, kind: &Option<Vec<u8>>, id: &Option<Vec<u8>>) -> bool {
    let size = kind.as_ref().map_or(0, Vec::len) + id.as_ref().map_or(0, Vec::len);
    if let Some(next) = remaining.checked_sub(size) {
        *remaining = next;
        true
    } else {
        false
    }
}

/// The caller owns the coherent read snapshot and preflights query prerequisites.
/// Caps bound input materialization, not whole-report SQL time or ratified SLOs.
pub(super) fn inspect(conn: &Connection) -> Result<Observation> {
    let mut remaining = MAX_INPUT_BYTES;
    let mut heads = BTreeMap::<Key, Head>::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT CASE WHEN typeof(stream_kind)='text' THEN substr(CAST(stream_kind AS BLOB),1,17) END,
             CASE WHEN typeof(stream_id)='text' THEN substr(CAST(stream_id AS BLOB),1,1281) END,
             CASE WHEN typeof(earliest_sequence)='integer' AND earliest_sequence BETWEEN 0 AND 9007199254740991 THEN earliest_sequence END,
             CASE WHEN typeof(next_sequence)='integer' AND next_sequence BETWEEN 0 AND 9007199254740991 THEN next_sequence END
             FROM main.automation_event_stream_heads LIMIT {}", MAX_HEADS + 1))?;
        let mut rows = statement.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            if count == MAX_HEADS {
                return Ok(Observation::BudgetExceeded);
            }
            count += 1;
            let kind = row.get::<_, Option<Vec<u8>>>(0)?;
            let id = row.get::<_, Option<Vec<u8>>>(1)?;
            if !consume(&mut remaining, &kind, &id) {
                return Ok(Observation::BudgetExceeded);
            }
            let Some(key) = decode_key(kind, id) else {
                return Ok(Observation::InvalidMetadata);
            };
            let (Some(earliest), Some(next)) =
                (row.get::<_, Option<i64>>(2)?, row.get::<_, Option<i64>>(3)?)
            else {
                return Ok(Observation::InvalidMetadata);
            };
            if earliest > next {
                return Ok(Observation::InvalidMetadata);
            }
            // The SQL projection admits only nonnegative safe integers.
            let head = Head {
                earliest: u64::try_from(earliest)?,
                next: u64::try_from(next)?,
                inside: 0,
                beyond: false,
            };
            if heads.insert(key, head).is_some() {
                return Ok(Observation::InvalidMetadata);
            }
        }
    }
    let mut stats = Stats {
        streams: u64::try_from(heads.len())?,
        mismatches: 0,
        orphans: 0,
        before_retention: 0,
        beyond_head: 0,
    };
    let mut seen = BTreeSet::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT CASE WHEN typeof(stream_kind)='text' THEN substr(CAST(stream_kind AS BLOB),1,17) END,
             CASE WHEN typeof(stream_id)='text' THEN substr(CAST(stream_id AS BLOB),1,1281) END,
             CASE WHEN typeof(sequence)='integer' AND sequence BETWEEN 0 AND 9007199254740991 THEN sequence END
             FROM main.automation_events LIMIT {}", MAX_EVENTS + 1))?;
        let mut rows = statement.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            if count == MAX_EVENTS {
                return Ok(Observation::BudgetExceeded);
            }
            count += 1;
            let kind = row.get::<_, Option<Vec<u8>>>(0)?;
            let id = row.get::<_, Option<Vec<u8>>>(1)?;
            if !consume(&mut remaining, &kind, &id) {
                return Ok(Observation::BudgetExceeded);
            }
            let Some(key) = decode_key(kind, id) else {
                return Ok(Observation::InvalidMetadata);
            };
            let Some(sequence) = row.get::<_, Option<i64>>(2)? else {
                return Ok(Observation::InvalidMetadata);
            };
            let sequence = u64::try_from(sequence)?;
            if !seen.insert((key.clone(), sequence)) {
                return Ok(Observation::InvalidMetadata);
            }
            if let Some(head) = heads.get_mut(&key) {
                if sequence < head.earliest {
                    stats.before_retention += 1;
                } else if sequence >= head.next {
                    stats.beyond_head += 1;
                    head.beyond = true;
                } else {
                    head.inside += 1;
                }
            } else {
                stats.orphans += 1;
            }
        }
    }
    // Distinct integral values inside a finite interval fill it iff their count
    // equals its width. Do not enumerate a potentially enormous missing range.
    stats.mismatches = u64::try_from(
        heads
            .values()
            .filter(|head| head.inside != head.next - head.earliest || head.beyond)
            .count(),
    )?;
    Ok(Observation::Complete(stats))
}
