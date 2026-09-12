#[cfg(feature = "threads-test-clock")]
use std::fs;
use std::path::Path;
#[cfg(feature = "threads-test-clock")]
use std::path::PathBuf;
#[cfg(feature = "threads-test-clock")]
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "threads-test-clock")]
use anyhow::Context;
use anyhow::Result;
use time::OffsetDateTime;

#[cfg(feature = "threads-test-clock")]
const FIXTURE_ROOT_DIRECTORY: &str = "test-fixtures";
#[cfg(feature = "threads-test-clock")]
const FIXTURE_DIRECTORY_NAME: &str = "threads-deterministic-clock";
#[cfg(feature = "threads-test-clock")]
const ACTIVATION_FILE: &str = "enabled";
#[cfg(feature = "threads-test-clock")]
const ACTIVATION_SENTINEL: &str = "threads_test_clock_v1";
#[cfg(feature = "threads-test-clock")]
const CAPABILITY_FILE: &str = "capability";
#[cfg(feature = "threads-test-clock")]
const STATE_FILE: &str = "state.json";
#[cfg(feature = "threads-test-clock")]
const FINAL_COMMIT_PAUSE_FILE: &str = "pause-final-commit";
#[cfg(feature = "threads-test-clock")]
const FINAL_COMMIT_PAUSE_REACHED_FILE: &str = "pause-final-commit.reached";
#[cfg(feature = "threads-test-clock")]
const FINAL_COMMIT_PAUSE_REACHED_SENTINEL: &str = "threads_test_final_commit_pause_reached_v1";
#[cfg(feature = "threads-test-clock")]
const FINAL_COMMIT_PAUSE_RELEASE_FILE: &str = "pause-final-commit.release";
#[cfg(feature = "threads-test-clock")]
const FINAL_COMMIT_PAUSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockSource {
    WallClock,
    #[cfg(feature = "threads-test-clock")]
    DeterministicFixture,
}

#[cfg(feature = "threads-test-clock")]
impl ClockSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::WallClock => "wall_clock",
            Self::DeterministicFixture => "deterministic_fixture",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockSnapshot {
    pub(crate) now: OffsetDateTime,
    pub(crate) source: ClockSource,
}

pub(crate) fn now(coven_home: &Path) -> Result<OffsetDateTime> {
    snapshot(coven_home).map(|snapshot| snapshot.now)
}

pub(crate) fn snapshot(coven_home: &Path) -> Result<ClockSnapshot> {
    #[cfg(feature = "threads-test-clock")]
    if let Some(fixture) = ActiveFixture::load(coven_home)? {
        return Ok(ClockSnapshot {
            now: fixture.state.now,
            source: ClockSource::DeterministicFixture,
        });
    }

    let _ = coven_home;
    Ok(ClockSnapshot {
        now: OffsetDateTime::now_utc(),
        source: ClockSource::WallClock,
    })
}

#[cfg(feature = "threads-test-clock")]
#[derive(Debug)]
pub(crate) struct InactiveFixture;

#[cfg(feature = "threads-test-clock")]
impl std::fmt::Display for InactiveFixture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("deterministic Threads clock fixture is not active")
    }
}

#[cfg(feature = "threads-test-clock")]
impl std::error::Error for InactiveFixture {}

#[cfg(feature = "threads-test-clock")]
#[derive(Debug)]
pub(crate) struct InvalidFixtureCapability;

#[cfg(feature = "threads-test-clock")]
impl std::fmt::Display for InvalidFixtureCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("deterministic Threads clock capability was rejected")
    }
}

#[cfg(feature = "threads-test-clock")]
impl std::error::Error for InvalidFixtureCapability {}

#[cfg(feature = "threads-test-clock")]
#[derive(Debug)]
pub(crate) struct NonMonotonicFixtureTime {
    pub(crate) current: OffsetDateTime,
    pub(crate) requested: OffsetDateTime,
}

#[cfg(feature = "threads-test-clock")]
impl std::fmt::Display for NonMonotonicFixtureTime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "deterministic Threads clock cannot move backwards from {} to {}",
            self.current, self.requested
        )
    }
}

#[cfg(feature = "threads-test-clock")]
impl std::error::Error for NonMonotonicFixtureTime {}

#[cfg(feature = "threads-test-clock")]
pub(crate) fn authorize_fixture(coven_home: &Path, capability: &str) -> Result<ClockSnapshot> {
    let fixture = ActiveFixture::require(coven_home, capability)?;
    Ok(ClockSnapshot {
        now: fixture.state.now,
        source: ClockSource::DeterministicFixture,
    })
}

#[cfg(feature = "threads-test-clock")]
pub(crate) fn set_now(
    coven_home: &Path,
    capability: &str,
    requested: OffsetDateTime,
) -> Result<ClockSnapshot> {
    let _guard = fixture_control_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("deterministic Threads clock lock is poisoned"))?;
    let fixture = ActiveFixture::require(coven_home, capability)?;
    if requested < fixture.state.now {
        return Err(NonMonotonicFixtureTime {
            current: fixture.state.now,
            requested,
        }
        .into());
    }
    write_fixture_state(
        &fixture.state_path,
        &PersistedFixtureState { now: requested },
    )?;
    Ok(ClockSnapshot {
        now: requested,
        source: ClockSource::DeterministicFixture,
    })
}

#[cfg(feature = "threads-test-clock")]
pub(crate) fn fixture_mode_enabled(coven_home: &Path) -> Result<bool> {
    ActiveFixture::activation_requested(coven_home)
}

#[cfg(not(feature = "threads-test-clock"))]
pub(crate) fn fixture_mode_enabled(_coven_home: &Path) -> Result<bool> {
    Ok(false)
}

#[cfg(feature = "threads-test-clock")]
pub(crate) fn fixture_control_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(feature = "threads-test-clock")]
pub(crate) fn pause_final_commit_if_requested(coven_home: &Path) -> Result<()> {
    let Some(fixture) = ActiveFixture::load(coven_home)? else {
        return Ok(());
    };
    let fixture_dir = fixture_directory(coven_home);
    let pause_path = fixture_dir.join(FINAL_COMMIT_PAUSE_FILE);
    match fs::symlink_metadata(&pause_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading deterministic Threads final commit pause {}",
                    pause_path.display()
                )
            });
        }
    }
    crate::mobile_memory::config::validate_private_file(&pause_path)?;
    let requested = read_capability(&pause_path)?;
    if requested != fixture.capability {
        return Err(InvalidFixtureCapability.into());
    }

    let reached_path = fixture_dir.join(FINAL_COMMIT_PAUSE_REACHED_FILE);
    crate::mobile_memory::config::atomic_replace_private(
        &reached_path,
        format!("{FINAL_COMMIT_PAUSE_REACHED_SENTINEL}\n").as_bytes(),
    )
    .with_context(|| {
        format!(
            "recording deterministic Threads final commit pause {}",
            reached_path.display()
        )
    })?;
    let release_path = fixture_dir.join(FINAL_COMMIT_PAUSE_RELEASE_FILE);
    let deadline = std::time::Instant::now() + FINAL_COMMIT_PAUSE_TIMEOUT;
    loop {
        match fs::symlink_metadata(&release_path) {
            Ok(_) => {
                crate::mobile_memory::config::validate_private_file(&release_path)?;
                let release = read_capability(&release_path)?;
                if release != fixture.capability {
                    return Err(InvalidFixtureCapability.into());
                }
                let _ = fs::remove_file(&pause_path);
                let _ = fs::remove_file(&release_path);
                let _ = fs::remove_file(&reached_path);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "deterministic Threads final commit pause timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "reading deterministic Threads final commit release {}",
                        release_path.display()
                    )
                });
            }
        }
    }
}

#[cfg(feature = "threads-test-clock")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedFixtureState {
    #[serde(with = "time::serde::rfc3339")]
    now: OffsetDateTime,
}

#[cfg(feature = "threads-test-clock")]
struct ActiveFixture {
    capability: String,
    state_path: PathBuf,
    state: PersistedFixtureState,
}

#[cfg(feature = "threads-test-clock")]
impl ActiveFixture {
    fn load(coven_home: &Path) -> Result<Option<Self>> {
        if !Self::activation_requested(coven_home)? {
            return Ok(None);
        }

        validate_fixture_directories(coven_home)?;
        let capability_path = fixture_directory(coven_home).join(CAPABILITY_FILE);
        match fs::symlink_metadata(&capability_path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(error).with_context(|| {
                    format!(
                        "deterministic Threads clock is active but missing {}",
                        capability_path.display()
                    )
                });
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "reading deterministic Threads clock capability {}",
                        capability_path.display()
                    )
                });
            }
        }
        crate::mobile_memory::config::validate_private_file(&capability_path)?;
        let capability = read_capability(&capability_path)?;
        let state_path = fixture_directory(coven_home).join(STATE_FILE);
        crate::mobile_memory::config::validate_private_file(&state_path)?;
        let state = read_fixture_state(&state_path)?;
        Ok(Some(Self {
            capability,
            state_path,
            state,
        }))
    }

    fn activation_requested(coven_home: &Path) -> Result<bool> {
        let activation_path = fixture_directory(coven_home).join(ACTIVATION_FILE);
        match fs::symlink_metadata(&activation_path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "reading deterministic Threads clock activation {}",
                        activation_path.display()
                    )
                });
            }
        }
        validate_fixture_directories(coven_home)?;
        crate::mobile_memory::config::validate_private_file(&activation_path)?;
        read_activation_marker(&activation_path)?;
        Ok(true)
    }

    fn require(coven_home: &Path, capability: &str) -> Result<Self> {
        let fixture = Self::load(coven_home)?.ok_or(InactiveFixture)?;
        if fixture.capability != capability {
            return Err(InvalidFixtureCapability.into());
        }
        Ok(fixture)
    }
}

#[cfg(feature = "threads-test-clock")]
fn fixture_root_directory(coven_home: &Path) -> PathBuf {
    coven_home.join(FIXTURE_ROOT_DIRECTORY)
}

#[cfg(feature = "threads-test-clock")]
fn fixture_directory(coven_home: &Path) -> PathBuf {
    fixture_root_directory(coven_home).join(FIXTURE_DIRECTORY_NAME)
}

#[cfg(feature = "threads-test-clock")]
fn validate_fixture_directories(coven_home: &Path) -> Result<()> {
    crate::mobile_memory::config::validate_private_directory(&fixture_root_directory(coven_home))?;
    crate::mobile_memory::config::validate_private_directory(&fixture_directory(coven_home))
}

#[cfg(feature = "threads-test-clock")]
fn read_activation_marker(path: &Path) -> Result<()> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading deterministic Threads clock {}", path.display()))?;
    let marker = raw.trim_end_matches(['\r', '\n']);
    anyhow::ensure!(
        marker == ACTIVATION_SENTINEL,
        "deterministic Threads clock activation marker is invalid"
    );
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn read_capability(path: &Path) -> Result<String> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading deterministic Threads clock {}", path.display()))?;
    let capability = raw.trim_end_matches(['\r', '\n']);
    anyhow::ensure!(
        !capability.is_empty() && !capability.chars().any(char::is_control),
        "deterministic Threads clock capability is invalid"
    );
    Ok(capability.to_string())
}

#[cfg(feature = "threads-test-clock")]
fn read_fixture_state(path: &Path) -> Result<PersistedFixtureState> {
    let raw = fs::read(path)
        .with_context(|| format!("reading deterministic Threads clock {}", path.display()))?;
    serde_json::from_slice(&raw)
        .with_context(|| format!("parsing deterministic Threads clock {}", path.display()))
}

#[cfg(feature = "threads-test-clock")]
fn write_fixture_state(path: &Path, state: &PersistedFixtureState) -> Result<()> {
    let encoded =
        serde_json::to_vec_pretty(state).context("serializing deterministic Threads clock")?;
    crate::mobile_memory::config::atomic_replace_private(path, &encoded)
}

#[cfg(all(test, feature = "threads-test-clock"))]
pub(crate) fn seed_fixture_for_tests(
    coven_home: &Path,
    capability: &str,
    now: OffsetDateTime,
) -> Result<()> {
    seed_fixture_for_tests_with_activation(coven_home, capability, now, true)
}

#[cfg(all(test, feature = "threads-test-clock"))]
fn seed_fixture_for_tests_with_activation(
    coven_home: &Path,
    capability: &str,
    now: OffsetDateTime,
    activated: bool,
) -> Result<()> {
    let fixture_root = fixture_root_directory(coven_home);
    let fixture_dir = fixture_directory(coven_home);
    fs::create_dir_all(&fixture_dir).with_context(|| {
        format!(
            "creating deterministic Threads clock {}",
            fixture_dir.display()
        )
    })?;
    secure_fixture_directory(&fixture_root)?;
    secure_fixture_directory(&fixture_dir)?;
    let activation_path = fixture_dir.join(ACTIVATION_FILE);
    let capability_path = fixture_dir.join(CAPABILITY_FILE);
    let state_path = fixture_dir.join(STATE_FILE);
    let state = PersistedFixtureState { now };
    let encoded =
        serde_json::to_vec_pretty(&state).context("serializing deterministic Threads clock")?;
    if activated {
        let _ = crate::mobile_memory::config::atomic_create_private(
            &activation_path,
            format!("{ACTIVATION_SENTINEL}\n").as_bytes(),
        )?;
    }
    let _ = crate::mobile_memory::config::atomic_create_private(
        &capability_path,
        capability.as_bytes(),
    )?;
    let _ = crate::mobile_memory::config::atomic_create_private(&state_path, &encoded)?;
    write_fixture_state(&state_path, &state)
}

#[cfg(all(test, feature = "threads-test-clock"))]
fn secure_fixture_directory(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing deterministic Threads clock {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_snapshot_uses_wall_clock() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let snapshot = snapshot(temp.path())?;
        assert_eq!(snapshot.source, ClockSource::WallClock);
        Ok(())
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn fixture_state_accepts_documented_rfc3339_time() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let initial = time::OffsetDateTime::parse(
            "2026-09-09T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )?;
        seed_fixture_for_tests(home, "fixture-cap", initial)?;
        crate::mobile_memory::config::atomic_replace_private(
            &fixture_directory(home).join(STATE_FILE),
            br#"{"now":"2026-09-09T10:00:00Z"}"#,
        )?;
        assert_eq!(now(home)?, initial);
        Ok(())
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn malformed_fixture_state_fails_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_fixture_for_tests(
            home,
            "cap-1",
            time::OffsetDateTime::parse(
                "2026-09-09T10:00:00Z",
                &time::format_description::well_known::Rfc3339,
            )?,
        )?;
        crate::mobile_memory::config::atomic_replace_private(
            &fixture_directory(home).join(STATE_FILE),
            br#"{"now":"not-a-time"}"#,
        )?;

        let error = now(home).expect_err("malformed state must fail closed");
        assert!(
            format!("{error:#}").contains("parsing deterministic Threads clock"),
            "got {error:#}"
        );
        Ok(())
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn fixture_requires_explicit_activation_marker() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let initial = time::OffsetDateTime::parse(
            "2026-09-09T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )?;
        seed_fixture_for_tests_with_activation(home, "cap-0", initial, false)?;

        let snapshot = snapshot(home)?;
        assert_eq!(snapshot.source, ClockSource::WallClock);
        assert_ne!(snapshot.now, initial);
        Ok(())
    }

    #[cfg(all(feature = "threads-test-clock", unix))]
    #[test]
    fn fixture_rejects_insecure_directory_chain() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let initial = time::OffsetDateTime::parse(
            "2026-09-09T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )?;
        seed_fixture_for_tests(home, "cap-3", initial)?;

        let fixture_root = fixture_root_directory(home);
        fs::set_permissions(&fixture_root, fs::Permissions::from_mode(0o755))?;
        let error = snapshot(home).expect_err("insecure fixture root must be rejected");
        assert!(
            format!("{error:#}").contains("insecure permissions"),
            "got {error:#}"
        );
        Ok(())
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn fixture_rejects_non_monotonic_updates_and_persists_monotonic_time() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let initial = time::OffsetDateTime::parse(
            "2026-09-09T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )?;
        seed_fixture_for_tests(home, "cap-2", initial)?;

        let advanced = set_now(home, "cap-2", initial + time::Duration::minutes(5))?;
        assert_eq!(advanced.now, initial + time::Duration::minutes(5));
        assert_eq!(advanced.source, ClockSource::DeterministicFixture);
        assert_eq!(now(home)?, initial + time::Duration::minutes(5));

        let error = set_now(home, "cap-2", initial + time::Duration::minutes(4))
            .expect_err("time must not move backwards");
        let non_monotonic = error
            .downcast_ref::<NonMonotonicFixtureTime>()
            .expect("non-monotonic error type");
        assert_eq!(non_monotonic.current, initial + time::Duration::minutes(5));
        assert_eq!(
            non_monotonic.requested,
            initial + time::Duration::minutes(4)
        );
        Ok(())
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn final_commit_pause_requires_active_fixture_capability() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let initial = time::OffsetDateTime::parse(
            "2026-09-09T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )?;
        seed_fixture_for_tests(home, "cap-4", initial)?;
        let fixture_dir = fixture_directory(home);
        crate::mobile_memory::config::atomic_create_private(
            &fixture_dir.join(FINAL_COMMIT_PAUSE_FILE),
            b"cap-4",
        )?;
        crate::mobile_memory::config::atomic_create_private(
            &fixture_dir.join(FINAL_COMMIT_PAUSE_RELEASE_FILE),
            b"cap-4",
        )?;

        pause_final_commit_if_requested(home)?;

        assert!(!fixture_dir.join(FINAL_COMMIT_PAUSE_FILE).exists());
        assert!(!fixture_dir.join(FINAL_COMMIT_PAUSE_RELEASE_FILE).exists());
        assert!(!fixture_dir.join(FINAL_COMMIT_PAUSE_REACHED_FILE).exists());

        crate::mobile_memory::config::atomic_create_private(
            &fixture_dir.join(FINAL_COMMIT_PAUSE_FILE),
            b"wrong-cap",
        )?;
        let error = pause_final_commit_if_requested(home)
            .expect_err("pause control must require the active fixture capability");
        assert!(
            error.downcast_ref::<InvalidFixtureCapability>().is_some(),
            "got {error:#}"
        );
        Ok(())
    }
}
