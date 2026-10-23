//! Advisory, attempt-correlated context for a failed background startup.
//! Health remains authoritative; this module never retries or repairs anything.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;

use super::{
    append_daemon_recovery_log, daemon_recovery_log_path, DaemonStatus, LifecycleDeadline,
};
use crate::mobile_memory::config::MobileConfigValidationFailure;

const MAX_LOG_BYTES: u64 = 16 * 1024;
const OBSERVATION_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    MobileConfiguration,
    MobileInitialization,
}

impl Failure {
    const ALL: [Self; 2] = [Self::MobileConfiguration, Self::MobileInitialization];

    fn code(self) -> &'static str {
        match self {
            Self::MobileConfiguration => "mobile-configuration",
            Self::MobileInitialization => "mobile-initialization",
        }
    }

    fn guidance(self) -> &'static str {
        match self {
            Self::MobileConfiguration => "invalid mobile gateway configuration; check mobile/gateway.json: bind must use a non-zero private-network port and advertisedEndpoint must be a matching HTTPS endpoint; then run `coven daemon restart`",
            Self::MobileInitialization => "mobile gateway initialization failed; check mobile/gateway.json, its permissions and local address/port availability; then run `coven daemon restart`",
        }
    }

    fn record(self, status: &DaemonStatus) -> String {
        // The existing nanosecond started-at identity distinguishes PID reuse.
        // Hash it so even a directly supplied identity cannot inject log text.
        let attempt = blake3::hash(status.started_at.as_bytes());
        format!(
            "startup_failure pid={} attempt={} code={}",
            status.pid,
            attempt.to_hex(),
            self.code()
        )
    }
}

pub(super) fn record(home: &Path, status: &DaemonStatus, error: &anyhow::Error) {
    let failure = if error.is::<MobileConfigValidationFailure>() {
        Failure::MobileConfiguration
    } else {
        Failure::MobileInitialization
    };
    append_daemon_recovery_log(home, &failure.record(status));
}

pub(super) struct Observer {
    failure: Option<Receiver<Failure>>,
    // Requests wake the worker for a fresh read after failed health. Dropping
    // the sender cancels its idle wait. A stalled OS read cannot be cancelled,
    // but never extends the foreground diagnostic budget.
    wake: Option<Sender<()>>,
}

impl Observer {
    pub(super) fn start(home: &Path, status: &DaemonStatus, deadline: LifecycleDeadline) -> Self {
        let home = home.to_path_buf();
        let expected = Failure::ALL.map(|failure| failure.record(status));
        Self::with_reader(
            move || read_failure(&home, &expected).ok().flatten(),
            deadline,
        )
    }

    fn with_reader(
        mut read: impl FnMut() -> Option<Failure> + Send + 'static,
        deadline: LifecycleDeadline,
    ) -> Self {
        let (failure_tx, failure_rx) = mpsc::sync_channel(1);
        let (wake_tx, wake_rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("coven-startup-failure".to_owned())
            .spawn(move || {
                while Instant::now() < deadline.instant {
                    if matches!(wake_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                        return;
                    }
                    if let Some(failure) = read() {
                        let _ = failure_tx.send(failure);
                        return;
                    }
                    let remaining = deadline.instant.saturating_duration_since(Instant::now());
                    if matches!(
                        wake_rx.recv_timeout(remaining.min(OBSERVATION_INTERVAL)),
                        Err(mpsc::RecvTimeoutError::Disconnected)
                    ) {
                        return;
                    }
                }
            });
        if worker.is_err() {
            return Self {
                failure: None,
                wake: None,
            };
        }
        Self {
            failure: Some(failure_rx),
            wake: Some(wake_tx),
        }
    }

    pub(super) fn enrich<T>(self, result: Result<T>, deadline: LifecycleDeadline) -> Result<T> {
        result.map_err(|error| {
            let Some(receiver) = &self.failure else {
                return error;
            };
            // A completed observation is useful even if health consumed the
            // entire budget. Never add a diagnostic grace period after it.
            let failure = receiver.try_recv().ok().or_else(|| {
                let remaining = deadline.instant.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return None;
                }
                // Do not spend the entire collection budget waiting for the
                // worker's equally long polling interval to elapse. The failure
                // record precedes startup error unwinding; request a fresh read.
                if let Some(wake) = &self.wake {
                    let _ = wake.send(());
                }
                receiver
                    .recv_timeout(remaining.min(OBSERVATION_INTERVAL))
                    .ok()
            });
            match failure {
                Some(failure) => error.context(failure.guidance()),
                None => error,
            }
        })
    }
}

fn read_failure(home: &Path, expected: &[String; 2]) -> io::Result<Option<Failure>> {
    let path = daemon_recovery_log_path(home);
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Ok(None);
    }
    let offset = metadata.len().saturating_sub(MAX_LOG_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
    // Ignore a potentially partial first line when the tail truncates history.
    for line in String::from_utf8_lossy(&bytes)
        .lines()
        .skip(usize::from(offset > 0))
    {
        let Some((_, message)) = line.split_once("] ") else {
            continue;
        };
        for (index, expected) in expected.iter().enumerate() {
            if message == expected {
                return Ok(Some(Failure::ALL[index]));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn status(pid: u32, attempt: &str) -> DaemonStatus {
        DaemonStatus {
            pid,
            started_at: attempt.to_owned(),
            socket: "private-path-must-not-leak".to_owned(),
            process_creation_time: None,
        }
    }

    #[test]
    fn startup_failure_matches_only_exact_attempt_and_fixed_codes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let selected = status(123, "selected-attempt");
        let expected = Failure::ALL.map(|failure| failure.record(&selected));
        for message in [
            Failure::MobileConfiguration.record(&status(456, "selected-attempt")),
            Failure::MobileConfiguration.record(&status(123, "old-attempt")),
            expected[0].replace("mobile-configuration", "unknown-code"),
            format!("{} secret-private-value", expected[0]),
            "startup_checkpoint phase=mobile-gateway-end elapsed_ms=10".to_owned(),
        ] {
            fs::write(
                daemon_recovery_log_path(temp.path()),
                format!("[synthetic] {message}\n"),
            )?;
            assert_eq!(read_failure(temp.path(), &expected)?, None);
        }
        for failure in Failure::ALL {
            fs::write(
                daemon_recovery_log_path(temp.path()),
                format!("[synthetic] {}\n", failure.record(&selected)),
            )?;
            assert_eq!(read_failure(temp.path(), &expected)?, Some(failure));
        }
        Ok(())
    }

    #[test]
    fn startup_failure_records_never_copy_private_input_or_original_errors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let selected = status(123, "private-identity\n[forged] new-record");
        let error =
            anyhow::anyhow!("credential-and-private-path").context(MobileConfigValidationFailure);
        record(temp.path(), &selected, &error);
        let log = fs::read_to_string(daemon_recovery_log_path(temp.path()))?;
        assert_eq!(log.lines().count(), 1);
        for private in [
            "private-identity",
            "forged",
            "credential-and-private-path",
            &selected.socket,
        ] {
            assert!(
                !log.contains(private),
                "private input escaped classification"
            );
        }
        assert!(log.contains("code=mobile-configuration"));
        Ok(())
    }

    #[test]
    fn startup_failure_reads_only_recent_regular_log() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let selected = status(123, "attempt");
        let expected = Failure::ALL.map(|failure| failure.record(&selected));
        let path = daemon_recovery_log_path(temp.path());
        fs::write(
            &path,
            format!(
                "[old] {}\n{}",
                expected[0],
                "x".repeat(MAX_LOG_BYTES as usize)
            ),
        )?;
        assert_eq!(read_failure(temp.path(), &expected)?, None);
        fs::remove_file(&path)?;
        fs::create_dir(&path)?;
        assert_eq!(read_failure(temp.path(), &expected)?, None);
        fs::remove_dir(&path)?;
        assert!(read_failure(temp.path(), &expected).is_err());
        #[cfg(unix)]
        {
            let other = temp.path().join("other");
            fs::write(&other, format!("[other] {}\n", expected[0]))?;
            std::os::unix::fs::symlink(&other, &path)?;
            assert_eq!(read_failure(temp.path(), &expected)?, None);
            fs::remove_file(&path)?;
            use std::os::unix::ffi::OsStrExt;
            let fifo = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            assert_eq!(read_failure(temp.path(), &expected)?, None);
        }
        Ok(())
    }

    #[test]
    fn startup_failure_collection_cannot_delay_success_or_extend_expired_deadline() -> Result<()> {
        for healthy in [true, false] {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let (finished_tx, finished_rx) = mpsc::channel();
            let deadline = LifecycleDeadline::after(Duration::from_secs(30))?;
            let observer = Observer::with_reader(
                move || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    finished_tx.send(()).unwrap();
                    Some(Failure::MobileConfiguration)
                },
                deadline,
            );
            entered_rx.recv_timeout(Duration::from_secs(30))?;
            // The producer cannot finish until after enrich returns: no timing
            // threshold or scheduler race is needed to prove independence.
            let result = if healthy {
                Ok(42)
            } else {
                Err(anyhow::anyhow!("original failure"))
            };
            let result = observer.enrich(
                result,
                if healthy {
                    deadline
                } else {
                    LifecycleDeadline::from_instant(Instant::now())
                },
            );
            release_tx.send(())?;
            finished_rx.recv_timeout(Duration::from_secs(30))?;
            if healthy {
                assert_eq!(result?, 42);
            } else {
                assert_eq!(result.unwrap_err().to_string(), "original failure");
            }
        }
        Ok(())
    }

    #[test]
    fn startup_failure_is_context_only_and_preserves_original_cause() -> Result<()> {
        for healthy in [true, false] {
            let (tx, rx) = mpsc::sync_channel(1);
            tx.send(Failure::MobileConfiguration)?;
            let observer = Observer {
                failure: Some(rx),
                wake: None,
            };
            let result = if healthy {
                Ok(42)
            } else {
                Err(anyhow::anyhow!("original failure"))
            };
            let result = observer.enrich(result, LifecycleDeadline::from_instant(Instant::now()));
            if healthy {
                assert_eq!(result?, 42);
            } else {
                let error = result.unwrap_err();
                assert!(error.to_string().contains("mobile/gateway.json"));
                assert_eq!(error.root_cause().to_string(), "original failure");
            }
        }
        Ok(())
    }

    #[test]
    fn startup_failure_requests_observation_only_after_failed_health() {
        for healthy in [true, false] {
            let (_failure_tx, failure_rx) = mpsc::sync_channel(1);
            let (wake_tx, wake_rx) = mpsc::channel();
            let observer = Observer {
                failure: Some(failure_rx),
                wake: Some(wake_tx),
            };
            let result = if healthy {
                Ok(42)
            } else {
                Err(anyhow::anyhow!("original failure"))
            };
            let result = observer.enrich(
                result,
                LifecycleDeadline::after(Duration::from_secs(30)).unwrap(),
            );
            // Inspect the request after the foreground call releases its sender;
            // no scheduling or elapsed-time threshold selects the outcome.
            if healthy {
                assert_eq!(result.unwrap(), 42);
                assert_eq!(wake_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
            } else {
                assert_eq!(result.unwrap_err().to_string(), "original failure");
                assert_eq!(wake_rx.try_recv(), Ok(()));
            }
        }
    }

    #[test]
    fn startup_failure_wake_requests_another_read_instead_of_cancelling() -> Result<()> {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (observed_tx, observed_rx) = mpsc::channel();
        let mut first = true;
        let observer = Observer::with_reader(
            move || {
                if first {
                    first = false;
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    None
                } else {
                    observed_tx.send(()).unwrap();
                    Some(Failure::MobileConfiguration)
                }
            },
            LifecycleDeadline::after(Duration::from_secs(30))?,
        );
        entered_rx.recv_timeout(Duration::from_secs(30))?;
        // Queue the request during the first read. It must trigger a new read
        // when the worker reaches its idle wait, rather than terminate it.
        observer.wake.as_ref().expect("observer worker").send(())?;
        release_tx.send(())?;
        observed_rx.recv_timeout(Duration::from_secs(30))?;
        assert_eq!(
            observer
                .failure
                .as_ref()
                .expect("observation receiver")
                .recv_timeout(Duration::from_secs(30))?,
            Failure::MobileConfiguration
        );
        Ok(())
    }

    #[test]
    fn startup_failure_observer_cancels_without_repeating_reads() -> Result<()> {
        struct Finished(Sender<()>);
        impl Drop for Finished {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let finished = Finished(finished_tx);
        let reads = Arc::new(AtomicUsize::new(0));
        let worker_reads = Arc::clone(&reads);
        let observer = Observer::with_reader(
            move || {
                let _keep = &finished;
                worker_reads.fetch_add(1, Ordering::SeqCst);
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                None
            },
            LifecycleDeadline::after(Duration::from_secs(30))?,
        );
        entered_rx.recv_timeout(Duration::from_secs(30))?;
        drop(observer);
        release_tx.send(())?;
        finished_rx.recv_timeout(Duration::from_secs(30))?;
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        Ok(())
    }
}
