//! The production scoped read gate (#1603), as
//! `docs/architecture/coven-automations-scoped-read-gate-proposal.md`
//! recommends.
//!
//! Five sources can withdraw a running attempt's authority: a role key
//! revocation, a familiar revision revocation, a definition revision that is
//! no longer current and active, a stop or cancellation, and the attempt or
//! run settling. All five are SQLite writes, and SQLite takes one writer at a
//! time, so the store's write lock is the serialization point.
//!
//! - [`authorized`] is the predicate, P. The launch, [`StoreGate::admit`] and
//!   [`StoreGate::commit`] all evaluate it, and nothing else decides whether
//!   the attempt may still read.
//! - `admit` evaluates P in a read transaction and records nothing. It is an
//!   early refusal, not the authority.
//! - `commit` is the authority. In one `BEGIN IMMEDIATE` transaction it
//!   evaluates P again and inserts one immutable commitment row. When P fails
//!   it records a `withheld` row in the same transaction and refuses
//!   (`attempt_closed`), so whichever of a revocation and a commitment takes
//!   the write lock first wins. P's time is read after the lock is taken, so
//!   a run whose `timeout_at` passes while the call waits is withheld too. A
//!   busy or unreadable store refuses.
//!
//! A row holds the result's SHA-256 and length, never its content, the path
//! asked for, a search pattern or an entry name.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;

use super::contract::authority::AuthorityProfileErrorCode;
use super::contract::canonical_json::sha256_hex;
use super::scoped_read::service::{Committing, Gate};
use super::scoped_read::Refusal;

/// One row per committed or withheld operation. Rows are never updated or
/// deleted, as for Threads decisions.
pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS automation_scoped_read_commitments (
        attempt_id TEXT NOT NULL REFERENCES automation_attempts (id),
        channel_id TEXT NOT NULL,
        sequence INTEGER NOT NULL CHECK (sequence >= 1),
        tool TEXT CHECK (tool IS NULL OR tool IN ('read_file', 'list_directory', 'search')),
        outcome TEXT NOT NULL CHECK (outcome IN ('served', 'refused', 'withheld')),
        refusal TEXT,
        result_digest TEXT CHECK (result_digest IS NULL OR length(result_digest) = 64),
        result_bytes INTEGER,
        committed_at TEXT NOT NULL,
        PRIMARY KEY (attempt_id, channel_id, sequence),
        CHECK ((outcome = 'withheld') = (result_digest IS NULL))
    );
    CREATE TRIGGER IF NOT EXISTS automation_scoped_read_commitments_immutable
    BEFORE UPDATE ON automation_scoped_read_commitments
    BEGIN SELECT RAISE(ABORT, 'scoped read commitments are immutable'); END;
    CREATE TRIGGER IF NOT EXISTS automation_scoped_read_commitments_retained
    BEFORE DELETE ON automation_scoped_read_commitments
    BEGIN SELECT RAISE(ABORT, 'scoped read commitments are retained'); END;
";

/// What the launch verified, fixed for the attempt: whose it is, the binding
/// it pinned, and what that binding and its decision rest on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Held {
    pub attempt_id: String,
    pub run_id: String,
    pub session_id: String,
    /// The SHA-256 of the attempt's pinned `authority_extension_json`, as
    /// verified at launch.
    pub extension_digest: String,
    pub identity_revision_id: String,
    pub automation_id: String,
    pub automation_revision: u64,
    /// The `dispatch-authority` key that signed the binding, the
    /// `threads-decision` key that signed the decision, and the
    /// `owner-principal` key that signed its request.
    pub key_ids: Vec<String>,
}

impl Held {
    /// The held facts of a verified launch: the attempt's `pinned`
    /// extension text, its execution `binding`, and the recorded Threads
    /// `decision` and its `request`.
    pub(crate) fn new(
        pinned: &str,
        binding: &Value,
        session_id: &str,
        decision: &Value,
        request: &Value,
    ) -> Result<Self> {
        let text = |value: &Value, name: &str| {
            value[name]
                .as_str()
                .map(str::to_owned)
                .with_context(|| format!("the authority evidence carries {name}"))
        };
        let base = &binding["base"];
        Ok(Self {
            attempt_id: text(base, "attemptId")?,
            run_id: text(base, "runId")?,
            session_id: session_id.to_owned(),
            extension_digest: sha256_hex(pinned.as_bytes()),
            identity_revision_id: text(&binding["familiar"], "identityRevisionId")?,
            automation_id: text(base, "automationId")?,
            automation_revision: base["automationRevision"]
                .as_u64()
                .context("the binding carries automationRevision")?,
            key_ids: vec![
                text(&binding["authentication"], "keyId")?,
                text(&decision["integrity"], "key_id")?,
                text(&request["integrity"], "key_id")?,
            ],
        })
    }
}

/// Why P no longer holds, with the profile code that fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Withdrawal {
    pub code: AuthorityProfileErrorCode,
    pub why: &'static str,
}

/// P: whether the attempt `held` describes may still read at `now`.
///
/// 1. The attempt, run, occurrence and session are live and owned by this
///    session, as `runner::claim_stop_fence` requires (`Stale`).
/// 2. The attempt's pinned binding is the one verified at launch
///    (`BindingMismatch`).
/// 3. No stop fence is held and no cancellation is requested or stopping
///    (`FenceStale`).
/// 4. None of the keys the binding and decision rest on is revoked
///    (`AuthenticationUnverifiable`).
/// 5. The familiar revision is neither retired nor revoked
///    (`FamiliarStatusInvalid`).
/// 6. The definition revision is still current and active, as
///    `owner_grants::authorize_scheduled_dispatch` requires
///    (`DefinitionMismatch`).
/// 7. The run's `timeout_at` has not passed (`Stale`).
///
/// It only reads. A store that has never stopped a run has no stop tables,
/// and so nothing pending.
pub(crate) fn authorized(
    conn: &Connection,
    held: &Held,
    now: DateTime<Utc>,
) -> Result<std::result::Result<(), Withdrawal>> {
    let withdrawn = |code, why| Ok(Err(Withdrawal { code, why }));

    // 1. Live and owned.
    let timeout_at: Option<Option<String>> = conn
        .query_row(
            "SELECT r.timeout_at
             FROM automation_runs AS r
             JOIN sessions AS s ON s.id = r.session_id
             JOIN automation_attempts AS a
               ON a.run_id = r.id
              AND (
                  a.session_id = r.session_id
                  OR (a.state = 'dispatching' AND a.session_id IS NULL)
              )
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id
             WHERE r.id = ?1
               AND a.id = ?2
               AND r.session_id = ?3
               AND r.status = 'running'
               AND s.status IN ('created', 'running', 'orphaned')
               AND a.state IN ('dispatching', 'started', 'observing')
               AND o.state IN ('claimed', 'running')",
            params![held.run_id, held.attempt_id, held.session_id],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read the attempt's liveness")?;
    let Some(timeout_at) = timeout_at else {
        return withdrawn(
            AuthorityProfileErrorCode::Stale,
            "the attempt is no longer live and owned",
        );
    };

    // 2. The binding.
    let pinned: Option<String> = conn
        .query_row(
            "SELECT authority_extension_json FROM automation_attempts WHERE id = ?1",
            [&held.attempt_id],
            |row| row.get(0),
        )
        .context("failed to read the attempt's pinned authority")?;
    if pinned
        .as_deref()
        .map(|text| sha256_hex(text.as_bytes()))
        .as_ref()
        != Some(&held.extension_digest)
    {
        return withdrawn(
            AuthorityProfileErrorCode::BindingMismatch,
            "the attempt's pinned binding changed",
        );
    }

    // 3. Stops.
    let table = |name: &str| -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
        .with_context(|| format!("failed to look for {name}"))
    };
    let pending = |sql: &str| -> Result<bool> {
        conn.query_row(sql, [&held.run_id], |row| row.get(0))
            .context("failed to read the run's stop state")
    };
    let stopping = (table("automation_stop_fences")?
        && pending("SELECT EXISTS (SELECT 1 FROM automation_stop_fences WHERE run_id = ?1)")?)
        || (table("automation_cancellations")?
            && pending(
                "SELECT EXISTS (SELECT 1 FROM automation_cancellations
                                WHERE run_id = ?1 AND state IN ('requested', 'stopping'))",
            )?);
    if stopping {
        return withdrawn(
            AuthorityProfileErrorCode::FenceStale,
            "a stop or cancellation is pending",
        );
    }

    // 4. Keys, by primary key rather than by verifying the signatures again.
    for key_id in &held.key_ids {
        let live: Option<bool> = conn
            .query_row(
                "SELECT revoked_at IS NULL FROM automation_authority_keys WHERE key_id = ?1",
                [key_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to read an authority key")?;
        if live != Some(true) {
            return withdrawn(
                AuthorityProfileErrorCode::AuthenticationUnverifiable,
                "a key the attempt's authority rests on was revoked",
            );
        }
    }

    // 5. The familiar revision.
    let familiar: Option<String> = conn
        .query_row(
            "SELECT status FROM familiar_ledger_revisions WHERE revision_id = ?1",
            [&held.identity_revision_id],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read the familiar revision")?;
    if !matches!(familiar.as_deref(), Some("active" | "superseded")) {
        return withdrawn(
            AuthorityProfileErrorCode::FamiliarStatusInvalid,
            "the familiar revision is retired, revoked or unknown",
        );
    }

    // 6. The definition revision.
    if super::owner_grants::current_and_active(conn, &held.automation_id, held.automation_revision)?
        .is_err()
    {
        return withdrawn(
            AuthorityProfileErrorCode::DefinitionMismatch,
            "the definition revision is no longer current and active",
        );
    }

    // 7. The run's time.
    if let Some(timeout_at) = timeout_at {
        let timeout_at = DateTime::parse_from_rfc3339(&timeout_at)
            .context("the run's timeout_at is a timestamp")?;
        if now >= timeout_at {
            return withdrawn(AuthorityProfileErrorCode::Stale, "the run's time is up");
        }
    }
    Ok(Ok(()))
}

/// The gate a trusted launch's scoped read service runs under. It holds its
/// own store connection and what the launch verified.
pub(crate) struct StoreGate {
    conn: Connection,
    held: Held,
    channel_id: String,
    /// The most a call waits for the store's write lock.
    busy: Duration,
    /// When the run ends, if it has a deadline. No wait outlasts it.
    deadline: Option<Instant>,
    /// The wall clock P is evaluated against.
    clock: Clock,
    /// Runs just before `commit` asks for the write lock, so a test can
    /// order a deadline against the wait.
    #[cfg(test)]
    locking: Option<Box<dyn FnMut() + Send>>,
}

impl StoreGate {
    /// A gate over the store in `coven_home`, for the attempt `held`
    /// describes. Each gate is a new channel: its rows carry a random id, so a
    /// restarted attempt never continues an old channel's sequence.
    pub(crate) fn open(
        coven_home: &Path,
        held: Held,
        busy: Duration,
        deadline: Option<Instant>,
    ) -> Result<Self> {
        let conn = crate::store::open_store(&crate::api::store_path(coven_home))?;
        conn.execute_batch(SCHEMA)
            .context("failed to create the scoped read commitments table")?;
        Ok(Self {
            conn,
            held,
            channel_id: random_hex(16)?,
            busy,
            deadline,
            clock: Clock::default(),
            #[cfg(test)]
            locking: None,
        })
    }

    /// Bounds how long the next transaction waits for the store.
    fn wait_at_most(&self) -> std::result::Result<(), Refusal> {
        let wait = match self.deadline {
            Some(deadline) => self
                .busy
                .min(deadline.saturating_duration_since(Instant::now())),
            None => self.busy,
        };
        self.conn
            .busy_timeout(wait)
            .map_err(|_| Refusal::AttemptClosed)
    }

    /// Before an operation runs: P holds. Records nothing.
    pub(crate) fn admit(&mut self) -> std::result::Result<(), Refusal> {
        self.wait_at_most()?;
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| Refusal::AttemptClosed)?;
        let verdict = authorized(&transaction, &self.held, self.clock.now());
        // Read only: the transaction rolls back.
        drop(transaction);
        match verdict {
            Ok(Ok(())) => Ok(()),
            _ => Err(Refusal::AttemptClosed),
        }
    }

    /// After a result is built and before any of it is written: in one
    /// `BEGIN IMMEDIATE` transaction, evaluates P and records the operation.
    /// `Ok` means the row committed and those bytes are authorized. When P
    /// fails, a `withheld` row commits instead and the call is refused, as it
    /// is when the store is busy or unreadable.
    pub(crate) fn commit(
        &mut self,
        committing: &Committing<'_>,
    ) -> std::result::Result<(), Refusal> {
        self.wait_at_most()?;
        #[cfg(test)]
        if let Some(locking) = self.locking.as_mut() {
            locking();
        }
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Refusal::AttemptClosed)?;
        // The time is read only once the write lock is held: the wait for it
        // can outlast the run's `timeout_at`, and P must hold when the row
        // commits, not when the call began to wait (#1605).
        let now = self.clock.now();
        let authorized = matches!(authorized(&transaction, &self.held, now), Ok(Ok(())));
        let (outcome, refusal, digest, bytes) = match (authorized, committing.refusal) {
            (true, None) => (
                "served",
                None,
                Some(committing.digest),
                Some(committing.bytes),
            ),
            (true, Some(refusal)) => (
                "refused",
                Some(refusal.code()),
                Some(committing.digest),
                Some(committing.bytes),
            ),
            (false, _) => ("withheld", Some(Refusal::AttemptClosed.code()), None, None),
        };
        transaction
            .execute(
                "INSERT INTO automation_scoped_read_commitments (
                    attempt_id, channel_id, sequence, tool, outcome, refusal,
                    result_digest, result_bytes, committed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    self.held.attempt_id,
                    self.channel_id,
                    i64::try_from(committing.sequence).map_err(|_| Refusal::AttemptClosed)?,
                    committing.tool,
                    outcome,
                    refusal,
                    digest,
                    bytes.map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX)),
                    now.to_rfc3339_opts(SecondsFormat::Millis, true),
                ],
            )
            .map_err(|_| Refusal::AttemptClosed)?;
        transaction.commit().map_err(|_| Refusal::AttemptClosed)?;
        if authorized {
            Ok(())
        } else {
            Err(Refusal::AttemptClosed)
        }
    }
}

/// The wall clock, which a test can replace to move it.
#[derive(Default)]
struct Clock {
    #[cfg(test)]
    replaced: Option<Box<dyn Fn() -> DateTime<Utc> + Send>>,
}

impl Clock {
    fn now(&self) -> DateTime<Utc> {
        #[cfg(test)]
        if let Some(now) = &self.replaced {
            return now();
        }
        Utc::now()
    }
}

impl Gate for StoreGate {
    fn admit(&mut self) -> std::result::Result<(), Refusal> {
        StoreGate::admit(self)
    }

    fn commit(&mut self, committing: &Committing<'_>) -> std::result::Result<(), Refusal> {
        StoreGate::commit(self, committing)
    }
}

fn random_hex(bytes: usize) -> Result<String> {
    use ring::rand::SecureRandom;
    let mut buffer = vec![0u8; bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut buffer)
        .map_err(|_| anyhow::anyhow!("no system randomness for a scoped read channel id"))?;
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests;
