//! Synthetic owner-root and device keys; no daemon, relay, account or installed state.
use super::*;
use crate::mobile_memory::assurance::{
    AssuranceClass, AssuranceContextMode, AssuranceProofBinding, CanonicalAssuranceProof,
    NewAuthorizationKey,
};
use crate::mobile_memory::grant::DeviceGrant;
use crate::mobile_memory::introduction::IntroductionAuthority;
use crate::mobile_memory::registry::DeviceRecord;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};

const AUDIENCE: [u8; 32] = [7; 32];

fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_encoded_point(false).as_bytes())
}

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
}

struct Fixture {
    home: tempfile::TempDir,
    now: DateTime<Utc>,
    root: SigningKey,
    device: SigningKey,
    step_up: SigningKey,
    device_id: Uuid,
    grant_id: Uuid,
    store: OwnerPolicyStore,
}

fn base_policy(device_id: Uuid) -> IntroductionPolicy {
    IntroductionPolicy {
        eligible_introducers: vec![device_id],
        required_approvals: 1,
        allowed_scopes: vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin],
        maximum_grant_lifetime_seconds: 86400,
        required_assurance: RequestedAssurance::FreshUserVerification,
        maximum_step_up_age_seconds: 60,
        minimum_attestation: Default::default(),
    }
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let root = SigningKey::from_slice(&[11; 32]).unwrap();
        let device = SigningKey::from_slice(&[1; 32]).unwrap();
        let step_up = SigningKey::from_slice(&[2; 32]).unwrap();
        let device_id = Uuid::from_u128(1);
        let scopes = vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin];
        let mut grant =
            DeviceGrant::for_device(device_id, &public(&device), scopes.clone(), now).unwrap();
        grant.expires_at = Some(now + chrono::Duration::days(2));
        DeviceRegistry::load(home.path())
            .unwrap()
            .register_with_grant_and_authorization(
                DeviceRecord {
                    id: device_id,
                    display_name: "Synthetic approver".into(),
                    public_key_x963: public(&device),
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
        Self {
            home,
            now,
            root,
            device,
            step_up,
            device_id,
            grant_id: grant.id,
            store,
        }
    }

    fn proposal(
        &self,
        revision: u64,
        previous: Option<String>,
        policy: IntroductionPolicy,
    ) -> PolicyProposal {
        PolicyProposal {
            revision,
            previous_digest: previous,
            policy,
            nonce: [revision as u8 + 40; 32],
            issued_at: self.now,
            expires_at: self.now + chrono::Duration::seconds(120),
        }
    }

    fn install(&self) -> InstalledPolicy {
        let proposal = self.proposal(1, None, base_policy(self.device_id));
        let action = self
            .store
            .install_action(&public(&self.root), &proposal)
            .unwrap();
        self.store
            .install(
                &public(&self.root),
                &proposal,
                &sign(&self.root, &action.canonical_bytes().unwrap()),
                self.now,
            )
            .unwrap()
    }

    fn next(&self, head: &InstalledPolicy, policy: IntroductionPolicy) -> PolicyProposal {
        self.proposal(head.revision + 1, Some(head.digest.clone()), policy)
    }

    fn root_approval(&self, proposal: &PolicyProposal) -> PolicyApproval {
        let action = self.store.replace_action(proposal).unwrap();
        PolicyApproval::OwnerRoot {
            signature: sign(&self.root, &action.canonical_bytes().unwrap()),
        }
    }

    fn device_approval(&self, proposal: &PolicyProposal) -> PolicyApproval {
        let challenge = self
            .store
            .issue_replacement_challenge(self.device_id, self.now)
            .unwrap();
        let mut action = self.store.replace_action(proposal).unwrap();
        action.operation = "devices.introduction_policy.replace".into();
        let key = DeviceRegistry::load(self.home.path())
            .unwrap()
            .authorization_key(self.device_id)
            .unwrap()
            .unwrap();
        let mut step_up = PresentedAssuranceProof {
            context_mode: AssuranceContextMode::Action,
            challenge: challenge.challenge,
            issued_at: self.now,
            expires_at: challenge.expires_at,
            requested_assurance: RequestedAssurance::FreshBiometric,
            signature: String::new(),
        };
        step_up.signature = sign(
            &self.step_up,
            &CanonicalAssuranceProof::for_action(
                AssuranceProofBinding {
                    device_id: self.device_id,
                    grant_id: self.grant_id,
                    revocation_epoch: 0,
                    authorization_key_id: key.subject_key_id,
                },
                &step_up,
                &action,
            )
            .unwrap()
            .canonical_bytes(),
        );
        PolicyApproval::Introducer {
            device_id: self.device_id,
            possession_signature: sign(&self.device, &action.canonical_bytes().unwrap()),
            step_up,
        }
    }

    fn audit_events(&self) -> Vec<String> {
        let path = self.home.path().join("mobile").join("audit.jsonl");
        std::fs::read_to_string(path)
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

    fn policy_path(&self) -> PathBuf {
        self.home.path().join("mobile").join(POLICY_FILE)
    }
}

fn tighter(device_id: Uuid) -> IntroductionPolicy {
    let mut policy = base_policy(device_id);
    policy.allowed_scopes = vec![DeviceScope::MemoryRead];
    policy.required_assurance = RequestedAssurance::FreshBiometric;
    policy.maximum_step_up_age_seconds = 30;
    policy
}

fn broader(device_id: Uuid) -> IntroductionPolicy {
    let mut policy = base_policy(device_id);
    policy.maximum_grant_lifetime_seconds = 7 * 86400;
    policy
}

#[test]
fn install_binds_root_and_is_audited_once() {
    let fixture = Fixture::new();
    assert!(fixture.store.current().unwrap().is_none());
    let head = fixture.install();
    assert_eq!(head.revision, 1);
    assert_eq!(head.owner_root_key, public(&fixture.root));
    assert_eq!(fixture.audit_events(), ["introduction_policy_installed"]);
    let stored = std::fs::read_to_string(fixture.policy_path()).unwrap();
    assert!(!stored.contains(&URL_SAFE_NO_PAD.encode(fixture.root.to_bytes())));

    let again = fixture.proposal(1, None, base_policy(fixture.device_id));
    let action = fixture
        .store
        .install_action(&public(&fixture.root), &again)
        .unwrap();
    let error = fixture
        .store
        .install(
            &public(&fixture.root),
            &again,
            &sign(&fixture.root, &action.canonical_bytes().unwrap()),
            fixture.now,
        )
        .unwrap_err();
    assert!(error.to_string().contains("already installed"));
}

#[test]
fn install_requires_possession_of_the_registered_root() {
    let fixture = Fixture::new();
    let attacker = SigningKey::from_slice(&[66; 32]).unwrap();
    let proposal = fixture.proposal(1, None, base_policy(fixture.device_id));
    let action = fixture
        .store
        .install_action(&public(&fixture.root), &proposal)
        .unwrap();
    // Registering the owner's key with a signature from another key fails.
    assert!(fixture
        .store
        .install(
            &public(&fixture.root),
            &proposal,
            &sign(&attacker, &action.canonical_bytes().unwrap()),
            fixture.now,
        )
        .is_err());
    // A chain cannot start mid-history.
    let mid = fixture.proposal(
        2,
        Some(URL_SAFE_NO_PAD.encode([1; 32])),
        base_policy(fixture.device_id),
    );
    let action = fixture
        .store
        .install_action(&public(&fixture.root), &mid)
        .unwrap();
    assert!(fixture
        .store
        .install(
            &public(&fixture.root),
            &mid,
            &sign(&fixture.root, &action.canonical_bytes().unwrap()),
            fixture.now,
        )
        .is_err());
    assert!(fixture.store.current().unwrap().is_none());
    assert_eq!(
        fixture.audit_events(),
        [
            "introduction_policy_rejected",
            "introduction_policy_rejected"
        ]
    );
}

#[test]
fn owner_root_may_downgrade_and_the_chain_advances() {
    let fixture = Fixture::new();
    let head = fixture.install();
    let proposal = fixture.next(&head, broader(fixture.device_id));
    let replaced = fixture
        .store
        .replace(&proposal, &fixture.root_approval(&proposal), fixture.now)
        .unwrap();
    assert_eq!(replaced.revision, 2);
    assert_eq!(replaced.policy.maximum_grant_lifetime_seconds, 7 * 86400);
    assert_eq!(
        fixture.audit_events(),
        [
            "introduction_policy_installed",
            "introduction_policy_replaced"
        ]
    );
    // Root-approved heads re-verify on every load.
    assert_eq!(
        fixture.store.current().unwrap().unwrap().digest,
        replaced.digest
    );
}

#[test]
fn trusted_device_may_tighten_but_never_downgrade() {
    let fixture = Fixture::new();
    let head = fixture.install();
    let downgrade = fixture.next(&head, broader(fixture.device_id));
    let error = fixture
        .store
        .replace(
            &downgrade,
            &fixture.device_approval(&downgrade),
            fixture.now,
        )
        .unwrap_err();
    assert!(error.to_string().contains("owner-root step-up"));
    assert_eq!(fixture.store.current().unwrap().unwrap().revision, 1);

    let tighten = fixture.next(&head, tighter(fixture.device_id));
    let replaced = fixture
        .store
        .replace(&tighten, &fixture.device_approval(&tighten), fixture.now)
        .unwrap();
    assert_eq!(replaced.revision, 2);
    assert_eq!(replaced.owner_root_key, head.owner_root_key);
    assert_eq!(
        fixture.audit_events(),
        [
            "introduction_policy_installed",
            "introduction_policy_rejected",
            "introduction_policy_replaced"
        ]
    );
}

#[test]
fn signed_replacements_cannot_be_replayed_or_rolled_back() {
    let fixture = Fixture::new();
    let head = fixture.install();
    let first = fixture.next(&head, broader(fixture.device_id));
    let first_approval = fixture.root_approval(&first);
    let second_head = fixture
        .store
        .replace(&first, &first_approval, fixture.now)
        .unwrap();
    // Replay of the committed signature no longer extends the head.
    assert!(fixture
        .store
        .replace(&first, &first_approval, fixture.now)
        .is_err());

    let restore = fixture.next(&second_head, base_policy(fixture.device_id));
    let restore_approval = fixture.root_approval(&restore);
    fixture
        .store
        .replace(&restore, &restore_approval, fixture.now)
        .unwrap();
    // Re-applying an older signed transition (rollback) is refused.
    assert!(fixture
        .store
        .replace(&first, &first_approval, fixture.now)
        .is_err());
    assert_eq!(fixture.store.current().unwrap().unwrap().revision, 3);
}

#[test]
fn tampered_expired_or_foreign_approvals_fail_closed() {
    let fixture = Fixture::new();
    let head = fixture.install();
    let proposal = fixture.next(&head, broader(fixture.device_id));
    let approval = fixture.root_approval(&proposal);

    // Tampered transcript: signed proposal, broader policy submitted.
    let mut tampered = proposal.clone();
    tampered.policy.maximum_grant_lifetime_seconds += 1;
    assert!(fixture
        .store
        .replace(&tampered, &approval, fixture.now)
        .is_err());

    // Expired and not-yet-valid approvals.
    assert!(fixture
        .store
        .replace(&proposal, &approval, proposal.expires_at)
        .is_err());
    assert!(fixture
        .store
        .replace(
            &proposal,
            &approval,
            fixture.now - chrono::Duration::seconds(1)
        )
        .is_err());
    let mut long = proposal.clone();
    long.expires_at = long.issued_at + chrono::Duration::seconds(301);
    // Over-long windows cannot even be encoded for signing, and a signature
    // over the in-window action does not transfer to the long one.
    assert!(fixture
        .store
        .replace_action(&long)
        .unwrap()
        .canonical_bytes()
        .is_err());
    assert!(fixture
        .store
        .replace(&long, &approval, fixture.now)
        .is_err());

    // Malicious relay/account service: its own key cannot stand in for the
    // registered owner root, and a store for another installation is refused.
    let service = SigningKey::from_slice(&[99; 32]).unwrap();
    let forged = PolicyApproval::OwnerRoot {
        signature: sign(
            &service,
            &fixture
                .store
                .replace_action(&proposal)
                .unwrap()
                .canonical_bytes()
                .unwrap(),
        ),
    };
    assert!(fixture
        .store
        .replace(&proposal, &forged, fixture.now)
        .is_err());
    let other = OwnerPolicyStore::open(fixture.home.path(), [8; 32]).unwrap();
    assert!(other.current().is_err());
    assert!(other.replace(&proposal, &approval, fixture.now).is_err());

    // The intact approval still applies once.
    fixture
        .store
        .replace(&proposal, &approval, fixture.now)
        .unwrap();
}

#[test]
fn ineligible_or_revoked_devices_cannot_approve() {
    let fixture = Fixture::new();
    let head = fixture.install();
    let tighten = fixture.next(&head, tighter(fixture.device_id));
    let approval = fixture.device_approval(&tighten);
    DeviceRegistry::load(fixture.home.path())
        .unwrap()
        .revoke(fixture.device_id, fixture.now)
        .unwrap();
    assert!(fixture
        .store
        .replace(&tighten, &approval, fixture.now)
        .is_err());
    assert!(fixture
        .store
        .issue_replacement_challenge(Uuid::from_u128(9), fixture.now)
        .is_err());
    assert_eq!(fixture.store.current().unwrap().unwrap().revision, 1);
}

#[test]
fn edited_store_is_refused_on_load() {
    let fixture = Fixture::new();
    fixture.install();
    let path = fixture.policy_path();
    let original = std::fs::read_to_string(&path).unwrap();

    let widened = original.replace(
        "\"maximumGrantLifetimeSeconds\": 86400",
        "\"maximumGrantLifetimeSeconds\": 864000",
    );
    assert_ne!(widened, original);
    atomic_replace_private(&path, widened.as_bytes()).unwrap();
    assert!(fixture.store.current().is_err());
    assert!(IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).is_err());

    // Recomputing the digest still fails the root signature check.
    let mut value: serde_json::Value = serde_json::from_str(&widened).unwrap();
    let stored: StoredOwnerPolicy = serde_json::from_value(value.clone()).unwrap();
    let digest = transition_digest(
        &stored.audience,
        &stored.owner_root_key,
        stored.revision,
        None,
        &stored.policy,
    )
    .unwrap();
    value["digest"] = digest.clone().into();
    value["approval"]["action"]["effectDigest"] = digest.into();
    atomic_replace_private(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    let error = fixture.store.current().unwrap_err();
    assert!(format!("{error:#}").contains("root signature"));
}

#[test]
fn installed_authority_retires_after_replacement() {
    let fixture = Fixture::new();
    let head = fixture.install();
    let authority = IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
    assert_eq!(
        authority.policy_digest().unwrap(),
        base_policy(fixture.device_id).digest().unwrap()
    );
    let tighten = fixture.next(&head, tighter(fixture.device_id));
    fixture
        .store
        .replace(&tighten, &fixture.root_approval(&tighten), fixture.now)
        .unwrap();
    let reloaded = IntroductionAuthority::load_installed(fixture.home.path(), AUDIENCE).unwrap();
    assert_eq!(
        reloaded.policy_digest().unwrap(),
        tighter(fixture.device_id).digest().unwrap()
    );
    assert!(IntroductionAuthority::load_installed(fixture.home.path(), [8; 32]).is_err());
}

#[test]
fn unchanged_or_uninstalled_replacements_are_refused() {
    let fixture = Fixture::new();
    let early = fixture.proposal(
        2,
        Some(URL_SAFE_NO_PAD.encode([1; 32])),
        base_policy(fixture.device_id),
    );
    assert!(fixture.store.replace_action(&early).is_err());
    let head = fixture.install();
    let same = fixture.next(&head, base_policy(fixture.device_id));
    let error = fixture
        .store
        .replace(&same, &fixture.root_approval(&same), fixture.now)
        .unwrap_err();
    assert!(error.to_string().contains("unchanged"));
}

#[test]
fn strictness_covers_every_policy_dimension() {
    let id = Uuid::from_u128(1);
    let base = base_policy(id);
    assert!(is_at_least_as_strict(&base, &base));
    assert!(is_at_least_as_strict(&tighter(id), &base));
    assert!(!is_at_least_as_strict(&base, &tighter(id)));
    let mut more_introducers = base.clone();
    more_introducers
        .eligible_introducers
        .push(Uuid::from_u128(2));
    let mut more_scopes = base_policy(id);
    more_scopes.allowed_scopes = vec![
        DeviceScope::MemoryRead,
        DeviceScope::MessageSend,
        DeviceScope::DeviceAdmin,
    ];
    more_scopes.allowed_scopes.sort();
    let mut longer_step_up = base.clone();
    longer_step_up.maximum_step_up_age_seconds = 120;
    let mut weaker = tighter(id);
    weaker.required_assurance = RequestedAssurance::FreshUserVerification;
    for widened in [more_introducers, more_scopes, longer_step_up, broader(id)] {
        assert!(!is_at_least_as_strict(&widened, &base));
    }
    assert!(!is_at_least_as_strict(&weaker, &tighter(id)));
}
