//! Stop ownership with a lease: run stop fences, and owner cancellations that
//! are still pending. Each is counted, and partitioned by whether its lease
//! has expired.
use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;

use super::super::leases::{classify_expiry, Expiry};
use super::{Fact, Gap, Observation, MAX_SAFE};

pub(super) struct Partition {
    table: &'static str,
    columns: &'static [&'static str],
    /// Which main rows hold stop ownership. Fixed SQL, never caller input.
    selection: &'static str,
    facts: [Fact; 4],
    gap: Gap,
}

/// One row per run whose stop a cancellation, a timeout or a recovery owns.
pub(super) const STOP_FENCES: Partition = Partition {
    table: "automation_stop_fences",
    columns: &["execution_expires_at"],
    selection: "1",
    facts: [
        Fact::StopFences,
        Fact::ActiveStopFences,
        Fact::ExpiredStopFences,
        Fact::InvalidStopFenceExpiries,
    ],
    gap: Gap::StopFenceStoreUnavailable,
};

/// The owner cancellations the expiry reconciler still has to settle. Their
/// state is compared byte for byte, as the reconciler's column comparison is.
pub(super) const PENDING_CANCELLATIONS: Partition = Partition {
    table: "automation_cancellations",
    columns: &["state", "execution_expires_at"],
    selection: "state COLLATE BINARY IN ('requested', 'stopping')",
    facts: [
        Fact::PendingCancellations,
        Fact::ActivePendingCancellations,
        Fact::ExpiredPendingCancellations,
        Fact::InvalidPendingCancellationExpiries,
    ],
    gap: Gap::CancellationStoreUnavailable,
};

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

impl Partition {
    /// The caller owns the read snapshot. Only each row's bounded lease
    /// timestamp is selected: no run, session, owner, key or reason.
    pub(super) fn inspect(&self, conn: &Connection, now: DateTime<Utc>) -> Result<Measurement> {
        if !super::projection_available(conn, self.table, self.columns)? {
            return Ok(Measurement::Unavailable);
        }
        // The scheduler reads the lease as text, so any other storage class is
        // as unreadable to it as a malformed timestamp.
        let mut statement = conn.prepare(&format!(
            "SELECT CASE WHEN typeof(execution_expires_at) = 'text'
                    THEN substr(CAST(execution_expires_at AS BLOB), 1, 65) END
             FROM main.{} WHERE {}",
            self.table, self.selection
        ))?;
        let mut rows = statement.query([])?;
        let (mut total, mut active, mut expired, mut invalid) = (0, 0, 0, 0);
        while let Some(row) = rows.next()? {
            ensure!(total < MAX_SAFE, "{} count exceeds safe range", self.table);
            total += 1;
            let value: Option<Vec<u8>> = row.get(0)?;
            // The lease is NOT NULL text; a missing one is as unreadable as a
            // malformed one. An expiry equal to the observation instant has
            // expired.
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

    pub(super) fn append(&self, observation: &mut Observation, measurement: Measurement) {
        match measurement {
            Measurement::Complete {
                total,
                active,
                expired,
                invalid,
            } => observation.facts.extend(
                self.facts
                    .into_iter()
                    .zip([total, active, expired, invalid]),
            ),
            Measurement::Unavailable => observation.gaps.push(self.gap),
        }
    }

    /// Strips in the reverse of append order: the later partition's gap, when
    /// present, is last.
    pub(super) fn validate_and_strip(&self, observation: &mut Observation) -> Result<()> {
        if observation.gaps.last() == Some(&self.gap) {
            ensure!(
                self.facts
                    .iter()
                    .all(|fact| !observation.facts.contains_key(fact)),
                "unavailable {} carry facts",
                self.table
            );
            observation.gaps.pop();
            return Ok(());
        }
        let [Some(total), Some(active), Some(expired), Some(invalid)] =
            self.facts.map(|fact| observation.facts.remove(&fact))
        else {
            anyhow::bail!("missing {} facts", self.table);
        };
        ensure!(
            total <= MAX_SAFE
                && active
                    .checked_add(expired)
                    .and_then(|sum| sum.checked_add(invalid))
                    == Some(total),
            "invalid {} expiry partition",
            self.table
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rusqlite::types::Value;

    const PARTITIONS: [&Partition; 2] = [&STOP_FENCES, &PENDING_CANCELLATIONS];

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 22, 9, 0, 0).unwrap()
    }

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_stop_fences(
                run_id, session_id, owner, operation_key, acquired_at, execution_expires_at);
             CREATE TABLE automation_cancellations(
                adoption_key, run_id, session_id, reason, state, execution_expires_at);",
        )
        .unwrap();
        conn
    }

    /// One row of stop ownership: a fence, or a cancellation in `state`.
    fn own(conn: &Connection, partition: &Partition, run: &str, state: &str, expires: Value) {
        if partition.table == STOP_FENCES.table {
            conn.execute(
                "INSERT INTO automation_stop_fences VALUES
                 (?1, 'PRIVATE-SESSION', 'timeout', 'PRIVATE-KEY', '2026-10-22T08:59:00Z', ?2)",
                rusqlite::params![run, expires],
            )
        } else {
            conn.execute(
                "INSERT INTO automation_cancellations VALUES
                 ('PRIVATE-KEY-' || ?1, ?1, 'PRIVATE-SESSION', 'PRIVATE-REASON', ?2, ?3)",
                rusqlite::params![run, state, expires],
            )
        }
        .unwrap();
    }

    fn lease(value: &str) -> Value {
        Value::Text(value.into())
    }

    fn complete(total: u64, active: u64, expired: u64, invalid: u64) -> Measurement {
        Measurement::Complete {
            total,
            active,
            expired,
            invalid,
        }
    }

    #[test]
    fn leases_partition_at_the_observation_instant() {
        for partition in PARTITIONS {
            let conn = fixture();
            own(
                &conn,
                partition,
                "a",
                "requested",
                lease("2026-10-22T09:00:30Z"),
            );
            // An offset is compared as an instant: 10:00:30+01:00 is 09:00:30Z.
            own(
                &conn,
                partition,
                "b",
                "stopping",
                lease("2026-10-22T10:00:30+01:00"),
            );
            own(
                &conn,
                partition,
                "c",
                "requested",
                lease("2026-10-22T09:00:00Z"),
            );
            own(
                &conn,
                partition,
                "d",
                "stopping",
                lease("2026-10-22T08:59:30.000Z"),
            );
            assert_eq!(
                partition.inspect(&conn, now()).unwrap(),
                complete(4, 2, 2, 0),
                "{}",
                partition.table
            );
        }
    }

    #[test]
    fn only_requested_and_stopping_cancellations_are_pending() {
        for (state, pending) in [
            ("requested", 1),
            ("stopping", 1),
            ("cancelled", 0),
            ("rejected", 0),
            ("recovery_required", 0),
            ("REQUESTED", 0),
            ("stopping ", 0),
        ] {
            let conn = fixture();
            own(
                &conn,
                &PENDING_CANCELLATIONS,
                "a",
                state,
                lease("2026-10-22T08:00:00Z"),
            );
            assert_eq!(
                PENDING_CANCELLATIONS.inspect(&conn, now()).unwrap(),
                complete(pending, 0, pending, 0),
                "{state}"
            );
        }
        // A permissive column collation cannot widen the selection.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_cancellations(state TEXT COLLATE NOCASE, execution_expires_at);
             INSERT INTO automation_cancellations VALUES ('REQUESTED', '2026-10-22T08:00:00Z');",
        )
        .unwrap();
        assert_eq!(
            PENDING_CANCELLATIONS.inspect(&conn, now()).unwrap(),
            complete(0, 0, 0, 0)
        );
    }

    #[test]
    fn unreadable_leases_are_counted_apart_and_never_echoed() {
        for partition in PARTITIONS {
            for value in [
                Value::Null,
                Value::Integer(1),
                Value::Real(1.5),
                // A valid timestamp is still unreadable stored as a blob.
                Value::Blob(b"2026-10-22T09:00:30Z".to_vec()),
                lease("PRIVATE LEASE"),
                lease("2026-10-22T09:00:30Z\0"),
                // 65 bytes of an otherwise valid timestamp.
                Value::Text(format!("2026-10-22T09:00:30.{}Z", "0".repeat(44))),
            ] {
                let conn = fixture();
                own(&conn, partition, "a", "requested", value.clone());
                own(
                    &conn,
                    partition,
                    "b",
                    "stopping",
                    lease("2026-10-22T09:00:30Z"),
                );
                assert_eq!(
                    partition.inspect(&conn, now()).unwrap(),
                    complete(2, 1, 0, 1),
                    "{} {value:?}",
                    partition.table
                );
            }
        }
    }

    #[test]
    fn a_lease_of_exactly_sixty_four_bytes_is_read() {
        for partition in PARTITIONS {
            let conn = fixture();
            let value = format!("2026-10-22T09:00:30.{}Z", "0".repeat(43));
            assert_eq!(value.len(), 64);
            own(&conn, partition, "a", "requested", Value::Text(value));
            assert_eq!(
                partition.inspect(&conn, now()).unwrap(),
                complete(1, 1, 0, 0),
                "{}",
                partition.table
            );
        }
    }

    #[test]
    fn main_dependencies_cannot_be_replaced_by_temp() {
        for partition in PARTITIONS {
            let columns = partition.columns.join(", ");
            // A view in the table's place, and a table missing each column.
            let mut creates = vec![
                None,
                Some(format!(
                    "CREATE VIEW {} AS SELECT {}",
                    partition.table,
                    partition
                        .columns
                        .iter()
                        .map(|column| format!("'requested' AS {column}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            ];
            for omitted in partition.columns {
                let kept = partition
                    .columns
                    .iter()
                    .filter(|column| column != &omitted)
                    .copied()
                    .chain(["run_id"])
                    .collect::<Vec<_>>()
                    .join(", ");
                creates.push(Some(format!("CREATE TABLE {}({kept})", partition.table)));
            }
            for create in creates {
                let conn = Connection::open_in_memory().unwrap();
                if let Some(create) = &create {
                    conn.execute_batch(create).unwrap();
                }
                conn.execute_batch(&format!(
                    "CREATE TEMP TABLE {table}({columns});
                     INSERT INTO temp.{table}({columns}) VALUES ({values})",
                    table = partition.table,
                    values = partition
                        .columns
                        .iter()
                        .map(|column| if *column == "state" {
                            "'requested'"
                        } else {
                            "'2026-10-22T09:00:30Z'"
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .unwrap();
                assert_eq!(
                    partition.inspect(&conn, now()).unwrap(),
                    Measurement::Unavailable,
                    "{} {create:?}",
                    partition.table
                );
            }
            // A TEMP shadow neither adds rows to a main table nor hides its own.
            let conn = fixture();
            own(
                &conn,
                partition,
                "a",
                "stopping",
                lease("2026-10-22T08:00:00Z"),
            );
            conn.execute_batch(&format!(
                "CREATE TEMP TABLE {table}(state, execution_expires_at);
                 INSERT INTO temp.{table} VALUES ('requested', '2026-10-22T09:00:30Z')",
                table = partition.table
            ))
            .unwrap();
            assert_eq!(
                partition.inspect(&conn, now()).unwrap(),
                complete(1, 0, 1, 0),
                "{}",
                partition.table
            );
        }
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
        for partition in PARTITIONS {
            let [total, active, expired, invalid] = partition.facts;
            let whole = [(total, 3), (active, 1), (expired, 1), (invalid, 1)];
            let mut accepted = observation(&whole, vec![Gap::FenceConsistencyNotMeasured]);
            partition.validate_and_strip(&mut accepted).unwrap();
            assert!(accepted.facts.is_empty());
            assert_eq!(accepted.gaps, [Gap::FenceConsistencyNotMeasured]);
            let mut unavailable =
                observation(&[], vec![Gap::FenceConsistencyNotMeasured, partition.gap]);
            partition.validate_and_strip(&mut unavailable).unwrap();
            assert_eq!(unavailable.gaps, [Gap::FenceConsistencyNotMeasured]);

            for (facts, gaps) in [
                // A count the partition does not add up to.
                (
                    vec![(total, 2), (active, 1), (expired, 0), (invalid, 0)],
                    vec![],
                ),
                // A missing fact.
                (whole[..3].to_vec(), vec![]),
                // Facts beside the gap that says they were not read.
                (whole.to_vec(), vec![partition.gap]),
                (vec![(total, 0)], vec![partition.gap]),
                // The gap anywhere but last.
                (
                    vec![],
                    vec![partition.gap, Gap::FenceConsistencyNotMeasured],
                ),
                // A whole partition beyond the safe-integer range.
                (
                    vec![
                        (total, MAX_SAFE + 1),
                        (active, MAX_SAFE + 1),
                        (expired, 0),
                        (invalid, 0),
                    ],
                    vec![],
                ),
                // An overflowing partition.
                (
                    vec![(total, 1), (active, u64::MAX), (expired, 2), (invalid, 0)],
                    vec![],
                ),
            ] {
                let mut forged = observation(&facts, gaps.clone());
                assert!(
                    partition.validate_and_strip(&mut forged).is_err(),
                    "{} {facts:?} {gaps:?}",
                    partition.table
                );
            }
        }
    }
}
