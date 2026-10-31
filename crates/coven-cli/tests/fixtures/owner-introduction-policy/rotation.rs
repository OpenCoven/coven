//! Synthetic owner-root rotation; no daemon, relay, account or installed state.
use super::*;
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

    /// Refusal leaves the old root installed and audits exactly one rejection.
    fn refused(
        &self,
        incoming: &SigningKey,
        proposal: &PolicyProposal,
        previous_signature: &str,
        new_signature: &str,
    ) -> String {
        let before = self.store.current().unwrap().unwrap();
        let rejected = self.rejections();
        let error = self
            .store
            .rotate_root(
                &public(incoming),
                proposal,
                previous_signature,
                new_signature,
                self.now,
            )
            .unwrap_err();
        let after = self.store.current().unwrap().unwrap();
        assert_eq!(after.digest, before.digest);
        assert_eq!(after.owner_root_key, before.owner_root_key);
        assert_eq!(self.rejections(), rejected + 1);
        format!("{error:#}")
    }

    fn rejections(&self) -> usize {
        self.audit()
            .iter()
            .filter(|event| *event == "owner_root_rotation_rejected")
            .count()
    }

    /// Every file under the home except the policy head and the audit log.
    fn untouched_state(&self) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        fn walk(dir: &Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if !path.ends_with(POLICY_FILE) && !path.ends_with("audit.jsonl") {
                    out.push((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(self.home.path(), &mut out);
        out.sort();
        out
    }
}

fn service_key() -> SigningKey {
    // Stands in for a relay, hosted-account or any other non-owner key.
    SigningKey::from_slice(&[66; 32]).unwrap()
}

fn recovery_request(
    fixture: &Fixture,
    head: &InstalledPolicy,
    seed: u8,
) -> (
    crate::mobile_memory::recovery::AccessRecoveryRequest,
    SigningKey,
) {
    use crate::mobile_memory::grant::DeviceGrant;
    let destination = SigningKey::from_slice(&[seed; 32]).unwrap();
    let id = Uuid::from_u128(u128::from(seed));
    let mut grant = DeviceGrant::for_device(
        id,
        &public(&destination),
        vec![DeviceScope::MemoryRead],
        fixture.now,
    )
    .unwrap();
    grant.expires_at = Some(fixture.now + chrono::Duration::hours(1));
    let request = crate::mobile_memory::recovery::AccessRecoveryRequest {
        version: 1,
        audience: AUDIENCE,
        policy_revision: head.revision,
        policy_head_digest: head.digest.clone(),
        destination_id: id,
        destination_public_key: public(&destination),
        device_name: "Replacement phone".into(),
        device_context: "Synthetic recovery".into(),
        grant,
        // Only uniqueness matters; a fixed value would be a reused nonce.
        nonce: rand::random(),
        issued_at: fixture.now,
        expires_at: fixture.now + chrono::Duration::seconds(120),
    };
    (request, destination)
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
    // The rotation head re-verifies both signatures on load.
    assert_eq!(
        fixture.store.current().unwrap().unwrap().digest,
        head.digest
    );
}

#[test]
fn rotation_without_the_outgoing_root_signature_is_refused() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let (_, new) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
    // Absent, or supplied by the incoming key in the outgoing slot.
    let error = fixture.refused(&fixture.new, &proposal, "", &new);
    assert!(error.contains("outgoing owner-root signature is invalid"));
    let error = fixture.refused(&fixture.new, &proposal, &new, &new);
    assert!(error.contains("outgoing owner-root signature is invalid"));
}

#[test]
fn rotation_without_the_incoming_root_signature_is_refused() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let (old, _) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
    // Absent, or the outgoing root vouching for a key it does not hold.
    let error = fixture.refused(&fixture.new, &proposal, &old, "");
    assert!(error.contains("incoming owner-root signature is invalid"));
    let error = fixture.refused(&fixture.new, &proposal, &old, &old);
    assert!(error.contains("incoming owner-root signature is invalid"));
}

#[test]
fn a_relay_or_account_key_cannot_install_itself_as_owner_root() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let service = service_key();
    // The service signs both slots for itself: it holds the incoming key but
    // never the outgoing root.
    let (old, new) = fixture.signatures(&service, &proposal, &service, &service);
    let error = fixture.refused(&service, &proposal, &old, &new);
    assert!(error.contains("outgoing owner-root signature is invalid"));
}

#[test]
fn a_genuine_signature_pair_cannot_be_reused_for_another_successor() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let service = service_key();
    let (old, new) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
    // The pair as signed, offered for another incoming key.
    let error = fixture.refused(&service, &proposal, &old, &new);
    assert!(error.contains("outgoing owner-root signature is invalid"));
    // The genuine outgoing signature plus a genuine possession signature by
    // the other successor over its own action: the outgoing root never
    // signed that action, because the action binds the incoming key.
    let (_, by_service) = fixture.signatures(&service, &proposal, &service, &service);
    let error = fixture.refused(&service, &proposal, &old, &by_service);
    assert!(error.contains("outgoing owner-root signature is invalid"));
    // The intended rotation still succeeds with the untouched pair.
    fixture
        .store
        .rotate_root(&public(&fixture.new), &proposal, &old, &new, fixture.now)
        .unwrap();
}

#[test]
fn a_policy_change_cannot_be_smuggled_into_a_rotation() {
    let fixture = Fixture::new();
    // Each field of the policy, changed under signatures from both roots.
    let mutations: [fn(&mut IntroductionPolicy); 4] = [
        |policy| policy.maximum_grant_lifetime_seconds = 7 * 86400,
        |policy| policy.allowed_scopes = vec![DeviceScope::MemoryRead],
        |policy| policy.eligible_introducers.push(Uuid::from_u128(9)),
        |policy| policy.maximum_step_up_age_seconds = 120,
    ];
    for mutate in mutations {
        let mut changed = policy();
        mutate(&mut changed);
        let proposal = fixture.proposal(changed);
        let (old, new) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
        let error = fixture.refused(&fixture.new, &proposal, &old, &new);
        assert!(error.contains("keep the current policy"), "{error}");
    }
}

#[test]
fn rotation_to_the_same_key_is_refused() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let (old, new) = fixture.signatures(&fixture.old, &proposal, &fixture.old, &fixture.old);
    let error = fixture.refused(&fixture.old, &proposal, &old, &new);
    assert!(error.contains("must change the key"));
}

#[test]
fn a_rotation_cannot_be_replayed() {
    let fixture = Fixture::new();
    let proposal = fixture.proposal(policy());
    let (old, new) = fixture.signatures(&fixture.new, &proposal, &fixture.old, &fixture.new);
    fixture
        .store
        .rotate_root(&public(&fixture.new), &proposal, &old, &new, fixture.now)
        .unwrap();
    // The same envelope again no longer extends the chain.
    let error = fixture.refused(&fixture.new, &proposal, &old, &new);
    assert!(error.contains("does not extend the current chain"));
    // Moved to the next chain position, the successor is now the installed
    // key, so the envelope is refused before any signature is checked.
    let next = fixture.proposal(policy());
    let error = fixture.refused(&fixture.new, &next, &old, &new);
    assert!(error.contains("must change the key"));
}

#[test]
fn an_incoming_key_edited_on_disk_is_refused_on_load() {
    let fixture = Fixture::new();
    fixture.rotate();
    // Swapping in another incoming key, even with a recomputed digest, fails
    // the possession signature check on load.
    let path = fixture.home.path().join("mobile").join(POLICY_FILE);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["ownerRootKey"] = public(&service_key()).into();
    let stored: StoredOwnerPolicy = serde_json::from_value(value.clone()).unwrap();
    let digest = transition_digest(
        &stored.audience,
        &stored.owner_root_key,
        stored.revision,
        stored.previous_digest.as_deref(),
        &stored.policy,
        stored.root_generation.as_deref(),
    )
    .unwrap();
    value["digest"] = digest.clone().into();
    value["approval"]["action"]["effectDigest"] = digest.into();
    atomic_replace_private(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    let error = fixture.store.current().unwrap_err();
    assert!(format!("{error:#}").contains("rotation"));
}

#[test]
fn the_old_root_is_rejected_after_rotation() {
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
    // Nor can the old root rotate the root back to itself.
    let back = fixture.proposal(policy());
    let (old, new) = fixture.signatures(&fixture.old, &back, &fixture.old, &fixture.old);
    let error = fixture.refused(&fixture.old, &back, &old, &new);
    assert!(error.contains("outgoing owner-root signature is invalid"));
    let by_new = PolicyApproval::OwnerRoot {
        signature: sign(&fixture.new, &action.canonical_bytes().unwrap()),
    };
    fixture
        .store
        .replace(&downgrade, &by_new, fixture.now)
        .unwrap();
}

#[test]
fn rotation_leaves_devices_grants_and_familiars_unchanged() {
    use crate::mobile_memory::recovery::AccessRecoveryAuthority;
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.home.path().join("familiars")).unwrap();
    std::fs::write(
        fixture.home.path().join("familiars/astra.json"),
        b"{\"id\":\"astra\"}",
    )
    .unwrap();
    // A device and grant enrolled under the old root.
    let head = fixture.store.current().unwrap().unwrap();
    let (request, destination) = recovery_request(&fixture, &head, 3);
    AccessRecoveryAuthority::load(fixture.home.path(), AUDIENCE)
        .unwrap()
        .recover(
            &request,
            &sign(&destination, &request.possession_bytes().unwrap()),
            &sign(
                &fixture.old,
                &request.action().unwrap().canonical_bytes().unwrap(),
            ),
            fixture.now,
        )
        .unwrap();
    let before = fixture.untouched_state();
    assert!(before
        .iter()
        .any(|(path, _)| path.ends_with("familiars/astra.json")));
    fixture.rotate();
    assert_eq!(fixture.untouched_state(), before);
}

#[test]
fn a_stale_recovery_approval_is_retired_by_rotation() {
    use crate::mobile_memory::recovery::AccessRecoveryAuthority;
    let fixture = Fixture::new();
    let old_head = fixture.store.current().unwrap().unwrap();
    // Signed by the outgoing root before the rotation, never used.
    let (stale, destination) = recovery_request(&fixture, &old_head, 3);
    let stale_destination = sign(&destination, &stale.possession_bytes().unwrap());
    let stale_root = sign(
        &fixture.old,
        &stale.action().unwrap().canonical_bytes().unwrap(),
    );
    let new_head = fixture.rotate();
    let recovery = AccessRecoveryAuthority::load(fixture.home.path(), AUDIENCE).unwrap();
    let error = recovery
        .recover(&stale, &stale_destination, &stale_root, fixture.now)
        .unwrap_err();
    assert!(error.to_string().contains("policy state"), "{error:#}");
    // Even the new root cannot revive a request bound to the old head.
    let by_new = sign(
        &fixture.new,
        &stale.action().unwrap().canonical_bytes().unwrap(),
    );
    let error = recovery
        .recover(&stale, &stale_destination, &by_new, fixture.now)
        .unwrap_err();
    assert!(error.to_string().contains("policy state"), "{error:#}");
    // A request bound to the new head and signed by the new root recovers.
    let (fresh, destination) = recovery_request(&fixture, &new_head, 4);
    recovery
        .recover(
            &fresh,
            &sign(&destination, &fresh.possession_bytes().unwrap()),
            &sign(
                &fixture.new,
                &fresh.action().unwrap().canonical_bytes().unwrap(),
            ),
            fixture.now,
        )
        .unwrap();
}
