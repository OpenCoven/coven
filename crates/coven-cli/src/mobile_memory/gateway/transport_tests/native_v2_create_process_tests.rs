//! Real v2 prepare/floor crash cuts inside a contained native gateway process.
//! Include as a CHILD of gateway::transport_tests so its private Fixture
//! constructor remains available. Requires native-fixture-phases.patch.
use super::*;
use crate::mobile_memory::config::MobileWritePhase;

fn wait_native_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "native fixture did not publish {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn real_v2_floor_temp_created_kill_restarts_without_a_truncated_authority_floor() {
    let _gateway = super::super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let fixture = Fixture::with_authorization();
    let _retained = super::super::process::RetainedOwnerGuard;
    let home = &fixture.state.coven_home;
    let relay = crate::mobile_memory::transport::test_support::Relay::start();
    storage::RelayConfig {
        listener_limit: storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(home)
    .unwrap();
    let floor = home.join("mobile").join(format!(
        ".transport-v2-{}.required",
        fixture.pairing.device_id
    ));
    assert!(
        !floor.exists(),
        "fixture must start before v2 protocol publication"
    );
    let ready = home.join("v2-create-held");
    let outcome = home.join("v2-create-first-outcome");
    let mut first_observation = None;
    let mut runtime = super::super::process::native_fixture::start_v2_prepare_fixture(
        home,
        Some(&floor),
        MobileWritePhase::TempCreated,
        &ready,
        fixture.pairing.device_id,
        &outcome,
        |handle| first_observation = Some(handle.observation()),
    )
    .unwrap()
    .unwrap();
    wait_native_file(&ready);
    let selected = std::fs::read(&ready).unwrap();
    let before_kill = std::fs::read(&floor).ok();
    let started = Instant::now();
    super::super::process::native_fixture::crash_fixture(&mut runtime).unwrap();
    let elapsed = started.elapsed();
    let observed = first_observation.unwrap();
    assert!(
        observed.exited() && observed.reaped(),
        "actual v2 writer was not observed exited/reaped"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "forced v2 cut cleanup observed {elapsed:?}"
    );
    assert_eq!(selected, b"held-temp_created-skip-0\n");
    assert!(
        !floor.exists(),
        "pre-publication kill left final v2 authority: before={before_kill:?}, after={:?}",
        std::fs::read(&floor).ok()
    );
    assert!(
        !outcome.exists(),
        "killed v2 prepare must not report a completed handover"
    );

    // New contained native process, same profile, same actual v2 prepare. The
    // orphan temporary file is not authority and does not block a fresh floor.
    let recovered_outcome = home.join("v2-create-recovered-outcome");
    let mut recovered = super::super::process::native_fixture::start_v2_prepare_fixture(
        home,
        None,
        MobileWritePhase::Written,
        &home.join("v2-unused-ready"),
        fixture.pairing.device_id,
        &recovered_outcome,
        |_| {},
    )
    .unwrap()
    .unwrap();
    wait_native_file(&recovered_outcome);
    let response: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&recovered_outcome).unwrap()).unwrap();
    let succeeded = response["ok"] == true;
    let generation = response["data"]["generation"].as_u64();
    recovered
        .shutdown_until(Instant::now() + Duration::from_secs(2))
        .unwrap();
    assert!(
        succeeded,
        "same-profile v2 prepare did not recover after pre-publication kill"
    );
    assert_eq!(generation, Some(0));
    assert_eq!(std::fs::read(&floor).unwrap(), b"coven.mobile.routing.v2\n");
}
