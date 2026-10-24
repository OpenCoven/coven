//! A persisted decision request is trusted only through the server-owned
//! reservation purpose written before it; these pin how each purpose maps to
//! an origin and how every mismatch fails closed.
use super::*;
use crate::api::proposal_apply_intent::{append_proposal_apply_intent, ProposalApplyingState};

const PROPOSAL: &str = "p-1";

fn store() -> (tempfile::TempDir, rusqlite::Connection) {
    let home = tempfile::tempdir().unwrap();
    let conn = crate::store::open_store(&crate::api::store_path(home.path())).unwrap();
    (home, conn)
}

fn reserve(conn: &rusqlite::Connection, decision: &str, purpose: &str) {
    conn.execute(
        "INSERT INTO coven_ward_audit_reservations (token, purpose, reserved_bytes)
         VALUES (?1, ?2, 0)",
        rusqlite::params![format!("proposal:{PROPOSAL}:{decision}"), purpose],
    )
    .unwrap();
}

fn claimed_at() -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn request(fields: serde_json::Value) -> ProposalDecisionRequest {
    let mut value = serde_json::json!({
        "decision": "approve",
        "rationale": "looks right",
        "claimedAt": claimed_at(),
    });
    for (key, field) in fields.as_object().unwrap() {
        value[key] = field.clone();
    }
    serde_json::from_value(value).unwrap()
}

fn origin(
    conn: &rusqlite::Connection,
    decision: &str,
    request: Option<&ProposalDecisionRequest>,
    applying: bool,
) -> Result<TrustedProposalDecisionOrigin> {
    trusted_decision_approver(conn, PROPOSAL, decision, request, applying)
}

fn refused(result: Result<TrustedProposalDecisionOrigin>, message: &str) {
    let error = result.expect_err(message);
    assert!(invalid_proposal_decision_origin(&error), "{error:#}");
    assert!(error.to_string().contains(message), "{error:#}");
}

fn needs_review(result: Result<TrustedProposalDecisionOrigin>, reason: &str) {
    let error = result.expect_err(reason);
    let review = error
        .downcast_ref::<HistoricalDecisionReviewRequired>()
        .unwrap_or_else(|| panic!("not a review requirement: {error:#}"));
    assert_eq!(review.0, reason);
    assert!(!invalid_proposal_decision_origin(&error));
}

#[test]
fn decision_origin_purposes_bind_the_exact_request_digest() {
    let request = request(serde_json::json!({}));
    let purpose = bound_proposal_decision_purpose("proposal-approval-automatic", &request).unwrap();
    let digest: String = Sha256::digest(serde_json::to_vec(&request).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(purpose, format!("proposal-approval-automatic:{digest}"));
    let other = self::request(serde_json::json!({"rationale": "changed"}));
    assert_ne!(
        bound_proposal_decision_purpose("proposal-approval-automatic", &other).unwrap(),
        purpose
    );
}

#[test]
fn decision_origin_requires_a_durable_reservation() {
    let (_home, conn) = store();
    refused(
        origin(
            &conn,
            "approve",
            Some(&request(serde_json::json!({}))),
            true,
        ),
        "proposal decision request has no durable reservation",
    );
}

#[test]
fn decision_origin_trusts_owner_local_and_automatic_purposes_only_as_bound() {
    let (_home, conn) = store();
    let manual = request(serde_json::json!({
        "revisionRequired": true,
        "decisionActor": OWNER_LOCAL_DECISION_ACTOR,
    }));
    reserve(
        &conn,
        "approve",
        &bound_proposal_decision_purpose("proposal-approval-owner-local", &manual).unwrap(),
    );
    let trusted = origin(&conn, "approve", Some(&manual), true).unwrap();
    assert_eq!(
        trusted,
        TrustedProposalDecisionOrigin::OwnerLocal(coven_threads_core::WriterId::new(
            OWNER_LOCAL_DECISION_ACTOR
        ))
    );
    assert_eq!(
        trusted.approver().map(|id| id.as_str()),
        Some(OWNER_LOCAL_DECISION_ACTOR)
    );
    assert!(trusted.verify_apply_intent_actor());
    // The same request bytes cannot claim the automatic purpose.
    let automatic_purpose =
        bound_proposal_decision_purpose("proposal-approval-automatic", &manual).unwrap();
    assert!(origin_for(&conn, Some(&manual), &automatic_purpose).is_err());

    let automatic = request(serde_json::json!({}));
    let purpose =
        bound_proposal_decision_purpose("proposal-approval-automatic", &automatic).unwrap();
    let trusted = origin_for(&conn, Some(&automatic), &purpose).unwrap();
    assert_eq!(trusted, TrustedProposalDecisionOrigin::Automatic);
    assert_eq!(trusted.approver(), None);
    assert!(trusted.verify_apply_intent_actor());

    let actor_on_automatic = request(serde_json::json!({"decisionActor": "someone"}));
    let purpose =
        bound_proposal_decision_purpose("proposal-approval-automatic", &actor_on_automatic)
            .unwrap();
    refused(
        origin_for(&conn, Some(&actor_on_automatic), &purpose),
        "automatic proposal decision origin does not match its durable reservation",
    );
    let unbound_manual = request(serde_json::json!({"decisionActor": OWNER_LOCAL_DECISION_ACTOR}));
    let purpose =
        bound_proposal_decision_purpose("proposal-approval-owner-local", &unbound_manual).unwrap();
    refused(
        origin_for(&conn, Some(&unbound_manual), &purpose),
        "manual proposal decision origin does not match its durable reservation",
    );
    refused(
        origin_for(&conn, Some(&automatic), "proposal-approval-automatic:0000"),
        "proposal decision reservation purpose is invalid",
    );
    let invalid = request(serde_json::json!({"decision": "maybe"}));
    refused(
        origin_for(&conn, Some(&invalid), "anything"),
        "proposal decision request has an invalid decision",
    );
}

fn origin_for(
    conn: &rusqlite::Connection,
    request: Option<&ProposalDecisionRequest>,
    purpose: &str,
) -> Result<TrustedProposalDecisionOrigin> {
    trusted_decision_approver_for_purpose(conn, PROPOSAL, "approve", request, true, purpose)
}

#[test]
fn decision_origin_trusts_an_expiry_only_as_a_server_owned_rejection() {
    let (_home, conn) = store();
    let expiry = request(serde_json::json!({"decision": "reject", "expired": true}));
    let purpose = bound_proposal_decision_purpose("proposal-expiry", &expiry).unwrap();
    let decide = |request: &ProposalDecisionRequest, purpose: &str, applying: bool| {
        trusted_decision_approver_for_purpose(
            &conn,
            PROPOSAL,
            "reject",
            Some(request),
            applying,
            purpose,
        )
    };
    let trusted = decide(&expiry, &purpose, false).unwrap();
    assert_eq!(trusted, TrustedProposalDecisionOrigin::Expiry);
    assert!(!trusted.verify_apply_intent_actor());
    refused(
        decide(&expiry, &purpose, true),
        "applying proposal cannot carry an expiry request",
    );
    for fields in [
        serde_json::json!({"decision": "approve", "expired": true}),
        serde_json::json!({"decision": "reject", "expired": true, "revisionRequired": true}),
        serde_json::json!({"decision": "reject", "expired": true, "decisionActor": "someone"}),
    ] {
        let forged = request(fields);
        let purpose = bound_proposal_decision_purpose("proposal-expiry", &forged).unwrap();
        refused(
            decide(&forged, &purpose, false),
            "proposal expiry does not match its server-owned durable reservation",
        );
    }
}

#[test]
fn decision_origin_admits_legacy_applies_only_under_the_legacy_purpose() {
    let (_home, conn) = store();
    reserve(&conn, "approve", "proposal-approval");
    assert_eq!(
        origin(&conn, "approve", None, true).unwrap(),
        TrustedProposalDecisionOrigin::Legacy
    );
    needs_review(
        origin(&conn, "approve", None, false),
        "no bound historical apply",
    );
    reserve(&conn, "reject", "proposal-approval");
    refused(
        origin(&conn, "reject", None, true),
        "applying proposal without a request lacks compatible legacy authority",
    );
    refused(
        trusted_decision_approver_for_purpose(
            &conn,
            PROPOSAL,
            "veto",
            None,
            true,
            "proposal-approval",
        ),
        "proposal decision reservation has an invalid decision",
    );
}

#[test]
fn decision_origin_resumes_a_historical_request_only_when_bound_to_its_apply_intent() {
    let (_home, conn) = store();
    reserve(&conn, "approve", "proposal-approval");
    let historical = request(serde_json::json!({}));
    needs_review(
        origin(&conn, "approve", Some(&historical), false),
        "no bound historical apply",
    );
    needs_review(
        origin(&conn, "approve", Some(&historical), true),
        "missing apply intent",
    );

    let state = |rationale: &str| -> ProposalApplyingState {
        serde_json::from_value(serde_json::json!({
            "decision": "approve",
            "recoveryCommitment": [],
            "weaveHash": [1],
            "beforeImages": [],
            "rationale": rationale,
        }))
        .unwrap()
    };
    let append = |state: &ProposalApplyingState, at: time::OffsetDateTime| {
        append_proposal_apply_intent(
            &conn,
            PROPOSAL,
            "familiar-1",
            None,
            &[],
            state,
            coven_threads_core::Channel::Mutation,
            at,
        )
        .unwrap();
    };
    append(&state("different"), claimed_at());
    needs_review(
        origin(&conn, "approve", Some(&historical), true),
        "request is not bound to the recorded apply",
    );
    append(
        &state("looks right"),
        claimed_at() + time::Duration::seconds(1),
    );
    needs_review(
        origin(&conn, "approve", Some(&historical), true),
        "request is not bound to the recorded apply",
    );
    append(&state("looks right"), claimed_at());
    let trusted = origin(&conn, "approve", Some(&historical), true).unwrap();
    assert_eq!(trusted, TrustedProposalDecisionOrigin::HistoricalRequest);
    assert!(!trusted.verify_apply_intent_actor());
}

#[test]
fn decision_origin_errors_are_classified_apart_from_other_failures() {
    assert!(invalid_proposal_decision_origin(&invalid_decision_origin(
        "x"
    )));
    assert!(!invalid_proposal_decision_origin(&anyhow::anyhow!("x")));
    assert_eq!(HISTORICAL_UNKNOWN_DECISION_ACTOR, "unknown:historical");
}
