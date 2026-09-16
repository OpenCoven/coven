//! Independent guardian process: owns the worker's whole process group, the
//! fixed lease and owner-death fencing, and survives controller/driver loss.
//!
//! Protocol (trusted backend IPC, never worker-visible):
//! * argv: `owner_pid executable_path`
//! * stdin line 1: `lease <remaining_ms>` (the deadline starts on receipt so
//!   handoff latency only shortens the lease); line 2: the single-line Seatbelt
//!   profile; later lines: `spawn`, `kill`, `release`. EOF is owner loss.
//! * fds 3/4/5: the controller-owned worker stdin/stdout/stderr pipes.
//! * stdout lines: `ready`, `spawned <pid>`, `spawn-failed`, `terminated`,
//!   `pending`. After `pending` the guardian keeps killing the group and
//!   only exits once it can say `terminated`. A `spawn` may answer
//!   `terminated` rather than `spawned <pid>`: the owner/lease boundary is
//!   re-read after the fork and a worker that crossed it mid-spawn is killed
//!   before it is ever reported as running.

use std::ffi::{c_char, c_int, CString};
use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// `sandbox_init`/`sandbox_free_error` live in libsystem_sandbox, which is
// re-exported by libSystem; the explicit link attribute records that
// dependency instead of relying on it implicitly.
#[link(name = "System", kind = "dylib")]
extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

const TICK: Duration = Duration::from_millis(10);
const KILL_BUDGET: Duration = Duration::from_secs(2);

/// Closes every descriptor at or above `from` that is not close-on-exec.
/// Descriptors already marked close-on-exec (including std's internal exec
/// status pipe) close at `execve`; only inheritable ones must go explicitly.
pub(crate) fn close_inheritable_from(from: c_int) {
    // SAFETY: fcntl/close/getdtablesize are async-signal-safe and only touch
    // descriptors this process owns.
    unsafe {
        let max = libc::getdtablesize();
        let mut fd = from;
        while fd < max {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::close(fd);
            }
            fd += 1;
        }
    }
}

fn group_alive(pgid: i32) -> bool {
    // SAFETY: killpg with signal 0 only probes for existence.
    let rc = unsafe { libc::killpg(pgid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn owner_alive(pid: i32) -> bool {
    // SAFETY: kill with signal 0 only probes for existence.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The owner/lease boundary this attempt is authorised against.
///
/// Read once before the fork and again after it: the guardian is
/// single-threaded, so nothing can fence it while it is inside
/// `fork`/`sandbox_init`/`exec`, and the boundary it checked may have passed
/// by the time the worker exists.
fn boundary_open(deadline: Instant, owner: i32) -> bool {
    Instant::now() < deadline && owner_alive(owner)
}

fn say(line: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

struct Stdin {
    buffer: Vec<u8>,
    eof: bool,
}

impl Stdin {
    fn new() -> Self {
        // SAFETY: fcntl on our own stdin; nonblocking keeps this process
        // single-threaded so the later fork/pre_exec path stays safe.
        unsafe {
            let flags = libc::fcntl(0, libc::F_GETFL);
            libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        Self {
            buffer: Vec::new(),
            eof: false,
        }
    }

    fn pump(&mut self) {
        let mut chunk = [0u8; 256];
        loop {
            match io::stdin().read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return;
                }
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.eof = true;
                    return;
                }
            }
        }
    }

    fn line(&mut self) -> Option<String> {
        let end = self.buffer.iter().position(|b| *b == b'\n')?;
        let line: Vec<u8> = self.buffer.drain(..=end).collect();
        Some(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned())
    }

    fn wait_line(&mut self, budget: Duration) -> Option<String> {
        let start = Instant::now();
        loop {
            self.pump();
            if let Some(line) = self.line() {
                return Some(line);
            }
            if self.eof || start.elapsed() > budget {
                return None;
            }
            std::thread::sleep(TICK);
        }
    }
}

struct Worker {
    child: Child,
    pgid: i32,
    /// Set once `try_wait` has collected the leader's exit status. std caches
    /// the status internally, but the flag keeps the reaping contract explicit.
    reaped: bool,
}

impl Worker {
    fn spawn(executable: &str, profile: CString, stdio: [OwnedFd; 3]) -> io::Result<Self> {
        let [stdin, stdout, stderr] = stdio;
        let mut command = Command::new(executable);
        command
            .env_clear()
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .process_group(0);
        // SAFETY: runs in the forked child before exec. It only closes
        // descriptors and installs the Seatbelt profile; no allocation-heavy
        // work happens here and the guardian is single-threaded.
        unsafe {
            command.pre_exec(move || {
                close_inheritable_from(3);
                let mut error: *mut c_char = std::ptr::null_mut();
                if sandbox_init(profile.as_ptr(), 0, &mut error) != 0 {
                    if !error.is_null() {
                        sandbox_free_error(error);
                    }
                    return Err(io::Error::from_raw_os_error(libc::EPERM));
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        let pgid = c_int::try_from(child.id()).map_err(|_| io::Error::other("pid"))?;
        Ok(Self {
            child,
            pgid,
            reaped: false,
        })
    }

    fn reap(&mut self) -> bool {
        if !self.reaped && matches!(self.child.try_wait(), Ok(Some(_))) {
            self.reaped = true;
        }
        self.reaped
    }

    /// SIGKILLs the whole group until no member remains and the leader is
    /// reaped, within a bounded budget. Returns true only on full termination.
    fn terminate(&mut self) -> bool {
        let start = Instant::now();
        loop {
            // SAFETY: killpg on the group this guardian created.
            unsafe {
                libc::killpg(self.pgid, libc::SIGKILL);
            }
            if self.reap() && !group_alive(self.pgid) {
                return true;
            }
            if start.elapsed() > KILL_BUDGET {
                return false;
            }
            std::thread::sleep(TICK);
        }
    }

    /// Natural exit of the leader plus an empty group is confirmed termination.
    fn finished(&mut self) -> bool {
        self.reap() && !group_alive(self.pgid)
    }
}

/// Terminates the group and exits. If the first bounded attempt fails the
/// guardian reports `pending` once, then stays alive and keeps killing until
/// the group is gone so a stuck worker is never orphaned by its own guardian.
fn finish(worker: Option<&mut Worker>) -> ! {
    let Some(worker) = worker else {
        say("terminated");
        std::process::exit(0)
    };
    if !worker.terminate() {
        say("pending");
        while !worker.terminate() {
            std::thread::sleep(TICK);
        }
    }
    say("terminated");
    std::process::exit(0)
}

/// Entry point for a host binary that detects `Role::Guardian`. Never returns.
pub fn guardian_main() -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (owner, executable) = match args.as_slice() {
        [owner, executable] => match owner.parse::<i32>() {
            Ok(owner) => (owner, executable.clone()),
            Err(_) => std::process::exit(64),
        },
        _ => std::process::exit(64),
    };
    // SAFETY: descriptors 3-5 were dup2'd for this process by the backend
    // immediately before exec and are owned by nobody else here.
    let stdio = unsafe {
        [
            OwnedFd::from_raw_fd(3),
            OwnedFd::from_raw_fd(4),
            OwnedFd::from_raw_fd(5),
        ]
    };
    for fd in 3..=5 {
        // SAFETY: mark the adopted pipes close-on-exec so only the deliberate
        // dup2 into the worker's 0/1/2 survives its exec.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    close_inheritable_from(6);

    let mut stdin = Stdin::new();
    let remaining_ms = match stdin
        .wait_line(Duration::from_secs(5))
        .as_deref()
        .and_then(|line| line.strip_prefix("lease "))
        .and_then(|ms| ms.parse::<u64>().ok())
    {
        Some(ms) if ms > 0 => ms,
        _ => std::process::exit(65),
    };
    let deadline = Instant::now() + Duration::from_millis(remaining_ms);
    let profile = match stdin
        .wait_line(Duration::from_secs(5))
        .and_then(|line| CString::new(line).ok())
    {
        Some(profile) => profile,
        None => std::process::exit(65),
    };
    say("ready");

    let mut stdio = Some(stdio);
    let mut worker: Option<Worker> = None;
    loop {
        stdin.pump();
        while let Some(line) = stdin.line() {
            match line.as_str() {
                "spawn" if worker.is_none() => match stdio.take() {
                    Some(fds) if boundary_open(deadline, owner) => {
                        match Worker::spawn(&executable, profile.clone(), fds) {
                            Ok(mut spawned) => {
                                // The boundary was open when the fork was
                                // authorised; re-read it now the worker
                                // exists. A worker whose owner exited or whose
                                // lease expired during the spawn is killed
                                // here and never reported as running. `finish`
                                // rather than `spawn-failed`: the group must
                                // still be proven empty, and only `terminated`
                                // carries that proof to the controller.
                                if !boundary_open(deadline, owner) {
                                    finish(Some(&mut spawned));
                                }
                                say(&format!("spawned {}", spawned.child.id()));
                                worker = Some(spawned);
                            }
                            Err(_) => {
                                say("spawn-failed");
                                std::process::exit(1);
                            }
                        }
                    }
                    _ => {
                        say("spawn-failed");
                        std::process::exit(1);
                    }
                },
                "kill" => finish(worker.as_mut()),
                "release" => std::process::exit(0),
                _ => {}
            }
        }
        if stdin.eof || !boundary_open(deadline, owner) {
            finish(worker.as_mut());
        }
        if let Some(active) = worker.as_mut() {
            if active.finished() {
                say("terminated");
                std::process::exit(0);
            }
        }
        std::thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::boundary_open;
    use std::time::{Duration, Instant};

    /// A pid far above `kern.maxproc`, so `kill` can only answer `ESRCH`.
    const ABSENT_OWNER: i32 = i32::MAX;

    #[test]
    fn boundary_is_open_only_while_both_halves_hold() {
        let owner = std::process::id() as i32;
        let future = Instant::now() + Duration::from_secs(60);
        let past = Instant::now() - Duration::from_secs(1);

        assert!(boundary_open(future, owner));
        assert!(!boundary_open(past, owner), "expired lease");
        assert!(!boundary_open(future, ABSENT_OWNER), "dead owner");
        assert!(!boundary_open(past, ABSENT_OWNER));
    }
}
