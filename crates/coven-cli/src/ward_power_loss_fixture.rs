//! Synthetic Ward power-loss fixture for real-daemon journeys (#1640).
//!
//! Compiled only with the `threads-test-clock` feature, so default and release
//! builds contain none of it. It activates only when the daemon's own home
//! holds an owner-private `test-fixtures/ward-power-loss/` directory with an
//! activation sentinel and a capability. When active, the daemon registers the
//! xSync-observing VFS for exactly `<home>/coven.sqlite3` and its WAL before
//! the store is first opened.
//!
//! Control is owner-IPC only and requires the capability. Requests carry no
//! paths: the observed store, the frozen-image directory and the one-shot
//! faults are all fixed. Nothing here touches Threads verification, Ward
//! adjudication or any write authority; it only records which store bytes
//! were synced and can fail the next store sync or store-directory barrier.
//! Daemon termination alone is not a power-loss model: a journey freezes the
//! synced images, terminates the daemon and restores those images.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};

use crate::power_loss_tests::PowerLoss;

const FIXTURE_ROOT_DIRECTORY: &str = "test-fixtures";
const FIXTURE_DIRECTORY_NAME: &str = "ward-power-loss";
const ACTIVATION_FILE: &str = "enabled";
const ACTIVATION_SENTINEL: &str = "ward_power_loss_v1";
const CAPABILITY_FILE: &str = "capability";
const FROZEN_DATABASE: &str = "frozen-coven.sqlite3";
const FROZEN_WAL: &str = "frozen-coven.sqlite3-wal";

struct Active {
    home: PathBuf,
    capability: String,
    loss: PowerLoss,
    fail_next_directory_barrier: bool,
}

fn active() -> &'static Mutex<Option<Active>> {
    static ACTIVE: OnceLock<Mutex<Option<Active>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug)]
pub(crate) struct InactiveFixture;
impl std::fmt::Display for InactiveFixture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("synthetic Ward power-loss fixture is not active")
    }
}
impl std::error::Error for InactiveFixture {}

#[derive(Debug)]
pub(crate) struct InvalidFixtureCapability;
impl std::fmt::Display for InvalidFixtureCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("synthetic Ward power-loss capability was rejected")
    }
}
impl std::error::Error for InvalidFixtureCapability {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Action {
    /// Checkpoint the store with TRUNCATE and restart observation from it.
    Baseline,
    /// Copy the last synced database/WAL images into the fixed frozen directory.
    Freeze,
    /// Fail the next SQLite xSync of the observed store once.
    FailNextStoreSync,
    /// Fail the next post-COMMIT store-directory barrier once.
    FailNextStoreDirectoryBarrier,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ControlRequest {
    pub(crate) capability: String,
    pub(crate) action: Action,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlReport {
    pub(crate) action: &'static str,
    pub(crate) database_bytes: Option<usize>,
    pub(crate) wal_bytes: Option<usize>,
}

fn fixture_root(coven_home: &Path) -> PathBuf {
    coven_home.join(FIXTURE_ROOT_DIRECTORY)
}

fn fixture_directory(coven_home: &Path) -> PathBuf {
    fixture_root(coven_home).join(FIXTURE_DIRECTORY_NAME)
}

fn read_marker(path: &Path, label: &str) -> Result<String> {
    crate::mobile_memory::config::validate_private_file(path)?;
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading Ward power-loss fixture {label}"))?;
    let value = raw.trim_end_matches(['\r', '\n']);
    anyhow::ensure!(
        !value.is_empty() && !value.chars().any(char::is_control),
        "Ward power-loss fixture {label} is invalid"
    );
    Ok(value.to_owned())
}

/// Register the observing VFS when this daemon's home requests the fixture.
/// Must run before the daemon first opens its store. A home without the
/// activation file is untouched; a malformed or non-private fixture fails
/// startup closed rather than running unobserved.
pub(crate) fn activate_for_daemon(coven_home: &Path) -> Result<()> {
    let directory = fixture_directory(coven_home);
    let activation = directory.join(ACTIVATION_FILE);
    match fs::symlink_metadata(&activation) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).context("reading Ward power-loss fixture activation");
        }
    }
    crate::mobile_memory::config::validate_private_directory(&fixture_root(coven_home))?;
    crate::mobile_memory::config::validate_private_directory(&directory)?;
    anyhow::ensure!(
        read_marker(&activation, "activation")? == ACTIVATION_SENTINEL,
        "Ward power-loss fixture activation marker is invalid"
    );
    let capability = read_marker(&directory.join(CAPABILITY_FILE), "capability")?;
    let mut slot = active()
        .lock()
        .map_err(|_| anyhow::anyhow!("Ward power-loss fixture lock is poisoned"))?;
    anyhow::ensure!(
        slot.is_none(),
        "Ward power-loss fixture is already active in this process"
    );
    let loss = PowerLoss::observe(&coven_home.join("coven.sqlite3"));
    *slot = Some(Active {
        home: coven_home.to_path_buf(),
        capability,
        loss,
        fail_next_directory_barrier: false,
    });
    Ok(())
}

/// Consume an armed store-directory barrier failure for `database`. Called by
/// the durable transaction after COMMIT, so a failure here leaves committed
/// evidence exactly as a real failed directory sync would.
pub(crate) fn take_store_directory_barrier_failure(database: Option<&str>) -> Result<()> {
    let Some(database) = database.filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    let mut slot = active()
        .lock()
        .map_err(|_| anyhow::anyhow!("Ward power-loss fixture lock is poisoned"))?;
    let Some(active) = slot.as_mut() else {
        return Ok(());
    };
    if !active.fail_next_directory_barrier
        || !same_file_path(Path::new(database), active.loss.database_path())
    {
        return Ok(());
    }
    active.fail_next_directory_barrier = false;
    anyhow::bail!("injected Ward power-loss store directory barrier failure")
}

fn same_file_path(candidate: &Path, observed: &Path) -> bool {
    let canonical = candidate
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(candidate.file_name())
        .map(|(parent, name)| parent.join(name));
    canonical.as_deref() == Some(observed)
}

/// Apply one owner-IPC control action. `coven_home` must be the daemon's own
/// home; the request names no paths.
pub(crate) fn control(coven_home: &Path, request: &ControlRequest) -> Result<ControlReport> {
    let lock = || {
        active()
            .lock()
            .map_err(|_| anyhow::anyhow!("Ward power-loss fixture lock is poisoned"))
    };
    let authorize = |slot: &Option<Active>| -> Result<()> {
        let active = slot
            .as_ref()
            .filter(|active| active.home == coven_home)
            .ok_or(InactiveFixture)?;
        if active.capability != request.capability {
            return Err(InvalidFixtureCapability.into());
        }
        Ok(())
    };
    let database = {
        let slot = lock()?;
        authorize(&slot)?;
        slot.as_ref()
            .map(|active| active.loss.database_path().to_path_buf())
            .ok_or(InactiveFixture)?
    };
    if request.action == Action::Baseline {
        // Release the fixture lock while SQLite runs: a store open must never
        // re-enter it through the durable-transaction barrier hook.
        let conn = crate::store::open_store(&database)?;
        let (busy, _, _): (i64, i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        anyhow::ensure!(
            busy == 0,
            "Ward power-loss baseline checkpoint was blocked by another connection"
        );
        drop(conn);
    }
    let mut slot = lock()?;
    authorize(&slot)?;
    let active = slot.as_mut().ok_or(InactiveFixture)?;
    let images = || active.loss.synced_images();
    match request.action {
        Action::Baseline => {
            active.loss.restart_from_checkpointed_database()?;
            let images = images();
            Ok(ControlReport {
                action: "baseline",
                database_bytes: images.get(active.loss.database_path()).map(Vec::len),
                wal_bytes: images.get(active.loss.wal_path()).map(Vec::len),
            })
        }
        Action::Freeze => {
            let images = images();
            let database = images
                .get(active.loss.database_path())
                .context("Ward power-loss fixture never observed a synced database")?;
            let directory = fixture_directory(coven_home);
            crate::mobile_memory::config::validate_private_directory(&directory)?;
            crate::mobile_memory::config::atomic_replace_private(
                &directory.join(FROZEN_DATABASE),
                database,
            )?;
            let wal = images.get(active.loss.wal_path());
            match wal {
                Some(bytes) => crate::mobile_memory::config::atomic_replace_private(
                    &directory.join(FROZEN_WAL),
                    bytes,
                )?,
                None => match fs::remove_file(directory.join(FROZEN_WAL)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("clearing frozen Ward WAL image"),
                },
            }
            Ok(ControlReport {
                action: "freeze",
                database_bytes: Some(database.len()),
                wal_bytes: wal.map(Vec::len),
            })
        }
        Action::FailNextStoreSync => {
            active.loss.fail_next_sync();
            Ok(ControlReport {
                action: "fail-next-store-sync",
                database_bytes: None,
                wal_bytes: None,
            })
        }
        Action::FailNextStoreDirectoryBarrier => {
            active.fail_next_directory_barrier = true;
            Ok(ControlReport {
                action: "fail-next-store-directory-barrier",
                database_bytes: None,
                wal_bytes: None,
            })
        }
    }
}
