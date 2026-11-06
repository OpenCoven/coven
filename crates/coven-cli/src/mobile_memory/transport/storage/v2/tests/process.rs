//! Real independent-process coverage of the production file consumption lock.
use super::*;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const CHILD: &str =
    "mobile_memory::transport::storage::v2::tests::process::routing_consumer_process_fixture";

#[test]
#[ignore = "fixture entry point launched by independent_process_consumers_serialize_same_and_different_rooms"]
fn routing_consumer_process_fixture() {
    let home = PathBuf::from(std::env::var_os("COVEN_ROUTING_FIXTURE_HOME").expect("fixture home"));
    let number = std::env::var("COVEN_ROUTING_FIXTURE_NUMBER").unwrap();
    let counter: u64 = std::env::var("COVEN_ROUTING_FIXTURE_COUNTER")
        .unwrap()
        .parse()
        .unwrap();
    let identity = crate::mobile_memory::identity::load_existing_host_identity(&home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let registry = DeviceRegistry::load(&home).unwrap();
    let id = Uuid::from_u128(5);
    let snapshot = load(&home, &registry, host.verifying_key(), id, Transport::Relay)
        .unwrap()
        .unwrap();
    let proof = snapshot.state(Transport::Relay).proof(counter).unwrap();
    fs::write(home.join(format!("ready-{number}")), b"ready").unwrap();
    let started = Instant::now();
    while !home.join("start-consumers").exists() {
        // Hang guard only. Parent publishes the explicit readiness barrier;
        // scheduler speed never decides which consumption result is valid.
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "parent barrier unavailable after {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    match consume(
        &home,
        &registry,
        &snapshot.material.pairing,
        Transport::Relay,
        proof,
    ) {
        Ok(()) => {}
        Err(error) => {
            assert_eq!(
                error.to_string(),
                "routing room outside authenticated window",
                "unexpected refusal: {error:#}"
            );
            std::process::exit(2);
        }
    }
}

struct OwnedChild(Option<Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
#[test]
fn independent_process_consumers_serialize_same_and_different_rooms() {
    for counters in [[0_u64, 0], [0, 1], [2, 1]] {
        let f = Fixture::new();
        let handover = f.prepare();
        f.commit(&handover);
        let mut children = Vec::new();
        for (number, counter) in counters.into_iter().enumerate() {
            children.push(OwnedChild(Some(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", CHILD, "--ignored", "--nocapture"])
                    .env("COVEN_ROUTING_FIXTURE_HOME", f.home.path())
                    .env("COVEN_ROUTING_FIXTURE_NUMBER", number.to_string())
                    .env("COVEN_ROUTING_FIXTURE_COUNTER", counter.to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            )));
        }
        let started = Instant::now();
        while !(0..2).all(|number| f.home.path().join(format!("ready-{number}")).exists()) {
            for child in &mut children {
                assert!(
                    child.0.as_mut().unwrap().try_wait().unwrap().is_none(),
                    "consumer exited before readiness for {counters:?}"
                );
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "consumers not ready after {:?} for {counters:?}",
                started.elapsed()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        fs::write(f.home.path().join("start-consumers"), b"start").unwrap();
        let mut successes = 0;
        for mut child in children {
            let output = child.0.take().unwrap().wait_with_output().unwrap();
            match output.status.code() {
                Some(0) => successes += 1,
                Some(2) => {}
                other => panic!(
                    "unexpected native consumer status {other:?}: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            }
        }
        if counters[0] == counters[1] {
            assert_eq!(successes, 1)
        } else {
            assert!((1..=2).contains(&successes));
        }
        assert_eq!(
            f.snapshot().unwrap().floors,
            [0, counters.into_iter().max().unwrap() + 1]
        );
    }
}
