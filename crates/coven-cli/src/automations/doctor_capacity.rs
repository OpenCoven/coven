//! Point-in-time native filesystem estimate, separate from the SQLite snapshot.
use super::MAX_SAFE;
use rusqlite::Connection;
use std::{io, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Observation {
    Observed { bytes: u64, sampled_at: u64 },
    Unavailable,
    InvalidMetadata,
}

pub(super) fn inspect(conn: &Connection) -> Observation {
    inspect_with(
        conn,
        |path| fs2::available_space(path),
        || chrono::Utc::now().timestamp_millis(),
    )
}

fn inspect_with(
    conn: &Connection,
    probe: impl FnOnce(&Path) -> io::Result<u64>,
    clock: impl FnOnce() -> i64,
) -> Observation {
    let Some(path) = conn.path().filter(|path| !path.is_empty()) else {
        return Observation::Unavailable;
    };
    let Ok(bytes) = probe(Path::new(path)) else {
        return Observation::Unavailable;
    };
    let Some(sampled_at) = u64::try_from(clock()).ok().filter(|at| *at <= MAX_SAFE) else {
        return Observation::InvalidMetadata;
    };
    if bytes > MAX_SAFE {
        return Observation::InvalidMetadata;
    }
    Observation::Observed { bytes, sampled_at }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_probe_keeps_zero_and_safe_boundaries_without_exporting_errors() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("PRIVATE_store.sqlite3");
        let conn = Connection::open(&path).unwrap();
        for bytes in [0, 1234, MAX_SAFE] {
            assert_eq!(
                inspect_with(
                    &conn,
                    |observed| {
                        assert_eq!(
                            observed.canonicalize().unwrap(),
                            path.canonicalize().unwrap()
                        );
                        Ok(bytes)
                    },
                    || 123
                ),
                Observation::Observed {
                    bytes,
                    sampled_at: 123
                }
            );
        }
        for bytes in [MAX_SAFE + 1, u64::MAX] {
            assert_eq!(
                inspect_with(&conn, |_| Ok(bytes), || 123),
                Observation::InvalidMetadata
            );
        }
        for at in [-1, i64::MAX] {
            assert_eq!(
                inspect_with(&conn, |_| Ok(123), || at),
                Observation::InvalidMetadata
            );
        }
        let result = inspect_with(
            &conn,
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "PRIVATE_PATH PRIVATE_ERROR",
                ))
            },
            || panic!("failed probe cannot have a sample"),
        );
        assert_eq!(result, Observation::Unavailable);
        assert!(!format!("{result:?}").contains("PRIVATE"));
    }

    #[test]
    fn capacity_probe_does_not_substitute_cwd_for_an_in_memory_store() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(
            inspect_with(
                &conn,
                |_| panic!("must not probe unrelated filesystem"),
                || panic!("no sample")
            ),
            Observation::Unavailable
        );
    }
}
