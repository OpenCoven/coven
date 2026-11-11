//! The durable apply intent of an approved Threads proposal: the applying
//! state (decision, recovery commitment, weave hash, before-images) appended to
//! the Ward audit log before any file changes, and the latest one read back by
//! recovery. When to append it, and what recovery does with it, stay with the
//! API coordinator.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::store;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProposalApplyingState {
    pub(super) decision: String,
    pub(super) recovery_commitment: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) baseline_snapshot: Option<BTreeMap<String, Option<Vec<u8>>>>,
    pub(super) weave_hash: Vec<u8>,
    pub(super) before_images: Vec<ProposalBeforeImage>,
    pub(super) rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) probe_summary: Option<crate::ward_probes::ProbeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProposalBeforeImage {
    pub(super) target: String,
    /// Gate-2-resolved surface captured with the apply intent. Older authority
    /// recovery records predate this field and rederive it from live Ward
    /// adjudication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) resolved: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) contents: Option<coven_threads_core::StagedContents>,
}

// Preserve the borrowed apply-intent inputs while passing the decision's captured time.
#[allow(clippy::too_many_arguments)]
pub(super) fn append_proposal_apply_intent(
    conn: &rusqlite::Connection,
    proposal_id: &str,
    familiar_id: &str,
    approver: Option<&coven_threads_core::WriterId>,
    files_touched: &[String],
    state: &ProposalApplyingState,
    channel: coven_threads_core::Channel,
    now: time::OffsetDateTime,
) -> Result<()> {
    let detail = serde_json::to_string(state).context("serializing proposal apply intent")?;
    let files_touched = serde_json::to_string(files_touched)?;
    let now = now.format(&time::format_description::well_known::Rfc3339)?;
    store::durable_transaction(conn, || {
        conn.execute(
            "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_version, ward_hash,
            tier, decision, approver, diff_hash, detail, files_touched,
            channel, thread_id, submitted_at, decided_at
         ) VALUES (
            'validation_verdict', ?1, ?2, NULL, ?3, NULL,
            'proposal-apply-intent', ?4, NULL, ?5, ?6, ?7, NULL, ?8, ?8
         )",
            rusqlite::params![
                proposal_id,
                familiar_id,
                state.weave_hash,
                approver.map(coven_threads_core::WriterId::as_str),
                detail,
                files_touched,
                format!("{channel:?}").to_lowercase(),
                now,
            ],
        )
        .context("appending proposal apply intent")?;
        Ok(())
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RecordedProposalApplyIntent {
    pub(super) state: ProposalApplyingState,
    pub(super) approver: Option<String>,
}

#[derive(Debug)]
struct InvalidProposalApplyIntent(String);

impl std::fmt::Display for InvalidProposalApplyIntent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvalidProposalApplyIntent {}

pub(super) fn invalid_proposal_apply_intent(error: &anyhow::Error) -> bool {
    error.downcast_ref::<InvalidProposalApplyIntent>().is_some()
}

pub(super) fn load_proposal_apply_intent(
    conn: &rusqlite::Connection,
    proposal_id: &str,
) -> Result<Option<RecordedProposalApplyIntent>> {
    use rusqlite::OptionalExtension;

    let row: Option<(String, Option<String>)> = match conn
        .query_row(
            "SELECT detail, approver
             FROM ward_audit
             WHERE proposal_id = ?1
               AND event_type = 'validation_verdict'
               AND decision = 'proposal-apply-intent'
             ORDER BY id DESC
             LIMIT 1",
            [proposal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
    {
        Ok(row) => row,
        Err(
            error @ (rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)),
        ) => {
            return Err(InvalidProposalApplyIntent(format!(
                "proposal apply intent columns are invalid: {error}"
            ))
            .into());
        }
        Err(error) => return Err(error).context("loading proposal apply intent"),
    };
    row.map(|(detail, approver)| {
        Ok(RecordedProposalApplyIntent {
            state: serde_json::from_str(&detail).map_err(|error| {
                InvalidProposalApplyIntent(format!(
                    "proposal apply intent detail is invalid: {error}"
                ))
            })?,
            approver,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests;
