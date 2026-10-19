//! Filesystem-name metadata only, separate from SQLite snapshot/WAL semantics.
use super::MAX_SAFE;
use rusqlite::Connection;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Observation {
    Observed {
        present: bool,
        bytes: u64,
        sampled_at: u64,
    },
    Unavailable,
    InvalidMetadata,
}

struct FileMetadata {
    regular: bool,
    bytes: u64,
}

pub(super) fn inspect(conn: &Connection) -> Observation {
    inspect_with(
        conn,
        |path| {
            // Do not follow a sidecar symlink or read any file contents.
            std::fs::symlink_metadata(path).map(|metadata| FileMetadata {
                regular: metadata.is_file(),
                bytes: metadata.len(),
            })
        },
        || chrono::Utc::now().timestamp_millis(),
    )
}

fn inspect_with(
    conn: &Connection,
    probe: impl Fn(&Path) -> io::Result<FileMetadata>,
    clock: impl FnOnce() -> i64,
) -> Observation {
    let Some(path) = conn.path().filter(|path| !path.is_empty()) else {
        return Observation::Unavailable;
    };
    if !probe(Path::new(path)).is_ok_and(|metadata| metadata.regular) {
        return Observation::Unavailable;
    }
    let mut name = std::ffi::OsString::from(path);
    name.push("-wal");
    let (present, bytes) = match probe(&PathBuf::from(name)) {
        Ok(metadata) if metadata.regular => (true, metadata.bytes),
        Ok(_) => return Observation::InvalidMetadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => (false, 0),
        Err(_) => return Observation::Unavailable,
    };
    let Some(sampled_at) = u64::try_from(clock()).ok().filter(|at| *at <= MAX_SAFE) else {
        return Observation::InvalidMetadata;
    };
    if bytes > MAX_SAFE {
        return Observation::InvalidMetadata;
    }
    Observation::Observed {
        present,
        bytes,
        sampled_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidecar_probe_distinguishes_absence_unavailable_invalid_and_safe_values() {
        let root = tempfile::tempdir().unwrap();
        let conn = Connection::open(root.path().join("PRIVATE_store.sqlite3")).unwrap();
        let main = Path::new(conn.path().unwrap());
        for bytes in [0, 1234, MAX_SAFE, MAX_SAFE + 1, u64::MAX] {
            let value = inspect_with(
                &conn,
                |path| {
                    Ok(FileMetadata {
                        regular: true,
                        bytes: if path == main { 0 } else { bytes },
                    })
                },
                || 123,
            );
            assert_eq!(
                value,
                if bytes <= MAX_SAFE {
                    Observation::Observed {
                        present: true,
                        bytes,
                        sampled_at: 123,
                    }
                } else {
                    Observation::InvalidMetadata
                }
            );
        }
        assert_eq!(
            inspect_with(
                &conn,
                |path| if path == main {
                    Ok(FileMetadata {
                        regular: true,
                        bytes: 0,
                    })
                } else {
                    Err(io::Error::from(io::ErrorKind::NotFound))
                },
                || 123
            ),
            Observation::Observed {
                present: false,
                bytes: 0,
                sampled_at: 123
            }
        );
        for at in [-1, i64::MAX] {
            assert_eq!(
                inspect_with(
                    &conn,
                    |_| Ok(FileMetadata {
                        regular: true,
                        bytes: 1
                    }),
                    || at
                ),
                Observation::InvalidMetadata
            );
        }
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            let result = inspect_with(
                &conn,
                |path| {
                    if path == main {
                        Ok(FileMetadata {
                            regular: true,
                            bytes: 0,
                        })
                    } else {
                        Err(io::Error::new(kind, "PRIVATE_PATH PRIVATE_ERROR"))
                    }
                },
                || panic!("failed probe has no sample"),
            );
            assert_eq!(result, Observation::Unavailable);
            assert!(!format!("{result:?}").contains("PRIVATE"));
        }
        assert_eq!(
            inspect_with(
                &conn,
                |path| Ok(FileMetadata {
                    regular: path == main,
                    bytes: 1
                }),
                || panic!("invalid sidecar has no sample")
            ),
            Observation::InvalidMetadata
        );
        assert_eq!(
            inspect_with(
                &conn,
                |_| Err(io::Error::from(io::ErrorKind::NotFound)),
                || panic!("missing source has no sample")
            ),
            Observation::Unavailable
        );
        let memory = Connection::open_in_memory().unwrap();
        assert_eq!(
            inspect_with(
                &memory,
                |_| panic!("no cwd fallback"),
                || panic!("no sample")
            ),
            Observation::Unavailable
        );
    }

    #[test]
    fn native_absent_sidecar_is_observed_without_creating_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("PRIVATE_store.sqlite3");
        let conn = Connection::open(&path).unwrap();
        assert!(matches!(
            inspect(&conn),
            Observation::Observed {
                present: false,
                bytes: 0,
                ..
            }
        ));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn native_sidecar_symlink_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let conn = Connection::open(root.path().join("store.sqlite3")).unwrap();
        let private = root.path().join("PRIVATE");
        std::fs::write(&private, b"PRIVATE_CONTENT").unwrap();
        std::os::unix::fs::symlink(&private, root.path().join("store.sqlite3-wal")).unwrap();
        assert_eq!(inspect(&conn), Observation::InvalidMetadata);
        assert_eq!(std::fs::read(&private).unwrap(), b"PRIVATE_CONTENT");
    }
}
