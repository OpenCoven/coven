use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MAX_LOG_BYTES: usize = 16 * 1024;

pub fn capture_startup_checkpoints(home: &Path) -> String {
    let home = home.to_path_buf();
    capture_with_reader(
        move || read_startup_checkpoints(&home),
        Duration::from_secs(1),
    )
}

fn capture_with_reader(
    read: impl FnOnce() -> io::Result<String> + Send + 'static,
    timeout: Duration,
) -> String {
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return "startup diagnostics unavailable: reader deadline overflowed".to_owned();
    };
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    if let Err(error) = std::thread::Builder::new()
        .name("coven-test-startup-log".to_owned())
        .spawn(move || {
            let _ = finished_tx.send(read());
        })
    {
        return format!("startup diagnostics reader unavailable: {:?}", error.kind());
    }
    match finished_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(captured)) => captured,
        Ok(Err(error)) => format!("startup diagnostics unavailable: {:?}", error.kind()),
        Err(mpsc::RecvTimeoutError::Timeout) => format!(
            "startup diagnostics unavailable: reader timed out after {}ms",
            timeout.as_millis()
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            "startup diagnostics unavailable: reader disconnected".to_owned()
        }
    }
}

fn read_startup_checkpoints(home: &Path) -> io::Result<String> {
    let path = home.join("daemon-recovery.log");
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        return Ok("startup diagnostics unavailable: log is not a regular file".to_owned());
    }
    let mut file = std::fs::File::open(path)?;
    let offset = file.metadata()?.len().saturating_sub(MAX_LOG_BYTES as u64);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BYTES as u64).read_to_end(&mut bytes)?;
    let mut captured = format!(
        "startup diagnostics: recent_bytes={} older_bytes_omitted={offset}; only fixed startup fields retained\n",
        bytes.len()
    );
    let mut observations = 0;
    for line in String::from_utf8_lossy(&bytes).lines() {
        if let Some(observation) = startup_observation(line) {
            captured.push_str(&observation);
            captured.push('\n');
            observations += 1;
        }
    }
    if observations == 0 {
        captured.push_str("no recognized startup observations\n");
    }
    Ok(captured)
}

fn startup_observation(line: &str) -> Option<String> {
    let (_, message) = line.split_once("] ")?;
    let mut fields = message.split_whitespace();
    let kind = fields.next()?;
    let phase = fields.next()?.strip_prefix("phase=")?;
    let (measurement, value) = fields.next()?.split_once('=')?;
    let observer = fields.next();
    if fields.next().is_some() {
        return None;
    }
    // Allowlist producer fields rather than copying paths or arbitrary log text.
    let recognized = match (kind, measurement) {
        ("startup_budget", "remaining_ms") => matches!(phase, "before-spawn" | "after-spawn"),
        ("startup_checkpoint", "elapsed_ms") => matches!(
            phase,
            "daemon-store-begin"
                | "store-initialize-begin"
                | "store-connection-configured"
                | "store-ward-complete"
                | "store-runtime-complete"
                | "store-main-lock-acquired"
                | "store-main-schema-complete"
                | "store-commit-complete"
                | "store-initialize-end"
                | "store-close-begin"
                | "daemon-store-end"
                | "status-publication-begin"
                | "status-publication-end"
        ),
        _ => false,
    };
    let value = value.parse::<u128>().ok()?;
    let mut record = recognized.then(|| format!("{kind} phase={phase} {measurement}={value}"))?;
    if let Some(observer) = observer {
        let observer = observer
            .strip_prefix("prior_observer_ms=")?
            .parse::<u128>()
            .ok()?;
        if kind != "startup_checkpoint" || observer > value {
            return None;
        }
        record.push_str(&format!(" prior_observer_ms={observer}"));
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn startup_failure_checkpoints_preserve_timings_without_private_log_text() -> io::Result<()> {
        let home = tempfile::tempdir()?;
        fs::write(
            home.path().join("daemon-recovery.log"),
            format!(
                "[synthetic] startup_budget phase=before-spawn remaining_ms=4900\n\
                 [synthetic] startup_budget phase=after-spawn remaining_ms=4800\n\
                 [synthetic] startup_checkpoint phase=store-runtime-complete elapsed_ms=3011\n\
                 [synthetic] startup_checkpoint phase=store-close-begin elapsed_ms=3012\n\
                 [synthetic] startup_checkpoint phase=store-initialize-end elapsed_ms=5300 prior_observer_ms=5100\n\
                 [synthetic] startup_checkpoint phase=store-close-begin elapsed_ms=500 prior_observer_ms=999999\n\
                 [synthetic] startup_checkpoint phase=store-close-begin elapsed_ms=500 prior_observer_ms=synthetic-private-value\n\
                 [synthetic] startup_checkpoint phase=store-close-begin elapsed_ms=500 prior_observer_ms=100 extra=synthetic-private-value\n\
                 [synthetic] private fixture path={} synthetic-private-value\n\
                 [synthetic] startup_checkpoint phase=synthetic-private-value elapsed_ms=1\n\
                 [synthetic] startup_budget phase=before-spawn remaining_ms=synthetic-private-value\n\
                 [synthetic] startup_budget phase=after-spawn remaining_ms=1 extra=synthetic-private-value\n",
                home.path().display()
            ),
        )?;
        let captured = capture_startup_checkpoints(home.path());
        assert!(captured.contains("startup_budget phase=before-spawn remaining_ms=4900"));
        assert!(captured.contains("startup_budget phase=after-spawn remaining_ms=4800"));
        assert!(
            captured.contains("startup_checkpoint phase=store-runtime-complete elapsed_ms=3011")
        );
        assert!(captured.contains("startup_checkpoint phase=store-close-begin elapsed_ms=3012"));
        assert!(captured.contains(
            "startup_checkpoint phase=store-initialize-end elapsed_ms=5300 prior_observer_ms=5100"
        ));
        assert!(!captured.contains("prior_observer_ms=999999"), "{captured}");
        assert!(!captured.contains("synthetic-private-value"), "{captured}");
        assert!(
            !captured.contains(&home.path().display().to_string()),
            "{captured}"
        );
        Ok(())
    }

    #[test]
    fn startup_failure_checkpoints_read_only_the_bounded_recent_tail() -> io::Result<()> {
        let home = tempfile::tempdir()?;
        let log = format!(
            "[synthetic] startup_budget phase=before-spawn remaining_ms=9999\n{}\n\
             [synthetic] startup_checkpoint phase=daemon-store-end elapsed_ms=3012\n",
            "x".repeat(MAX_LOG_BYTES * 2)
        );
        fs::write(home.path().join("daemon-recovery.log"), log)?;
        let captured = capture_startup_checkpoints(home.path());
        assert!(captured.contains("older_bytes_omitted="), "{captured}");
        assert!(captured.contains("startup_checkpoint phase=daemon-store-end elapsed_ms=3012"));
        assert!(!captured.contains("remaining_ms=9999"));
        assert!(captured.len() < MAX_LOG_BYTES);
        Ok(())
    }

    #[test]
    fn startup_failure_checkpoints_report_missing_or_nonregular_logs() -> io::Result<()> {
        let home = tempfile::tempdir()?;
        assert!(capture_startup_checkpoints(home.path()).contains("NotFound"));
        fs::create_dir(home.path().join("daemon-recovery.log"))?;
        assert!(capture_startup_checkpoints(home.path()).contains("not a regular file"));
        Ok(())
    }

    #[test]
    fn startup_failure_checkpoints_do_not_invent_unavailable_timings() -> io::Result<()> {
        let home = tempfile::tempdir()?;
        fs::write(
            home.path().join("daemon-recovery.log"),
            "unrelated log entry\n",
        )?;
        let captured = capture_startup_checkpoints(home.path());
        assert!(
            captured.contains("no recognized startup observations"),
            "{captured}"
        );
        Ok(())
    }

    #[test]
    fn startup_failure_checkpoints_do_not_wait_for_a_stalled_reader() {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let captured = capture_with_reader(
            move || {
                release_rx.recv().unwrap();
                finished_tx.send(()).unwrap();
                Ok("synthetic late diagnostics".to_owned())
            },
            Duration::ZERO,
        );
        assert!(captured.contains("timed out"), "{captured}");
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}
