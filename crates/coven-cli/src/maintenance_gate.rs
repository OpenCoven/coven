//! Repository-wide maintenance exclusion for Coven writers.
//!
//! The files live below git's *common* directory, not a worktree's `.git`
//! link, so every worktree of one checkout observes the same state.  This is
//! deliberately a protocol built from exclusive file creation and fenced
//! records rather than an advisory process lock: Cave and the CLI are separate
//! processes and must make the same decision before they mutate a repository.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const GATE_DIR: &str = "coven-maintenance-gate";
const OWNER_FILE: &str = "owner";
const WRITERS_DIR: &str = "writers";
const LOCK_FILE: &str = "lock";
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const LOCK_WAIT: Duration = Duration::from_secs(5);
const WRITER_TTL: Duration = Duration::from_secs(90);
const OWNER_TTL: Duration = Duration::from_secs(120);
const RENEW_EVERY: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Owner {
    pub owner_id: String,
    pub generation: String,
    pub expires_at: u64,
    pub phase: OwnerPhase,
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
        }
    }
}

impl std::error::Error for GateError {}

#[derive(Debug, Clone)]
pub struct MaintenanceGate {
    common_dir: PathBuf,
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
    fn at(common_dir: PathBuf) -> Self {
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
    pub fn acquire_owner(&self, owner_id: impl Into<String>) -> Result<OwnerLease> {
        self.ensure_layout()?;
        let owner = Owner {
            owner_id: owner_id.into(),
            generation: Uuid::new_v4().to_string(),
            expires_at: unix_now() + OWNER_TTL.as_secs(),
            phase: OwnerPhase::Draining,
        };
        self.with_lock(|| {
            if let Some(existing) = self.read_owner()? {
                if existing.expires_at > unix_now() {
                    return Err(GateError::OwnerHeld(existing).into());
                }
                fs::remove_file(self.owner_path())
                    .with_context(|| "failed to remove expired maintenance owner")?;
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
            let writers = self.read_writers(now)?;
            Ok(GateStatus { owner, writers })
        })
    }

    pub fn heartbeat_owner(&self, owner_id: &str, generation: &str) -> Result<GateStatus> {
        let mut lease = OwnerLease {
            gate: self.clone(),
            owner: Owner {
                owner_id: owner_id.to_string(),
                generation: generation.to_string(),
                expires_at: 0,
                phase: OwnerPhase::Draining,
            },
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

    fn release_writer(&self, path: &Path, generation: &str) {
        let _ = self.with_lock(|| {
            let Ok(data) = fs::read(path) else {
                return Ok(());
            };
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
    generation: String,
    stopper: Arc<(Mutex<bool>, Condvar)>,
    renewer: Option<thread::JoinHandle<()>>,
}

impl WriterLease {
    fn new(gate: MaintenanceGate, path: PathBuf, intent: WriterIntent) -> Self {
        let stopper = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stopper = Arc::clone(&stopper);
        let thread_gate = gate.clone();
        let thread_path = path.clone();
        let generation = intent.generation.clone();
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
                if thread_gate.renew_writer(&thread_path, &generation).is_err() {
                    // Keep trying: a transient rename failure must not make a
                    // still-running session invisible to a maintenance owner.
                }
            }
        });
        Self {
            gate,
            path,
            generation: intent.generation,
            stopper,
            renewer: Some(renewer),
        }
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
        self.gate.release_writer(&self.path, &self.generation);
    }
}

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
            let mut next = current;
            next.expires_at = unix_now() + OWNER_TTL.as_secs();
            next.phase = if writers.is_empty() {
                OwnerPhase::Held
            } else {
                OwnerPhase::Draining
            };
            self.gate.replace(&self.gate.owner_path(), &next)?;
            self.owner = next.clone();
            Ok(GateStatus {
                owner: Some(next),
                writers,
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

struct GateLock {
    path: PathBuf,
}

impl GateLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        let started = SystemTime::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_stale(&path, LOCK_STALE_AFTER) {
                        let _ = fs::remove_file(&path);
                    }
                    if SystemTime::now()
                        .duration_since(started)
                        .unwrap_or_default()
                        > LOCK_WAIT
                    {
                        return Err(GateError::Contended.into());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to acquire {}", path.display()))
                }
            }
        }
    }
}

impl Drop for GateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path, after: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > after)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_rejects_a_writer_until_release() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at(temp.path().to_path_buf());
        let owner = gate.acquire_owner("cave")?;
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
        let gate = MaintenanceGate::at(temp.path().to_path_buf());
        let writer = gate.acquire_writer("session-1", "session")?;
        let mut owner = gate.acquire_owner("cave")?;
        assert_eq!(owner.owner().phase, OwnerPhase::Draining);
        drop(writer);
        assert!(owner.refresh_phase()?.writers.is_empty());
        owner.assert_held()?;
        owner.release()?;
        Ok(())
    }

    #[test]
    fn malformed_owner_fails_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let gate = MaintenanceGate::at(temp.path().to_path_buf());
        gate.ensure_layout()?;
        fs::write(gate.owner_path(), b"not-json")?;
        let error = gate.acquire_writer("session-1", "session").unwrap_err();
        assert!(error
            .downcast_ref::<GateError>()
            .is_some_and(|e| matches!(e, GateError::OwnerMalformed { .. })));
        Ok(())
    }
}
