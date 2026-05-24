use anyhow::{bail, Result};
use std::path::PathBuf;
use sysinfo::{Pid, Process, Signal, System};

/// Hardcoded cache directories eligible for clearing. Never uses glob expansion.
const USER_CACHE_DIRS: &[&str] = &["Library/Caches"];
const SYSTEM_CACHE_DIR: &str = "/Library/Caches";

#[derive(Debug, PartialEq, Eq)]
struct ProcessIdentity {
    name: String,
    start_time: u64,
    exe: Option<PathBuf>,
    cmd: Vec<String>,
}

impl ProcessIdentity {
    fn from_process(process: &Process) -> Self {
        Self {
            name: process.name().to_string(),
            start_time: process.start_time(),
            exe: process.exe().map(PathBuf::from),
            cmd: process.cmd().to_vec(),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self == other
    }
}

/// Kill a process by PID. Requires --confirm flag at the CLI layer.
/// Uses SIGTERM only (no SIGKILL in v1). Re-checks PID identity before signaling.
pub fn kill_by_pid(pid: u32, confirm: bool) -> Result<()> {
    if !confirm {
        bail!("Refusing to kill process {pid} without --confirm. Add --confirm to proceed.");
    }

    let mut sys = System::new_all();
    sys.refresh_all();

    let pid_key = Pid::from_u32(pid);
    let identity = sys
        .process(pid_key)
        .map(ProcessIdentity::from_process)
        .ok_or_else(|| anyhow::anyhow!("No process found with PID {pid}"))?;

    eprintln!("Sending SIGTERM to PID {pid} ({})...", identity.name);

    // Re-check identity immediately before signaling to avoid PID reuse mistakes
    sys.refresh_process(pid_key);
    let proc = sys
        .process(pid_key)
        .ok_or_else(|| anyhow::anyhow!("Process {pid} disappeared before signal could be sent"))?;

    let current_identity = ProcessIdentity::from_process(proc);
    if !identity.matches(&current_identity) {
        bail!(
            "PID {pid} identity changed ({} → {}). Refusing to signal to avoid PID reuse mistake.",
            identity.name,
            current_identity.name,
        );
    }

    let sent = proc.kill_with(Signal::Term);
    match sent {
        Some(true) => {
            println!("SIGTERM sent to PID {pid} ({}).", identity.name);
            Ok(())
        }
        Some(false) | None => {
            bail!(
                "Failed to send SIGTERM to PID {pid} ({}). Check permissions.",
                identity.name
            );
        }
    }
}

/// Clear user and system caches. Requires --confirm flag at the CLI layer.
/// Only removes contents of the hardcoded list above.
pub fn clear_caches(confirm: bool) -> Result<()> {
    if !confirm {
        bail!("Refusing to clear caches without --confirm. Add --confirm to proceed.");
    }

    let home = dirs_next::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    let mut cleared = 0usize;
    let mut errors = 0usize;

    for rel in USER_CACHE_DIRS {
        let path = home.join(rel);
        clear_directory_contents(&path, &mut cleared, &mut errors);
    }

    // System cache (may need elevated privs — best-effort)
    let sys_cache = PathBuf::from(SYSTEM_CACHE_DIR);
    clear_directory_contents(&sys_cache, &mut cleared, &mut errors);

    if errors > 0 {
        println!(
            "Cache clear: removed {cleared} item(s), {errors} error(s) (some may require elevated privileges)."
        );
    } else {
        println!("Cache clear: removed {cleared} item(s).");
    }

    Ok(())
}

fn clear_directory_contents(path: &PathBuf, cleared: &mut usize, errors: &mut usize) {
    if !path.exists() {
        return;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            *errors += 1;
            return;
        }
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        let metadata = match std::fs::symlink_metadata(&entry_path) {
            Ok(metadata) => metadata,
            Err(_) => {
                *errors += 1;
                continue;
            }
        };
        let result = if metadata.file_type().is_dir() {
            std::fs::remove_dir_all(&entry_path)
        } else {
            std::fs::remove_file(&entry_path)
        };
        match result {
            Ok(()) => *cleared += 1,
            Err(_) => *errors += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_identity_requires_more_than_name_to_match() {
        let first = ProcessIdentity {
            name: "worker".to_string(),
            start_time: 100,
            exe: Some(PathBuf::from("/bin/worker")),
            cmd: vec!["worker".to_string()],
        };
        let reused_pid = ProcessIdentity {
            name: "worker".to_string(),
            start_time: 200,
            exe: Some(PathBuf::from("/bin/worker")),
            cmd: vec!["worker".to_string()],
        };

        assert!(!first.matches(&reused_pid));
    }

    #[cfg(unix)]
    #[test]
    fn clear_directory_contents_removes_symlink_without_traversing_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "do not delete").unwrap();
        symlink(&outside, cache.join("outside-link")).unwrap();

        let mut cleared = 0;
        let mut errors = 0;
        clear_directory_contents(&cache, &mut cleared, &mut errors);

        assert_eq!(errors, 0);
        assert_eq!(cleared, 1);
        assert!(outside.join("keep.txt").exists());
        assert!(!cache.join("outside-link").exists());
    }
}
