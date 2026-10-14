//! Rebuild a vector index from persisted chunks without changing the source store.

use crate::{db::MetaDb, embed::DIM, fs_security, index::VecIndex};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

struct RebuildOutput {
    path: PathBuf,
    complete: bool,
}

impl Drop for RebuildOutput {
    fn drop(&mut self) {
        if !self.complete {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn rebuild_index(
    db: &MetaDb,
    output: &Path,
    mut embed: impl FnMut(Vec<String>) -> Result<Vec<Vec<f32>>>,
) -> Result<usize> {
    // A bare relative filename has an empty parent, which the privacy
    // helper cannot canonicalize. Resolve it before reserving the output.
    let output = if output.is_absolute() {
        output.to_owned()
    } else {
        std::env::current_dir()?.join(output)
    };
    let output = output.as_path();
    fs_security::ensure_private_parent(output)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Reserve a NEW destination before embedding. Never replace a legacy
    // index, the metadata DB, or a symlink supplied as the output path.
    drop(
        options
            .open(output)
            .with_context(|| format!("rebuild requires a new output file: {}", output.display()))?,
    );
    let mut guard = RebuildOutput {
        path: output.to_owned(),
        complete: false,
    };
    let mut index = VecIndex::fresh(output)?;
    let ids = db.all_ids()?;
    for batch in ids.chunks(256) {
        let docs = db.get_many(batch)?;
        anyhow::ensure!(docs.len() == batch.len(), "metadata changed during rebuild");
        let vectors = embed(docs.iter().map(|doc| doc.chunk.clone()).collect())?;
        anyhow::ensure!(
            vectors.len() == docs.len() && vectors.iter().all(|vector| vector.len() == DIM),
            "embedding batch has an invalid count or dimension"
        );
        index.add(&docs.iter().map(|doc| doc.id).collect::<Vec<_>>(), &vectors)?;
    }
    index.save()?;
    guard.complete = true;
    Ok(ids.len())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{embed::DIM, index::VecIndex, MemoryDoc};

    fn fixture() -> (std::path::PathBuf, MetaDb, u64) {
        let root = std::env::temp_dir().join(format!("coven-rebuild-{}", uuid::Uuid::new_v4()));
        let db = MetaDb::open(&root.join("metadata.sqlite3")).unwrap();
        let id = db
            .insert(&MemoryDoc {
                id: 0,
                path: "fixture.txt".into(),
                familiar: "coven".into(),
                chunk: "persisted chunk".into(),
                chunk_offset: 0,
                content_hash: "fixture-hash".into(),
                ingested_at: 0,
            })
            .unwrap();
        (root, db, id)
    }

    #[test]
    fn rebuild_preserves_ids_metadata_and_legacy_index() {
        let (root, db, id) = fixture();
        let legacy = root.join("legacy.tvim");
        let original = b"TVIM\x03 legacy index fixture";
        std::fs::write(&legacy, original).unwrap();
        let output = root.join("rebuilt.tvim");
        let count = rebuild_index(&db, &output, |texts| {
            assert_eq!(texts, vec!["persisted chunk"]);
            Ok(vec![vec![1.0; DIM]])
        })
        .unwrap();
        assert_eq!(count, 1);
        let index = VecIndex::open(&output).unwrap();
        assert_eq!(index.search(&vec![1.0; DIM], 1).unwrap().1, vec![id]);
        assert_eq!(db.get(id).unwrap().unwrap().chunk, "persisted chunk");
        assert_eq!(std::fs::read(legacy).unwrap(), original);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rebuild_refuses_to_overwrite_an_existing_output() {
        let (root, db, _) = fixture();
        let output = root.join("existing.tvim");
        std::fs::write(&output, b"existing bytes").unwrap();
        assert!(rebuild_index(&db, &output, |_| panic!("must not embed")).is_err());
        assert_eq!(std::fs::read(output).unwrap(), b"existing bytes");
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_rebuild_cleans_only_its_new_output() {
        let (root, db, id) = fixture();
        let output = root.join("failed.tvim");
        let error = rebuild_index(&db, &output, |_| anyhow::bail!("embedding failed")).unwrap_err();
        assert!(error.to_string().contains("embedding failed"), "{error:#}");
        assert!(!output.exists());
        assert!(db.get(id).unwrap().is_some());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_embedding_batch_does_not_publish_an_index() {
        let (root, db, _) = fixture();
        let output = root.join("invalid.tvim");
        assert!(rebuild_index(&db, &output, |_| Ok(vec![vec![1.0; 1]])).is_err());
        assert!(!output.exists());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rebuild_refuses_a_dangling_output_symlink() {
        let (root, db, _) = fixture();
        let output = root.join("link.tvim");
        let target = root.join("missing.tvim");
        std::os::unix::fs::symlink(&target, &output).unwrap();
        assert!(rebuild_index(&db, &output, |_| panic!("must not embed")).is_err());
        assert!(std::fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!target.exists());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
