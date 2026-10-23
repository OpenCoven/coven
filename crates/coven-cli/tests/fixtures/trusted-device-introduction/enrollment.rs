//! Synthetic local enrollment: owner-root policy, one introducer, one new
//! installation. No daemon, relay, account or installed state.
use super::*;
use crate::mobile_memory::assurance::{
    AssuranceClass, AssuranceProofBinding, CanonicalAssuranceProof, NewAuthorizationKey,
};
use crate::mobile_memory::grant::{DeviceGrant, DeviceScope};
use crate::mobile_memory::introduction::{IntroductionPolicy, IntroductionRequest};
use crate::mobile_memory::owner_policy::{OwnerPolicyStore, PolicyApproval, PolicyProposal};
use crate::mobile_memory::registry::{DeviceRecord, DeviceRegistry};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};

const AUDIENCE: [u8; 32] = [7; 32];

fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_encoded_point(false).as_bytes())
}

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
}

pub(in crate::mobile_memory) struct Fixture {
    pub(in crate::mobile_memory) home: tempfile::TempDir,
    pub(in crate::mobile_memory) now: DateTime<Utc>,
    pub(in crate::mobile_memory) root: SigningKey,
    pub(in crate::mobile_memory) source: SigningKey,
    pub(in crate::mobile_memory) step_up: SigningKey,
    pub(in crate::mobile_memory) destination: SigningKey,
    pub(in crate::mobile_memory) ledger: IntroductionLedger,
    pub(in crate::mobile_memory) envelope: IntroductionEnvelope,
}

fn policy() -> IntroductionPolicy {
    IntroductionPolicy {
        eligible_introducers: vec![Uuid::from_u128(1)],
        required_approvals: 1,
        allowed_scopes: vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin],
        maximum_grant_lifetime_seconds: 86400,
        required_assurance: RequestedAssurance::FreshBiometric,
        maximum_step_up_age_seconds: 30,
    }
}

impl Fixture {
    pub(in crate::mobile_memory) fn new() -> Self {
        Self::new_at(DateTime::from_timestamp(1_800_000_000, 0).unwrap())
    }

    pub(in crate::mobile_memory) fn new_at(now: DateTime<Utc>) -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = SigningKey::from_slice(&[11; 32]).unwrap();
        let source = SigningKey::from_slice(&[1; 32]).unwrap();
        let step_up = SigningKey::from_slice(&[2; 32]).unwrap();
        let destination = SigningKey::from_slice(&[3; 32]).unwrap();
        let source_id = Uuid::from_u128(1);
        let scopes = vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin];
        let mut grant =
            DeviceGrant::for_device(source_id, &public(&source), scopes.clone(), now).unwrap();
        grant.expires_at = Some(now + chrono::Duration::days(2));
        DeviceRegistry::load(home.path())
            .unwrap()
            .register_with_grant_and_authorization(
                DeviceRecord {
                    id: source_id,
                    display_name: "Synthetic introducer".into(),
                    public_key_x963: public(&source),
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
        let store = OwnerPolicyStore::open(home.path(), AUDIENCE).unwrap();
        let proposal = PolicyProposal {
            revision: 1,
            previous_digest: None,
            policy: policy(),
            nonce: [4; 32],
            issued_at: now,
            expires_at: now + chrono::Duration::seconds(60),
        };
        let action = store.install_action(&public(&root), &proposal).unwrap();
        store
            .install(
                &public(&root),
                &proposal,
                &sign(&root, &action.canonical_bytes().unwrap()),
                now,
            )
            .unwrap();
        let destination_id = Uuid::from_u128(2);
        let mut destination_grant = DeviceGrant::for_device(
            destination_id,
            &public(&destination),
            vec![DeviceScope::MemoryRead],
            now,
        )
        .unwrap();
        destination_grant.expires_at = Some(now + chrono::Duration::hours(1));
        let request = IntroductionRequest {
            version: 1,
            audience: AUDIENCE,
            policy_digest: policy().digest().unwrap(),
            introducer_id: source_id,
            introducer_grant_id: grant.id,
            introducer_revocation_epoch: 0,
            destination_id,
            destination_public_key: public(&destination),
            device_name: "Self-built desktop".into(),
            device_context: "Unattested development build on a local network".into(),
            grant: destination_grant,
            nonce: [9; 32],
            issued_at: now,
            expires_at: now + chrono::Duration::seconds(120),
        };
        let envelope = IntroductionEnvelope {
            destination_signature: sign(&destination, &request.possession_bytes().unwrap()),
            request: IntroductionTranscript::Single(request),
            passkey_assertion: None,
        };
        Self {
            ledger: IntroductionLedger::new(home.path(), AUDIENCE),
            home,
            now,
            root,
            source,
            step_up,
            destination,
            envelope,
        }
    }

    /// What a genuine introducer app produces after rendering the request.
    pub(in crate::mobile_memory) fn approval(&self) -> MobileIntroductionApproval {
        let authority = IntroductionAuthority::load_installed(self.home.path(), AUDIENCE).unwrap();
        let request = v1(&self.envelope);
        let challenge = authority.issue_challenge(request, self.now).unwrap();
        let key = DeviceRegistry::load(self.home.path())
            .unwrap()
            .authorization_key(request.introducer_id)
            .unwrap()
            .unwrap();
        let mut proof = PresentedAssuranceProof {
            context_mode: AssuranceContextMode::Action,
            challenge: challenge.challenge,
            issued_at: self.now,
            expires_at: challenge.expires_at,
            requested_assurance: RequestedAssurance::FreshBiometric,
            signature: String::new(),
        };
        let action = request.action().unwrap();
        proof.signature = sign(
            &self.step_up,
            &CanonicalAssuranceProof::for_action(
                AssuranceProofBinding {
                    device_id: request.introducer_id,
                    grant_id: request.introducer_grant_id,
                    revocation_epoch: 0,
                    authorization_key_id: key.subject_key_id,
                },
                &proof,
                &action,
            )
            .unwrap()
            .canonical_bytes(),
        );
        MobileIntroductionApproval {
            possession_signature: sign(&self.source, &action.canonical_bytes().unwrap()),
            step_up: MobileStepUpProof {
                challenge: URL_SAFE_NO_PAD.encode(proof.challenge),
                issued_at: proof.issued_at,
                expires_at: proof.expires_at,
                requested_assurance: proof.requested_assurance,
                signature: proof.signature,
            },
        }
    }

    /// An enrolled device that is not the request's introducer.
    pub(in crate::mobile_memory) fn enroll_outsider(&self, key: &SigningKey) -> Uuid {
        let id = Uuid::from_u128(5);
        let grant =
            DeviceGrant::for_device(id, &public(key), vec![DeviceScope::DeviceAdmin], self.now)
                .unwrap();
        DeviceRegistry::load(self.home.path())
            .unwrap()
            .register_with_grant(
                DeviceRecord {
                    id,
                    display_name: "Synthetic bystander".into(),
                    public_key_x963: public(key),
                    paired_at: self.now,
                    revoked_at: None,
                    suspended_at: None,
                    scopes: vec![DeviceScope::DeviceAdmin],
                },
                grant,
            )
            .unwrap();
        id
    }

    fn enrolled(&self) -> bool {
        DeviceRegistry::load(self.home.path())
            .unwrap()
            .device(v1(&self.envelope).destination_id)
            .unwrap()
            .is_some()
    }

    pub(in crate::mobile_memory) fn introducer(&self) -> Uuid {
        v1(&self.envelope).introducer_id
    }
}

#[test]
fn owner_admission_then_introducer_approval_enrolls_once() {
    let fixture = Fixture::new();
    let admitted = fixture
        .ledger
        .admit(fixture.envelope.clone(), fixture.now)
        .unwrap();
    assert_eq!(admitted.state, IntroductionState::AwaitingApproval);

    let pending = fixture
        .ledger
        .pending_for(fixture.introducer(), fixture.now)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].action,
        fixture.envelope.request.action().unwrap()
    );
    assert!(fixture
        .ledger
        .pending_for(Uuid::from_u128(7), fixture.now)
        .unwrap()
        .is_empty());

    let done = fixture
        .ledger
        .approve(
            admitted.id,
            fixture.introducer(),
            fixture.approval(),
            fixture.now,
        )
        .unwrap();
    assert_eq!(done.state, IntroductionState::Introduced);
    assert_eq!(done.device_id, Some(v1(&fixture.envelope).destination_id));
    assert!(fixture.enrolled());

    // Replay of the approval, or of the whole request, cannot enroll again.
    assert!(fixture
        .ledger
        .approve(
            admitted.id,
            fixture.introducer(),
            fixture.approval(),
            fixture.now
        )
        .is_err());
    assert!(fixture
        .ledger
        .admit(fixture.envelope.clone(), fixture.now)
        .is_err());
    // A restarted daemon (empty ledger) still refuses the replayed request.
    let fresh = IntroductionLedger::new(fixture.home.path(), AUDIENCE);
    let error = fresh
        .admit(fixture.envelope.clone(), fixture.now)
        .unwrap_err();
    assert!(error.to_string().contains("already enrolled"));
}

#[test]
fn tampered_or_unsigned_requests_are_never_admitted() {
    let fixture = Fixture::new();
    let mut tampered = fixture.envelope.clone();
    v1_mut(&mut tampered).grant.scopes = vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin];
    assert!(fixture.ledger.admit(tampered, fixture.now).is_err());

    let mut renamed = fixture.envelope.clone();
    v1_mut(&mut renamed).device_name = "Owner's phone".into();
    assert!(fixture.ledger.admit(renamed, fixture.now).is_err());

    // A relay holding its own key cannot sign for the destination.
    let mut forged = fixture.envelope.clone();
    let relay = SigningKey::from_slice(&[99; 32]).unwrap();
    forged.destination_signature = sign(&relay, &forged.request.possession_bytes().unwrap());
    assert!(fixture.ledger.admit(forged, fixture.now).is_err());

    let mut foreign = fixture.envelope.clone();
    v1_mut(&mut foreign).audience = [8; 32];
    assert!(fixture.ledger.admit(foreign, fixture.now).is_err());

    assert!(fixture
        .ledger
        .pending_for(fixture.introducer(), fixture.now)
        .unwrap()
        .is_empty());
    let _ = &fixture.destination;
}

#[test]
fn malicious_service_approvals_are_terminal_and_enroll_nothing() {
    let fixture = Fixture::new();
    let admitted = fixture
        .ledger
        .admit(fixture.envelope.clone(), fixture.now)
        .unwrap();
    // Another authenticated device cannot even find the request.
    assert!(fixture
        .ledger
        .approve(
            admitted.id,
            Uuid::from_u128(7),
            fixture.approval(),
            fixture.now
        )
        .is_err());
    assert_eq!(
        fixture
            .ledger
            .status(admitted.id, fixture.now)
            .unwrap()
            .state,
        IntroductionState::AwaitingApproval
    );

    // A step-up signed by anything but the enrolled key fails once, terminally.
    let mut forged = fixture.approval();
    forged.step_up.signature = sign(&fixture.root, b"not the canonical proof");
    let error = fixture
        .ledger
        .approve(admitted.id, fixture.introducer(), forged, fixture.now)
        .unwrap_err();
    assert!(format!("{error:#}").contains("Rejected"));
    assert!(fixture
        .ledger
        .approve(
            admitted.id,
            fixture.introducer(),
            fixture.approval(),
            fixture.now
        )
        .is_err());
    assert!(!fixture.enrolled());
}

#[test]
fn expiry_cancellation_and_policy_replacement_fail_closed() {
    let fixture = Fixture::new();
    let admitted = fixture
        .ledger
        .admit(fixture.envelope.clone(), fixture.now)
        .unwrap();
    let later = v1(&fixture.envelope).expires_at;
    assert_eq!(
        fixture.ledger.status(admitted.id, later).unwrap().state,
        IntroductionState::Expired
    );
    assert!(fixture
        .ledger
        .approve(admitted.id, fixture.introducer(), fixture.approval(), later)
        .is_err());

    let cancelled_ledger = IntroductionLedger::new(fixture.home.path(), AUDIENCE);
    let second = cancelled_ledger
        .admit(fixture.envelope.clone(), fixture.now)
        .unwrap();
    assert_eq!(
        cancelled_ledger
            .cancel(second.id, fixture.now)
            .unwrap()
            .state,
        IntroductionState::Cancelled
    );
    assert!(cancelled_ledger
        .approve(
            second.id,
            fixture.introducer(),
            fixture.approval(),
            fixture.now
        )
        .is_err());

    // Replacing the owner policy retires a request admitted under the old one.
    let replaced_ledger = IntroductionLedger::new(fixture.home.path(), AUDIENCE);
    let third = replaced_ledger
        .admit(fixture.envelope.clone(), fixture.now)
        .unwrap();
    let approval = fixture.approval();
    let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let head = store.current().unwrap().unwrap();
    let mut tighter = policy();
    tighter.maximum_step_up_age_seconds = 20;
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
                signature: sign(&fixture.root, &action.canonical_bytes().unwrap()),
            },
            fixture.now,
        )
        .unwrap();
    assert!(replaced_ledger
        .approve(third.id, fixture.introducer(), approval, fixture.now)
        .is_err());
    assert!(!fixture.enrolled());
}

#[test]
fn admission_requires_an_installed_policy() {
    let fixture = Fixture::new();
    std::fs::remove_file(
        fixture
            .home
            .path()
            .join("mobile")
            .join("introduction-policy.json"),
    )
    .unwrap();
    let error = fixture
        .ledger
        .admit(fixture.envelope.clone(), fixture.now)
        .unwrap_err();
    assert!(error.to_string().contains("not installed"));
}

#[test]
fn review_renders_every_signed_field() {
    let fixture = Fixture::new();
    let request = v1(&fixture.envelope);
    let action = request.action().unwrap();
    let head = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE)
        .unwrap()
        .current()
        .unwrap()
        .unwrap();
    let review = render_introduction_review(&fixture.envelope.request, &action, &head);
    for expected in [
        "devices.introduction.approve".to_owned(),
        action.target.clone(),
        format!("revision 1 ({})", request.policy_digest),
        request.introducer_id.to_string(),
        request.introducer_grant_id.to_string(),
        request.destination_id.to_string(),
        "Self-built desktop".into(),
        "Unattested development build on a local network".into(),
        request.grant.id.to_string(),
        "memory_read".into(),
        "possession".into(),
        "any authenticated".into(),
        "2027-01-15T09:00:00.000Z".into(),
        action.nonce.clone(),
        "2027-01-15T08:02:00.000Z".into(),
        action.effect_digest.clone(),
        "not device-identity evidence".into(),
    ] {
        assert!(
            review.contains(&expected),
            "review omits {expected}:\n{review}"
        );
    }
}

fn v1(envelope: &IntroductionEnvelope) -> &IntroductionRequest {
    match &envelope.request {
        IntroductionTranscript::Single(request) => request,
        IntroductionTranscript::Threshold(_) => panic!("fixture envelopes are v1"),
    }
}

fn v1_mut(envelope: &mut IntroductionEnvelope) -> &mut IntroductionRequest {
    match &mut envelope.request {
        IntroductionTranscript::Single(request) => request,
        IntroductionTranscript::Threshold(_) => panic!("fixture envelopes are v1"),
    }
}
