//! Synthetic owner-root access recovery; no daemon, relay, account or installed state.
use super::*;
use crate::mobile_memory::assurance::RequestedAssurance;
use crate::mobile_memory::introduction::IntroductionPolicy;
use crate::mobile_memory::owner_policy::{PolicyApproval, PolicyProposal};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};

const AUDIENCE: [u8; 32] = [7; 32];

fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
}

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
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

struct Fixture {
    home: tempfile::TempDir,
    now: DateTime<Utc>,
    root: SigningKey,
    destination: SigningKey,
    authority: AccessRecoveryAuthority,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let root = SigningKey::from_slice(&[11; 32]).unwrap();
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
        // Familiar state recovery must never touch.
        std::fs::create_dir_all(home.path().join("familiars")).unwrap();
        std::fs::write(
            home.path().join("familiars/astra.json"),
            b"{\"id\":\"astra\"}",
        )
        .unwrap();
        Self {
            authority: AccessRecoveryAuthority::load(home.path(), AUDIENCE).unwrap(),
            home,
            now,
            root,
            destination: SigningKey::from_slice(&[3; 32]).unwrap(),
        }
    }

    fn request(&self) -> AccessRecoveryRequest {
        let head = self.authority.current_policy().unwrap();
        let destination_id = Uuid::from_u128(2);
        let mut grant = DeviceGrant::for_device(
            destination_id,
            &public(&self.destination),
            vec![DeviceScope::MemoryRead],
            self.now,
        )
        .unwrap();
        grant.expires_at = Some(self.now + chrono::Duration::hours(1));
        AccessRecoveryRequest {
            version: 1,
            audience: AUDIENCE,
            policy_revision: head.revision,
            policy_head_digest: head.digest,
            destination_id,
            destination_public_key: public(&self.destination),
            device_name: "Replacement phone".into(),
            device_context: "Recovering after every trusted device was lost".into(),
            grant,
            nonce: [9; 32],
            issued_at: self.now,
            expires_at: self.now + chrono::Duration::seconds(120),
        }
    }

    fn signatures(&self, request: &AccessRecoveryRequest, root: &SigningKey) -> (String, String) {
        (
            sign(&self.destination, &request.possession_bytes().unwrap()),
            sign(root, &request.action().unwrap().canonical_bytes().unwrap()),
        )
    }

    fn enrolled(&self) -> bool {
        DeviceRegistry::load(self.home.path())
            .unwrap()
            .device(Uuid::from_u128(2))
            .unwrap()
            .is_some()
    }

    fn audit(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.path().join("mobile/audit.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["event"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
}

#[test]
fn owner_root_recovers_access_as_a_distinct_audited_event() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (destination, root) = fixture.signatures(&request, &fixture.root);
    let before = fixture.authority.current_policy().unwrap();
    let record = fixture
        .authority
        .recover(&request, &destination, &root, fixture.now)
        .unwrap();
    assert_eq!(record.device.id, request.destination_id);
    assert!(fixture.enrolled());
    assert_eq!(
        fixture.audit(),
        ["introduction_policy_installed", "access_recovered"]
    );
    // Recovering access rotates nothing: same owner root, same policy head,
    // and familiar state is byte-for-byte untouched.
    let after = fixture.authority.current_policy().unwrap();
    assert_eq!(after.owner_root_key, before.owner_root_key);
    assert_eq!(after.digest, before.digest);
    assert_eq!(
        std::fs::read(fixture.home.path().join("familiars/astra.json")).unwrap(),
        b"{\"id\":\"astra\"}"
    );
    // The registry still serializes ordinary introductions without a kind,
    // so only recovery records carry one.
    let registry = std::fs::read_to_string(fixture.home.path().join("mobile/devices.json"))
        .unwrap_or_default();
    assert!(registry.contains("\"kind\": \"access_recovery\""));
}

#[test]
fn the_policy_lock_is_held_from_head_check_through_the_recovery_write() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (destination, root) = fixture.signatures(&request, &fixture.root);
    let store = OwnerPolicyStore::open(fixture.home.path(), AUDIENCE).unwrap();
    assert!(!store.is_locked().unwrap());

    // Every check has passed and nothing is written yet: a root rotation or
    // policy replacement here would retire the request, so it must not land.
    let home = fixture.home.path().to_owned();
    let observed = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = observed.clone();
    BEFORE_RECOVERY_WRITE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let store = OwnerPolicyStore::open(&home, AUDIENCE).unwrap();
            seen.set(Some(store.is_locked().unwrap()));
        }));
    });
    fixture
        .authority
        .recover(&request, &destination, &root, fixture.now)
        .unwrap();
    assert!(fixture.enrolled());
    assert_eq!(observed.get(), Some(true));
    assert!(!store.is_locked().unwrap());
}

#[test]
fn recovery_cannot_be_replayed() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (destination, root) = fixture.signatures(&request, &fixture.root);
    fixture
        .authority
        .recover(&request, &destination, &root, fixture.now)
        .unwrap();
    assert!(fixture
        .authority
        .recover(&request, &destination, &root, fixture.now)
        .is_err());
}

#[test]
fn a_relay_or_account_service_cannot_recover_without_the_owner_root() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let service = SigningKey::from_slice(&[66; 32]).unwrap();
    let (destination, forged) = fixture.signatures(&request, &service);
    let error = fixture
        .authority
        .recover(&request, &destination, &forged, fixture.now)
        .unwrap_err();
    assert!(format!("{error:#}").contains("owner-root signature"));
    // Nor can it substitute its own destination key under a genuine root
    // signature: the transcript binds the destination key.
    let (_, root) = fixture.signatures(&request, &fixture.root);
    let impostor = sign(&service, &request.possession_bytes().unwrap());
    assert!(fixture
        .authority
        .recover(&request, &impostor, &root, fixture.now)
        .is_err());
    assert!(!fixture.enrolled());
    assert_eq!(
        fixture.audit(),
        [
            "introduction_policy_installed",
            "access_recovery_rejected",
            "access_recovery_rejected"
        ]
    );
}

#[test]
fn tampered_expired_or_out_of_policy_recoveries_fail_closed() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (destination, root) = fixture.signatures(&request, &fixture.root);

    let mut tampered = request.clone();
    tampered.grant.scopes = vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin];
    assert!(fixture
        .authority
        .recover(&tampered, &destination, &root, fixture.now)
        .is_err());

    assert!(fixture
        .authority
        .recover(&request, &destination, &root, request.expires_at)
        .is_err());

    let mut broad = fixture.request();
    broad.grant.scopes = vec![DeviceScope::SecretsRead];
    broad.grant.subject_key_id = request.grant.subject_key_id.clone();
    let (d, r) = fixture.signatures(&broad, &fixture.root);
    assert!(fixture
        .authority
        .recover(&broad, &d, &r, fixture.now)
        .is_err());

    let mut long = fixture.request();
    long.grant.expires_at = Some(fixture.now + chrono::Duration::days(2));
    let (d, r) = fixture.signatures(&long, &fixture.root);
    assert!(fixture
        .authority
        .recover(&long, &d, &r, fixture.now)
        .is_err());

    let mut foreign = fixture.request();
    foreign.audience = [8; 32];
    let (d, r) = fixture.signatures(&foreign, &fixture.root);
    assert!(fixture
        .authority
        .recover(&foreign, &d, &r, fixture.now)
        .is_err());
    assert!(!fixture.enrolled());
}

#[test]
fn a_policy_change_retires_an_unused_root_signature() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (destination, root) = fixture.signatures(&request, &fixture.root);
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
    let error = fixture
        .authority
        .recover(&request, &destination, &root, fixture.now)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("another installation or policy state"));
    assert!(!fixture.enrolled());
}

#[test]
fn review_names_every_signed_field_and_what_recovery_does_not_do() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let action = request.action().unwrap();
    let review = render_recovery_review(&request, &action);
    for expected in [
        "owner.access_recovery.approve".to_owned(),
        action.target.clone(),
        format!("revision 1 ({})", request.policy_head_digest),
        request.destination_id.to_string(),
        "Replacement phone".into(),
        "Recovering after every trusted device was lost".into(),
        request.grant.id.to_string(),
        "memory_read".into(),
        action.nonce.clone(),
        action.effect_digest.clone(),
        "does not rotate the owner-root key".into(),
        "familiar identity".into(),
    ] {
        assert!(
            review.contains(&expected),
            "review omits {expected}:\n{review}"
        );
    }
}
