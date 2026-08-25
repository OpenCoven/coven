//! Repository-wide maintenance exclusion for Coven writers.
//!
//! The files live below git's *common* directory, not a worktree's `.git`
//! link, so every worktree of one checkout observes the same state. Owner and
//! writer state remain fenced records, while short metadata mutations are
//! serialized by a cross-process advisory lock used by Coven processes. Cave
//! participates through Coven's CLI/API commands rather than implementing the
//! lock itself.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const GATE_DIR: &str = "coven-maintenance-gate";
const OWNER_FILE: &str = "owner";
const WRITERS_DIR: &str = "writers";
const LOCK_FILE: &str = "lock";
const LOCK_WAIT: Duration = Duration::from_secs(5);
const WRITER_TTL: Duration = Duration::from_secs(90);
const OWNER_TTL: Duration = Duration::from_secs(120);
const RENEW_EVERY: Duration = Duration::from_secs(20);
pub const MAINTENANCE_PARTICIPANT_ENV: &str = "COVEN_MAINTENANCE_PARTICIPANT";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Owner {
    pub owner_id: String,
    pub generation: String,
    pub expires_at: u64,
    pub phase: OwnerPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participant: Option<WriterParticipant>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerPhase {
    Draining,
    Held,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriterIntent {
    pub id: String,
    pub kind: String,
    pub generation: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriterParticipant {
    pub id: String,
    pub generation: String,
}

impl WriterParticipant {
    pub fn encode(&self) -> Result<String> {
        serde_json::to_string(self).context("failed to serialize maintenance participant")
    }

    pub fn decode(value: &str) -> Result<Self> {
        let participant: Self =
            serde_json::from_str(value).context("failed to parse maintenance participant")?;
        if participant.id.trim().is_empty() || participant.generation.trim().is_empty() {
            anyhow::bail!("maintenance participant must include non-blank id and generation");
        }
        Ok(participant)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateStatus {
    pub owner: Option<Owner>,
    pub writers: Vec<WriterIntent>,
}

#[derive(Debug)]
pub enum GateError {
    OwnerHeld(Owner),
    OwnerMalformed { path: PathBuf },
    WriterMalformed { path: PathBuf },
    OwnerChanged,
    LeaseExpired,
    Contended,
    ParticipantInvalid,
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OwnerHeld(owner) => write!(
                f,
                "repository maintenance is {} by {} until {}",
                match owner.phase {
                    OwnerPhase::Draining => "draining for",
                    OwnerPhase::Held => "held",
                },
                owner.owner_id,
                owner.expires_at
            ),
            Self::OwnerMalformed { path } => write!(
                f,
                "maintenance owner state is malformed at {}; refusing to proceed",
                path.display()
            ),
            Self::WriterMalformed { path } => write!(
                f,
                "maintenance writer state is malformed at {}; refusing to proceed",
                path.display()
            ),
            Self::OwnerChanged => write!(f, "maintenance ownership changed; acquire a new fence"),
            Self::LeaseExpired => write!(f, "maintenance lease expired; acquire a new fence"),
            Self::Contended => write!(f, "maintenance state is contended; retry"),
            Self::ParticipantInvalid => write!(
                f,
                "maintenance participant is stale, missing, or mismatched"
            ),
        }
    }
}

impl std::error::Error for GateError {}

#[derive(Debug, Clone)]
pub struct MaintenanceGate {
    common_dir: PathBuf,
}

/// Whether `project_root` could possibly sit inside a git repository.
///
/// Only ever used to skip work: a `false` result means no `.git` entry and no
/// bare-repository layout exists anywhere from `project_root` up to the
/// filesystem root, which `git rev-parse` would report as "not a git
/// repository" too. A `true` result decides nothing — `git` still runs and
/// still has the final say. Deliberately conservative, because a wrong `false`
/// would silently disable the maintenance fence:
///
/// - a `.git` file (worktree link) counts as much as a `.git` directory;
/// - a bare repository has no `.git` at all, so its `HEAD` + `objects` layout
///   counts too;
/// - `GIT_DIR` / `GIT_COMMON_DIR` can point anywhere, so their presence in the
///   environment skips this check entirely.
///
/// Nothing is cached. A `git init` between two launches is picked up by the
/// next one.
fn maybe_in_repository(project_root: &Path) -> bool {
    if std::env::var_os("GIT_DIR").is_some() || std::env::var_os("GIT_COMMON_DIR").is_some() {
        return true;
    }
    let mut current = Some(project_root);
    while let Some(directory) = current {
        if directory.join(".git").exists()
            || (directory.join("HEAD").exists() && directory.join("objects").is_dir())
        {
            return true;
        }
        current = directory.parent();
    }
    false
}

impl MaintenanceGate {
    /// Resolve the common directory for `project_root`. This accepts a
    /// worktree root as well as the primary checkout.
    pub fn discover(project_root: &Path) -> Result<Self> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .output()
            .context("failed to resolve git common directory")?;
        if !output.status.success() {
            anyhow::bail!(
                "repository maintenance gating requires a git worktree: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let common_dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let common_dir = fs::canonicalize(&common_dir)
            .with_context(|| format!("failed to canonicalize {}", common_dir.display()))?;
        Ok(Self { common_dir })
    }

    /// Session launches are also supported for ordinary directories. Those
    /// directories have no shared git identity, so there is no repository-wide
    /// exclusion domain to join; callers use this only for launch paths, never
    /// for claim or owner commands (which require a repository).
    pub fn discover_optional(project_root: &Path) -> Result<Option<Self>> {
        // Every session launch calls this, and spawning `git` costs more than
        // the rest of launch-to-first-output put together (~14 ms of the
        // ~37 ms floor on macOS, measured in bead coven-mwb). A directory with
        // no repository anywhere above it cannot be a worktree, and that check
        // is a handful of stats. Anything that might be a repository still
        // goes to `git`, which stays the authority.
        if !maybe_in_repository(project_root) {
            return Ok(None);
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .output()
            .context("failed to resolve git common directory")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // An ordinary directory has no common Git identity to coordinate
            // against. Other Git failures (for example, a broken worktree or
            // inaccessible common directory) must not silently disable the
            // maintenance fence.
            if stderr.contains("not a git repository") {
                return Ok(None);
            }
            anyhow::bail!(
                "failed to resolve repository maintenance gate: {}",
                stderr.trim()
            );
        }
        let common_dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        Ok(Some(Self {
            common_dir: fs::canonicalize(&common_dir)
                .with_context(|| format!("failed to canonicalize {}", common_dir.display()))?,
        }))
    }

    #[cfg(test)]
    pub(crate) fn at_for_test(common_dir: PathBuf) -> Self {
        Self { common_dir }
    }

    fn dir(&self) -> PathBuf {
        self.common_dir.join(GATE_DIR)
    }

    fn owner_path(&self) -> PathBuf {
        self.dir().join(OWNER_FILE)
    }

    fn writers_dir(&self) -> PathBuf {
        self.dir().join(WRITERS_DIR)
    }

    fn lock_path(&self) -> PathBuf {
        self.dir().join(LOCK_FILE)
    }

    fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(self.writers_dir()).with_context(|| {
            format!(
                "failed to create maintenance directory {}",
                self.dir().display()
            )
        })
    }

    /// Register a writer before a process can begin touching a repository.
    /// A returned lease renews itself until it is dropped, so an owner can
    /// distinguish a live session from a process that died mid-operation.
    pub fn acquire_writer(
        &self,
        id: impl Into<String>,
        kind: impl Into<String>,
    ) -> Result<WriterLease> {
        self.ensure_layout()?;
        let intent = WriterIntent {
            id: id.into(),
            kind: kind.into(),
            generation: Uuid::new_v4().to_string(),
            expires_at: unix_now() + WRITER_TTL.as_secs(),
        };
        let path = self.writers_dir().join(writer_file_name(&intent.id));
        self.with_lock(|| {
            if let Some(owner) = self.read_owner()? {
                if owner.expires_at > unix_now() {
                    return Err(GateError::OwnerHeld(owner).into());
                }
                fs::remove_file(self.owner_path())
                    .with_context(|| "failed to remove expired maintenance owner")?;
            }
            self.write_new(&path, &intent)
                .with_context(|| format!("failed to publish writer intent {}", path.display()))?;
            Ok(())
        })?;
        Ok(WriterLease::new(self.clone(), path, intent))
    }

    /// Acquire an owner fence. New writers are refused from the instant this
    /// record is published. Existing writers remain visible while Cave drains
    /// them; the phase becomes `held` only after they have all left.
    pub fn acquire_owner(
        &self,
        owner_id: impl Into<String>,
        participant: Option<WriterParticipant>,
    ) -> Result<OwnerLease> {
        self.ensure_layout()?;
        let owner = Owner {
            owner_id: owner_id.into(),
            generation: Uuid::new_v4().to_string(),
            expires_at: unix_now() + OWNER_TTL.as_secs(),
            phase: OwnerPhase::Draining,
            participant,
        };
        self.with_lock(|| {
            if let Some(existing) = self.read_owner()? {
                if existing.expires_at > unix_now() {
                    return Err(GateError::OwnerHeld(existing).into());
                }
                fs::remove_file(self.owner_path())
                    .with_context(|| "failed to remove expired maintenance owner")?;
            }
            if let Some(participant) = &owner.participant {
                self.validate_participant(participant, unix_now())?;
            }
            self.write_new(&self.owner_path(), &owner)
                .with_context(|| "failed to publish maintenance owner")?;
            Ok(())
        })?;
        let mut lease = OwnerLease {
            gate: self.clone(),
            owner,
        };
        lease.refresh_phase()?;
        Ok(lease)
    }

    pub fn status(&self) -> Result<GateStatus> {
        self.ensure_layout()?;
        self.with_lock(|| {
            let now = unix_now();
            let owner = self.read_owner()?;
            let mut writers = self.read_writers(now)?;
            if let Some(owner) = owner.as_ref() {
                writers = blocking_writers(owner, writers);
            }
            Ok(GateStatus { owner, writers })
        })
    }

    pub fn heartbeat_owner(&self, owner_id: &str, generation: &str) -> Result<GateStatus> {
        let owner = self.with_lock(|| {
            let owner = self.read_owner()?.ok_or(GateError::OwnerChanged)?;
            if owner.owner_id != owner_id || owner.generation != generation {
                return Err(GateError::OwnerChanged.into());
            }
            Ok(owner)
        })?;
        let mut lease = OwnerLease {
            gate: self.clone(),
            owner,
        };
        lease.refresh_phase()
    }

    pub fn release_owner(&self, owner_id: &str, generation: &str) -> Result<()> {
        OwnerLease {
            gate: self.clone(),
            owner: Owner {
                owner_id: owner_id.to_string(),
                generation: generation.to_string(),
                expires_at: 0,
                phase: OwnerPhase::Draining,
                participant: None,
            },
        }
        .release()
    }

    fn read_owner(&self) -> Result<Option<Owner>> {
        let path = self.owner_path();
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()))
            }
        };
        serde_json::from_slice(&data)
            .map(Some)
            .map_err(|_| GateError::OwnerMalformed { path }.into())
    }

    fn read_writers(&self, now: u64) -> Result<Vec<WriterIntent>> {
        let mut writers = Vec::new();
        for entry in fs::read_dir(self.writers_dir())? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() {
                return Err(GateError::WriterMalformed { path }.into());
            }
            let data = fs::read(&path)?;
            let writer: WriterIntent = serde_json::from_slice(&data)
                .map_err(|_| GateError::WriterMalformed { path: path.clone() })?;
            if writer.expires_at > now {
                writers.push(writer);
            } else {
                fs::remove_file(&path).with_context(|| {
                    format!("failed to remove expired writer {}", path.display())
                })?;
            }
        }
        writers.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(writers)
    }

    fn renew_writer(&self, path: &Path, generation: &str) -> Result<()> {
        self.with_lock(|| {
            let data = fs::read(path).with_context(|| "writer intent vanished")?;
            let mut intent: WriterIntent =
                serde_json::from_slice(&data).map_err(|_| GateError::WriterMalformed {
                    path: path.to_path_buf(),
                })?;
            if intent.generation != generation {
                return Err(GateError::OwnerChanged.into());
            }
            intent.expires_at = unix_now() + WRITER_TTL.as_secs();
            self.replace(path, &intent)
        })
    }

    fn validate_participant(&self, participant: &WriterParticipant, now: u64) -> Result<()> {
        let path = self.writers_dir().join(writer_file_name(&participant.id));
        let metadata = fs::symlink_metadata(&path).map_err(|_| GateError::ParticipantInvalid)?;
        if !metadata.file_type().is_file() {
            return Err(GateError::ParticipantInvalid.into());
        }
        let mut file = fs::File::open(&path).map_err(|_| GateError::ParticipantInvalid)?;
        let mut data = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
        file.read_to_end(&mut data)
            .map_err(|_| GateError::ParticipantInvalid)?;
        let writer: WriterIntent =
            serde_json::from_slice(&data).map_err(|_| GateError::ParticipantInvalid)?;
        if writer.id != participant.id
            || writer.generation != participant.generation
            || writer.expires_at <= now
        {
            return Err(GateError::ParticipantInvalid.into());
        }
        Ok(())
    }

    fn release_writer(&self, path: &Path, generation: &str) {
        let _ = self.with_lock(|| {
            let Ok(metadata) = fs::symlink_metadata(path) else {
                return Ok(());
            };
            if !metadata.file_type().is_file() {
                return Ok(());
            }
            let Ok(mut file) = fs::File::open(path) else {
                return Ok(());
            };
            let mut data = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
            if file.read_to_end(&mut data).is_err() {
                return Ok(());
            }
            let Ok(intent) = serde_json::from_slice::<WriterIntent>(&data) else {
                return Ok(());
            };
            if intent.generation == generation {
                let _ = fs::remove_file(path);
            }
            Ok(())
        });
    }

    fn with_lock<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        self.ensure_layout()?;
        let _lock = GateLock::acquire(self.lock_path())?;
        operation()
    }

    fn write_new<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn replace<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let staged = path.with_file_name(format!(
            "@{}.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            Uuid::new_v4()
        ));
        fs::write(&staged, serde_json::to_vec(value)?)?;
        replace_file(&staged, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(staged: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(staged, path)
}

#[cfg(windows)]
fn replace_file(staged: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    extern "system" {
        #[link_name = "MoveFileExW"]
        fn move_file_ex_w(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let existing = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let new = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe {
        move_file_ex_w(
            existing.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub struct WriterLease {
    gate: MaintenanceGate,
    path: PathBuf,
    participant: WriterParticipant,
    stopper: Arc<(Mutex<bool>, Condvar)>,
    renewer: Option<thread::JoinHandle<()>>,
}

impl WriterLease {
    fn new(gate: MaintenanceGate, path: PathBuf, intent: WriterIntent) -> Self {
        let stopper = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stopper = Arc::clone(&stopper);
        let thread_gate = gate.clone();
        let thread_path = path.clone();
        let participant = WriterParticipant {
            id: intent.id.clone(),
            generation: intent.generation.clone(),
        };
        let thread_participant = participant.clone();
        let renewer = thread::spawn(move || {
            loop {
                let (stopped, wake) = &*thread_stopper;
                let guard = stopped.lock().expect("writer lease stop lock poisoned");
                let (guard, _) = wake
                    .wait_timeout(guard, RENEW_EVERY)
                    .expect("writer lease wait poisoned");
                if *guard {
                    return;
                }
                drop(guard);
                if thread_gate
                    .renew_writer(&thread_path, &thread_participant.generation)
                    .is_err()
                {
                    // Keep trying: a transient rename failure must not make a
                    // still-running session invisible to a maintenance owner.
                }
            }
        });
        Self {
            gate,
            path,
            participant,
            stopper,
            renewer: Some(renewer),
        }
    }

    pub fn participant(&self) -> &WriterParticipant {
        &self.participant
    }

    pub fn participant_capability(&self) -> Result<String> {
        self.participant().encode()
    }
}

impl Drop for WriterLease {
    fn drop(&mut self) {
        let (stopped, wake) = &*self.stopper;
        if let Ok(mut stopped) = stopped.lock() {
            *stopped = true;
            wake.notify_one();
        }
        if let Some(renewer) = self.renewer.take() {
            let _ = renewer.join();
        }
        self.gate
            .release_writer(&self.path, &self.participant.generation);
    }
}

#[derive(Debug)]
pub struct OwnerLease {
    gate: MaintenanceGate,
    owner: Owner,
}

impl OwnerLease {
    #[cfg(test)]
    pub fn owner(&self) -> &Owner {
        &self.owner
    }

    pub fn refresh_phase(&mut self) -> Result<GateStatus> {
        self.gate.with_lock(|| {
            let current = self.gate.read_owner()?.ok_or(GateError::OwnerChanged)?;
            if current.generation != self.owner.generation
                || current.owner_id != self.owner.owner_id
            {
                return Err(GateError::OwnerChanged.into());
            }
            if current.expires_at <= unix_now() {
                return Err(GateError::LeaseExpired.into());
            }
            let writers = self.gate.read_writers(unix_now())?;
            let blocking_writers = blocking_writers(&current, writers);
            let mut next = current;
            next.expires_at = unix_now() + OWNER_TTL.as_secs();
            next.phase = if blocking_writers.is_empty() {
                OwnerPhase::Held
            } else {
                OwnerPhase::Draining
            };
            self.gate.replace(&self.gate.owner_path(), &next)?;
            self.owner = next.clone();
            Ok(GateStatus {
                owner: Some(next),
                writers: blocking_writers,
            })
        })
    }

    #[cfg(test)]
    pub fn assert_held(&self) -> Result<()> {
        let current = self.gate.read_owner()?.ok_or(GateError::OwnerChanged)?;
        if current.generation != self.owner.generation || current.owner_id != self.owner.owner_id {
            return Err(GateError::OwnerChanged.into());
        }
        if current.expires_at <= unix_now() {
            return Err(GateError::LeaseExpired.into());
        }
        if current.phase != OwnerPhase::Held {
            anyhow::bail!("maintenance owner is still draining writers")
        }
        Ok(())
    }

    pub fn release(self) -> Result<()> {
        self.gate.with_lock(|| {
            let current = self.gate.read_owner()?.ok_or(GateError::OwnerChanged)?;
            if current.generation != self.owner.generation
                || current.owner_id != self.owner.owner_id
            {
                return Err(GateError::OwnerChanged.into());
            }
            if current.expires_at <= unix_now() {
                return Err(GateError::LeaseExpired.into());
            }
            fs::remove_file(self.gate.owner_path())
                .context("failed to release maintenance owner")?;
            Ok(())
        })
    }
}

#[derive(Debug)]
struct GateLock {
    file: fs::File,
}

impl GateLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        Self::acquire_with_wait(path, LOCK_WAIT)
    }

    fn acquire_with_wait(path: PathBuf, wait: Duration) -> Result<Self> {
        let file = crate::state_lock::open_lock_file(&path)
            .with_context(|| format!("failed to open maintenance lock {}", path.display()))?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if crate::state_lock::is_lock_contended(&error) => {
                    if started.elapsed() >= wait {
                        return Err(GateError::Contended.into());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to acquire {}", path.display()));
                }
            }
        }
    }
}

impl Drop for GateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn writer_file_name(id: &str) -> String {
    format!(
        "{}.json",
        id.chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            })
            .collect::<String>()
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn blocking_writers(owner: &Owner, writers: Vec<WriterIntent>) -> Vec<WriterIntent> {
    writers
        .into_iter()
        .filter(|writer| {
            owner.participant.as_ref().is_none_or(|participant| {
                writer.id != participant.id || writer.generation != participant.generation
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn plain_directory_has_no_gate_and_never_consults_git() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(!maybe_in_repository(temp.path()));
        assert!(MaintenanceGate::discover_optional(temp.path())?.is_none());
        Ok(())
    }

    #[test]
    fn repository_layouts_still_reach_git() -> Result<()> {
        // Each of these must fall through to `git`, because treating one as a
        // plain directory would silently drop the maintenance fence.
        let worktree_dir = tempfile::tempdir()?;
        std::fs::create_dir(worktree_dir.path().join(".git"))?;
        assert!(maybe_in_repository(worktree_dir.path()));

        let linked = tempfile::tempdir()?;
        std::fs::write(
            linked.path().join(".git"),
            b"gitdir: /elsewhere/.git/worktrees/w",
        )?;
        assert!(maybe_in_repository(linked.path()));

        let nested = tempfile::tempdir()?;
        std::fs::create_dir(nested.path().join(".git"))?;
        let deep = nested.path().join("crates/coven-cli/src");
        std::fs::create_dir_all(&deep)?;
        assert!(maybe_in_repository(&deep));

        let bare = tempfile::tempdir()?;
        std::fs::write(bare.path().join("HEAD"), b"ref: refs/heads/main\n")?;
        std::fs::create_dir(bare.path().join("objects"))?;
        assert!(maybe_in_repository(bare.path()));
        Ok(())
    }

    #[test]
    fn a_repository_created_after_a_launch_is_seen_by_the_next_one() -> Result<()> {
        // Nothing is cached, so the fence cannot be disabled for the lifetime
        // of a daemon by an early miss.
        let temp = tempfile::tempdir()?;
        assert!(!maybe_in_repository(temp.path()));
        std::fs::create_dir(temp.path().join(".git"))?;
        assert!(maybe_in_repository(temp.path()));
        Ok(())
    }

    fn assert_contended(error: &anyhow::Error) {
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|error| matches!(error, GateError::Contended)));
    }

    #[test]
    fn live_gate_lock_blocks_a_second_acquirer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        gate.ensure_layout()?;
        let _first = GateLock::acquire(gate.lock_path())?;

        let error = GateLock::acquire_with_wait(gate.lock_path(), Duration::ZERO).unwrap_err();

        assert_contended(&error);
        Ok(())
    }

    #[test]
    fn dropping_gate_lock_allows_the_next_acquirer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        gate.ensure_layout()?;
        let first = GateLock::acquire(gate.lock_path())?;
        drop(first);

        let _second = GateLock::acquire_with_wait(gate.lock_path(), Duration::ZERO)?;

        Ok(())
    }

    #[test]
    fn stale_mtime_does_not_allow_live_gate_lock_takeover() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        gate.ensure_layout()?;
        let _first = GateLock::acquire(gate.lock_path())?;
        let lock_file = fs::OpenOptions::new().write(true).open(gate.lock_path())?;
        lock_file.set_times(
            FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(120)),
        )?;

        let error = GateLock::acquire_with_wait(gate.lock_path(), Duration::ZERO).unwrap_err();

        assert_contended(&error);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_gate_lock_is_refused_without_touching_the_target() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        gate.ensure_layout()?;
        let outside = tempfile::NamedTempFile::new()?;
        fs::write(outside.path(), b"outside-lock-target")?;
        symlink(outside.path(), gate.lock_path())?;

        let error = GateLock::acquire(gate.lock_path()).expect_err("symlinked gate lock must fail");

        assert!(format!("{error:#}").contains(&gate.lock_path().display().to_string()));
        assert_eq!(fs::read(outside.path())?, b"outside-lock-target");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn multiply_linked_gate_lock_is_refused() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        gate.ensure_layout()?;
        fs::write(gate.lock_path(), b"lock")?;
        let alias = temp.path().join("lock-alias");
        fs::hard_link(gate.lock_path(), &alias)?;

        let error =
            GateLock::acquire(gate.lock_path()).expect_err("multiply linked gate lock must fail");

        assert!(format!("{error:#}").contains(&gate.lock_path().display().to_string()));
        Ok(())
    }

    #[test]
    fn owner_rejects_a_writer_until_release() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        let owner = gate.acquire_owner("cave", None)?;
        assert_eq!(owner.owner().phase, OwnerPhase::Held);
        let error = gate.acquire_writer("session-1", "session").unwrap_err();
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|e| matches!(e, GateError::OwnerHeld(_))));
        owner.release()?;
        drop(gate.acquire_writer("session-1", "session")?);
        Ok(())
    }

    #[test]
    fn owner_drains_existing_writer_then_becomes_held() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        let writer = gate.acquire_writer("session-1", "session")?;
        let mut owner = gate.acquire_owner("cave", None)?;
        assert_eq!(owner.owner().phase, OwnerPhase::Draining);
        drop(writer);
        assert!(owner.refresh_phase()?.writers.is_empty());
        owner.assert_held()?;
        owner.release()?;
        Ok(())
    }

    #[test]
    fn owner_excludes_only_its_exact_participant_writer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        let writer_self = gate.acquire_writer("session-self", "session")?;
        let writer_other = gate.acquire_writer("session-other", "session")?;
        let participant = writer_self.participant().clone();
        let mut owner = gate.acquire_owner("cave", Some(participant))?;

        let status = owner.refresh_phase()?;
        assert_eq!(status.writers.len(), 1);
        assert_eq!(status.writers[0].id, "session-other");
        assert_eq!(owner.owner().phase, OwnerPhase::Draining);

        drop(writer_other);
        assert!(owner.refresh_phase()?.writers.is_empty());
        owner.assert_held()?;
        owner.release()?;
        Ok(())
    }

    #[test]
    fn owner_rejects_a_stale_or_forged_participant_generation() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        let writer = gate.acquire_writer("session-self", "session")?;
        let mut participant = writer.participant().clone();
        participant.generation.push_str("-forged");

        let error = gate
            .acquire_owner("cave", Some(participant))
            .expect_err("forged participant must be rejected");
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|e| matches!(e, GateError::ParticipantInvalid)));
        assert!(gate.read_owner()?.is_none());
        Ok(())
    }

    #[test]
    fn owner_does_not_exclude_a_replacement_writer_with_the_same_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        let first_writer = gate.acquire_writer("session-self", "session")?;
        let participant = first_writer.participant().clone();
        drop(first_writer);

        let replacement_writer = gate.acquire_writer("session-self", "session")?;
        let error = gate
            .acquire_owner("cave", Some(participant))
            .expect_err("stale participant must be rejected");
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|e| matches!(e, GateError::ParticipantInvalid)));
        assert!(gate.read_owner()?.is_none());
        drop(replacement_writer);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn owner_rejects_symlinked_participant_writer_without_touching_target() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        let writer = gate.acquire_writer("session-self", "session")?;
        let participant = writer.participant().clone();
        let writer_path = gate.writers_dir().join(writer_file_name(&participant.id));
        let external_path = temp.path().join("external-writer.json");
        let external_writer = WriterIntent {
            id: participant.id.clone(),
            kind: "session".into(),
            generation: participant.generation.clone(),
            expires_at: unix_now() + WRITER_TTL.as_secs(),
        };
        fs::write(&external_path, serde_json::to_vec(&external_writer)?)?;
        std::mem::forget(writer);
        fs::remove_file(&writer_path)?;
        symlink(&external_path, &writer_path)?;

        let before = fs::read(&external_path)?;
        let error = gate
            .acquire_owner("cave", Some(participant))
            .expect_err("symlinked participant writer must be rejected");
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|e| matches!(e, GateError::ParticipantInvalid)));
        assert_eq!(fs::read(&external_path)?, before);
        assert!(gate.read_owner()?.is_none());
        Ok(())
    }

    #[test]
    fn owner_rejects_non_regular_participant_writer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        gate.ensure_layout()?;
        let participant = WriterParticipant {
            id: "session-self".into(),
            generation: "gen-1".into(),
        };
        let writer_path = gate.writers_dir().join(writer_file_name(&participant.id));
        fs::create_dir(&writer_path)?;

        let error = gate
            .acquire_owner("cave", Some(participant))
            .expect_err("directory participant writer must be rejected");
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|e| matches!(e, GateError::ParticipantInvalid)));
        assert!(gate.read_owner()?.is_none());
        Ok(())
    }

    #[test]
    fn owner_without_participant_remains_backward_compatible() -> Result<()> {
        let owner: Owner = serde_json::from_str(
            r#"{"owner_id":"cave","generation":"g1","expires_at":123,"phase":"held"}"#,
        )?;
        assert_eq!(owner.participant, None);
        Ok(())
    }

    #[test]
    fn writer_participant_encode_decode_round_trips_and_rejects_blank_fields() -> Result<()> {
        let participant = WriterParticipant {
            id: "session-self".into(),
            generation: "gen-1".into(),
        };
        let encoded = participant.encode()?;
        assert_eq!(&WriterParticipant::decode(&encoded)?, &participant);
        assert!(WriterParticipant::decode(r#"{"id":" ","generation":"gen"}"#).is_err());
        assert!(WriterParticipant::decode(r#"{"id":"session","generation":" "}"#).is_err());
        Ok(())
    }

    #[test]
    fn writer_lease_participant_capability_uses_public_env_name() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        let writer = gate.acquire_writer("session-self", "session")?;
        let capability = writer.participant_capability()?;
        let decoded = WriterParticipant::decode(&capability)?;
        assert_eq!(decoded, *writer.participant());
        assert_eq!(MAINTENANCE_PARTICIPANT_ENV, "COVEN_MAINTENANCE_PARTICIPANT");
        Ok(())
    }

    #[test]
    fn malformed_owner_fails_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at_for_test(temp.path().to_path_buf());
        gate.ensure_layout()?;
        fs::write(gate.owner_path(), b"not-json")?;
        let error = gate.acquire_writer("session-1", "session").unwrap_err();
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|e| matches!(e, GateError::OwnerMalformed { .. })));
        Ok(())
    }
}
