//! Attestation at local enrollment: optional by default, an owner-chosen floor
//! when the owner tightens policy. Synthetic chains; no vendor root or device.
use super::*;
use crate::mobile_memory::attestation::tests::{key_description, Ca};
use crate::mobile_memory::attestation::{AttestationEvidence, OfficialAndroidApp};
use crate::mobile_memory::device::DeviceAuthority;
use crate::mobile_memory::enrollment::tests::Fixture;
use crate::mobile_memory::owner_policy::{OwnerPolicyStore, PolicyApproval, PolicyProposal};
use p256::ecdsa::{signature::Signer, Signature};

const AUDIENCE: [u8; 32] = [7; 32];

fn sign(key: &p256::ecdsa::SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
}

/// Root-signed tightening to `minimum`; returns the new policy digest.
fn require(fixture: &Fixture, minimum: DeviceAttestation) -> String {
    let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let head = store.current().unwrap().unwrap();
    let mut policy = head.policy.clone();
    policy.minimum_attestation = minimum;
    let proposal = PolicyProposal {
        revision: head.revision + 1,
        previous_digest: Some(head.digest),
        policy: policy.clone(),
        nonce: [70 + head.revision as u8; 32],
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
    policy.digest().unwrap()
}

/// The envelope a client would build for the current policy.
fn envelope(
    fixture: &Fixture,
    policy_digest: String,
    evidence: Option<&Ca>,
) -> IntroductionEnvelope {
    let IntroductionTranscript::Single(mut request) = fixture.envelope.request.clone() else {
        unreachable!()
    };
    request.policy_digest = policy_digest;
    let possession = request.possession_bytes().unwrap();
    let attestation = evidence.map(|ca| AttestationEvidence::AndroidKey {
        chain: vec![
            ca.leaf(
                &fixture.destination,
                key_description(1, &attestation_challenge(&possession), None),
            ),
            ca.root_b64(),
        ],
    });
    IntroductionEnvelope {
        destination_signature: sign(&fixture.destination, &possession),
        request: IntroductionTranscript::Single(request),
        passkey_assertion: None,
        attestation,
    }
}

fn approve(
    fixture: &Fixture,
    ledger: &IntroductionLedger,
    envelope: &IntroductionEnvelope,
) -> IntroductionStatus {
    let admitted = ledger.admit(envelope.clone(), fixture.now).unwrap();
    ledger
        .approve(
            admitted.id,
            fixture.introducer(),
            fixture.approval_for(envelope),
            fixture.now,
        )
        .unwrap()
}

#[test]
fn self_built_unattested_clients_still_enroll_under_default_policy() {
    let fixture = Fixture::new();
    let ca = Ca::new();
    let mut envelope = envelope(&fixture, fixture.policy_digest(), None);
    // Evidence that cannot be verified (no owner anchors) is not an error.
    envelope.attestation = Some(AttestationEvidence::AndroidKey {
        chain: vec![ca.root_b64()],
    });
    let done = approve(&fixture, &fixture.ledger, &envelope);
    assert_eq!(done.state, IntroductionState::Introduced);
    assert_eq!(done.attestation, DeviceAttestation::UnattestedDevice);
    let view = DeviceAuthority::load(fixture.home.path())
        .unwrap()
        .inspect(&done.device_id.unwrap().to_string(), fixture.now)
        .unwrap();
    assert_eq!(view.attestation, DeviceAttestation::UnattestedDevice);
}

#[test]
fn an_owner_floor_requires_verified_evidence_and_records_it() {
    let fixture = Fixture::new();
    let ca = Ca::new();
    AttestationTrust {
        android_roots: vec![ca.root_b64()],
        official_android_apps: vec![OfficialAndroidApp {
            package_name: "org.opencoven.pocket".into(),
            signature_digest: URL_SAFE_NO_PAD.encode([44; 32]),
        }],
    }
    .save(fixture.home.path())
    .unwrap();
    let digest = require(&fixture, DeviceAttestation::VerifiedHardwareKey);

    // Without evidence, or with evidence from an unrecognized CA, admission fails.
    let error = fixture
        .ledger
        .admit(envelope(&fixture, digest.clone(), None), fixture.now)
        .unwrap_err();
    assert!(error.to_string().contains("requires verified_hardware_key"));
    let rogue = Ca::new();
    assert!(fixture
        .ledger
        .admit(
            envelope(&fixture, digest.clone(), Some(&rogue)),
            fixture.now
        )
        .is_err());

    let attested = envelope(&fixture, digest, Some(&ca));
    let done = approve(&fixture, &fixture.ledger, &attested);
    assert_eq!(done.attestation, DeviceAttestation::VerifiedHardwareKey);
    let view = DeviceAuthority::load(fixture.home.path())
        .unwrap()
        .inspect(&done.device_id.unwrap().to_string(), fixture.now)
        .unwrap();
    assert_eq!(view.attestation, DeviceAttestation::VerifiedHardwareKey);
}

#[test]
fn lowering_the_floor_is_a_downgrade() {
    let fixture = Fixture::new();
    require(&fixture, DeviceAttestation::VerifiedOfficialApp);
    let head = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE)
        .unwrap()
        .current()
        .unwrap()
        .unwrap();
    let mut lower = head.policy.clone();
    lower.minimum_attestation = DeviceAttestation::VerifiedHardwareKey;
    assert_eq!(
        crate::mobile_memory::owner_policy::broadened_dimensions(&lower, &head.policy),
        ["minimum attestation"]
    );
    // The default floor leaves the policy digest exactly as before.
    let mut unchanged = head.policy;
    unchanged.minimum_attestation = DeviceAttestation::UnattestedDevice;
    let serialized = serde_json::to_string(&unchanged).unwrap();
    assert!(!serialized.contains("minimumAttestation"));
}
