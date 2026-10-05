//! Synthetic signed local requests; no daemon, relay, account or installed state.
use super::*;
use crate::mobile_memory::assurance::{
    AssuranceClass, AssuranceContextMode, AssuranceProofBinding, CanonicalAssuranceProof,
    NewAuthorizationKey,
};
use crate::mobile_memory::config::atomic_replace_private;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use std::sync::{Arc, Barrier};

struct Fixture {
    home: tempfile::TempDir,
    now: DateTime<Utc>,
    policy: IntroductionPolicy,
    source: SigningKey,
    step_up: SigningKey,
    destination: SigningKey,
    request: IntroductionRequest,
}

fn public(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_encoded_point(false).as_bytes())
}

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    let signature: Signature = key.sign(bytes);
    URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let source = SigningKey::from_slice(&[1; 32]).unwrap();
        let step_up = SigningKey::from_slice(&[2; 32]).unwrap();
        let destination = SigningKey::from_slice(&[3; 32]).unwrap();
        let source_id = Uuid::from_u128(1);
        let scopes = vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin];
        let record = DeviceRecord {
            id: source_id,
            display_name: "Synthetic introducer".into(),
            public_key_x963: public(&source),
            paired_at: now,
            revoked_at: None,
            suspended_at: None,
            scopes: scopes.clone(),
        };
        let mut grant =
            DeviceGrant::for_device(source_id, &public(&source), scopes.clone(), now).unwrap();
        grant.expires_at = Some(now + chrono::Duration::days(2));
        DeviceRegistry::load(home.path())
            .unwrap()
            .register_with_grant_and_authorization(
                record,
                grant.clone(),
                Some(NewAuthorizationKey {
                    public_key_x963: public(&step_up),
                    assurance_class: AssuranceClass::BiometricOnly,
                    enrolled_at: now,
                }),
            )
            .unwrap();
        let policy = IntroductionPolicy {
            eligible_introducers: vec![source_id],
            required_approvals: 1,
            allowed_scopes: scopes,
            maximum_grant_lifetime_seconds: 86400,
            required_assurance: RequestedAssurance::FreshBiometric,
            maximum_step_up_age_seconds: 30,
        };
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
            audience: [7; 32],
            policy_digest: policy.digest().unwrap(),
            introducer_id: source_id,
            introducer_grant_id: grant.id,
            introducer_revocation_epoch: grant.revocation_epoch,
            destination_id,
            destination_public_key: public(&destination),
            device_name: "Synthetic endpoint".into(),
            device_context: "Self-built test desktop / local fixture".into(),
            grant: destination_grant,
            nonce: [9; 32],
            issued_at: now,
            expires_at: now + chrono::Duration::seconds(60),
        };
        Self {
            home,
            now,
            policy,
            source,
            step_up,
            destination,
            request,
        }
    }

    fn authority(&self) -> IntroductionAuthority {
        IntroductionAuthority::load(self.home.path(), [7; 32], Some(self.policy.clone())).unwrap()
    }

    fn approval(&self, authority: &IntroductionAuthority) -> IntroductionApproval {
        let challenge = authority.issue_challenge(&self.request, self.now).unwrap();
        let key = authority
            .registry
            .authorization_key(self.request.introducer_id)
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
        let action = self.request.action().unwrap();
        proof.signature = sign(
            &self.step_up,
            &CanonicalAssuranceProof::for_action(
                AssuranceProofBinding {
                    device_id: self.request.introducer_id,
                    grant_id: self.request.introducer_grant_id,
                    revocation_epoch: self.request.introducer_revocation_epoch,
                    authorization_key_id: key.subject_key_id,
                },
                &proof,
                &action,
            )
            .unwrap()
            .canonical_bytes(),
        );
        IntroductionApproval {
            possession_signature: sign(&self.source, &action.canonical_bytes().unwrap()),
            step_up: proof,
        }
    }

    fn destination_signature(&self) -> String {
        sign(&self.destination, &self.request.possession_bytes().unwrap())
    }

    fn assert_absent(&self) {
        assert!(DeviceRegistry::load(self.home.path())
            .unwrap()
            .device(self.request.destination_id)
            .unwrap()
            .is_none());
    }
}

#[test]
fn trusted_device_introduction_commits_exact_grant_and_private_audit_once() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    let enrolled = authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap();
    assert_eq!(enrolled.grant, f.request.grant);
    assert_eq!(
        enrolled.device.public_key_x963,
        f.request.destination_public_key
    );
    assert_eq!(enrolled.device.display_name, f.request.device_name);
    let reloaded = DeviceRegistry::load(f.home.path()).unwrap();
    assert_eq!(
        reloaded
            .authorization_record(enrolled.device.id)
            .unwrap()
            .unwrap(),
        enrolled
    );
    let source = reloaded
        .authorization_record(f.request.introducer_id)
        .unwrap()
        .unwrap();
    assert_eq!(source.grant.id, f.request.introducer_grant_id);
    assert!(source.device.revoked_at.is_none());
    assert!(reloaded
        .authorization_key(f.request.destination_id)
        .unwrap()
        .is_none());
    let audit = std::fs::read_to_string(f.home.path().join("mobile/audit.jsonl")).unwrap();
    let value: serde_json::Value = serde_json::from_str(audit.trim()).unwrap();
    assert_eq!(value["event"], "device_introduced");
    assert_eq!(value.as_object().unwrap().len(), 3);
    for private in [
        &f.request.destination_public_key,
        &f.request.device_name,
        &f.request.device_context,
        &approval.step_up.signature,
        &f.request.introducer_id.to_string(),
    ] {
        assert!(!audit.contains(private), "audit leaked request material");
    }
    assert_eq!(f.authority().reconcile_audits(f.now).unwrap(), 0);
    assert!(f
        .authority()
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap_err()
        .to_string()
        .contains("already consumed"));
}

#[test]
fn trusted_device_introduction_has_no_policy_or_threshold_fallback() {
    let f = Fixture::new();
    assert!(IntroductionAuthority::load(f.home.path(), [7; 32], None).is_err());
    for count in [0, 2, 3, u16::MAX] {
        let mut policy = f.policy.clone();
        policy.required_approvals = count;
        assert!(IntroductionAuthority::load(f.home.path(), [7; 32], Some(policy)).is_err());
    }
    for age in [0, 121] {
        let mut policy = f.policy.clone();
        policy.maximum_step_up_age_seconds = age;
        assert!(IntroductionAuthority::load(f.home.path(), [7; 32], Some(policy)).is_err());
    }
    f.assert_absent();
}

#[test]
fn trusted_device_introduction_binds_every_material_field() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    let signature = f.destination_signature();
    let mutations: Vec<fn(&mut IntroductionRequest)> = vec![
        |r| r.version = 2,
        |r| r.audience = [8; 32],
        |r| r.policy_digest = URL_SAFE_NO_PAD.encode([1; 32]),
        |r| r.destination_id = Uuid::from_u128(9),
        |r| {
            r.destination_public_key = public(&SigningKey::from_slice(&[4; 32]).unwrap());
            r.grant = DeviceGrant::for_device(
                r.destination_id,
                &r.destination_public_key,
                r.grant.scopes.clone(),
                r.issued_at,
            )
            .unwrap();
            r.grant.expires_at = Some(r.issued_at + chrono::Duration::hours(1));
        },
        |r| r.grant.scopes.push(DeviceScope::DeviceAdmin),
        |r| r.device_name.push_str(" changed"),
        |r| r.device_context.push_str(" changed"),
        |r| r.nonce = [10; 32],
        |r| r.expires_at += chrono::Duration::seconds(1),
        |r| r.grant.expires_at = Some(r.issued_at + chrono::Duration::hours(2)),
        |r| r.grant.id = Uuid::from_u128(90),
        |r| r.introducer_grant_id = Uuid::from_u128(91),
        |r| r.introducer_revocation_epoch = 1,
        |r| r.issued_at -= chrono::Duration::seconds(1),
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let mut request = f.request.clone();
        mutate(&mut request);
        assert!(
            authority
                .introduce(&request, &signature, &approval, f.now)
                .is_err(),
            "mutation {index} accepted"
        );
        f.assert_absent();
    }
    authority
        .introduce(&f.request, &signature, &approval, f.now)
        .unwrap();
}

#[test]
fn trusted_device_introduction_requires_both_possession_keys_and_verified_step_up() {
    for wrong in 0..5 {
        let f = Fixture::new();
        let authority = f.authority();
        let mut approval = f.approval(&authority);
        let mut destination = f.destination_signature();
        match wrong {
            0 => destination = sign(&f.source, &f.request.possession_bytes().unwrap()),
            1 => {
                approval.possession_signature = sign(
                    &f.destination,
                    &f.request.action().unwrap().canonical_bytes().unwrap(),
                )
            }
            2 => approval.step_up.signature = sign(&f.source, b"fresh_biometric"),
            3 => {
                approval.step_up.requested_assurance = RequestedAssurance::FreshUserVerification;
                let key = authority
                    .registry
                    .authorization_key(f.request.introducer_id)
                    .unwrap()
                    .unwrap();
                // A valid signature at a weaker requested assurance must still
                // fail the explicitly configured biometric minimum.
                approval.step_up.signature = sign(
                    &f.step_up,
                    &CanonicalAssuranceProof::for_action(
                        AssuranceProofBinding {
                            device_id: f.request.introducer_id,
                            grant_id: f.request.introducer_grant_id,
                            revocation_epoch: f.request.introducer_revocation_epoch,
                            authorization_key_id: key.subject_key_id,
                        },
                        &approval.step_up,
                        &f.request.action().unwrap(),
                    )
                    .unwrap()
                    .canonical_bytes(),
                );
            }
            4 => approval.step_up.signature.clear(),
            _ => unreachable!(),
        }
        assert!(authority
            .introduce(&f.request, &destination, &approval, f.now)
            .is_err());
        f.assert_absent();
    }
}

#[test]
fn trusted_device_introduction_reloads_revocation_suspension_grants_and_keys() {
    for mutation in 0..5 {
        let f = Fixture::new();
        let authority = f.authority();
        let approval = f.approval(&authority);
        let registry = DeviceRegistry::load(f.home.path()).unwrap();
        match mutation {
            0 => registry.revoke(f.request.introducer_id, f.now).unwrap(),
            1 => registry.suspend(f.request.introducer_id, f.now).unwrap(),
            2 => registry
                .revoke_authorization_key(f.request.introducer_id, f.now)
                .unwrap(),
            3 | 4 => {
                let current = registry
                    .authorization_record(f.request.introducer_id)
                    .unwrap()
                    .unwrap()
                    .grant;
                let scopes = if mutation == 3 {
                    current.scopes.clone()
                } else {
                    vec![DeviceScope::MemoryRead]
                };
                let grant = current
                    .reissue(
                        &public(&f.source),
                        scopes,
                        current.restrictions.clone(),
                        current.minimum_assurance,
                        f.now,
                        f.now + chrono::Duration::hours(1),
                    )
                    .unwrap();
                registry
                    .replace_grant(f.request.introducer_id, grant)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            authority
                .introduce(&f.request, &f.destination_signature(), &approval, f.now)
                .is_err(),
            "mutation {mutation} accepted"
        );
        f.assert_absent();
    }
}

#[test]
fn trusted_device_introduction_refuses_time_boundaries_and_wrong_instances() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    for now in [
        f.now - chrono::Duration::milliseconds(1),
        f.now + chrono::Duration::seconds(31),
        f.request.expires_at,
    ] {
        assert!(authority
            .introduce(&f.request, &f.destination_signature(), &approval, now)
            .is_err());
    }
    let other =
        IntroductionAuthority::load(f.home.path(), [8; 32], Some(f.policy.clone())).unwrap();
    assert!(other
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .is_err());
    f.assert_absent();
    authority
        .introduce(
            &f.request,
            &f.destination_signature(),
            &approval,
            f.now + chrono::Duration::seconds(30),
        )
        .unwrap();
}

#[test]
fn trusted_device_introduction_concurrent_stale_instances_have_one_winner() {
    let f = Fixture::new();
    let approvals: Vec<_> = (0..8)
        .map(|_| {
            let a = f.authority();
            let proof = f.approval(&a);
            (a, proof)
        })
        .collect();
    let barrier = Arc::new(Barrier::new(8));
    let winners = std::thread::scope(|scope| {
        let workers: Vec<_> = approvals
            .into_iter()
            .map(|(authority, approval)| {
                let barrier = barrier.clone();
                let request = &f.request;
                let signature = f.destination_signature();
                scope.spawn(move || {
                    barrier.wait();
                    authority
                        .introduce(request, &signature, &approval, f.now)
                        .is_ok()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| usize::from(w.join().unwrap()))
            .sum::<usize>()
    });
    assert_eq!(winners, 1);
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.home.path().join("mobile/devices.json")).unwrap())
            .unwrap();
    assert_eq!(stored["devices"].as_array().unwrap().len(), 2);
    assert_eq!(
        stored["introductionTransitions"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn trusted_device_introduction_failed_registry_commit_burns_proof_without_success() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    crate::mobile_memory::registry::fail_next_device_registry_write(
        &f.home.path().join("mobile/devices.json"),
    );
    assert!(authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .is_err());
    f.assert_absent();
    let restarted = f.authority();
    assert!(restarted
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .is_err());
    assert_eq!(restarted.reconcile_audits(f.now).unwrap(), 0);
    f.assert_absent();
    let new_approval = f.approval(&restarted);
    restarted
        .introduce(&f.request, &f.destination_signature(), &new_approval, f.now)
        .unwrap();
}

#[test]
fn trusted_device_introduction_reconciles_only_committed_audit_after_restart() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    let audit = f.home.path().join("mobile/audit.jsonl");
    std::fs::create_dir(&audit).unwrap();
    let error = authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("committed; audit delivery pending"),
        "{error:#}"
    );
    assert!(DeviceRegistry::load(f.home.path())
        .unwrap()
        .device(f.request.destination_id)
        .unwrap()
        .is_some());
    std::fs::remove_dir(&audit).unwrap();
    let restarted = f.authority();
    assert_eq!(restarted.reconcile_audits(f.now).unwrap(), 1);
    assert_eq!(restarted.reconcile_audits(f.now).unwrap(), 0);
    assert_eq!(std::fs::read_to_string(&audit).unwrap().lines().count(), 1);
    assert!(restarted
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .is_err());
}

#[test]
fn trusted_device_introduction_corrupt_consumption_state_fails_closed() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    let path = f.home.path().join("mobile/devices.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["introductionTransitions"] = serde_json::json!([{"transitionId": Uuid::new_v4(), "nonceDigest":"malformed", "occurredAt":f.now,"auditedAt":null}]);
    atomic_replace_private(&path, &serde_json::to_vec(&stored).unwrap()).unwrap();
    assert!(authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .is_err());
    assert!(authority.reconcile_audits(f.now).is_err());
}

#[test]
fn trusted_device_introduction_expired_source_and_missing_admin_fail_closed() {
    for missing_admin in [false, true] {
        let mut f = Fixture::new();
        let registry = DeviceRegistry::load(f.home.path()).unwrap();
        let current = registry
            .authorization_record(f.request.introducer_id)
            .unwrap()
            .unwrap()
            .grant;
        let scopes = if missing_admin {
            vec![DeviceScope::MemoryRead]
        } else {
            current.scopes.clone()
        };
        let grant = current
            .reissue(
                &public(&f.source),
                scopes,
                current.restrictions.clone(),
                current.minimum_assurance,
                f.now,
                f.now + chrono::Duration::seconds(2),
            )
            .unwrap();
        f.request.introducer_grant_id = grant.id;
        f.request.introducer_revocation_epoch = grant.revocation_epoch;
        f.request.grant.expires_at = grant.expires_at;
        registry
            .replace_grant(f.request.introducer_id, grant)
            .unwrap();
        let authority = f.authority();
        if missing_admin {
            assert!(authority.issue_challenge(&f.request, f.now).is_err());
        } else {
            let approval = f.approval(&authority);
            assert!(authority
                .introduce(
                    &f.request,
                    &f.destination_signature(),
                    &approval,
                    f.now + chrono::Duration::seconds(2)
                )
                .is_err());
        }
        f.assert_absent();
    }
}

#[test]
fn trusted_device_introduction_policy_and_delegation_constraints_are_enforced_before_signing() {
    for change in 0..7 {
        let mut f = Fixture::new();
        match change {
            0 => f.policy.eligible_introducers = vec![Uuid::from_u128(50)],
            1 => f.policy.allowed_scopes = vec![DeviceScope::DeviceAdmin],
            2 => f.policy.maximum_grant_lifetime_seconds = 10,
            3 => f.request.grant.expires_at = None,
            4 => f.request.grant.scopes = vec![DeviceScope::IdentityAdmin],
            5 => f.request.grant.not_before = f.now + chrono::Duration::seconds(1),
            6 => {
                let registry = DeviceRegistry::load(f.home.path()).unwrap();
                let current = registry
                    .authorization_record(f.request.introducer_id)
                    .unwrap()
                    .unwrap()
                    .grant;
                let mut restrictions = current.restrictions.clone();
                restrictions.require_fresh_user_verification_for = vec![DeviceScope::MemoryRead];
                let grant = current
                    .reissue(
                        &public(&f.source),
                        current.scopes.clone(),
                        restrictions,
                        current.minimum_assurance,
                        f.now,
                        f.now + chrono::Duration::days(1),
                    )
                    .unwrap();
                f.request.introducer_grant_id = grant.id;
                f.request.introducer_revocation_epoch = grant.revocation_epoch;
                registry
                    .replace_grant(f.request.introducer_id, grant)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        f.request.policy_digest = f.policy.digest().unwrap();
        assert!(
            f.authority().issue_challenge(&f.request, f.now).is_err(),
            "constraint {change} ignored"
        );
        f.assert_absent();
    }
}

#[test]
fn trusted_device_introduction_assurance_class_and_key_epoch_are_authoritative() {
    for change in 0..3 {
        let f = Fixture::new();
        let authority = f.authority();
        let approval = f.approval(&authority);
        let path = f.home.path().join("mobile/authorization-keys.json");
        let mut stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match change {
            0 => stored["keys"][0]["assuranceClass"] = "user_verification".into(),
            1 => stored["keys"][0]["assuranceClass"] = "device_credential".into(),
            2 => stored["keys"][0]["keyEpoch"] = 2.into(),
            _ => unreachable!(),
        }
        atomic_replace_private(&path, &serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(authority
            .introduce(&f.request, &f.destination_signature(), &approval, f.now)
            .is_err());
        f.assert_absent();
    }
}

#[test]
fn trusted_device_introduction_explicit_user_verification_policy_accepts_its_key_class() {
    let mut f = Fixture::new();
    f.policy.required_assurance = RequestedAssurance::FreshUserVerification;
    f.request.policy_digest = f.policy.digest().unwrap();
    let path = f.home.path().join("mobile/authorization-keys.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["keys"][0]["assuranceClass"] = "user_verification".into();
    atomic_replace_private(&path, &serde_json::to_vec(&stored).unwrap()).unwrap();
    let authority = f.authority();
    // Even requesting biometric in the signed proof only produces the enrolled
    // key's UserVerification ceiling; this explicit owner policy permits that.
    let approval = f.approval(&authority);
    authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap();
}

#[test]
fn trusted_device_introduction_audit_append_before_receipt_restart_deduplicates() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap();
    let path = f.home.path().join("mobile/devices.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["introductionTransitions"][0]["auditedAt"] = serde_json::Value::Null;
    atomic_replace_private(&path, &serde_json::to_vec(&stored).unwrap()).unwrap();
    assert_eq!(f.authority().reconcile_audits(f.now).unwrap(), 1);
    assert_eq!(
        std::fs::read_to_string(f.home.path().join("mobile/audit.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn trusted_device_introduction_consumption_survives_other_writers_and_forget_all() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap();
    let registry = DeviceRegistry::load(f.home.path()).unwrap();
    registry.suspend(f.request.destination_id, f.now).unwrap();
    registry.resume(f.request.destination_id).unwrap();
    registry.forget_all().unwrap();
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.home.path().join("mobile/devices.json")).unwrap())
            .unwrap();
    assert_eq!(
        stored["introductionTransitions"].as_array().unwrap().len(),
        1
    );
    assert!(stored["devices"].as_array().unwrap().is_empty());
    assert!(f
        .authority()
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap_err()
        .to_string()
        .contains("already consumed"));
}

#[test]
fn trusted_device_introduction_post_replacement_error_is_uncertain_until_reconciled() {
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    crate::mobile_memory::registry::fail_after_introduction_write(
        &f.home.path().join("mobile/devices.json"),
    );
    let error = authority
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .unwrap_err();
    assert!(error.is::<IntroductionCommitUncertain>(), "{error:#}");
    let audit = std::fs::read_to_string(f.home.path().join("mobile/audit.jsonl")).unwrap();
    assert!(audit.contains("device_introduction_uncertain"));
    assert!(!audit.contains("device_introduction_rejected"));
    let restarted = f.authority();
    assert_eq!(restarted.reconcile_audits(f.now).unwrap(), 1);
    assert!(restarted
        .introduce(&f.request, &f.destination_signature(), &approval, f.now)
        .is_err());
    assert_eq!(
        DeviceRegistry::load(f.home.path())
            .unwrap()
            .authorization_record(f.request.destination_id)
            .unwrap()
            .unwrap()
            .grant,
        f.request.grant
    );
}

#[test]
fn trusted_device_introduction_does_not_assert_transport_evidence() {
    let mut f = Fixture::new();
    let registry = DeviceRegistry::load(f.home.path()).unwrap();
    let current = registry
        .authorization_record(f.request.introducer_id)
        .unwrap()
        .unwrap()
        .grant;
    let mut restrictions = current.restrictions.clone();
    restrictions.transport = GrantTransportConstraint::DirectOnly;
    let grant = current
        .reissue(
            &public(&f.source),
            current.scopes.clone(),
            restrictions,
            current.minimum_assurance,
            f.now,
            f.now + chrono::Duration::days(1),
        )
        .unwrap();
    f.request.introducer_grant_id = grant.id;
    f.request.introducer_revocation_epoch = grant.revocation_epoch;
    registry
        .replace_grant(f.request.introducer_id, grant)
        .unwrap();
    assert!(f
        .authority()
        .issue_challenge(&f.request, f.now)
        .unwrap_err()
        .to_string()
        .contains("transport restriction"));
    f.assert_absent();
}

#[test]
fn trusted_device_introduction_process_worker() {
    use std::io::Read;
    let Some(home) = std::env::var_os("COVEN_INTRODUCTION_TEST_HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let worker = std::env::var("COVEN_INTRODUCTION_TEST_WORKER").unwrap();
    let input: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("request-fixture.json")).unwrap()).unwrap();
    let request: IntroductionRequest = serde_json::from_value(input["request"].clone()).unwrap();
    let policy = IntroductionPolicy {
        eligible_introducers: vec![request.introducer_id],
        required_approvals: 1,
        allowed_scopes: vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin],
        maximum_grant_lifetime_seconds: 86400,
        required_assurance: RequestedAssurance::FreshBiometric,
        maximum_step_up_age_seconds: 30,
    };
    let authority = IntroductionAuthority::load(&home, [7; 32], Some(policy)).unwrap();
    let approval = IntroductionApproval {
        possession_signature: input["possessionSignature"].as_str().unwrap().into(),
        step_up: PresentedAssuranceProof {
            context_mode: AssuranceContextMode::Action,
            challenge: serde_json::from_value(input["challenge"].clone()).unwrap(),
            issued_at: request.issued_at,
            expires_at: request.expires_at,
            requested_assurance: RequestedAssurance::FreshBiometric,
            signature: input["stepUpSignature"].as_str().unwrap().into(),
        },
    };
    std::fs::write(home.join(format!("ready-{worker}")), b"ready").unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
    let result = match authority.introduce(
        &request,
        input["destinationSignature"].as_str().unwrap(),
        &approval,
        request.issued_at,
    ) {
        Ok(_) => "committed",
        Err(error) if error.to_string().contains("already consumed") => "consumed",
        Err(error) => panic!("unexpected introduction error: {error:#}"),
    };
    std::fs::write(home.join(format!("result-{worker}")), result).unwrap();
}

#[test]
fn trusted_device_introduction_independent_processes_consume_once() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let f = Fixture::new();
    let authority = f.authority();
    let approval = f.approval(&authority);
    std::fs::write(
        f.home.path().join("request-fixture.json"),
        serde_json::to_vec(&serde_json::json!({
            "request": f.request, "possessionSignature": approval.possession_signature,
            "challenge": approval.step_up.challenge, "stepUpSignature": approval.step_up.signature,
            "destinationSignature": f.destination_signature(),
        }))
        .unwrap(),
    )
    .unwrap();
    let mut workers: Vec<_> = (0..2).map(|worker| {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "mobile_memory::introduction::tests::trusted_device_introduction_process_worker", "--nocapture"])
            .env("COVEN_INTRODUCTION_TEST_HOME", f.home.path())
            .env("COVEN_INTRODUCTION_TEST_WORKER", worker.to_string())
            .env("COVEN_HOME", f.home.path())
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap()
    }).collect();
    let started = std::time::Instant::now();
    while !(0..2).all(|worker| f.home.path().join(format!("ready-{worker}")).exists()) {
        // Hang guard only; process readiness, not elapsed time, is the barrier.
        if started.elapsed() > std::time::Duration::from_secs(60) {
            for child in &mut workers {
                let _ = child.kill();
                let _ = child.wait();
            }
            panic!(
                "introduction processes did not reach barrier after {:?}",
                started.elapsed()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for child in &mut workers {
        child.stdin.take().unwrap().write_all(&[1]).unwrap();
    }
    let started = std::time::Instant::now();
    while !workers
        .iter_mut()
        .all(|child| child.try_wait().unwrap().is_some())
    {
        // Hang guard only, so a lock regression fails rather than wedging CI.
        if started.elapsed() > std::time::Duration::from_secs(60) {
            for child in &mut workers {
                let _ = child.kill();
                let _ = child.wait();
            }
            panic!(
                "introduction processes did not finish after {:?}",
                started.elapsed()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for child in workers {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let mut outcomes: Vec<_> = (0..2)
        .map(|worker| {
            std::fs::read_to_string(f.home.path().join(format!("result-{worker}"))).unwrap()
        })
        .collect();
    outcomes.sort();
    assert_eq!(outcomes, ["committed", "consumed"]);
}
