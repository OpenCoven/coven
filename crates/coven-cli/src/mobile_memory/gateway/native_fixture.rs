//! Real native gateway child adapter for controlled storage and lifecycle cuts.
//! Checkpoint holds model admitted work without claiming a physical disk stall.
use super::super::super::config::MobileWritePhase;
use super::*;
use std::path::{Path, PathBuf};

const FIXTURE: &str = "mobile_memory::gateway::process::native_fixture::worker_fixture";
const HOME_ENV: &str = "COVEN_TEST_MOBILE_WORKER_HOME";
const HOLD_ENV: &str = "COVEN_TEST_MOBILE_WORKER_HOLD_WRITTEN";
const HOLD_PHASE_ENV: &str = "COVEN_TEST_MOBILE_WORKER_HOLD_PHASE";
const HOLD_SKIP_ENV: &str = "COVEN_TEST_MOBILE_WORKER_HOLD_SKIP";
const FAIL_REGISTRY_ENV: &str = "COVEN_TEST_MOBILE_WORKER_FAIL_REGISTRY_COMMIT";
const WAIT_ENV: &str = "COVEN_TEST_MOBILE_WORKER_OBSERVE_LOCK";
const INITIAL_ENROLLMENT_ENV: &str = "COVEN_TEST_MOBILE_WORKER_PENDING_ENROLLMENT";
const READY_ENV: &str = "COVEN_TEST_MOBILE_WORKER_READY";
const V2_DEVICE_ENV: &str = "COVEN_TEST_MOBILE_WORKER_PREPARE_V2_DEVICE";
const V2_OUTCOME_ENV: &str = "COVEN_TEST_MOBILE_WORKER_PREPARE_V2_OUTCOME";

#[derive(Default)]
struct FixtureExtras<'a> {
    prepared: Option<&'a Path>,
    cut: Option<EnrollmentCut<'a>>,
    v2_prepare: Option<(uuid::Uuid, &'a Path)>,
    shutdown: StartupShutdown<'a>,
}

/// Native cut selection; skip_matching counts only checkpoints for this exact
/// path and phase. fail_registry_commit injects the repository's real device
/// write error after pairing setup, selecting actual compensation if requested.
#[derive(Clone, Copy)]
pub(crate) struct EnrollmentCut<'a> {
    pub target: &'a Path,
    pub phase: MobileWritePhase,
    pub skip_matching: usize,
    pub fail_registry_commit: bool,
}

impl EnrollmentCut<'_> {
    pub(crate) fn marker(&self) -> Vec<u8> {
        format!(
            "held-{}-skip-{}\n",
            phase_name(self.phase),
            self.skip_matching
        )
        .into_bytes()
    }
}

fn phase_name(phase: MobileWritePhase) -> &'static str {
    match phase {
        MobileWritePhase::TempCreated => "temp_created",
        MobileWritePhase::Written => "written",
        MobileWritePhase::FileSynced => "file_synced",
        MobileWritePhase::Renamed => "renamed",
        MobileWritePhase::DirectorySynced => "directory_synced",
        MobileWritePhase::RetainedParentSynced => "retained_parent_synced",
        MobileWritePhase::RetainedHomeSynced => "retained_home_synced",
    }
}

fn parse_phase(value: &str) -> MobileWritePhase {
    match value {
        "temp_created" => MobileWritePhase::TempCreated,
        "written" => MobileWritePhase::Written,
        "file_synced" => MobileWritePhase::FileSynced,
        "renamed" => MobileWritePhase::Renamed,
        "directory_synced" => MobileWritePhase::DirectorySynced,
        "retained_parent_synced" => MobileWritePhase::RetainedParentSynced,
        "retained_home_synced" => MobileWritePhase::RetainedHomeSynced,
        _ => panic!("unsupported native mobile write cut phase: {value}"),
    }
}

/// Callers must supply distinct test-owned ready paths, absent before startup.
/// Hold targets refer to the child's REAL config::held_write hook, not the
/// parent's process-local map. Readiness means that exact child checkpoint was
/// entered. This is still a synthetic hold, not a physical disk stall.
pub(crate) fn start_fixture(
    home: &Path,
    held_written_target: Option<&Path>,
    observed_file_lock: Option<&Path>,
    ready: &Path,
) -> Result<Option<Handle>> {
    start_fixture_observing(home, held_written_target, observed_file_lock, ready, |_| {})
}

pub(crate) fn start_fixture_observing(
    home: &Path,
    held_written_target: Option<&Path>,
    observed_file_lock: Option<&Path>,
    ready: &Path,
    observe: impl FnOnce(&Handle),
) -> Result<Option<Handle>> {
    start_fixture_with_enrollment(
        home,
        held_written_target,
        observed_file_lock,
        ready,
        FixtureExtras::default(),
        observe,
    )
}

pub(crate) fn start_enrollment_fixture(
    home: &Path,
    target: &Path,
    ready: &Path,
    prepared: &Path,
) -> Result<Option<Handle>> {
    start_fixture_with_enrollment(
        home,
        Some(target),
        None,
        ready,
        FixtureExtras {
            prepared: Some(prepared),
            ..FixtureExtras::default()
        },
        |_| {},
    )
}

pub(crate) fn start_enrollment_cut_fixture(
    home: &Path,
    cut: EnrollmentCut<'_>,
    ready: &Path,
    prepared: &Path,
) -> Result<Option<Handle>> {
    start_fixture_with_enrollment(
        home,
        Some(cut.target),
        None,
        ready,
        FixtureExtras {
            prepared: Some(prepared),
            cut: Some(cut),
            ..FixtureExtras::default()
        },
        |_| {},
    )
}

fn start_fixture_with_enrollment(
    home: &Path,
    held_written_target: Option<&Path>,
    observed_file_lock: Option<&Path>,
    ready: &Path,
    extras: FixtureExtras<'_>,
    observe: impl FnOnce(&Handle),
) -> Result<Option<Handle>> {
    let FixtureExtras {
        prepared,
        cut,
        v2_prepare,
        shutdown,
    } = extras;
    let user_home = home.join("test-worker-user-home");
    std::fs::create_dir_all(&user_home)?;
    ensure!(!ready.exists(), "worker readiness path must be fresh");
    ensure!(
        prepared.is_none_or(|path| !path.exists()),
        "pending pairing fixture path must be fresh"
    );
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", FIXTURE, "--ignored", "--nocapture"]);
    // Normal settings initialization still runs against this isolated HOME.
    // Keep normal provider/settings behavior in production; never point unit
    // fixture children at the user's actual configured memory/provider roots.
    command
        .env("HOME", &user_home)
        .env("USERPROFILE", &user_home)
        .env("XDG_CONFIG_HOME", user_home.join(".config"))
        .env("COVEN_HOME", home)
        .env(HOME_ENV, home)
        .env(READY_ENV, ready);
    if let Some(path) = held_written_target {
        command.env(HOLD_ENV, path);
    } else {
        command.env_remove(HOLD_ENV);
    }
    if let Some(path) = observed_file_lock {
        command.env(WAIT_ENV, path);
    } else {
        command.env_remove(WAIT_ENV);
    }
    if let Some(prepared) = prepared {
        command.env(INITIAL_ENROLLMENT_ENV, prepared);
    } else {
        command.env_remove(INITIAL_ENROLLMENT_ENV);
    }
    // Explicit removal prevents a parent test's cut knobs from leaking into
    // ordinary/restart fixture children.
    command
        .env_remove(HOLD_PHASE_ENV)
        .env_remove(HOLD_SKIP_ENV)
        .env_remove(FAIL_REGISTRY_ENV);
    if let Some(cut) = cut {
        ensure!(
            held_written_target == Some(cut.target),
            "native cut target mismatch"
        );
        command
            .env(HOLD_PHASE_ENV, phase_name(cut.phase))
            .env(HOLD_SKIP_ENV, cut.skip_matching.to_string());
        if cut.fail_registry_commit {
            command.env(FAIL_REGISTRY_ENV, "1");
        }
    }
    if let Some((device, outcome)) = v2_prepare {
        ensure!(!outcome.exists(), "v2 fixture outcome must be fresh");
        command
            .env(V2_DEVICE_ENV, device.to_string())
            .env(V2_OUTCOME_ENV, outcome);
    } else {
        command.env_remove(V2_DEVICE_ENV).env_remove(V2_OUTCOME_ENV);
    }
    match shutdown {
        Some(predicate) => {
            start_command_observing_with_shutdown(command, SOURCE, true, observe, Some(predicate))
        }
        None => start_command_observing(command, SOURCE, true, observe),
    }
}

#[test]
#[ignore = "explicit mobile worker child fixture; exits its process"]
fn worker_fixture() {
    let Some(home) = std::env::var_os(HOME_ENV).map(PathBuf::from) else {
        return;
    };
    let ready = PathBuf::from(std::env::var_os(READY_ENV).expect("fixture ready path"));
    // Register process-local observers before the worker enters any filesystem
    // work. No observation bridge writes until a real failed attempt/phase.
    let _held = std::env::var_os(HOLD_ENV).map(|path| {
        let selected_phase = std::env::var(HOLD_PHASE_ENV).ok();
        let phase = selected_phase
            .as_deref()
            .map(parse_phase)
            .unwrap_or(MobileWritePhase::Written);
        let skip = std::env::var(HOLD_SKIP_ENV)
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .expect("native cut skip must be a nonnegative integer")
            })
            .unwrap_or(0);
        let (held, entered) =
            super::super::super::config::held_write::hold_after(Path::new(&path), phase, skip);
        let marker = if selected_phase.is_some() {
            EnrollmentCut {
                target: Path::new(&path),
                phase,
                skip_matching: skip,
                fail_registry_commit: false,
            }
            .marker()
        } else {
            b"held-written\n".to_vec()
        };
        let ready = ready.clone();
        std::thread::spawn(move || {
            if entered.recv().is_ok() {
                publish_test_observation(&ready, &marker).expect("publish held phase");
            }
        });
        held
    });
    let _observed = std::env::var_os(WAIT_ENV).map(|path| {
        let (observer, entered) = super::super::super::store_lock::observe(Path::new(&path));
        std::thread::spawn(move || {
            if entered.recv().is_ok() {
                publish_test_observation(&ready, b"failed-file-lock\n")
                    .expect("publish failed lock attempt");
            }
        });
        observer
    });
    prepare_pending_initial_enrollment_fixture();
    prepare_v2_fixture_action();
    match run_worker(PROTOCOL, &home) {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("mobile test worker failed: {error:#}");
            std::process::exit(70);
        }
    }
}

fn prepare_pending_initial_enrollment_fixture() {
    let Some(prepared) = std::env::var_os(INITIAL_ENROLLMENT_ENV).map(PathBuf::from) else {
        return;
    };
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let state = loop {
            match super::super::active_gateway() {
                Ok(state) => break state,
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "native gateway never became available for pairing fixture"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        };
        let _scope = state.storage_runtime.enter();
        let (id, phrase) = state
            .pairing
            .prepare_native_initial_enrollment_fixture(state.host_fingerprint)
            .unwrap();
        let injected_failure = std::env::var_os(FAIL_REGISTRY_ENV).is_some();
        if injected_failure {
            super::super::super::registry::fail_next_device_registry_write(
                &state.coven_home.join("mobile/devices.json"),
            );
        }
        let bytes = serde_json::to_vec(&serde_json::json!({ "id": id, "phrase": phrase, "registryWriteFailureInjected": injected_failure })).unwrap();
        // Publish the completed setup atomically; the parent's observation
        // must never parse the output while this thread is still writing it.
        super::super::super::config::atomic_replace_private(&prepared, &bytes).unwrap();
    });
}

/// Exercise real v2 prepare in the contained native gateway, with no new route.
pub(crate) fn start_v2_prepare_fixture(
    home: &Path,
    held: Option<&Path>,
    phase: MobileWritePhase,
    ready: &Path,
    device: uuid::Uuid,
    outcome: &Path,
    observe: impl FnOnce(&Handle),
) -> Result<Option<Handle>> {
    let cut = held.map(|target| EnrollmentCut {
        target,
        phase,
        skip_matching: 0,
        fail_registry_commit: false,
    });
    start_fixture_with_enrollment(
        home,
        held,
        None,
        ready,
        FixtureExtras {
            cut,
            v2_prepare: Some((device, outcome)),
            ..FixtureExtras::default()
        },
        observe,
    )
}

pub(crate) fn crash_fixture(handle: &mut Handle) -> Result<()> {
    let owned = handle
        .owned
        .as_ref()
        .context("fixture runtime already stopped")?;
    owned.close_admission();
    owned.terminate();
    handle.shutdown_until(std::time::Instant::now() + std::time::Duration::from_millis(1800))
}

fn prepare_v2_fixture_action() {
    let Some(device) = std::env::var_os(V2_DEVICE_ENV) else {
        return;
    };
    let device = uuid::Uuid::parse_str(&device.to_string_lossy()).expect("v2 fixture device UUID");
    let outcome = PathBuf::from(std::env::var_os(V2_OUTCOME_ENV).expect("v2 fixture outcome path"));
    std::thread::spawn(move || {
        use p256::pkcs8::DecodePrivateKey;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let state = loop {
            if let Ok(state) = super::super::active_gateway() {
                break state;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native gateway never became available for v2 fixture"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let _scope = state.storage_runtime.enter();
        let result = (|| -> Result<_> {
            let identity =
                super::super::super::identity::load_existing_host_identity(&state.coven_home)?;
            let host = p256::ecdsa::SigningKey::from_pkcs8_der(&identity.private_key_der)?;
            super::super::super::transport::storage::v2::prepare_direct(
                &state.coven_home,
                &state.registry,
                &host,
                device,
            )
        })();
        let result = match result {
            Ok(handover) => serde_json::json!({"ok":true,"data":handover}),
            Err(error) => serde_json::json!({"ok":false,"error":format!("{error:#}")}),
        };
        let encoded = serde_json::to_vec(&result).expect("encode v2 fixture outcome");
        super::super::super::config::atomic_replace_private(&outcome, &encoded)
            .expect("publish v2 fixture outcome");
    });
}

/// Only the daemon lifecycle opts into the daemon termination predicate.
/// Standalone component factories never consult the process-global flag.
pub(crate) fn start_daemon_fixture(
    home: &Path,
    held: Option<&Path>,
    ready: &Path,
    observe: impl FnOnce(&Handle),
    shutdown: &dyn Fn() -> Option<Instant>,
) -> Result<Option<Handle>> {
    start_fixture_with_enrollment(
        home,
        held,
        None,
        ready,
        FixtureExtras {
            shutdown: Some(shutdown),
            ..FixtureExtras::default()
        },
        observe,
    )
}
