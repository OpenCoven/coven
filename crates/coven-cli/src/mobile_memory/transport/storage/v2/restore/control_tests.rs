//! Process faults at trusted direct-control persistence boundaries. The caller's
//! signed HTTPS ingress is tested separately; this fixture invokes Rust owners.
use super::*;
use crate::mobile_memory::identity;
use crate::mobile_memory::{
    config,
    transport::storage::v2::{
        self as v2, device,
        tests::{
            process_crash::{self, Cut, Failure, Role},
            Fixture,
        },
    },
};
use p256::pkcs8::DecodePrivateKey;
use std::path::PathBuf;
use std::{
    collections::BTreeMap,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const ENTRY: &str =
    "mobile_memory::transport::storage::v2::restore::control_tests::direct_control_process_fixture";
#[derive(Debug, Clone, Copy)]
enum Operation {
    Prepare,
    Adopt,
    Acknowledge,
    Reacknowledge,
    HostResync(Transport),
    DeviceResync(Transport),
    HostRecover(bool),
    DeviceRecover(bool),
    Export,
}
impl Operation {
    fn code(self) -> String {
        match self {
            Self::Prepare => "prepare".into(),
            Self::Adopt => "adopt".into(),
            Self::Acknowledge => "ack".into(),
            Self::Reacknowledge => "reack".into(),
            Self::HostResync(t) => format!(
                "host-resync-{}",
                if t == Transport::Direct {
                    "direct"
                } else {
                    "relay"
                }
            ),
            Self::DeviceResync(t) => format!(
                "device-resync-{}",
                if t == Transport::Direct {
                    "direct"
                } else {
                    "relay"
                }
            ),
            Self::HostRecover(missing) => format!("host-recover-{missing}"),
            Self::DeviceRecover(missing) => format!("device-recover-{missing}"),
            Self::Export => "export".into(),
        }
    }
    fn parse(code: &str) -> Self {
        for op in OPERATIONS {
            if op.code() == code {
                return op;
            }
        }
        panic!("unknown fixture operation")
    }
}
const OPERATIONS: [Operation; 13] = [
    Operation::Prepare,
    Operation::Adopt,
    Operation::Acknowledge,
    Operation::Reacknowledge,
    Operation::HostResync(Transport::Direct),
    Operation::HostResync(Transport::Relay),
    Operation::DeviceResync(Transport::Direct),
    Operation::DeviceResync(Transport::Relay),
    Operation::HostRecover(true),
    Operation::HostRecover(false),
    Operation::DeviceRecover(true),
    Operation::DeviceRecover(false),
    Operation::Export,
];
#[test]
#[ignore = "native child entry invoked by control matrix"]
fn direct_control_process_fixture() {
    let home = PathBuf::from(std::env::var_os("CONTROL_TEST_HOME").unwrap());
    let phone = PathBuf::from(std::env::var_os("CONTROL_TEST_DEVICE").unwrap());
    let input = PathBuf::from(std::env::var_os("CONTROL_TEST_INPUT").unwrap());
    let output = PathBuf::from(std::env::var_os("CONTROL_TEST_OUTPUT").unwrap());
    let trace = PathBuf::from(std::env::var_os("CONTROL_TEST_TRACE").unwrap());
    let operation = Operation::parse(&std::env::var("CONTROL_TEST_OPERATION").unwrap());
    let cut: usize = std::env::var("CONTROL_TEST_CUT").unwrap().parse().unwrap();
    let kill = std::env::var("CONTROL_TEST_KILL").unwrap() == "true";
    let identity = identity::load_existing_host_identity(&home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let signer = SigningKey::from_slice(&[2; 32]).unwrap();
    let registry = DeviceRegistry::load(&home).unwrap();
    let id = Uuid::from_u128(5);
    let mut ordinal = 0;
    let mut lines = String::new();
    config::set_write_observer(Box::new(move |path, phase| {
        ordinal += 1;
        lines.push_str(&format!(
            "{ordinal} {} {phase:?}\n",
            path.file_name().unwrap().to_string_lossy()
        ));
        let temporary = trace.with_extension("tmp");
        fs::write(&temporary, &lines)?;
        fs::rename(temporary, &trace)?;
        if ordinal == cut {
            if kill {
                loop {
                    std::thread::park();
                }
            }
            anyhow::bail!("injected direct-control write failure");
        }
        Ok(())
    }));
    let read_handover =
        || -> Handover { serde_json::from_slice(&read_private(&input).unwrap()).unwrap() };
    let result: Result<()> = match operation {
        Operation::Prepare => prepare_direct(&home, &registry, &host, id).map(|_| ()),
        Operation::Adopt => device::adopt_direct(
            &phone,
            read_handover(),
            host.verifying_key(),
            id,
            signer.verifying_key(),
        )
        .map(|_| ()),
        Operation::Acknowledge | Operation::Reacknowledge => acknowledge_direct(
            &home,
            &registry,
            &host,
            id,
            &read_handover().acknowledgment(),
        ),
        Operation::HostResync(transport) => {
            resync_direct(&home, &registry, &host, id, transport, 0, 3).map(|_| ())
        }
        Operation::DeviceResync(transport) => device::resync_direct(
            &phone,
            host.verifying_key(),
            id,
            signer.verifying_key(),
            transport,
            HostState {
                generation: 0,
                floor: 3,
            },
        ),
        Operation::HostRecover(_) => {
            // The enrolled phone supplies its intact authenticated generation,
            // never a guess derived from the corrupt/missing host file.
            let anchor = device::begin(
                &phone,
                host.verifying_key(),
                id,
                signer.verifying_key(),
                Transport::Direct,
            )
            .unwrap()
            .state()
            .generation;
            recover_direct(&home, &registry, &host, id, Some(anchor)).map(|_| ())
        }
        Operation::DeviceRecover(_) => device::adopt_recovery_direct(
            &phone,
            read_handover(),
            host.verifying_key(),
            id,
            signer.verifying_key(),
        )
        .map(|_| ()),
        Operation::Export => export::export_authority(&home, &output).map(|_| ()),
    };
    result.unwrap();
}
fn optional_bytes(path: &Path) -> Option<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("cannot read fixture file {}: {error}", path.display()),
    }
}
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Case {
    f: Fixture,
    phone: tempfile::TempDir,
    control: tempfile::TempDir,
    input: PathBuf,
    output: PathBuf,
    initial: Option<Handover>,
}
impl Case {
    fn new(op: Operation) -> Self {
        let f = Fixture::new();
        let phone = tempfile::tempdir().unwrap();
        let initial = f.prepare();
        let ack = device::adopt_direct(
            phone.path(),
            initial.clone(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        let control = tempfile::tempdir().unwrap();
        crate::daemon::ensure_private_coven_home(control.path()).unwrap();
        let input = control.path().join("handover.json");
        let output = control.path().join("authority.json");
        let handover = match op {
            Operation::Adopt
            | Operation::Acknowledge
            | Operation::Reacknowledge
            | Operation::DeviceRecover(_) => Some(f.prepare()),
            _ => None,
        };
        if let Some(handover) = &handover {
            atomic_replace_private(&input, &serde_json::to_vec(handover).unwrap()).unwrap();
        }
        if matches!(op, Operation::Acknowledge | Operation::Reacknowledge) {
            let ack = device::adopt_direct(
                phone.path(),
                handover.clone().unwrap(),
                f.host.verifying_key(),
                f.id,
                f.device.verifying_key(),
            )
            .unwrap();
            if matches!(op, Operation::Reacknowledge) {
                acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
                for transport in [Transport::Direct, Transport::Relay] {
                    let state =
                        resync_direct(f.home.path(), &f.registry, &f.host, f.id, transport, 1, 2)
                            .unwrap();
                    device::resync_direct(
                        phone.path(),
                        f.host.verifying_key(),
                        f.id,
                        f.device.verifying_key(),
                        transport,
                        state,
                    )
                    .unwrap();
                }
            }
        }
        match op {
            Operation::HostResync(transport) => {
                for _ in 0..3 {
                    let result = process_crash::round(
                        &f,
                        phone.path(),
                        Role::Device,
                        transport,
                        Some(Cut::Send {
                            ordinal: 3,
                            after: false,
                        }),
                        Failure::Error,
                    );
                    assert!(result.is_err());
                }
                assert_eq!(f.snapshot().unwrap().state(transport).floor, 0);
                assert_eq!(
                    device::begin(
                        phone.path(),
                        f.host.verifying_key(),
                        f.id,
                        f.device.verifying_key(),
                        transport
                    )
                    .unwrap()
                    .state()
                    .next,
                    3
                );
            }
            Operation::DeviceResync(transport) => {
                resync_direct(f.home.path(), &f.registry, &f.host, f.id, transport, 0, 3).unwrap();
            }
            Operation::HostRecover(missing) => {
                let path = f
                    .home
                    .path()
                    .join(format!("mobile/transport-{}.json", f.id));
                if missing {
                    fs::remove_file(path).unwrap();
                } else {
                    atomic_replace_private(&path, b"{bad-json}").unwrap();
                }
            }
            Operation::DeviceRecover(missing) => {
                let path = phone
                    .path()
                    .join(format!("mobile/device-transport-{}.json", f.id));
                if missing {
                    fs::remove_file(path).unwrap();
                } else {
                    atomic_replace_private(&path, b"{bad-json}").unwrap();
                }
            }
            _ => {}
        }
        Self {
            f,
            phone,
            control,
            input,
            output,
            initial: Some(initial),
        }
    }
    fn state_files(&self) -> [PathBuf; 2] {
        [
            self.f
                .home
                .path()
                .join(format!("mobile/transport-{}.json", self.f.id)),
            self.phone
                .path()
                .join(format!("mobile/device-transport-{}.json", self.f.id)),
        ]
    }
    fn authority_files(&self) -> BTreeMap<String, Option<Vec<u8>>> {
        [
            "host-key.pem",
            "host-identity.json",
            "devices.json",
            "authorization-keys.json",
            "introduction-policy.json",
            "gateway.json",
            "relay.json",
            "transport-restore.json",
            ".transport-restore.required",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([
            format!("transport-{}.json", self.f.id),
            format!(".transport-v2-{}.required", self.f.id),
        ])
        .map(|name| {
            let bytes = optional_bytes(&self.f.home.path().join("mobile").join(&name));
            (name, bytes)
        })
        .collect()
    }
    fn reset(&self, files: &[Option<Vec<u8>>; 2]) {
        // Independent fixture cases: both owned endpoints are offline and the
        // previous owned child is reaped before restoring this baseline.
        for (path, bytes) in self.state_files().iter().zip(files) {
            if let Some(bytes) = bytes {
                atomic_replace_private(path, bytes).unwrap();
            } else if path.try_exists().unwrap() {
                fs::remove_file(path).unwrap();
            }
        }
        if self.output.try_exists().unwrap() {
            fs::remove_file(&self.output).unwrap();
        }
    }
}
fn run(c: &Case, operation: Operation, cut: usize, kill: bool) -> Vec<String> {
    let ipc = tempfile::tempdir().unwrap();
    let trace = ipc.path().join("trace");
    let stdout = ipc.path().join("stdout");
    let stderr = ipc.path().join("stderr");
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", ENTRY, "--ignored", "--nocapture"])
            .env("CONTROL_TEST_HOME", c.f.home.path())
            .env("CONTROL_TEST_DEVICE", c.phone.path())
            .env("CONTROL_TEST_INPUT", &c.input)
            .env("CONTROL_TEST_OUTPUT", &c.output)
            .env("CONTROL_TEST_TRACE", &trace)
            .env("CONTROL_TEST_OPERATION", operation.code())
            .env("CONTROL_TEST_CUT", cut.to_string())
            .env("CONTROL_TEST_KILL", kill.to_string())
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    let mut killed = false;
    let status = loop {
        let count = fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .count();
        if cut != 0 && kill && !killed && count == cut {
            child.0.kill().unwrap();
            killed = true;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        // Hang guard only: a parked or unexpectedly exited child cannot hide.
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "control child stalled after {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let out = fs::read_to_string(stdout).unwrap();
    let err = fs::read_to_string(stderr).unwrap();
    if cut == 0 {
        assert!(status.success(), "{out} {err}");
    } else if kill {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(libc::SIGKILL), "{out} {err}");
        }
        assert!(!status.success());
    } else {
        assert!(!status.success());
        assert!(
            err.contains("injected direct-control write failure"),
            "{out} {err}"
        );
    }
    let lines: Vec<_> = fs::read_to_string(trace)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    if cut != 0 {
        assert_eq!(lines.len(), cut, "child missed selected boundary");
    }
    lines
}
fn complete(c: &Case, operation: Operation) {
    let f = &c.f;
    match operation {
        Operation::HostResync(transport) | Operation::DeviceResync(transport) => {
            let state =
                resync_direct(f.home.path(), &f.registry, &f.host, f.id, transport, 0, 3).unwrap();
            assert_eq!(state.floor, 3);
            device::resync_direct(
                c.phone.path(),
                f.host.verifying_key(),
                f.id,
                f.device.verifying_key(),
                transport,
                state,
            )
            .unwrap();
            for t in [Transport::Direct, Transport::Relay] {
                let expected = if t == transport { 3 } else { 0 };
                assert_eq!(f.snapshot().unwrap().state(t).floor, expected);
                let state = device::begin(
                    c.phone.path(),
                    f.host.verifying_key(),
                    f.id,
                    f.device.verifying_key(),
                    t,
                )
                .unwrap()
                .state();
                assert_eq!(
                    (state.generation, state.next, state.floor),
                    (0, expected, expected)
                );
            }
        }
        Operation::Export => {
            let authority = c.authority_files();
            if c.output.try_exists().unwrap() {
                assert!(export::export_authority(f.home.path(), &c.output).is_err());
            }
            let retry = c.control.path().join("retry-authority.json");
            export::export_authority(f.home.path(), &retry).unwrap();
            export::read_authenticated(&retry, f.host.verifying_key()).unwrap();
            assert!(
                c.authority_files() == authority,
                "export retry mutated authority metadata"
            );
            fs::remove_file(retry).unwrap();
        }
        _ => {
            let retained_seed = v2::read(&c.state_files()[0], f.id)
                .ok()
                .flatten()
                .and_then(|s| {
                    s.pending
                        .as_ref()
                        .or(s.committed.as_ref())
                        .filter(|g| g.generation == 1)
                        .map(|g| g.material.routing_seed)
                });
            let handover = if c.input.exists() {
                serde_json::from_slice::<Handover>(&read_private(&c.input).unwrap()).unwrap()
            } else if matches!(operation, Operation::HostRecover(_)) {
                recover_direct(f.home.path(), &f.registry, &f.host, f.id, Some(0)).unwrap()
            } else {
                prepare_direct(f.home.path(), &f.registry, &f.host, f.id).unwrap()
            };
            assert_eq!(handover.generation, 1);
            assert!(
                handover.routing_seed != c.initial.as_ref().unwrap().routing_seed,
                "new generation reused old seed"
            );
            if let Some(seed) = retained_seed {
                assert!(
                    seed == handover.routing_seed,
                    "retry replaced retained pending seed"
                );
            }
            let ack = if matches!(operation, Operation::DeviceRecover(_)) {
                device::adopt_recovery_direct(
                    c.phone.path(),
                    handover.clone(),
                    f.host.verifying_key(),
                    f.id,
                    f.device.verifying_key(),
                )
                .unwrap()
            } else {
                device::adopt_direct(
                    c.phone.path(),
                    handover.clone(),
                    f.host.verifying_key(),
                    f.id,
                    f.device.verifying_key(),
                )
                .unwrap()
            };
            acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
            let snapshot = f.snapshot().unwrap();
            assert_eq!(snapshot.generation, 1);
            assert!(*snapshot.material.routing_seed == handover.routing_seed);
            for transport in [Transport::Direct, Transport::Relay] {
                let expected = if matches!(operation, Operation::Reacknowledge) {
                    2
                } else {
                    0
                };
                assert_eq!(snapshot.state(transport).floor, expected);
                let state = device::begin(
                    c.phone.path(),
                    f.host.verifying_key(),
                    f.id,
                    f.device.verifying_key(),
                    transport,
                )
                .unwrap()
                .state();
                assert_eq!(
                    (state.generation, state.next, state.floor),
                    (1, expected, expected)
                );
            }
        }
    }
    for transport in [Transport::Direct, Transport::Relay] {
        process_crash::round(
            f,
            c.phone.path(),
            Role::Host,
            transport,
            None,
            Failure::Kill,
        )
        .unwrap();
    }
}
#[test]
fn direct_control_process_faults_preserve_generations_seeds_and_resync_floors() {
    let mut counts = BTreeMap::new();
    for operation in OPERATIONS {
        let c = Case::new(operation);
        let baseline = c.state_files().map(|p| optional_bytes(&p));
        let authority = c.authority_files();
        let trace = run(&c, operation, 0, false);
        assert!(
            !trace.is_empty(),
            "control operation had no observed writes"
        );
        if matches!(operation, Operation::Prepare) {
            assert!(
                trace
                    .iter()
                    .any(|s| s.contains(".transport-v2-") && s.ends_with(" FileSynced")),
                "retained protocol file sync not observed: {trace:?}"
            );
        }
        if matches!(operation, Operation::Export) {
            assert_eq!(
                trace
                    .iter()
                    .filter(|s| s.ends_with("authority.json FileSynced"))
                    .count(),
                2,
                "export generic and retained file barriers must both be observed"
            );
            assert!(trace
                .iter()
                .any(|s| s.ends_with("authority.json RetainedHomeSynced")));
        }
        c.reset(&baseline);
        for kill in [true, false] {
            for cut in 1..=trace.len() {
                c.reset(&baseline);
                let observed = run(&c, operation, cut, kill);
                assert_eq!(
                    observed,
                    trace[..cut],
                    "{} write order changed",
                    operation.code()
                );
                if matches!(operation, Operation::Export) {
                    assert!(
                        c.authority_files() == authority,
                        "export mutated endpoint authority"
                    );
                }
                complete(&c, operation);
                *counts.entry(operation.code()).or_insert(0) += 1;
                println!(
                    "control-process operation={} kill={kill} cut={cut} {} recovered",
                    operation.code(),
                    trace[cut - 1]
                );
            }
        }
    }
    println!("control-process counts={counts:?}");
}

#[test]
fn first_provisioning_process_faults_never_guess_an_unavailable_generation() {
    first_provisioning(false);
}
#[test]
fn v1_upgrade_process_faults_never_guess_or_reopen_legacy_routing() {
    first_provisioning(true);
}
fn first_provisioning(legacy: bool) {
    let f = Fixture::new();
    let legacy_seed = if legacy {
        Some(
            *crate::mobile_memory::transport::storage::load_or_provision(
                f.home.path(),
                &f.registry,
                &f.host,
                f.id,
            )
            .unwrap()
            .routing_seed,
        )
    } else {
        None
    };
    let phone = tempfile::tempdir().unwrap();
    let control = tempfile::tempdir().unwrap();
    crate::daemon::ensure_private_coven_home(control.path()).unwrap();
    let input = control.path().join("unused.json");
    let output = control.path().join("unused-export.json");
    let c = Case {
        f,
        phone,
        control,
        input,
        output,
        initial: None,
    };
    let snapshot = tempfile::tempdir().unwrap();
    let mobile = ensure_private_mobile_dir(snapshot.path()).unwrap();
    let baseline = c.state_files().map(|p| optional_bytes(&p));
    for name in [
        "host-key.pem",
        "host-identity.json",
        "devices.json",
        "authorization-keys.json",
        "introduction-policy.json",
        "gateway.json",
        "relay.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([format!("transport-{}.json", c.f.id)])
    {
        if let Some(bytes) = optional_bytes(&c.f.home.path().join("mobile").join(&name)) {
            atomic_replace_private(&mobile.join(&name), &bytes).unwrap();
        }
    }
    let authority = c.control.path().join("pre-provision-authority.json");
    export::export_authority(c.f.home.path(), &authority).unwrap();
    let pin = identity::load_existing_host_identity(c.f.home.path())
        .unwrap()
        .public_key_fingerprint;
    let grant_id =
        c.f.registry
            .authorization_record(c.f.id)
            .unwrap()
            .unwrap()
            .grant
            .id;
    let marker =
        c.f.home
            .path()
            .join("mobile")
            .join(format!(".transport-v2-{}.required", c.f.id));
    let trace = run(&c, Operation::Prepare, 0, false);
    assert!(
        trace
            .iter()
            .any(|s| s.contains(".transport-v2-") && s.ends_with(" TempCreated")),
        "initial canonical creation is not observed: {trace:?}"
    );
    assert!(
        trace
            .iter()
            .any(|s| s.contains(".transport-v2-") && s.ends_with(" DirectorySynced")),
        "initial namespace durability is not observed: {trace:?}"
    );
    let mut restored = 0;
    let mut ordinary = 0;
    for kill in [true, false] {
        for cut in 1..=trace.len() {
            c.reset(&baseline);
            if marker.try_exists().unwrap() {
                fs::remove_file(&marker).unwrap();
            }
            let observed = run(&c, Operation::Prepare, cut, kill);
            assert_eq!(observed, trace[..cut]);
            let stored = v2::read(&c.state_files()[0], c.f.id).unwrap();
            if stored.is_none() && marker.try_exists().unwrap() {
                assert!(prepare_direct(c.f.home.path(), &c.f.registry, &c.f.host, c.f.id).is_err());
                assert!(
                    recover_direct(c.f.home.path(), &c.f.registry, &c.f.host, c.f.id, None)
                        .is_err()
                );
                assert!(crate::mobile_memory::transport::storage::load_or_provision(
                    c.f.home.path(),
                    &c.f.registry,
                    &c.f.host,
                    c.f.id
                )
                .is_err());
                assert!(
                    optional_bytes(&c.state_files()[0]) == baseline[0],
                    "refusal manufactured generation state"
                );
                // With no retained anchor, use the explicit operator ceremony
                // and a separate empty destination. No input generation is guessed.
                let destination = tempfile::tempdir().unwrap();
                let state = install::install_selected_snapshot_fixture(
                    destination.path(),
                    snapshot.path(),
                    &authority,
                    pin,
                    Uuid::new_v4(),
                )
                .unwrap();
                assert_eq!(state.pairings[0].generation, 1);
                assert_eq!(state.pairings[0].revocation_epoch, 1);
                let registry = DeviceRegistry::load(destination.path()).unwrap();
                assert_eq!(
                    registry
                        .authorization_record(c.f.id)
                        .unwrap()
                        .unwrap()
                        .grant
                        .id,
                    grant_id
                );
                let recovered = Fixture {
                    home: destination,
                    registry,
                    host: c.f.host.clone(),
                    device: c.f.device.clone(),
                    id: c.f.id,
                };
                let handover = recovered.prepare();
                if let Some(seed) = legacy_seed {
                    assert!(seed != handover.routing_seed, "restore reused v1 seed");
                }
                finish_initial(&recovered, c.phone.path(), handover, 1);
                assert!(optional_bytes(&c.state_files()[0]) == baseline[0]);
                restored += 1;
            } else {
                let pending = stored
                    .as_ref()
                    .and_then(|s| s.pending.as_ref())
                    .map(|g| (g.material.routing_seed, g.material.noise_public));
                let handover = c.f.prepare();
                assert_eq!(handover.generation, 0);
                if let Some(seed) = legacy_seed {
                    assert!(seed != handover.routing_seed, "upgrade reused v1 seed");
                }
                if let Some((seed, noise)) = pending {
                    assert!(
                        seed == handover.routing_seed && noise == handover.noise_public,
                        "retry replaced retained initial material"
                    );
                }
                finish_initial(&c.f, c.phone.path(), handover, 0);
                ordinary += 1;
            }
            println!(
                "first-provision legacy={legacy} kill={kill} cut={cut} {} recovered",
                trace[cut - 1]
            );
        }
    }
    println!("first-provision legacy={legacy} ordinary={ordinary} explicit-restore={restored}");
    assert!(ordinary > 0 && restored > 0);
}
fn finish_initial(f: &Fixture, phone: &Path, handover: Handover, generation: u64) {
    let ack = device::adopt_direct(
        phone,
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    assert_eq!(f.snapshot().unwrap().generation, generation);
    for transport in [Transport::Direct, Transport::Relay] {
        process_crash::round(f, phone, Role::Host, transport, None, Failure::Kill).unwrap();
    }
}
