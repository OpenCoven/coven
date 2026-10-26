//! Synthetic 2-of-3 threshold introductions; no daemon, relay, account or installed state.
use super::*;
use crate::mobile_memory::assurance::{
    AssuranceClass, AssuranceContextMode, AssuranceProofBinding, CanonicalAssuranceProof,
    NewAuthorizationKey,
};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};

const AUDIENCE: [u8; 32] = [7; 32];

fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
}

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
}

struct Approver {
    id: Uuid,
    grant: DeviceGrant,
    possession: SigningKey,
    step_up: SigningKey,
}

struct Fixture {
    home: tempfile::TempDir,
    now: DateTime<Utc>,
    approvers: Vec<Approver>,
    destination: SigningKey,
    policy: IntroductionPolicy,
}

impl Fixture {
    /// Three eligible introducers; the third holds only `memory_read`.
    fn new(required_approvals: u16) -> Self {
        let home = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let registry = DeviceRegistry::load(home.path()).unwrap();
        let approvers: Vec<_> = (1..=3u8)
            .map(|n| {
                let id = Uuid::from_u128(u128::from(n));
                let possession = SigningKey::from_slice(&[n; 32]).unwrap();
                let step_up = SigningKey::from_slice(&[n + 10; 32]).unwrap();
                let scopes = if n == 3 {
                    vec![DeviceScope::MemoryRead]
                } else {
                    vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin]
                };
                let mut grant =
                    DeviceGrant::for_device(id, &public(&possession), scopes.clone(), now).unwrap();
                grant.expires_at = Some(now + chrono::Duration::days(2));
                registry
                    .register_with_grant_and_authorization(
                        DeviceRecord {
                            id,
                            display_name: format!("Synthetic approver {n}"),
                            public_key_x963: public(&possession),
                            paired_at: now,
                            revoked_at: None,
                            suspended_at: None,
                            scopes,
                        },
                        grant.clone(),
                        Some(NewAuthorizationKey {
                            public_key_x963: public(&step_up),
                            assurance_class: AssuranceClass::BiometricOnly,
                            enrolled_at: now,
                        }),
                    )
                    .unwrap();
                Approver {
                    id,
                    grant,
                    possession,
                    step_up,
                }
            })
            .collect();
        let policy = IntroductionPolicy {
            eligible_introducers: approvers.iter().map(|a| a.id).collect(),
            required_approvals,
            allowed_scopes: vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin],
            maximum_grant_lifetime_seconds: 86400,
            required_assurance: RequestedAssurance::FreshBiometric,
            maximum_step_up_age_seconds: 60,
        };
        Self {
            home,
            now,
            approvers,
            destination: SigningKey::from_slice(&[40; 32]).unwrap(),
            policy,
        }
    }

    fn authority(&self) -> IntroductionAuthority {
        IntroductionAuthority::load(self.home.path(), AUDIENCE, Some(self.policy.clone())).unwrap()
    }

    fn request(&self, named: &[usize]) -> ThresholdIntroductionRequest {
        let destination_id = Uuid::from_u128(50);
        let mut grant = DeviceGrant::for_device(
            destination_id,
            &public(&self.destination),
            vec![DeviceScope::MemoryRead],
            self.now,
        )
        .unwrap();
        grant.expires_at = Some(self.now + chrono::Duration::hours(1));
        ThresholdIntroductionRequest {
            version: 2,
            audience: AUDIENCE,
            policy_digest: self.policy.digest().unwrap(),
            approvers: named
                .iter()
                .map(|&i| IntroductionApprover {
                    device_id: self.approvers[i].id,
                    grant_id: self.approvers[i].grant.id,
                    revocation_epoch: 0,
                })
                .collect(),
            destination_id,
            destination_public_key: public(&self.destination),
            device_name: "Threshold desktop".into(),
            device_context: "Self-hosted build approved by two trusted devices".into(),
            grant,
            nonce: [9; 32],
            issued_at: self.now,
            expires_at: self.now + chrono::Duration::seconds(120),
        }
    }

    fn destination_signature(&self, request: &ThresholdIntroductionRequest) -> String {
        sign(&self.destination, &request.possession_bytes().unwrap())
    }

    fn approval(
        &self,
        authority: &IntroductionAuthority,
        request: &ThresholdIntroductionRequest,
        index: usize,
        step_up_key: &SigningKey,
    ) -> IntroductionApproval {
        self.approval_at(authority, request, index, step_up_key, self.now)
    }

    /// An approval whose challenge and step-up are issued at `at`.
    fn approval_at(
        &self,
        authority: &IntroductionAuthority,
        request: &ThresholdIntroductionRequest,
        index: usize,
        step_up_key: &SigningKey,
        at: DateTime<Utc>,
    ) -> IntroductionApproval {
        let approver = &self.approvers[index];
        let challenge = authority
            .issue_threshold_challenge(request, approver.id, at)
            .unwrap();
        let key = authority
            .registry
            .authorization_key(approver.id)
            .unwrap()
            .unwrap();
        let action = request.action().unwrap();
        let mut step_up = PresentedAssuranceProof {
            context_mode: AssuranceContextMode::Action,
            challenge: challenge.challenge,
            issued_at: at,
            // Bounded by both the challenge and the approved action.
            expires_at: challenge.expires_at.min(request.expires_at),
            requested_assurance: RequestedAssurance::FreshBiometric,
            signature: String::new(),
        };
        step_up.signature = sign(
            step_up_key,
            &CanonicalAssuranceProof::for_action(
                AssuranceProofBinding {
                    device_id: approver.id,
                    grant_id: approver.grant.id,
                    revocation_epoch: 0,
                    authorization_key_id: key.subject_key_id,
                },
                &step_up,
                &action,
            )
            .unwrap()
            .canonical_bytes(),
        );
        IntroductionApproval {
            possession_signature: sign(&approver.possession, &action.canonical_bytes().unwrap()),
            step_up,
        }
    }

    fn genuine(
        &self,
        authority: &IntroductionAuthority,
        request: &ThresholdIntroductionRequest,
        index: usize,
    ) -> IntroductionApproval {
        self.approval(authority, request, index, &self.approvers[index].step_up)
    }

    fn enrolled(&self) -> bool {
        DeviceRegistry::load(self.home.path())
            .unwrap()
            .device(Uuid::from_u128(50))
            .unwrap()
            .is_some()
    }
}

#[test]
fn explicit_threshold_bounds_are_enforced() {
    let fixture = Fixture::new(2);
    for n in [0, 4, u16::MAX] {
        let mut policy = fixture.policy.clone();
        policy.required_approvals = n;
        assert!(IntroductionAuthority::load(fixture.home.path(), AUDIENCE, Some(policy)).is_err());
    }
    for n in [1, 2, 3] {
        let mut policy = fixture.policy.clone();
        policy.required_approvals = n;
        assert!(IntroductionAuthority::load(fixture.home.path(), AUDIENCE, Some(policy)).is_ok());
    }
}

#[test]
fn two_of_three_commits_once_with_both_named_approvals() {
    let fixture = Fixture::new(2);
    let authority = fixture.authority();
    let request = fixture.request(&[0, 1]);
    let signature = fixture.destination_signature(&request);
    authority
        .admit_threshold(&request, &signature, fixture.now)
        .unwrap();
    let a = fixture.genuine(&authority, &request, 0);
    let b = fixture.genuine(&authority, &request, 1);
    let record = authority
        .introduce_threshold(
            &request,
            &signature,
            &[(fixture.approvers[1].id, &b), (fixture.approvers[0].id, &a)],
            fixture.now,
        )
        .unwrap();
    assert_eq!(record.device.id, request.destination_id);
    assert!(fixture.enrolled());

    // Replaying the committed transcript with fresh approvals is refused.
    let a = fixture.genuine(&authority, &request, 0);
    let b = fixture.genuine(&authority, &request, 1);
    let error = authority
        .introduce_threshold(
            &request,
            &signature,
            &[(fixture.approvers[0].id, &a), (fixture.approvers[1].id, &b)],
            fixture.now,
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("already consumed"));
}

#[test]
fn fewer_extra_or_duplicate_approvals_never_commit() {
    let fixture = Fixture::new(2);
    let authority = fixture.authority();
    let request = fixture.request(&[0, 1]);
    let signature = fixture.destination_signature(&request);
    let a = fixture.genuine(&authority, &request, 0);
    let b = fixture.genuine(&authority, &request, 1);
    let ids: Vec<_> = fixture.approvers.iter().map(|a| a.id).collect();
    for approvals in [
        vec![(ids[0], &a)],
        vec![(ids[0], &a), (ids[0], &a)],
        vec![(ids[0], &a), (ids[1], &b), (ids[2], &b)],
        vec![(ids[0], &a), (ids[2], &b)],
    ] {
        let error = authority
            .introduce_threshold(&request, &signature, &approvals, fixture.now)
            .unwrap_err();
        assert!(error.to_string().contains("do not match"), "{error:#}");
    }
    // An unnamed eligible device cannot even obtain a challenge for it.
    assert!(authority
        .issue_threshold_challenge(&request, ids[2], fixture.now)
        .is_err());
    assert!(!fixture.enrolled());
}

#[test]
fn one_forged_approval_fails_the_whole_threshold() {
    let fixture = Fixture::new(2);
    let authority = fixture.authority();
    let request = fixture.request(&[0, 1]);
    let signature = fixture.destination_signature(&request);
    let a = fixture.genuine(&authority, &request, 0);
    // A compromised relay/account service holds neither approver's key.
    let service = SigningKey::from_slice(&[77; 32]).unwrap();
    let forged = fixture.approval(&authority, &request, 1, &service);
    let ids = [fixture.approvers[0].id, fixture.approvers[1].id];
    assert!(authority
        .introduce_threshold(
            &request,
            &signature,
            &[(ids[0], &a), (ids[1], &forged)],
            fixture.now
        )
        .is_err());
    assert!(!fixture.enrolled());

    // The nonce was never consumed, so fresh genuine approvals still work.
    let a = fixture.genuine(&authority, &request, 0);
    let b = fixture.genuine(&authority, &request, 1);
    authority
        .introduce_threshold(
            &request,
            &signature,
            &[(ids[0], &a), (ids[1], &b)],
            fixture.now,
        )
        .unwrap();
    assert!(fixture.enrolled());
}

#[test]
fn request_shape_must_match_the_policy_threshold() {
    let fixture = Fixture::new(2);
    let authority = fixture.authority();
    // Three named approvers under N = 2, and an unsorted or duplicate list.
    for named in [&[0usize, 1, 2][..], &[1, 0], &[0, 0]] {
        let request = fixture.request(named);
        assert!(authority
            .admit_threshold(
                &request,
                &fixture.destination_signature_unchecked(&request),
                fixture.now
            )
            .is_err());
    }
    let mut wrong_version = fixture.request(&[0, 1]);
    wrong_version.version = 1;
    assert!(wrong_version.action().is_err());

    // A v1 single-introducer request cannot satisfy N = 2.
    let single = IntroductionRequest {
        version: 1,
        audience: AUDIENCE,
        policy_digest: fixture.policy.digest().unwrap(),
        introducer_id: fixture.approvers[0].id,
        introducer_grant_id: fixture.approvers[0].grant.id,
        introducer_revocation_epoch: 0,
        destination_id: Uuid::from_u128(50),
        destination_public_key: public(&fixture.destination),
        device_name: "Threshold desktop".into(),
        device_context: "Single approver attempt".into(),
        grant: fixture.request(&[0, 1]).grant,
        nonce: [8; 32],
        issued_at: fixture.now,
        expires_at: fixture.now + chrono::Duration::seconds(120),
    };
    let signature = sign(&fixture.destination, &single.possession_bytes().unwrap());
    assert!(authority.admit(&single, &signature, fixture.now).is_err());
    assert!(authority.issue_challenge(&single, fixture.now).is_err());
}

#[test]
fn destination_grant_is_bounded_by_every_approver() {
    let fixture = Fixture::new(2);
    let authority = fixture.authority();
    // Approver 3 lacks device_admin, so it cannot approve a grant at all
    // (introduction needs device_admin) nor co-sign one broader than itself.
    let request = fixture.request(&[0, 2]);
    let signature = fixture.destination_signature(&request);
    assert!(authority
        .admit_threshold(&request, &signature, fixture.now)
        .is_err());
    assert!(authority
        .issue_threshold_challenge(&request, fixture.approvers[2].id, fixture.now)
        .is_err());
}

#[test]
fn revocation_between_approvals_and_expiry_fail_closed() {
    let fixture = Fixture::new(2);
    let authority = fixture.authority();
    let request = fixture.request(&[0, 1]);
    let signature = fixture.destination_signature(&request);
    let a = fixture.genuine(&authority, &request, 0);
    let b = fixture.genuine(&authority, &request, 1);
    let ids = [fixture.approvers[0].id, fixture.approvers[1].id];
    assert!(authority
        .introduce_threshold(
            &request,
            &signature,
            &[(ids[0], &a), (ids[1], &b)],
            request.expires_at
        )
        .is_err());
    DeviceRegistry::load(fixture.home.path())
        .unwrap()
        .revoke(ids[1], fixture.now)
        .unwrap();
    let a = fixture.genuine(&authority, &request, 0);
    assert!(authority
        .introduce_threshold(
            &request,
            &signature,
            &[(ids[0], &a), (ids[1], &b)],
            fixture.now
        )
        .is_err());
    assert!(!fixture.enrolled());
}

#[test]
fn v1_and_v2_transcripts_are_domain_separated() {
    let fixture = Fixture::new(1);
    let mut policy = fixture.policy.clone();
    policy.required_approvals = 2;
    let fixture = Fixture { policy, ..fixture };
    let v2 = fixture.request(&[0, 1]);
    let action = v2.action().unwrap();
    // The same signed bytes never verify as a different version's action.
    let mut material = b"COVEN-INTRODUCTION/1\0".to_vec();
    material.extend(serde_jcs::to_vec(&v2).unwrap());
    assert_ne!(
        action.effect_digest,
        URL_SAFE_NO_PAD.encode(Sha256::digest(material))
    );
}

impl Fixture {
    /// Signature over whatever the request encodes, valid or not.
    fn destination_signature_unchecked(&self, request: &ThresholdIntroductionRequest) -> String {
        request
            .possession_bytes()
            .map(|bytes| sign(&self.destination, &bytes))
            .unwrap_or_default()
    }
}

mod ledger {
    use super::*;
    use crate::mobile_memory::enrollment::{
        IntroductionEnvelope, IntroductionLedger, IntroductionState, MobileIntroductionApproval,
        MobileStepUpProof,
    };
    use crate::mobile_memory::owner_policy::{OwnerPolicyStore, PolicyApproval, PolicyProposal};

    fn mobile(approval: IntroductionApproval) -> MobileIntroductionApproval {
        MobileIntroductionApproval {
            possession_signature: approval.possession_signature,
            step_up: MobileStepUpProof {
                challenge: URL_SAFE_NO_PAD.encode(approval.step_up.challenge),
                issued_at: approval.step_up.issued_at,
                expires_at: approval.step_up.expires_at,
                requested_assurance: approval.step_up.requested_assurance,
                signature: approval.step_up.signature,
            },
        }
    }

    fn installed(fixture: &Fixture) -> (IntroductionLedger, IntroductionEnvelope) {
        let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
        let root = SigningKey::from_slice(&[90; 32]).unwrap();
        let proposal = PolicyProposal {
            revision: 1,
            previous_digest: None,
            policy: fixture.policy.clone(),
            nonce: [4; 32],
            issued_at: fixture.now,
            expires_at: fixture.now + chrono::Duration::seconds(60),
        };
        let action = store.install_action(&public(&root), &proposal).unwrap();
        store
            .install(
                &public(&root),
                &proposal,
                &sign(&root, &action.canonical_bytes().unwrap()),
                fixture.now,
            )
            .unwrap();
        let request = fixture.request(&[0, 1]);
        let envelope = IntroductionEnvelope {
            destination_signature: fixture.destination_signature(&request),
            request: IntroductionTranscript::Threshold(request),
        };
        (
            IntroductionLedger::new(fixture.home.path(), AUDIENCE),
            envelope,
        )
    }

    fn approval(
        fixture: &Fixture,
        envelope: &IntroductionEnvelope,
        index: usize,
    ) -> MobileIntroductionApproval {
        let IntroductionTranscript::Threshold(request) = &envelope.request else {
            unreachable!()
        };
        let authority =
            IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
        mobile(fixture.genuine(&authority, request, index))
    }

    #[test]
    fn ledger_collects_named_approvals_and_commits_at_n() {
        let fixture = Fixture::new(2);
        let (ledger, envelope) = installed(&fixture);
        let ids: Vec<_> = fixture.approvers.iter().map(|a| a.id).collect();
        let admitted = ledger.admit(envelope.clone(), fixture.now).unwrap();
        assert_eq!(
            (admitted.approvals_received, admitted.approvals_required),
            (0, 2)
        );
        assert!(ledger.pending_for(ids[2], fixture.now).unwrap().is_empty());

        let first = ledger
            .approve(
                admitted.id,
                ids[0],
                approval(&fixture, &envelope, 0),
                fixture.now,
            )
            .unwrap();
        assert_eq!(first.state, IntroductionState::AwaitingApproval);
        assert_eq!(first.approvals_received, 1);
        assert!(!fixture.enrolled());
        // N-1 approvals never commit, and an approver cannot count twice.
        assert!(ledger.pending_for(ids[0], fixture.now).unwrap().is_empty());
        assert_eq!(ledger.pending_for(ids[1], fixture.now).unwrap().len(), 1);
        assert!(ledger
            .approve(
                admitted.id,
                ids[0],
                approval(&fixture, &envelope, 0),
                fixture.now
            )
            .is_err());
        assert!(ledger
            .approve(
                admitted.id,
                ids[2],
                approval(&fixture, &envelope, 0),
                fixture.now
            )
            .is_err());
        assert_eq!(
            ledger.status(admitted.id, fixture.now).unwrap().state,
            IntroductionState::AwaitingApproval
        );

        let done = ledger
            .approve(
                admitted.id,
                ids[1],
                approval(&fixture, &envelope, 1),
                fixture.now,
            )
            .unwrap();
        assert_eq!(done.state, IntroductionState::Introduced);
        assert!(fixture.enrolled());
    }

    #[test]
    fn a_forged_final_approval_is_terminal() {
        let fixture = Fixture::new(2);
        let (ledger, envelope) = installed(&fixture);
        let ids: Vec<_> = fixture.approvers.iter().map(|a| a.id).collect();
        let admitted = ledger.admit(envelope.clone(), fixture.now).unwrap();
        ledger
            .approve(
                admitted.id,
                ids[0],
                approval(&fixture, &envelope, 0),
                fixture.now,
            )
            .unwrap();
        let mut forged = approval(&fixture, &envelope, 1);
        forged.possession_signature = sign(&fixture.approvers[0].possession, b"other bytes");
        assert!(ledger
            .approve(admitted.id, ids[1], forged, fixture.now)
            .is_err());
        assert_eq!(
            ledger.status(admitted.id, fixture.now).unwrap().state,
            IntroductionState::Rejected
        );
        assert!(!fixture.enrolled());
    }

    fn approval_at(
        fixture: &Fixture,
        envelope: &IntroductionEnvelope,
        index: usize,
        at: DateTime<Utc>,
    ) -> MobileIntroductionApproval {
        let IntroductionTranscript::Threshold(request) = &envelope.request else {
            unreachable!()
        };
        let authority =
            IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
        mobile(fixture.approval_at(
            &authority,
            request,
            index,
            &fixture.approvers[index].step_up,
            at,
        ))
    }

    /// Owner-root replacement raising N from 2 to 3.
    fn require_three(fixture: &Fixture) {
        let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
        let head = store.current().unwrap().unwrap();
        let root = SigningKey::from_slice(&[90; 32]).unwrap();
        let mut tighter = fixture.policy.clone();
        tighter.required_approvals = 3;
        let proposal = PolicyProposal {
            revision: 2,
            previous_digest: Some(head.digest),
            policy: tighter,
            nonce: [5; 32],
            issued_at: fixture.now,
            expires_at: fixture.now + chrono::Duration::seconds(60),
        };
        let action = store.replace_action(&proposal).unwrap();
        store
            .replace(
                &proposal,
                &PolicyApproval::OwnerRoot {
                    signature: sign(&root, &action.canonical_bytes().unwrap()),
                },
                fixture.now,
            )
            .unwrap();
    }

    #[test]
    fn a_stale_earlier_approval_is_dropped_and_recovers_with_a_fresh_step_up() {
        let fixture = Fixture::new(2);
        let (ledger, envelope) = installed(&fixture);
        let ids: Vec<_> = fixture.approvers.iter().map(|a| a.id).collect();
        let admitted = ledger.admit(envelope.clone(), fixture.now).unwrap();
        ledger
            .approve(
                admitted.id,
                ids[0],
                approval(&fixture, &envelope, 0),
                fixture.now,
            )
            .unwrap();

        // The final approval arrives after the first step-up aged out (60 s).
        let late = fixture.now + chrono::Duration::seconds(70);
        let status = ledger
            .approve(
                admitted.id,
                ids[1],
                approval_at(&fixture, &envelope, 1, late),
                late,
            )
            .unwrap();
        assert_eq!(status.state, IntroductionState::AwaitingApproval);
        assert_eq!(
            (status.approvals_received, status.approvals_required),
            (1, 2)
        );
        assert_eq!(status.step_up_expired, [ids[0]]);
        assert!(!fixture.enrolled());
        // Only the expired approver is asked again; N is unchanged.
        assert_eq!(ledger.pending_for(ids[0], late).unwrap().len(), 1);
        assert!(ledger.pending_for(ids[1], late).unwrap().is_empty());

        let fresh = late + chrono::Duration::seconds(10);
        let done = ledger
            .approve(
                admitted.id,
                ids[0],
                approval_at(&fixture, &envelope, 0, fresh),
                fresh,
            )
            .unwrap();
        assert_eq!(done.state, IntroductionState::Introduced);
        assert!(done.step_up_expired.is_empty());
        assert!(fixture.enrolled());
    }

    #[test]
    fn the_policy_lock_is_held_from_head_check_through_the_registry_write() {
        let fixture = Fixture::new(2);
        let (ledger, envelope) = installed(&fixture);
        let ids: Vec<_> = fixture.approvers.iter().map(|a| a.id).collect();
        let admitted = ledger.admit(envelope.clone(), fixture.now).unwrap();
        ledger
            .approve(
                admitted.id,
                ids[0],
                approval(&fixture, &envelope, 0),
                fixture.now,
            )
            .unwrap();
        let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
        assert!(!store.is_locked().unwrap());

        // Every check has passed and nothing is written yet: a replacement
        // here would retire the request, so it must not be able to land.
        let home = fixture.home.path().to_owned();
        let observed = std::rc::Rc::new(std::cell::Cell::new(None));
        let seen = observed.clone();
        BEFORE_INTRODUCTION_WRITE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let store = OwnerPolicyStore::open(&home, AUDIENCE).unwrap();
                seen.set(Some(store.is_locked().unwrap()));
            }));
        });
        let done = ledger
            .approve(
                admitted.id,
                ids[1],
                approval(&fixture, &envelope, 1),
                fixture.now,
            )
            .unwrap();
        assert_eq!(done.state, IntroductionState::Introduced);
        assert_eq!(observed.get(), Some(true));
        assert!(!store.is_locked().unwrap());
    }

    #[test]
    fn a_tightened_policy_retires_a_fully_approved_introduction() {
        let fixture = Fixture::new(2);
        let _ = installed(&fixture);
        let request = fixture.request(&[0, 1]);
        // Authority and both approvals are bound to the N = 2 head.
        let authority =
            IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
        let a = fixture.genuine(&authority, &request, 0);
        let b = fixture.genuine(&authority, &request, 1);
        require_three(&fixture);
        let error = authority
            .introduce_threshold(
                &request,
                &fixture.destination_signature(&request),
                &[(fixture.approvers[0].id, &a), (fixture.approvers[1].id, &b)],
                fixture.now,
            )
            .unwrap_err();
        assert!(error.to_string().contains("owner policy was replaced"));
        assert!(!fixture.enrolled());
    }
}
