//! `SeatbeltDriver`: the macOS backend behind the restricted worker controller.

use std::collections::hash_map::RandomState;
use std::ffi::CString;
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
    /// Workspace is not a private (`0700`), caller-owned directory.
    Workspace,
    /// `bin/coven-worker-target` is missing, not a private executable file, or
    /// not on the workspace filesystem.
    Executable,
    /// `closure/` is missing or not on the workspace filesystem.
    Closure,
    /// Guardian binary is not an absolute, caller-owned, executable file.
    Guardian,
    /// A path contains characters the profile cannot quote.
    Path,
    Stdio,
}

pub struct SeatbeltConfig {
    /// Private workspace holding `bin/coven-worker-target` and `closure/`.
    pub workspace: PathBuf,
    /// Trusted host binary re-executed as the guardian (`Role::Guardian`).
    pub guardian: PathBuf,
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
        let identity = Identity::of(&file.metadata()?);
        Ok(Self {
            file,
            path,
            identity,
        })
    }

    /// Physical revalidation: the retained descriptor and the pathname the
    /// kernel will exec must still name the same inode.
    fn unchanged(&self) -> bool {
        let by_fd = self.file.metadata().map(|m| Identity::of(&m));
        let by_path = fs::symlink_metadata(&self.path).map(|m| Identity::of(&m));
        matches!((by_fd, by_path), (Ok(a), Ok(b)) if a == self.identity && b == self.identity)
    }
}

fn private_to_caller(metadata: &fs::Metadata) -> bool {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    metadata.uid() == uid && metadata.mode() & 0o077 == 0
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
        "(allow process-fork)",
        "(allow signal (target same-sandbox))",
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
        if !ws_meta.is_dir() || !private_to_caller(&ws_meta) {
            return Err(SealError::Workspace);
        }

        let executable = Held::open(workspace.path.join("bin").join(TARGET_NAME), 0)
            .map_err(|_| SealError::Executable)?;
        let exe_meta = executable
            .file
            .metadata()
            .map_err(|_| SealError::Executable)?;
        if !exe_meta.is_file()
            || exe_meta.mode() & 0o100 == 0
            || exe_meta.mode() & 0o022 != 0
            || exe_meta.uid() != ws_meta.uid()
            || exe_meta.dev() != ws_meta.dev()
        {
            return Err(SealError::Executable);
        }

        let closure = Held::open(workspace.path.join("closure"), libc::O_DIRECTORY)
            .map_err(|_| SealError::Closure)?;
        if closure.identity.dev != ws_meta.dev() {
            return Err(SealError::Closure);
        }

        if !config.guardian.is_absolute() {
            return Err(SealError::Guardian);
        }
        let guardian_binary = Held::open(config.guardian, 0).map_err(|_| SealError::Guardian)?;
        let guardian_meta = guardian_binary
            .file
            .metadata()
            .map_err(|_| SealError::Guardian)?;
        if !guardian_meta.is_file()
            || guardian_meta.mode() & 0o100 == 0
            || guardian_meta.uid() != ws_meta.uid()
        {
            return Err(SealError::Guardian);
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

    fn spawn_guardian(&mut self, remaining_ms: u64) -> Result<Guardian, BackendError> {
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
            .arg(remaining_ms.to_string())
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
        let now =
            u64::try_from(self.origin.elapsed().as_millis()).map_err(|_| BackendError::Rejected)?;
        let remaining = lease.deadline_ms.saturating_sub(now);
        if remaining == 0 || self.guardian.is_some() {
            return Err(BackendError::Rejected);
        }
        self.revalidate_resources()?;
        let guardian = self.spawn_guardian(remaining)?;
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
        guardian.send("spawn")?;
        match guardian.next_line(IPC_BUDGET) {
            Some(line) if line.starts_with("spawned ") => Ok(receipt),
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
        let _ = guardian.child.wait();
        self.absorb(&lines);
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
