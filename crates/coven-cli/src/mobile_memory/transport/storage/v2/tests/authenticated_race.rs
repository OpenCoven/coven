//! Authenticated competing processes over independent real TLS relay endpoints.
use super::*;
use crate::mobile_memory::transport::{
    cancellation::Cancellation, routing::channel, test_support::Relay, websocket,
};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const ENTRY: &str =
    "mobile_memory::transport::storage::v2::tests::authenticated_race::authenticated_race_child";
fn wait(path: &Path, deadline: Instant) {
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "race barrier unavailable: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "native child invoked by authenticated_endpoint_process_races"]
fn authenticated_race_child() {
    let home = std::path::PathBuf::from(std::env::var_os("COVEN_RACE_HOME").unwrap());
    let control = std::path::PathBuf::from(std::env::var_os("COVEN_RACE_CONTROL").unwrap());
    let number = std::env::var("COVEN_RACE_NUMBER").unwrap();
    let counter: u64 = std::env::var("COVEN_RACE_COUNTER")
        .unwrap()
        .parse()
        .unwrap();
    let config: websocket::EndpointConfig =
        serde_json::from_str(&std::env::var("COVEN_RACE_ENDPOINT").unwrap()).unwrap();
    let endpoint = config.parse().unwrap();
    let identity = crate::mobile_memory::identity::load_existing_host_identity(&home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let registry = DeviceRegistry::load(&home).unwrap();
    let snapshot = load(
        &home,
        &registry,
        host.verifying_key(),
        Uuid::from_u128(5),
        Transport::Relay,
    )
    .unwrap()
    .unwrap();
    let (room, credential) = snapshot.rendezvous(Transport::Relay, counter).unwrap();
    let cancellation = Cancellation::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    let address = endpoint.resolve(&cancellation, deadline).unwrap()[0];
    let mut records = websocket::connect_cancellable(
        &endpoint,
        address,
        websocket::Role::Host,
        &room,
        &credential,
        deadline,
        &cancellation,
    )
    .unwrap();
    records.fixture_registration_barrier().unwrap();
    fs::write(control.join(format!("joined-{number}")), b"joined").unwrap();
    let result = channel::accept(
        records,
        channel::HostAdmission {
            pairing: &snapshot.material.pairing,
            private: &snapshot.material.noise_private,
            host: &host,
            context: &room,
            direct: false,
            proof: snapshot.state(Transport::Relay).proof(counter).unwrap(),
        },
        |pairing, _| {
            check_channel(
                &home,
                &registry,
                pairing,
                Transport::Relay,
                Some(snapshot.generation),
            )
        },
        |proof, deadline| {
            refresh_proof(
                &home,
                &registry,
                &snapshot.material.pairing,
                Transport::Relay,
                proof,
                deadline,
            )
        },
        |proof| {
            fs::write(control.join(format!("verified-{number}")), b"verified")?;
            wait(&control.join(format!("release-{number}")), proof.deadline());
            consume_verified(&home, &registry, proof)
        },
    );
    let status = match result {
        Ok(_) => "committed",
        Err(error) => {
            assert_eq!(
                error.to_string(),
                "routing room outside authenticated window",
                "unexpected race error: {error:#}"
            );
            "stale"
        }
    };
    fs::write(control.join(format!("result-{number}")), status).unwrap();
}
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn authenticated_endpoint_process_races() {
    for (counters, ordered) in [([0_u64, 0], false), ([0, 1], false), ([2, 1], true)] {
        let f = Fixture::new();
        let handover = f.prepare();
        f.commit(&handover);
        let control = tempfile::tempdir().unwrap();
        let relays = [Relay::start_literal(), Relay::start_literal()];
        let phones = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        for (number, phone) in phones.iter().enumerate() {
            device::adopt_direct(
                phone.path(),
                handover.clone(),
                f.host.verifying_key(),
                f.id,
                f.device.verifying_key(),
            )
            .unwrap();
            // Explicit cloned-device snapshot at an already admissible counter.
            let path = phone
                .path()
                .join(format!("mobile/device-transport-{}.json", f.id));
            let mut image: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            image["states"][1]["next"] = serde_json::json!(counters[number]);
            atomic_replace_private(&path, &serde_json::to_vec(&image).unwrap()).unwrap();
        }
        let mut children = Vec::new();
        for number in 0..2 {
            children.push(Owned(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", ENTRY, "--ignored", "--nocapture"])
                    .env("COVEN_RACE_HOME", f.home.path())
                    .env("COVEN_RACE_CONTROL", control.path())
                    .env("COVEN_RACE_NUMBER", number.to_string())
                    .env("COVEN_RACE_COUNTER", counters[number].to_string())
                    .env(
                        "COVEN_RACE_ENDPOINT",
                        serde_json::to_string(&relays[number].configuration()).unwrap(),
                    )
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap(),
            ));
        }
        std::thread::scope(|scope| {
            let mut clients = Vec::new();
            for number in 0..2 {
                let f = &f;
                let phone = &phones[number];
                let relay = &relays[number];
                let control = control.path();
                clients.push(scope.spawn(move || {
                    wait(
                        &control.join(format!("joined-{number}")),
                        Instant::now() + Duration::from_secs(30),
                    );
                    let mut attempt = device::begin(
                        phone.path(),
                        f.host.verifying_key(),
                        f.id,
                        f.device.verifying_key(),
                        Transport::Relay,
                    )
                    .unwrap();
                    let (room, credential) = attempt.rendezvous().unwrap();
                    let records = relay
                        .connect(websocket::Role::Client, &room, &credential)
                        .unwrap();
                    let channel = attempt.connect(records, &f.device);
                    // Keep the session open until the host reports, as a
                    // device does. The relay sends its peer-disconnect close
                    // ahead of queued records, so dropping the channel right
                    // after device-finished-v2 can discard that advisory
                    // completion; completion loss has its own cases.
                    wait(
                        &control.join(format!("result-{number}")),
                        Instant::now() + Duration::from_secs(30),
                    );
                    channel.is_ok()
                }));
            }
            for number in 0..2 {
                wait(
                    &control.path().join(format!("verified-{number}")),
                    Instant::now() + Duration::from_secs(30),
                );
            }
            fs::write(control.path().join("release-0"), b"go").unwrap();
            if ordered {
                wait(
                    &control.path().join("result-0"),
                    Instant::now() + Duration::from_secs(30),
                );
            }
            fs::write(control.path().join("release-1"), b"go").unwrap();
            let successes = clients
                .into_iter()
                .filter_map(|c| c.join().unwrap().then_some(()))
                .count();
            if counters[0] == counters[1] || ordered {
                assert_eq!(successes, 1);
            } else {
                assert!((1..=2).contains(&successes));
            }
        });
        for (number, child) in children.iter_mut().enumerate() {
            wait(
                &control.path().join(format!("result-{number}")),
                Instant::now() + Duration::from_secs(30),
            );
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                // Lifecycle hang guard; challenge deadline remains 30 seconds.
                assert!(Instant::now() < deadline, "race child did not exit");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if ordered {
            assert_eq!(fs::read(control.path().join("result-1")).unwrap(), b"stale");
        }
        assert_eq!(
            f.snapshot().unwrap().floors,
            [0, counters.into_iter().max().unwrap() + 1]
        );
    }
}
