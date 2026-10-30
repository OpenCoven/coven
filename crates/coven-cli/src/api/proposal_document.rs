//! The typed, persisted Threads proposal envelope: legacy or scheduled
//! authority plus review metadata and decision progress, its stored form, and
//! its revision. Decoding assumes envelope preflight has already run. Reading
//! files, claims, policy, and every revision comparison stay with the
//! coordinator.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::proposal_preflight::PROPOSAL_ENVELOPE_MAX_BYTES;
use super::proposal_revision::hash_canonical_json;
use super::{ProposalApplyingState, ProposalDecisionRequest};
use crate::ward;

struct PresentField<T>(Option<T>);

impl<T> Default for PresentField<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T> Deserialize<'de> for PresentField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalEnvelopeWire {
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    pending: Option<coven_threads_core::PendingProposal>,
    #[serde(default)]
    classification: Option<coven_threads_core::ProposalClassification>,
    #[serde(default)]
    materialized_diff: Option<coven_threads_core::MaterializedDiff>,
    #[serde(default)]
    region_evidence: Option<Vec<coven_threads_core::RegionEvidence>>,
    #[serde(default)]
    lifecycle: Option<crate::proposal_scheduler::ProposalLifecycle>,
    #[serde(default)]
    veto_deadline: PresentField<Option<time::OffsetDateTime>>,
    #[serde(default)]
    earliest_close: PresentField<Option<time::OffsetDateTime>>,
    #[serde(default)]
    id: Option<coven_threads_core::ProposalId>,
    #[serde(default)]
    familiar_id: Option<coven_threads_core::FamiliarId>,
    #[serde(default)]
    writer: Option<coven_threads_core::WriterId>,
    #[serde(default)]
    channel: Option<coven_threads_core::Channel>,
    #[serde(default)]
    thread_id: Option<coven_threads_core::ThreadId>,
    #[serde(default)]
    fray: Option<coven_threads_core::FrayOrSnap>,
    #[serde(default)]
    edits: Option<Vec<coven_threads_core::StagedEdit>>,
    #[serde(default)]
    staged_at: Option<time::OffsetDateTime>,
    #[serde(rename = "reviewKind", default)]
    review_kind: PresentField<String>,
    #[serde(rename = "identityEvidence", default)]
    identity_evidence: PresentField<[u8; 32]>,
    #[serde(rename = "autoRegressionEvidence", default)]
    auto_regression_evidence: PresentField<[u8; 32]>,
    #[serde(default)]
    probes: PresentField<Box<serde_json::value::RawValue>>,
    #[serde(rename = "decisionRequest", default)]
    decision_request: PresentField<ProposalDecisionRequest>,
    #[serde(rename = "decisionState", default)]
    decision_state: PresentField<ProposalApplyingState>,
}

pub(super) enum ProposalAuthority {
    Legacy(coven_threads_core::PendingProposal),
    Scheduled(Box<crate::proposal_scheduler::ScheduledProposal>),
}

pub(super) struct ProposalEnvelopeDocument {
    pub(super) authority: ProposalAuthority,
    pub(super) review_kind: Option<String>,
    pub(super) identity_evidence: Option<[u8; 32]>,
    pub(super) auto_regression_evidence: Option<[u8; 32]>,
    pub(super) probes: Option<Box<serde_json::value::RawValue>>,
    pub(super) decision_request: Option<ProposalDecisionRequest>,
    pub(super) decision_state: Option<ProposalApplyingState>,
}

#[derive(Serialize)]
struct StoredLegacyProposalRef<'a> {
    #[serde(flatten)]
    pending: &'a coven_threads_core::PendingProposal,
    #[serde(rename = "reviewKind", skip_serializing_if = "Option::is_none")]
    review_kind: Option<&'a String>,
    #[serde(rename = "identityEvidence", skip_serializing_if = "Option::is_none")]
    identity_evidence: Option<[u8; 32]>,
    #[serde(
        rename = "autoRegressionEvidence",
        skip_serializing_if = "Option::is_none"
    )]
    auto_regression_evidence: Option<[u8; 32]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probes: Option<&'a serde_json::value::RawValue>,
    #[serde(rename = "decisionRequest", skip_serializing_if = "Option::is_none")]
    decision_request: Option<&'a ProposalDecisionRequest>,
    #[serde(rename = "decisionState", skip_serializing_if = "Option::is_none")]
    decision_state: Option<&'a ProposalApplyingState>,
}

#[derive(Serialize)]
struct StoredScheduledProposalRef<'a> {
    #[serde(flatten)]
    scheduled: &'a crate::proposal_scheduler::ScheduledProposal,
    #[serde(rename = "reviewKind", skip_serializing_if = "Option::is_none")]
    review_kind: Option<&'a String>,
    #[serde(rename = "identityEvidence", skip_serializing_if = "Option::is_none")]
    identity_evidence: Option<[u8; 32]>,
    #[serde(
        rename = "autoRegressionEvidence",
        skip_serializing_if = "Option::is_none"
    )]
    auto_regression_evidence: Option<[u8; 32]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probes: Option<&'a serde_json::value::RawValue>,
    #[serde(rename = "decisionRequest", skip_serializing_if = "Option::is_none")]
    decision_request: Option<&'a ProposalDecisionRequest>,
    #[serde(rename = "decisionState", skip_serializing_if = "Option::is_none")]
    decision_state: Option<&'a ProposalApplyingState>,
}

impl ProposalEnvelopeDocument {
    pub(super) fn parse_preflighted(raw: &[u8]) -> Result<Self> {
        let wire: ProposalEnvelopeWire =
            serde_json::from_slice(raw).context("parsing typed proposal envelope")?;
        let scheduled_shape = wire.schema.is_some()
            || wire.pending.is_some()
            || wire.classification.is_some()
            || wire.materialized_diff.is_some()
            || wire.region_evidence.is_some()
            || wire.lifecycle.is_some()
            || wire.veto_deadline.0.is_some()
            || wire.earliest_close.0.is_some();

        let authority = if scheduled_shape {
            anyhow::ensure!(
                wire.id.is_none()
                    && wire.familiar_id.is_none()
                    && wire.writer.is_none()
                    && wire.channel.is_none()
                    && wire.thread_id.is_none()
                    && wire.fray.is_none()
                    && wire.edits.is_none(),
                "scheduled proposal mixes legacy authority fields"
            );
            ProposalAuthority::Scheduled(Box::new(
                crate::proposal_scheduler::ScheduledProposal::from_persisted_parts(
                    wire.schema
                        .as_deref()
                        .context("scheduled proposal is missing schema")?,
                    wire.pending
                        .context("scheduled proposal is missing pending")?,
                    wire.classification
                        .context("scheduled proposal is missing classification")?,
                    wire.materialized_diff
                        .context("scheduled proposal is missing materialized_diff")?,
                    wire.region_evidence
                        .context("scheduled proposal is missing region_evidence")?,
                    wire.lifecycle
                        .context("scheduled proposal is missing lifecycle")?,
                    wire.staged_at
                        .context("scheduled proposal is missing staged_at")?,
                    wire.veto_deadline
                        .0
                        .context("scheduled proposal is missing veto_deadline")?,
                    wire.earliest_close
                        .0
                        .context("scheduled proposal is missing earliest_close")?,
                )?,
            ))
        } else {
            ProposalAuthority::Legacy(coven_threads_core::PendingProposal {
                id: wire.id.context("pending proposal is missing id")?,
                familiar_id: wire
                    .familiar_id
                    .context("pending proposal is missing familiar_id")?,
                writer: wire.writer.context("pending proposal is missing writer")?,
                channel: wire
                    .channel
                    .context("pending proposal is missing channel")?,
                thread_id: wire
                    .thread_id
                    .context("pending proposal is missing thread_id")?,
                fray: wire.fray.context("pending proposal is missing fray")?,
                edits: wire.edits.context("pending proposal is missing edits")?,
                staged_at: wire
                    .staged_at
                    .context("pending proposal is missing staged_at")?,
            })
        };
        let review_kind = wire.review_kind.0;
        let probes = wire.probes.0;
        let decision_request = wire.decision_request.0;
        if let Some(request) = decision_request.as_ref() {
            request.validate_replacement_intent()?;
        }
        let decision_state = wire.decision_state.0;
        Ok(Self {
            authority,
            review_kind,
            identity_evidence: wire.identity_evidence.0,
            auto_regression_evidence: wire.auto_regression_evidence.0,
            probes,
            decision_request,
            decision_state,
        })
    }

    pub(super) fn pending(&self) -> &coven_threads_core::PendingProposal {
        match &self.authority {
            ProposalAuthority::Legacy(pending) => pending,
            ProposalAuthority::Scheduled(scheduled) => scheduled.pending(),
        }
    }

    pub(super) fn scheduled(&self) -> Option<&crate::proposal_scheduler::ScheduledProposal> {
        match &self.authority {
            ProposalAuthority::Legacy(_) => None,
            ProposalAuthority::Scheduled(scheduled) => Some(scheduled),
        }
    }

    pub(super) fn serialize_with_decision(
        &self,
        decision_request: Option<&ProposalDecisionRequest>,
        decision_state: Option<&ProposalApplyingState>,
    ) -> Result<Vec<u8>> {
        let bytes = match &self.authority {
            ProposalAuthority::Legacy(pending) => serde_json::to_vec(&StoredLegacyProposalRef {
                pending,
                review_kind: self.review_kind.as_ref(),
                identity_evidence: self.identity_evidence,
                auto_regression_evidence: self.auto_regression_evidence,
                probes: self.probes.as_deref(),
                decision_request,
                decision_state,
            }),
            ProposalAuthority::Scheduled(scheduled) => {
                serde_json::to_vec(&StoredScheduledProposalRef {
                    scheduled,
                    review_kind: self.review_kind.as_ref(),
                    identity_evidence: self.identity_evidence,
                    auto_regression_evidence: self.auto_regression_evidence,
                    probes: self.probes.as_deref(),
                    decision_request,
                    decision_state,
                })
            }
        }
        .context("serializing proposal envelope")?;
        let attempted_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if attempted_bytes > PROPOSAL_ENVELOPE_MAX_BYTES {
            return Err(ward::WardEditBudgetFailure::ProposalEnvelopeBytes {
                attempted_bytes,
                max_bytes: PROPOSAL_ENVELOPE_MAX_BYTES,
            }
            .into());
        }
        Ok(bytes)
    }

    pub(super) fn authority_bytes(&self) -> Result<Vec<u8>> {
        self.serialize_with_decision(None, None)
    }

    pub(super) fn revision(&self) -> Result<String> {
        let authority = self.authority_bytes()?;
        let mut digest = Sha256::new();
        hash_canonical_json(&authority, &mut digest, 0)?;
        let digest = digest.finalize();
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }
}

#[cfg(test)]
mod tests;
