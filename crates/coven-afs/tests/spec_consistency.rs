//! Integration tests exercising the AgentFS SPEC v0.4 contract:
//! filesystem consistency rules, chunking invariants, overlay lookup
//! semantics, whiteout lifecycle, origin tracking, KV, and the tool-call
//! audit rules.

use coven_afs::{AgentFs, OverlayFs, ROOT_INO, S_IFDIR, S_IFMT, S_IFREG};
use serde_json::json;

fn assert_consistent(fs: &AgentFs) {
    let violations = fs.check_consistency().expect("consistency query");
    assert!(violations.is_empty(), "violations: {violations:?}");
}

// ---- initialization ------------------------------------------------------

#[test]
fn fresh_filesystem_satisfies_spec_init() {
    let fs = AgentFs::in_memory().unwrap();
    // Rule 1: root inode (ino=1) exists, mode 0o040755 = 16877, nlink=1.
    let root = fs.stat_ino(ROOT_INO).unwrap();
    assert_eq!(root.mode, 16877);
    assert_eq!(root.mode & S_IFMT, S_IFDIR);
    assert_eq!(root.nlink, 1);
    assert_eq!(fs.chunk_size(), 4096);
    assert_consistent(&fs);
}

#[test]
fn reopening_keeps_immutable_chunk_size() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("agent.db");
    {
        AgentFs::create_with_chunk_size(&db, 512).unwrap();
    }
    // A different requested size on reopen must not change the stored config.
    let fs = AgentFs::create_with_chunk_size(&db, 4096).unwrap();
    assert_eq!(fs.chunk_size(), 512);
}

// ---- files + chunking ----------------------------------------------------

#[test]
fn write_read_roundtrip_across_chunk_boundaries() {
    let mut fs = AgentFs::in_memory_with_chunk_size(8).unwrap();
    for len in [0usize, 1, 7, 8, 9, 16, 17, 100] {
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        fs.write_file("/f", &data).unwrap();
        assert_eq!(fs.read_file("/f").unwrap(), data, "len {len}");
        assert_eq!(fs.stat("/f").unwrap().size, len as i64);
        assert_consistent(&fs);
    }
}

#[test]
fn empty_file_has_inode_but_no_chunks() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.write_file("/empty", b"").unwrap();
    let meta = fs.stat("/empty").unwrap();
    assert_eq!(meta.size, 0);
    assert_eq!(meta.mode & S_IFMT, S_IFREG);
    assert_eq!(fs.read_file("/empty").unwrap(), Vec::<u8>::new());
    assert_consistent(&fs);
}

#[test]
fn read_at_offsets() {
    let mut fs = AgentFs::in_memory_with_chunk_size(4).unwrap();
    fs.write_file("/f", b"0123456789").unwrap();
    assert_eq!(fs.read_at("/f", 0, 4).unwrap(), b"0123");
    assert_eq!(fs.read_at("/f", 3, 4).unwrap(), b"3456"); // spans chunks
    assert_eq!(fs.read_at("/f", 8, 10).unwrap(), b"89"); // truncated at EOF
    assert_eq!(fs.read_at("/f", 10, 4).unwrap(), b""); // at EOF
    assert_eq!(fs.read_at("/f", 99, 4).unwrap(), b""); // past EOF
}

#[test]
fn overwrite_replaces_chunks() {
    let mut fs = AgentFs::in_memory_with_chunk_size(4).unwrap();
    fs.write_file("/f", b"a longer initial payload").unwrap();
    fs.write_file("/f", b"tiny").unwrap();
    assert_eq!(fs.read_file("/f").unwrap(), b"tiny");
    assert_consistent(&fs); // rule 7 catches stale chunks
}

#[test]
fn implicit_parent_directories() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.write_file("/a/b/c/file.txt", b"x").unwrap();
    assert!(fs.stat("/a").unwrap().is_dir());
    assert!(fs.stat("/a/b/c").unwrap().is_dir());
    assert_eq!(fs.readdir("/a/b/c").unwrap(), vec!["file.txt"]);
    assert_consistent(&fs);
}

// ---- delete / rename / links ----------------------------------------------

#[test]
fn delete_removes_inode_and_data_on_last_link() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.write_file("/f", b"data").unwrap();
    fs.remove_file("/f").unwrap();
    assert!(!fs.exists("/f").unwrap());
    assert_consistent(&fs); // rule 8 catches orphan inodes
}

#[test]
fn hard_links_share_inode_and_survive_source_deletion() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.write_file("/f", b"shared").unwrap();
    fs.hardlink("/f", "/g").unwrap();
    let (fa, fb) = (fs.stat("/f").unwrap(), fs.stat("/g").unwrap());
    assert_eq!(fa.ino, fb.ino);
    assert_eq!(fa.nlink, 2);
    fs.remove_file("/f").unwrap();
    assert_eq!(fs.read_file("/g").unwrap(), b"shared");
    assert_eq!(fs.stat("/g").unwrap().nlink, 1);
    assert_consistent(&fs);
}

#[test]
fn rename_moves_dentry_and_replaces_destination() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.write_file("/a/src.txt", b"payload").unwrap();
    fs.write_file("/b/dst.txt", b"old").unwrap();
    fs.rename("/a/src.txt", "/b/dst.txt").unwrap();
    assert!(!fs.exists("/a/src.txt").unwrap());
    assert_eq!(fs.read_file("/b/dst.txt").unwrap(), b"payload");
    assert_consistent(&fs);
}

#[test]
fn rename_rejects_moving_dir_into_itself() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.mkdir_p("/a/b").unwrap();
    assert!(fs.rename("/a", "/a/b/c").is_err());
}

#[test]
fn symlink_roundtrip() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.write_file("/target", b"t").unwrap();
    fs.symlink("/target", "/link").unwrap();
    let meta = fs.stat("/link").unwrap();
    assert!(meta.is_symlink());
    assert_eq!(meta.size, "/target".len() as i64);
    assert_eq!(fs.read_link("/link").unwrap(), "/target");
    fs.remove_file("/link").unwrap();
    assert!(fs.exists("/target").unwrap());
    assert_consistent(&fs);
}

#[test]
fn remove_dir_requires_empty() {
    let mut fs = AgentFs::in_memory().unwrap();
    fs.write_file("/d/f", b"x").unwrap();
    assert!(fs.remove_dir("/d").is_err());
    fs.remove_file("/d/f").unwrap();
    fs.remove_dir("/d").unwrap();
    assert!(!fs.exists("/d").unwrap());
    assert_consistent(&fs);
}

// ---- overlay ---------------------------------------------------------------

fn overlay_fixture() -> (tempfile::TempDir, OverlayFs) {
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.db");
    {
        let mut base = AgentFs::create(&base_path).unwrap();
        base.write_file("/shared.txt", b"from base").unwrap();
        base.write_file("/docs/readme.md", b"# base readme")
            .unwrap();
        base.write_file("/docs/guide.md", b"guide").unwrap();
    }
    let overlay = OverlayFs::open(dir.path().join("delta.db"), &base_path).unwrap();
    (dir, overlay)
}

#[test]
fn overlay_lookup_order_delta_whiteout_base() {
    let (_dir, mut ov) = overlay_fixture();
    // Base visible through empty delta.
    assert_eq!(ov.read_file("/shared.txt").unwrap(), b"from base");
    // Delta shadows base.
    ov.write_file("/shared.txt", b"from delta").unwrap();
    assert_eq!(ov.read_file("/shared.txt").unwrap(), b"from delta");
    // Whiteout hides base (delete removes delta copy AND whiteouts base).
    ov.remove_file("/shared.txt").unwrap();
    assert!(!ov.exists("/shared.txt").unwrap());
    assert!(ov.read_file("/shared.txt").is_err());
    // Base layer itself is untouched.
    assert_eq!(ov.base().read_file("/shared.txt").unwrap(), b"from base");
}

#[test]
fn whiteout_removed_when_file_recreated() {
    let (_dir, mut ov) = overlay_fixture();
    ov.remove_file("/shared.txt").unwrap();
    assert!(ov.has_whiteout("/shared.txt").unwrap());
    ov.write_file("/shared.txt", b"reborn").unwrap();
    // SPEC overlay rule 1: whiteout MUST be removed on re-creation.
    assert!(!ov.has_whiteout("/shared.txt").unwrap());
    assert_eq!(ov.read_file("/shared.txt").unwrap(), b"reborn");
}

#[test]
fn deleting_delta_only_file_leaves_no_whiteout() {
    let (_dir, mut ov) = overlay_fixture();
    ov.write_file("/delta-only.txt", b"x").unwrap();
    ov.remove_file("/delta-only.txt").unwrap();
    // SPEC overlay rule 2 is conditional: whiteouts are only for base files.
    assert!(!ov.has_whiteout("/delta-only.txt").unwrap());
    assert!(!ov.exists("/delta-only.txt").unwrap());
}

#[test]
fn copy_up_preserves_metadata_and_origin_ino() {
    let (_dir, mut ov) = overlay_fixture();
    let base_meta = ov.base().stat("/shared.txt").unwrap();
    let delta_ino = ov.copy_up("/shared.txt").unwrap();
    // Origin mapping stored (SPEC overlay rule 5)…
    assert_eq!(ov.origin_ino(delta_ino).unwrap(), Some(base_meta.ino));
    // …and stat returns the BASE inode number (rule 6: kernel cache safety).
    let meta = ov.stat("/shared.txt").unwrap();
    assert_eq!(meta.ino, base_meta.ino);
    assert_eq!(meta.mode, base_meta.mode);
    assert_eq!(meta.mtime, base_meta.mtime);
    assert_eq!(ov.read_file("/shared.txt").unwrap(), b"from base");
    // Copy-up is idempotent.
    assert_eq!(ov.copy_up("/shared.txt").unwrap(), delta_ino);
}

#[test]
fn overwrite_of_base_file_records_origin() {
    let (_dir, mut ov) = overlay_fixture();
    let base_ino = ov.base().stat("/shared.txt").unwrap().ino;
    ov.write_file("/shared.txt", b"replaced").unwrap();
    assert_eq!(ov.stat("/shared.txt").unwrap().ino, base_ino);
}

#[test]
fn overlay_readdir_merges_and_hides_whiteouts() {
    let (_dir, mut ov) = overlay_fixture();
    ov.write_file("/docs/notes.md", b"delta notes").unwrap();
    ov.remove_file("/docs/guide.md").unwrap();
    assert_eq!(
        ov.readdir("/docs").unwrap(),
        vec!["notes.md".to_string(), "readme.md".to_string()]
    );
    // Root merge: delta /docs dir + base names, deduplicated.
    let root = ov.readdir("/").unwrap();
    assert_eq!(root, vec!["docs".to_string(), "shared.txt".to_string()]);
}

#[test]
fn overlay_layers_stay_spec_consistent() {
    let (_dir, mut ov) = overlay_fixture();
    ov.write_file("/shared.txt", b"delta").unwrap();
    ov.copy_up("/docs/readme.md").unwrap();
    ov.remove_file("/docs/guide.md").unwrap();
    assert_consistent(ov.delta());
    assert_consistent(ov.base());
}

// ---- kv ---------------------------------------------------------------------

#[test]
fn kv_roundtrip_upsert_delete() {
    let fs = AgentFs::in_memory().unwrap();
    fs.kv_set("session:state", &json!({"phase": "gather"}))
        .unwrap();
    fs.kv_set("session:state", &json!({"phase": "publish"}))
        .unwrap();
    assert_eq!(
        fs.kv_get("session:state").unwrap(),
        Some(json!({"phase": "publish"}))
    );
    fs.kv_set("a", &json!(1)).unwrap();
    assert_eq!(fs.kv_keys().unwrap(), vec!["a", "session:state"]);
    assert!(fs.kv_delete("a").unwrap());
    assert!(!fs.kv_delete("a").unwrap());
    assert_eq!(fs.kv_get("a").unwrap(), None);
}

// ---- tool calls ---------------------------------------------------------------

#[test]
fn tool_call_records_and_computes_duration() {
    let fs = AgentFs::in_memory().unwrap();
    fs.record_tool_call(
        "read_file",
        Some(&json!({"path": "/x"})),
        Some(&json!({"bytes": 42})),
        None,
        1_000,
        1_003,
    )
    .unwrap();
    fs.record_tool_call("web_search", None, None, Some("timeout"), 1_010, 1_010)
        .unwrap();

    let calls = fs.tool_calls_by_name("read_file").unwrap();
    assert_eq!(calls.len(), 1);
    // SPEC rule 3: duration_ms = (completed_at - started_at) * 1000.
    assert_eq!(calls[0].duration_ms, 3_000);
    assert!(calls[0].error.is_none());

    let recent = fs.recent_tool_calls(1_005).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].name, "web_search");
    assert_eq!(recent[0].duration_ms, 0);
}

#[test]
fn tool_call_enforces_result_error_mutual_exclusion() {
    let fs = AgentFs::in_memory().unwrap();
    // SPEC rule 1: exactly one of result/error.
    assert!(fs.record_tool_call("t", None, None, None, 1, 2).is_err());
    assert!(fs
        .record_tool_call("t", None, Some(&json!(1)), Some("boom"), 1, 2)
        .is_err());
    assert!(fs
        .record_tool_call("t", None, Some(&json!(1)), None, 2, 1)
        .is_err());
}

// ---- read-only ---------------------------------------------------------------

#[test]
fn read_only_open_rejects_mutation_but_allows_reads() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("agent.db");
    {
        let mut fs = AgentFs::create(&db).unwrap();
        fs.write_file("/f", b"x").unwrap();
        fs.kv_set("k", &json!(true)).unwrap();
    }
    let mut ro = AgentFs::open_read_only(&db).unwrap();
    assert_eq!(ro.read_file("/f").unwrap(), b"x");
    assert_eq!(ro.kv_get("k").unwrap(), Some(json!(true)));
    assert!(matches!(
        ro.write_file("/g", b"y"),
        Err(coven_afs::Error::ReadOnly)
    ));
    assert!(matches!(
        ro.kv_set("k2", &json!(1)),
        Err(coven_afs::Error::ReadOnly)
    ));
    assert!(matches!(
        ro.record_tool_call("t", None, Some(&json!(1)), None, 1, 2),
        Err(coven_afs::Error::ReadOnly)
    ));
}
