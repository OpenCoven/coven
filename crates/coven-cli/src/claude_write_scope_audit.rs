//! Filesystem-object admission for denied Claude roots. No grants are widened.

#[cfg(unix)]
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    os::unix::fs::MetadataExt,
    path::PathBuf,
};

// Large read-only dependency trees are legitimate granted projects. Keep the
// audit finite, but do not reject an ordinary 1.5M-entry project at 200K.
#[cfg(unix)]
const MAX_ENTRIES: usize = 2_000_000;

#[cfg(unix)]
#[derive(Clone, PartialEq, Eq)]
struct ObjectIdentity {
    dev: u64,
    ino: u64,
    links: u64,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

#[cfg(unix)]
impl ObjectIdentity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            links: metadata.nlink(),
            len: metadata.len(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        }
    }
}

#[cfg(unix)]
struct DeniedObject {
    identity: ObjectIdentity,
    paths: Vec<PathBuf>,
}

#[cfg(unix)]
type DeniedObjects = BTreeMap<(u64, u64), DeniedObject>;

#[cfg(unix)]
fn alias_refusal() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied,
        "writeScope protected or read-only files have hardlink aliases outside the proven denied paths; remove the extra links or narrow the grants")
}

#[cfg(unix)]
fn collect_denied_objects(roots: &[PathBuf], limit: usize) -> io::Result<DeniedObjects> {
    let mut roots = roots.to_vec();
    roots.sort();
    roots.dedup();
    let mut disjoint: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !disjoint.iter().any(|parent| root.starts_with(parent)) {
            if !fs::symlink_metadata(&root)?.is_dir() {
                return Err(alias_refusal());
            }
            disjoint.push(root);
        }
    }
    // Canonical roots are disjoint and symlinks are not followed, so a path
    // cannot vote twice. Avoid retaining every dependency filename in memory.
    let mut pending = disjoint;
    let mut count = 0usize;
    let mut objects: DeniedObjects = BTreeMap::new();
    let mut directories = BTreeSet::new();
    while let Some(path) = pending.pop() {
        count += 1;
        if count > limit {
            return Err(io::Error::other(format!(
                "writeScope protected-resource audit exceeds {limit} entries"
            )));
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            // macOS firmlinks can give distinct canonical paths to the same
            // directory. Each directory entry may vote only once, regardless
            // of spelling; distinct hardlink names within it still count.
            if !directories.insert((metadata.dev(), metadata.ino())) {
                continue;
            }
            for entry in fs::read_dir(&path)? {
                if pending.len() >= limit.saturating_sub(count) {
                    return Err(io::Error::other(format!(
                        "writeScope protected-resource audit exceeds {limit} entries"
                    )));
                }
                pending.push(entry?.path());
            }
        } else if metadata.is_file() && metadata.nlink() > 1 {
            let identity = ObjectIdentity::of(&metadata);
            let object = objects
                .entry((identity.dev, identity.ino))
                .or_insert_with(|| DeniedObject {
                    identity: identity.clone(),
                    paths: Vec::new(),
                });
            if object.identity != identity {
                return Err(alias_refusal());
            }
            object.paths.push(path);
        }
    }
    Ok(objects)
}

#[cfg(unix)]
fn recheck_denied_objects(objects: DeniedObjects) -> io::Result<()> {
    for object in objects.into_values() {
        if object.paths.len() as u64 != object.identity.links {
            return Err(alias_refusal());
        }
        for path in object.paths {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_file() || ObjectIdentity::of(&metadata) != object.identity {
                return Err(alias_refusal());
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn audit_denied_objects(roots: &[PathBuf]) -> io::Result<()> {
    recheck_denied_objects(collect_denied_objects(roots, MAX_ENTRIES)?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "coven-denied-alias-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn directory(&self, name: &str) -> PathBuf {
            let dir = self.0.join(name);
            fs::create_dir_all(&dir).unwrap();
            dir
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn accepts_every_hardlink_name_inside_one_denied_tree() {
        let f = Fixture::new();
        let denied = f.directory("denied");
        fs::write(denied.join("esbuild"), "binary sentinel").unwrap();
        fs::hard_link(denied.join("esbuild"), denied.join("package-esbuild")).unwrap();
        audit_denied_objects(std::slice::from_ref(&denied)).unwrap();
        assert_eq!(
            fs::read_to_string(denied.join("esbuild")).unwrap(),
            "binary sentinel"
        );
    }

    #[test]
    fn accepts_aliases_across_two_denied_roots_and_deduplicates_overlap() {
        let f = Fixture::new();
        let first = f.directory("first");
        let child = f.directory("first/child");
        let second = f.directory("second");
        fs::write(child.join("source"), "protected").unwrap();
        fs::hard_link(child.join("source"), second.join("copy")).unwrap();
        audit_denied_objects(&[first.clone(), child, second.clone(), first]).unwrap();
    }

    #[test]
    fn refuses_any_unaccounted_name_even_when_other_names_are_denied() {
        let f = Fixture::new();
        let denied = f.directory("denied");
        let writable = f.directory("writable");
        fs::write(denied.join("source"), "protected").unwrap();
        fs::hard_link(denied.join("source"), denied.join("allowed-copy")).unwrap();
        fs::hard_link(denied.join("source"), writable.join("escape")).unwrap();
        assert!(audit_denied_objects(&[denied]).is_err());
        assert_eq!(
            fs::read_to_string(writable.join("escape")).unwrap(),
            "protected"
        );
    }

    #[test]
    fn entry_budget_refuses_before_ignoring_unvisited_data() {
        let f = Fixture::new();
        let denied = f.directory("denied");
        fs::write(denied.join("one"), "one").unwrap();
        fs::write(denied.join("two"), "two").unwrap();
        assert!(collect_denied_objects(std::slice::from_ref(&denied), 2).is_err());
        assert!(collect_denied_objects(&[denied], 3).is_ok());
    }

    #[test]
    fn a_new_alias_between_enumeration_and_recheck_is_refused() {
        let f = Fixture::new();
        let denied = f.directory("denied");
        fs::write(denied.join("source"), "protected").unwrap();
        fs::hard_link(denied.join("source"), denied.join("copy")).unwrap();
        let objects = collect_denied_objects(std::slice::from_ref(&denied), 100).unwrap();
        fs::hard_link(denied.join("source"), f.0.join("late-escape")).unwrap();
        assert!(recheck_denied_objects(objects).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn firmlink_spellings_cannot_credit_the_same_directory_entry_twice() {
        let f = Fixture::new();
        let denied = fs::canonicalize(f.directory("denied")).unwrap();
        let alternate = fs::canonicalize(
            PathBuf::from("/System/Volumes/Data").join(denied.strip_prefix("/").unwrap()),
        )
        .unwrap();
        assert_ne!(denied, alternate);
        let first = fs::metadata(&denied).unwrap();
        let second = fs::metadata(&alternate).unwrap();
        assert_eq!((first.dev(), first.ino()), (second.dev(), second.ino()));
        fs::write(denied.join("source"), "protected").unwrap();
        fs::hard_link(denied.join("source"), f.0.join("writable-alias")).unwrap();
        assert!(audit_denied_objects(&[denied, alternate]).is_err());
    }
}
