use std::thread;
use std::time::{Duration, Instant};

pub(super) fn wait_for_piped_process_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        #[cfg(target_os = "macos")]
        let exited = macos_process_has_exited(pid).expect("observe fixture process exit");
        #[cfg(not(target_os = "macos"))]
        let exited = {
            let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
            result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        };
        if exited {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(target_os = "macos")]
fn macos_process_has_exited(pid: u32) -> std::io::Result<bool> {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let descriptor = unsafe { libc::kqueue() };
    if descriptor == -1 {
        return Err(io::Error::last_os_error());
    }
    // The successful kqueue call transfers a fresh descriptor to this scope.
    let queue = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let change = libc::kevent {
        ident: pid as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut event = unsafe { std::mem::zeroed::<libc::kevent>() };
    let timeout = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let count = loop {
        // Both arrays hold one event and remain valid for the zero-timeout call.
        let count = unsafe { libc::kevent(queue.as_raw_fd(), &change, 1, &mut event, 1, &timeout) };
        if count != -1 {
            break count;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(true);
        }
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if count == 0 {
        return Ok(false);
    }
    if event.flags & libc::EV_ERROR != 0 {
        let code = i32::try_from(event.data)
            .map_err(|_| io::Error::other("invalid process exit observer error"))?;
        // Darwin no longer permits NOTE_EXIT registration on an unreaped zombie.
        return if code == libc::ESRCH {
            Ok(true)
        } else {
            Err(io::Error::from_raw_os_error(code))
        };
    }
    if count != 1
        || event.ident != pid as usize
        || event.filter != libc::EVFILT_PROC
        || event.fflags & libc::NOTE_EXIT == 0
    {
        return Err(io::Error::other("unexpected process exit observer event"));
    }
    Ok(true)
}

#[cfg(target_os = "macos")]
#[test]
fn process_exit_observation_distinguishes_live_unreaped_and_reaped_children() -> std::io::Result<()>
{
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut child = Command::new("/bin/sleep")
        .arg("120")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let live = wait_for_piped_process_exit(child.id(), Duration::ZERO);
    child.kill()?;
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    // Hold the exited child unreaped so launchd scheduling cannot affect this case.
    let waited = loop {
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            break Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            break Err(error);
        }
    };
    let unreaped = wait_for_piped_process_exit(child.id(), Duration::ZERO);
    child.wait()?;
    waited?;
    let reaped = wait_for_piped_process_exit(child.id(), Duration::ZERO);

    assert!(!live, "a running child must not be reported as exited");
    assert!(
        unreaped,
        "exit must not depend on reaping an already-dead child"
    );
    assert!(reaped, "a reaped child must be reported as exited");
    Ok(())
}
