//! Per-project store of chat conversation ids so a fresh `coven chat`
//! invocation can resume the prior conversation with each harness.
//!
//! On-disk form is a small JSON document at
//! `<coven_home>/chat-conversations/<project-key>.json`. The project key is a
//! deterministic FNV-1a hash of the canonicalized project root, hex-encoded;
//! changing the project root means a new file (which is the right behavior —
//! different projects shouldn't share a thread). See
//! `docs/chat-persistence.md` for the broader design.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DIR_NAME: &str = "chat-conversations";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredConversations {
    /// Display copy of the project root that produced this file's key.
    /// Stored for human inspection / collision debugging — not used to look
    /// the file back up.
    #[serde(default)]
    pub(super) project_root: String,
    /// Harness id → conversation id (e.g. `"claude" → "<uuid>"`).
    #[serde(default)]
    pub(super) conversations: HashMap<String, String>,
}

pub(super) fn conversations_dir(coven_home: &Path) -> PathBuf {
    coven_home.join(DIR_NAME)
}

pub(super) fn conversations_file(coven_home: &Path, project_root: &Path) -> PathBuf {
    conversations_dir(coven_home).join(format!("{}.json", project_key(project_root)))
}

fn temporary_conversations_file(coven_home: &Path, project_root: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    conversations_dir(coven_home).join(format!(
        ".{}.json.{}.{}.tmp",
        project_key(project_root),
        std::process::id(),
        nanos
    ))
}

/// Load stored conversation ids for `project_root`, falling back to an empty
/// set on any error (missing file, corrupt JSON, permissions). Conversation
/// state is best-effort — a failure to read must not block the chat from
/// coming up.
pub(super) fn load_for_project(coven_home: &Path, project_root: &Path) -> HashMap<String, String> {
    let path = conversations_file(coven_home, project_root);
    let Ok(data) = std::fs::read(&path) else {
        return HashMap::new();
    };
    serde_json::from_slice::<StoredConversations>(&data)
        .map(|stored| stored.conversations)
        .unwrap_or_default()
}

/// Persist `conversations` for `project_root` atomically. Returns the
/// underlying io error so callers can log it; chat should *not* abort the
/// turn on a persistence failure (the in-memory state is still authoritative).
pub(super) fn save_for_project(
    coven_home: &Path,
    project_root: &Path,
    conversations: &HashMap<String, String>,
) -> std::io::Result<()> {
    let dir = conversations_dir(coven_home);
    std::fs::create_dir_all(&dir)?;
    let stored = StoredConversations {
        project_root: project_root.to_string_lossy().into_owned(),
        conversations: conversations.clone(),
    };
    let body = serde_json::to_vec_pretty(&stored)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let path = conversations_file(coven_home, project_root);
    let temp_path = temporary_conversations_file(coven_home, project_root);
    std::fs::write(&temp_path, body)?;
    replace_conversations_file(&temp_path, &path)?;
    if let Ok(handle) = std::fs::File::open(&dir) {
        let _ = handle.sync_all();
    }
    Ok(())
}

/// Remove the stored conversations file for `project_root`. Missing-file is
/// treated as success (the post-condition is "no file"). Other io errors are
/// returned for logging — callers should not abort.
pub(super) fn clear_for_project(coven_home: &Path, project_root: &Path) -> std::io::Result<()> {
    let path = conversations_file(coven_home, project_root);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// FNV-1a 64-bit hash of the canonical project root, hex-encoded. Stable
/// across Rust versions and machines (unlike `std::collections::hash_map`'s
/// SipHash, which Rust may reseed). 16 hex chars is plenty of namespace for
/// per-user storage and short enough to type if you ever need to inspect a
/// file by hand.
fn project_key(project_root: &Path) -> String {
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let bytes = canonical.as_os_str().as_encoded_bytes();
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(not(windows))]
fn replace_conversations_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(temp_path, path)
}

#[cfg(windows)]
fn replace_conversations_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "Kernel32")]
    extern "system" {
        #[link_name = "MoveFileExW"]
        fn move_file_ex_w(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let existing: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let new: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        move_file_ex_w(
            existing.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_empty_when_file_missing() {
        let temp = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let loaded = load_for_project(temp.path(), project.path());
        assert!(loaded.is_empty());
    }

    #[test]
    fn save_then_load_round_trips_conversation_ids() {
        let temp = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let mut conversations = HashMap::new();
        conversations.insert("claude".to_string(), "claude-uuid".to_string());
        conversations.insert("codex".to_string(), "codex-uuid".to_string());

        save_for_project(temp.path(), project.path(), &conversations).expect("save");
        let loaded = load_for_project(temp.path(), project.path());

        assert_eq!(loaded, conversations);
    }

    #[test]
    fn corrupt_conversations_file_falls_back_to_empty() {
        let temp = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let dir = conversations_dir(temp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            conversations_file(temp.path(), project.path()),
            b"{ not json",
        )
        .unwrap();

        let loaded = load_for_project(temp.path(), project.path());
        assert!(loaded.is_empty());
    }

    #[test]
    fn save_overwrites_existing_conversations_file() {
        let temp = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let mut first = HashMap::new();
        first.insert("claude".to_string(), "old-uuid".to_string());
        save_for_project(temp.path(), project.path(), &first).expect("first save");

        let mut second = HashMap::new();
        second.insert("claude".to_string(), "new-uuid".to_string());
        save_for_project(temp.path(), project.path(), &second).expect("second save");

        assert_eq!(load_for_project(temp.path(), project.path()), second);
    }

    #[test]
    fn clear_removes_existing_conversations_file_and_treats_missing_as_success() {
        let temp = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let mut conversations = HashMap::new();
        conversations.insert("claude".to_string(), "claude-uuid".to_string());
        save_for_project(temp.path(), project.path(), &conversations).expect("save");

        clear_for_project(temp.path(), project.path()).expect("clear existing");
        assert!(!conversations_file(temp.path(), project.path()).exists());

        // Second clear on an already-empty store must not error.
        clear_for_project(temp.path(), project.path()).expect("clear missing");
    }

    #[test]
    fn different_projects_get_different_files() {
        let temp = tempfile::tempdir().unwrap();
        let project_a = tempfile::tempdir().unwrap();
        let project_b = tempfile::tempdir().unwrap();
        let mut conversations_a = HashMap::new();
        conversations_a.insert("claude".to_string(), "a-uuid".to_string());
        let mut conversations_b = HashMap::new();
        conversations_b.insert("claude".to_string(), "b-uuid".to_string());

        save_for_project(temp.path(), project_a.path(), &conversations_a).expect("save a");
        save_for_project(temp.path(), project_b.path(), &conversations_b).expect("save b");

        assert_eq!(
            load_for_project(temp.path(), project_a.path()),
            conversations_a
        );
        assert_eq!(
            load_for_project(temp.path(), project_b.path()),
            conversations_b
        );
    }

    #[test]
    fn project_key_is_deterministic() {
        let project = tempfile::tempdir().unwrap();
        assert_eq!(project_key(project.path()), project_key(project.path()));
    }

    #[test]
    fn project_key_changes_with_path() {
        let project_a = tempfile::tempdir().unwrap();
        let project_b = tempfile::tempdir().unwrap();
        assert_ne!(project_key(project_a.path()), project_key(project_b.path()));
    }

    #[test]
    fn temporary_conversations_file_stays_in_chat_conversations_dir() {
        let temp = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let temp_path = temporary_conversations_file(temp.path(), project.path());
        assert_eq!(
            temp_path.parent(),
            Some(conversations_dir(temp.path()).as_path())
        );
        assert_ne!(temp_path, conversations_file(temp.path(), project.path()));
    }
}
