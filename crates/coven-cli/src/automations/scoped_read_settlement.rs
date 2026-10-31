//! Scoped read settlement evidence (#1606).
//!
//! A scoped read attempt leaves two records in the store, written by
//! different steps:
//! - **Commitments.** [`super::store_gate::StoreGate::commit`] inserts one row
//!   per operation under the write lock, before any byte of its result is
//!   written. A row says the result was *authorized*, not that it was
//!   delivered: the daemon can crash, or the write can fail, after it
//!   commits.
//! - **The close.** When the service stops, the gate records what each tool
//!   call did, as the service saw it, including whether its reply was
//!   written ([`record`]). A channel the daemon never closed, because it
//!   crashed or was killed, has no close.
//!
//! [`reconcile`] compares the two. Settlement treats a scoped read attempt's
//! evidence as complete only when exactly one channel closed and every
//! operation it lists matches its commitment: the same sequence, tool,
//! outcome, refusal and digest, with a committed result also written. A
//! missing close, a second channel, a commitment no operation accounts for,
//! an operation with no commitment, or any field that differs is a
//! [`Discrepancy`], and the evidence stays partial: the run is held for
//! recovery rather than settled, and nothing is replayed or invented.
//!
//! Records that agree still do not settle a run whose service withheld an
//! operation (#1608). The gate withholds when it cannot confirm the
//! attempt's authority: it was withdrawn, the run's deadline passed, or the
//! store could not be reached. A success would rest on that authority, and
//! whatever the harness reports after it is not evidence the service
//! answered.
//!
//! Like a commitment, a close holds no file content, no path, no search
//! pattern and no entry name.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::scoped_read::service::{Operation, Outcome};

/// One row per closed channel. Rows are never updated or deleted.
pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS automation_scoped_read_closes (
        attempt_id TEXT NOT NULL REFERENCES automation_attempts (id),
        channel_id TEXT NOT NULL,
        operations INTEGER NOT NULL CHECK (operations >= 0),
        operations_json TEXT NOT NULL,
        closed_at TEXT NOT NULL,
        PRIMARY KEY (attempt_id, channel_id)
    );
    CREATE TRIGGER IF NOT EXISTS automation_scoped_read_closes_immutable
    BEFORE UPDATE ON automation_scoped_read_closes
    BEGIN SELECT RAISE(ABORT, 'scoped read closes are immutable'); END;
    CREATE TRIGGER IF NOT EXISTS automation_scoped_read_closes_retained
    BEFORE DELETE ON automation_scoped_read_closes
    BEGIN SELECT RAISE(ABORT, 'scoped read closes are retained'); END;
";

/// One operation as a close records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Closed {
    sequence: u64,
    tool: Option<String>,
    outcome: String,
    refusal: Option<String>,
    digest: Option<String>,
    written: bool,
}

// Only the store gate, which is unix-only, records a close.
#[cfg_attr(not(unix), allow(dead_code))]
impl Closed {
    fn of(operation: &Operation) -> Self {
        let (outcome, refusal, digest) = match &operation.outcome {
            Outcome::Served { digest } => ("served", None, Some(digest.clone())),
            Outcome::Refused { refusal, digest } => {
                ("refused", Some(refusal.code()), Some(digest.clone()))
            }
            Outcome::Withheld { refusal } => ("withheld", Some(refusal.code()), None),
        };
        Self {
            sequence: operation.sequence,
            tool: operation.tool.map(str::to_owned),
            outcome: outcome.to_owned(),
            refusal: refusal.map(str::to_owned),
            digest,
            written: operation.written,
        }
    }
}

/// Records that the channel `channel_id` of `attempt_id` closed after
/// `operations`.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn record(
    conn: &Connection,
    attempt_id: &str,
    channel_id: &str,
    operations: &[Operation],
    now: DateTime<Utc>,
) -> Result<()> {
    let closed: Vec<Closed> = operations.iter().map(Closed::of).collect();
    let text = serde_json::to_string(&closed).context("a scoped read close is JSON")?;
    conn.execute(
        "INSERT INTO automation_scoped_read_closes (
            attempt_id, channel_id, operations, operations_json, closed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            attempt_id,
            channel_id,
            i64::try_from(closed.len()).context("the operation count fits")?,
            text,
            now.to_rfc3339_opts(SecondsFormat::Millis, true),
        ],
    )
    .context("failed to record the scoped read close")?;
    Ok(())
}

/// Why a scoped read attempt's commitments and close do not reconcile. Each
/// is a stable code, never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Discrepancy {
    /// No channel of the attempt closed: the daemon stopped, or the close
    /// could not be written, before the service's operations were recorded.
    NotClosed,
    /// More than one channel of the attempt closed.
    SeveralChannels,
    /// The close's record cannot be read as written.
    CloseInvalid,
    /// The close lists sequences other than 1 to its count in order, or an
    /// operation after one that was withheld.
    SequenceInvalid,
    /// A commitment comes from a channel that never closed.
    ForeignChannel,
    /// A commitment has no operation in the close.
    CommitmentUnaccounted,
    /// A served or refused operation has no commitment.
    OperationUncommitted,
    /// An operation's commitment names another tool.
    ToolMismatch,
    /// An operation's commitment has another outcome.
    OutcomeMismatch,
    /// An operation's commitment has another refusal.
    RefusalMismatch,
    /// An operation's commitment authorized other bytes.
    DigestMismatch,
    /// A committed result was not written: authorized, never delivered.
    CommittedNotWritten,
    /// The service ran more operations than the harness's stream shows tool
    /// calls.
    OperationNotInStream,
    /// The records agree, and the service withheld an operation: the
    /// attempt's authority ended, or could not be confirmed, during the run.
    OperationWithheld,
}

impl Discrepancy {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::NotClosed => "scoped_read_not_closed",
            Self::SeveralChannels => "scoped_read_several_channels",
            Self::CloseInvalid => "scoped_read_close_invalid",
            Self::SequenceInvalid => "scoped_read_sequence_invalid",
            Self::ForeignChannel => "scoped_read_foreign_channel",
            Self::CommitmentUnaccounted => "scoped_read_commitment_unaccounted",
            Self::OperationUncommitted => "scoped_read_operation_uncommitted",
            Self::ToolMismatch => "scoped_read_tool_mismatch",
            Self::OutcomeMismatch => "scoped_read_outcome_mismatch",
            Self::RefusalMismatch => "scoped_read_refusal_mismatch",
            Self::DigestMismatch => "scoped_read_digest_mismatch",
            Self::CommittedNotWritten => "scoped_read_committed_not_written",
            Self::OperationNotInStream => "scoped_read_operation_not_in_stream",
            Self::OperationWithheld => "scoped_read_operation_withheld",
        }
    }
}

struct Commitment {
    channel_id: String,
    sequence: i64,
    tool: Option<String>,
    outcome: String,
    refusal: Option<String>,
    digest: Option<String>,
}

/// Whether `attempt_id`'s scoped read commitments and close reconcile, and if
/// so how many operations its channel ran. With `stream_calls`, the number of
/// service tool calls the harness's stream shows, the service must not have
/// run more operations than that. It only reads.
pub(crate) fn reconcile(
    conn: &Connection,
    attempt_id: &str,
    stream_calls: Option<usize>,
) -> Result<std::result::Result<usize, Discrepancy>> {
    use Discrepancy::*;

    let table = |name: &str| -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
        .with_context(|| format!("failed to look for {name}"))
    };

    let closes: Vec<(String, i64, String)> = if table("automation_scoped_read_closes")? {
        let mut statement = conn
            .prepare(
                "SELECT channel_id, operations, operations_json
                 FROM automation_scoped_read_closes WHERE attempt_id = ?1",
            )
            .context("failed to prepare the scoped read close read")?;
        let rows = statement
            .query_map([attempt_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .context("failed to read the scoped read closes")?;
        rows.collect::<rusqlite::Result<_>>()
            .context("failed to read a scoped read close")?
    } else {
        Vec::new()
    };
    let (channel_id, count, text) = match closes.as_slice() {
        [] => return Ok(Err(NotClosed)),
        [close] => close,
        _ => return Ok(Err(SeveralChannels)),
    };
    let Ok(closed) = serde_json::from_str::<Vec<Closed>>(text) else {
        return Ok(Err(CloseInvalid));
    };
    if usize::try_from(*count).ok() != Some(closed.len()) {
        return Ok(Err(CloseInvalid));
    }
    for (index, operation) in closed.iter().enumerate() {
        let withheld = operation.outcome == "withheld";
        let shaped = match operation.outcome.as_str() {
            "served" => operation.refusal.is_none() && operation.digest.is_some(),
            "refused" => operation.refusal.is_some() && operation.digest.is_some(),
            "withheld" => operation.refusal.is_some() && operation.digest.is_none(),
            _ => false,
        };
        if !shaped {
            return Ok(Err(CloseInvalid));
        }
        if operation.sequence != index as u64 + 1 || (withheld && index + 1 != closed.len()) {
            return Ok(Err(SequenceInvalid));
        }
    }

    let commitments: Vec<Commitment> = if table("automation_scoped_read_commitments")? {
        let mut statement = conn
            .prepare(
                "SELECT channel_id, sequence, tool, outcome, refusal, result_digest
                 FROM automation_scoped_read_commitments
                 WHERE attempt_id = ?1
                 ORDER BY channel_id, sequence",
            )
            .context("failed to prepare the scoped read commitment read")?;
        let rows = statement
            .query_map([attempt_id], |row| {
                Ok(Commitment {
                    channel_id: row.get(0)?,
                    sequence: row.get(1)?,
                    tool: row.get(2)?,
                    outcome: row.get(3)?,
                    refusal: row.get(4)?,
                    digest: row.get(5)?,
                })
            })
            .context("failed to read the scoped read commitments")?;
        rows.collect::<rusqlite::Result<_>>()
            .context("failed to read a scoped read commitment")?
    } else {
        Vec::new()
    };
    if commitments
        .iter()
        .any(|commitment| &commitment.channel_id != channel_id)
    {
        return Ok(Err(ForeignChannel));
    }

    let mut accounted = 0;
    for operation in &closed {
        let commitment = commitments
            .iter()
            .find(|commitment| u64::try_from(commitment.sequence).ok() == Some(operation.sequence));
        let Some(commitment) = commitment else {
            // A gate that refused at admit, or could not reach the store,
            // records nothing.
            if operation.outcome == "withheld" {
                continue;
            }
            return Ok(Err(OperationUncommitted));
        };
        accounted += 1;
        if commitment.tool != operation.tool {
            return Ok(Err(ToolMismatch));
        }
        if commitment.outcome != operation.outcome {
            return Ok(Err(OutcomeMismatch));
        }
        if commitment.refusal != operation.refusal {
            return Ok(Err(RefusalMismatch));
        }
        if commitment.digest != operation.digest {
            return Ok(Err(DigestMismatch));
        }
        if operation.outcome != "withheld" && !operation.written {
            return Ok(Err(CommittedNotWritten));
        }
    }
    if accounted != commitments.len() {
        return Ok(Err(CommitmentUnaccounted));
    }
    if stream_calls.is_some_and(|calls| closed.len() > calls) {
        return Ok(Err(OperationNotInStream));
    }
    if closed
        .iter()
        .any(|operation| operation.outcome == "withheld")
    {
        return Ok(Err(OperationWithheld));
    }
    Ok(Ok(closed.len()))
}

#[cfg(all(test, unix))]
mod tests;
