//! `SeatbeltDriver`: the macOS backend behind the restricted worker controller.

use std::collections::hash_map::RandomState;
use std::ffi::{c_int, c_void, CString};
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use coven_restricted_runtime::{
    BackendError, Binding, CleanupOutcome, CleanupReport, Clock, ClockError, Driver, Operation,
    Reading, Receipt, Request, Termination, TerminationReport,
};

use crate::{GUARDIAN_NAME, TARGET_NAME};

const IPC_BUDGET: Duration = Duration::from_secs(3);

// `acl_get_fd_np`/`acl_free` live in libsystem_c, re-exported by libSystem.
// `libc` carries no Darwin ACL bindings, so they are declared here the way
// `guardian.rs` declares `sandbox_init`.
#[link(name = "System", kind = "dylib")]
extern "C" {
    fn acl_get_fd_np(fd: c_int, acl_type: c_int) -> *mut c_void;
    fn acl_free(obj: *mut c_void) -> c_int;
}

/// `ACL_TYPE_EXTENDED` from `<sys/acl.h>`: the only ACL type Darwin stores.
const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;

/// Process-local monotonic clock. The backend and controller must share one
/// instance's origin; `SeatbeltDriver::seal` hands back that clock.
pub struct InstantClock {
    origin: Instant,
    era: u64,
}

impl Clock for InstantClock {
    fn observe(&mut self) -> Result<Reading, ClockError> {
        let millis =
            u64::try_from(self.origin.elapsed().as_millis()).map_err(|_| ClockError::Overflow)?;
        Ok(Reading {
            era: self.era,
            millis,
        })
    }
}

/// Worker ends of the three controller-owned stdio pipes.
pub struct WorkerStdio {
    stdin: OwnedFd,
    stdout: OwnedFd,
    stderr: OwnedFd,
}

/// Controller ends of the same pipes: write to `stdin`, read the others.
pub struct ControllerStdio {
    pub stdin: File,
    pub stdout: File,
    pub stderr: File,
}

impl WorkerStdio {
    /// Creates three fresh close-on-exec pipes.
    pub fn pipes() -> io::Result<(WorkerStdio, ControllerStdio)> {
        let (stdin_r, stdin_w) = pipe()?;
        let (stdout_r, stdout_w) = pipe()?;
        let (stderr_r, stderr_w) = pipe()?;
        Ok((
            WorkerStdio {
                stdin: stdin_r,
                stdout: stdout_w,
                stderr: stderr_w,
            },
            ControllerStdio {
                stdin: File::from(stdin_w),
                stdout: File::from(stdout_r),
                stderr: File::from(stderr_r),
            },
        ))
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: pipe writes two descriptors into the provided array; fcntl only
    // sets close-on-exec on descriptors this process just created.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
    }
}

/// Reasons sealing refused. No paths or OS diagnostics are carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealError {
    /// Workspace is not a private (`0700`), caller-owned directory, or an
    /// extended ACL grants another principal access to it.
    Workspace,
    /// `bin/coven-worker-target` is missing, not a private executable file, on
    /// another filesystem, or widened by an extended ACL.
    Executable,
    /// `closure/` is missing or not on the workspace filesystem.
    Closure,
    /// Guardian binary is not an absolute, caller-owned, executable file, or
    /// an extended ACL widens it.
    Guardian,
    /// A path contains characters the profile cannot quote.
    Path,
    Stdio,
}

pub struct SeatbeltConfig {
    /// Private workspace holding `bin/coven-worker-target`,
    /// `bin/coven-worker-guardian` (a staged copy of the trusted host binary,
    /// re-executed as `Role::Guardian`) and `closure/`.
    pub workspace: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
}

impl Identity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }

    fn id(self) -> u128 {
        (u128::from(self.dev) << 64) | u128::from(self.ino)
    }
}

struct Held {
    file: File,
    path: PathBuf,
    identity: Identity,
}

impl Held {
    fn open(path: PathBuf, flags: i32) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(flags | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        Self::from_file(file, path)
    }

    /// Opens `name` relative to an already-held directory descriptor so no
    /// ancestor component can be swapped for a symlink after `dir` was opened.
    fn open_at(dir: &Held, name: &str, flags: i32) -> io::Result<Self> {
        let c_name = CString::new(name).map_err(|_| io::Error::other("name"))?;
        // SAFETY: openat on a descriptor this driver owns with a NUL-terminated
        // single path component; the returned descriptor is owned immediately.
        let fd = unsafe {
            libc::openat(
                dir.file.as_raw_fd(),
                c_name.as_ptr(),
                flags | libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh descriptor returned by openat above.
        let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        Self::from_file(file, dir.path.join(name))
    }

    fn from_file(file: File, path: PathBuf) -> io::Result<Self> {
        let identity = Identity::of(&file.metadata()?);
        Ok(Self {
            file,
            path,
            identity,
        })
    }

    /// A regular, caller-owned, executable file that only its owner can write,
    /// on the workspace filesystem, with no extended ACL widening that.
    fn private_executable(&self, workspace: &fs::Metadata) -> io::Result<bool> {
        let meta = self.file.metadata()?;
        Ok(meta.is_file()
            && meta.mode() & 0o100 != 0
            && meta.mode() & 0o022 == 0
            && meta.uid() == workspace.uid()
            && meta.dev() == workspace.dev()
            && self.owner_only_acl())
    }

    /// A caller-owned object no other principal can open: owner-only mode bits
    /// and no extended ACL.
    fn private_to_caller(&self) -> io::Result<bool> {
        let meta = self.file.metadata()?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        Ok(meta.uid() == uid && meta.mode() & 0o077 == 0 && self.owner_only_acl())
    }

    /// True when the kernel holds no extended ACL for this object.
    ///
    /// Mode bits are not the whole access story on Darwin: an ACL entry can
    /// grant another principal write or traverse while `stat` still reports
    /// caller ownership and `0700`, so a mode-only check would call a shared
    /// path private and let a non-owner replace the staged target or guardian.
    /// Any ACL at all is refused — an owner-only ACL would be redundant with
    /// the mode bits already required here, so there is nothing to allow.
    fn owner_only_acl(&self) -> bool {
        // SAFETY: acl_get_fd_np on a descriptor this driver owns. A non-null
        // return is a fresh allocation, freed here before the value is dropped.
        unsafe {
            let acl = acl_get_fd_np(self.file.as_raw_fd(), ACL_TYPE_EXTENDED);
            if acl.is_null() {
                // Darwin reports "no ACL" as ENOENT; any other errno leaves
                // the access set unproven, which is not private enough.
                return io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT);
            }
            acl_free(acl);
            false
        }
    }

    /// Physical revalidation: the retained descriptor and the pathname the
    /// kernel will exec must still name the same inode.
    fn unchanged(&self) -> bool {
        let by_fd = self.file.metadata().map(|m| Identity::of(&m));
        let by_path = fs::symlink_metadata(&self.path).map(|m| Identity::of(&m));
        matches!((by_fd, by_path), (Ok(a), Ok(b)) if a == self.identity && b == self.identity)
    }
}

fn fresh_id() -> u128 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(u64::from(std::process::id()));
    let high = hasher.finish();
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(high);
    (u128::from(high) << 64) | u128::from(hasher.finish())
}

fn quote(path: &Path) -> Result<String, SealError> {
    let text = path.to_str().ok_or(SealError::Path)?;
    if text
        .bytes()
        .any(|b| matches!(b, b'"' | b'\\' | b'\n' | b'\r' | 0))
    {
        return Err(SealError::Path);
    }
    Ok(format!("\"{text}\""))
}

/// Single-line Seatbelt profile: deny everything, then allow exactly one
/// executable image, read-only access to the sealed workspace, and the
/// read-only loader closure every Mach-O needs to start (`/usr/lib` plus
/// Apple's own `dyld-support.sb` shared-cache rules). `/dev/null` is the only
/// device; `sysctl-read` is required by the Rust runtime's stack guard setup.
/// `process-fork` stays denied: the contract is one worker with no children.
fn profile(workspace: &Path, executable: &Path) -> Result<String, SealError> {
    let ws = quote(workspace)?;
    let exe = quote(executable)?;
    Ok([
        "(version 1)",
        "(deny default)",
        "(import \"dyld-support.sb\")",
        &format!("(allow process-exec (literal {exe}))"),
        &format!("(allow file-read* (subpath {ws}))"),
        "(allow file-read* (subpath \"/usr/lib\") (literal \"/dev/null\"))",
        "(allow sysctl-read)",
    ]
    .join(" "))
}

struct Guardian {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Guardian {
    fn send(&mut self, line: &str) -> Result<(), BackendError> {
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|_| BackendError::Unavailable)
    }

    fn next_line(&mut self, budget: Duration) -> Option<String> {
        self.lines.recv_timeout(budget).ok()
    }

    fn drain(&mut self) -> Vec<String> {
        self.lines.try_iter().collect()
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// One sealed offline worker: private workspace, pinned executable, fixed
/// Seatbelt profile, controller-owned stdio pipes, and an independent
/// guardian process that owns the worker's process group and lease.
pub struct SeatbeltDriver {
    binding: Binding,
    workspace: Held,
    bin: Held,
    executable: Held,
    closure: Held,
    guardian_binary: Held,
    profile: String,
    stdio: Option<WorkerStdio>,
    origin: Instant,
    guardian: Option<Guardian>,
    launched: bool,
    terminated: bool,
}

impl SeatbeltDriver {
    /// Takes custody of the workspace, executable, closure directory, guardian
    /// binary and worker stdio, and allocates fresh attempt/worker identities.
    /// Nothing is executed. The returned clock shares the backend's domain.
    pub fn seal(
        config: SeatbeltConfig,
        stdio: WorkerStdio,
    ) -> Result<(Self, InstantClock), SealError> {
        let workspace_path =
            fs::canonicalize(&config.workspace).map_err(|_| SealError::Workspace)?;
        let workspace =
            Held::open(workspace_path, libc::O_DIRECTORY).map_err(|_| SealError::Workspace)?;
        let ws_meta = workspace
            .file
            .metadata()
            .map_err(|_| SealError::Workspace)?;
        if !ws_meta.is_dir()
            || !workspace
                .private_to_caller()
                .map_err(|_| SealError::Workspace)?
        {
            return Err(SealError::Workspace);
        }

        let bin = Held::open_at(&workspace, "bin", libc::O_DIRECTORY)
            .map_err(|_| SealError::Executable)?;
        let bin_meta = bin.file.metadata().map_err(|_| SealError::Executable)?;
        if !bin_meta.is_dir()
            || bin_meta.dev() != ws_meta.dev()
            || !bin.private_to_caller().map_err(|_| SealError::Executable)?
        {
            return Err(SealError::Executable);
        }
        let executable = Held::open_at(&bin, TARGET_NAME, 0).map_err(|_| SealError::Executable)?;
        if !executable
            .private_executable(&ws_meta)
            .map_err(|_| SealError::Executable)?
        {
            return Err(SealError::Executable);
        }
        let guardian_binary =
            Held::open_at(&bin, GUARDIAN_NAME, 0).map_err(|_| SealError::Guardian)?;
        if !guardian_binary
            .private_executable(&ws_meta)
            .map_err(|_| SealError::Guardian)?
        {
            return Err(SealError::Guardian);
        }

        let closure = Held::open_at(&workspace, "closure", libc::O_DIRECTORY)
            .map_err(|_| SealError::Closure)?;
        if closure.identity.dev != ws_meta.dev() {
            return Err(SealError::Closure);
        }

        let profile = profile(&workspace.path, &executable.path)?;
        let stdin_meta = File::from(stdio.stdin.try_clone().map_err(|_| SealError::Stdio)?)
            .metadata()
            .map_err(|_| SealError::Stdio)?;

        let mut closure_hash = RandomState::new().build_hasher();
        closure_hash.write(profile.as_bytes());
        let binding = Binding {
            attempt: fresh_id(),
            backend: fresh_id(),
            worker: fresh_id(),
            workspace: workspace.identity.id(),
            executable: executable.identity.id(),
            runtime_closure: closure.identity.id() ^ (u128::from(closure_hash.finish()) << 64),
            stdio_pipes: Identity::of(&stdin_meta).id(),
        };
        let origin = Instant::now();
        Ok((
            Self {
                binding,
                workspace,
                bin,
                executable,
                closure,
                guardian_binary,
                profile,
                stdio: Some(stdio),
                origin,
                guardian: None,
                launched: false,
                terminated: false,
            },
            InstantClock { origin, era: 1 },
        ))
    }

    fn receipt(&self, request: &Request, operation: Operation) -> Result<Receipt, BackendError> {
        if request.receipt.operation != operation || request.receipt.binding != self.binding {
            return Err(BackendError::Rejected);
        }
        Ok(request.receipt)
    }

    fn revalidate_resources(&self) -> Result<(), BackendError> {
        let held = [
            &self.workspace,
            &self.bin,
            &self.executable,
            &self.closure,
            &self.guardian_binary,
        ];
        if held.iter().all(|h| h.unchanged()) {
            Ok(())
        } else {
            Err(BackendError::Rejected)
        }
    }

    fn fenced(&self, request: &Request) -> bool {
        request.stop_signal().reason().is_some()
    }

    fn now_ms(&self) -> Result<u64, BackendError> {
        u64::try_from(self.origin.elapsed().as_millis()).map_err(|_| BackendError::Rejected)
    }

    fn spawn_guardian(&mut self, deadline_ms: u64) -> Result<Guardian, BackendError> {
        let stdio = self.stdio.take().ok_or(BackendError::Rejected)?;
        // Re-home the worker pipes above any descriptor std may allocate for
        // the guardian's own stdio so the child-side dup2 into 3-5 cannot
        // clobber a source before it is copied.
        let high = [&stdio.stdin, &stdio.stdout, &stdio.stderr].map(|fd| {
            // SAFETY: F_DUPFD_CLOEXEC on descriptors this driver owns.
            let dup = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
            (dup >= 0).then(|| unsafe { OwnedFd::from_raw_fd(dup) })
        });
        let [Some(stdin), Some(stdout), Some(stderr)] = high else {
            return Err(BackendError::Unavailable);
        };
        drop(stdio);
        let fds = [stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()];
        let mut command = Command::new(&self.guardian_binary.path);
        command
            .arg0(GUARDIAN_NAME)
            .arg(std::process::id().to_string())
            .arg(&self.executable.path)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        // SAFETY: only dup2/fcntl/close in the forked child before exec.
        unsafe {
            command.pre_exec(move || {
                for (target, fd) in (3..=5).zip(fds) {
                    if libc::dup2(fd, target) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                crate::guardian::close_inheritable_from(6);
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|_| BackendError::Unavailable)?;
        drop((stdin, stdout, stderr));
        let stdin = child.stdin.take().ok_or(BackendError::Unavailable)?;
        let stdout = child.stdout.take().ok_or(BackendError::Unavailable)?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut guardian = Guardian {
            child,
            stdin,
            lines,
        };
        // Charge the whole handoff to the lease: the remaining budget is
        // measured immediately before it is written and the guardian starts
        // its deadline the moment it reads the line, so IPC latency can only
        // shorten the lease, never extend it.
        let remaining = deadline_ms.saturating_sub(self.now_ms()?);
        if remaining == 0 {
            return Err(BackendError::Rejected);
        }
        guardian.send(&format!("lease {remaining}"))?;
        guardian.send(&self.profile)?;
        match guardian.next_line(IPC_BUDGET).as_deref() {
            Some("ready") => Ok(guardian),
            _ => Err(BackendError::Unavailable),
        }
    }

    fn absorb(&mut self, lines: &[String]) {
        if lines.iter().any(|l| l == "terminated") {
            self.terminated = true;
        }
    }
}

impl Driver for SeatbeltDriver {
    fn binding(&self) -> Binding {
        self.binding
    }

    fn prepare(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        let receipt = self.receipt(request, Operation::Prepare)?;
        if self.stdio.is_none() || self.guardian.is_some() {
            return Err(BackendError::Rejected);
        }
        self.revalidate_resources()?;
        Ok(receipt)
    }

    fn install_restrictions(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        let receipt = self.receipt(request, Operation::Install)?;
        // The profile is fixed at seal time and applied by the guardian inside
        // the forked worker before its exec; a profile that fails to compile
        // aborts that exec, so no target code can run unrestricted.
        CString::new(self.profile.clone()).map_err(|_| BackendError::Rejected)?;
        self.revalidate_resources()?;
        Ok(receipt)
    }

    fn retain_lifetime(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        let receipt = self.receipt(request, Operation::RetainLifetime)?;
        let lease = request.lease.ok_or(BackendError::Rejected)?;
        if self.now_ms()? >= lease.deadline_ms || self.guardian.is_some() {
            return Err(BackendError::Rejected);
        }
        self.revalidate_resources()?;
        let guardian = self.spawn_guardian(lease.deadline_ms)?;
        self.guardian = Some(guardian);
        Ok(receipt)
    }

    fn revalidate(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        let receipt = self.receipt(request, Operation::Revalidate)?;
        self.revalidate_resources()?;
        match self.guardian.as_mut().map(Guardian::alive) {
            Some(true) => Ok(receipt),
            _ => Err(BackendError::Unavailable),
        }
    }

    fn execute(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        let receipt = self.receipt(request, Operation::Execute)?;
        if self.launched || self.fenced(request) {
            return Err(BackendError::Rejected);
        }
        self.revalidate_resources()?;
        let guardian = self.guardian.as_mut().ok_or(BackendError::Rejected)?;
        if !guardian.alive() {
            return Err(BackendError::Unavailable);
        }
        self.launched = true;
        // Final look at the shared signal before the irreversible send. A
        // cancel that lands between the send and the acknowledgement is
        // honoured by killing the fresh worker instead of reporting success.
        if request.stop_signal().reason().is_some() {
            return Err(BackendError::Rejected);
        }
        guardian.send("spawn")?;
        match guardian.next_line(IPC_BUDGET) {
            Some(line) if line.starts_with("spawned ") => {
                if request.stop_signal().reason().is_some() {
                    let _ = guardian.send("kill");
                    return Err(BackendError::Rejected);
                }
                Ok(receipt)
            }
            Some(_) => Err(BackendError::Rejected),
            None => Err(BackendError::Unavailable),
        }
    }

    fn request_cleanup(&mut self, request: &Request) -> Result<CleanupReport, BackendError> {
        let receipt = self.receipt(request, Operation::Cleanup)?;
        drop(self.stdio.take());
        let Some(guardian) = self.guardian.as_mut() else {
            return Ok(CleanupReport {
                receipt,
                outcome: CleanupOutcome::Released,
            });
        };
        let mut lines = guardian.drain();
        if guardian.send("kill").is_ok() {
            while let Some(line) = guardian.next_line(IPC_BUDGET) {
                let done = line == "terminated" || line == "pending";
                lines.push(line);
                if done {
                    break;
                }
            }
        }
        self.absorb(&lines);
        if self.terminated {
            if let Some(guardian) = self.guardian.as_mut() {
                let _ = guardian.child.wait();
            }
        }
        Ok(CleanupReport {
            receipt,
            outcome: if self.terminated {
                CleanupOutcome::Released
            } else {
                CleanupOutcome::Pending
            },
        })
    }

    fn observe_termination(
        &mut self,
        request: &Request,
    ) -> Result<TerminationReport, BackendError> {
        let receipt = self.receipt(request, Operation::ObserveTermination)?;
        let (mut lines, guardian_alive) = match self.guardian.as_mut() {
            Some(g) => (g.drain(), g.alive()),
            None => return Err(BackendError::Rejected),
        };
        if !guardian_alive {
            // The guardian exited; collect its final lines before judging.
            let guardian = self.guardian.as_mut().ok_or(BackendError::Rejected)?;
            while let Some(line) = guardian.next_line(Duration::from_millis(200)) {
                lines.push(line);
            }
        }
        self.absorb(&lines);
        if self.terminated {
            return Ok(TerminationReport {
                receipt,
                outcome: Termination::Confirmed,
            });
        }
        if !guardian_alive {
            // The guardian left without confirming an empty process group.
            return Err(BackendError::Unavailable);
        }
        Ok(TerminationReport {
            receipt,
            outcome: Termination::Pending,
        })
    }
}
