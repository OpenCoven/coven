//! Synthetic Ward power-loss fixture for real-daemon journeys (#1640).
//!
//! Compiled only with the `threads-test-clock` feature. Builds without that
//! feature, including the current release CLI artifact builds, contain none of it.
//! It activates only when the daemon's own home
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
use std::time::Duration;

use anyhow::{Context, Result};

use crate::power_loss_tests::{CommitAfterCheckpoint, PowerLoss};

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
    /// Set once the daemon's startup store has closed. Until then its
    /// post-readiness checkpoint may still run.
    startup_store_closed: bool,
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

/// A baseline was refused because the store was not quiescent. Nothing was
/// changed, so the caller may ask again.
#[derive(Debug)]
pub(crate) struct FixtureNotQuiescent(&'static str);
impl std::fmt::Display for FixtureNotQuiescent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Ward power-loss baseline refused: {}", self.0)
    }
}
impl std::error::Error for FixtureNotQuiescent {}

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
        startup_store_closed: false,
    });
    Ok(())
}

/// Record that the daemon serving `coven_home` has closed its startup store.
///
/// That close runs a PASSIVE checkpoint after readiness (#1583). A baseline
/// that overlapped it would find the checkpoint lock held, and one that
/// preceded it would have the checkpoint sync the WAL underneath a journey's
/// armed fault or unsynced tail. Baselines are refused until this is called.
pub(crate) fn note_startup_store_closed(coven_home: &Path) {
    let Ok(mut slot) = active().lock() else {
        return;
    };
    if let Some(active) = slot.as_mut().filter(|active| active.home == coven_home) {
        active.startup_store_closed = true;
    }
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

/// How long one baseline waits on a reader or writer. It stays well under the
/// client's five-second response deadline, so a busy store reaches the caller
/// as a refusal it can wait on, not as a client timeout.
const BASELINE_BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// Run the baseline's TRUNCATE checkpoint once.
///
/// SQLite reports a checkpoint lock held by another connection as busy at
/// once, without calling the busy handler. That and a reader or writer that
/// outlasts the busy timeout both leave the WAL in place, so both are refused
/// as not quiescent: observation never restarts from an unfinished checkpoint.
fn truncate_checkpoint_for_baseline(conn: &rusqlite::Connection) -> Result<()> {
    let (busy, _, _): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(FixtureNotQuiescent("its checkpoint was blocked by another connection").into());
    }
    Ok(())
}

/// Restart observation after the baseline's checkpoint. A daemon commit that
/// slipped in behind the checkpoint is refused as not quiescent: nothing was
/// discarded, and a repeated baseline checkpoints it.
fn restart_observation(loss: &PowerLoss) -> Result<()> {
    loss.restart_from_checkpointed_database().map_err(|error| {
        if error.is::<CommitAfterCheckpoint>() {
            FixtureNotQuiescent("a commit reached the WAL behind its checkpoint").into()
        } else {
            error
        }
    })
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
        let active = slot.as_ref().ok_or(InactiveFixture)?;
        if request.action == Action::Baseline && !active.startup_store_closed {
            return Err(FixtureNotQuiescent("the daemon's startup store is still open").into());
        }
        active.loss.database_path().to_path_buf()
    };
    if request.action == Action::Baseline {
        // Release the fixture lock while SQLite runs: a store open must never
        // re-enter it through the durable-transaction barrier hook.
        let conn = crate::store::open_store(&database)?;
        conn.busy_timeout(BASELINE_BUSY_TIMEOUT)?;
        truncate_checkpoint_for_baseline(&conn)?;
        drop(conn);
    }
    let mut slot = lock()?;
    authorize(&slot)?;
    let active = slot.as_mut().ok_or(InactiveFixture)?;
    let images = || active.loss.synced_images();
    match request.action {
        Action::Baseline => {
            restart_observation(&active.loss)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

    // Every test here carries a `threads_` prefix on purpose: hosted CI runs
    // the feature-built unit tests with a `threads_` name filter.

    static CHECKPOINTER_PARKED: AtomicBool = AtomicBool::new(false);
    static BASELINE_BUSY_HANDLER_CALLS: AtomicU32 = AtomicU32::new(0);

    /// Busy handler for the concurrent checkpointer. SQLite calls it while the
    /// checkpointer waits for the writer lock, which it does only after taking
    /// the checkpoint lock, so a parked checkpointer holds that lock.
    fn park_holding_the_checkpoint_lock(waits: i32) -> bool {
        CHECKPOINTER_PARKED.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1));
        // Hang guard only: about a minute, far above what load can produce.
        waits < 60_000
    }

    fn count_and_give_up(_waits: i32) -> bool {
        BASELINE_BUSY_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst);
        false
    }

    #[test]
    fn threads_baseline_checkpoint_refuses_a_held_checkpoint_lock_without_waiting() -> Result<()> {
        // Hang guard only, far above what load can produce.
        const HANG_GUARD: Duration = Duration::from_secs(60);
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("coven.sqlite3");
        let mut conn = crate::store::open_store(&path)?;
        conn.execute(
            "INSERT INTO store_meta(key, value) VALUES ('baseline-probe', 'frame')",
            [],
        )?;
        conn.busy_handler(Some(count_and_give_up))?;

        // A writer makes a FULL checkpoint on another connection wait, and
        // that checkpoint takes the checkpoint lock before it waits.
        let writer = rusqlite::Connection::open(&path)?;
        writer.execute_batch("BEGIN IMMEDIATE")?;
        let (done_tx, done_rx) = mpsc::channel();
        let checkpointer_path = path.clone();
        let checkpointer = std::thread::spawn(move || {
            let result = (|| -> Result<i64> {
                let mut conn = rusqlite::Connection::open(&checkpointer_path)?;
                conn.busy_handler(Some(park_holding_the_checkpoint_lock))?;
                Ok(conn.query_row("PRAGMA wal_checkpoint(FULL)", [], |row| row.get(0))?)
            })();
            let _ = done_tx.send(result);
        });
        let started = Instant::now();
        while !CHECKPOINTER_PARKED.load(Ordering::SeqCst) {
            if let Ok(result) = done_rx.try_recv() {
                anyhow::bail!("the concurrent checkpoint finished without waiting: {result:?}");
            }
            anyhow::ensure!(
                started.elapsed() < HANG_GUARD,
                "the concurrent checkpoint never parked after {:?}",
                started.elapsed()
            );
            std::thread::sleep(Duration::from_millis(1));
        }

        let error = truncate_checkpoint_for_baseline(&conn)
            .expect_err("a blocked baseline checkpoint must not report success");
        assert!(error.is::<FixtureNotQuiescent>(), "{error:#}");
        // The writer is still open, so a wait on the writer lock would have
        // called this connection's busy handler. Zero calls means the refusal
        // came from the checkpoint lock, which SQLite never waits on.
        assert_eq!(BASELINE_BUSY_HANDLER_CALLS.load(Ordering::SeqCst), 0);

        writer.execute_batch("ROLLBACK")?;
        checkpointer
            .join()
            .map_err(|_| anyhow::anyhow!("the concurrent checkpointer panicked"))?;
        let concurrent_busy = done_rx.recv()??;
        assert_eq!(
            concurrent_busy, 0,
            "the concurrent checkpoint did not complete"
        );

        truncate_checkpoint_for_baseline(&conn)?;
        let mut wal = path.into_os_string();
        wal.push("-wal");
        assert_eq!(
            fs::metadata(&wal)?.len(),
            0,
            "the baseline did not leave a truncated WAL"
        );
        Ok(())
    }

    fn write_marker(path: &Path, contents: &str) -> Result<()> {
        use std::io::Write as _;
        let parent = path.parent().context("marker parent")?;
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        staged.write_all(contents.as_bytes())?;
        staged
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
        Ok(())
    }

    #[test]
    fn threads_baseline_is_refused_until_the_startup_store_has_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let directory = fixture_directory(&home);
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&fixture_root(&home), &directory] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            }
        }
        write_marker(&directory.join(ACTIVATION_FILE), "ward_power_loss_v1\n")?;
        write_marker(&directory.join(CAPABILITY_FILE), "fixture-cap")?;
        // This is the only test that activates the process-wide fixture.
        activate_for_daemon(&home)?;
        let startup_store = crate::store::open_store(&home.join("coven.sqlite3"))?;
        let baseline = ControlRequest {
            capability: "fixture-cap".to_owned(),
            action: Action::Baseline,
        };

        let error = control(&home, &baseline)
            .expect_err("a baseline must wait for the startup store to close");
        assert!(error.is::<FixtureNotQuiescent>(), "{error:#}");
        // Another daemon's close does not open this fixture's gate.
        note_startup_store_closed(temp.path().join("another-home").as_path());
        let error = control(&home, &baseline)
            .expect_err("another home's startup close must not open the gate");
        assert!(error.is::<FixtureNotQuiescent>(), "{error:#}");
        // The other actions never waited on the startup store.
        control(
            &home,
            &ControlRequest {
                capability: "fixture-cap".to_owned(),
                action: Action::FailNextStoreDirectoryBarrier,
            },
        )?;
        take_store_directory_barrier_failure(home.join("coven.sqlite3").to_str())
            .expect_err("the armed barrier fault must be consumed here");

        note_startup_store_closed(&home);
        let report = control(&home, &baseline)?;
        assert_eq!(report.action, "baseline");
        assert!(report.database_bytes.is_some_and(|bytes| bytes > 0));

        // A commit behind the checkpoint is refused the same way, with the
        // observed images left alone, and the next baseline takes it in.
        startup_store.execute(
            "INSERT INTO store_meta(key, value) VALUES ('behind-the-checkpoint', 'frame')",
            [],
        )?;
        {
            let slot = active().lock().expect("fixture lock");
            let loss = &slot.as_ref().expect("active fixture").loss;
            let before = loss.synced_images();
            let error = restart_observation(loss)
                .expect_err("a commit behind the checkpoint must not be discarded");
            assert!(error.is::<FixtureNotQuiescent>(), "{error:#}");
            assert_eq!(loss.synced_images(), before);
        }
        control(&home, &baseline)?;
        Ok(())
    }
}
