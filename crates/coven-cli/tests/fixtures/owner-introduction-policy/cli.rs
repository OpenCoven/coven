//! Owner CLI review/sign flow with a canned reader; no TTY, daemon or installed state.
use super::*;

const AUDIENCE: [u8; 32] = [7; 32];

fn policy(lifetime_days: u32) -> IntroductionPolicy {
    IntroductionPolicy {
        eligible_introducers: vec![Uuid::from_u128(1), Uuid::from_u128(2)],
        required_approvals: 1,
        allowed_scopes: vec![DeviceScope::MemoryRead, DeviceScope::DeviceAdmin],
        maximum_grant_lifetime_seconds: lifetime_days * 86_400,
        required_assurance: RequestedAssurance::FreshBiometric,
        maximum_step_up_age_seconds: 30,
    }
}

fn setup() -> (
    tempfile::TempDir,
    OwnerPolicyStore,
    SigningKey,
    DateTime<Utc>,
) {
    let home = tempfile::tempdir().unwrap();
    let store = OwnerPolicyStore::open(home.path(), AUDIENCE).unwrap();
    let key = SigningKey::from_slice(&[11; 32]).unwrap();
    (
        home,
        store,
        key,
        DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    )
}

fn answering<'a>(
    word: &'static str,
    seen: &'a mut Vec<String>,
) -> impl FnMut(&str) -> Result<String> + 'a {
    move |prompt| {
        seen.push(prompt.to_owned());
        Ok(word.to_owned())
    }
}

#[test]
fn review_shows_every_signed_field_before_signing() {
    let (_home, store, key, now) = setup();
    let nonce: [u8; 32] = rand::random();
    let mut prompts = Vec::new();
    let head = apply_root_change(
        &store,
        PolicyChangeKind::Install,
        &key,
        policy(1),
        now,
        nonce,
        &mut answering("install", &mut prompts),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prompts.len(), 1);
    let review = &prompts[0];
    let proposal = PolicyProposal {
        revision: 1,
        previous_digest: None,
        policy: policy(1),
        nonce,
        issued_at: now,
        expires_at: now + chrono::Duration::seconds(ROOT_ACTION_SECONDS),
    };
    let action = store.install_action(&public_key(&key), &proposal).unwrap();
    for expected in [
        action.operation.as_str(),
        "device_admin",
        action.target.as_str(),
        key_fingerprint(&public_key(&key)).as_str(),
        "(none: first install)",
        "00000000-0000-0000-0000-000000000001, 00000000-0000-0000-0000-000000000002",
        "memory_read, device_admin",
        "86400 seconds",
        "fresh_biometric",
        "30 seconds",
        action.nonce.as_str(),
        "2027-01-15T08:00:00.000Z",
        "2027-01-15T08:02:00.000Z",
        action.effect_digest.as_str(),
    ] {
        assert!(
            review.contains(expected),
            "review omits {expected}:\n{review}"
        );
    }
    assert_eq!(head.digest, action.effect_digest);
    assert!(review.ends_with("(anything else cancels): "));
}

#[test]
fn anything_but_the_word_cancels_without_signing() {
    let (home, store, key, now) = setup();
    let mut prompts = Vec::new();
    for answer in ["", "y", "yes", "Install", "replace"] {
        let outcome = apply_root_change(
            &store,
            PolicyChangeKind::Install,
            &key,
            policy(1),
            now,
            rand::random(),
            &mut answering(answer, &mut prompts),
        )
        .unwrap();
        assert!(outcome.is_none());
    }
    assert!(store.current().unwrap().is_none());
    assert!(!home.path().join("mobile").join("audit.jsonl").exists());
}

#[test]
fn downgrade_is_flagged_and_still_requires_the_registered_root() {
    let (_home, store, key, now) = setup();
    let mut prompts = Vec::new();
    apply_root_change(
        &store,
        PolicyChangeKind::Install,
        &key,
        policy(1),
        now,
        rand::random(),
        &mut answering("install", &mut prompts),
    )
    .unwrap();

    // Another key, e.g. one held by a relay/account service, is refused
    // before the owner is ever asked to confirm.
    let other = SigningKey::from_slice(&[12; 32]).unwrap();
    let mut unused = Vec::new();
    let error = apply_root_change(
        &store,
        PolicyChangeKind::Replace,
        &other,
        policy(7),
        now,
        rand::random(),
        &mut answering("replace", &mut unused),
    )
    .unwrap_err();
    assert!(error.to_string().contains("not the installed owner-root"));
    assert!(unused.is_empty());

    let head = apply_root_change(
        &store,
        PolicyChangeKind::Replace,
        &key,
        policy(7),
        now,
        rand::random(),
        &mut answering("replace", &mut prompts),
    )
    .unwrap()
    .unwrap();
    assert_eq!(head.revision, 2);
    assert!(prompts[1].contains("DOWNGRADE: broadens maximum grant lifetime"));

    apply_root_change(
        &store,
        PolicyChangeKind::Replace,
        &key,
        policy(1),
        now,
        rand::random(),
        &mut answering("replace", &mut prompts),
    )
    .unwrap()
    .unwrap();
    assert!(prompts[2].contains("keeps or tightens"));
}

#[test]
fn install_and_replace_refuse_the_wrong_lifecycle_state() {
    let (_home, store, key, now) = setup();
    let mut prompts = Vec::new();
    assert!(apply_root_change(
        &store,
        PolicyChangeKind::Replace,
        &key,
        policy(1),
        now,
        rand::random(),
        &mut answering("replace", &mut prompts),
    )
    .is_err());
    apply_root_change(
        &store,
        PolicyChangeKind::Install,
        &key,
        policy(1),
        now,
        rand::random(),
        &mut answering("install", &mut prompts),
    )
    .unwrap();
    assert!(apply_root_change(
        &store,
        PolicyChangeKind::Install,
        &key,
        policy(2),
        now,
        rand::random(),
        &mut answering("install", &mut prompts),
    )
    .is_err());
    assert_eq!(prompts.len(), 1);
}

#[test]
fn root_key_paths_inside_coven_home_are_refused() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("mobile")).unwrap();
    assert!(refuse_inside_home(home.path(), &home.path().join("root.pem")).is_err());
    assert!(refuse_inside_home(home.path(), &home.path().join("mobile").join("root.pem")).is_err());
    assert!(refuse_inside_home(
        home.path(),
        &home.path().join("mobile").join("..").join("root.pem")
    )
    .is_err());
    let elsewhere = tempfile::tempdir().unwrap();
    assert!(refuse_inside_home(home.path(), &elsewhere.path().join("root.pem")).is_ok());
}
