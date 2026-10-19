//! Bounded retained planning metadata, never runtime-start or SLO evidence.
use anyhow::{ensure, Result};
use chrono::{DateTime, FixedOffset};
use rusqlite::Connection;

use super::{Fact, Gap, Observation, MAX_SAFE};

pub(super) const MAX_ROWS: u64 = 256;
const FACTS: [Fact; 2] = [
    Fact::ScheduledOccurrencesPlanningChecked,
    Fact::MaximumScheduledOccurrencePlanningDelayMs,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Measurement {
    Complete { checked: u64, maximum: u64 },
    Unavailable,
    InvalidMetadata,
    BudgetExceeded,
}

fn instant(bytes: Option<Vec<u8>>) -> Option<DateTime<FixedOffset>> {
    let bytes = bytes?;
    if bytes.len() > 64 {
        return None;
    }
    let instant = DateTime::parse_from_rfc3339(std::str::from_utf8(&bytes).ok()?).ok()?;
    u64::try_from(instant.timestamp_millis())
        .ok()
        .filter(|value| *value <= MAX_SAFE)?;
    Some(instant)
}

pub(super) fn inspect(conn: &Connection) -> Result<Measurement> {
    if !super::projection_available(
        conn,
        "automation_occurrences",
        &["kind", "scheduled_for", "created_at"],
    )? {
        return Ok(Measurement::Unavailable);
    }
    let mut statement = conn.prepare(
        "SELECT CASE WHEN typeof(kind)='text' THEN substr(CAST(kind AS BLOB),1,10) END,
         CASE WHEN typeof(scheduled_for)='text' THEN substr(CAST(scheduled_for AS BLOB),1,65) END,
         CASE WHEN typeof(created_at)='text' THEN substr(CAST(created_at AS BLOB),1,65) END
         FROM main.automation_occurrences LIMIT 257",
    )?;
    let mut rows = statement.query([])?;
    let mut total = 0;
    let mut checked = 0;
    let mut maximum = 0;
    while let Some(row) = rows.next()? {
        if total == MAX_ROWS {
            return Ok(Measurement::BudgetExceeded);
        }
        total += 1;
        let kind: Option<Vec<u8>> = row.get(0)?;
        match kind.as_deref() {
            Some(b"manual") => continue,
            Some(b"scheduled") => {}
            _ => return Ok(Measurement::InvalidMetadata),
        }
        let (Some(scheduled), Some(created)) = (instant(row.get(1)?), instant(row.get(2)?)) else {
            return Ok(Measurement::InvalidMetadata);
        };
        let delay = (created - scheduled).num_milliseconds().max(0) as u64;
        if delay > MAX_SAFE {
            return Ok(Measurement::InvalidMetadata);
        }
        checked += 1;
        maximum = maximum.max(delay);
    }
    Ok(Measurement::Complete { checked, maximum })
}

pub(super) fn append(observation: &mut Observation, measurement: Measurement) {
    match measurement {
        Measurement::Complete { checked, maximum } => {
            observation
                .facts
                .extend(FACTS.into_iter().zip([checked, maximum]));
        }
        Measurement::Unavailable => observation
            .gaps
            .push(Gap::OccurrencePlanningStoreUnavailable),
        Measurement::InvalidMetadata => observation
            .gaps
            .push(Gap::OccurrencePlanningMetadataInvalid),
        Measurement::BudgetExceeded => observation
            .gaps
            .push(Gap::OccurrencePlanningReadBudgetExceeded),
    }
}

/// Validate this independent group, then leave the existing pass contract to its
/// own validator. No planning gap can hide malformed pass facts or vice versa.
pub(super) fn validate_and_strip(observation: &mut Observation) -> Result<()> {
    let incomplete = matches!(
        observation.gaps.last(),
        Some(
            Gap::OccurrencePlanningStoreUnavailable
                | Gap::OccurrencePlanningMetadataInvalid
                | Gap::OccurrencePlanningReadBudgetExceeded
        )
    );
    if incomplete {
        ensure!(
            FACTS
                .iter()
                .all(|fact| !observation.facts.contains_key(fact)),
            "incomplete planning measurement carries facts"
        );
        observation.gaps.pop();
    } else {
        let checked = observation.facts.remove(&FACTS[0]);
        let maximum = observation.facts.remove(&FACTS[1]);
        let (Some(checked), Some(maximum)) = (checked, maximum) else {
            anyhow::bail!("missing planning measurement facts");
        };
        ensure!(
            checked <= MAX_ROWS && maximum <= MAX_SAFE && (checked != 0 || maximum == 0),
            "invalid planning measurement facts"
        );
    }
    Ok(())
}

/// A completed scan visited every retained occurrence, including manual rows.
/// Independent counts from the same snapshot cannot contradict that upper bound.
pub(super) fn validate_cohort(observations: &[Observation]) -> Result<()> {
    if !observations.iter().any(|observation| {
        (observation.category == super::Category::PlanningStartLag
            && observation
                .facts
                .contains_key(&Fact::ScheduledOccurrencesPlanningChecked))
            || (observation.category == super::Category::RepeatedFailures
                && observation
                    .facts
                    .contains_key(&Fact::RetainedOccurrenceFailureHistoriesChecked))
    }) {
        return Ok(());
    }
    let history = observations.iter().find(|observation| {
        observation.category == super::Category::RepeatedFailures
            && observation
                .facts
                .contains_key(&Fact::RetainedOccurrenceFailureHistoriesChecked)
    });
    let empty_history = history.is_some_and(|observation| {
        observation.facts[&Fact::RetainedOccurrenceFailureHistoriesChecked] == 0
    });
    let minimum_failed = history.map_or(0, |observation| {
        let maximum = observation.facts[&Fact::MaximumRetainedFailuresSinceSuccess];
        if maximum >= 2 {
            maximum
                + 2 * (observation.facts[&Fact::RoutinesWithRepeatedRetainedFailuresSinceSuccess]
                    - 1)
        } else {
            maximum
        }
    });
    let occurrence_cap = if empty_history { 0 } else { MAX_ROWS };
    for observation in observations {
        match observation.category {
            super::Category::Queue => {
                let sum: u64 = [
                    Fact::PlannedOccurrences,
                    Fact::ClaimedOccurrences,
                    Fact::RunningOccurrences,
                    Fact::RecoveryRequiredOccurrences,
                ]
                .iter()
                .filter_map(|fact| observation.facts.get(fact))
                .sum();
                // Failed occurrence rows are disjoint from these four states.
                ensure!(
                    sum + minimum_failed <= occurrence_cap,
                    "queue contradicts completed retained occurrence cohort"
                );
            }
            super::Category::LeasesFences => {
                for fact in [
                    Fact::LeaseBearingOccurrences,
                    Fact::MismatchedUnlaunchedSchedulerClaims,
                ] {
                    ensure!(
                        observation
                            .facts
                            .get(&fact)
                            .is_none_or(|count| *count <= occurrence_cap),
                        "lease metadata contradicts completed retained occurrence cohort"
                    );
                }
                if let Some(mismatched) = observation
                    .facts
                    .get(&Fact::MismatchedUnlaunchedSchedulerClaims)
                {
                    ensure!(
                        mismatched + minimum_failed <= occurrence_cap,
                        "claimed rows contradict completed failure cohort"
                    );
                }
                if let (Some(total), Some(outside)) = (
                    observation.facts.get(&Fact::LeaseBearingOccurrences),
                    observation
                        .facts
                        .get(&Fact::LeaseOutsideActiveStateOccurrences),
                ) {
                    // Core lease validation already proves outside <= total.
                    ensure!(
                        total - outside + minimum_failed <= occurrence_cap,
                        "active-state leases contradict completed failure cohort"
                    );
                }
            }
            super::Category::PlanningStartLag if empty_history => {
                ensure!(
                    observation
                        .facts
                        .get(&Fact::ScheduledOccurrencesPlanningChecked)
                        .is_none_or(|count| *count == 0),
                    "planning contradicts empty retained history"
                );
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::types::Value;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE automation_occurrences(kind, scheduled_for, created_at)")
            .unwrap();
        conn
    }

    #[test]
    fn malformed_selected_metadata_never_becomes_a_zero_delay() {
        for column in ["kind", "scheduled_for", "created_at"] {
            for value in [
                Value::Null,
                Value::Integer(1),
                Value::Real(1.5),
                Value::Blob(b"2026-10-18T00:00:00Z".to_vec()),
                Value::Blob(vec![255]),
                Value::Text("PRIVATE-PLANNING".into()),
                Value::Text("x".repeat(1_048_576)),
                Value::Text("2026-10-18T00:00:00Z\0PRIVATE-PLANNING".into()),
            ] {
                let conn = fixture();
                conn.execute_batch(
                    "INSERT INTO automation_occurrences VALUES
                    ('scheduled','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z')",
                )
                .unwrap();
                conn.execute(
                    &format!("UPDATE automation_occurrences SET {column}=?1"),
                    [value],
                )
                .unwrap();
                let before = conn.total_changes();
                assert_eq!(
                    inspect(&conn).unwrap(),
                    Measurement::InvalidMetadata,
                    "column {column}"
                );
                assert_eq!(conn.total_changes(), before);
            }
        }
    }

    #[test]
    fn exact_kind_is_required_even_with_nocase_collation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE automation_occurrences(kind TEXT COLLATE NOCASE, scheduled_for, created_at);
            INSERT INTO automation_occurrences VALUES ('SCHEDULED','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z')").unwrap();
        assert_eq!(inspect(&conn).unwrap(), Measurement::InvalidMetadata);
    }

    #[test]
    fn the_row_cap_never_returns_a_partial_maximum() {
        let conn = fixture();
        conn.execute_batch("WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM rows WHERE n<256)
            INSERT INTO automation_occurrences SELECT 'scheduled','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z' FROM rows").unwrap();
        assert_eq!(
            inspect(&conn).unwrap(),
            Measurement::Complete {
                checked: 256,
                maximum: 1000
            }
        );
        conn.execute_batch("INSERT INTO automation_occurrences VALUES ('manual',NULL,NULL)")
            .unwrap();
        assert_eq!(inspect(&conn).unwrap(), Measurement::BudgetExceeded);
    }

    #[test]
    fn unavailable_main_metadata_cannot_be_supplied_by_temp() {
        for omitted in [
            None,
            Some("kind"),
            Some("scheduled_for"),
            Some("created_at"),
        ] {
            let conn = Connection::open_in_memory().unwrap();
            if let Some(omitted) = omitted {
                let columns = ["kind", "scheduled_for", "created_at"]
                    .into_iter()
                    .filter(|column| *column != omitted)
                    .collect::<Vec<_>>()
                    .join(",");
                conn.execute_batch(&format!("CREATE TABLE automation_occurrences ({columns})"))
                    .unwrap();
            }
            conn.execute_batch("CREATE TEMP TABLE automation_occurrences(kind,scheduled_for,created_at);
                INSERT INTO temp.automation_occurrences VALUES ('scheduled','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z')").unwrap();
            assert_eq!(inspect(&conn).unwrap(), Measurement::Unavailable);
        }
    }

    #[test]
    fn empty_and_manual_only_cohorts_are_measured_empty() {
        let conn = fixture();
        assert_eq!(
            inspect(&conn).unwrap(),
            Measurement::Complete {
                checked: 0,
                maximum: 0
            }
        );
        conn.execute_batch("INSERT INTO automation_occurrences VALUES ('manual',NULL,NULL);
            CREATE TEMP TABLE automation_occurrences(kind,scheduled_for,created_at);
            INSERT INTO temp.automation_occurrences VALUES ('scheduled','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z')").unwrap();
        assert_eq!(
            inspect(&conn).unwrap(),
            Measurement::Complete {
                checked: 0,
                maximum: 0
            }
        );
    }

    #[test]
    fn instant_requires_safe_epoch_and_bounded_valid_text() {
        for value in [
            "1969-12-31T23:59:59Z",
            "2026-10-18T00:00:00Z\0",
            "PRIVATE",
            "",
        ] {
            assert!(instant(Some(value.as_bytes().to_vec())).is_none());
        }
        assert!(instant(Some(b"1970-01-01T00:00:00Z".to_vec())).is_some());
    }
}
