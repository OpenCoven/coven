//! Run stop fences: how many are held, and whether their leases have expired.
use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;

use super::super::leases::{classify_expiry, Expiry};
use super::{Fact, Gap, Observation, MAX_SAFE};

const FACTS: [Fact; 4] = [
    Fact::StopFences,
    Fact::ActiveStopFences,
    Fact::ExpiredStopFences,
    Fact::InvalidStopFenceExpiries,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Measurement {
    Complete {
        total: u64,
        active: u64,
        expired: u64,
        invalid: u64,
    },
    Unavailable,
}

/// The caller owns the read snapshot. Only each fence's bounded lease
/// timestamp is selected: no run, session, owner or operation key.
pub(super) fn inspect(conn: &Connection, now: DateTime<Utc>) -> Result<Measurement> {
    if !super::projection_available(conn, "automation_stop_fences", &["execution_expires_at"])? {
        return Ok(Measurement::Unavailable);
    }
    // The scheduler reads the lease as text, so any other storage class is as
    // unreadable to it as a malformed timestamp.
    let mut statement = conn.prepare(
        "SELECT CASE WHEN typeof(execution_expires_at) = 'text'
                THEN substr(CAST(execution_expires_at AS BLOB), 1, 65) END
         FROM main.automation_stop_fences",
    )?;
    let mut rows = statement.query([])?;
    let (mut total, mut active, mut expired, mut invalid) = (0, 0, 0, 0);
    while let Some(row) = rows.next()? {
        ensure!(total < MAX_SAFE, "stop fence count exceeds safe range");
        total += 1;
        let value: Option<Vec<u8>> = row.get(0)?;
        // The lease is NOT NULL text; a missing one is as unreadable as a
        // malformed one. An expiry equal to the observation instant has expired.
        match classify_expiry(value.as_deref(), now).1 {
            Expiry::Active => active += 1,
            Expiry::Expired => expired += 1,
            Expiry::Missing | Expiry::Invalid => invalid += 1,
        }
    }
    Ok(Measurement::Complete {
        total,
        active,
        expired,
        invalid,
    })
}

pub(super) fn append(observation: &mut Observation, measurement: Measurement) {
    match measurement {
        Measurement::Complete {
            total,
            active,
            expired,
            invalid,
        } => observation
            .facts
            .extend(FACTS.into_iter().zip([total, active, expired, invalid])),
        Measurement::Unavailable => observation.gaps.push(Gap::StopFenceStoreUnavailable),
    }
}

pub(super) fn validate_and_strip(observation: &mut Observation) -> Result<()> {
    if observation.gaps.last() == Some(&Gap::StopFenceStoreUnavailable) {
        ensure!(
            FACTS
                .iter()
                .all(|fact| !observation.facts.contains_key(fact)),
            "unavailable stop fences carry facts"
        );
        observation.gaps.pop();
        return Ok(());
    }
    let [Some(total), Some(active), Some(expired), Some(invalid)] =
        FACTS.map(|fact| observation.facts.remove(&fact))
    else {
        anyhow::bail!("missing stop-fence facts");
    };
    ensure!(
        total <= MAX_SAFE
            && active
                .checked_add(expired)
                .and_then(|sum| sum.checked_add(invalid))
                == Some(total),
        "invalid stop-fence expiry partition"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rusqlite::types::Value;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 22, 9, 0, 0).unwrap()
    }

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_stop_fences(
                run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)",
        )
        .unwrap();
        conn
    }

    fn fence(conn: &Connection, run: &str, expires: Value) {
        conn.execute(
            "INSERT INTO automation_stop_fences VALUES
             (?1, 'PRIVATE-SESSION', 'timeout', 'PRIVATE-KEY', '2026-10-22T08:59:00Z', ?2)",
            rusqlite::params![run, expires],
        )
        .unwrap();
    }

    #[test]
    fn leases_partition_at_the_observation_instant() {
        let conn = fixture();
        fence(&conn, "a", Value::Text("2026-10-22T09:00:30Z".into()));
        // An offset is compared as an instant: 10:00:30+01:00 is 09:00:30Z.
        fence(&conn, "b", Value::Text("2026-10-22T10:00:30+01:00".into()));
        fence(&conn, "c", Value::Text("2026-10-22T09:00:00Z".into()));
        fence(&conn, "d", Value::Text("2026-10-22T08:59:30.000Z".into()));
        assert_eq!(
            inspect(&conn, now()).unwrap(),
            Measurement::Complete {
                total: 4,
                active: 2,
                expired: 2,
                invalid: 0
            }
        );
    }

    #[test]
    fn unreadable_leases_are_counted_apart_and_never_echoed() {
        for value in [
            Value::Null,
            Value::Integer(1),
            Value::Real(1.5),
            // A valid timestamp is still unreadable stored as a blob.
            Value::Blob(b"2026-10-22T09:00:30Z".to_vec()),
            Value::Text("PRIVATE LEASE".into()),
            Value::Text("2026-10-22T09:00:30Z\0".into()),
            // 65 bytes of an otherwise valid timestamp.
            Value::Text(format!("2026-10-22T09:00:30.{}Z", "0".repeat(44))),
        ] {
            let conn = fixture();
            fence(&conn, "a", value.clone());
            fence(&conn, "b", Value::Text("2026-10-22T09:00:30Z".into()));
            assert_eq!(
                inspect(&conn, now()).unwrap(),
                Measurement::Complete {
                    total: 2,
                    active: 1,
                    expired: 0,
                    invalid: 1
                },
                "{value:?}"
            );
        }
    }

    #[test]
    fn a_lease_of_exactly_sixty_four_bytes_is_read() {
        let conn = fixture();
        let lease = format!("2026-10-22T09:00:30.{}Z", "0".repeat(43));
        assert_eq!(lease.len(), 64);
        fence(&conn, "a", Value::Text(lease));
        assert_eq!(
            inspect(&conn, now()).unwrap(),
            Measurement::Complete {
                total: 1,
                active: 1,
                expired: 0,
                invalid: 0
            }
        );
    }

    #[test]
    fn main_dependencies_cannot_be_replaced_by_temp() {
        for create in [
            None,
            Some("CREATE TABLE automation_stop_fences(run_id, owner)"),
            Some("CREATE VIEW automation_stop_fences AS SELECT 1 AS execution_expires_at"),
        ] {
            let conn = Connection::open_in_memory().unwrap();
            if let Some(create) = create {
                conn.execute_batch(create).unwrap();
            }
            conn.execute_batch(
                "CREATE TEMP TABLE automation_stop_fences(execution_expires_at);
                 INSERT INTO temp.automation_stop_fences VALUES ('2026-10-22T09:00:30Z')",
            )
            .unwrap();
            assert_eq!(
                inspect(&conn, now()).unwrap(),
                Measurement::Unavailable,
                "{create:?}"
            );
        }
        // A TEMP shadow neither adds rows to a main table nor hides its own.
        let conn = fixture();
        fence(&conn, "a", Value::Text("2026-10-22T08:00:00Z".into()));
        conn.execute_batch(
            "CREATE TEMP TABLE automation_stop_fences(execution_expires_at);
             INSERT INTO temp.automation_stop_fences VALUES ('2026-10-22T09:00:30Z')",
        )
        .unwrap();
        assert_eq!(
            inspect(&conn, now()).unwrap(),
            Measurement::Complete {
                total: 1,
                active: 0,
                expired: 1,
                invalid: 0
            }
        );
    }

    fn observation(facts: &[(Fact, u64)], gaps: Vec<Gap>) -> Observation {
        Observation {
            category: super::super::Category::LeasesFences,
            facts: facts.iter().copied().collect(),
            gaps,
            next_step: super::super::NextStep::InspectLeaseMetadata,
        }
    }

    #[test]
    fn validation_requires_a_whole_partition_or_the_gap_alone() {
        let whole = [
            (Fact::StopFences, 3),
            (Fact::ActiveStopFences, 1),
            (Fact::ExpiredStopFences, 1),
            (Fact::InvalidStopFenceExpiries, 1),
        ];
        let mut accepted = observation(&whole, vec![Gap::FenceConsistencyNotMeasured]);
        validate_and_strip(&mut accepted).unwrap();
        assert!(accepted.facts.is_empty());
        assert_eq!(accepted.gaps, [Gap::FenceConsistencyNotMeasured]);
        let mut unavailable = observation(
            &[],
            vec![
                Gap::FenceConsistencyNotMeasured,
                Gap::StopFenceStoreUnavailable,
            ],
        );
        validate_and_strip(&mut unavailable).unwrap();
        assert_eq!(unavailable.gaps, [Gap::FenceConsistencyNotMeasured]);

        for (facts, gaps) in [
            // A count the partition does not add up to.
            (
                vec![
                    (Fact::StopFences, 2),
                    (Fact::ActiveStopFences, 1),
                    (Fact::ExpiredStopFences, 0),
                    (Fact::InvalidStopFenceExpiries, 0),
                ],
                vec![],
            ),
            // A missing fact.
            (whole[..3].to_vec(), vec![]),
            // Facts beside the gap that says they were not read.
            (whole.to_vec(), vec![Gap::StopFenceStoreUnavailable]),
            (
                vec![(Fact::StopFences, 0)],
                vec![Gap::StopFenceStoreUnavailable],
            ),
            // The gap anywhere but last.
            (
                vec![],
                vec![
                    Gap::StopFenceStoreUnavailable,
                    Gap::FenceConsistencyNotMeasured,
                ],
            ),
            // A whole partition beyond the safe-integer range.
            (
                vec![
                    (Fact::StopFences, MAX_SAFE + 1),
                    (Fact::ActiveStopFences, MAX_SAFE + 1),
                    (Fact::ExpiredStopFences, 0),
                    (Fact::InvalidStopFenceExpiries, 0),
                ],
                vec![],
            ),
            // An overflowing partition.
            (
                vec![
                    (Fact::StopFences, 1),
                    (Fact::ActiveStopFences, u64::MAX),
                    (Fact::ExpiredStopFences, 2),
                    (Fact::InvalidStopFenceExpiries, 0),
                ],
                vec![],
            ),
        ] {
            let mut forged = observation(&facts, gaps.clone());
            assert!(
                validate_and_strip(&mut forged).is_err(),
                "{facts:?} {gaps:?}"
            );
        }
    }
}
