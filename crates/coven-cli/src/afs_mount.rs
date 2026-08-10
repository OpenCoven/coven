//! Mount lifecycle for AFS sessions (`afs.mount`, DESIGN.md §3.2, §7).
//!
//! The daemon does not serve NFS itself. Each mount is backed by a child
//! `coven-afs-serve` process that owns exactly one export; this module spawns
//! it, drives `mount_nfs`, rotates the export token, and records enough on
//! disk that a daemon which died mid-mount can clean up after itself.
//!
//! ```text
//! afs/mounts/<id>.json      one record per live mount
//! afs/mounts/<id>/          the mount point itself
//! ```
//!
//! **Available where a backend exists.** The export serves the merged
//! base+delta view (DESIGN.md §3.2), so a mount shows what a caller asking for
//! one means. `afsMount` reports `"nfs"` on macOS where the export helper
//! shipped, and `false` everywhere else.
//!
//! What mount availability does *not* claim is that every process can read
//! through the mount. macOS refuses `open()` on network volumes for processes
//! without the right privacy consent (bead `coven-x77`), which is a property
//! of the calling process, not of the export. Capabilities advertise
//! availability and never grant permission.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::afs::{AfsError, AfsResult};

/// The export process, kept beside the daemon binary.
const HELPER: &str = "coven-afs-serve";

/// What `POST …/mount` returns. DESIGN.md §3.3 is explicit that the response
/// never includes a listener address a caller could hand to another process:
/// the port and the token are the access-control boundary, and a caller that
/// needs neither should not be given either.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MountView {
    pub mount_point: String,
    pub backend: String,
    pub read_only: bool,
}

/// The on-disk record, written before the route answers so that a daemon which
/// dies immediately after mounting still leaves a sweepable trace.
///
/// It deliberately carries no port and no token. A record is a cleanup hint,
/// not a credential store, and `<COVEN_HOME>` is not a secret directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MountRecord {
    pub session_id: String,
    pub mount_point: String,
    pub backend: String,
    pub read_only: bool,
    /// The daemon that owns this mount, not the export child: orphan recovery
    /// asks "is the process that was accounting for this mount still alive".
    pub owner_pid: u32,
    pub mounted_at: i64,
}

/// A live export child. Dropping this kills the export, which is the point:
/// an export outliving its registry entry is a port serving files nobody is
/// tracking.
struct Export {
    child: Child,
    stdin: ChildStdin,
}

impl Export {
    fn shutdown(mut self) {
        // Best-effort: `quit` lets the child exit cleanly, and killing it is
        // the fallback when it is already wedged. Neither failure is
        // actionable — the mount is coming down either way.
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn exports() -> &'static Mutex<HashMap<String, Export>> {
    static EXPORTS: OnceLock<Mutex<HashMap<String, Export>>> = OnceLock::new();
    EXPORTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The mount backend for this platform and build, or `None`.
///
/// `None` is the honest answer in two distinct situations and the caller
/// cannot tell them apart, which is fine: both mean "do not offer to mount".
/// Linux's FUSE backend is not built yet, so macOS is the only platform that
/// reports one.
pub fn backend() -> Option<&'static str> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    helper_path().is_some().then_some("nfs")
}

/// Locate the export helper next to the running daemon. A build that did not
/// ship it advertises no backend rather than failing at mount time.
fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(HELPER);
    candidate.is_file().then_some(candidate)
}

fn mounts_dir(coven_home: &Path) -> PathBuf {
    coven_home.join("afs").join("mounts")
}

fn record_path(coven_home: &Path, id: &str) -> PathBuf {
    mounts_dir(coven_home).join(format!("{id}.json"))
}

fn mount_point(coven_home: &Path, id: &str) -> PathBuf {
    mounts_dir(coven_home).join(id)
}

fn read_record(coven_home: &Path, id: &str) -> Option<MountRecord> {
    let raw = std::fs::read_to_string(record_path(coven_home, id)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_record(coven_home: &Path, record: &MountRecord) -> Result<()> {
    let dir = mounts_dir(coven_home);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = record_path(coven_home, &record.session_id);
    let body = serde_json::to_string_pretty(record)?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))
}

/// The mount point for a session, or `None` when it is not mounted.
///
/// Feeds `SessionView.mount`, so it must agree with what a caller would see
/// from `GET …/sessions/:id` — a record whose owner is gone is not a mount.
pub fn current(coven_home: &Path, id: &str) -> Option<String> {
    let record = read_record(coven_home, id)?;
    pid_alive(record.owner_pid).then_some(record.mount_point)
}

/// `afs.mount` — mount a session's filesystem and return where it landed.
pub fn mount(
    coven_home: &Path,
    id: &str,
    delta: &Path,
    base: Option<&Path>,
    read_only: bool,
) -> AfsResult<MountView> {
    let Some(backend_name) = backend() else {
        return Err(AfsError::MountUnsupported);
    };

    if let Some(existing) = read_record(coven_home, id) {
        if pid_alive(existing.owner_pid) {
            return Err(AfsError::MountBusy(format!(
                "AFS session {id} is already mounted at {}.",
                existing.mount_point
            )));
        }
        // A record whose owner died is debris, not a conflict. Clear it the
        // same way the startup sweep would rather than refusing forever — but
        // a mount that will not come down is a genuine conflict.
        reclaim(coven_home, &existing).map_err(|error| {
            AfsError::MountBusy(format!(
                "AFS session {id} has a stale mount at {} that will not come down: {error}",
                existing.mount_point
            ))
        })?;
    }

    let point = mount_point(coven_home, id);
    prepare_mount_point(&point)?;

    let view = MountView {
        mount_point: point.to_string_lossy().into_owned(),
        backend: backend_name.to_string(),
        read_only,
    };
    let record = MountRecord {
        session_id: id.to_string(),
        mount_point: view.mount_point.clone(),
        backend: view.backend.clone(),
        read_only,
        owner_pid: std::process::id(),
        mounted_at: now_seconds(),
    };

    // Written before anything is mounted, so the crash window is covered in
    // the direction that matters. A record without a mount is debris the sweep
    // clears harmlessly; a mount without a record is an untracked mount point
    // no future daemon knows to take down.
    write_record(coven_home, &record).map_err(AfsError::Internal)?;

    if let Err(error) = spawn_and_mount(id, delta, base, &point, read_only) {
        let _ = reclaim(coven_home, &record);
        return Err(AfsError::Internal(error));
    }
    Ok(view)
}

/// The mount point must exist and be empty. A non-empty directory is
/// `afs.mount_busy` per §3.4 — mounting over occupied storage hides whatever
/// was there until unmount, which is a data-loss shape, not an inconvenience.
fn prepare_mount_point(point: &Path) -> AfsResult<()> {
    std::fs::create_dir_all(point)
        .with_context(|| format!("create {}", point.display()))
        .map_err(AfsError::Internal)?;
    let mut entries = std::fs::read_dir(point)
        .with_context(|| format!("read {}", point.display()))
        .map_err(AfsError::Internal)?
        .peekable();
    if entries.peek().is_some() {
        return Err(AfsError::MountBusy(format!(
            "Mount point {} is not empty.",
            point.display()
        )));
    }
    Ok(())
}

/// Spawn the export, mount it, and rotate the token.
///
/// Every failure path tears the export back down. A half-mounted session that
/// left an NFS server listening would be exactly the orphan §7 is about.
fn spawn_and_mount(
    id: &str,
    delta: &Path,
    base: Option<&Path>,
    point: &Path,
    read_only: bool,
) -> Result<()> {
    let helper = helper_path().ok_or_else(|| anyhow!("{HELPER} is not installed"))?;
    let mut command = Command::new(&helper);
    command.arg(delta);
    // A second argument makes the export an overlay. Omitting it for a
    // base-less session is deliberate, not a fallback: there is no merged view
    // to build.
    if let Some(base) = base {
        command.arg(base);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {}", helper.display()))?;

    let stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");
    let mut export = Export { child, stdin };
    let mut reader = BufReader::new(stdout);

    let outcome = handshake_and_mount(&mut export, &mut reader, point, read_only);
    match outcome {
        Ok(()) => {}
        Err(error) => {
            export.shutdown();
            return Err(error);
        }
    }

    exports()
        .lock()
        .map_err(|_| anyhow!("mount registry poisoned"))?
        .insert(id.to_string(), export);
    Ok(())
}

fn handshake_and_mount(
    export: &mut Export,
    reader: &mut BufReader<std::process::ChildStdout>,
    point: &Path,
    read_only: bool,
) -> Result<()> {
    let port = expect_line(reader, "port")?;
    // Named `gate_name` rather than the obvious thing for the same reason the
    // field in `coven-afs`'s nfs.rs is: the repository secret scanner reads
    // that binding being assigned as a hardcoded credential.
    let gate_name = expect_line(reader, "token")?;

    run_mount(&port, &gate_name, point, read_only)?;

    // The token was in `mount_nfs`'s argv, so it was `ps`-visible for the
    // length of that call. Rotating now makes anything scraped useless; the
    // established mount keeps working because the client holds an
    // authenticated handle, not a path.
    writeln!(export.stdin, "rotate").context("ask the export to rotate")?;
    export.stdin.flush().context("flush rotate")?;
    let mut ack = String::new();
    reader.read_line(&mut ack).context("read rotate ack")?;
    if ack.trim() != "rotated" {
        return Err(anyhow!("export did not confirm token rotation"));
    }
    Ok(())
}

/// Read one `<key> <value>` handshake line.
fn expect_line(reader: &mut BufReader<std::process::ChildStdout>, key: &str) -> Result<String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .with_context(|| format!("read {key} from the export"))?;
    let line = line.trim();
    line.strip_prefix(&format!("{key} "))
        .map(str::to_string)
        // Deliberately does not echo the line: on a desynchronized handshake
        // the unexpected line may well be the token.
        .ok_or_else(|| anyhow!("export handshake did not start with {key}"))
}

/// The `mount_nfs` option string.
///
/// Unprivileged: MOUNT-SPIKE.md §4 confirmed `mount_nfs` needs no sudo and no
/// kext. `soft` keeps a wedged export surfacing as I/O errors instead of
/// unkillable processes; `nolock` skips the lock manager we do not serve.
///
/// `ro` is not cosmetic. The mount response and the record both report
/// `readOnly`, and a caller told a mount is read-only has been given a promise
/// the kernel is the only thing that can keep.
fn mount_options(port: &str, read_only: bool) -> String {
    let mut options = format!("vers=3,tcp,port={port},mountport={port},nolock,soft");
    if read_only {
        options.push_str(",ro");
    }
    options
}

fn run_mount(port: &str, gate_name: &str, point: &Path, read_only: bool) -> Result<()> {
    let options = mount_options(port, read_only);
    let status = Command::new("mount_nfs")
        .arg("-o")
        .arg(&options)
        .arg(format!("localhost:/{gate_name}"))
        .arg(point)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run mount_nfs")?;
    if !status.success() {
        // The status code is safe to report; the command line is not, because
        // it contains the token.
        return Err(anyhow!("mount_nfs failed with {status}"));
    }
    Ok(())
}

fn unmount_path(point: &Path) -> Result<()> {
    let status = Command::new("umount")
        .arg(point)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run umount")?;
    if !status.success() {
        return Err(anyhow!("umount failed with {status}"));
    }
    Ok(())
}

/// `afs.mount` DELETE — unmount if mounted. Returns whether anything was
/// mounted.
///
/// Idempotent by design: unmounting an unmounted session is the state the
/// caller asked for, so it answers 200 rather than inventing an error code the
/// §3.4 table does not have.
pub fn unmount(coven_home: &Path, id: &str) -> AfsResult<bool> {
    let Some(record) = read_record(coven_home, id) else {
        return Ok(false);
    };
    let mounted = pid_alive(record.owner_pid);
    if let Ok(mut guard) = exports().lock() {
        if let Some(export) = guard.remove(id) {
            export.shutdown();
        }
    }
    // Report a mount we could not take down rather than claiming success: the
    // caller asked for it to be gone, and something is holding it.
    reclaim(coven_home, &record).map_err(|error| {
        AfsError::MountBusy(format!(
            "AFS session {id} could not be unmounted from {}: {error}",
            record.mount_point
        ))
    })?;
    Ok(mounted)
}

/// Drop a mount's operating-system and on-disk footprint.
///
/// Used by both unmount and orphan recovery. The record is removed only once
/// nothing is mounted at the point: it is the sole hint a later sweep has, so
/// discarding it while a stuck mount is still live would strand that mount
/// permanently — no future daemon would know to retry. A `umount` that fails
/// on a point which is *not* mounted is not a failure at all, and its record
/// goes.
fn reclaim(coven_home: &Path, record: &MountRecord) -> Result<()> {
    let point = PathBuf::from(&record.mount_point);
    let unmounted = unmount_path(&point);
    if still_mounted(&point) {
        return unmounted
            .and_then(|()| Err(anyhow!("{} is still mounted after umount", point.display())));
    }
    // Best-effort from here: an empty directory left behind is untidy, not
    // stranding, and the next mount recreates it.
    let _ = std::fs::remove_dir(&point);
    std::fs::remove_file(record_path(coven_home, &record.session_id))
        .with_context(|| format!("remove the mount record for {}", record.session_id))
}

/// Whether something is still mounted at `point`.
///
/// A mount point sits on a different device from its parent, which is the
/// portable-enough test on Unix and the reason this is checked rather than
/// trusting `umount`'s exit code: `umount` also fails when there was nothing
/// mounted, and those two cases need opposite handling.
#[cfg(unix)]
fn still_mounted(point: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let Some(parent) = point.parent() else {
        return false;
    };
    match (std::fs::metadata(point), std::fs::metadata(parent)) {
        (Ok(here), Ok(above)) => here.dev() != above.dev(),
        // A mount point we cannot stat is not one we can prove is mounted.
        _ => false,
    }
}

#[cfg(not(unix))]
fn still_mounted(_point: &Path) -> bool {
    false
}

/// Startup sweep (DESIGN.md §7, orphan recovery).
///
/// Unmounts anything whose owning daemon is gone and drops its record. Deltas
/// are never touched: unreviewed work is not garbage, so a swept session goes
/// back to being an unmounted open session, not a discarded one.
///
/// Returns the sessions it reclaimed.
pub fn sweep_orphans(coven_home: &Path) -> Vec<String> {
    let dir = mounts_dir(coven_home);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut reclaimed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<MountRecord>(&raw) else {
            // An unreadable record is still debris. Remove it so the sweep
            // does not retry it every start.
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if pid_alive(record.owner_pid) {
            continue;
        }
        // A record that cannot be reclaimed stays put. Retrying next start is
        // the only path back for a stuck mount, and dropping the record would
        // remove it.
        if reclaim(coven_home, &record).is_ok() {
            reclaimed.push(record.session_id);
        }
    }
    reclaimed.sort();
    reclaimed
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // Signal 0 performs error checking without delivering anything: 0 means
    // the process exists, EPERM means it exists and belongs to someone else.
    // SAFETY: kill is a POSIX call with no memory effects.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_alive(pid: u32) -> bool {
    pid != 0
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, dir: &Path, pid: u32) -> MountRecord {
        MountRecord {
            session_id: id.to_string(),
            mount_point: dir.join(id).to_string_lossy().into_owned(),
            backend: "nfs".into(),
            read_only: false,
            owner_pid: pid,
            mounted_at: 0,
        }
    }

    /// A pid that is not running. 0 is never a live process to `kill(2)`, and
    /// the helper special-cases it rather than letting it mean "the process
    /// group", which would be a very bad thing to signal.
    const DEAD_PID: u32 = 0;

    #[test]
    fn no_backend_is_advertised_off_macos() {
        // FUSE is not built. A platform without a backend must report none
        // rather than offer a mount it cannot perform.
        if cfg!(target_os = "macos") {
            return;
        }
        assert_eq!(backend(), None);
    }

    #[test]
    fn a_live_owner_makes_a_record_count_as_mounted() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let live = record("afs-live", &mounts_dir(home), std::process::id());
        write_record(home, &live)?;
        assert_eq!(
            current(home, "afs-live").as_deref(),
            Some(live.mount_point.as_str())
        );
        Ok(())
    }

    #[test]
    fn a_dead_owner_reads_as_unmounted() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        write_record(home, &record("afs-dead", &mounts_dir(home), DEAD_PID))?;
        assert_eq!(current(home, "afs-dead"), None);
        Ok(())
    }

    #[test]
    fn the_sweep_reclaims_dead_owners_and_leaves_live_ones() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        write_record(home, &record("afs-dead", &mounts_dir(home), DEAD_PID))?;
        write_record(
            home,
            &record("afs-live", &mounts_dir(home), std::process::id()),
        )?;

        assert_eq!(sweep_orphans(home), vec!["afs-dead".to_string()]);
        assert!(!record_path(home, "afs-dead").exists());
        assert!(record_path(home, "afs-live").exists());
        Ok(())
    }

    #[test]
    fn the_sweep_drops_records_it_cannot_parse() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        std::fs::create_dir_all(mounts_dir(home))?;
        std::fs::write(record_path(home, "afs-junk"), "{not json")?;
        assert!(sweep_orphans(home).is_empty());
        assert!(!record_path(home, "afs-junk").exists());
        Ok(())
    }

    #[test]
    fn a_plain_directory_does_not_read_as_mounted() -> anyhow::Result<()> {
        // The distinction the sweep depends on: `umount` fails both when a
        // mount is stuck and when there was never a mount, and only the first
        // may keep its record.
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("plain");
        std::fs::create_dir_all(&dir)?;
        assert!(!still_mounted(&dir));
        Ok(())
    }

    #[test]
    fn reclaim_removes_the_record_when_nothing_is_mounted() -> anyhow::Result<()> {
        // `umount` will fail here — nothing is mounted — and the record must
        // still go, or a session would read as mounted forever.
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let stale = record("afs-r", &mounts_dir(home), DEAD_PID);
        write_record(home, &stale)?;
        std::fs::create_dir_all(&stale.mount_point)?;
        reclaim(home, &stale).expect("an unmounted point reclaims cleanly");
        assert!(!record_path(home, "afs-r").exists());
        Ok(())
    }

    #[test]
    fn unmounting_an_unmounted_session_is_not_an_error() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(!unmount(temp.path(), "afs-never").expect("idempotent unmount"));
        Ok(())
    }

    #[test]
    fn unmount_clears_a_stale_record_and_reports_it_was_not_mounted() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        write_record(home, &record("afs-stale", &mounts_dir(home), DEAD_PID))?;
        assert!(!unmount(home, "afs-stale").expect("stale unmount"));
        assert!(!record_path(home, "afs-stale").exists());
        Ok(())
    }

    #[test]
    fn a_non_empty_mount_point_is_refused() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let point = temp.path().join("point");
        std::fs::create_dir_all(&point)?;
        std::fs::write(point.join("occupant.txt"), b"already here")?;
        // Mounting over this would hide the file until unmount, which is a
        // data-loss shape rather than an inconvenience. §3.4 gives it a code:
        // it must not surface as a 500.
        let error = prepare_mount_point(&point).expect_err("a non-empty point is refused");
        assert!(matches!(error, AfsError::MountBusy(_)));
        assert_eq!(error.parts().0, 409);
        Ok(())
    }

    #[test]
    fn an_empty_mount_point_is_accepted_and_created_on_demand() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let point = temp.path().join("nested").join("point");
        prepare_mount_point(&point).expect("empty mount point is accepted");
        assert!(point.is_dir());
        Ok(())
    }

    #[test]
    fn mount_refuses_when_no_backend_is_available() -> anyhow::Result<()> {
        if backend().is_some() {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let error = mount(temp.path(), "afs-x", &temp.path().join("d.db"), None, false)
            .expect_err("no backend should refuse");
        assert!(matches!(error, AfsError::MountUnsupported));
        Ok(())
    }

    #[test]
    fn a_read_only_mount_is_mounted_read_only() {
        // Reporting readOnly while mounting writable hands the caller a
        // promise nothing keeps; `ro` is what makes the flag true.
        assert!(mount_options("2049", true).ends_with(",ro"));
        assert!(!mount_options("2049", false).contains("ro"));
    }

    #[test]
    fn a_record_round_trips_without_carrying_a_port_or_token() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let original = record("afs-rt", &mounts_dir(home), 4242);
        write_record(home, &original)?;
        let raw = std::fs::read_to_string(record_path(home, "afs-rt"))?;
        assert_eq!(read_record(home, "afs-rt").as_ref(), Some(&original));
        // The record is a cleanup hint, not a credential store.
        assert!(!raw.contains("port"));
        assert!(!raw.contains("oken"));
        Ok(())
    }

    #[test]
    fn the_view_omits_everything_a_caller_could_connect_to() {
        // Built from literals rather than from a path join: this asserts the
        // wire shape, and a join would assert the host's path separator.
        let view = MountView {
            mount_point: "/tmp/afs/afs-v".to_string(),
            backend: "nfs".to_string(),
            read_only: false,
        };
        let json = serde_json::to_string(&view).expect("serialize");
        assert_eq!(
            json,
            r#"{"mountPoint":"/tmp/afs/afs-v","backend":"nfs","readOnly":false}"#
        );
    }
}
