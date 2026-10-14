use coven_memory::{db::MetaDb, index::VecIndex};
use std::process::Command;

#[test]
fn rebuild_empty_store_does_not_load_legacy_index_or_download_a_model() {
    let root = std::env::temp_dir().join(format!("coven-rebuild-cli-{}", uuid::Uuid::new_v4()));
    let db_path = root.join("metadata.sqlite3");
    drop(MetaDb::open(&db_path).unwrap());
    let legacy = root.join("legacy.tvim");
    let original = b"TVIM\x03 legacy index fixture";
    std::fs::write(&legacy, original).unwrap();
    let rebuilt = root.join("rebuilt.tvim");
    let output = Command::new(env!("CARGO_BIN_EXE_coven-memory"))
        .current_dir(&root)
        .arg("--db")
        .arg(&db_path)
        .arg("--index")
        .arg(&legacy)
        .arg("rebuild")
        .arg("--output")
        .arg("rebuilt.tvim")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "empty rebuild must not initialize the model"
    );
    assert!(VecIndex::open(&rebuilt).unwrap().is_empty());
    assert_eq!(std::fs::read(legacy).unwrap(), original);
    std::fs::remove_dir_all(root).unwrap();
}
