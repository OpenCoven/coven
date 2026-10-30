//! The persisted envelope is the coordinator's only record of a proposal's
//! authority and decision progress; these pin how it decodes, which fields its
//! revision covers, and how it refuses ambiguous or incomplete records.
use super::*;

fn surface() -> coven_threads_core::SurfaceId {
    coven_threads_core::SurfaceId::new("TOOLS.md")
}

fn staged_at() -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn pending() -> coven_threads_core::PendingProposal {
    coven_threads_core::PendingProposal {
        id: coven_threads_core::ProposalId::new(),
        familiar_id: coven_threads_core::FamiliarId::new(),
        writer: coven_threads_core::WriterId::new("principal:fpr-val"),
        channel: coven_threads_core::Channel::Mutation,
        thread_id: coven_threads_core::ThreadId::new(),
        fray: coven_threads_core::FrayOrSnap::Frayed {
            strand: None,
            channel: coven_threads_core::Channel::Mutation,
            reason: coven_threads_core::FrayReason::Other("document fixture".to_string()),
        },
        edits: vec![coven_threads_core::StagedEdit {
            surface: surface(),
            contents: coven_threads_core::StagedContents::from_bytes(b"after"),
        }],
        staged_at: staged_at(),
    }
}

fn scheduled() -> crate::proposal_scheduler::ScheduledProposal {
    let pending = pending();
    let diff =
        coven_threads_core::MaterializedDiff::try_new(vec![coven_threads_core::SurfaceDiff {
            surface: surface(),
            before: Some(b"before".to_vec()),
            after: Some(b"after".to_vec()),
        }])
        .unwrap();
    let evidence =
        coven_threads_core::SurfaceRegionRegistry::default_registry().classify_all(&diff);
    let classification = coven_threads_core::ProposalClassification {
        proposal_id: pending.id,
        familiar_id: pending.familiar_id,
        channel: coven_threads_core::Channel::Mutation,
        affected_surfaces: vec![surface()],
        affected_regions: evidence.iter().map(|item| item.region_id.clone()).collect(),
        path_tier_floor: 1,
        approval_path: coven_threads_core::ApprovalPath::HumanApproval,
        evidence_replay_hash: coven_threads_core::evidence_replay_hash(&diff, &evidence),
        classified_at: staged_at(),
    };
    crate::proposal_scheduler::ScheduledProposal::try_new(pending, classification, diff).unwrap()
}

fn object<T: serde::Serialize>(value: &T) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::to_value(value).unwrap() {
        serde_json::Value::Object(object) => object,
        other => panic!("fixture is not an object: {other}"),
    }
}

fn parse(object: &serde_json::Map<String, serde_json::Value>) -> Result<ProposalEnvelopeDocument> {
    ProposalEnvelopeDocument::parse_preflighted(&serde_json::to_vec(object).unwrap())
}

fn decision_request() -> ProposalDecisionRequest {
    serde_json::from_value(serde_json::json!({
        "decision": "approve",
        "rationale": "looks right",
        "claimedAt": staged_at(),
    }))
    .unwrap()
}

fn applying_state() -> ProposalApplyingState {
    serde_json::from_value(serde_json::json!({
        "decision": "approve",
        "recoveryCommitment": [1, 2, 3],
        "weaveHash": [4, 5, 6],
        "beforeImages": [],
        "rationale": null,
    }))
    .unwrap()
}

fn error(object: &serde_json::Map<String, serde_json::Value>) -> String {
    format!(
        "{:#}",
        parse(object).err().expect("envelope must be refused")
    )
}

#[test]
fn proposal_document_decodes_legacy_and_scheduled_authority_by_shape() {
    let legacy = pending();
    let document = parse(&object(&legacy)).unwrap();
    assert!(document.scheduled().is_none());
    assert_eq!(document.pending(), &legacy);

    let scheduled = scheduled();
    let document = parse(&object(&scheduled)).unwrap();
    assert_eq!(document.scheduled(), Some(&scheduled));
    assert_eq!(document.pending(), scheduled.pending());
}

#[test]
fn proposal_document_round_trips_authority_bytes_with_a_stable_revision() {
    for raw in [object(&pending()), object(&scheduled())] {
        let document = parse(&raw).unwrap();
        let revision = document.revision().unwrap();
        assert_eq!(revision.len(), 64);
        assert!(revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        let restored =
            ProposalEnvelopeDocument::parse_preflighted(&document.authority_bytes().unwrap())
                .unwrap();
        assert_eq!(restored.revision().unwrap(), revision);
        assert_eq!(
            restored.authority_bytes().unwrap(),
            document.authority_bytes().unwrap()
        );
    }
}

#[test]
fn proposal_document_revision_excludes_decision_progress_but_covers_review_metadata() {
    for raw in [object(&pending()), object(&scheduled())] {
        let document = parse(&raw).unwrap();
        let revision = document.revision().unwrap();

        let progressed = ProposalEnvelopeDocument::parse_preflighted(
            &document
                .serialize_with_decision(Some(&decision_request()), Some(&applying_state()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(progressed.decision_request, Some(decision_request()));
        assert_eq!(progressed.decision_state, Some(applying_state()));
        assert_eq!(progressed.revision().unwrap(), revision);

        let mut reviewed = raw.clone();
        reviewed.insert("reviewKind".into(), "coherence".into());
        reviewed.insert(
            "identityEvidence".into(),
            serde_json::to_value([7_u8; 32]).unwrap(),
        );
        let reviewed = parse(&reviewed).unwrap();
        assert_eq!(reviewed.review_kind.as_deref(), Some("coherence"));
        assert_eq!(reviewed.identity_evidence, Some([7; 32]));
        assert_ne!(reviewed.revision().unwrap(), revision);
    }
}

#[test]
fn proposal_document_refuses_mixed_or_incomplete_authority() {
    let mut mixed = object(&scheduled());
    mixed.insert("writer".into(), "principal:other".into());
    assert!(error(&mixed).contains("scheduled proposal mixes legacy authority fields"));

    let mut legacy_with_schema = object(&pending());
    legacy_with_schema.insert("schema".into(), "anything".into());
    assert!(error(&legacy_with_schema).contains("scheduled proposal mixes legacy authority fields"));

    for (field, message) in [
        ("writer", "pending proposal is missing writer"),
        ("staged_at", "pending proposal is missing staged_at"),
    ] {
        let mut missing = object(&pending());
        missing.remove(field);
        assert!(
            error(&missing).contains(message),
            "{field}: {}",
            error(&missing)
        );
    }
    for (field, message) in [
        ("lifecycle", "scheduled proposal is missing lifecycle"),
        (
            "veto_deadline",
            "scheduled proposal is missing veto_deadline",
        ),
        (
            "earliest_close",
            "scheduled proposal is missing earliest_close",
        ),
    ] {
        let mut missing = object(&scheduled());
        missing.remove(field);
        assert!(
            error(&missing).contains(message),
            "{field}: {}",
            error(&missing)
        );
    }
}

#[test]
fn proposal_document_distinguishes_present_null_from_absent_fields() {
    let mut raw = object(&scheduled());
    assert_eq!(raw.get("veto_deadline"), Some(&serde_json::Value::Null));
    assert!(parse(&raw).is_ok(), "a present null deadline is a value");
    raw.remove("veto_deadline");
    assert!(parse(&raw).is_err(), "an absent deadline is missing");

    let mut null_review = object(&pending());
    null_review.insert("reviewKind".into(), serde_json::Value::Null);
    assert!(error(&null_review).contains("invalid type: null"));
}

#[test]
fn proposal_document_refuses_unknown_fields_and_incomplete_replacement_intent() {
    let mut unknown = object(&pending());
    unknown.insert("unexpected".into(), true.into());
    assert!(error(&unknown).contains("unknown field `unexpected`"));

    let mut request = serde_json::to_value(decision_request()).unwrap();
    request["decision"] = "reject".into();
    request["replacementProposalId"] = serde_json::json!(uuid::Uuid::nil());
    let mut incomplete = object(&pending());
    incomplete.insert("decisionRequest".into(), request.clone());
    assert!(error(&incomplete).contains("incomplete persisted replacement intent"));

    request["replacementProposalRevision"] = "f".repeat(64).into();
    let mut complete = object(&pending());
    complete.insert("decisionRequest".into(), request);
    assert!(parse(&complete).unwrap().decision_request.is_some());
}
