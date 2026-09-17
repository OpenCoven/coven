//! Whole-process conformance evidence for the macOS Seatbelt backend.
//!
//! This binary plays three roles, selected only by `argv[0]`:
//! * default: the test, driving the real controller against `SeatbeltDriver`;
//! * `coven-worker-guardian`: the independent guardian (`guardian_main`);
//! * `coven-worker-target`: the sealed worker, which probes what the kernel
//!   lets it do and reports one line per probe on stdout.
//!
//! No harness, provider, shell, network, or ambient credential is involved.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("seatbelt conformance: skipped (macOS only)");
}

#[cfg(target_os = "macos")]
fn main() {
    use coven_restricted_runtime_macos::{guardian_main, role, Role};
    match role() {
        Some(Role::Guardian) => guardian_main(),
        Some(Role::Target) => target::run(),
        None => suite::run(),
    }
}

#[cfg(target_os = "macos")]
mod target {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::process::Command;

    fn report(out: &mut impl Write, name: &str, ok: bool) {
        let _ = writeln!(out, "{name}:{}", if ok { "allowed" } else { "denied" });
    }

    /// Replaces this image with `/bin/ls` through a bare `execv` — no fork, so
    /// the kernel's answer is about `process-exec` alone.
    ///
    /// Returns only when the exec was refused, so the caller reports `denied`.
    /// A profile that wrongly allowed the exec would leave `/bin/ls` running in
    /// the worker's place and neither this probe's line nor any line after it
    /// would reach the test, which asserts on both.
    fn exec_other_allowed() -> bool {
        let Ok(path) = CString::new("/bin/ls") else {
            return false;
        };
        let argv = [path.as_ptr(), std::ptr::null()];
        // SAFETY: execv with a NUL-terminated path and a NULL-terminated argv
        // that both outlive the call. It returns only on failure.
        unsafe {
            libc::execv(path.as_ptr(), argv.as_ptr());
        }
        false
    }

    /// Never reads the environment or its arguments beyond argv[0].
    pub fn run() {
        let exe = PathBuf::from(std::env::args_os().next().expect("argv0"));
        let workspace = exe
            .parent()
            .and_then(|bin| bin.parent())
            .expect("workspace")
            .to_path_buf();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "pid:{}", std::process::id());
        let _ = writeln!(out, "env:{}", std::env::vars_os().count());
        // `closure/escape` asks the worker to leave the process group the
        // guardian created. `setsid` is refused by the kernel for a group
        // leader (EPERM), but `setpgid` into another group of the same session
        // — the guardian's own, whose id is `getppid()` — is not, and Seatbelt
        // has no operation for either call. The fence must hold regardless.
        if workspace.join("closure").join("escape").exists() {
            // SAFETY: setsid/setpgid/getppid/getpgrp/getpid only change or
            // read this process's own session and group membership.
            let escaped = unsafe {
                if libc::setsid() < 0 {
                    libc::setpgid(0, libc::getppid());
                }
                libc::getpgrp() != libc::getpid()
            };
            report(&mut out, "escape", escaped);
        }

        let mut inside = String::new();
        let inside_ok = File::open(workspace.join("closure").join("note.txt"))
            .and_then(|mut f| f.read_to_string(&mut inside))
            .is_ok()
            && inside == "sealed\n";
        report(&mut out, "inside-read", inside_ok);
        report(&mut out, "outside-read", File::open("/etc/hosts").is_ok());
        report(&mut out, "home-read", std::fs::read_dir("/Users").is_ok());
        report(
            &mut out,
            "inside-write",
            File::create(workspace.join("closure").join("scratch")).is_ok(),
        );
        report(
            &mut out,
            "outside-write",
            File::create("/tmp/coven-seatbelt-escape").is_ok(),
        );
        // Flushed first so the probes above survive if the exec below is ever
        // allowed and discards this process's buffered stdout.
        let _ = out.flush();
        report(&mut out, "exec-other", exec_other_allowed());
        // Spawning anything, even its own image, needs `process-fork`, which
        // the profile denies: one worker, no children. This is fork evidence,
        // not exec evidence — `exec-other` above carries that on its own.
        report(&mut out, "spawn-child", Command::new(&exe).output().is_ok());
        let _ = out.flush();
        drop(out);

        if workspace.join("closure").join("linger").exists() {
            std::thread::sleep(std::time::Duration::from_secs(120));
        }
    }
}

#[cfg(target_os = "macos")]
mod suite {
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use coven_restricted_runtime::{CleanupState, Controller, Error, Execution, State, Status};
    use coven_restricted_runtime_macos::{
        ControllerStdio, InstantClock, SealError, SeatbeltConfig, SeatbeltDriver, WorkerStdio,
        GUARDIAN_NAME, TARGET_NAME,
    };

    /// Hang guard only: far above anything load can produce.
    const HANG_GUARD: Duration = Duration::from_secs(20);

    struct Workspace {
        root: PathBuf,
    }

    impl Workspace {
        fn create(label: &str, linger: bool) -> Self {
            Self::create_with(label, linger, false)
        }

        fn create_with(label: &str, linger: bool, escape: bool) -> Self {
            let root = std::env::temp_dir().join(format!(
                "coven-seatbelt-{label}-{}-{}",
                std::process::id(),
                fresh()
            ));
            fs::create_dir_all(root.join("bin")).unwrap();
            fs::create_dir_all(root.join("closure")).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(root.join("bin"), fs::Permissions::from_mode(0o700)).unwrap();
            for name in [TARGET_NAME, GUARDIAN_NAME] {
                let staged = root.join("bin").join(name);
                fs::copy(std::env::current_exe().unwrap(), &staged).unwrap();
                fs::set_permissions(&staged, fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::write(root.join("closure").join("note.txt"), "sealed\n").unwrap();
            if linger {
                fs::write(root.join("closure").join("linger"), "").unwrap();
            }
            if escape {
                fs::write(root.join("closure").join("escape"), "").unwrap();
            }
            Self { root }
        }

        fn adopt(root: PathBuf) -> Self {
            Self { root }
        }

        fn config(&self) -> SeatbeltConfig {
            SeatbeltConfig {
                workspace: self.root.clone(),
            }
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fresh() -> u64 {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(0);
        h.finish()
    }

    type Ctl = Controller<SeatbeltDriver, InstantClock>;

    fn launch(ws: &Workspace, lease_ms: u64) -> (Ctl, ControllerStdio) {
        let (worker, controller_stdio) = WorkerStdio::pipes().unwrap();
        let (driver, clock) = SeatbeltDriver::seal(ws.config(), worker).expect("seal");
        let binding = coven_restricted_runtime::Driver::binding(&driver);
        let mut ctl = Controller::new(driver, clock, binding);
        assert_eq!(ctl.start(lease_ms).unwrap().state, State::Preparing);
        assert_eq!(ctl.advance().unwrap().state, State::Installing);
        assert_eq!(ctl.advance().unwrap().state, State::RetainingLifetime);
        assert_eq!(ctl.advance().unwrap().state, State::Revalidating);
        let status = ctl.advance().unwrap();
        assert_eq!(status.state, State::Running);
        assert_eq!(status.execution, Execution::PossiblyStarted);
        (ctl, controller_stdio)
    }

    fn read_all(stdout: &mut fs::File) -> String {
        let mut text = String::new();
        stdout.read_to_string(&mut text).unwrap();
        text
    }

    /// Reads until the worker's final probe line; the worker may keep running.
    fn read_report(stdout: &mut fs::File) -> String {
        let mut text = Vec::new();
        let mut byte = [0u8; 1];
        while !text.ends_with(b"spawn-child:allowed\n") && !text.ends_with(b"spawn-child:denied\n")
        {
            match stdout.read(&mut byte) {
                Ok(1) => text.push(byte[0]),
                _ => break,
            }
        }
        String::from_utf8(text).unwrap()
    }

    fn await_termination(ctl: &mut Ctl) -> Status {
        let start = Instant::now();
        loop {
            match ctl.observe_termination() {
                Ok(status) if status.state == State::Terminated => return status,
                Ok(_) => {}
                Err(e) => panic!("observation failed: {e:?} after {:?}", start.elapsed()),
            }
            assert!(
                start.elapsed() < HANG_GUARD,
                "worker never terminated within {HANG_GUARD:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_line(report: &str, line: &str) {
        assert!(
            report.lines().any(|l| l == line),
            "expected `{line}` in worker report:\n{report}"
        );
    }

    fn kernel_denies_everything_outside_the_sealed_closure() {
        let ws = Workspace::create("probe", false);
        let (mut ctl, mut stdio) = launch(&ws, 10_000);
        drop(stdio.stdin);
        let report = read_all(&mut stdio.stdout);
        let status = await_termination(&mut ctl);
        assert_eq!(status.reason, None, "{status:?}");

        assert_line(&report, "env:0");
        assert_line(&report, "inside-read:allowed");
        assert_line(&report, "outside-read:denied");
        assert_line(&report, "home-read:denied");
        assert_line(&report, "inside-write:denied");
        assert_line(&report, "outside-write:denied");
        assert_line(&report, "exec-other:denied");
        assert_line(&report, "spawn-child:denied");
        assert!(!Path::new("/tmp/coven-seatbelt-escape").exists());
        assert!(!ws.root.join("closure").join("scratch").exists());

        let stderr = read_all(&mut stdio.stderr);
        assert!(stderr.is_empty(), "worker stderr: {stderr}");
        assert_eq!(
            ctl.request_cleanup().unwrap().cleanup,
            CleanupState::Released
        );
    }

    fn guardian_kills_the_group_when_the_lease_expires() {
        let ws = Workspace::create("expiry", true);
        let started = Instant::now();
        let (mut ctl, mut stdio) = launch(&ws, 600);
        let report = read_all(&mut stdio.stdout);
        assert_line(&report, "inside-read:allowed");
        let status = await_termination(&mut ctl);
        let elapsed = started.elapsed();
        assert_eq!(
            status.reason,
            Some(Error::Expired),
            "{status:?} after {elapsed:?}"
        );
        // Discriminating bound: a lingering worker sleeps 120s; only the
        // guardian's autonomous SIGKILL at the 600ms lease can end it early.
        assert!(
            elapsed >= Duration::from_millis(600) && elapsed < Duration::from_secs(10),
            "terminated after {elapsed:?}"
        );
        assert_eq!(
            ctl.request_cleanup().unwrap().cleanup,
            CleanupState::Released
        );
    }

    fn explicit_cancel_cleanup_terminates_the_group() {
        let ws = Workspace::create("cancel", true);
        let (mut ctl, mut stdio) = launch(&ws, 30_000);
        let report = read_report(&mut stdio.stdout);
        assert_line(&report, "inside-read:allowed");
        ctl.stop_signal().cancel().unwrap();
        assert_eq!(ctl.poll(), Err(Error::Cancelled));
        assert_eq!(ctl.status().state, State::Stopping);
        let status = ctl.request_cleanup().unwrap();
        assert_eq!(status.cleanup, CleanupState::Pending, "{status:?}");
        let status = await_termination(&mut ctl);
        assert_eq!(status.cleanup, CleanupState::Released, "{status:?}");
        assert_eq!(status.execution, Execution::PossiblyStarted);
        assert_eq!(status.reason, Some(Error::Cancelled));
        let mut leftover = String::new();
        let _ = stdio.stderr.read_to_string(&mut leftover);
        assert!(leftover.is_empty(), "worker stderr: {leftover}");
    }

    fn worker_pid(report: &str) -> i32 {
        report
            .lines()
            .find_map(|l| l.strip_prefix("pid:"))
            .and_then(|p| p.parse().ok())
            .expect("worker pid line")
    }

    fn process_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only probes for existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// Child role for the owner-death case: launch a lingering worker, hand
    /// its pid to the parent, then die without any cleanup or fence.
    fn owner_probe(root: PathBuf) -> ! {
        let ws = Workspace::adopt(root);
        let (_ctl, mut stdio) = launch(&ws, 60_000);
        let report = read_report(&mut stdio.stdout);
        println!("{}", worker_pid(&report));
        std::mem::forget(ws);
        std::process::exit(0)
    }

    fn guardian_kills_the_group_when_the_owner_dies() {
        let ws = Workspace::create("owner", true);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--owner-probe")
            .arg(&ws.root)
            .output()
            .expect("owner probe");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let pid: i32 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap();
        // The owner is gone with no controller fence or cleanup; only the
        // independent guardian can end the lingering worker (120s sleep).
        let start = Instant::now();
        while process_alive(pid) {
            assert!(
                start.elapsed() < HANG_GUARD,
                "worker {pid} survived owner death for {:?}",
                start.elapsed()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn seal_refuses_unsafe_workspaces() {
        let ws = Workspace::create("perms", false);
        fs::set_permissions(&ws.root, fs::Permissions::from_mode(0o755)).unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Workspace)
        ));
        fs::set_permissions(&ws.root, fs::Permissions::from_mode(0o700)).unwrap();

        let target = ws.root.join("bin").join(TARGET_NAME);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o722)).unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Executable)
        ));
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();

        fs::remove_dir_all(ws.root.join("closure")).unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Closure)
        ));
        fs::create_dir_all(ws.root.join("closure")).unwrap();

        // A symlink anywhere under `closure/` cannot be pinned by content.
        std::os::unix::fs::symlink("/etc/hosts", ws.root.join("closure").join("hosts")).unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Closure)
        ));
        fs::remove_file(ws.root.join("closure").join("hosts")).unwrap();

        // A FIFO under `closure/` must be refused, not waited on: a plain
        // O_RDONLY open would park until a writer appeared.
        let fifo =
            std::ffi::CString::new(ws.root.join("closure").join("fifo").to_str().unwrap()).unwrap();
        // SAFETY: mkfifo with a NUL-terminated path inside the test workspace.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        let started = Instant::now();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Closure)
        ));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "seal blocked on the FIFO"
        );
        fs::remove_file(ws.root.join("closure").join("fifo")).unwrap();

        let guardian = ws.root.join("bin").join(GUARDIAN_NAME);
        fs::set_permissions(&guardian, fs::Permissions::from_mode(0o722)).unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Guardian)
        ));
        fs::remove_file(&guardian).unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Guardian)
        ));

        // A symlinked `bin/` must not be followed even though the leaf exists.
        let real_bin = ws.root.join("bin-real");
        fs::rename(ws.root.join("bin"), &real_bin).unwrap();
        std::os::unix::fs::symlink(&real_bin, ws.root.join("bin")).unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        assert!(matches!(
            SeatbeltDriver::seal(ws.config(), worker).map(|_| ()),
            Err(SealError::Executable)
        ));
    }

    /// Edits the extended ACL with `/bin/chmod`. Not a shell: the tool is
    /// exec'd directly, and Darwin exposes no libc binding for this.
    fn acl(path: &Path, edit: &[&str]) {
        let status = Command::new("/bin/chmod")
            .args(edit)
            .arg(path)
            .status()
            .expect("chmod");
        assert!(status.success(), "chmod {edit:?} on {}", path.display());
    }

    /// Mode bits say `0700`, caller-owned — the state the old check called
    /// private — while an extended ACL hands another principal write access.
    fn seal_refuses_an_extended_acl() {
        let ws = Workspace::create("acl", false);
        let target = ws.root.join("bin").join(TARGET_NAME);

        for (path, expected) in [
            (ws.root.clone(), SealError::Workspace),
            (target.clone(), SealError::Executable),
        ] {
            acl(&path, &["+a", "everyone allow write"]);
            // The widening must be invisible to a mode-only check, or this
            // case would prove nothing about the ACL.
            let meta = fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o700, "{path:?}");

            let (worker, _ends) = WorkerStdio::pipes().unwrap();
            let refused = SeatbeltDriver::seal(ws.config(), worker).map(|_| ());
            assert_eq!(refused, Err(expected), "{path:?}");

            // Removing the ACL restores the seal: nothing else changed.
            acl(&path, &["-N"]);
            let (worker, _ends) = WorkerStdio::pipes().unwrap();
            assert!(
                SeatbeltDriver::seal(ws.config(), worker).is_ok(),
                "{path:?}"
            );
        }
    }

    fn substituted_executable_is_refused_before_execution() {
        let ws = Workspace::create("swap", false);
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        let (driver, clock) = SeatbeltDriver::seal(ws.config(), worker).expect("seal");
        let binding = coven_restricted_runtime::Driver::binding(&driver);
        let mut ctl = Controller::new(driver, clock, binding);
        ctl.start(10_000).unwrap();
        ctl.advance().unwrap();
        ctl.advance().unwrap();
        ctl.advance().unwrap();
        // Replace the pinned image with a different inode at the same path.
        let target = ws.root.join("bin").join(TARGET_NAME);
        let swapped = ws.root.join("bin").join("swapped");
        fs::copy(&target, &swapped).unwrap();
        fs::rename(&swapped, &target).unwrap();
        let result = ctl.advance();
        assert_eq!(
            result,
            Err(Error::Backend(
                coven_restricted_runtime::Operation::Revalidate,
                coven_restricted_runtime::BackendError::Rejected
            ))
        );
        let status = ctl.status();
        assert_eq!(status.state, State::Refused);
        assert_eq!(status.execution, Execution::NotStarted);
        assert_eq!(
            ctl.request_cleanup().unwrap().cleanup,
            CleanupState::Released
        );
        assert_eq!(ctl.observe_termination(), Err(Error::InvalidState));
    }

    /// A worker that leaves its process group must still die at lease expiry
    /// and on cancel: the guardian fences by pid as well as pgid.
    fn worker_that_leaves_its_process_group_is_still_fenced() {
        // Lease expiry.
        let ws = Workspace::create_with("escape-lease", true, true);
        let started = Instant::now();
        let (mut ctl, mut stdio) = launch(&ws, 600);
        let report = read_report(&mut stdio.stdout);
        assert_line(&report, "escape:allowed");
        let pid = worker_pid(&report);
        let status = await_termination(&mut ctl);
        let elapsed = started.elapsed();
        assert_eq!(status.reason, Some(Error::Expired), "{status:?}");
        assert!(
            elapsed >= Duration::from_millis(600) && elapsed < Duration::from_secs(10),
            "terminated after {elapsed:?}"
        );
        assert!(
            !process_alive(pid),
            "escaped worker {pid} survived the lease"
        );
        assert_eq!(
            ctl.request_cleanup().unwrap().cleanup,
            CleanupState::Released
        );

        // Explicit cancel.
        let ws = Workspace::create_with("escape-cancel", true, true);
        let (mut ctl, mut stdio) = launch(&ws, 30_000);
        let report = read_report(&mut stdio.stdout);
        assert_line(&report, "escape:allowed");
        let pid = worker_pid(&report);
        ctl.stop_signal().cancel().unwrap();
        assert_eq!(ctl.poll(), Err(Error::Cancelled));
        let _ = ctl.request_cleanup().unwrap();
        let status = await_termination(&mut ctl);
        assert_eq!(status.cleanup, CleanupState::Released, "{status:?}");
        assert!(!process_alive(pid), "escaped worker {pid} survived cancel");
    }

    /// The closure is pinned by content, not by its directory inode: editing
    /// a file in place or adding one after seal fails revalidation before the
    /// worker is ever executed.
    fn modified_closure_is_refused_before_execution() {
        for (label, tamper) in [
            (
                "closure-edit",
                (|root: &Path| {
                    fs::write(
                        root.join("closure").join("note.txt"),
                        "tampered
",
                    )
                    .unwrap();
                }) as fn(&Path),
            ),
            ("closure-add", |root: &Path| {
                fs::write(root.join("closure").join("extra"), "").unwrap();
            }),
            ("closure-mode", |root: &Path| {
                fs::set_permissions(
                    root.join("closure").join("note.txt"),
                    fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }),
        ] {
            let ws = Workspace::create(label, false);
            let (worker, _ends) = WorkerStdio::pipes().unwrap();
            let (driver, clock) = SeatbeltDriver::seal(ws.config(), worker).expect("seal");
            let binding = coven_restricted_runtime::Driver::binding(&driver);
            let mut ctl = Controller::new(driver, clock, binding);
            ctl.start(10_000).unwrap();
            ctl.advance().unwrap();
            ctl.advance().unwrap();
            ctl.advance().unwrap();
            tamper(&ws.root);
            let result = ctl.advance();
            assert_eq!(
                result,
                Err(Error::Backend(
                    coven_restricted_runtime::Operation::Revalidate,
                    coven_restricted_runtime::BackendError::Rejected
                )),
                "{label}"
            );
            let status = ctl.status();
            assert_eq!(status.state, State::Refused, "{label}");
            assert_eq!(status.execution, Execution::NotStarted, "{label}");
            assert_eq!(
                ctl.request_cleanup().unwrap().cleanup,
                CleanupState::Released
            );
        }

        // Two seals of the same untouched closure agree on its identity; a
        // different closure has a different identity.
        let ws = Workspace::create("closure-identity", false);
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        let (a, _) = SeatbeltDriver::seal(ws.config(), worker).expect("seal");
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        let (b, _) = SeatbeltDriver::seal(ws.config(), worker).expect("seal");
        let (ba, bb) = (
            coven_restricted_runtime::Driver::binding(&a),
            coven_restricted_runtime::Driver::binding(&b),
        );
        assert_eq!(ba.runtime_closure, bb.runtime_closure);
        fs::write(ws.root.join("closure").join("note.txt"), "other\n").unwrap();
        let (worker, _ends) = WorkerStdio::pipes().unwrap();
        let (c, _) = SeatbeltDriver::seal(ws.config(), worker).expect("seal");
        assert_ne!(
            ba.runtime_closure,
            coven_restricted_runtime::Driver::binding(&c).runtime_closure
        );
    }

    pub fn run() {
        let mut args = std::env::args_os().skip(1);
        if args.next().is_some_and(|a| a == "--owner-probe") {
            owner_probe(PathBuf::from(args.next().expect("workspace path")));
        }
        let cases: [(&str, fn()); 9] = [
            (
                "kernel_denies_everything_outside_the_sealed_closure",
                kernel_denies_everything_outside_the_sealed_closure,
            ),
            (
                "guardian_kills_the_group_when_the_lease_expires",
                guardian_kills_the_group_when_the_lease_expires,
            ),
            (
                "explicit_cancel_cleanup_terminates_the_group",
                explicit_cancel_cleanup_terminates_the_group,
            ),
            (
                "guardian_kills_the_group_when_the_owner_dies",
                guardian_kills_the_group_when_the_owner_dies,
            ),
            (
                "seal_refuses_unsafe_workspaces",
                seal_refuses_unsafe_workspaces,
            ),
            ("seal_refuses_an_extended_acl", seal_refuses_an_extended_acl),
            (
                "substituted_executable_is_refused_before_execution",
                substituted_executable_is_refused_before_execution,
            ),
            (
                "worker_that_leaves_its_process_group_is_still_fenced",
                worker_that_leaves_its_process_group_is_still_fenced,
            ),
            (
                "modified_closure_is_refused_before_execution",
                modified_closure_is_refused_before_execution,
            ),
        ];
        let mut failed = 0;
        for (name, case) in cases {
            match std::panic::catch_unwind(case) {
                Ok(()) => println!("test {name} ... ok"),
                Err(_) => {
                    failed += 1;
                    println!("test {name} ... FAILED");
                }
            }
        }
        println!(
            "\ntest result: {}. {} passed; {failed} failed",
            if failed == 0 { "ok" } else { "FAILED" },
            cases.len() - failed
        );
        if failed != 0 {
            std::process::exit(1);
        }
    }
}
