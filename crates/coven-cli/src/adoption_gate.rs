use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};

const LOCK_DIR: &str = "request-adoption-locks";

pub struct AdoptionGate {
    files: Vec<std::fs::File>,
}

impl Drop for AdoptionGate {
    fn drop(&mut self) {
        // Leave lock files on disk: drop releases the OS locks, and unlink/recreate can split
        // mutual exclusion between waiters on the old inode and callers opening a replacement.
        for file in &self.files {
            let _ = file.unlock();
        }
    }
}

impl AdoptionGate {
    pub fn acquire(
        coven_home: &Path,
        request_key: &str,
        attempt_scope: Option<&[&str]>,
    ) -> Result<Self> {
        crate::daemon::ensure_private_coven_home(coven_home)?;
        let directory = coven_home.join(LOCK_DIR);
        fs::create_dir_all(&directory).with_context(|| {
            format!(
                "failed to create adoption lock directory {}",
                directory.display()
            )
        })?;
        let paths = lock_paths_for_acquire(&directory, request_key, attempt_scope);
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let file = crate::state_lock::open_lock_file(&path)?;
            file.lock_exclusive().with_context(|| {
                format!("failed to acquire request-adoption lock {}", path.display())
            })?;
            files.push(file);
        }
        Ok(Self { files })
    }
}

fn lock_path(directory: &Path, kind: &str, fields: &[&str]) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(b"opencoven:psyche:o3:");
    hasher.update(kind.as_bytes());
    for field in fields {
        hasher.update([0]);
        hasher.update(field.as_bytes());
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    directory.join(format!("{kind}-{digest}.lock"))
}

fn lock_paths_for_acquire(
    directory: &Path,
    request_key: &str,
    attempt_scope: Option<&[&str]>,
) -> Vec<PathBuf> {
    let mut paths = vec![lock_path(directory, "key", &[request_key])];
    if let Some(scope) = attempt_scope {
        paths.push(lock_path(directory, "scope", scope));
    }
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use anyhow::{bail, Context};
    use fs2::FileExt;

    const CHILD_MODE_ENV: &str = "COVEN_ADOPTION_GATE_CHILD_MODE";
    const CHILD_HOME_ENV: &str = "COVEN_ADOPTION_GATE_CHILD_HOME";
    const CHILD_KEY_ENV: &str = "COVEN_ADOPTION_GATE_CHILD_KEY";
    const CHILD_READY_ENV: &str = "COVEN_ADOPTION_GATE_CHILD_READY";
    const CHILD_RELEASE_ENV: &str = "COVEN_ADOPTION_GATE_CHILD_RELEASE";
    const CHILD_TEST_NAME: &str = "adoption_gate::tests::subprocess_child_holds_gate";
    static NEXT_SCRATCH_ID: AtomicU64 = AtomicU64::new(0);

    struct ScratchDir {
        path: PathBuf,
    }

    impl ScratchDir {
        fn new(label: &str) -> Result<Self> {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("adoption-gate-tests");
            fs::create_dir_all(&root)
                .with_context(|| format!("create scratch root {}", root.display()))?;
            let unique = format!(
                "{label}-{}-{}-{}",
                std::process::id(),
                NEXT_SCRATCH_ID.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            );
            let path = root.join(unique);
            if path.exists() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("remove stale scratch {}", path.display()))?;
            }
            fs::create_dir(&path).with_context(|| format!("create scratch {}", path.display()))?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn scope_fields() -> [&'static str; 5] {
        [
            "principal-7",
            "project-11",
            "graph-13",
            "node-17",
            "attempt-19",
        ]
    }

    fn request_key() -> &'static str {
        "psyche/request:key"
    }

    fn lock_file_name_matches(name: &str, kind: &str) -> bool {
        let Some(rest) = name
            .strip_prefix(kind)
            .and_then(|value| value.strip_prefix('-'))
            .and_then(|value| value.strip_suffix(".lock"))
        else {
            return false;
        };
        rest.len() == 64
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.exists() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        }
        bail!("timed out waiting for {}", path.display())
    }

    fn assert_lock_contended(path: &Path) -> Result<()> {
        let file = crate::state_lock::open_lock_file(path)?;
        let error = file
            .try_lock_exclusive()
            .expect_err("lock must stay contended while guard is alive");
        assert!(
            crate::state_lock::is_lock_contended(&error),
            "unexpected lock error for {}: {error}",
            path.display()
        );
        Ok(())
    }

    fn assert_lock_available(path: &Path) -> Result<()> {
        let file = crate::state_lock::open_lock_file(path)?;
        file.try_lock_exclusive()
            .with_context(|| format!("lock should be available {}", path.display()))?;
        file.unlock()
            .with_context(|| format!("unlock probe {}", path.display()))?;
        Ok(())
    }

    #[test]
    fn lock_paths_are_digest_named_and_never_embed_caller_values() -> Result<()> {
        let scratch = ScratchDir::new("names-private")?;
        let directory = scratch.path().join("locks");
        fs::create_dir_all(&directory)?;
        let scope = scope_fields();
        let key_path = lock_path(&directory, "key", &[request_key()]);
        let scope_path = lock_path(&directory, "scope", &scope);

        let key_name = key_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap();
        let scope_name = scope_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap();
        assert!(lock_file_name_matches(key_name, "key"), "{key_name}");
        assert!(lock_file_name_matches(scope_name, "scope"), "{scope_name}");

        for raw in [
            request_key(),
            scope[0],
            scope[1],
            scope[2],
            scope[3],
            scope[4],
        ] {
            assert!(
                !key_path.display().to_string().contains(raw),
                "key path leaked {raw}: {}",
                key_path.display()
            );
            assert!(
                !scope_path.display().to_string().contains(raw),
                "scope path leaked {raw}: {}",
                scope_path.display()
            );
        }
        Ok(())
    }

    #[test]
    fn same_key_contenders_block_until_first_guard_drops() -> Result<()> {
        let scratch = ScratchDir::new("same-key-blocks")?;
        let home = scratch.path().join("home");
        let first = AdoptionGate::acquire(&home, request_key(), None)?;
        let barrier = Arc::new(Barrier::new(2));
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let home_for_thread = home.clone();
        let barrier_for_thread = Arc::clone(&barrier);
        let contender = thread::spawn(move || -> Result<()> {
            barrier_for_thread.wait();
            let _guard = AdoptionGate::acquire(&home_for_thread, request_key(), None)?;
            acquired_tx.send(()).expect("notify acquisition");
            Ok(())
        });

        barrier.wait();
        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "same-key contender must block until the first guard drops"
        );
        drop(first);
        acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .context("same-key contender must acquire after release")?;
        contender.join().expect("same-key contender thread")?;
        Ok(())
    }

    #[test]
    fn disjoint_keys_proceed_independently() -> Result<()> {
        let scratch = ScratchDir::new("disjoint-keys")?;
        let home = scratch.path().join("home");
        let _first = AdoptionGate::acquire(&home, request_key(), None)?;
        let _second = AdoptionGate::acquire(&home, "different/request:key", None)?;
        Ok(())
    }

    #[test]
    fn launch_locks_are_acquired_in_sorted_full_path_order() -> Result<()> {
        let scratch = ScratchDir::new("sorted-paths")?;
        let directory = scratch.path().join("locks");
        fs::create_dir_all(&directory)?;
        let scope = scope_fields();

        let actual = lock_paths_for_acquire(&directory, request_key(), Some(&scope));
        let mut expected = vec![
            lock_path(&directory, "scope", &scope),
            lock_path(&directory, "key", &[request_key()]),
        ];
        expected.sort();
        assert_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn dropping_guard_releases_every_os_lock() -> Result<()> {
        let scratch = ScratchDir::new("guard-release")?;
        let home = scratch.path().join("home");
        let scope = scope_fields();
        let guard = AdoptionGate::acquire(&home, request_key(), Some(&scope))?;
        let directory = home.join(LOCK_DIR);
        let paths = lock_paths_for_acquire(&directory, request_key(), Some(&scope));
        for path in &paths {
            assert_lock_contended(path)?;
        }
        drop(guard);
        for path in &paths {
            assert_lock_available(path)?;
        }
        Ok(())
    }

    #[test]
    fn child_process_contention_proves_process_independent_locking() -> Result<()> {
        let scratch = ScratchDir::new("child-process")?;
        let home = scratch.path().join("home");
        let ready = scratch.path().join("child-ready");
        let release = scratch.path().join("child-release");
        let child = Command::new(std::env::current_exe().context("current test executable")?)
            .arg("--ignored")
            .arg("--exact")
            .arg(CHILD_TEST_NAME)
            .arg("--nocapture")
            .env(CHILD_MODE_ENV, "hold")
            .env(CHILD_HOME_ENV, &home)
            .env(CHILD_KEY_ENV, request_key())
            .env(CHILD_READY_ENV, &ready)
            .env(CHILD_RELEASE_ENV, &release)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn adoption gate child test")?;

        wait_for_path(&ready, Duration::from_secs(5))?;

        let barrier = Arc::new(Barrier::new(2));
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let home_for_thread = home.clone();
        let barrier_for_thread = Arc::clone(&barrier);
        let contender = thread::spawn(move || -> Result<()> {
            barrier_for_thread.wait();
            let _guard = AdoptionGate::acquire(&home_for_thread, request_key(), None)?;
            acquired_tx
                .send(())
                .expect("same-key child contention acquired");
            Ok(())
        });

        barrier.wait();
        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "same-key acquisition must wait while the child process holds the gate"
        );
        let _different = AdoptionGate::acquire(&home, "different/request:key", None)?;
        fs::write(&release, b"release")
            .with_context(|| format!("write child release marker {}", release.display()))?;
        acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .context("same-key acquisition should finish after child release")?;
        contender
            .join()
            .expect("same-key child contention thread")?;

        let output = child
            .wait_with_output()
            .context("wait for adoption gate child test")?;
        if !output.status.success() {
            bail!(
                "child test failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    #[test]
    #[ignore]
    fn subprocess_child_holds_gate() -> Result<()> {
        if std::env::var_os(CHILD_MODE_ENV).is_none() {
            return Ok(());
        }

        let home = PathBuf::from(
            std::env::var_os(CHILD_HOME_ENV).expect("child test must receive a home path"),
        );
        let key = std::env::var(CHILD_KEY_ENV).expect("child test must receive a request key");
        let ready = PathBuf::from(
            std::env::var_os(CHILD_READY_ENV).expect("child test must receive a ready marker"),
        );
        let release = PathBuf::from(
            std::env::var_os(CHILD_RELEASE_ENV).expect("child test must receive a release marker"),
        );

        let _guard = AdoptionGate::acquire(&home, &key, None)?;
        fs::write(&ready, b"ready")
            .with_context(|| format!("write child ready marker {}", ready.display()))?;
        wait_for_path(&release, Duration::from_secs(10))?;
        Ok(())
    }
}
