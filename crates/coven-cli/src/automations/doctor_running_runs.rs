//! Running runs that settlement has not finished: ended sessions, and a
//! latest attempt that failed or whose stop outcome is unknown.
use anyhow::{ensure, Result};
use rusqlite::Connection;

use super::super::runner::FINISHED_SESSION_STATUSES;
use super::{Category, Fact, Gap, Observation, MAX_SAFE};

const FACTS: [Fact; 4] = [
    Fact::RunningRuns,
    Fact::RunningRunsWithEndedSessions,
    Fact::RunningRunsWithFailedLatestAttempt,
    Fact::RunningRunsWithAmbiguousLatestAttempt,
];

pub(super) const PREREQUISITES: [(&str, &[&str]); 3] = [
    (
        "automation_runs",
        &["id", "status", "session_id", "authority_profile"],
    ),
    ("sessions", &["id", "status"]),
    (
        "automation_attempts",
        &["run_id", "attempt_number", "state"],
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Measurement {
    Complete {
        running: u64,
        ended: u64,
        failed: u64,
        ambiguous: u64,
    },
    Unavailable,
}

/// The caller owns the read snapshot. These are the predicates settlement
/// (`runner::settle_finished_runs_with`) selects runs by: a session in one of
/// the statuses it treats as finished, and a plain run whose latest attempt
/// failed. Settlement compares both in Rust, exactly, so they are compared
/// here byte for byte. A failed latest attempt is stored state, not a promise
/// that `attempt.retry.v1` will accept a retry. An ambiguous latest attempt,
/// whose stop outcome is unknown, is counted for every run, plain or with an
/// authority profile, since each is resolved differently. Only counts leave.
pub(super) fn inspect(conn: &Connection) -> Result<Measurement> {
    for (table, columns) in PREREQUISITES {
        if !super::projection_available(conn, table, columns)? {
            return Ok(Measurement::Unavailable);
        }
    }
    let finished = FINISHED_SESSION_STATUSES
        .map(|status| format!("'{status}'"))
        .join(", ");
    let counts: [i64; 4] = conn.query_row(
        &format!(
            "SELECT COUNT(*),
                    COALESCE(SUM(s.status COLLATE BINARY IN ({finished})), 0),
                    COALESCE(SUM(r.authority_profile IS NULL
                                 AND r.latest COLLATE BINARY = 'failed'), 0),
                    COALESCE(SUM(r.latest COLLATE BINARY = 'ambiguous'), 0)
             FROM (
                 SELECT run.session_id, run.authority_profile, (
                     SELECT a.state FROM main.automation_attempts AS a
                     WHERE a.run_id = run.id
                     ORDER BY a.attempt_number DESC
                     LIMIT 1
                 ) AS latest
                 FROM main.automation_runs AS run
                 WHERE run.status = 'running'
             ) AS r
             LEFT JOIN main.sessions AS s ON s.id = r.session_id"
        ),
        [],
        |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?]),
    )?;
    let [Ok(running), Ok(ended), Ok(failed), Ok(ambiguous)] = counts.map(u64::try_from) else {
        anyhow::bail!("running run count is negative");
    };
    ensure!(running <= MAX_SAFE, "running run count exceeds safe range");
    Ok(Measurement::Complete {
        running,
        ended,
        failed,
        ambiguous,
    })
}

pub(super) fn append(observation: &mut Observation, measurement: Measurement) {
    match measurement {
        Measurement::Complete {
            running,
            ended,
            failed,
            ambiguous,
        } => observation
            .facts
            .extend(FACTS.into_iter().zip([running, ended, failed, ambiguous])),
        Measurement::Unavailable => observation.gaps.push(Gap::RunningRunStoreUnavailable),
    }
}

pub(super) fn validate_and_strip(observation: &mut Observation) -> Result<()> {
    if observation.gaps.last() == Some(&Gap::RunningRunStoreUnavailable) {
        ensure!(
            FACTS
                .iter()
                .all(|fact| !observation.facts.contains_key(fact)),
            "unavailable running runs carry facts"
        );
        observation.gaps.pop();
        return Ok(());
    }
    let [Some(running), Some(ended), Some(failed), Some(ambiguous)] =
        FACTS.map(|fact| observation.facts.remove(&fact))
    else {
        anyhow::bail!("missing running-run facts");
    };
    // A run whose attempt failed or went ambiguous has usually ended its
    // session too, so those subsets overlap the ended one. A latest attempt
    // has one state, so failed and ambiguous are disjoint.
    ensure!(
        running <= MAX_SAFE
            && ended <= running
            && failed
                .checked_add(ambiguous)
                .is_some_and(|sum| sum <= running),
        "invalid running-run subsets"
    );
    Ok(())
}

/// Each run with a failed latest attempt contributes that attempt to
/// `failed_attempts`, read in the same snapshot with the same comparison.
pub(super) fn validate_cohort(observations: &[Observation]) -> Result<()> {
    let fact = |category, fact| {
        observations
            .iter()
            .find(|observation| observation.category == category)
            .and_then(|observation| observation.facts.get(&fact).copied())
    };
    if let (Some(failed_latest), Some(failed_attempts)) = (
        fact(Category::Queue, Fact::RunningRunsWithFailedLatestAttempt),
        fact(Category::RepeatedFailures, Fact::FailedAttempts),
    ) {
        ensure!(
            failed_latest <= failed_attempts,
            "running runs contradict failed attempts"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_runs(id, status, session_id, authority_profile);
             CREATE TABLE sessions(id, status);
             CREATE TABLE automation_attempts(run_id, attempt_number, state);",
        )
        .unwrap();
        conn
    }

    fn run(
        conn: &Connection,
        id: &str,
        status: &str,
        session: Option<&str>,
        profile: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO automation_runs VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, status, session, profile],
        )
        .unwrap();
    }

    fn complete(running: u64, ended: u64, failed: u64, ambiguous: u64) -> Measurement {
        Measurement::Complete {
            running,
            ended,
            failed,
            ambiguous,
        }
    }

    #[test]
    fn ended_sessions_use_settlements_terminal_statuses() {
        for (status, ended) in [
            ("completed", 1),
            ("failed", 1),
            ("cancelled", 1),
            ("killed", 1),
            ("idle", 1),
            ("running", 0),
            ("created", 0),
            // Settlement does not treat an orphaned session as finished.
            ("orphaned", 0),
            ("KILLED", 0),
        ] {
            let conn = fixture();
            run(
                &conn,
                "PRIVATE-RUN",
                "running",
                Some("PRIVATE-SESSION"),
                None,
            );
            conn.execute(
                "INSERT INTO sessions VALUES ('PRIVATE-SESSION', ?1)",
                [status],
            )
            .unwrap();
            assert_eq!(
                inspect(&conn).unwrap(),
                complete(1, ended, 0, 0),
                "{status}"
            );
        }
        // No session row, or none recorded, is not an ended session.
        let conn = fixture();
        run(&conn, "a", "running", Some("missing"), None);
        run(&conn, "b", "running", None, None);
        assert_eq!(inspect(&conn).unwrap(), complete(2, 0, 0, 0));
    }

    #[test]
    fn a_permissive_collation_cannot_widen_any_subset() {
        for state in ["FAILED", "AMBIGUOUS"] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(
                "CREATE TABLE automation_runs(id, status, session_id, authority_profile);
                 CREATE TABLE sessions(id, status TEXT COLLATE NOCASE);
                 CREATE TABLE automation_attempts(run_id, attempt_number, state TEXT COLLATE NOCASE);
                 INSERT INTO automation_runs VALUES ('r', 'running', 's', NULL);
                 INSERT INTO sessions VALUES ('s', 'KILLED');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO automation_attempts VALUES ('r', 1, ?1)",
                [state],
            )
            .unwrap();
            assert_eq!(inspect(&conn).unwrap(), complete(1, 0, 0, 0), "{state}");
        }
    }

    #[test]
    fn the_latest_attempt_alone_decides_failed_and_ambiguous() {
        // Each case is its own store, so no two can cancel out in a total.
        for (attempts, profile, status, failed, ambiguous) in [
            (&[(1, "failed")][..], None, "running", 1, 0),
            (&[(1, "failed"), (2, "started")], None, "running", 0, 0),
            (&[(2, "failed"), (1, "started")], None, "running", 1, 0),
            (&[(1, "started"), (2, "failed")], None, "running", 1, 0),
            // Settlement holds only a plain run's failure.
            (&[(1, "failed")], Some("PRIVATE-PROFILE"), "running", 0, 0),
            (&[(1, "ambiguous")], None, "running", 0, 1),
            // An unknown stop outcome is counted whatever the profile.
            (
                &[(1, "ambiguous")],
                Some("PRIVATE-PROFILE"),
                "running",
                0,
                1,
            ),
            (&[(2, "ambiguous"), (1, "failed")], None, "running", 0, 1),
            (&[(1, "ambiguous"), (2, "started")], None, "running", 0, 0),
            (&[], None, "running", 0, 0),
            (&[(1, "failed")], None, "failed", 0, 0),
            (&[(1, "ambiguous")], None, "failed", 0, 0),
        ] {
            let conn = fixture();
            run(&conn, "r", status, Some("s"), profile);
            conn.execute("INSERT INTO sessions VALUES ('s', 'failed')", [])
                .unwrap();
            for (number, state) in attempts {
                conn.execute(
                    "INSERT INTO automation_attempts VALUES ('r', ?1, ?2)",
                    rusqlite::params![number, state],
                )
                .unwrap();
            }
            let running = u64::from(status == "running");
            assert_eq!(
                inspect(&conn).unwrap(),
                complete(running, running, failed, ambiguous),
                "{attempts:?} {profile:?} {status}"
            );
        }
    }

    #[test]
    fn main_dependencies_cannot_be_replaced_by_temp() {
        for (table, columns) in PREREQUISITES {
            for column in columns.iter().copied().map(Some).chain([None]) {
                let conn = Connection::open_in_memory().unwrap();
                for (other, other_columns) in PREREQUISITES {
                    let kept = other_columns
                        .iter()
                        .filter(|c| other != table || Some(**c) != column)
                        .copied()
                        .collect::<Vec<_>>();
                    if other == table && column.is_none() {
                        conn.execute_batch(&format!(
                            "CREATE VIEW {other} AS SELECT {}",
                            other_columns
                                .iter()
                                .map(|c| format!("NULL AS {c}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .unwrap();
                    } else {
                        conn.execute_batch(&format!("CREATE TABLE {other}({})", kept.join(", ")))
                            .unwrap();
                    }
                    conn.execute_batch(&format!(
                        "CREATE TEMP TABLE {other}({})",
                        other_columns.join(", ")
                    ))
                    .unwrap();
                }
                assert_eq!(
                    inspect(&conn).unwrap(),
                    Measurement::Unavailable,
                    "{table} {column:?}"
                );
            }
        }
        // TEMP rows neither add to main tables nor hide theirs.
        let conn = fixture();
        run(&conn, "a", "running", Some("s"), None);
        conn.execute_batch(
            "INSERT INTO sessions VALUES ('s', 'running');
             CREATE TEMP TABLE automation_runs(id, status, session_id, authority_profile);
             CREATE TEMP TABLE sessions(id, status);
             CREATE TEMP TABLE automation_attempts(run_id, attempt_number, state);
             INSERT INTO temp.automation_runs VALUES ('b', 'running', 't', NULL);
             INSERT INTO temp.sessions VALUES ('s', 'killed'), ('t', 'killed');
             INSERT INTO temp.automation_attempts VALUES ('a', 1, 'ambiguous'), ('a', 2, 'failed');",
        )
        .unwrap();
        assert_eq!(inspect(&conn).unwrap(), complete(1, 0, 0, 0));
    }

    fn observation(facts: &[(Fact, u64)], gaps: Vec<Gap>) -> Observation {
        Observation {
            category: super::super::Category::Queue,
            facts: facts.iter().copied().collect(),
            gaps,
            next_step: super::super::NextStep::InspectSchedulerStatus,
        }
    }

    #[test]
    fn validation_requires_bounded_subsets_or_the_gap_alone() {
        let whole = [
            (Fact::RunningRuns, 3),
            (Fact::RunningRunsWithEndedSessions, 3),
            (Fact::RunningRunsWithFailedLatestAttempt, 2),
            (Fact::RunningRunsWithAmbiguousLatestAttempt, 1),
        ];
        let mut accepted = observation(&whole, vec![]);
        validate_and_strip(&mut accepted).unwrap();
        assert!(accepted.facts.is_empty());
        let mut unavailable = observation(
            &[],
            vec![Gap::InvalidOccurrenceState, Gap::RunningRunStoreUnavailable],
        );
        validate_and_strip(&mut unavailable).unwrap();
        assert_eq!(unavailable.gaps, [Gap::InvalidOccurrenceState]);

        for (facts, gaps) in [
            // A subset larger than the running total.
            (
                vec![
                    (Fact::RunningRuns, 1),
                    (Fact::RunningRunsWithEndedSessions, 2),
                    (Fact::RunningRunsWithFailedLatestAttempt, 0),
                    (Fact::RunningRunsWithAmbiguousLatestAttempt, 0),
                ],
                vec![],
            ),
            (
                vec![
                    (Fact::RunningRuns, 1),
                    (Fact::RunningRunsWithEndedSessions, 0),
                    (Fact::RunningRunsWithFailedLatestAttempt, 2),
                    (Fact::RunningRunsWithAmbiguousLatestAttempt, 0),
                ],
                vec![],
            ),
            (
                vec![
                    (Fact::RunningRuns, 1),
                    (Fact::RunningRunsWithEndedSessions, 0),
                    (Fact::RunningRunsWithFailedLatestAttempt, 0),
                    (Fact::RunningRunsWithAmbiguousLatestAttempt, 2),
                ],
                vec![],
            ),
            // One latest attempt cannot be both failed and ambiguous.
            (
                vec![
                    (Fact::RunningRuns, 1),
                    (Fact::RunningRunsWithEndedSessions, 1),
                    (Fact::RunningRunsWithFailedLatestAttempt, 1),
                    (Fact::RunningRunsWithAmbiguousLatestAttempt, 1),
                ],
                vec![],
            ),
            (
                vec![
                    (Fact::RunningRuns, 1),
                    (Fact::RunningRunsWithEndedSessions, 0),
                    (Fact::RunningRunsWithFailedLatestAttempt, u64::MAX),
                    (Fact::RunningRunsWithAmbiguousLatestAttempt, 2),
                ],
                vec![],
            ),
            // A total beyond the safe-integer range.
            (
                vec![
                    (Fact::RunningRuns, MAX_SAFE + 1),
                    (Fact::RunningRunsWithEndedSessions, 0),
                    (Fact::RunningRunsWithFailedLatestAttempt, 0),
                    (Fact::RunningRunsWithAmbiguousLatestAttempt, 0),
                ],
                vec![],
            ),
            // A missing fact.
            (whole[..3].to_vec(), vec![]),
            // Facts beside the gap that says they were not read.
            (whole.to_vec(), vec![Gap::RunningRunStoreUnavailable]),
            (
                vec![(Fact::RunningRunsWithFailedLatestAttempt, 0)],
                vec![Gap::RunningRunStoreUnavailable],
            ),
            // The gap anywhere but last.
            (
                vec![],
                vec![Gap::RunningRunStoreUnavailable, Gap::InvalidOccurrenceState],
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
