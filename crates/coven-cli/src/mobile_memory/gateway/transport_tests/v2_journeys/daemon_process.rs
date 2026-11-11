//! The journey across daemon process restarts (#1684).
//!
//! The runtime here is not built by the test: a child process runs the daemon's
//! own `serve_forever`, which starts the mobile runtime as `coven daemon` does.
//! Where the machine has a private-network address the daemon binds its gateway
//! listener on it, so signed control and direct WSS go through the gateway's own
//! accept loop. The operator's device authority stays in this process, as the
//! `coven memory mobile device` command is a separate process from the daemon.
//!
//! Limits. Unix only, like the other daemon-process fixtures. Without a
//! private-network address the daemon cannot bind its listener; the handover
//! then runs on the in-process runtime before the daemon starts and only the
//! relay profile reaches the daemon process. The child is the test executable
//! running the daemon's serve loop, not the installed `coven` binary.
use super::*;
use std::process::{Child, Command, Stdio};

const CHILD_ENV: &str = "COVEN_TEST_V2_JOURNEY_DAEMON_CHILD";
const TEST: &str = "mobile_memory::gateway::transport_tests::v2_journeys::daemon_process::integrated_v2_journey_survives_daemon_process_restarts";

/// A private-network address of this machine that delivers a local connection
/// to a listener bound on it, with a port that was free a moment ago. The
/// gateway refuses loopback. Completing `connect` is not enough: an overlay
/// interface can complete it without the listener ever accepting.
fn private_listener() -> Option<SocketAddr> {
    [
        "100.100.100.100:9",
        "10.0.0.1:9",
        "192.168.1.1:9",
        "172.16.0.1:9",
        "1.1.1.1:9",
    ]
    .into_iter()
    .find_map(|destination| {
        // Connecting a datagram socket sends nothing; it selects the source address.
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect(destination).ok()?;
        let ip = socket.local_addr().ok()?.ip();
        if !ip.is_ipv4() || !crate::mobile_memory::config::is_private_mobile_address_for_test(ip) {
            return None;
        }
        let probe = TcpListener::bind(SocketAddr::new(ip, 0)).ok()?;
        probe.set_nonblocking(true).ok()?;
        let address = probe.local_addr().ok()?;
        let _client = TcpStream::connect_timeout(&address, Duration::from_millis(500)).ok()?;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match probe.accept() {
                Ok(_) => return Some(address),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return None,
            }
        }
    })
}

/// One daemon process serving the host installation.
struct Daemon {
    child: Child,
    log: PathBuf,
}
impl Daemon {
    fn spawn(host: &Host) -> Self {
        let home = host.home.path();
        let user = home.join("isolated-user");
        std::fs::create_dir_all(&user).unwrap();
        let log = home.join(DAEMON_LOG);
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env("COVEN_HOME", home)
            .env("HOME", &user)
            .env("TMPDIR", home)
            .current_dir(home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr);
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut daemon = Self {
            child: command.spawn().unwrap(),
            log,
        };
        // The gateway listener is bound during daemon startup. A relay-only
        // daemon has no such signal; its first relay connection waits instead.
        if let Some(external) = &host.external {
            let started = Instant::now();
            while TcpStream::connect_timeout(&external.address, Duration::from_millis(250)).is_err()
            {
                assert!(
                    daemon.child.try_wait().unwrap().is_none(),
                    "the daemon exited during startup"
                );
                assert!(
                    started.elapsed() < HANG_GUARD,
                    "the daemon never bound its gateway listener"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        daemon
    }

    /// End the process with no chance to run any shutdown path.
    fn kill(mut self) {
        self.child.kill().unwrap();
        assert!(!self.child.wait().unwrap().success());
    }

    /// The operator's `coven daemon stop`, then the process's own clean exit.
    fn stop(mut self, host: &Host) {
        assert!(crate::daemon::stop_background_server(host.home.path()).unwrap());
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "the daemon did not stop cleanly: {status}"
                );
                return;
            }
            assert!(
                started.elapsed() < HANG_GUARD,
                "the daemon stayed alive after stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if std::thread::panicking() {
            let log = std::fs::read(&self.log).unwrap_or_default();
            eprintln!("daemon standard error:\n{}", String::from_utf8_lossy(&log));
        }
    }
}

/// Channels that were live when their daemon went away or their authority
/// changed: each fails at its next request.
fn assert_closed(phone: &Phone, host: &Host, live: Vec<Live>, direct_error: Option<&str>) {
    for mut live in live {
        assert!(
            live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
            "a channel answered after it should have closed"
        );
        if let Some(expected) = direct_error {
            host.assert_refusal(live.slot, expected);
        }
    }
}

#[test]
fn integrated_v2_journey_survives_daemon_process_restarts() {
    if std::env::var_os(CHILD_ENV).is_some() {
        let home = crate::coven_home_dir().unwrap();
        crate::daemon::serve_forever(&home, Utc::now().to_rfc3339(), None, &[]).unwrap();
        return;
    }
    let _guard = super::super::super::tests::TEST_GATEWAY_LOCK
        .lock()
        .unwrap();
    let listener = private_listener();
    // Keep the daemon's Unix socket path below the macOS pathname limit.
    let mut host = Host::with(
        tempfile::Builder::new()
            .prefix("coven-v2-journey-")
            .tempdir_in("/tmp")
            .unwrap(),
        listener,
    );
    let phone = host.enroll(42);
    let profiles: &[Transport] = if listener.is_some() {
        &[Transport::Direct, Transport::Relay]
    } else {
        &[Transport::Relay]
    };
    let identity_path = host.home.path().join("mobile/host-identity.json");
    let identity_before = std::fs::read(&identity_path).unwrap();
    if listener.is_none() {
        host.start();
        phone.handover(&host, Assurance::Possession);
        host.stop();
    }
    let daemon = Daemon::spawn(&host);
    if listener.is_some() {
        // Signed prepare and acknowledge through the daemon's own accept loop.
        assert!(host.routing_bytes(phone.id).is_none());
        phone.handover(&host, Assurance::Possession);
    }
    let connect = |host: &Host| -> Vec<Live> {
        profiles
            .iter()
            .map(|&transport| {
                let mut live = phone.connect(host, transport);
                read_device(&phone, &mut live);
                live
            })
            .collect()
    };
    let floors = |each: u64| {
        let mut floors = [0, 0];
        for &transport in profiles {
            floors[index(transport)] = each;
        }
        floors
    };
    let unreachable = |host: &Host| {
        let (device, routing) = (phone.device_bytes(), host.routing_bytes(phone.id));
        for &transport in profiles {
            assert!(
                phone.attempt(host, transport).1.is_err(),
                "{transport:?}: admitted with no daemon running"
            );
        }
        assert_eq!(phone.device_bytes(), device);
        assert_eq!(host.routing_bytes(phone.id), routing);
    };

    // Connect and send signed requests on each profile's first counter room.
    let first_rooms = phone.image();
    let mut live = connect(&host);
    let replayed = phone.request("GET", DEVICE, &[]);
    assert_eq!(live[0].exchange(&replayed).unwrap().status, 200);
    assert_eq!(
        live[0].exchange(&replayed).unwrap().outcome(),
        (409, "request_replayed")
    );
    assert_eq!(host.committed(phone.id), (0, floors(1)));

    // The daemon process is killed outright. Nothing is admitted while it is
    // gone, and neither side's durable state moves.
    daemon.kill();
    assert_closed(&phone, &host, live, None);
    unreachable(&host);

    // A new daemon process serves the next room from durable counters alone,
    // and the rooms consumed before the kill stay consumed.
    let daemon = Daemon::spawn(&host);
    let live = connect(&host);
    assert_eq!(host.committed(phone.id), (0, floors(2)));
    for &transport in profiles {
        first_rooms.refused(&host, transport, "direct pairing unavailable");
    }
    assert_eq!(std::fs::read(&identity_path).unwrap(), identity_before);

    // An ordinary stop and start reconnects the same way.
    daemon.stop(&host);
    assert_closed(&phone, &host, live, None);
    unreachable(&host);
    let daemon = Daemon::spawn(&host);
    let mut live = connect(&host);
    assert_eq!(host.committed(phone.id), (0, floors(3)));

    // The operator acts from this process and the daemon enforces it in its
    // own. With a gateway listener, a grant reissue closes the channels and the
    // device hands over to generation 1 through the daemon.
    if listener.is_some() {
        let superseded = phone.image();
        host.reissue(phone.id, AssuranceLevel::Possession);
        assert_closed(&phone, &host, live, Some("grant changed"));
        for &transport in profiles {
            phone.refused(&host, transport, "direct pairing unavailable");
        }
        assert_eq!(phone.handover(&host, Assurance::Possession).0, 1);
        live = connect(&host);
        assert_eq!(host.committed(phone.id), (1, floors(1)));
        for &transport in profiles {
            superseded.refused(&host, transport, "direct pairing unavailable");
        }
    }

    // Suspension keeps the grant and its epoch, so only the daemon's re-read of
    // the device's status closes these channels. After a resume the same
    // generation is served on the next room.
    let committed = host.committed(phone.id);
    host.suspend(phone.id);
    assert_closed(&phone, &host, live, Some("device unavailable"));
    for &transport in profiles {
        phone.refused(&host, transport, "direct pairing unavailable");
    }
    assert_eq!(host.committed(phone.id), committed);
    host.resume(phone.id);
    let live = connect(&host);
    assert_eq!(host.committed(phone.id).0, committed.0);

    // Revocation closes the channels and nothing is admitted afterwards.
    let routing = host.routing_bytes(phone.id);
    host.revoke(phone.id);
    assert!(host.is_revoked(phone.id));
    assert_closed(&phone, &host, live, Some("device unavailable"));
    for &transport in profiles {
        phone.refused(&host, transport, "direct pairing unavailable");
    }
    if listener.is_some() {
        assert_eq!(
            phone
                .control(
                    &host,
                    serde_json::json!({"version":2,"operation":"prepare"}),
                    Assurance::Possession
                )
                .outcome(),
            (403, "device_revoked")
        );
    }
    assert_eq!(host.routing_bytes(phone.id), routing);
    daemon.stop(&host);
    // Written past libtest's capture, so a hosted log records which profiles
    // this run reached: without a private-network address only the relay
    // profile reaches the daemon process.
    let _ = writeln!(
        std::io::stderr(),
        "v2-journey daemon-process profiles={profiles:?} gateway-listener={} verified",
        listener.is_some()
    );
}
