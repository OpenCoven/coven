//! Independent guardian process: owns the worker's whole process group, the
//! fixed lease and owner-death fencing, and survives controller/driver loss.
//!
//! Protocol (trusted backend IPC, never worker-visible):
//! * argv: `owner_pid remaining_ms executable_path`
//! * stdin line 1: the single-line Seatbelt profile; later lines: `spawn`,
//!   `kill`, `release`. EOF is treated as owner loss.
//! * fds 3/4/5: the controller-owned worker stdin/stdout/stderr pipes.
//! * stdout lines: `ready`, `spawned <pid>`, `spawn-failed`, `terminated`,
//!   `pending`.

use std::ffi::{c_char, c_int, CString};
use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
        Ok(Self { child, pgid })
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
            let reaped = matches!(self.child.try_wait(), Ok(Some(_)));
            if reaped && !group_alive(self.pgid) {
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
        matches!(self.child.try_wait(), Ok(Some(_))) && !group_alive(self.pgid)
    }
}

fn finish(worker: Option<&mut Worker>) -> ! {
    let terminated = worker.is_none_or(Worker::terminate);
    say(if terminated { "terminated" } else { "pending" });
    std::process::exit(if terminated { 0 } else { 2 })
}

/// Entry point for a host binary that detects `Role::Guardian`. Never returns.
pub fn guardian_main() -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (owner, remaining_ms, executable) = match args.as_slice() {
        [owner, remaining, executable] => match (owner.parse::<i32>(), remaining.parse::<u64>()) {
            (Ok(owner), Ok(remaining)) => (owner, remaining, executable.clone()),
            _ => std::process::exit(64),
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
    let profile = match stdin
        .wait_line(Duration::from_secs(5))
        .and_then(|line| CString::new(line).ok())
    {
        Some(profile) => profile,
        None => std::process::exit(65),
    };
    let deadline = Instant::now() + Duration::from_millis(remaining_ms);
    say("ready");

    let mut stdio = Some(stdio);
    let mut worker: Option<Worker> = None;
    loop {
        stdin.pump();
        while let Some(line) = stdin.line() {
            match line.as_str() {
                "spawn" if worker.is_none() => match stdio.take() {
                    Some(fds) if Instant::now() < deadline && owner_alive(owner) => {
                        match Worker::spawn(&executable, profile.clone(), fds) {
                            Ok(spawned) => {
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
        if stdin.eof || !owner_alive(owner) || Instant::now() >= deadline {
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
