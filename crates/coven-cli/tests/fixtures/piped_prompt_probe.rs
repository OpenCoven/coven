use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
extern "C" {
    fn close(fd: i32) -> i32;
}

#[cfg(windows)]
#[link(name = "Kernel32")]
extern "system" {
    fn GetStdHandle(kind: u32) -> *mut core::ffi::c_void;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

fn close_stdin() {
    #[cfg(unix)]
    unsafe {
        let _ = close(0);
    }
    #[cfg(windows)]
    unsafe {
        const STD_INPUT_HANDLE: u32 = (-10_i32) as u32;
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        if !handle.is_null() && handle as isize != -1 {
            let _ = CloseHandle(handle);
        }
    }
}

fn required_arg(name: &str) -> String {
    env::args()
        .nth(match name {
            "mode" => 1,
            "first" => 2,
            "second" => 3,
            _ => unreachable!(),
        })
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn optional_arg(name: &str) -> Option<String> {
    env::args().nth(match name {
        "mode" => 1,
        "first" => 2,
        "second" => 3,
        _ => unreachable!(),
    })
}

/// Blocks until `path` exists. A fixed sleep standing in for "the descendant
/// produced output" always loses the race under CPU contention, so the root
/// waits for the descendant to prove it wrote to both inherited pipes.
fn wait_for_marker(path: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if fs::metadata(path).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("descendant readiness marker {path} never appeared");
}

fn main() {
    let mode = required_arg("mode");
    if !mode.starts_with("output-child") {
        if let Ok(delay_ms) = env::var("COVEN_TEST_PIPED_STARTUP_DELAY_MS") {
            thread::sleep(Duration::from_millis(
                delay_ms.parse().expect("startup delay milliseconds"),
            ));
        }
    }
    match mode.as_str() {
        mode @ ("duplex" | "duplex-contained") => {
            let output_bytes = required_arg("first")
                .parse::<usize>()
                .expect("output byte count");
            let receipt = required_arg("second");
            if mode == "duplex-contained" {
                Command::new(env::current_exe().expect("current executable"))
                    .arg("output-child-short")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("spawn contained duplex descendant");
            }
            // Fail a regressed parent-side duplex deadlock in bounded time.
            // The repaired launcher finishes comfortably before this guard.
            thread::spawn(|| {
                thread::sleep(Duration::from_secs(10));
                std::process::exit(124);
            });

            let chunk = vec![b'o'; 8192];
            let mut stdout = io::stdout().lock();
            let mut remaining = output_bytes;
            while remaining != 0 {
                let count = remaining.min(chunk.len());
                stdout.write_all(&chunk[..count]).expect("write stdout");
                remaining -= count;
            }
            stdout.flush().expect("flush stdout");

            let mut prompt = Vec::new();
            io::stdin()
                .read_to_end(&mut prompt)
                .expect("read complete prompt");
            fs::write(receipt, prompt).expect("write prompt receipt");
        }
        "never-read" => {
            let pid_file = required_arg("first");
            fs::write(pid_file, std::process::id().to_string()).expect("write pid");
            println!("ready");
            io::stdout().flush().expect("flush ready marker");
            thread::sleep(Duration::from_secs(120));
        }
        "close-stdin" => {
            let pid_file = required_arg("first");
            close_stdin();
            fs::write(pid_file, std::process::id().to_string()).expect("write pid");
            thread::sleep(Duration::from_secs(120));
        }
        "exit-zero-close-stdin" => {
            let pid_file = required_arg("first");
            close_stdin();
            fs::write(pid_file, std::process::id().to_string()).expect("write pid");
            // Exit successfully without consuming any prompt bytes. The
            // launcher must treat the failed required transport as terminal
            // failure rather than trusting this unrelated exit code.
        }
        "descendant-output" => {
            let pid_file = required_arg("first");
            let child = Command::new(env::current_exe().expect("current executable"))
                .arg("output-child")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn output descendant");
            fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
            // The descendant intentionally owns both inherited output pipes;
            // cancellation is complete only once its containing Job/process
            // group is gone and both parent-side drains have reached EOF.
            thread::sleep(Duration::from_secs(120));
        }
        "root-exit-output-descendant" => {
            let pid_file = required_arg("first");
            let child = Command::new(env::current_exe().expect("current executable"))
                .arg("output-child")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn inherited-output descendant");
            fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
            // Let the descendant prove both inherited pipes are live before
            // the group leader exits successfully.
            thread::sleep(Duration::from_millis(50));
        }
        "root-exit-short-output-descendant" => {
            let pid_file = required_arg("first");
            let ready_marker = optional_arg("second");
            let mut command = Command::new(env::current_exe().expect("current executable"));
            command.arg("output-child-short");
            if let Some(marker) = ready_marker.as_ref() {
                command.arg(marker);
            }
            let child = command
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn short inherited-output descendant");
            fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
            // Exit only once the descendant has proven both inherited pipes
            // carry real output.
            match ready_marker.as_deref() {
                Some(marker) => wait_for_marker(marker),
                None => thread::sleep(Duration::from_millis(50)),
            }
        }
        "root-exit-closed-descendant" => {
            let pid_file = required_arg("first");
            let child = Command::new(env::current_exe().expect("current executable"))
                .arg("output-child")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn closed-pipe descendant");
            fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
            // Exit successfully while the descendant remains alive and has
            // already closed every pipe observed by the supervisor.
        }
        "closed-descendant" => {
            let pid_file = required_arg("first");
            let child = Command::new(env::current_exe().expect("current executable"))
                .arg("output-child")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn closed-pipe descendant");
            fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
            thread::sleep(Duration::from_secs(120));
        }
        "output-child" => loop {
            io::stdout().write_all(b"stdout-tick\n").expect("stdout tick");
            io::stdout().flush().expect("flush stdout tick");
            io::stderr().write_all(b"stderr-tick\n").expect("stderr tick");
            io::stderr().flush().expect("flush stderr tick");
            thread::sleep(Duration::from_millis(5));
        },
        "output-child-short" => {
            let ready_marker = optional_arg("first");
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let mut announced = false;
            while std::time::Instant::now() < deadline {
                io::stdout().write_all(b"stdout-tick\n").expect("stdout tick");
                io::stdout().flush().expect("flush stdout tick");
                io::stderr().write_all(b"stderr-tick\n").expect("stderr tick");
                io::stderr().flush().expect("flush stderr tick");
                if !announced {
                    // Both inherited pipes now hold real bytes; release the
                    // root so it can exit.
                    if let Some(marker) = ready_marker.as_ref() {
                        fs::write(marker, b"ready").expect("write readiness marker");
                    }
                    announced = true;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
        mode => panic!("unsupported probe mode {mode}"),
    }
}
