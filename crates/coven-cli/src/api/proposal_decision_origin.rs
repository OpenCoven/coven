//! The origin of a Threads proposal decision: which actor a persisted
//! decision request may be attributed to, trusted only through the
//! server-owned reservation purpose that binds its exact digest, or through a
//! historical apply intent it matches. Every mismatch is an invalid origin or
//! a renewed-review requirement. Claims, recovery and what each origin may do
//! stay with the API coordinator.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::proposal_apply_intent::{invalid_proposal_apply_intent, load_proposal_apply_intent};
use super::{
    HistoricalDecisionReviewRequired, ProposalDecisionRequest, OWNER_LOCAL_DECISION_ACTOR,
};

#[derive(Debug)]
struct InvalidProposalDecisionOrigin(String);

impl std::fmt::Display for InvalidProposalDecisionOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvalidProposalDecisionOrigin {}

pub(super) fn invalid_proposal_decision_origin(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<InvalidProposalDecisionOrigin>()
        .is_some()
}

fn invalid_decision_origin(message: impl Into<String>) -> anyhow::Error {
    InvalidProposalDecisionOrigin(message.into()).into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TrustedProposalDecisionOrigin {
    OwnerLocal(coven_threads_core::WriterId),
    Automatic,
    Expiry,
    Legacy,
    HistoricalRequest,
}

pub(super) const HISTORICAL_UNKNOWN_DECISION_ACTOR: &str = "unknown:historical";

impl TrustedProposalDecisionOrigin {
    pub(super) fn approver(&self) -> Option<&coven_threads_core::WriterId> {
        match self {
            Self::OwnerLocal(approver) => Some(approver),
            Self::Automatic | Self::Expiry | Self::Legacy | Self::HistoricalRequest => None,
        }
    }

    pub(super) fn verify_apply_intent_actor(&self) -> bool {
        matches!(self, Self::OwnerLocal(_) | Self::Automatic)
    }
}

pub(super) fn trusted_decision_approver(
    conn: &rusqlite::Connection,
    proposal_id: &str,
    reservation_decision: &str,
    request: Option<&ProposalDecisionRequest>,
    applying: bool,
) -> Result<TrustedProposalDecisionOrigin> {
    use rusqlite::OptionalExtension;

    let purpose: Option<String> = conn
        .query_row(
            "SELECT purpose
             FROM coven_ward_audit_reservations
             WHERE token = ?1",
            [format!("proposal:{proposal_id}:{reservation_decision}")],
            |row| row.get(0),
        )
        .optional()
        .context("loading proposal decision origin")?;
    let Some(purpose) = purpose else {
        return Err(invalid_decision_origin(
            "proposal decision request has no durable reservation",
        ));
    };
    trusted_decision_approver_for_purpose(
        conn,
        proposal_id,
        reservation_decision,
        request,
        applying,
        &purpose,
    )
}

pub(super) fn trusted_decision_approver_for_purpose(
    conn: &rusqlite::Connection,
    proposal_id: &str,
    reservation_decision: &str,
    request: Option<&ProposalDecisionRequest>,
    applying: bool,
    purpose: &str,
) -> Result<TrustedProposalDecisionOrigin> {
    let Some(request) = request else {
        let legacy_purpose = match reservation_decision {
            "approve" => "proposal-approval",
            "reject" => "proposal-rejection",
            _ => {
                return Err(invalid_decision_origin(
                    "proposal decision reservation has an invalid decision",
                ))
            }
        };
        if purpose != legacy_purpose {
            return Err(invalid_decision_origin(
                "applying proposal without a request lacks compatible legacy authority",
            ));
        }
        if !applying {
            return Err(HistoricalDecisionReviewRequired("no bound historical apply").into());
        }
        return Ok(TrustedProposalDecisionOrigin::Legacy);
    };
    if request.expired {
        if applying {
            return Err(invalid_decision_origin(
                "applying proposal cannot carry an expiry request",
            ));
        }
        let expected_purpose = bound_proposal_decision_purpose("proposal-expiry", request)?;
        if request.decision != "reject"
            || purpose != expected_purpose
            || request.revision_required
            || request.expected_revision.is_some()
            || request.decision_actor.is_some()
        {
            return Err(invalid_decision_origin(
                "proposal expiry does not match its server-owned durable reservation",
            ));
        }
        return Ok(TrustedProposalDecisionOrigin::Expiry);
    }
    let historical_purpose = match reservation_decision {
        "approve" => "proposal-approval",
        "reject" => "proposal-rejection",
        _ => "",
    };
    if purpose == historical_purpose
        && request.decision_actor.is_none()
        && request.replacement_proposal_id.is_none()
        && request.replacement_proposal_revision.is_none()
    {
        // Main 0f861716 and the pre-publication v3 producer persisted requests
        // before origin hashes existed. Their append-only apply intent, not
        // the proposal writer, proves the historical operation being resumed.
        if !applying || request.decision != "approve" || request.decision != reservation_decision {
            return Err(HistoricalDecisionReviewRequired("no bound historical apply").into());
        }
        let intent = match load_proposal_apply_intent(conn, proposal_id) {
            Ok(Some(intent)) => intent,
            Ok(None) => return Err(HistoricalDecisionReviewRequired("missing apply intent").into()),
            Err(error) if invalid_proposal_apply_intent(&error) => {
                return Err(HistoricalDecisionReviewRequired("unverifiable apply intent").into());
            }
            Err(error) => return Err(error),
        };
        let decided_at: String = conn
            .query_row(
                "SELECT decided_at FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'validation_verdict'
               AND decision = 'proposal-apply-intent' ORDER BY id DESC LIMIT 1",
                [proposal_id],
                |row| row.get(0),
            )
            .context("loading historical apply time")?;
        let claimed_at = request
            .claimed_at
            .format(&time::format_description::well_known::Rfc3339)?;
        if intent.state.decision != request.decision
            || intent.state.rationale != request.rationale
            || decided_at != claimed_at
        {
            return Err(HistoricalDecisionReviewRequired(
                "request is not bound to the recorded apply",
            )
            .into());
        }
        return Ok(TrustedProposalDecisionOrigin::HistoricalRequest);
    }
    let expected_purpose = match request.decision.as_str() {
        "approve" => (
            "proposal-approval-owner-local",
            "proposal-approval-automatic",
        ),
        "reject" => (
            "proposal-rejection-owner-local",
            "proposal-rejection-automatic",
        ),
        _ => {
            return Err(invalid_decision_origin(
                "proposal decision request has an invalid decision",
            ))
        }
    };
    let manual_purpose = bound_proposal_decision_purpose(expected_purpose.0, request)?;
    let automatic_purpose = bound_proposal_decision_purpose(expected_purpose.1, request)?;
    match purpose {
        purpose if purpose == manual_purpose => {
            if !request.revision_required
                || request.decision_actor.as_deref() != Some(OWNER_LOCAL_DECISION_ACTOR)
            {
                return Err(invalid_decision_origin(
                    "manual proposal decision origin does not match its durable reservation",
                ));
            }
            Ok(TrustedProposalDecisionOrigin::OwnerLocal(
                coven_threads_core::WriterId::new(OWNER_LOCAL_DECISION_ACTOR),
            ))
        }
        purpose if purpose == automatic_purpose => {
            if request.revision_required || request.decision_actor.is_some() {
                return Err(invalid_decision_origin(
                    "automatic proposal decision origin does not match its durable reservation",
                ));
            }
            Ok(TrustedProposalDecisionOrigin::Automatic)
        }
        _ => Err(invalid_decision_origin(
            "proposal decision reservation purpose is invalid",
        )),
    }
}

pub(super) fn bound_proposal_decision_purpose(
    purpose: &str,
    request: &ProposalDecisionRequest,
) -> Result<String> {
    let request = serde_json::to_vec(request).context("serializing proposal decision request")?;
    let digest = Sha256::digest(request);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(format!("{purpose}:{encoded}"))
}

#[cfg(test)]
mod tests;
