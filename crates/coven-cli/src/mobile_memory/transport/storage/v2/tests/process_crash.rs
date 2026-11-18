//! Actual process termination at authenticated protocol boundaries, plus explicitly
//! modeled power-loss images. Neither fixture is physical power-loss evidence.
use super::*;
use crate::mobile_memory::{
    config::{self, MobileWritePhase as Phase},
    transport::{routing::channel, Records},
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const ENTRY: &str = "mobile_memory::transport::storage::v2::tests::process_crash::authenticated_crash_process_fixture";
const PHASES: [Phase; 7] = [
    Phase::TempCreated,
    Phase::Written,
    Phase::FileSynced,
    Phase::Renamed,
    Phase::DirectorySynced,
    Phase::RetainedParentSynced,
    Phase::RetainedHomeSynced,
];
#[derive(Debug, Clone, Copy)]
pub(crate) enum Role {
    Host,
    Device,
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum Failure {
    Kill,
    Error,
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum Cut {
    Write(Phase),
    Send { ordinal: usize, after: bool },
}
impl Cut {
    fn encode(self) -> String {
        match self {
            Self::Write(p) => format!("write:{p:?}"),
            Self::Send { ordinal, after } => format!("send:{ordinal}:{after}"),
        }
    }
    fn parse(value: &str) -> Self {
        let pieces: Vec<_> = value.split(':').collect();
        match pieces.as_slice() {
            ["write", phase] => Self::Write(
                PHASES
                    .into_iter()
                    .find(|p| format!("{p:?}") == *phase)
                    .expect("known write phase"),
            ),
            ["send", n, after] => Self::Send {
                ordinal: n.parse().unwrap(),
                after: after.parse().unwrap(),
            },
            _ => panic!("invalid fixture cut"),
        }
    }
}
fn publish(control: &Path, name: &str, bytes: &[u8]) {
    let temporary = control.join(format!("{name}.tmp"));
    fs::write(&temporary, bytes).unwrap();
    fs::rename(temporary, control.join(name)).unwrap();
}
fn stop(control: &Path, cut: Cut, failure: Failure) -> Result<()> {
    publish(control, "checkpoint", cut.encode().as_bytes());
    if matches!(failure, Failure::Error) {
        anyhow::bail!("injected mobile boundary failure");
    }
    loop {
        std::thread::park();
    }
}
struct Framed {
    stream: TcpStream,
    cut: Option<Cut>,
    control: PathBuf,
    sends: usize,
    failure: Failure,
}
impl Framed {
    fn new(stream: TcpStream, cut: Option<Cut>, control: &Path, failure: Failure) -> Self {
        Self {
            stream,
            cut,
            control: control.to_owned(),
            sends: 0,
            failure,
        }
    }
    fn read_exact(&mut self, mut bytes: &mut [u8], deadline: Instant) -> Result<()> {
        while !bytes.is_empty() {
            ensure!(Instant::now() < deadline, "fixture record deadline");
            self.stream
                .set_read_timeout(Some(deadline.saturating_duration_since(Instant::now())))?;
            match self.stream.read(bytes) {
                Ok(0) => anyhow::bail!("fixture peer EOF"),
                Ok(count) => bytes = &mut bytes[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
    fn write_all(&mut self, mut bytes: &[u8], deadline: Instant) -> Result<()> {
        while !bytes.is_empty() {
            ensure!(Instant::now() < deadline, "fixture record deadline");
            self.stream
                .set_write_timeout(Some(deadline.saturating_duration_since(Instant::now())))?;
            match self.stream.write(bytes) {
                Ok(0) => anyhow::bail!("fixture zero write"),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}
impl Records for Framed {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        ensure!(
            Instant::now() < deadline && bytes.len() <= 65_535,
            "fixture record bounds"
        );
        self.sends += 1;
        if matches!(self.cut,Some(Cut::Send{ordinal,after:false}) if ordinal==self.sends) {
            stop(&self.control, self.cut.unwrap(), self.failure)?;
        }
        self.write_all(&(bytes.len() as u32).to_be_bytes(), deadline)?;
        self.write_all(bytes, deadline)?;
        if matches!(self.cut,Some(Cut::Send{ordinal,after:true}) if ordinal==self.sends) {
            stop(&self.control, self.cut.unwrap(), self.failure)?;
        }
        Ok(())
    }
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        let mut size = [0; 4];
        self.read_exact(&mut size, deadline)?;
        let size = u32::from_be_bytes(size) as usize;
        ensure!(size <= 65_535, "oversized fixture record");
        let mut bytes = vec![0; size];
        self.read_exact(&mut bytes, deadline)?;
        Ok(bytes)
    }
    fn receive_idle(&mut self) -> Result<Vec<u8>> {
        // Hang guard only; fixtures exercise challenge-bound records.
        self.receive(Instant::now() + Duration::from_secs(120))
    }
}
fn serve(
    home: &Path,
    signer: &SigningKey,
    registry: &DeviceRegistry,
    id: Uuid,
    transport: Transport,
    counter: u64,
    records: Framed,
) -> Result<()> {
    let snapshot = load(home, registry, signer.verifying_key(), id, transport)?
        .context("missing host material")?;
    let room = snapshot.rendezvous(transport, counter)?.0;
    let admission = channel::HostAdmission {
        pairing: &snapshot.material.pairing,
        private: &snapshot.material.noise_private,
        host: signer,
        context: &room,
        direct: transport == Transport::Direct,
        proof: snapshot.state(transport).proof(counter)?,
    };
    channel::accept(
        records,
        admission,
        |pairing, _| {
            check_channel(
                home,
                registry,
                pairing,
                transport,
                Some(snapshot.generation),
            )
        },
        |proof, deadline| {
            refresh_proof(
                home,
                registry,
                &snapshot.material.pairing,
                transport,
                proof,
                deadline,
            )
        },
        |proof| consume_verified(home, registry, proof),
    )
    .map(|_| ())
}
fn connect(
    home: &Path,
    host: &VerifyingKey,
    signer: &SigningKey,
    id: Uuid,
    transport: Transport,
    records: Framed,
) -> Result<()> {
    device::begin(home, host, id, signer.verifying_key(), transport)?
        .connect(records, signer)
        .map(|_| ())
}
#[test]
#[ignore = "invoked with explicit fixture homes by native process-crash parents"]
fn authenticated_crash_process_fixture() {
    let home = PathBuf::from(std::env::var_os("COVEN_CRASH_HOME").expect("explicit home"));
    let control = PathBuf::from(std::env::var_os("COVEN_CRASH_CONTROL").unwrap());
    let id = Uuid::parse_str(&std::env::var("COVEN_CRASH_DEVICE").unwrap()).unwrap();
    let transport = match std::env::var("COVEN_CRASH_TRANSPORT").unwrap().as_str() {
        "direct" => Transport::Direct,
        "relay" => Transport::Relay,
        _ => panic!("transport"),
    };
    let cut = std::env::var("COVEN_CRASH_CUT")
        .ok()
        .map(|c| Cut::parse(&c));
    let role = std::env::var("COVEN_CRASH_ROLE").unwrap();
    let failure = match std::env::var("COVEN_CRASH_FAILURE").unwrap().as_str() {
        "kill" => Failure::Kill,
        "error" => Failure::Error,
        _ => panic!("failure mode"),
    };
    let target = home.join("mobile").join(if role == "host" {
        format!("transport-{id}.json")
    } else {
        format!("device-transport-{id}.json")
    });
    if let Some(Cut::Write(expected)) = cut {
        let control = control.clone();
        config::set_write_observer(Box::new(move |path, phase| {
            if path == target && phase == expected {
                return stop(&control, Cut::Write(phase), failure);
            }
            Ok(())
        }));
    }
    if role == "host" {
        let identity = crate::mobile_memory::identity::load_existing_host_identity(&home).unwrap();
        let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
        let registry = DeviceRegistry::load(&home).unwrap();
        let counter = std::env::var("COVEN_CRASH_COUNTER")
            .unwrap()
            .parse()
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        publish(
            &control,
            "bound",
            listener.local_addr().unwrap().to_string().as_bytes(),
        );
        let (stream, _) = listener.accept().unwrap();
        serve(
            &home,
            &signer,
            &registry,
            id,
            transport,
            counter,
            Framed::new(stream, cut, &control, failure),
        )
        .unwrap();
    } else {
        let host = VerifyingKey::from_sec1_bytes(
            &URL_SAFE_NO_PAD
                .decode(std::env::var("COVEN_CRASH_HOST_PUBLIC").unwrap())
                .unwrap(),
        )
        .unwrap();
        let signer = SigningKey::from_slice(&[2; 32]).unwrap();
        let stream = TcpStream::connect(std::env::var("COVEN_CRASH_ADDRESS").unwrap()).unwrap();
        connect(
            &home,
            &host,
            &signer,
            id,
            transport,
            Framed::new(stream, cut, &control, failure),
        )
        .unwrap();
    }
}
type PipeReader = std::thread::JoinHandle<Vec<u8>>;

struct Owned {
    child: Option<Child>,
    readers: Option<(PipeReader, PipeReader)>,
}
impl Owned {
    fn new(mut child: Child) -> Self {
        fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes).unwrap();
                bytes
            })
        }
        let stdout = drain(child.stdout.take().unwrap());
        let stderr = drain(child.stderr.take().unwrap());
        Self {
            child: Some(child),
            readers: Some((stdout, stderr)),
        }
    }
    fn finish(&mut self) -> std::process::Output {
        let status = self.child.as_mut().unwrap().wait().unwrap();
        self.child.take();
        let (stdout, stderr) = self.readers.take().unwrap();
        std::process::Output {
            status,
            stdout: stdout.join().unwrap(),
            stderr: stderr.join().unwrap(),
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some((stdout, stderr)) = self.readers.take() {
            let _ = stdout.join();
            let _ = stderr.join();
        }
    }
}
/// One observation of a file the child publishes by rename before it may
/// exit. The exit is sampled first: a child that publishes and exits between
/// the two samples has then already published, so the file check that follows
/// an observed exit is decisive (#1741).
fn poll_file<S>(path: &Path, exited: impl FnOnce() -> Option<S>) -> Result<Option<Vec<u8>>, S> {
    let status = exited();
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => status.map_or(Ok(None), Err),
        Err(e) => panic!("reading {}: {e}", path.display()),
    }
}
fn wait_file(child: &mut Owned, control: &Path, name: &str) -> Vec<u8> {
    let started = Instant::now();
    loop {
        let observed = poll_file(&control.join(name), || {
            child.child.as_mut().unwrap().try_wait().unwrap()
        });
        match observed {
            Ok(Some(bytes)) => return bytes,
            Ok(None) => {}
            Err(status) => {
                let output = child.finish();
                panic!(
                    "child exited {status} before {name}: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        // Explicit readiness, with early exit detection. This is only a hang
        // guard, not a short budget selecting a valid persistence outcome.
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "child {name} unavailable after {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn a_file_published_just_before_the_child_exits_is_not_an_early_exit() {
    let control = tempfile::tempdir().unwrap();
    let path = control.path().join("checkpoint");
    // Still running, nothing published.
    assert_eq!(poll_file(&path, || None::<i32>), Ok(None));
    // Exited without publishing: a real early exit.
    assert_eq!(poll_file(&path, || Some(101)), Err(101));
    // The child publishes and exits while the parent is sampling the exit,
    // which is the latest moment the two can interleave.
    let observed = poll_file(&path, || {
        publish(control.path(), "checkpoint", b"cut");
        Some(101)
    });
    assert_eq!(observed, Ok(Some(b"cut".to_vec())));
}
pub(crate) fn round(
    f: &Fixture,
    device_home: &Path,
    role: Role,
    transport: Transport,
    cut: Option<Cut>,
    failure: Failure,
) -> Result<(), String> {
    let control = tempfile::tempdir().unwrap();
    let counter = device::begin(
        device_home,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        transport,
    )
    .unwrap()
    .state()
    .next;
    let listener = matches!(role, Role::Device).then(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    });
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", ENTRY, "--ignored", "--nocapture"])
        .env(
            "COVEN_CRASH_HOME",
            if matches!(role, Role::Host) {
                f.home.path()
            } else {
                device_home
            },
        )
        .env("COVEN_CRASH_CONTROL", control.path())
        .env("COVEN_CRASH_DEVICE", f.id.to_string())
        .env(
            "COVEN_CRASH_TRANSPORT",
            if transport == Transport::Direct {
                "direct"
            } else {
                "relay"
            },
        )
        .env(
            "COVEN_CRASH_ROLE",
            if matches!(role, Role::Host) {
                "host"
            } else {
                "device"
            },
        )
        .env(
            "COVEN_CRASH_FAILURE",
            if matches!(failure, Failure::Kill) {
                "kill"
            } else {
                "error"
            },
        )
        .env("COVEN_CRASH_COUNTER", counter.to_string())
        .env(
            "COVEN_CRASH_HOST_PUBLIC",
            URL_SAFE_NO_PAD.encode(f.host.verifying_key().to_sec1_point(false).as_bytes()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cut) = cut {
        command.env("COVEN_CRASH_CUT", cut.encode());
    } else {
        command.env_remove("COVEN_CRASH_CUT");
    }
    if let Some(listener) = &listener {
        command.env(
            "COVEN_CRASH_ADDRESS",
            listener.local_addr().unwrap().to_string(),
        );
    }
    let mut child = Owned::new(command.spawn().unwrap());
    let stream = if matches!(role, Role::Host) {
        Some(
            TcpStream::connect(
                String::from_utf8(wait_file(&mut child, control.path(), "bound")).unwrap(),
            )
            .unwrap(),
        )
    } else {
        None
    };
    let cancelled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        struct Cancel<'a>(&'a AtomicBool);
        impl Drop for Cancel<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        // Drops before the scope joins on panic, unblocking an accept whose
        // child exited before connecting. No silent fixture wedge.
        let _cancel = Cancel(&cancelled);
        let peer = scope.spawn(|| -> Result<()> {
            if let Some(stream) = stream {
                connect(
                    device_home,
                    f.host.verifying_key(),
                    &f.device,
                    f.id,
                    transport,
                    Framed::new(stream, None, control.path(), Failure::Kill),
                )
            } else {
                let started = Instant::now();
                let stream = loop {
                    ensure!(
                        !cancelled.load(Ordering::Acquire),
                        "fixture child ended before connect"
                    );
                    match listener.as_ref().unwrap().accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            ensure!(
                                started.elapsed() < Duration::from_secs(120),
                                "fixture accept unavailable after {:?}",
                                started.elapsed()
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                // macOS inherits O_NONBLOCK from the polling listener.
                stream.set_nonblocking(false)?;
                serve(
                    f.home.path(),
                    &f.host,
                    &f.registry,
                    f.id,
                    transport,
                    counter,
                    Framed::new(stream, None, control.path(), Failure::Kill),
                )
            }
        });
        if let Some(cut) = cut {
            assert_eq!(
                wait_file(&mut child, control.path(), "checkpoint"),
                cut.encode().as_bytes(),
                "wrong cut point"
            );
            if matches!(failure, Failure::Kill) {
                child.child.as_mut().unwrap().kill().unwrap();
            }
        }
        // Completion is also bounded: an incorrectly parked error fixture
        // cannot wedge the suite after having published its readiness event.
        let started = Instant::now();
        while child.child.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "fixture child did not exit after {:?}",
                started.elapsed()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child.finish();
        if cut.is_some() {
            assert!(!output.status.success(), "fault child returned success");
            if matches!(failure, Failure::Kill) {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    assert_eq!(output.status.signal(), Some(libc::SIGKILL));
                }
            } else {
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("injected mobile boundary failure")
                        || String::from_utf8_lossy(&output.stdout)
                            .contains("injected mobile boundary failure"),
                    "unexpected fixture error: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        } else {
            assert!(
                output.status.success(),
                "normal child failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        cancelled.store(true, Ordering::Release);
        peer.join().unwrap().map_err(|e| format!("{e:#}"))
    })
}
fn paired() -> (Fixture, tempfile::TempDir) {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let handover = f.prepare();
    let ack = device::adopt_direct(
        home.path(),
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    (f, home)
}
fn counters(f: &Fixture, home: &Path, transport: Transport) -> (u64, u64) {
    (
        f.snapshot().unwrap().state(transport).floor,
        device::begin(
            home,
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            transport,
        )
        .unwrap()
        .state()
        .next,
    )
}
#[test]
fn authenticated_process_kills_at_each_counter_write_boundary_recover_legal_images() {
    for transport in [Transport::Direct, Transport::Relay] {
        for role in [Role::Host, Role::Device] {
            for failure in [Failure::Kill, Failure::Error] {
                for phase in PHASES {
                    let (f, home) = paired();
                    let host_path = f
                        .home
                        .path()
                        .join(format!("mobile/transport-{}.json", f.id));
                    let device_path = home
                        .path()
                        .join(format!("mobile/device-transport-{}.json", f.id));
                    let before_host = fs::read(&host_path).unwrap();
                    let before_device = fs::read(&device_path).unwrap();
                    let result = round(
                        &f,
                        home.path(),
                        role,
                        transport,
                        Some(Cut::Write(phase)),
                        failure,
                    );
                    assert!(
                        result.is_err(),
                        "{transport:?} {role:?} {phase:?}: peer completed before cut"
                    );
                    let renamed = matches!(
                        phase,
                        Phase::Renamed
                            | Phase::DirectorySynced
                            | Phase::RetainedParentSynced
                            | Phase::RetainedHomeSynced
                    );
                    assert_eq!(
                        counters(&f, home.path(), transport),
                        match role {
                            Role::Host => (u64::from(renamed), 1),
                            Role::Device => (0, u64::from(renamed)),
                        }
                    );
                    let other = if transport == Transport::Direct {
                        Transport::Relay
                    } else {
                        Transport::Direct
                    };
                    assert_eq!(counters(&f, home.path(), other), (0, 0));
                    let after_host = fs::read(&host_path).unwrap();
                    let after_device = fs::read(&device_path).unwrap();
                    let uncertain_namespace =
                        phase == Phase::Renamed || (!cfg!(unix) && phase == Phase::DirectorySynced);
                    for retain_new in if uncertain_namespace {
                        vec![false, true]
                    } else {
                        vec![renamed]
                    } {
                        // Initialize an explicitly modeled post-power-loss image. Before
                        // rename only the old name is possible; before namespace durability
                        // either name is possible; after a checked barrier require the new.
                        let h = if matches!(role, Role::Host) && !retain_new {
                            &before_host
                        } else {
                            &after_host
                        };
                        let d = if matches!(role, Role::Device) && !retain_new {
                            &before_device
                        } else {
                            &after_device
                        };
                        atomic_replace_private(&host_path, h).unwrap();
                        atomic_replace_private(&device_path, d).unwrap();
                        let (_, next) = counters(&f, home.path(), transport);
                        round(&f, home.path(), role, transport, None, Failure::Kill).unwrap();
                        assert_eq!(counters(&f, home.path(), transport), (next + 1, next + 1));
                        println!("write-cut {failure:?} {transport:?} {role:?} {phase:?} modeled_new={retain_new} recovered={}",next+1);
                    }
                }
            }
        }
    }
}
#[test]
fn authenticated_process_kills_before_and_after_each_handshake_send_preserve_counters() {
    for transport in [Transport::Direct, Transport::Relay] {
        for role in [Role::Host, Role::Device] {
            for failure in [Failure::Kill, Failure::Error] {
                for ordinal in 1..=4 {
                    for after in [false, true] {
                        let (f, home) = paired();
                        let result = round(
                            &f,
                            home.path(),
                            role,
                            transport,
                            Some(Cut::Send { ordinal, after }),
                            failure,
                        );
                        let (c, d) = counters(&f, home.path(), transport);
                        let other = if transport == Transport::Direct {
                            Transport::Relay
                        } else {
                            Transport::Direct
                        };
                        assert_eq!(counters(&f, home.path(), other), (0, 0));
                        match (role, ordinal, after) {
                            (_, 1 | 2, _) | (Role::Host, 3, false) => assert_eq!((c, d), (0, 0)),
                            (Role::Device, 3, false) => assert_eq!((c, d), (0, 1)),
                            (Role::Host, 3, true) => assert!(c == 0 && d <= 1),
                            (Role::Device, 3, true) => assert!(d == 1 && c <= 1),
                            (_, 4, _) => assert_eq!((c, d), (1, 1)),
                            _ => unreachable!(),
                        }
                        if ordinal < 4 || !after {
                            assert!(
                                result.is_err(),
                                "peer completed at {role:?} send {ordinal} after={after}"
                            );
                        }
                        round(&f, home.path(), role, transport, None, Failure::Kill).unwrap();
                        assert_eq!(counters(&f, home.path(), transport), (d + 1, d + 1));
                        println!("send-cut {failure:?} {transport:?} {role:?} ordinal={ordinal} after={after} observed_c={c} observed_d={d} peer_ok={}",result.is_ok());
                    }
                }
            }
        }
    }
}
