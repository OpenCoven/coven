//! Synthetic WebAuthn authenticator and passkey factor; no browser, relying
//! party server, account service or vendor SDK.
use super::*;
use crate::mobile_memory::introduction::threshold_tests::Fixture as Threshold;
use crate::mobile_memory::introduction::{
    IntroductionApproval, IntroductionAuthority, ThresholdIntroductionRequest,
};
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
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
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

fn audit_events(fixture: &Threshold) -> Vec<String> {
    fs::read_to_string(fixture.home.path().join("mobile/audit.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn returning_to_an_old_root_does_not_resurrect_its_retired_passkey() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let other = SigningKey::from_slice(&[91; 32]).unwrap();
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    let policy = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    for (old, next, nonce) in [(&root, &other, 71), (&other, &root, 72)] {
        let head = policy.current().unwrap().unwrap();
        let proposal = PolicyProposal {
            revision: head.revision + 1,
            previous_digest: Some(head.digest),
            policy: head.policy,
            nonce: [nonce; 32],
            issued_at: fixture.now,
            expires_at: fixture.now + chrono::Duration::seconds(60),
        };
        let bytes = policy
            .rotation_action(&public(next), &proposal)
            .unwrap()
            .canonical_bytes()
            .unwrap();
        policy
            .rotate_root(
                &public(next),
                &proposal,
                &sign(old, &bytes),
                &sign(next, &bytes),
                fixture.now,
            )
            .unwrap();
        let passkeys = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
        assert!(
            passkeys.list().unwrap().is_empty(),
            "rotation must retire the old factor even when its root key returns"
        );
    }
    assert_eq!(
        policy.current().unwrap().unwrap().owner_root_key,
        public(&root)
    );
    let authority = IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
    let request = passkey_request(&fixture, &authenticator, &[0]);
    let device = fixture.genuine(&authority, &request, 0);
    authenticator.counter += 1;
    let assertion = authenticator.get(&action_challenge(&request.action().unwrap()).unwrap());
    assert!(authority
        .introduce_threshold_with_passkey(
            &request,
            &fixture.destination_signature(&request),
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now
        )
        .is_err());
    assert!(!fixture.enrolled());
    register(&fixture, &mut authenticator, &root).unwrap();
    assert_eq!(
        PasskeyStore::open(fixture.home.path(), AUDIENCE)
            .unwrap()
            .list()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn registration_refuses_terminal_controls_and_non_origin_urls_before_signing() {
    let fixture = Threshold::new(2);
    install(&fixture);
    let authenticator = Authenticator::synced();
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    for origin in [
        "https://coven.example.org\nforged review",
        "https://coven.example.org\u{1b}[2J\u{1b}[Hforged review",
        "https://coven.example.org/path",
        "https://user:password@coven.example.org",
        "https://coven.example.org?query=1",
        "https://coven.example.org#fragment",
        "https://",
    ] {
        let mut request = registration(&fixture, &authenticator);
        request.origins = vec![origin.into()];
        assert!(
            store.registration_action(&request).is_err(),
            "origin must be rejected before it reaches the review: {origin:?}"
        );
    }
    for origin in [ORIGIN, "https://coven.example.org:8443"] {
        let mut request = registration(&fixture, &authenticator);
        request.origins = vec![origin.into()];
        store.registration_action(&request).unwrap();
    }
}

#[test]
fn a_registration_with_pending_audit_is_reconciled_after_reopen() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    let audit = fixture.home.path().join("mobile/audit.jsonl");
    let prior = fs::read(&audit).unwrap();
    fs::remove_file(&audit).unwrap();
    fs::create_dir(&audit).unwrap();
    let error = register(&fixture, &mut authenticator, &root).unwrap_err();
    assert!(format!("{error:#}").contains("audit"));
    fs::remove_dir(&audit).unwrap();
    atomic_replace_private(&audit, &prior).unwrap();
    let reopened = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert_eq!(reopened.list().unwrap().len(), 1);
    assert_eq!(
        audit_events(&fixture)
            .iter()
            .filter(|e| *e == "passkey_registered")
            .count(),
        1
    );
    assert_eq!(
        audit_events(&fixture)
            .iter()
            .filter(|e| *e == "passkey_registration_rejected")
            .count(),
        0
    );
    reopened.list().unwrap();
    assert_eq!(
        audit_events(&fixture)
            .iter()
            .filter(|e| *e == "passkey_registered")
            .count(),
        1
    );
}

#[test]
fn a_post_replacement_failure_never_reports_an_activated_factor_as_rejected() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    FAIL_AFTER_PASSKEY_REPLACE.with(|fail| fail.set(true));
    register(&fixture, &mut authenticator, &root).unwrap_err();
    assert_eq!(
        audit_events(&fixture)
            .iter()
            .filter(|e| *e == "passkey_registration_rejected")
            .count(),
        0
    );
    let reopened = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert_eq!(reopened.list().unwrap().len(), 1);
    assert_eq!(
        audit_events(&fixture)
            .iter()
            .filter(|e| *e == "passkey_registered")
            .count(),
        1
    );
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
fn registration_review_shows_the_signed_key_scope_and_validity_window() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let authenticator = Authenticator::synced();
    let registration = registration(&fixture, &authenticator);
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let action = store.registration_action(&registration).unwrap();
    let review =
        render_registration_review(&registration, &action, &store.owner_policy().unwrap()).unwrap();
    for value in [
        public(&root),
        store.owner_policy().unwrap().root_generation,
        public(&authenticator.key),
        authenticator.credential_id,
        "device_admin".into(),
        "owner.passkey.register".into(),
        "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc".into(),
        RP_ID.into(),
        ORIGIN.into(),
        "ISEhISEhISEhISEhISEhISEhISEhISEhISEhISEhISE".into(),
        "2027-01-15T08:00:00.000Z".into(),
        "2027-01-15T08:02:00.000Z".into(),
        action.effect_digest,
    ] {
        assert!(review.contains(&value), "review omitted {value}");
    }
    assert!(review.contains("never a device"));
    assert!(review.contains("at most one approval"));
}

#[test]
fn edited_registration_material_cannot_replace_the_root_approved_credential() {
    for field in ["publicKeyX963", "credentialId", "rpId", "origins"] {
        let fixture = Threshold::new(2);
        let root = install(&fixture);
        let mut authenticator = Authenticator::synced();
        register(&fixture, &mut authenticator, &root).unwrap();
        let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);

        // Keep the genuine registration action and root signature, but edit
        // the material they authorize. Checking only that signature would
        // incorrectly authorize the replacement key, credential or RP.
        let mut stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&store.path).unwrap()).unwrap();
        let replacement = match field {
            "publicKeyX963" => {
                authenticator.key = SigningKey::from_slice(&[63; 32]).unwrap();
                serde_json::json!(public(&authenticator.key))
            }
            "credentialId" => {
                authenticator.credential_id = URL_SAFE_NO_PAD.encode([64; 16]);
                serde_json::json!(authenticator.credential_id)
            }
            "rpId" => serde_json::json!("other.example.org"),
            "origins" => serde_json::json!(["https://other.example.org"]),
            _ => unreachable!(),
        };
        stored["credentials"][0][field] = replacement;
        fs::write(&store.path, serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(
            store
                .list()
                .map_or(true, |credentials| credentials.is_empty()),
            "editing {field} must not retain an authorized passkey"
        );

        let authority =
            IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
        let request = passkey_request(&fixture, &authenticator, &[0]);
        let device = fixture.genuine(&authority, &request, 0);
        authenticator.counter += 1;
        let challenge = action_challenge(&request.action().unwrap()).unwrap();
        let (origin, rp) = match field {
            "rpId" => (ORIGIN, "other.example.org"),
            "origins" => ("https://other.example.org", RP_ID),
            _ => (ORIGIN, RP_ID),
        };
        let assertion = authenticator.assert(&challenge, origin, rp, "webauthn.get");
        assert!(authority
            .introduce_threshold_with_passkey(
                &request,
                &fixture.destination_signature(&request),
                &[(fixture.approvers[0].id, &device)],
                Some(&assertion),
                fixture.now,
            )
            .is_err());
        assert!(!fixture.enrolled(), "editing {field} must enroll nothing");
    }
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
fn a_passkey_has_only_one_threshold_slot_and_no_slot_under_n_one() {
    for required in [1, 3] {
        let fixture = Threshold::new(required);
        let root = install(&fixture);
        let mut authenticator = Authenticator::synced();
        register(&fixture, &mut authenticator, &root).unwrap();
        let authority =
            IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
        let request = passkey_request(&fixture, &authenticator, &[0]);
        assert!(authority
            .admit_threshold(
                &request,
                &fixture.destination_signature(&request),
                fixture.now
            )
            .is_err());
        assert!(!fixture.enrolled());
        // Supplying multiple credential IDs cannot turn the single optional
        // slot into two passkey approvals, even before policy admission.
        let mut two = serde_json::to_value(&request).unwrap();
        two["passkeyApprover"] = serde_json::json!([
            authenticator.credential_id,
            URL_SAFE_NO_PAD.encode([65; 16]),
        ]);
        assert!(serde_json::from_value::<ThresholdIntroductionRequest>(two).is_err());
    }
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
    authenticator.counter = 0;
    let error = PasskeyStore::open(fixture.home.path(), AUDIENCE)
        .unwrap()
        .verify_and_advance(&authenticator.get(&challenge), &challenge)
        .unwrap_err();
    assert!(error.to_string().contains("did not advance"));
}

#[test]
fn an_existing_credential_cannot_change_its_backup_eligibility() {
    for initially_synced in [true, false] {
        let fixture = Threshold::new(2);
        let root = install(&fixture);
        let mut authenticator = Authenticator::synced();
        if !initially_synced {
            authenticator.flags = FLAG_USER_PRESENT | FLAG_USER_VERIFIED;
        }
        register(&fixture, &mut authenticator, &root).unwrap();
        let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
        let before = fs::read(&store.path).unwrap();
        authenticator.counter += 1;
        authenticator.flags = if initially_synced {
            FLAG_USER_PRESENT | FLAG_USER_VERIFIED
        } else {
            FLAG_USER_PRESENT | FLAG_USER_VERIFIED | FLAG_BACKUP_ELIGIBLE
        };
        let error = store
            .verify_and_advance(&authenticator.get(&[42; 32]), &[42; 32])
            .unwrap_err();
        assert!(error.to_string().contains("backup eligibility"));
        assert_eq!(fs::read(&store.path).unwrap(), before);
        assert!(!fixture.enrolled());
    }
}

#[test]
fn cross_origin_missing_presence_and_inconsistent_flags_enroll_nothing() {
    for invalid in [
        "cross_origin",
        "no_up",
        "no_uv",
        "backup_flags",
        "signature",
    ] {
        let fixture = Threshold::new(2);
        let root = install(&fixture);
        let mut authenticator = Authenticator::synced();
        register(&fixture, &mut authenticator, &root).unwrap();
        let authority =
            IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
        let request = passkey_request(&fixture, &authenticator, &[0]);
        let challenge = action_challenge(&request.action().unwrap()).unwrap();
        authenticator.counter += 1;
        let valid = authenticator.get(&challenge);
        let mut assertion = valid.clone();
        let mut auth_data = URL_SAFE_NO_PAD
            .decode(&assertion.authenticator_data)
            .unwrap();
        if invalid == "cross_origin" {
            let mut client: serde_json::Value = serde_json::from_slice(
                &URL_SAFE_NO_PAD.decode(&assertion.client_data_json).unwrap(),
            )
            .unwrap();
            client["crossOrigin"] = serde_json::json!(true);
            assertion.client_data_json =
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&client).unwrap());
        } else if invalid == "no_up" {
            auth_data[32] &= !FLAG_USER_PRESENT;
        } else if invalid == "no_uv" {
            auth_data[32] &= !FLAG_USER_VERIFIED;
        } else if invalid == "backup_flags" {
            auth_data[32] &= !FLAG_BACKUP_ELIGIBLE;
        }
        assertion.authenticator_data = URL_SAFE_NO_PAD.encode(&auth_data);
        let mut signed = auth_data;
        signed.extend_from_slice(&Sha256::digest(
            URL_SAFE_NO_PAD.decode(&assertion.client_data_json).unwrap(),
        ));
        let attacker = SigningKey::from_slice(&[63; 32]).unwrap();
        assertion.signature = sign(
            if invalid == "signature" {
                &attacker
            } else {
                &authenticator.key
            },
            &signed,
        );
        let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
        let before = fs::read(&store.path).unwrap();
        let device = fixture.genuine(&authority, &request, 0);
        assert!(
            authority
                .introduce_threshold_with_passkey(
                    &request,
                    &fixture.destination_signature(&request),
                    &[(fixture.approvers[0].id, &device)],
                    Some(&assertion),
                    fixture.now,
                )
                .is_err(),
            "{invalid}"
        );
        assert!(!fixture.enrolled(), "{invalid}");
        assert_eq!(fs::read(&store.path).unwrap(), before, "{invalid}");
        // The unchanged genuine assertion remains usable: the refusal above
        // was caused by the modified input, not an invalid fixture.
        store.verify_and_advance(&valid, &challenge).unwrap();
    }
}

#[test]
fn a_zero_counter_passkey_still_requires_a_device_and_cannot_replay_a_consumed_nonce() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let registration = registration(&fixture, &authenticator);
    let action = store.registration_action(&registration).unwrap();
    store
        .register(
            &registration,
            &authenticator.get(&action_challenge(&action).unwrap()),
            &sign(&root, &action.canonical_bytes().unwrap()),
            fixture.now,
        )
        .unwrap();
    let authority = IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
    let request = passkey_request(&fixture, &authenticator, &[0]);
    let signature = fixture.destination_signature(&request);
    let assertion = authenticator.get(&action_challenge(&request.action().unwrap()).unwrap());
    assert!(authority
        .introduce_threshold_with_passkey(&request, &signature, &[], Some(&assertion), fixture.now)
        .is_err());
    assert!(!fixture.enrolled());
    let device = fixture.genuine(&authority, &request, 0);
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

    let mut replay = request.clone();
    replay.destination_id = uuid::Uuid::from_u128(51);
    let destination = SigningKey::from_slice(&[41; 32]).unwrap();
    replay.destination_public_key = public(&destination);
    replay.grant = crate::mobile_memory::grant::DeviceGrant::for_device(
        replay.destination_id,
        &public(&destination),
        vec![DeviceScope::MemoryRead],
        fixture.now,
    )
    .unwrap();
    replay.grant.expires_at = Some(fixture.now + chrono::Duration::hours(1));
    let signature = sign(&destination, &replay.possession_bytes().unwrap());
    let device = fixture.genuine(&authority, &replay, 0);
    let assertion = authenticator.get(&action_challenge(&replay.action().unwrap()).unwrap());
    let error = authority
        .introduce_threshold_with_passkey(
            &replay,
            &signature,
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now,
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("consumed"));
    assert!(DeviceRegistry::load(fixture.home.path())
        .unwrap()
        .device(replay.destination_id)
        .unwrap()
        .is_none());
    // Zero means the authenticator does not implement a counter. Fresh exact
    // actions still work; the registry nonce ledger provides replay refusal.
    replay.nonce = [43; 32];
    let signature = sign(&destination, &replay.possession_bytes().unwrap());
    let device = fixture.genuine(&authority, &replay, 0);
    let assertion = authenticator.get(&action_challenge(&replay.action().unwrap()).unwrap());
    authority
        .introduce_threshold_with_passkey(
            &replay,
            &signature,
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now,
        )
        .unwrap();
    assert!(DeviceRegistry::load(fixture.home.path())
        .unwrap()
        .device(replay.destination_id)
        .unwrap()
        .is_some());
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
    let authority = IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
    let request = passkey_request(&fixture, &authenticator, &[0]);
    let signature = fixture.destination_signature(&request);
    let device = fixture.genuine(&authority, &request, 0);
    authenticator.counter += 1;
    let assertion = authenticator.get(&action_challenge(&request.action().unwrap()).unwrap());
    let error = authority
        .introduce_threshold_with_passkey(
            &request,
            &signature,
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now,
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("passkey is not registered"));
    assert!(!fixture.enrolled());
    // Re-registration needs the new root and restores only that explicitly
    // approved factor. The same otherwise-valid introduction can then finish.
    register(&fixture, &mut authenticator, &new_root).unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
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
fn policy_replacement_keeps_a_factor_but_reinstallation_retires_it() {
    use crate::mobile_memory::owner_policy::PolicyApproval;
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    let policy = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let head = policy.current().unwrap().unwrap();
    let mut tightened = head.policy.clone();
    tightened.maximum_step_up_age_seconds -= 1;
    let proposal = PolicyProposal {
        revision: head.revision + 1,
        previous_digest: Some(head.digest),
        policy: tightened,
        nonce: [93; 32],
        issued_at: fixture.now,
        expires_at: fixture.now + chrono::Duration::seconds(60),
    };
    let action = policy.replace_action(&proposal).unwrap();
    policy
        .replace(
            &proposal,
            &PolicyApproval::OwnerRoot {
                signature: sign(&root, &action.canonical_bytes().unwrap()),
            },
            fixture.now,
        )
        .unwrap();
    let passkeys = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let credential = passkeys.list().unwrap().pop().unwrap();
    assert_eq!(credential.owner_root_generation, head.root_generation);
    authenticator.counter += 1;
    let challenge = [84; 32];
    passkeys
        .verify_and_advance(&authenticator.get(&challenge), &challenge)
        .unwrap();

    // Model an explicit owner-policy reset, retaining the old passkey store.
    fs::remove_file(fixture.home.path().join("mobile/introduction-policy.json")).unwrap();
    let proposal = PolicyProposal {
        revision: 1,
        previous_digest: None,
        policy: fixture.policy.clone(),
        nonce: [94; 32],
        issued_at: fixture.now,
        expires_at: fixture.now + chrono::Duration::seconds(60),
    };
    let action = policy.install_action(&public(&root), &proposal).unwrap();
    policy
        .install(
            &public(&root),
            &proposal,
            &sign(&root, &action.canonical_bytes().unwrap()),
            fixture.now,
        )
        .unwrap();
    assert!(passkeys.list().unwrap().is_empty());
    authenticator.counter += 1;
    assert!(passkeys
        .verify_and_advance(&authenticator.get(&challenge), &challenge)
        .is_err());
}

#[test]
fn failed_registry_commit_spends_the_counter_without_enrolling_and_needs_a_fresh_attempt() {
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
    let challenge = action_challenge(&request.action().unwrap()).unwrap();
    let assertion = authenticator.get(&challenge);
    crate::mobile_memory::registry::fail_next_device_registry_write(
        &fixture.home.path().join("mobile/devices.json"),
    );
    let error = authority
        .introduce_threshold_with_passkey(
            &request,
            &signature,
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now,
        )
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("registry write failure"),
        "{error:#}"
    );
    assert!(!fixture.enrolled());
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert_eq!(store.list().unwrap()[0].sign_count, authenticator.counter);
    assert!(store.verify_and_advance(&assertion, &challenge).is_err());
    let mut fresh = passkey_request(&fixture, &authenticator, &[0]);
    fresh.nonce = [95; 32];
    fresh.grant.id = Uuid::new_v4();
    let signature = fixture.destination_signature(&fresh);
    authority
        .admit_threshold(&fresh, &signature, fixture.now)
        .unwrap();
    let device = fixture.genuine(&authority, &fresh, 0);
    authenticator.counter += 1;
    let assertion = authenticator.get(&action_challenge(&fresh.action().unwrap()).unwrap());
    authority
        .introduce_threshold_with_passkey(
            &fresh,
            &signature,
            &[(fixture.approvers[0].id, &device)],
            Some(&assertion),
            fixture.now,
        )
        .unwrap();
    assert!(fixture.enrolled());
}

#[test]
fn audit_delivered_before_a_missing_receipt_is_deduplicated_on_reopen() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    // Model a crash after audit append, before the outbox receipt replacement.
    let mut stored: StoredPasskeys =
        serde_json::from_slice(&fs::read(&store.path).unwrap()).unwrap();
    stored.credentials[0].registration_audited_at = None;
    store.write(&stored.credentials).unwrap();
    let reopened = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert!(reopened.list().unwrap()[0]
        .registration_audited_at
        .is_some());
    assert_eq!(
        audit_events(&fixture)
            .iter()
            .filter(|event| *event == "passkey_registered")
            .count(),
        1
    );
    reopened.list().unwrap();
    assert_eq!(
        audit_events(&fixture)
            .iter()
            .filter(|event| *event == "passkey_registered")
            .count(),
        1
    );
}

#[test]
fn generationless_preview_passkey_stores_are_refused() {
    let fixture = Threshold::new(2);
    let root = install(&fixture);
    let mut authenticator = Authenticator::synced();
    register(&fixture, &mut authenticator, &root).unwrap();
    let store = PasskeyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&store.path).unwrap()).unwrap();
    value["version"] = 1.into();
    value["credentials"][0]
        .as_object_mut()
        .unwrap()
        .remove("ownerRootGeneration");
    atomic_replace_private(&store.path, &serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(store.list().is_err());
    assert!(!fixture.enrolled());
}
