//! Native installer interruption. Process death is not a power-loss simulation.
#[cfg(unix)]
mod mixed_sources;
#[cfg(unix)]
mod retained_images;
use super::*;
use crate::mobile_memory::config;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const ENTRY: &str = "mobile_memory::transport::storage::v2::restore::install::snapshot_install_tests::process::restore_process_fixture";

#[test]
#[ignore = "native child entry invoked by parent"]
fn restore_process_fixture() {
    let home = std::path::PathBuf::from(std::env::var_os("RESTORE_TEST_HOME").unwrap());
    let snapshot = std::path::PathBuf::from(std::env::var_os("RESTORE_TEST_SNAPSHOT").unwrap());
    let authority = std::path::PathBuf::from(std::env::var_os("RESTORE_TEST_AUTHORITY").unwrap());
    let trace = std::path::PathBuf::from(std::env::var_os("RESTORE_TEST_TRACE").unwrap());
    let cut: usize = std::env::var("RESTORE_TEST_CUT").unwrap().parse().unwrap();
    let kill = std::env::var("RESTORE_TEST_KILL").unwrap() == "true";
    let operation = std::env::var("RESTORE_TEST_OPERATION")
        .unwrap()
        .parse()
        .unwrap();
    let pin: [u8; 32] = URL_SAFE_NO_PAD
        .decode(std::env::var("RESTORE_TEST_PIN").unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let mut lines = String::new();
    let mut ordinal = 0;
    config::set_write_observer(Box::new(move |path, phase| {
        ordinal += 1;
        // Atomic IPC publication prevents a partial trace line from selecting a cut.
        // This deliberately makes no persistence claim about authority files.
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
            anyhow::bail!("injected restore process boundary error");
        }
        Ok(())
    }));
    install_from_snapshot(&home, &snapshot, &authority, pin, operation).unwrap();
}
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run(c: &Case, home: &Path, operation: Uuid, cut: usize, kill: bool) -> Vec<String> {
    run_sources(
        c.snapshot.path(),
        &c.authority,
        c.pin,
        home,
        operation,
        cut,
        kill,
    )
}
fn run_sources(
    snapshot: &Path,
    authority: &Path,
    pin: [u8; 32],
    home: &Path,
    operation: Uuid,
    cut: usize,
    kill: bool,
) -> Vec<String> {
    let control = tempfile::tempdir().unwrap();
    let trace = control.path().join("trace");
    let stdout = control.path().join("stdout");
    let stderr = control.path().join("stderr");
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", ENTRY, "--ignored", "--nocapture"])
            .env("RESTORE_TEST_HOME", home)
            .env("RESTORE_TEST_SNAPSHOT", snapshot)
            .env("RESTORE_TEST_AUTHORITY", authority)
            .env("RESTORE_TEST_TRACE", &trace)
            .env("RESTORE_TEST_OPERATION", operation.to_string())
            .env("RESTORE_TEST_PIN", URL_SAFE_NO_PAD.encode(pin))
            .env("RESTORE_TEST_CUT", cut.to_string())
            .env("RESTORE_TEST_KILL", kill.to_string())
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    let mut killed = false;
    let status = loop {
        let lines = fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .count();
        if cut != 0 && kill && !killed && lines == cut {
            child.0.kill().unwrap();
            killed = true;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        // A hang guard, not a sequencing or persistence deadline.
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "restore child stalled after {:?}",
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
            err.contains("injected restore process boundary error"),
            "{out} {err}"
        );
    }
    let lines: Vec<String> = fs::read_to_string(trace)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    if cut != 0 {
        assert_eq!(lines.len(), cut, "child did not stop at exact boundary");
    }
    lines
}
#[test]
fn snapshot_installer_process_interruptions_keep_fence_and_fixed_targets() {
    exercise(false, false);
}
#[test]
fn snapshot_installer_process_interruptions_preserve_post_snapshot_revocation() {
    exercise(true, false);
}
#[test]
fn snapshot_installer_process_interruptions_preserve_policy_keys_and_multiple_pairings() {
    exercise(false, true);
}
fn rich_case() -> Case {
    use crate::mobile_memory::{
        assurance::{AssuranceClass, AuthorizationKeyRegistry, NewAuthorizationKey},
        grant::DeviceGrant,
    };
    let mut c = case();
    // Keep the older policy in the selected snapshot, then tighten the live head.
    export::tests::install_policy(&c.f);
    let name = "introduction-policy.json";
    atomic_replace_private(
        &c.snapshot.path().join("mobile").join(name),
        &fs::read(c.f.home.path().join("mobile").join(name)).unwrap(),
    )
    .unwrap();
    export::tests::install_policy(&c.f);
    let second = SigningKey::from_slice(&[3; 32]).unwrap();
    let public = URL_SAFE_NO_PAD.encode(second.verifying_key().to_sec1_point(false).as_bytes());
    let mut record =
        c.f.registry
            .authorization_record(c.f.id)
            .unwrap()
            .unwrap()
            .device;
    record.id = Uuid::from_u128(6);
    record.public_key_x963 = public.clone();
    record.display_name = "Synthetic second restore phone".into();
    let grant =
        DeviceGrant::for_device(record.id, &public, record.scopes.clone(), Utc::now()).unwrap();
    c.f.registry.register_with_grant(record, grant).unwrap();
    let second_id = Uuid::from_u128(6);
    let handover = prepare_direct(c.f.home.path(), &c.f.registry, &c.f.host, second_id).unwrap();
    acknowledge_direct(
        c.f.home.path(),
        &c.f.registry,
        &c.f.host,
        second_id,
        &handover.acknowledgment(),
    )
    .unwrap();
    let keys = AuthorizationKeyRegistry::load(c.f.home.path()).unwrap();
    let authorization = SigningKey::from_slice(&[9; 32]).unwrap();
    keys.enroll_initial(
        second_id,
        &public,
        NewAuthorizationKey {
            public_key_x963: URL_SAFE_NO_PAD.encode(
                authorization
                    .verifying_key()
                    .to_sec1_point(false)
                    .as_bytes(),
            ),
            assurance_class: AssuranceClass::BiometricOnly,
            enrolled_at: Utc::now(),
        },
    )
    .unwrap();
    keys.revoke(second_id, Utc::now()).unwrap();
    c.f.registry.revoke(second_id, Utc::now()).unwrap();
    c.authority = c.authority.with_file_name("rich-latest.json");
    export::export_authority(c.f.home.path(), &c.authority).unwrap();
    c
}
fn check_rich_authority(c: &Case, home: &Path) {
    for name in ["introduction-policy.json", "authorization-keys.json"] {
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(c.f.home.path().join("mobile").join(name)).unwrap())
                .unwrap();
        let actual: serde_json::Value =
            serde_json::from_slice(&fs::read(home.join("mobile").join(name)).unwrap()).unwrap();
        assert_eq!(
            actual, expected,
            "restored {name} changed complete metadata"
        );
    }
    let registry = DeviceRegistry::load(home).unwrap();
    let second = Uuid::from_u128(6);
    let record = registry.authorization_record(second).unwrap().unwrap();
    assert!(record.device.revoked_at.is_some());
    assert_eq!(record.grant.revocation_epoch, 2);
    assert!(prepare_direct(home, &registry, &c.f.host, second).is_err());
    assert!(check_application(home, &registry, second).is_err());
}
fn exercise(revoked: bool, rich: bool) {
    let mut c = if rich { rich_case() } else { case() };
    if revoked {
        c.f.registry.revoke(c.f.id, Utc::now()).unwrap();
        let latest = c.authority.with_file_name("latest.json");
        export::export_authority(c.f.home.path(), &latest).unwrap();
        c.authority = latest;
    }
    let before = inventory(c.snapshot.path());
    let normal = tempfile::tempdir().unwrap();
    ensure_private_mobile_dir(normal.path()).unwrap();
    let trace = run(&c, normal.path(), Uuid::new_v4(), 0, false);
    assert!(
        trace
            .iter()
            .any(|s| s.contains("transport-restore.json RetainedParentSynced")),
        "restore journal retained barriers are not observed: {trace:?}"
    );
    if rich {
        assert!(trace
            .iter()
            .any(|s| s.contains("introduction-policy.json RetainedHomeSynced")));
        assert!(trace.iter().any(|s| s
            .contains("transport-00000000-0000-0000-0000-000000000006.json RetainedHomeSynced")));
    }
    for kill in [true, false] {
        for cut in 1..=trace.len() {
            let destination = tempfile::tempdir().unwrap();
            ensure_private_mobile_dir(destination.path()).unwrap();
            let operation = Uuid::new_v4();
            let observed = run(&c, destination.path(), operation, cut, kill);
            assert_eq!(observed, trace[..cut], "write ordering changed");
            let registry = DeviceRegistry::load(destination.path()).unwrap();
            if check_application(destination.path(), &registry, c.f.id).is_ok() {
                assert!(
                    registry.authorization_record(c.f.id).unwrap().is_none(),
                    "cut {cut} exposed pairing authority before fence"
                );
            }
            if destination.path().join("mobile/host-key.pem").exists() {
                assert!(
                    destination.path().join("mobile").join(MARKER).exists(),
                    "cut {cut} copied identity before marker"
                );
                journal_for_install(destination.path(), c.pin)
                    .unwrap()
                    .expect("snapshot key copied before valid journal");
            }
            let state = install_from_snapshot(
                destination.path(),
                c.snapshot.path(),
                &c.authority,
                c.pin,
                operation,
            )
            .unwrap();
            assert_eq!(state.installation_epoch, 1);
            assert_eq!(state.pairings.len(), if rich { 2 } else { 1 });
            if rich {
                let second = state
                    .pairings
                    .iter()
                    .find(|p| p.device_id == Uuid::from_u128(6))
                    .unwrap();
                assert_eq!((second.generation, second.revocation_epoch), (1, 2));
                check_rich_authority(&c, destination.path());
            }
            assert_eq!(state.pairings[0].generation, 1);
            assert_eq!(state.pairings[0].revocation_epoch, 1 + u64::from(revoked));
            assert!(!state.pairings[0].handover_complete);
            assert!(check_application(destination.path(), &registry, c.f.id).is_err());
            if revoked {
                assert!(prepare_direct(destination.path(), &registry, &c.f.host, c.f.id).is_err());
                assert!(registry
                    .authorization_record(c.f.id)
                    .unwrap()
                    .unwrap()
                    .device
                    .revoked_at
                    .is_some());
            } else {
                let fresh =
                    prepare_direct(destination.path(), &registry, &c.f.host, c.f.id).unwrap();
                let device_home = tempfile::tempdir().unwrap();
                let ack = super::super::super::super::device::adopt_direct(
                    device_home.path(),
                    fresh,
                    c.f.host.verifying_key(),
                    c.f.id,
                    c.f.device.verifying_key(),
                )
                .unwrap();
                acknowledge_direct(destination.path(), &registry, &c.f.host, c.f.id, &ack).unwrap();
                check_application(destination.path(), &registry, c.f.id).unwrap();
            }
            if rich {
                check_rich_authority(&c, destination.path());
            }
            assert_eq!(inventory(c.snapshot.path()), before);
            println!(
                "restore-process rich={rich} revoked={revoked} kill={kill} cut={cut} {}",
                trace[cut - 1]
            );
        }
    }
}

#[test]
fn interrupted_restore_repairs_partial_new_protocol_floor_only_under_its_fence() {
    let c = case();
    let normal = tempfile::tempdir().unwrap();
    ensure_private_mobile_dir(normal.path()).unwrap();
    let trace = run(&c, normal.path(), Uuid::new_v4(), 0, false);
    let cut = trace
        .iter()
        .position(|s| s.contains(".transport-v2-") && s.ends_with(" Written"))
        .unwrap()
        + 1;
    let destination = tempfile::tempdir().unwrap();
    ensure_private_mobile_dir(destination.path()).unwrap();
    let operation = Uuid::new_v4();
    run(&c, destination.path(), operation, cut, true);
    let marker = destination
        .path()
        .join("mobile")
        .join(format!(".transport-v2-{}.required", c.f.id));
    // Explicit retained image: create/write happened before file/namespace sync.
    // An empty retained new file is legal; SIGKILL alone did not produce it.
    atomic_replace_private(&marker, b"").unwrap();
    let registry = DeviceRegistry::load(destination.path()).unwrap();
    assert!(check_application(destination.path(), &registry, c.f.id).is_err());
    assert!(prepare_direct(destination.path(), &registry, &c.f.host, c.f.id).is_err());
    assert!(
        fs::read(&marker).unwrap().is_empty(),
        "ordinary ingress repaired marker"
    );
    assert!(install_from_snapshot(
        destination.path(),
        c.snapshot.path(),
        &c.authority,
        c.pin,
        Uuid::new_v4()
    )
    .is_err());
    assert!(
        fs::read(&marker).unwrap().is_empty(),
        "different restore operation repaired marker"
    );
    let state = install_from_snapshot(
        destination.path(),
        c.snapshot.path(),
        &c.authority,
        c.pin,
        operation,
    )
    .unwrap();
    assert_eq!(state.pairings[0].generation, 1);
    assert_eq!(fs::read(marker).unwrap(), b"coven.mobile.routing.v2\n");
    assert!(check_application(destination.path(), &registry, c.f.id).is_err());
}
