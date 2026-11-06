use super::super::tests::Fixture;
use super::*;

#[test]
fn device_adoption_persists_before_ack_and_repeated_handover_never_resets_consumed_state() {
    let f = Fixture::new();
    let device_home = tempfile::tempdir().unwrap();
    let handover = f.prepare();
    let ack = adopt_direct(
        device_home.path(),
        handover.clone(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    let mut attempt = begin(
        device_home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay,
    )
    .unwrap();
    let next = attempt
        .state()
        .consume(
            HostState {
                generation: 0,
                floor: 0,
            }
            .proof(0)
            .unwrap(),
        )
        .unwrap();
    attempt.persist_verified(next).unwrap();
    drop(attempt);
    adopt_direct(
        device_home.path(),
        handover.clone(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    let attempt = begin(
        device_home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay,
    )
    .unwrap();
    assert_eq!(
        attempt.state(),
        DeviceState {
            generation: 0,
            next: 1,
            floor: 0
        }
    );
    drop(attempt);
    let mut different = handover.clone();
    different.routing_seed[0] ^= 1;
    assert!(adopt_direct(
        device_home.path(),
        different,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key()
    )
    .is_err());
    let next = f.prepare();
    f.commit(&next);
    adopt_direct(
        device_home.path(),
        next,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    assert!(adopt_direct(
        device_home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key()
    )
    .is_err());
    assert_eq!(
        begin(
            device_home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay
        )
        .unwrap()
        .state(),
        DeviceState {
            generation: 1,
            next: 0,
            floor: 0
        }
    );
}

#[test]
fn device_attempt_holds_transport_lock_until_completion_or_drop() {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let handover = f.prepare();
    adopt_direct(
        home.path(),
        handover.clone(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    let attempt = begin(
        home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay,
    )
    .unwrap();
    assert!(begin(
        home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Direct
    )
    .is_err());
    assert!(adopt_direct(
        home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key()
    )
    .is_err());
    drop(attempt);
    assert!(begin(
        home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay
    )
    .is_ok());
}

#[test]
fn device_resync_is_durable_and_refuses_generation_or_floor_rollback() {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let handover = f.prepare();
    adopt_direct(
        home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    resync_direct(
        home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay,
        HostState {
            generation: 0,
            floor: 4,
        },
    )
    .unwrap();
    assert_eq!(
        begin(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay
        )
        .unwrap()
        .state(),
        DeviceState {
            generation: 0,
            next: 4,
            floor: 4
        }
    );
    for state in [
        HostState {
            generation: 0,
            floor: 3,
        },
        HostState {
            generation: 1,
            floor: 8,
        },
    ] {
        assert!(resync_direct(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay,
            state
        )
        .is_err());
    }
    assert_eq!(
        begin(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay
        )
        .unwrap()
        .state(),
        DeviceState {
            generation: 0,
            next: 4,
            floor: 4
        }
    );
}

struct Pipe {
    sender: std::sync::mpsc::Sender<Vec<u8>>,
    receiver: std::sync::mpsc::Receiver<Vec<u8>>,
}
impl Records for Pipe {
    fn send(&mut self, bytes: &[u8], deadline: std::time::Instant) -> Result<()> {
        ensure!(std::time::Instant::now() < deadline, "deadline expired");
        self.sender.send(bytes.to_vec())?;
        Ok(())
    }
    fn receive(&mut self, deadline: std::time::Instant) -> Result<Vec<u8>> {
        Ok(self
            .receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))?)
    }
    fn receive_idle(&mut self) -> Result<Vec<u8>> {
        self.receive(std::time::Instant::now() + std::time::Duration::from_secs(30))
    }
}
#[test]
fn real_v2_channel_commits_both_device_and_host_files_before_returning_success() {
    real_channel_consumption(Transport::Relay);
}
#[test]
fn verified_direct_proof_consumes_only_direct_state() {
    real_channel_consumption(Transport::Direct);
}
fn real_channel_consumption(transport: Transport) {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let handover = f.prepare();
    let ack = adopt_direct(
        home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    let snapshot = f.snapshot().unwrap();
    let signing_device = f.device.clone();
    let host_key = *f.host.verifying_key();
    let device_key = *f.device.verifying_key();
    let id = f.id;
    let mut attempt = begin(home.path(), &host_key, id, &device_key, transport).unwrap();
    let (room, _) = attempt.rendezvous().unwrap();
    assert_eq!(room, snapshot.rendezvous(transport, 0).unwrap().0);
    let (a_tx, a_rx) = std::sync::mpsc::channel();
    let (b_tx, b_rx) = std::sync::mpsc::channel();
    let host = std::thread::spawn(move || {
        let server = Pipe {
            sender: b_tx,
            receiver: a_rx,
        };
        let admission = channel::HostAdmission {
            pairing: &snapshot.material.pairing,
            private: &snapshot.material.noise_private,
            host: &f.host,
            context: &room,
            direct: transport == Transport::Direct,
            proof: snapshot.state(transport).proof(0).unwrap(),
        };
        let channel = channel::accept(
            server,
            admission,
            |pairing, direct| authority::check_current(&f.registry, pairing, direct, Utc::now()),
            |proof, deadline| {
                refresh_proof(
                    f.home.path(),
                    &f.registry,
                    &snapshot.material.pairing,
                    transport,
                    proof,
                    deadline,
                )
            },
            |proof| consume_verified(f.home.path(), &f.registry, proof),
        )
        .unwrap();
        assert!(channel.is_open());
        f
    });
    let client = attempt
        .connect(
            Pipe {
                sender: a_tx,
                receiver: b_rx,
            },
            &signing_device,
        )
        .unwrap();
    assert!(client.is_open());
    drop(attempt);
    let f = host.join().unwrap();
    assert_eq!(
        f.snapshot().unwrap().floors,
        if transport == Transport::Direct {
            [1, 0]
        } else {
            [0, 1]
        }
    );
    assert_eq!(
        begin(home.path(), &host_key, id, &device_key, transport)
            .unwrap()
            .state(),
        DeviceState {
            generation: 0,
            next: 1,
            floor: 0
        }
    );
}

fn exchange_with_directory_failure(
    f: &Fixture,
    home: &Path,
    fail_device: bool,
    fail_host: bool,
) -> (Result<(), String>, Result<(), String>) {
    let snapshot = f.snapshot().unwrap();
    let mut attempt = begin(
        home,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay,
    )
    .unwrap();
    let counter = attempt.state().next;
    let (room, _) = attempt.rendezvous().unwrap();
    let (a_tx, a_rx) = std::sync::mpsc::channel();
    let (b_tx, b_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let host = scope.spawn(|| {
            let admission = channel::HostAdmission {
                pairing: &snapshot.material.pairing,
                private: &snapshot.material.noise_private,
                host: &f.host,
                context: &room,
                direct: false,
                proof: snapshot.state(Transport::Relay).proof(counter).unwrap(),
            };
            channel::accept(
                Pipe {
                    sender: b_tx,
                    receiver: a_rx,
                },
                admission,
                |pairing, direct| {
                    authority::check_current(&f.registry, pairing, direct, Utc::now())
                },
                |proof, deadline| {
                    refresh_proof(
                        f.home.path(),
                        &f.registry,
                        &snapshot.material.pairing,
                        Transport::Relay,
                        proof,
                        deadline,
                    )
                },
                |proof| {
                    if fail_host {
                        crate::durable_fs::fail_next_directory_sync();
                    }
                    consume_verified(f.home.path(), &f.registry, proof)
                },
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        });
        if fail_device {
            crate::durable_fs::fail_next_directory_sync();
        }
        let client = attempt
            .connect(
                Pipe {
                    sender: a_tx,
                    receiver: b_rx,
                },
                &f.device,
            )
            .map(|_| ())
            .map_err(|e| e.to_string());
        drop(attempt);
        (client, host.join().unwrap())
    })
}
#[test]
fn device_and_host_directory_failures_refuse_success_and_recover_from_the_actual_retained_images() {
    for fail_device in [true, false] {
        let f = Fixture::new();
        let home = tempfile::tempdir().unwrap();
        let handover = f.prepare();
        let ack = adopt_direct(
            home.path(),
            handover,
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        let (client, host) =
            exchange_with_directory_failure(&f, home.path(), fail_device, !fail_device);
        assert!(client.is_err());
        assert!(host.is_err());
        let injected = if fail_device {
            client.unwrap_err()
        } else {
            host.unwrap_err()
        };
        assert!(
            injected.contains("injected directory sync failure"),
            "{injected}"
        );
        assert_eq!(
            begin(
                home.path(),
                f.host.verifying_key(),
                f.id,
                f.device.verifying_key(),
                Transport::Relay
            )
            .unwrap()
            .state(),
            DeviceState {
                generation: 0,
                next: 1,
                floor: 0
            }
        );
        assert_eq!(
            f.snapshot().unwrap().floors,
            [0, if fail_device { 0 } else { 1 }]
        );
        let (client, host) = exchange_with_directory_failure(&f, home.path(), false, false);
        client.unwrap();
        host.unwrap();
        assert_eq!(f.snapshot().unwrap().floors, [0, 2]);
        assert_eq!(
            begin(
                home.path(),
                f.host.verifying_key(),
                f.id,
                f.device.verifying_key(),
                Transport::Relay
            )
            .unwrap()
            .state(),
            DeviceState {
                generation: 0,
                next: 2,
                floor: if fail_device { 0 } else { 1 }
            }
        );
    }
}

#[test]
fn corrupt_device_state_requires_explicit_authenticated_recovery_handover() {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let initial = f.prepare();
    let ack = adopt_direct(
        home.path(),
        initial,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    let path = path(home.path(), f.id);
    atomic_replace_private(&path, b"{corrupt device state").unwrap();
    assert!(begin(
        home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay
    )
    .is_err());
    let handover = f.prepare();
    assert_eq!(handover.generation, 1);
    assert!(adopt_direct(
        home.path(),
        handover.clone(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key()
    )
    .is_err());
    let ack = adopt_recovery_direct(
        home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    assert_eq!(
        begin(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay
        )
        .unwrap()
        .state(),
        DeviceState {
            generation: 1,
            next: 0,
            floor: 0
        }
    );
}

#[test]
fn explicit_device_recovery_can_replace_a_corrupt_public_key_without_trusting_it() {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let first = f.prepare();
    let ack = adopt_direct(
        home.path(),
        first,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    let path = path(home.path(), f.id);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["handover"]["hostKey"] = serde_json::json!([1]);
    atomic_replace_private(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    let handover = f.prepare();
    let ack = adopt_recovery_direct(
        home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    assert_eq!(
        begin(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay
        )
        .unwrap()
        .state()
        .generation,
        1
    );
}

#[test]
fn endpoint_only_handover_retries_update_metadata_without_resetting_counters() {
    for disable in [false, true] {
        let f = Fixture::new();
        let home = tempfile::tempdir().unwrap();
        let mut first = f.prepare();
        let certificate =
            rcgen::generate_simple_self_signed(vec!["relay.example.test".into()]).unwrap();
        first.relay_endpoints = vec![EndpointConfig {
            url: "wss://relay.example.test/ws".into(),
            ca_certificates: vec![URL_SAFE_NO_PAD.encode(certificate.cert.der())],
        }];
        let ack = adopt_direct(
            home.path(),
            first.clone(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        let (client, host) = exchange_with_directory_failure(&f, home.path(), false, false);
        client.unwrap();
        host.unwrap();
        let mut updated = first;
        if disable {
            updated.relay_endpoints.clear();
        } else {
            let replacement =
                rcgen::generate_simple_self_signed(vec!["relay.example.test".into()]).unwrap();
            updated.relay_endpoints[0].ca_certificates =
                vec![URL_SAFE_NO_PAD.encode(replacement.cert.der())];
        }
        let expected = updated.relay_endpoints.clone();
        let ack = adopt_direct(
            home.path(),
            updated,
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        let attempt = begin(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay,
        )
        .unwrap();
        assert_eq!(
            attempt.state(),
            DeviceState {
                generation: 0,
                next: 1,
                floor: 0
            }
        );
        assert!(attempt.record.handover.relay_endpoints == expected);
        assert_eq!(f.snapshot().unwrap().floors, [0, 1]);
    }
}

#[cfg(unix)]
#[test]
fn ended_device_attempt_releases_lock_despite_an_unrelated_fork_duplicate() {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let handover = f.prepare();
    let ack = adopt_direct(
        home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    let attempt = begin(
        home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay,
    )
    .unwrap();
    // Only async-signal-safe libc calls run in the forked child. It models the
    // pre-exec interval of an unrelated concurrent process spawn, not a device.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
    if pid == 0 {
        loop {
            unsafe {
                libc::pause();
            }
        }
    }
    struct Fork(libc::pid_t);
    impl Drop for Fork {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
                while libc::waitpid(self.0, std::ptr::null_mut(), 0) < 0 {
                    if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                        break;
                    }
                }
            }
        }
    }
    let _child = Fork(pid);
    assert!(
        begin(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            Transport::Relay
        )
        .is_err(),
        "live owner lost exclusivity"
    );
    drop(attempt);
    let next = begin(
        home.path(),
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        Transport::Relay,
    );
    assert!(
        next.is_ok(),
        "ended attempt retained by unrelated fork: {}",
        next.err().map(|e| format!("{e:#}")).unwrap_or_default()
    );
}
