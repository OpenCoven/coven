//! Synthetic owner-root rotation; no daemon, relay, account or installed state.
use super::*;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};

const AUDIENCE: [u8; 32] = [7; 32];

fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_encoded_point(false).as_bytes())
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
        required_assurance: RequestedAssurance::FreshUserVerification,
        maximum_step_up_age_seconds: 60,
        minimum_attestation: Default::default(),
    }
}

struct Fixture {
    home: tempfile::TempDir,
    now: DateTime<Utc>,
    old: SigningKey,
    new: SigningKey,
    store: OwnerPolicyStore,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let old = SigningKey::from_slice(&[11; 32]).unwrap();
        let store = OwnerPolicyStore::open(home.path(), AUDIENCE).unwrap();
        let proposal = PolicyProposal {
            revision: 1,
            previous_digest: None,
            policy: policy(),
            nonce: [1; 32],
            issued_at: now,
            expires_at: now + chrono::Duration::seconds(60),
        };
        let action = store.install_action(&public(&old), &proposal).unwrap();
        store
            .install(
                &public(&old),
                &proposal,
                &sign(&old, &action.canonical_bytes().unwrap()),
                now,
            )
            .unwrap();
        Self {
            home,
            now,
            old,
            new: SigningKey::from_slice(&[12; 32]).unwrap(),
            store,
        }
    }

    fn proposal(&self, policy: IntroductionPolicy) -> PolicyProposal {
        let head = self.store.current().unwrap().unwrap();
        PolicyProposal {
            revision: head.revision + 1,
            previous_digest: Some(head.digest),
            policy,
            nonce: [head.revision as u8 + 20; 32],
            issued_at: self.now,
            expires_at: self.now + chrono::Duration::seconds(120),
        }
    }

    fn signatures(
        &self,
        incoming: &SigningKey,
        proposal: &PolicyProposal,
        by_old: &SigningKey,
        by_new: &SigningKey,
    ) -> (String, String) {
        let bytes = self
            .store
            .rotation_action(&public(incoming), proposal)
            .unwrap()
            .canonical_bytes()
            .unwrap();
        (sign(by_old, &bytes), sign(by_new, &bytes))
    }

    fn rotate(&self) -> InstalledPolicy {
        let proposal = self.proposal(policy());
        let (old, new) = self.signatures(&self.new, &proposal, &self.old, &self.new);
        self.store
            .rotate_root(&public(&self.new), &proposal, &old, &new, self.now)
            .unwrap()
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
fn rotation_is_an_explicit_audited_succession() {
    let fixture = Fixture::new();
    let before = fixture.store.current().unwrap().unwrap();
    let head = fixture.rotate();
    assert_eq!(head.owner_root_key, public(&fixture.new));
    assert_eq!(head.revision, before.revision + 1);
    assert_eq!(
        head.policy.digest().unwrap(),
        before.policy.digest().unwrap()
    );
    assert_ne!(head.digest, before.digest);
    assert_eq!(
        fixture.audit(),
        ["introduction_policy_installed", "owner_root_rotated"]
    );
    // The rotation head re-verifies on load.
    assert_eq!(
        fixture.store.current().unwrap().unwrap().digest,
        head.digest
    );
}

#[test]
fn rotation_needs_both_the_outgoing_and_incoming_root() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let service = SigningKey::from_slice(&[66; 32]).unwrap();
    // Only the new key, only the old key, or a relay/account key installing
    // itself: each is refused.
    for (by_old, by_new, incoming) in [
        (&fixture.new, &fixture.new, &fixture.new),
        (&fixture.old, &fixture.old, &fixture.new),
        (&service, &service, &service),
        (&fixture.old, &service, &fixture.new),
    ] {
        let (old, new) = fixture.signatures(incoming, &proposal, by_old, by_new);
        assert!(fixture
            .store
            .rotate_root(&public(incoming), &proposal, &old, &new, fixture.now)
            .is_err());
    }
    // A genuine pair of signatures for one successor cannot install another.
    let (old, new) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
    assert!(fixture
        .store
        .rotate_root(&public(&service), &proposal, &old, &new, fixture.now)
        .is_err());
    assert_eq!(
        fixture.store.current().unwrap().unwrap().owner_root_key,
        public(&fixture.old)
    );
    assert_eq!(
        fixture
            .audit()
            .iter()
            .filter(|e| *e == "owner_root_rotation_rejected")
            .count(),
        5
    );
}

#[test]
fn rotation_changes_only_the_key() {
    let fixture = Fixture::new();
    let mut changed = policy();
    changed.maximum_grant_lifetime_seconds = 7 * 86400;
    let proposal = fixture.proposal(changed);
    let (old, new) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
    let error = fixture
        .store
        .rotate_root(&public(&fixture.new), &proposal, &old, &new, fixture.now)
        .unwrap_err();
    assert!(error.to_string().contains("keep the current policy"));

    let same = fixture.proposal(policy());
    let (old, new) = fixture.signatures(&fixture.old, &same, &fixture.old, &fixture.old);
    assert!(fixture
        .store
        .rotate_root(&public(&fixture.old), &same, &old, &new, fixture.now)
        .is_err());
}

#[test]
fn after_rotation_only_the_new_root_approves() {
    let fixture = Fixture::new();
    fixture.rotate();
    let downgrade = {
        let mut broad = policy();
        broad.maximum_grant_lifetime_seconds = 7 * 86400;
        fixture.proposal(broad)
    };
    let action = fixture.store.replace_action(&downgrade).unwrap();
    let by_old = PolicyApproval::OwnerRoot {
        signature: sign(&fixture.old, &action.canonical_bytes().unwrap()),
    };
    assert!(fixture
        .store
        .replace(&downgrade, &by_old, fixture.now)
        .is_err());
    let by_new = PolicyApproval::OwnerRoot {
        signature: sign(&fixture.new, &action.canonical_bytes().unwrap()),
    };
    fixture
        .store
        .replace(&downgrade, &by_new, fixture.now)
        .unwrap();
}

#[test]
fn rotation_cannot_be_replayed_or_edited_on_disk() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let (old, new) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
    fixture
        .store
        .rotate_root(&public(&fixture.new), &proposal, &old, &new, fixture.now)
        .unwrap();
    assert!(fixture
        .store
        .rotate_root(&public(&fixture.new), &proposal, &old, &new, fixture.now)
        .is_err());

    // Swapping in another incoming key, even with a recomputed digest, fails
    // the possession signature check on load.
    let path = fixture.home.path().join("mobile").join(POLICY_FILE);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let attacker = SigningKey::from_slice(&[66; 32]).unwrap();
    value["ownerRootKey"] = public(&attacker).into();
    let stored: StoredOwnerPolicy = serde_json::from_value(value.clone()).unwrap();
    let digest = transition_digest(
        &stored.audience,
        &stored.owner_root_key,
        stored.revision,
        stored.previous_digest.as_deref(),
        &stored.policy,
    )
    .unwrap();
    value["digest"] = digest.clone().into();
    value["approval"]["action"]["effectDigest"] = digest.into();
    atomic_replace_private(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    let error = fixture.store.current().unwrap_err();
    assert!(format!("{error:#}").contains("rotation"));
}

#[test]
fn rotation_retires_recovery_approvals_bound_to_the_old_head() {
    use crate::mobile_memory::grant::DeviceGrant;
    use crate::mobile_memory::recovery::{AccessRecoveryAuthority, AccessRecoveryRequest};
    let fixture = Fixture::new();
    let head = fixture.store.current().unwrap().unwrap();
    let destination = SigningKey::from_slice(&[3; 32]).unwrap();
    let id = Uuid::from_u128(2);
    let mut grant = DeviceGrant::for_device(
        id,
        &public(&destination),
        vec![DeviceScope::MemoryRead],
        fixture.now,
    )
    .unwrap();
    grant.expires_at = Some(fixture.now + chrono::Duration::hours(1));
    let request = AccessRecoveryRequest {
        version: 1,
        audience: AUDIENCE,
        policy_revision: head.revision,
        policy_head_digest: head.digest,
        destination_id: id,
        destination_public_key: public(&destination),
        device_name: "Replacement phone".into(),
        device_context: "Signed before the owner rotated the root".into(),
        grant,
        nonce: [9; 32],
        issued_at: fixture.now,
        expires_at: fixture.now + chrono::Duration::seconds(120),
    };
    let destination_signature = sign(&destination, &request.possession_bytes().unwrap());
    let root_signature = sign(
        &fixture.old,
        &request.action().unwrap().canonical_bytes().unwrap(),
    );
    fixture.rotate();
    let recovery = AccessRecoveryAuthority::load(fixture.home.path(), AUDIENCE).unwrap();
    assert!(recovery
        .recover(
            &request,
            &destination_signature,
            &root_signature,
            fixture.now
        )
        .is_err());
}
