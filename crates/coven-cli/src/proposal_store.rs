use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
};

use anyhow::{Context, Result};
use fs2::FileExt;
use uuid::Uuid;

/// A daemon normally has only a handful of human decisions outstanding. This
/// allows a substantial review backlog while keeping every bounded list pass
/// small enough for interactive use.
pub(crate) const MAX_PENDING_PROPOSALS: usize = 64;
/// Local transports accept at most 4 MiB request bodies. Sixty-four MiB leaves
/// room for several maximum-size encoded proposals while placing a meaningful
/// ceiling on persistent disk use and aggregate parse work.
pub(crate) const MAX_PENDING_PROPOSAL_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const PROPOSAL_SCHEDULER_BATCH_LIMIT: usize = 16;
/// Human review should complete well inside a month; terminally rejecting
/// older proposals prevents abandoned decisions from reserving quota forever.
pub(crate) const PENDING_PROPOSAL_RETENTION_DAYS: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProposalQuotaExceeded {
    Count {
        current_count: usize,
        attempted_count: usize,
        max_count: usize,
    },
    Bytes {
        current_bytes: u64,
        incoming_bytes: u64,
        attempted_bytes: u64,
        max_bytes: u64,
    },
}

impl std::fmt::Display for ProposalQuotaExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count {
                current_count,
                attempted_count,
                max_count,
            } => write!(
                formatter,
                "pending proposal count quota exceeded: {current_count} existing, \
                 {attempted_count} attempted, {max_count} maximum"
            ),
            Self::Bytes {
                current_bytes,
                incoming_bytes,
                attempted_bytes,
                max_bytes,
            } => write!(
                formatter,
                "pending proposal byte quota exceeded: {current_bytes} existing bytes plus \
                 {incoming_bytes} incoming bytes is {attempted_bytes}, {max_bytes} maximum"
            ),
        }
    }
}

impl std::error::Error for ProposalQuotaExceeded {}

pub(crate) fn quota_failure(error: &anyhow::Error) -> Option<&ProposalQuotaExceeded> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ProposalQuotaExceeded>())
}

fn process_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct ProposalStoreLock {
    _process: MutexGuard<'static, ()>,
    file: fs::File,
}

impl ProposalStoreLock {
    fn acquire(pending_dir: &Path) -> Result<Self> {
        fs::create_dir_all(pending_dir)
            .with_context(|| format!("creating {}", pending_dir.display()))?;
        let process = process_lock()
            .lock()
            .map_err(|_| anyhow::anyhow!("pending proposal store lock is poisoned"))?;
        let lock_path = pending_dir.join(".quota.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening pending proposal lock {}", lock_path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("locking pending proposal store {}", pending_dir.display()))?;
        Ok(Self {
            _process: process,
            file,
        })
    }
}

impl Drop for ProposalStoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) fn is_active_proposal_file(name: &str) -> bool {
    name.ends_with(".json")
        || name.ends_with(".json.approve.deciding")
        || name.ends_with(".json.reject.deciding")
}

pub(crate) fn is_pending_proposal_file(name: &str) -> bool {
    name.ends_with(".json")
}

fn is_non_utf8_active_proposal_name(name: &OsStr) -> bool {
    if name.to_str().is_some() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        let name = name.as_bytes();
        [
            b".json".as_slice(),
            b".json.approve.deciding".as_slice(),
            b".json.reject.deciding".as_slice(),
        ]
        .into_iter()
        .any(|suffix| name.ends_with(suffix))
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn quarantine_locked(pending_dir: &Path, path: &Path, reason: &str) -> Result<Option<PathBuf>> {
    anyhow::ensure!(
        path.parent() == Some(pending_dir),
        "pending proposal quarantine source is outside the proposal store"
    );
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading quarantine source {}", path.display()))
        }
    }
    let file_name = path
        .file_name()
        .context("pending proposal quarantine source has no filename")?;
    let quarantine_dir = pending_dir.join("quarantine");
    fs::create_dir_all(&quarantine_dir)
        .with_context(|| format!("creating {}", quarantine_dir.display()))?;
    let destination_name = file_name.to_str().map_or_else(
        || format!("non-utf8.{reason}.{}", Uuid::new_v4()),
        |file_name| format!("{file_name}.{reason}.{}", Uuid::new_v4()),
    );
    let destination = quarantine_dir.join(destination_name);
    fs::rename(path, &destination).with_context(|| {
        format!(
            "quarantining pending proposal {} at {}",
            path.display(),
            destination.display()
        )
    })?;
    Ok(Some(destination))
}

fn reconcile_non_utf8_active_names_locked(pending_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(pending_dir)
        .with_context(|| format!("reading pending proposal store {}", pending_dir.display()))?
    {
        let entry = entry?;
        if !is_non_utf8_active_proposal_name(&entry.file_name()) {
            continue;
        }
        quarantine_locked(pending_dir, &entry.path(), "invalid-name")?;
    }
    Ok(())
}

pub(crate) fn reconcile_invalid_active_names(coven_home: &Path) -> Result<()> {
    let pending_dir = coven_home.join("pending");
    match fs::symlink_metadata(&pending_dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => anyhow::bail!("pending proposal store is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("reading pending proposal store {}", pending_dir.display())
            })
        }
    }
    let _guard = ProposalStoreLock::acquire(&pending_dir)?;
    reconcile_non_utf8_active_names_locked(&pending_dir)
}

pub(crate) fn quarantine(coven_home: &Path, path: &Path, reason: &str) -> Result<Option<PathBuf>> {
    let pending_dir = coven_home.join("pending");
    anyhow::ensure!(
        path.parent() == Some(pending_dir.as_path()),
        "pending proposal quarantine source is outside the proposal store"
    );
    let _guard = ProposalStoreLock::acquire(&pending_dir)?;
    quarantine_locked(&pending_dir, path, reason)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PendingProposalUsage {
    count: usize,
    bytes: u64,
}

fn pending_proposal_usage(
    pending_dir: &Path,
    excluded_path: Option<&Path>,
) -> Result<PendingProposalUsage> {
    reconcile_non_utf8_active_names_locked(pending_dir)?;
    let mut usage = PendingProposalUsage::default();
    for entry in fs::read_dir(pending_dir)
        .with_context(|| format!("reading pending proposal store {}", pending_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if excluded_path.is_some_and(|excluded| path == excluded) {
            continue;
        }
        let name = entry.file_name();
        if !name.to_str().is_some_and(is_active_proposal_file) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("reading pending proposal metadata {}", path.display()))?;
        if !metadata.file_type().is_file() {
            continue;
        }
        usage.count = usage.count.saturating_add(1);
        usage.bytes = usage.bytes.saturating_add(metadata.len());
        if excluded_path.is_none()
            && (usage.count >= MAX_PENDING_PROPOSALS || usage.bytes >= MAX_PENDING_PROPOSAL_BYTES)
        {
            break;
        }
    }
    Ok(usage)
}

fn validate_quota(usage: PendingProposalUsage, incoming_bytes: u64) -> Result<()> {
    let attempted_count = usage.count.saturating_add(1);
    if attempted_count > MAX_PENDING_PROPOSALS {
        return Err(ProposalQuotaExceeded::Count {
            current_count: usage.count,
            attempted_count,
            max_count: MAX_PENDING_PROPOSALS,
        }
        .into());
    }
    let attempted_bytes = usage.bytes.saturating_add(incoming_bytes);
    if attempted_bytes > MAX_PENDING_PROPOSAL_BYTES {
        return Err(ProposalQuotaExceeded::Bytes {
            current_bytes: usage.bytes,
            incoming_bytes,
            attempted_bytes,
            max_bytes: MAX_PENDING_PROPOSAL_BYTES,
        }
        .into());
    }
    Ok(())
}

fn validate_replacement_quota(usage: PendingProposalUsage, incoming_bytes: u64) -> Result<()> {
    let attempted_bytes = usage.bytes.saturating_add(incoming_bytes);
    if attempted_bytes > MAX_PENDING_PROPOSAL_BYTES {
        return Err(ProposalQuotaExceeded::Bytes {
            current_bytes: usage.bytes,
            incoming_bytes,
            attempted_bytes,
            max_bytes: MAX_PENDING_PROPOSAL_BYTES,
        }
        .into());
    }
    Ok(())
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let pending_dir = path
        .parent()
        .context("pending proposal path has no parent")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("pending proposal has a non-UTF-8 filename")?;
    let staged = pending_dir.join(format!(".{file_name}.{}.staged", Uuid::new_v4()));
    let write_result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staged)
            .with_context(|| format!("creating pending proposal stage {}", staged.display()))?;
        file.write_all(body)
            .with_context(|| format!("writing pending proposal stage {}", staged.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing pending proposal stage {}", staged.display()))?;
        fs::rename(&staged, path)
            .with_context(|| format!("committing pending proposal {}", path.display()))
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    write_result
}

pub(crate) fn publish_new(coven_home: &Path, path: &Path, body: &[u8]) -> Result<()> {
    let pending_dir = coven_home.join("pending");
    anyhow::ensure!(
        path.parent() == Some(pending_dir.as_path()),
        "pending proposal path is outside the proposal store"
    );
    let _guard = ProposalStoreLock::acquire(&pending_dir)?;
    let usage = pending_proposal_usage(&pending_dir, None)?;
    validate_quota(usage, u64::try_from(body.len()).unwrap_or(u64::MAX))?;
    write_atomic(path, body)
}

pub(crate) fn replace_existing(path: &Path, body: &[u8]) -> Result<()> {
    let pending_dir = path
        .parent()
        .context("pending proposal path has no parent")?;
    let _guard = ProposalStoreLock::acquire(pending_dir)?;
    anyhow::ensure!(
        path.exists(),
        "pending proposal disappeared before replacement"
    );
    let usage = pending_proposal_usage(pending_dir, Some(path))?;
    // Replacing a live proposal does not add an item. Ignoring the count cap
    // here lets rejection/expiry drain a pre-existing over-quota directory
    // after restart while the byte cap still prevents persistent growth.
    validate_replacement_quota(usage, u64::try_from(body.len()).unwrap_or(u64::MAX))?;
    write_atomic(path, body)
}

pub(crate) fn replace_existing_for_terminal_decision(path: &Path, body: &[u8]) -> Result<()> {
    let pending_dir = path
        .parent()
        .context("pending proposal path has no parent")?;
    let _guard = ProposalStoreLock::acquire(pending_dir)?;
    anyhow::ensure!(
        path.exists(),
        "pending proposal disappeared before replacement"
    );
    let incoming_bytes = u64::try_from(body.len()).unwrap_or(u64::MAX);
    if incoming_bytes > MAX_PENDING_PROPOSAL_BYTES {
        return Err(ProposalQuotaExceeded::Bytes {
            current_bytes: 0,
            incoming_bytes,
            attempted_bytes: incoming_bytes,
            max_bytes: MAX_PENDING_PROPOSAL_BYTES,
        }
        .into());
    }
    // Rejection and expiry never mutate a target and immediately consume the
    // claim after the terminal audit. Permit their small durable request
    // rewrite even when existing files already fill the aggregate quota so
    // operators and the scheduler can drain that backlog safely.
    write_atomic(path, body)
}

pub(crate) fn rename_existing(source: &Path, destination: &Path) -> Result<()> {
    let pending_dir = source
        .parent()
        .context("pending proposal source has no parent")?;
    anyhow::ensure!(
        destination.parent() == Some(pending_dir),
        "pending proposal rename leaves the proposal store"
    );
    let _guard = ProposalStoreLock::acquire(pending_dir)?;
    fs::rename(source, destination).with_context(|| {
        format!(
            "renaming pending proposal {} to {}",
            source.display(),
            destination.display()
        )
    })
}

pub(crate) fn remove_existing(path: &Path) -> Result<()> {
    let pending_dir = path
        .parent()
        .context("pending proposal path has no parent")?;
    let _guard = ProposalStoreLock::acquire(pending_dir)?;
    fs::remove_file(path).with_context(|| format!("removing pending proposal {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    const EXPECTED_MAX_PENDING_PROPOSAL_BYTES: u64 = 64 * 1024 * 1024;

    fn pending_path(home: &Path) -> PathBuf {
        home.join("pending")
            .join(format!("{}-{}.json", Uuid::new_v4(), Uuid::new_v4()))
    }

    #[test]
    fn concurrent_creators_do_not_over_admit_the_count_boundary() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let pending_dir = temp.path().join("pending");
        fs::create_dir_all(&pending_dir)?;
        for _ in 0..(MAX_PENDING_PROPOSALS - 1) {
            fs::write(pending_path(temp.path()), b"existing")?;
        }
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let home = temp.path().to_path_buf();
                let path = pending_path(&home);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    publish_new(&home, &path, b"incoming")
                })
            })
            .collect();

        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("quota worker did not panic"))
            .collect();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().err().and_then(quota_failure).is_some())
                .count(),
            7
        );
        let _guard = ProposalStoreLock::acquire(&pending_dir)?;
        assert_eq!(
            pending_proposal_usage(&pending_dir, None)?.count,
            MAX_PENDING_PROPOSALS
        );
        Ok(())
    }

    #[test]
    fn concurrent_creators_do_not_over_admit_the_byte_boundary() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let pending_dir = temp.path().join("pending");
        fs::create_dir_all(&pending_dir)?;
        let existing = pending_path(temp.path());
        let file = fs::File::create(existing)?;
        file.set_len(EXPECTED_MAX_PENDING_PROPOSAL_BYTES - 32)?;
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let home = temp.path().to_path_buf();
                let path = pending_path(&home);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    publish_new(&home, &path, &[b'x'; 32])
                })
            })
            .collect();

        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("quota worker did not panic"))
            .collect();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().err().and_then(quota_failure).is_some())
                .count(),
            1
        );
        let total_bytes = fs::read_dir(&pending_dir)?.try_fold(0_u64, |total, entry| {
            let entry = entry?;
            if is_active_proposal_file(&entry.file_name().to_string_lossy()) {
                total
                    .checked_add(entry.metadata()?.len())
                    .ok_or_else(|| std::io::Error::other("test byte total overflowed"))
            } else {
                Ok(total)
            }
        })?;
        assert_eq!(total_bytes, EXPECTED_MAX_PENDING_PROPOSAL_BYTES);
        Ok(())
    }

    #[test]
    fn single_proposal_over_global_bytes_is_rejected_without_staging_residue() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = pending_path(temp.path());
        let body = vec![b'x'; usize::try_from(MAX_PENDING_PROPOSAL_BYTES)? + 1];

        let error = publish_new(temp.path(), &path, &body)
            .expect_err("one proposal cannot exceed the global byte quota");

        assert!(matches!(
            quota_failure(&error),
            Some(ProposalQuotaExceeded::Bytes {
                current_bytes: 0,
                incoming_bytes,
                attempted_bytes,
                max_bytes: MAX_PENDING_PROPOSAL_BYTES,
            }) if *incoming_bytes == MAX_PENDING_PROPOSAL_BYTES + 1
                && *attempted_bytes == MAX_PENDING_PROPOSAL_BYTES + 1
        ));
        assert!(!path.exists());
        let entries = fs::read_dir(temp.path().join("pending"))?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()?;
        assert!(entries.iter().all(|name| !name.ends_with(".staged")));
        Ok(())
    }

    #[test]
    fn quota_reconciles_directory_state_after_restart_and_deletion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("pending"))?;
        let mut existing = Vec::new();
        for _ in 0..MAX_PENDING_PROPOSALS {
            let path = pending_path(temp.path());
            fs::write(&path, b"existing")?;
            existing.push(path);
        }
        let rejected_path = pending_path(temp.path());
        let error = publish_new(temp.path(), &rejected_path, b"incoming")
            .expect_err("reconciled files must fill the count quota");
        assert!(matches!(
            quota_failure(&error),
            Some(ProposalQuotaExceeded::Count {
                current_count: MAX_PENDING_PROPOSALS,
                ..
            })
        ));

        fs::remove_file(existing.pop().expect("fixture has an existing proposal"))?;
        publish_new(temp.path(), &rejected_path, b"incoming")?;

        let _guard = ProposalStoreLock::acquire(&temp.path().join("pending"))?;
        assert_eq!(
            pending_proposal_usage(&temp.path().join("pending"), None)?.count,
            MAX_PENDING_PROPOSALS
        );
        Ok(())
    }

    #[test]
    fn aggregate_byte_accounting_saturates_on_overflow() {
        let error = validate_quota(
            PendingProposalUsage {
                count: 1,
                bytes: u64::MAX,
            },
            1,
        )
        .expect_err("overflowed aggregate bytes must fail closed");

        assert!(matches!(
            quota_failure(&error),
            Some(ProposalQuotaExceeded::Bytes {
                attempted_bytes: u64::MAX,
                ..
            })
        ));
    }

    #[cfg(unix)]
    fn non_utf8_active_name(index: usize, suffix: &[u8]) -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;

        let mut name = format!("invalid-{index:03}-").into_bytes();
        name.push(0xff);
        name.extend_from_slice(suffix);
        std::ffi::OsString::from_vec(name)
    }

    #[cfg(unix)]
    #[test]
    fn classifies_non_utf8_active_suffix_without_lossy_aliasing() {
        let active = non_utf8_active_name(0, b".json.approve.deciding");
        let inactive = non_utf8_active_name(1, b".staged");

        assert!(is_non_utf8_active_proposal_name(&active));
        assert!(!is_non_utf8_active_proposal_name(&inactive));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_active_names_are_quarantined_and_do_not_consume_quota() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let pending = temp.path().join("pending");
        fs::create_dir_all(&pending)?;
        for index in 0..MAX_PENDING_PROPOSALS {
            let suffix = if index % 2 == 0 {
                b".json".as_slice()
            } else {
                b".json.approve.deciding".as_slice()
            };
            if let Err(error) = fs::write(
                pending.join(non_utf8_active_name(index, suffix)),
                b"invalid",
            ) {
                if error.raw_os_error() == Some(92) {
                    return Ok(());
                }
                return Err(error.into());
            }
        }
        let admitted = pending_path(temp.path());

        publish_new(temp.path(), &admitted, b"valid")?;

        assert!(admitted.exists());
        assert_eq!(fs::read_dir(&pending)?.count(), 3);
        assert_eq!(
            fs::read_dir(pending.join("quarantine"))?.count(),
            MAX_PENDING_PROPOSALS
        );
        let _guard = ProposalStoreLock::acquire(&pending)?;
        assert_eq!(pending_proposal_usage(&pending, None)?.count, 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_active_symlink_is_quarantined_without_following_target() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let pending = temp.path().join("pending");
        fs::create_dir_all(&pending)?;
        let outside = temp.path().join("outside");
        fs::write(&outside, b"private")?;
        let hostile = pending.join(non_utf8_active_name(0, b".json"));
        if let Err(error) = symlink(&outside, &hostile) {
            if error.raw_os_error() == Some(92) {
                return Ok(());
            }
            return Err(error.into());
        }

        let admitted = pending_path(temp.path());
        publish_new(temp.path(), &admitted, b"valid")?;

        assert_eq!(fs::read(&outside)?, b"private");
        assert!(fs::symlink_metadata(&hostile).is_err());
        let quarantined = fs::read_dir(pending.join("quarantine"))?
            .next()
            .transpose()?
            .context("non-UTF-8 symlink was quarantined")?;
        assert!(fs::symlink_metadata(quarantined.path())?
            .file_type()
            .is_symlink());
        Ok(())
    }
}
