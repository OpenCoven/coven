//! Retained failures after latest retained success; not a strict terminal streak.
use std::collections::BTreeMap;

use anyhow::{ensure, Result};
use chrono::{DateTime, FixedOffset};
use rusqlite::Connection;

use super::super::contract::types::{AutomationId, OccurrenceState};
use super::{Fact, Gap, Observation, MAX_SAFE};

pub(super) const MAX_ROWS: u64 = super::planning::MAX_ROWS;
const FACTS: [Fact; 3] = [
    Fact::RetainedOccurrenceFailureHistoriesChecked,
    Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess,
    Fact::MaximumRetainedFailuresSinceSuccess,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Measurement {
    Complete {
        histories: u64,
        repeated: u64,
        maximum: u64,
    },
    Unavailable,
    InvalidMetadata,
    BudgetExceeded,
}

fn text(bytes: Option<Vec<u8>>, maximum: usize) -> Option<String> {
    let bytes = bytes?;
    (bytes.len() <= maximum).then_some(())?;
    String::from_utf8(bytes).ok()
}

fn instant(bytes: Option<Vec<u8>>) -> Option<DateTime<FixedOffset>> {
    let value = DateTime::parse_from_rfc3339(&text(bytes, 64)?).ok()?;
    u64::try_from(value.timestamp_millis())
        .ok()
        .filter(|value| *value <= MAX_SAFE)?;
    Some(value)
}

pub(super) fn inspect(conn: &Connection) -> Result<Measurement> {
    if !super::projection_available(
        conn,
        "automation_occurrences",
        &["automation_id", "scheduled_for", "state"],
    )? {
        return Ok(Measurement::Unavailable);
    }
    let mut statement = conn.prepare(
        "SELECT CASE WHEN typeof(automation_id)='text' THEN substr(CAST(automation_id AS BLOB),1,97) END,
         CASE WHEN typeof(scheduled_for)='text' THEN substr(CAST(scheduled_for AS BLOB),1,65) END,
         CASE WHEN typeof(state)='text' THEN substr(CAST(state AS BLOB),1,18) END
         FROM main.automation_occurrences LIMIT 257",
    )?;
    let mut rows = statement.query([])?;
    let mut histories =
        BTreeMap::<AutomationId, Vec<(DateTime<FixedOffset>, OccurrenceState)>>::new();
    let mut total = 0;
    while let Some(row) = rows.next()? {
        if total == MAX_ROWS {
            return Ok(Measurement::BudgetExceeded);
        }
        total += 1;
        let id = text(row.get(0)?, 96).and_then(|value| AutomationId::new(value).ok());
        let at = instant(row.get(1)?);
        let state = text(row.get(2)?, 17).and_then(|value| {
            serde_json::from_value::<OccurrenceState>(serde_json::Value::String(value)).ok()
        });
        let (Some(id), Some(at), Some(state)) = (id, at, state) else {
            return Ok(Measurement::InvalidMetadata);
        };
        // Manual and scheduled writers can use different timestamp precision.
        // Keep distinct retained rows even when their instants are equal.
        histories.entry(id).or_default().push((at, state));
    }
    let mut repeated = 0;
    let mut maximum = 0;
    for history in histories.values() {
        let latest_success = history
            .iter()
            .filter_map(|(at, state)| (*state == OccurrenceState::Succeeded).then_some(at))
            .max();
        let failures = history
            .iter()
            .filter(|(at, state)| {
                *state == OccurrenceState::Failed
                    && latest_success.is_none_or(|success| at > success)
            })
            .count() as u64;
        repeated += u64::from(failures >= 2);
        maximum = maximum.max(failures);
    }
    Ok(Measurement::Complete {
        histories: histories.len() as u64,
        repeated,
        maximum,
    })
}

pub(super) fn append(observation: &mut Observation, measurement: Measurement) {
    match measurement {
        Measurement::Complete {
            histories,
            repeated,
            maximum,
        } => observation
            .facts
            .extend(FACTS.into_iter().zip([histories, repeated, maximum])),
        Measurement::Unavailable => observation
            .gaps
            .push(Gap::OccurrenceFailureHistoryStoreUnavailable),
        Measurement::InvalidMetadata => observation
            .gaps
            .push(Gap::OccurrenceFailureHistoryMetadataInvalid),
        Measurement::BudgetExceeded => observation
            .gaps
            .push(Gap::OccurrenceFailureHistoryReadBudgetExceeded),
    }
}

pub(super) fn validate_and_strip(observation: &mut Observation) -> Result<()> {
    let incomplete = matches!(
        observation.gaps.last(),
        Some(
            Gap::OccurrenceFailureHistoryStoreUnavailable
                | Gap::OccurrenceFailureHistoryMetadataInvalid
                | Gap::OccurrenceFailureHistoryReadBudgetExceeded
        )
    );
    if incomplete {
        ensure!(
            FACTS
                .iter()
                .all(|fact| !observation.facts.contains_key(fact)),
            "incomplete failure history carries facts"
        );
        observation.gaps.pop();
        return Ok(());
    }
    let [Some(histories), Some(repeated), Some(maximum)] =
        FACTS.map(|fact| observation.facts.remove(&fact))
    else {
        anyhow::bail!("missing failure-history facts");
    };
    ensure!(
        histories <= MAX_ROWS && repeated <= histories && maximum <= MAX_ROWS,
        "invalid failure-history bounds"
    );
    ensure!(
        (histories != 0 || maximum == 0) && ((repeated != 0) == (maximum >= 2)),
        "invalid repeated failure history"
    );
    // Each history needs a row; each repeated history needs >=2 failures; the
    // maximum history uses its advertised maximum. These are necessary bounds,
    // not evidence that missing/pruned history never existed.
    let minimum_rows = if maximum >= 2 {
        maximum + histories + repeated - 2
    } else {
        histories
    };
    ensure!(
        minimum_rows <= MAX_ROWS,
        "failure histories contradict completed cohort"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::types::Value;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_occurrences(automation_id, scheduled_for, state)",
        )
        .unwrap();
        conn
    }

    #[test]
    fn invalid_cells_do_not_become_empty_histories() {
        for column in ["automation_id", "scheduled_for", "state"] {
            for value in [
                Value::Null,
                Value::Integer(1),
                Value::Real(1.5),
                Value::Blob(b"failed".to_vec()),
                Value::Text("PRIVATE HISTORY".into()),
                Value::Text("x".repeat(1_048_576)),
                Value::Text("failed\0PRIVATE".into()),
            ] {
                let conn = fixture();
                conn.execute_batch("INSERT INTO automation_occurrences VALUES ('routine-a','2026-10-18T00:00:00Z','failed')").unwrap();
                conn.execute(
                    &format!("UPDATE automation_occurrences SET {column}=?1"),
                    [value],
                )
                .unwrap();
                assert_eq!(
                    inspect(&conn).unwrap(),
                    Measurement::InvalidMetadata,
                    "column {column}"
                );
                conn.execute(
                    &format!("UPDATE automation_occurrences SET {column}=CAST(x'ff' AS TEXT)"),
                    [],
                )
                .unwrap();
                assert_eq!(inspect(&conn).unwrap(), Measurement::InvalidMetadata);
            }
        }
    }

    #[test]
    fn a_success_excludes_failed_rows_at_the_same_instant() {
        let conn = fixture();
        conn.execute_batch(
            "INSERT INTO automation_occurrences VALUES
            ('a','2026-10-18T00:00:00Z','failed'),('a','2026-10-18T01:00:00+01:00','succeeded')",
        )
        .unwrap();
        assert_eq!(
            inspect(&conn).unwrap(),
            Measurement::Complete {
                histories: 1,
                repeated: 0,
                maximum: 0
            }
        );
    }

    #[test]
    fn main_dependencies_cannot_be_replaced_by_temp() {
        for omitted in [
            None,
            Some("automation_id"),
            Some("scheduled_for"),
            Some("state"),
        ] {
            let conn = Connection::open_in_memory().unwrap();
            if let Some(omitted) = omitted {
                let columns = ["automation_id", "scheduled_for", "state"]
                    .into_iter()
                    .filter(|v| *v != omitted)
                    .collect::<Vec<_>>()
                    .join(",");
                conn.execute_batch(&format!("CREATE TABLE automation_occurrences({columns})"))
                    .unwrap();
            }
            conn.execute_batch("CREATE TEMP TABLE automation_occurrences(automation_id,scheduled_for,state);
                INSERT INTO temp.automation_occurrences VALUES ('a','2026-10-18T00:00:00Z','failed')").unwrap();
            assert_eq!(inspect(&conn).unwrap(), Measurement::Unavailable);
        }
        let conn = fixture();
        conn.execute_batch(
            "CREATE TEMP TABLE automation_occurrences(automation_id,scheduled_for,state);
            INSERT INTO temp.automation_occurrences VALUES ('private','not-time','not-state')",
        )
        .unwrap();
        assert_eq!(
            inspect(&conn).unwrap(),
            Measurement::Complete {
                histories: 0,
                repeated: 0,
                maximum: 0
            }
        );
    }

    #[test]
    fn full_cap_is_measured_and_next_row_never_returns_partial_history() {
        let conn = fixture();
        conn.execute_batch("WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM rows WHERE n<256)
            INSERT INTO automation_occurrences SELECT 'routine-'||n,'2026-10-18T00:00:00Z','failed' FROM rows").unwrap();
        assert_eq!(
            inspect(&conn).unwrap(),
            Measurement::Complete {
                histories: 256,
                repeated: 0,
                maximum: 1
            }
        );
        conn.execute_batch("INSERT INTO automation_occurrences VALUES ('extra','2026-10-18T00:00:00Z','succeeded')").unwrap();
        assert_eq!(inspect(&conn).unwrap(), Measurement::BudgetExceeded);
    }

    #[test]
    fn exact_state_is_required_despite_permissive_collation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE automation_occurrences(automation_id,scheduled_for,state TEXT COLLATE NOCASE);
            INSERT INTO automation_occurrences VALUES ('a','2026-10-18T00:00:00Z','FAILED')").unwrap();
        assert_eq!(inspect(&conn).unwrap(), Measurement::InvalidMetadata);
    }
}

#[cfg(test)]
mod precision_regression {
    use super::*;
    #[test]
    fn manual_and_scheduled_timestamp_precision_can_share_an_instant() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE automation_occurrences(automation_id,scheduled_for,state,UNIQUE(automation_id,scheduled_for));
            INSERT INTO automation_occurrences VALUES
            ('a','2026-10-18T00:00:00.000Z','failed'),
            ('a','2026-10-18T00:00:00.000000000Z','failed')").unwrap();
        assert_eq!(
            inspect(&conn).unwrap(),
            Measurement::Complete {
                histories: 1,
                repeated: 1,
                maximum: 2
            }
        );
    }
}
