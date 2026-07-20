use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use coven_threads_core as threads;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProposalEnvelopeSchema {
    Phase5V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ProposalLifecycle {
    AwaitingHumanApproval,
    VetoWindowOpen,
    ReadyForReplay,
    Blocked { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ScheduledProposal {
    schema: ProposalEnvelopeSchema,
    pending: threads::PendingProposal,
    classification: threads::ProposalClassification,
    materialized_diff: threads::MaterializedDiff,
    region_evidence: Vec<threads::RegionEvidence>,
    lifecycle: ProposalLifecycle,
    staged_at: OffsetDateTime,
    veto_deadline: Option<OffsetDateTime>,
    earliest_close: Option<OffsetDateTime>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduledProposalWire {
    schema: ProposalEnvelopeSchema,
    pending: threads::PendingProposal,
    classification: threads::ProposalClassification,
    materialized_diff: threads::MaterializedDiff,
    region_evidence: Vec<threads::RegionEvidence>,
    lifecycle: ProposalLifecycle,
    staged_at: OffsetDateTime,
    veto_deadline: Option<OffsetDateTime>,
    earliest_close: Option<OffsetDateTime>,
}

impl ScheduledProposal {
    pub(crate) fn try_new(
        pending: threads::PendingProposal,
        classification: threads::ProposalClassification,
        materialized_diff: threads::MaterializedDiff,
    ) -> Result<Self> {
        if pending.id != classification.proposal_id {
            bail!("pending proposal id does not match classification");
        }
        if pending.familiar_id != classification.familiar_id {
            bail!("pending familiar id does not match classification");
        }
        if pending.channel != classification.channel {
            bail!("pending channel does not match classification");
        }
        if classification.path_tier_floor == 0 {
            bail!("protected surfaces are not schedulable through ApprovalPath");
        }
        if classification.path_tier_floor > 3 {
            bail!("classification path tier floor is outside the Ward tier range");
        }
        if classification.path_tier_floor == 1
            && matches!(
                &classification.approval_path,
                threads::ApprovalPath::AutoRegression { .. }
            )
        {
            bail!("reviewed tier requires familiar review or a stronger approval path");
        }

        let region_evidence =
            threads::SurfaceRegionRegistry::default_registry().classify_all(&materialized_diff);
        validate_materialized_edits(&pending, &classification, &materialized_diff)?;
        validate_region_evidence(&classification, &materialized_diff, &region_evidence)?;

        let replay_hash = threads::evidence_replay_hash(&materialized_diff, &region_evidence);
        if replay_hash != classification.evidence_replay_hash {
            bail!("classification replay hash does not commit the materialized evidence");
        }

        let staged_at = pending.staged_at;
        let (lifecycle, veto_deadline, earliest_close) =
            match classification.approval_path.veto_window() {
                Some(window) => (
                    ProposalLifecycle::VetoWindowOpen,
                    Some(
                        window
                            .deadline(staged_at)
                            .map_err(anyhow::Error::msg)
                            .context("deriving veto deadline")?,
                    ),
                    Some(
                        window
                            .earliest_close(staged_at)
                            .map_err(anyhow::Error::msg)
                            .context("deriving minimum visibility deadline")?,
                    ),
                ),
                None => match classification.approval_path {
                    threads::ApprovalPath::AutoRegression { .. } => {
                        (ProposalLifecycle::ReadyForReplay, None, None)
                    }
                    threads::ApprovalPath::HumanApproval
                    | threads::ApprovalPath::HumanApprovalWithRationale => {
                        (ProposalLifecycle::AwaitingHumanApproval, None, None)
                    }
                    threads::ApprovalPath::FamiliarCoherence { .. } => {
                        unreachable!("familiar coherence always has a veto window")
                    }
                },
            };

        Ok(Self {
            schema: ProposalEnvelopeSchema::Phase5V1,
            pending,
            classification,
            materialized_diff,
            region_evidence,
            lifecycle,
            staged_at,
            veto_deadline,
            earliest_close,
        })
    }

    pub(crate) fn lifecycle(&self) -> &ProposalLifecycle {
        &self.lifecycle
    }

    pub(crate) fn pending(&self) -> &threads::PendingProposal {
        &self.pending
    }

    pub(crate) fn classification(&self) -> &threads::ProposalClassification {
        &self.classification
    }

    pub(crate) fn materialized_diff(&self) -> &threads::MaterializedDiff {
        &self.materialized_diff
    }

    pub(crate) fn earliest_close(&self) -> Option<OffsetDateTime> {
        self.earliest_close
    }

    pub(crate) fn veto_deadline(&self) -> Option<OffsetDateTime> {
        self.veto_deadline
    }
}

impl TryFrom<ScheduledProposalWire> for ScheduledProposal {
    type Error = anyhow::Error;

    fn try_from(wire: ScheduledProposalWire) -> Result<Self> {
        let expected = Self::try_new(wire.pending, wire.classification, wire.materialized_diff)?;
        if wire.region_evidence != expected.region_evidence {
            bail!("persisted region evidence does not match daemon predicate replay");
        }
        if wire.schema != expected.schema
            || wire.staged_at != expected.staged_at
            || wire.lifecycle != expected.lifecycle
            || wire.veto_deadline != expected.veto_deadline
            || wire.earliest_close != expected.earliest_close
        {
            bail!("scheduled proposal carries inconsistent derived lifecycle state");
        }
        Ok(expected)
    }
}

impl<'de> Deserialize<'de> for ScheduledProposal {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        validate_scheduled_json_shape(&value).map_err(serde::de::Error::custom)?;
        serde_json::from_value::<ScheduledProposalWire>(value)
            .map_err(serde::de::Error::custom)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

fn validate_materialized_edits(
    pending: &threads::PendingProposal,
    classification: &threads::ProposalClassification,
    materialized_diff: &threads::MaterializedDiff,
) -> Result<()> {
    let mut staged_after = BTreeMap::new();
    for edit in &pending.edits {
        let surface = edit.surface.as_str().to_string();
        let after = edit
            .contents
            .to_bytes()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("decoding staged surface {surface}"))?;
        if staged_after.insert(surface.clone(), after).is_some() {
            bail!("pending proposal contains duplicate surface {surface}");
        }
    }

    let mut diff_after = BTreeMap::new();
    for surface in materialized_diff.surfaces() {
        let name = surface.surface.as_str().to_string();
        let Some(after) = &surface.after else {
            bail!("pending proposal cannot schedule deletion of surface {name}");
        };
        if diff_after.insert(name.clone(), after.clone()).is_some() {
            bail!("materialized diff contains duplicate surface {name}");
        }
    }
    if staged_after != diff_after {
        bail!("pending edits do not match the complete materialized diff");
    }

    let affected: BTreeSet<String> = classification
        .affected_surfaces
        .iter()
        .map(|surface| surface.as_str().to_string())
        .collect();
    if affected.len() != classification.affected_surfaces.len() {
        bail!("classification contains duplicate affected surfaces");
    }
    if affected != diff_after.keys().cloned().collect() {
        bail!("classification affected surfaces do not match the materialized diff");
    }
    Ok(())
}

fn validate_region_evidence(
    classification: &threads::ProposalClassification,
    materialized_diff: &threads::MaterializedDiff,
    region_evidence: &[threads::RegionEvidence],
) -> Result<()> {
    let classified_regions: BTreeSet<String> = classification
        .affected_regions
        .iter()
        .map(|region| region.as_str().to_string())
        .collect();
    if classified_regions.len() != classification.affected_regions.len() {
        bail!("classification contains duplicate affected regions");
    }

    let mut evidenced_regions = BTreeSet::new();
    let diff_surfaces: BTreeSet<&str> = materialized_diff
        .surfaces()
        .iter()
        .map(|surface| surface.surface.as_str())
        .collect();
    for evidence in region_evidence {
        if !evidenced_regions.insert(evidence.region_id.as_str().to_string()) {
            bail!(
                "region evidence contains duplicate region {}",
                evidence.region_id
            );
        }
        if evidence
            .affected_surfaces
            .iter()
            .any(|surface| !diff_surfaces.contains(surface.as_str()))
        {
            bail!(
                "region {} references a surface outside the materialized diff",
                evidence.region_id
            );
        }
    }
    if classified_regions != evidenced_regions {
        bail!("classification affected regions do not match region evidence");
    }
    let region_floor = threads::SurfaceRegionRegistry::path_tier_floor(region_evidence);
    if classification.path_tier_floor > region_floor {
        bail!("classification path tier floor is weaker than region evidence");
    }
    Ok(())
}

fn validate_scheduled_json_shape(value: &Value) -> Result<()> {
    let root = strict_object(
        value,
        "scheduled proposal",
        &[
            "schema",
            "pending",
            "classification",
            "materialized_diff",
            "region_evidence",
            "lifecycle",
            "staged_at",
            "veto_deadline",
            "earliest_close",
        ],
    )?;
    let pending = required_object(root, "pending", "pending proposal")?;
    reject_unknown_keys(
        pending,
        "pending proposal",
        &[
            "id",
            "familiar_id",
            "writer",
            "channel",
            "thread_id",
            "fray",
            "edits",
            "staged_at",
        ],
    )?;
    validate_fray(
        pending
            .get("fray")
            .ok_or_else(|| anyhow::anyhow!("pending proposal is missing fray"))?,
    )?;
    let edits = pending
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("pending proposal edits must be an array"))?;
    for edit in edits {
        let edit = strict_object(edit, "pending edit", &["surface", "contents"])?;
        strict_object(
            edit.get("contents")
                .ok_or_else(|| anyhow::anyhow!("pending edit is missing contents"))?,
            "staged contents",
            &["encoding", "data"],
        )?;
    }

    let classification = required_object(root, "classification", "proposal classification")?;
    reject_unknown_keys(
        classification,
        "proposal classification",
        &[
            "proposal_id",
            "familiar_id",
            "channel",
            "affected_surfaces",
            "affected_regions",
            "path_tier_floor",
            "approval_path",
            "evidence_replay_hash",
            "classified_at",
        ],
    )?;
    let approval_path = required_object(classification, "approval_path", "approval path")?;
    let approval_kind = approval_path
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("approval path is missing kind"))?;
    match approval_kind {
        "auto_regression" => {
            reject_unknown_keys(approval_path, "approval path", &["kind", "veto"])?;
        }
        "familiar_coherence" => {
            reject_unknown_keys(approval_path, "approval path", &["kind", "veto"])?;
            if approval_path.get("veto").is_none_or(Value::is_null) {
                bail!("familiar coherence approval path requires veto");
            }
        }
        "human_approval" | "human_approval_with_rationale" => {
            reject_unknown_keys(approval_path, "approval path", &["kind"])?;
        }
        _ => bail!("approval path contains unknown kind {approval_kind}"),
    }
    if let Some(veto) = approval_path.get("veto").filter(|value| !value.is_null()) {
        let veto = strict_object(veto, "veto window", &["duration", "min_visible"])?;
        for field in ["duration", "min_visible"] {
            strict_object(
                veto.get(field)
                    .ok_or_else(|| anyhow::anyhow!("veto window is missing {field}"))?,
                field,
                &["secs", "nanos"],
            )?;
        }
    }

    let diff = required_object(root, "materialized_diff", "materialized diff")?;
    reject_unknown_keys(diff, "materialized diff", &["surfaces"])?;
    let surfaces = diff
        .get("surfaces")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("materialized diff surfaces must be an array"))?;
    for surface in surfaces {
        strict_object(
            surface,
            "materialized surface",
            &["surface", "before", "after"],
        )?;
    }

    let evidence = root
        .get("region_evidence")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("region evidence must be an array"))?;
    for item in evidence {
        strict_object(
            item,
            "region evidence",
            &[
                "region_id",
                "affected_surfaces",
                "min_path_tier",
                "replay_bytes",
                "rationale",
            ],
        )?;
    }
    let lifecycle = strict_object(
        root.get("lifecycle")
            .ok_or_else(|| anyhow::anyhow!("scheduled proposal is missing lifecycle"))?,
        "proposal lifecycle",
        &["state", "reason"],
    )?;
    let lifecycle_state = lifecycle
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("proposal lifecycle is missing state"))?;
    match lifecycle_state {
        "awaiting_human_approval" | "veto_window_open" | "ready_for_replay" => {
            reject_unknown_keys(lifecycle, "proposal lifecycle", &["state"])?;
        }
        "blocked" => {
            reject_unknown_keys(lifecycle, "proposal lifecycle", &["state", "reason"])?;
            if lifecycle
                .get("reason")
                .and_then(Value::as_str)
                .is_none_or(|reason| reason.trim().is_empty())
            {
                bail!("blocked proposal lifecycle requires a nonblank reason");
            }
        }
        _ => bail!("proposal lifecycle contains unknown state {lifecycle_state}"),
    }
    Ok(())
}

fn validate_fray(value: &Value) -> Result<()> {
    let fray = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("pending fray must be an object"))?;
    if fray.len() != 1 {
        bail!("pending fray must contain exactly one variant");
    }
    let (variant, payload) = fray.iter().next().expect("length checked");
    let allowed = match variant.as_str() {
        "NotCovered" => &["channel"][..],
        "Frayed" => &["strand", "channel", "reason"][..],
        "Snapped" => &["channel", "reason"][..],
        _ => bail!("pending fray contains unknown variant {variant}"),
    };
    strict_object(payload, "pending fray payload", allowed)?;
    Ok(())
}

fn required_object<'a>(
    parent: &'a Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a Map<String, Value>> {
    parent
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("{label} must be an object"))
}

fn strict_object<'a>(
    value: &'a Value,
    label: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{label} must be an object"))?;
    reject_unknown_keys(object, label, allowed)?;
    Ok(object)
}

fn reject_unknown_keys(object: &Map<String, Value>, label: &str, allowed: &[&str]) -> Result<()> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("{label} contains unknown field {key}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use coven_threads_core as threads;

    use super::*;

    fn fixture(
        approval_path: threads::ApprovalPath,
    ) -> (
        threads::PendingProposal,
        threads::ProposalClassification,
        threads::MaterializedDiff,
        Vec<threads::RegionEvidence>,
    ) {
        fixture_for_surface(approval_path, "TOOLS.md", 1)
    }

    fn fixture_for_surface(
        approval_path: threads::ApprovalPath,
        surface: &str,
        path_tier_floor: u8,
    ) -> (
        threads::PendingProposal,
        threads::ProposalClassification,
        threads::MaterializedDiff,
        Vec<threads::RegionEvidence>,
    ) {
        let proposal_id = threads::ProposalId::new();
        let familiar_id = threads::FamiliarId::new();
        let surface = threads::SurfaceId::new(surface);
        let staged_at = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let pending = threads::PendingProposal {
            id: proposal_id,
            familiar_id,
            writer: threads::WriterId::new("principal:fpr-val"),
            channel: threads::Channel::Mutation,
            thread_id: threads::ThreadId::new(),
            fray: threads::FrayOrSnap::Frayed {
                strand: None,
                channel: threads::Channel::Mutation,
                reason: threads::FrayReason::Other("phase-5 test".to_string()),
            },
            edits: vec![threads::StagedEdit {
                surface: surface.clone(),
                contents: threads::StagedContents::from_bytes(b"after"),
            }],
            staged_at,
        };
        let diff = threads::MaterializedDiff::try_new(vec![threads::SurfaceDiff {
            surface: surface.clone(),
            before: Some(b"before".to_vec()),
            after: Some(b"after".to_vec()),
        }])
        .unwrap();
        let evidence = threads::SurfaceRegionRegistry::default_registry().classify_all(&diff);
        let classification = threads::ProposalClassification {
            proposal_id,
            familiar_id,
            channel: threads::Channel::Mutation,
            affected_surfaces: vec![surface],
            affected_regions: evidence.iter().map(|item| item.region_id.clone()).collect(),
            path_tier_floor,
            approval_path,
            evidence_replay_hash: threads::evidence_replay_hash(&diff, &evidence),
            classified_at: staged_at,
        };
        (pending, classification, diff, evidence)
    }

    #[test]
    fn human_proposal_roundtrips_as_awaiting_approval() {
        let (pending, classification, diff, _) = fixture(threads::ApprovalPath::HumanApproval);

        let scheduled = ScheduledProposal::try_new(pending, classification, diff).unwrap();
        let encoded = serde_json::to_string(&scheduled).unwrap();
        let decoded: ScheduledProposal = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, scheduled);
        assert_eq!(
            decoded.lifecycle(),
            &ProposalLifecycle::AwaitingHumanApproval
        );
        assert_eq!(decoded.veto_deadline, None);
        assert_eq!(decoded.earliest_close(), None);
    }

    #[test]
    fn veto_window_derives_deadline_and_minimum_visibility() {
        let window = threads::VetoWindow::new(Duration::from_secs(300), Duration::from_secs(60));
        let (pending, classification, diff, _) =
            fixture(threads::ApprovalPath::FamiliarCoherence { veto: window });
        let staged_at = pending.staged_at;

        let scheduled = ScheduledProposal::try_new(pending, classification, diff).unwrap();

        assert_eq!(scheduled.lifecycle(), &ProposalLifecycle::VetoWindowOpen);
        assert_eq!(
            scheduled.veto_deadline,
            Some(staged_at + time::Duration::seconds(300))
        );
        assert_eq!(
            scheduled.earliest_close(),
            Some(staged_at + time::Duration::seconds(60))
        );
    }

    #[test]
    fn protected_path_floor_is_not_schedulable() {
        let (pending, mut classification, diff, _) = fixture(threads::ApprovalPath::HumanApproval);
        classification.path_tier_floor = 0;

        let error = ScheduledProposal::try_new(pending, classification, diff)
            .expect_err("protected proposals must stay outside ApprovalPath");

        assert!(error.to_string().contains("protected"));
    }

    #[test]
    fn replay_hash_mismatch_fails_closed() {
        let (pending, mut classification, diff, _) = fixture(threads::ApprovalPath::HumanApproval);
        classification.evidence_replay_hash = [0xff; 32];

        let error = ScheduledProposal::try_new(pending, classification, diff)
            .expect_err("uncommitted evidence must be rejected");

        assert!(error.to_string().contains("replay hash"));
    }

    #[test]
    fn region_tier_zero_cannot_be_hidden_by_classification() {
        let (pending, classification, diff, _) =
            fixture_for_surface(threads::ApprovalPath::HumanApproval, "AGENTS.md", 1);

        let error = ScheduledProposal::try_new(pending, classification, diff)
            .expect_err("region protection must constrain the classification floor");

        assert!(error.to_string().contains("weaker than region evidence"));
    }

    #[test]
    fn deserialization_replays_region_predicates_instead_of_trusting_evidence() {
        let (pending, classification, diff, mut forged_evidence) =
            fixture(threads::ApprovalPath::HumanApproval);
        let scheduled = ScheduledProposal::try_new(pending, classification, diff.clone()).unwrap();
        forged_evidence[0].min_path_tier = 0;
        let mut value = serde_json::to_value(scheduled).unwrap();
        value["region_evidence"] = serde_json::to_value(&forged_evidence).unwrap();
        value["classification"]["evidence_replay_hash"] =
            serde_json::to_value(threads::evidence_replay_hash(&diff, &forged_evidence)).unwrap();

        let error = serde_json::from_value::<ScheduledProposal>(value)
            .expect_err("persisted evidence must match daemon predicate replay");

        assert!(
            error.to_string().contains("replay hash")
                || error.to_string().contains("daemon predicate replay")
        );
    }

    #[test]
    fn reviewed_tier_cannot_use_auto_approval() {
        let (pending, classification, diff, _) =
            fixture(threads::ApprovalPath::AutoRegression { veto: None });

        let error = ScheduledProposal::try_new(pending, classification, diff)
            .expect_err("tier one requires familiar review or stronger");

        assert!(error.to_string().contains("reviewed tier"));
    }

    #[test]
    fn deserialization_rejects_unknown_fields() {
        let (pending, classification, diff, _) = fixture(threads::ApprovalPath::HumanApproval);
        let scheduled = ScheduledProposal::try_new(pending, classification, diff).unwrap();
        let mut value = serde_json::to_value(scheduled).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("clientPolicy".to_string(), serde_json::json!("auto"));

        let error = serde_json::from_value::<ScheduledProposal>(value)
            .expect_err("clients cannot add policy-bearing fields");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn deserialization_rejects_unknown_nested_policy_fields() {
        let (pending, classification, diff, _) = fixture(threads::ApprovalPath::HumanApproval);
        let scheduled = ScheduledProposal::try_new(pending, classification, diff).unwrap();
        let mut value = serde_json::to_value(scheduled).unwrap();
        value["classification"]
            .as_object_mut()
            .unwrap()
            .insert("clientPolicy".to_string(), serde_json::json!("auto"));

        let error = serde_json::from_value::<ScheduledProposal>(value)
            .expect_err("nested policy fields must be rejected");

        assert!(error.to_string().contains("unknown field clientPolicy"));
    }

    #[test]
    fn deserialization_rejects_fields_from_the_wrong_policy_variant() {
        let (pending, classification, diff, _) = fixture(threads::ApprovalPath::HumanApproval);
        let scheduled = ScheduledProposal::try_new(pending, classification, diff).unwrap();
        let mut approval_value = serde_json::to_value(&scheduled).unwrap();
        approval_value["classification"]["approval_path"]["veto"] =
            serde_json::json!({"duration":{"secs":1,"nanos":0},"min_visible":{"secs":1,"nanos":0}});
        let approval_error = serde_json::from_value::<ScheduledProposal>(approval_value)
            .expect_err("human approval cannot carry veto settings");
        assert!(approval_error.to_string().contains("unknown field veto"));

        let mut lifecycle_value = serde_json::to_value(&scheduled).unwrap();
        lifecycle_value["lifecycle"]["reason"] = serde_json::json!("forged");
        let lifecycle_error = serde_json::from_value::<ScheduledProposal>(lifecycle_value)
            .expect_err("non-blocked lifecycle cannot carry a blocked reason");
        assert!(lifecycle_error.to_string().contains("unknown field reason"));
    }
}
