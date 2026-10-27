//! The gate's predicate and its commitments, against a dispatched trusted
//! attempt (proposal §4, tests 1 to 5).

use std::time::{Duration, Instant};

use chrono::{SecondsFormat, TimeDelta, Utc};

use super::*;
use crate::automations::scoped_read::service::Committing;
use crate::automations::trusted_launch::launch_scopes;
use crate::automations::trusted_launch::test_support::{r0, scope, Fixture, Launched};

const DIGEST: &str = "abababababababababababababababababababababababababababababababab";

/// A dispatched `notes` attempt and what its launch verified.
struct Dispatched {
    fixture: Fixture,
    run: String,
    launched: Launched,
    held: Held,
}

impl Dispatched {
    fn new() -> Self {
        let fixture = Fixture::new();
        fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
        let (run, launched) = fixture.dispatch("notes");
        let held = launch_scopes(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
            .expect("the dispatched attempt is authorized")
            .held;
        Self {
            fixture,
            run,
            launched,
            held,
        }
    }

    fn gate(&self) -> StoreGate {
        self.gate_for(self.held.clone())
    }

    fn gate_for(&self, held: Held) -> StoreGate {
        StoreGate::open(
            self.fixture.home.path(),
            held,
            Duration::from_millis(200),
            None,
        )
        .expect("the gate opens")
    }

    fn conn(&self) -> &Connection {
        &self.fixture.home.conn
    }

    /// Every commitment row: sequence, outcome, refusal and digest.
    fn rows(&self) -> Vec<(i64, String, Option<String>, Option<String>)> {
        let mut statement = self
            .conn()
            .prepare(
                "SELECT sequence, outcome, refusal, result_digest
                 FROM automation_scoped_read_commitments ORDER BY committed_at, sequence",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

fn served(sequence: u64) -> Committing<'static> {
    Committing {
        sequence,
        tool: Some("read_file"),
        refusal: None,
        digest: DIGEST,
        bytes: 42,
    }
}

fn withheld(sequence: i64) -> (i64, String, Option<String>, Option<String>) {
    (
        sequence,
        "withheld".to_owned(),
        Some("attempt_closed".to_owned()),
        None,
    )
}

/// A way to withdraw a running attempt's authority, and its name.
type Withdraw = (&'static str, fn(&Dispatched));

/// Each source that can withdraw a running attempt's authority.
fn withdrawals() -> Vec<Withdraw> {
    fn revoke(dispatched: &Dispatched, index: usize) {
        let key = &dispatched.held.key_ids[index];
        assert!(crate::automations::authority_keys::revoke_key(
            dispatched.conn(),
            dispatched.fixture.home.path(),
            key,
            "test",
            Utc::now(),
        )
        .unwrap());
    }
    vec![
        ("dispatch-authority key revoked", |d| revoke(d, 0)),
        ("threads-decision key revoked", |d| revoke(d, 1)),
        ("owner-principal key revoked", |d| revoke(d, 2)),
        ("familiar revision revoked", |d| {
            d.conn()
                .execute(
                    "UPDATE familiar_ledger_revisions
                     SET status = 'revoked', revoked_at = '2026-10-27T00:00:00.000Z',
                         revocation_reason = 'compromised'
                     WHERE revision_id = ?1",
                    [d.launched.projection.identity_revision_id.as_str()],
                )
                .unwrap();
        }),
        ("familiar revision retired", |d| {
            d.conn()
                .execute(
                    "UPDATE familiar_ledger_revisions SET status = 'retired'
                     WHERE revision_id = ?1",
                    [d.launched.projection.identity_revision_id.as_str()],
                )
                .unwrap();
        }),
        ("definition paused", |d| {
            d.conn()
                .execute(
                    "UPDATE automation_definitions SET lifecycle_state = 'paused'
                     WHERE id = 'notes'",
                    [],
                )
                .unwrap();
        }),
        ("stop fence claimed", |d| {
            crate::automations::runner::claim_stop_fence(
                d.conn(),
                &d.run,
                &d.launched.id,
                "timeout",
                None,
                None,
                Utc::now(),
            )
            .unwrap();
        }),
        ("run timed out", |d| {
            let past =
                (Utc::now() - TimeDelta::seconds(1)).to_rfc3339_opts(SecondsFormat::Millis, true);
            d.conn()
                .execute(
                    "UPDATE automation_runs SET timeout_at = ?2 WHERE id = ?1",
                    rusqlite::params![d.run, past],
                )
                .unwrap();
        }),
    ]
}

#[test]
fn a_revocation_that_commits_first_withholds_the_result() {
    for (what, withdraw) in withdrawals() {
        let dispatched = Dispatched::new();
        let mut gate = dispatched.gate();
        assert_eq!(gate.admit(), Ok(()), "{what}");
        withdraw(&dispatched);
        assert_eq!(
            gate.commit(&served(1)),
            Err(Refusal::AttemptClosed),
            "{what}"
        );
        assert_eq!(dispatched.rows(), [withheld(1)], "{what}");
        assert_eq!(gate.admit(), Err(Refusal::AttemptClosed), "{what}");
    }
}

#[test]
fn a_commitment_that_commits_first_stands_and_the_next_call_is_refused() {
    let dispatched = Dispatched::new();
    let mut gate = dispatched.gate();
    assert_eq!(gate.admit(), Ok(()));
    assert_eq!(gate.commit(&served(1)), Ok(()));
    let refused = Committing {
        refusal: Some(Refusal::ScopeMissing),
        ..served(2)
    };
    assert_eq!(gate.commit(&refused), Ok(()));
    withdrawals()[1].1(&dispatched);
    assert_eq!(gate.admit(), Err(Refusal::AttemptClosed));
    assert_eq!(gate.commit(&served(3)), Err(Refusal::AttemptClosed));
    assert_eq!(
        dispatched.rows(),
        [
            (1, "served".to_owned(), None, Some(DIGEST.to_owned())),
            (
                2,
                "refused".to_owned(),
                Some("scope_missing".to_owned()),
                Some(DIGEST.to_owned())
            ),
            withheld(3),
        ]
    );
}

#[test]
fn a_wrong_attempt_session_or_binding_is_refused_at_admit_and_commit() {
    let dispatched = Dispatched::new();
    let mut other_session = dispatched.held.clone();
    other_session.session_id = "session-elsewhere".to_owned();
    let mut other_attempt = dispatched.held.clone();
    other_attempt.attempt_id = "attempt-elsewhere".to_owned();
    for (what, held) in [("session", other_session), ("attempt", other_attempt)] {
        let mut gate = dispatched.gate_for(held);
        assert_eq!(gate.admit(), Err(Refusal::AttemptClosed), "{what}");
        assert_eq!(
            gate.commit(&served(1)),
            Err(Refusal::AttemptClosed),
            "{what}"
        );
    }
    assert!(
        !dispatched.rows().iter().any(|row| row.1 == "served"),
        "nothing is served under another attempt's authority"
    );

    // The attempt's pinned binding is rewritten after the launch verified it.
    let mut gate = dispatched.gate();
    let conn = dispatched.conn();
    conn.execute_batch("DROP TRIGGER automation_attempt_authority_immutable;")
        .unwrap();
    conn.execute(
        "UPDATE automation_attempts
         SET authority_extension_json = authority_extension_json || ' '
         WHERE id = ?1",
        [&dispatched.held.attempt_id],
    )
    .unwrap();
    assert_eq!(gate.admit(), Err(Refusal::AttemptClosed));
    assert_eq!(gate.commit(&served(1)), Err(Refusal::AttemptClosed));
    assert!(!dispatched.rows().iter().any(|row| row.1 == "served"));
}

#[test]
fn a_busy_store_withholds_the_result_unrecorded() {
    let dispatched = Dispatched::new();
    let mut gate = dispatched.gate();
    assert_eq!(gate.admit(), Ok(()));
    let other =
        crate::store::open_store(&crate::api::store_path(dispatched.fixture.home.path())).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    assert_eq!(gate.commit(&served(1)), Err(Refusal::AttemptClosed));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait is bounded"
    );
    other.execute_batch("ROLLBACK").unwrap();
    assert_eq!(dispatched.rows(), []);
}

#[test]
fn commitments_cannot_be_updated_or_deleted() {
    let dispatched = Dispatched::new();
    let mut gate = dispatched.gate();
    assert_eq!(gate.commit(&served(1)), Ok(()));
    let conn = dispatched.conn();
    let update = conn
        .execute(
            "UPDATE automation_scoped_read_commitments SET outcome = 'refused'",
            [],
        )
        .unwrap_err();
    assert!(update.to_string().contains("immutable"), "{update}");
    let delete = conn
        .execute("DELETE FROM automation_scoped_read_commitments", [])
        .unwrap_err();
    assert!(delete.to_string().contains("retained"), "{delete}");
    assert_eq!(dispatched.rows().len(), 1);
}
