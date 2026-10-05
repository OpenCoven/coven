//! The terminal observer (coven#857, slice 5).
//!
//! Decision 4 puts signed terminal observations in Coven's session executor,
//! which owns the session's process. When a session running an attempt under
//! Runtime Authority ends, the observer turns what the executor saw into
//! `coven.automations.runtime-terminal-evidence.v1`, signed with the daemon's
//! `terminal-observer` key, and stores it through the evidence inbox (#1193),
//! which verifies it against the attempt's pinned execution binding.
//!
//! It reports only what it observed, never what the binding granted. Today's
//! launches print plain text (`--print`, and codex without `--json`), so the
//! executor sees how the process ended but not which capabilities it
//! exercised, what effects they had, what result it produced or whether that
//! result was delivered. Each of those is `unknown`. Evidence with an unknown
//! member classifies as `authenticated_unknown`, which the evidence consumer
//! holds for recovery rather than settling. Structured tool events wait for the
//! Threads capability vocabulary (slice 4).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, SecondsFormat, TimeDelta, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};

use super::authority_keys::{
    self, AuthorityKeyRole, RoleSigningKey, PRODUCER_COMPONENT, PRODUCER_INSTANCE_ID,
};
use super::contract::authority::AutomationExecutionBinding;
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::runtime_terminal_evidence::{
    RuntimeTerminalEvidence, RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN,
    RUNTIME_TERMINAL_EVIDENCE_PROFILE,
};
use super::contract::types::TerminalOutcome;
use super::ed25519_trust::Ed25519TerminalEvidenceVerifier;
use super::runtime_terminal_evidence::{self, RuntimeTerminalEvidenceStoreOutcome};

/// Why a plain-text launch leaves capabilities and side effects unknown.
const UNSTRUCTURED_OUTPUT: &str = "unstructured_runtime_output";
/// The executor does not see the run's result artifact.
const RESULT_NOT_OBSERVED: &str = "result_not_observed";
/// Delivery happens outside the session, after it ends.
const DELIVERY_NOT_OBSERVED: &str = "delivery_not_observed";

/// What recording a session's ending did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Recorded {
    /// The session runs no Runtime Authority attempt, so there is nothing to
    /// observe.
    NotAuthority,
    Stored,
    /// The same evidence was already stored.
    Replayed,
    /// Nothing was stored, so the evidence consumer holds the run for
    /// recovery. The reason also goes to the daemon recovery log.
    Failed(String),
}

/// Records the signed observation of `session_id` ending as `disposition`.
/// The caller passes the transaction that records the ending, so the evidence
/// commits with it, before the ending is published. Only the writer that
/// actually ended the session calls this.
///
/// It never fails that transaction. Its own writes sit in a savepoint, and any
/// error rolls them back and stores nothing.
///
/// The key directory is found next to the store, because the daemon's store
/// is `COVEN_HOME/coven.sqlite3` and the key records live in it. The key must
/// already exist: dispatch under Runtime Authority creates every role key
/// before it launches, so a missing key means nothing should be trusted.
pub(crate) fn record(
    conn: &Connection,
    session_id: &str,
    disposition: TerminalOutcome,
    now: DateTime<Utc>,
) -> Recorded {
    let binding = match runtime_terminal_evidence::session_binding(conn, session_id) {
        Ok(None) => return Recorded::NotAuthority,
        Ok(Some(binding)) => binding,
        Err(error) => {
            return failed(
                conn,
                session_id,
                &format!("its binding is unusable: {error}"),
            )
        }
    };
    if let Err(error) = conn.execute_batch("SAVEPOINT terminal_observer") {
        return failed(conn, session_id, &format!("no savepoint: {error}"));
    }
    let stored = (|| -> Result<RuntimeTerminalEvidenceStoreOutcome> {
        let home = store_home(conn).context("the store has no directory")?;
        let key =
            authority_keys::existing_signing_key(conn, &home, AuthorityKeyRole::TerminalObserver)?
                .context("there is no terminal-observer key")?;
        let observation = TerminalObservation {
            session_id,
            disposition,
        };
        let evidence = observe(&key, &binding, &observation, now)?;
        let trusted = authority_keys::trusted_keys(conn, AuthorityKeyRole::TerminalObserver)?;
        runtime_terminal_evidence::store_runtime_terminal_evidence_in(
            conn,
            &evidence,
            &Ed25519TerminalEvidenceVerifier(&trusted),
        )
        .map_err(|error| anyhow::anyhow!("the evidence inbox refused it: {error}"))
    })();
    match stored {
        Ok(outcome) => match conn.execute_batch("RELEASE terminal_observer") {
            Ok(()) => match outcome {
                RuntimeTerminalEvidenceStoreOutcome::Stored => Recorded::Stored,
                RuntimeTerminalEvidenceStoreOutcome::Replayed => Recorded::Replayed,
            },
            Err(error) => failed(conn, session_id, &format!("release failed: {error}")),
        },
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK TO terminal_observer; RELEASE terminal_observer");
            failed(conn, session_id, &format!("{error:#}"))
        }
    }
}

fn failed(conn: &Connection, session_id: &str, reason: &str) -> Recorded {
    if let Some(home) = store_home(conn) {
        crate::daemon::append_daemon_recovery_log(
            &home,
            &format!(
                "terminal observer stored no evidence for session `{session_id}`, so its run is held: {reason}"
            ),
        );
    }
    Recorded::Failed(reason.to_owned())
}

/// The directory of the store `conn` writes, which is `COVEN_HOME`.
fn store_home(conn: &Connection) -> Option<PathBuf> {
    conn.path()
        .filter(|path| !path.is_empty())
        .and_then(|path| Path::new(path).parent())
        .map(Path::to_path_buf)
}

/// What the executor saw when the session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalObservation<'a> {
    pub session_id: &'a str,
    pub disposition: TerminalOutcome,
}

/// The signed evidence for `observation` of the attempt `binding` pins,
/// produced at `now`.
pub(crate) fn observe(
    key: &RoleSigningKey,
    binding: &AutomationExecutionBinding,
    observation: &TerminalObservation<'_>,
    now: DateTime<Utc>,
) -> Result<RuntimeTerminalEvidence> {
    let now = now
        .duration_trunc(TimeDelta::milliseconds(1))
        .context("observation time cannot be represented")?;
    let unknown = |reason: &str| json!({ "state": "unknown", "reasonCode": reason });
    let mut evidence = json!({
        "profile": RUNTIME_TERMINAL_EVIDENCE_PROFILE,
        // One observation per attempt: the inbox keys evidence by attempt.
        "evidenceId": format!("evidence:{}", binding.base.attempt_id.as_str()),
        "sessionId": observation.session_id,
        "runId": binding.base.run_id,
        "attemptId": binding.base.attempt_id,
        "binding": {
            "bindingId": binding.binding_id,
            "bindingDigest": binding.integrity,
        },
        "runtime": {
            "runtimeId": binding.runtime.runtime_id,
            "descriptorDigest": binding.runtime.descriptor_digest,
        },
        "producedAt": now.to_rfc3339_opts(SecondsFormat::Millis, true),
        "disposition": observation.disposition,
        "sideEffects": unknown(UNSTRUCTURED_OUTPUT),
        "exercisedCapabilities": unknown(UNSTRUCTURED_OUTPUT),
        "result": unknown(RESULT_NOT_OBSERVED),
        "delivery": unknown(DELIVERY_NOT_OBSERVED),
        "producer": {
            "component": PRODUCER_COMPONENT,
            "instanceId": PRODUCER_INSTANCE_ID,
            "implementationVersion": env!("CARGO_PKG_VERSION"),
        },
        "privacy": {
            "classification": "operational",
            "retention": { "classification": "standard" },
        },
    });
    let canonical = canonicalize(&evidence).context("terminal evidence is I-JSON")?;
    let mut preimage = RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN.to_vec();
    preimage.push(0);
    preimage.extend_from_slice(&canonical);
    let signed_digest = sha256_hex(&preimage);
    let signature = key.sign_digest(&digest_bytes(&signed_digest)?);
    evidence["integrity"] = digest_value(&sha256_hex(&canonical));
    evidence["authentication"] = json!({
        "method": "ed25519",
        "keyId": key.record().key_id,
        "proofRef": key.record().proof_ref,
        "signedDigest": digest_value(&signed_digest),
        "signature": signature,
    });
    serde_json::from_value(evidence).context("terminal evidence fits its contract")
}

fn digest_value(hex: &str) -> Value {
    json!({ "algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": hex })
}

fn digest_bytes(hex: &str) -> Result<[u8; 32]> {
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = hex
            .get(index * 2..index * 2 + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .context("a SHA-256 digest is 64 hex characters")?;
    }
    Ok(bytes)
}

/// A store holding one running session that runs a Runtime Authority attempt
/// pinned to the contract's fixture binding, as the evidence inbox's own
/// tests seed it.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;

    use chrono::{DateTime, TimeZone, Utc};
    use rusqlite::Connection;
    use serde_json::json;

    use crate::automations::authority_keys::{self, AuthorityKeyRole};
    use crate::automations::contract::authority::test_support::fixture;

    pub(crate) const SESSION_ID: &str = "session-daily-notes-1";
    pub(crate) const RUN_ID: &str = "run.daily-notes-1";

    /// When the fixture's session ends.
    pub(crate) fn ended_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 3, 12, 30, 0).unwrap()
    }

    /// Seeds the store under `home`. With `observer_key`, the daemon's
    /// terminal-observer key exists from before the run started.
    pub(crate) fn seed(home: &Path, observer_key: bool) -> Connection {
        let path = home.join(crate::STORE_FILE_NAME);
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_initialized_store(&path).unwrap();
        let extension = json!({
            "profile": "coven.automations.authority.v1",
            "kind": "AutomationAuthorityExtension",
            "executionBinding": fixture("binding"),
            "receiptEvidence": null
        });
        conn.execute_batch(
            "INSERT INTO sessions (
                id, project_root, harness, title, status, created_at, updated_at
             ) VALUES (
                'session-daily-notes-1', '/work/project', 'runtime:coven-code',
                'Terminal observer fixture', 'running',
                '2026-09-03T12:00:00.000Z', '2026-09-03T12:00:00.000Z'
             );
             INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, definition_digest,
                scheduled_for, kind, state, attempt, created_at, updated_at
             ) VALUES (
                'occurrence.daily-notes-20260903', 'daily-notes', 4,
                '1111111111111111111111111111111111111111111111111111111111111111',
                '2026-09-03T12:00:00.000Z', 'scheduled', 'running', 1,
                '2026-09-03T12:00:00.000Z', '2026-09-03T12:00:00.000Z'
             );
             INSERT INTO automation_runs (
                id, automation_id, automation_revision, definition_digest,
                occurrence_id, authority_profile, session_id, familiar_id,
                runtime, status, started_at
             ) VALUES (
                'run.daily-notes-1', 'daily-notes', 4,
                '1111111111111111111111111111111111111111111111111111111111111111',
                'occurrence.daily-notes-20260903', 'coven.automations.authority.v1',
                'session-daily-notes-1', 'charm', 'runtime:coven-code',
                'running', '2026-09-03T12:00:00.000Z'
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_attempts (
                id, run_id, occurrence_id, attempt_number, adoption_key,
                occurrence_fence_generation, dispatch_generation, state,
                retry_classification, authority_extension_json, not_before,
                session_id, opened_at
             ) VALUES (
                'attempt.daily-notes-1-1', 'run.daily-notes-1',
                'occurrence.daily-notes-20260903', 1, 'adopt:daily-notes-1-1',
                7, 1, 'observing', 'initial', ?1,
                '2026-09-03T12:00:00.000Z', 'session-daily-notes-1',
                '2026-09-03T12:00:00.000Z'
             )",
            [serde_json::to_string(&extension).unwrap()],
        )
        .unwrap();
        if observer_key {
            authority_keys::current_signing_key(
                &conn,
                home,
                AuthorityKeyRole::TerminalObserver,
                Utc.with_ymd_and_hms(2026, 9, 3, 11, 0, 0).unwrap(),
            )
            .unwrap();
        }
        conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::authority_keys::{self, AuthorityKeyRole};
    use crate::automations::contract::runtime_terminal_evidence::test_support::binding;
    use crate::automations::contract::runtime_terminal_evidence::{
        verify_runtime_terminal_evidence, RuntimeTerminalEvidenceClassification,
        RuntimeTerminalEvidenceErrorCode,
    };
    use crate::automations::ed25519_trust::Ed25519TerminalEvidenceVerifier;
    use chrono::TimeZone;
    use rusqlite::Connection;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, hour, 0, 0).unwrap()
    }

    #[test]
    fn a_plain_text_session_is_signed_honest_unknown_for_every_ending() {
        let temp = tempfile::tempdir().unwrap();
        let conn = Connection::open(temp.path().join("store.sqlite")).unwrap();
        let key = authority_keys::current_signing_key(
            &conn,
            temp.path(),
            AuthorityKeyRole::TerminalObserver,
            at(9),
        )
        .unwrap();
        let trusted =
            authority_keys::trusted_keys(&conn, AuthorityKeyRole::TerminalObserver).unwrap();
        let binding = binding();
        for disposition in [
            TerminalOutcome::Succeeded,
            TerminalOutcome::Failed,
            TerminalOutcome::Cancelled,
            TerminalOutcome::TimedOut,
            TerminalOutcome::Ambiguous,
        ] {
            let observation = TerminalObservation {
                session_id: "session-daily-notes-1",
                disposition,
            };
            let evidence = observe(&key, &binding, &observation, at(10)).unwrap();
            let value = serde_json::to_value(&evidence).unwrap();
            assert_eq!(value["disposition"], json!(disposition));
            for member in ["sideEffects", "exercisedCapabilities", "result", "delivery"] {
                assert_eq!(value[member]["state"], json!("unknown"), "{member}");
            }
            let verified = verify_runtime_terminal_evidence(
                evidence,
                &binding,
                &Ed25519TerminalEvidenceVerifier(&trusted),
            )
            .unwrap();
            assert_eq!(
                verified.classification,
                RuntimeTerminalEvidenceClassification::AuthenticatedUnknown
            );
        }
    }

    #[test]
    fn only_the_observer_key_inside_its_window_authenticates() {
        let temp = tempfile::tempdir().unwrap();
        let conn = Connection::open(temp.path().join("store.sqlite")).unwrap();
        let binding = binding();
        let observation = TerminalObservation {
            session_id: "session-daily-notes-1",
            disposition: TerminalOutcome::Succeeded,
        };
        let refusal = |role: AuthorityKeyRole, produced: DateTime<Utc>| {
            let key = authority_keys::current_signing_key(&conn, temp.path(), role, at(9)).unwrap();
            let evidence = observe(&key, &binding, &observation, produced).unwrap();
            let trusted =
                authority_keys::trusted_keys(&conn, AuthorityKeyRole::TerminalObserver).unwrap();
            verify_runtime_terminal_evidence(
                evidence,
                &binding,
                &Ed25519TerminalEvidenceVerifier(&trusted),
            )
            .err()
            .map(|error| error.code())
        };
        assert_eq!(refusal(AuthorityKeyRole::TerminalObserver, at(10)), None);
        assert_eq!(
            refusal(AuthorityKeyRole::DispatchAuthority, at(10)),
            Some(RuntimeTerminalEvidenceErrorCode::AuthenticationUnverifiable)
        );
        assert_eq!(
            refusal(AuthorityKeyRole::TerminalObserver, at(8)),
            Some(RuntimeTerminalEvidenceErrorCode::AuthenticationStale)
        );
    }

    use crate::automations::runtime_terminal_evidence::{
        read_verified_runtime_terminal_evidence, RuntimeTerminalEvidenceLookup,
    };
    use test_support::{ended_at, seed, SESSION_ID};

    fn stored(conn: &Connection) -> Option<Value> {
        read_verified_runtime_terminal_evidence(
            conn,
            RuntimeTerminalEvidenceLookup::AttemptId("attempt.daily-notes-1-1"),
            &Ed25519TerminalEvidenceVerifier(
                &authority_keys::trusted_keys(conn, AuthorityKeyRole::TerminalObserver).unwrap(),
            ),
        )
        .unwrap()
        .map(|verified| {
            assert_eq!(
                verified.classification,
                RuntimeTerminalEvidenceClassification::AuthenticatedUnknown
            );
            serde_json::to_value(verified.evidence).unwrap()
        })
    }

    #[test]
    fn the_ending_writer_records_verified_evidence_once() {
        let temp = tempfile::tempdir().unwrap();
        let conn = seed(temp.path(), true);
        let transaction = conn.unchecked_transaction().unwrap();
        assert_eq!(
            record(
                &transaction,
                SESSION_ID,
                TerminalOutcome::Failed,
                ended_at()
            ),
            Recorded::Stored
        );
        // The same observation again is a replay, not a second record.
        assert_eq!(
            record(
                &transaction,
                SESSION_ID,
                TerminalOutcome::Failed,
                ended_at()
            ),
            Recorded::Replayed
        );
        transaction.commit().unwrap();
        let evidence = stored(&conn).unwrap();
        assert_eq!(evidence["disposition"], json!("failed"));
        assert_eq!(evidence["sessionId"], json!(SESSION_ID));
        assert!(evidence["authentication"]["keyId"]
            .as_str()
            .unwrap()
            .starts_with("coven-local:terminal-observer:"));
    }

    #[test]
    fn a_session_without_a_runtime_authority_attempt_is_not_observed() {
        let temp = tempfile::tempdir().unwrap();
        let conn = seed(temp.path(), true);
        conn.execute_batch(
            "INSERT INTO sessions (
                id, project_root, harness, title, status, created_at, updated_at
             ) VALUES (
                'session-plain', '/work/project', 'runtime:coven-code', 'Plain run',
                'running', '2026-09-03T12:00:00.000Z', '2026-09-03T12:00:00.000Z'
             );
             INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, definition_digest,
                scheduled_for, kind, state, attempt, created_at, updated_at
             ) VALUES (
                'occurrence.plain', 'daily-notes', 4,
                '1111111111111111111111111111111111111111111111111111111111111111',
                '2026-09-03T13:00:00.000Z', 'scheduled', 'running', 1,
                '2026-09-03T12:00:00.000Z', '2026-09-03T12:00:00.000Z'
             );
             INSERT INTO automation_runs (
                id, automation_id, automation_revision, definition_digest,
                occurrence_id, session_id, familiar_id, runtime, status, started_at
             ) VALUES (
                'run.plain', 'daily-notes', 4,
                '1111111111111111111111111111111111111111111111111111111111111111',
                'occurrence.plain', 'session-plain', 'charm',
                'runtime:coven-code', 'running', '2026-09-03T12:00:00.000Z'
             );
             INSERT INTO automation_attempts (
                id, run_id, occurrence_id, attempt_number, adoption_key,
                occurrence_fence_generation, dispatch_generation, state,
                retry_classification, not_before, session_id, opened_at
             ) VALUES (
                'attempt.plain-1', 'run.plain', 'occurrence.plain', 1,
                'adopt:plain-1', 7, 1, 'observing', 'initial',
                '2026-09-03T12:00:00.000Z', 'session-plain', '2026-09-03T12:00:00.000Z'
             );",
        )
        .unwrap();
        assert_eq!(
            record(
                &conn,
                "session-plain",
                TerminalOutcome::Succeeded,
                ended_at()
            ),
            Recorded::NotAuthority
        );
        assert_eq!(
            record(
                &conn,
                "session-unknown",
                TerminalOutcome::Succeeded,
                ended_at()
            ),
            Recorded::NotAuthority
        );
    }

    #[test]
    fn a_failed_observation_stores_nothing_and_leaves_the_ending_intact() {
        let temp = tempfile::tempdir().unwrap();
        // No observer key: nothing can be signed.
        let conn = seed(temp.path(), false);
        let transaction = conn.unchecked_transaction().unwrap();
        crate::store::update_session_terminal_if_active(
            &transaction,
            SESSION_ID,
            "failed",
            Some(1),
            "2026-09-03T12:30:00.000Z",
        )
        .unwrap();
        let recorded = record(
            &transaction,
            SESSION_ID,
            TerminalOutcome::Failed,
            ended_at(),
        );
        assert!(
            matches!(&recorded, Recorded::Failed(reason) if reason.contains("terminal-observer key")),
            "{recorded:?}"
        );
        transaction.commit().unwrap();
        assert_eq!(stored(&conn), None);
        let status: String = conn
            .query_row(
                "SELECT status FROM sessions WHERE id = ?1",
                [SESSION_ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "failed");
        let log =
            std::fs::read_to_string(crate::daemon::daemon_recovery_log_path(temp.path())).unwrap();
        assert!(
            log.contains("stored no evidence for session `session-daily-notes-1`"),
            "{log}"
        );
    }

    #[test]
    fn a_confirmed_timeout_records_its_evidence_with_the_hold() {
        use crate::automations::runner::{
            settle_confirmed_stop, ConfirmedStop, ConfirmedStopSettlement,
        };
        let temp = tempfile::tempdir().unwrap();
        let conn = seed(temp.path(), true);
        assert_eq!(
            settle_confirmed_stop(
                &conn,
                test_support::RUN_ID,
                SESSION_ID,
                ConfirmedStop::TimedOut,
                ended_at()
            ),
            Ok(ConfirmedStopSettlement::RecoveryRequired)
        );
        assert_eq!(stored(&conn).unwrap()["disposition"], json!("timed_out"));
    }

    #[test]
    fn a_restart_recovered_ending_is_recorded_as_ambiguous() {
        use crate::automations::runner::{containment_receipt_path, recover_restart_containment};
        let temp = tempfile::tempdir().unwrap();
        let conn = seed(temp.path(), true);
        conn.execute(
            "UPDATE sessions SET status = 'orphaned' WHERE id = ?1",
            [SESSION_ID],
        )
        .unwrap();
        let receipt = containment_receipt_path(temp.path(), SESSION_ID);
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        std::fs::write(&receipt, crate::pty_runner::CONTAINMENT_QUIESCENT_RECEIPT).unwrap();
        assert_eq!(
            recover_restart_containment(temp.path(), &conn, ended_at(), None),
            Ok(1)
        );
        // How the process ended was not seen.
        assert_eq!(stored(&conn).unwrap()["disposition"], json!("ambiguous"));
    }
}
