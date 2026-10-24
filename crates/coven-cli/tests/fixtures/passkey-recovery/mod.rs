//! Synthetic WebAuthn authenticator and passkey factor; no browser, relying
//! party server, account service or vendor SDK.
use super::*;
use crate::mobile_memory::introduction::threshold_tests::Fixture as Threshold;
use crate::mobile_memory::introduction::{IntroductionApproval, ThresholdIntroductionRequest};
use crate::mobile_memory::owner_policy::PolicyProposal;
use crate::mobile_memory::registry::DeviceRegistry;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;

const AUDIENCE: [u8; 32] = [7; 32];
const RP_ID: &str = "coven.example.org";
const ORIGIN: &str = "https://coven.example.org";

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
}

fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_encoded_point(false).as_bytes())
}

/// Behaves like a platform authenticator: signs authData || SHA-256(clientData).
struct Authenticator {
    key: SigningKey,
    credential_id: String,
    counter: u32,
    flags: u8,
}

impl Authenticator {
    fn synced() -> Self {
        Self {
            key: SigningKey::from_slice(&[61; 32]).unwrap(),
            credential_id: URL_SAFE_NO_PAD.encode([62; 16]),
            counter: 0,
            flags: FLAG_USER_PRESENT
                | FLAG_USER_VERIFIED
                | FLAG_BACKUP_ELIGIBLE
                | FLAG_BACKUP_STATE,
        }
    }

    fn assert(
        &mut self,
        challenge: &[u8; 32],
        origin: &str,
        rp_id: &str,
        kind: &str,
    ) -> PasskeyAssertion {
        let mut authenticator_data = Sha256::digest(rp_id.as_bytes()).to_vec();
        authenticator_data.push(self.flags);
        authenticator_data.extend_from_slice(&self.counter.to_be_bytes());
        let client = serde_json::json!({
            "type": kind,
            "challenge": URL_SAFE_NO_PAD.encode(challenge),
            "origin": origin,
            "crossOrigin": false,
        });
        let client_data_json = serde_json::to_vec(&client).unwrap();
        let mut signed = authenticator_data.clone();
        signed.extend_from_slice(&Sha256::digest(&client_data_json));
        PasskeyAssertion {
            credential_id: self.credential_id.clone(),
            authenticator_data: URL_SAFE_NO_PAD.encode(authenticator_data),
            client_data_json: URL_SAFE_NO_PAD.encode(client_data_json),
            signature: sign(&self.key, &signed),
        }
    }

    fn get(&mut self, challenge: &[u8; 32]) -> PasskeyAssertion {
        self.assert(challenge, ORIGIN, RP_ID, "webauthn.get")
    }
}

const ROOT: [u8; 32] = [90; 32];

fn install(fixture: &Threshold) -> SigningKey {
    let root = SigningKey::from_slice(&ROOT).unwrap();
    let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
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
    root
}

fn registration(fixture: &Threshold, authenticator: &Authenticator) -> PasskeyRegistration {
    PasskeyRegistration {
        credential_id: authenticator.credential_id.clone(),
        rp_id: RP_ID.into(),
        origins: vec![ORIGIN.into()],
        public_key_spki: URL_SAFE_NO_PAD.encode(
            authenticator
                .key
                .verifying_key()
                .to_public_key_der()
                .unwrap()
                .as_bytes(),
        ),
        nonce: [33; 32],
        issued_at: fixture.now,
        expires_at: fixture.now + chrono::Duration::seconds(120),
    }
}

fn register(
    fixture: &Threshold,
    authenticator: &mut Authenticator,
    root: &SigningKey,
) -> Result<PasskeyCredential> {
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let registration = registration(fixture, authenticator);
    let action = store.registration_action(&registration).unwrap();
    authenticator.counter += 1;
    let possession = authenticator.get(&action_challenge(&action).unwrap());
    store.register(
        &registration,
        &possession,
        &sign(root, &action.canonical_bytes().unwrap()),
        fixture.now,
    )
}

fn passkey_request(
    fixture: &Threshold,
    authenticator: &Authenticator,
    devices: &[usize],
) -> ThresholdIntroductionRequest {
    let mut request = fixture.request(devices);
    request.passkey_approver = Some(authenticator.credential_id.clone());
    request
}

#[test]
fn registration_needs_the_owner_root_and_passkey_possession() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    let service = SigningKey::from_slice(&[66; 32]).unwrap();
    // A relay/account service cannot register a passkey into the trust domain.
    assert!(register(&fixture, &mut authenticator, &service).is_err());
    // Possession over another challenge is refused.
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let registration = registration(&fixture, &authenticator);
    let action = store.registration_action(&registration).unwrap();
    authenticator.counter += 1;
    let wrong = authenticator.get(&[0; 32]);
    assert!(store
        .register(
            &registration,
            &wrong,
            &sign(&root, &action.canonical_bytes().unwrap()),
            fixture.now
        )
        .is_err());

    let credential = register(&fixture, &mut authenticator, &root).unwrap();
    // A synced passkey is recorded as such, and is never a device.
    assert!(credential.backup_eligible && credential.backed_up);
    let devices = DeviceRegistry::load(fixture.home.path())
        .unwrap()
        .list_status()
        .unwrap();
    assert_eq!(devices.len(), 3);
    assert!(
        register(&fixture, &mut authenticator, &root).is_err(),
        "duplicate"
    );
}

#[test]
fn the_policy_lock_is_held_from_root_check_through_the_passkey_write() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert!(!store.is_locked().unwrap());

    // Every check has passed and nothing is written yet: a root rotation here
    // would retire the registration, so it must not be able to land.
    let home = fixture.home.path().to_owned();
    let observed = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = observed.clone();
    BEFORE_PASSKEY_WRITE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let store = OwnerPolicyStore::open(&home, AUDIENCE).unwrap();
            seen.set(Some(store.is_locked().unwrap()));
        }));
    });
    register(&fixture, &mut authenticator, &root).unwrap();
    assert_eq!(observed.get(), Some(true));
    assert!(!store.is_locked().unwrap());
    let passkeys = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert_eq!(passkeys.list().unwrap().len(), 1);
}

#[test]
fn passkey_counts_as_one_approval_beside_a_trusted_device() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    let authority = fixture.authority();
    let request = passkey_request(&fixture, &authenticator, &[0]);
    let signature = fixture.destination_signature(&request);
    authority
        .admit_threshold(&request, &signature, fixture.now)
        .unwrap();
    let device = fixture.genuine(&authority, &request, 0);
    authenticator.counter += 1;
    let assertion = authenticator.get(&action_challenge(&request.action().unwrap()).unwrap());
    authority
        .introduce_threshold_with_passkey(
            &request,
            &signature,
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now,
        )
        .unwrap();
    assert!(fixture.enrolled());
}

#[test]
fn a_passkey_can_never_approve_alone() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    // No device approver at all: the request itself is invalid.
    let alone = passkey_request(&fixture, &authenticator, &[]);
    assert!(alone.action().is_err());
    // Under N = 1 a passkey has no slot to fill.
    let single = Threshold::new(1);
    let mut one = single.request(&[0, 1]);
    one.approvers.clear();
    one.passkey_approver = Some(authenticator.credential_id.clone());
    assert!(one.action().is_err());
    // The device approval is still mandatory at commit.
    let authority = fixture.authority();
    let request = passkey_request(&fixture, &authenticator, &[0]);
    let signature = fixture.destination_signature(&request);
    authenticator.counter += 1;
    let assertion = authenticator.get(&action_challenge(&request.action().unwrap()).unwrap());
    assert!(authority
        .introduce_threshold_with_passkey(&request, &signature, &[], Some(&assertion), fixture.now)
        .is_err());
    assert!(!fixture.enrolled());
}

#[test]
fn assertions_bind_the_exact_action_origin_rp_and_counter() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    let authority = fixture.authority();
    let request = passkey_request(&fixture, &authenticator, &[0]);
    let signature = fixture.destination_signature(&request);
    let challenge = action_challenge(&request.action().unwrap()).unwrap();
    let ids = [(fixture.approvers[0].id)];
    let attempt = |assertion: &PasskeyAssertion| {
        let device: IntroductionApproval = fixture.genuine(&authority, &request, 0);
        authority.introduce_threshold_with_passkey(
            &request,
            &signature,
            &[(ids[0], &device)],
            Some(assertion),
            fixture.now,
        )
    };
    authenticator.counter += 1;
    for bad in [
        authenticator.get(&[1; 32]),
        authenticator.assert(&challenge, "https://evil.example", RP_ID, "webauthn.get"),
        authenticator.assert(&challenge, ORIGIN, "evil.example", "webauthn.get"),
        authenticator.assert(&challenge, ORIGIN, RP_ID, "webauthn.create"),
    ] {
        assert!(attempt(&bad).is_err());
    }
    let mut no_uv = Authenticator::synced();
    no_uv.flags = FLAG_USER_PRESENT;
    no_uv.counter = authenticator.counter + 1;
    assert!(attempt(&no_uv.get(&challenge)).is_err());
    // Counter must advance: a stale (cloned) authenticator is refused.
    let stale = authenticator.get(&challenge);
    authenticator.counter += 1;
    let fresh = authenticator.get(&challenge);
    attempt(&fresh).unwrap();
    assert!(fixture.enrolled());
    let error = PasskeyStore::open(fixture.home.path(), AUDIENCE)
        .unwrap()
        .verify_and_advance(&stale, &challenge)
        .unwrap_err();
    assert!(error.to_string().contains("did not advance"));
}

#[test]
fn rotating_the_owner_root_retires_passkey_registrations() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let head = store.current().unwrap().unwrap();
    let new_root = SigningKey::from_slice(&[91; 32]).unwrap();
    let proposal = PolicyProposal {
        revision: head.revision + 1,
        previous_digest: Some(head.digest),
        policy: head.policy,
        nonce: [34; 32],
        issued_at: fixture.now,
        expires_at: fixture.now + chrono::Duration::seconds(60),
    };
    let bytes = store
        .rotation_action(&public(&new_root), &proposal)
        .unwrap()
        .canonical_bytes()
        .unwrap();
    store
        .rotate_root(
            &public(&new_root),
            &proposal,
            &sign(&root, &bytes),
            &sign(&new_root, &bytes),
            fixture.now,
        )
        .unwrap();
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert!(store.list().unwrap().is_empty());
    // A retired passkey cannot fill an approval.
    let authority = fixture.authority();
    let request = passkey_request(&fixture, &authenticator, &[0]);
    let signature = fixture.destination_signature(&request);
    let device = fixture.genuine(&authority, &request, 0);
    authenticator.counter += 1;
    let assertion = authenticator.get(&action_challenge(&request.action().unwrap()).unwrap());
    assert!(authority
        .introduce_threshold_with_passkey(
            &request,
            &signature,
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now,
        )
        .is_err());
}
