//! Native Windows console-subsystem probe.
//!
//! This file is compiled directly with `rustc.exe` by Windows-only Rust and
//! npm wrapper tests. It intentionally remains a console-subsystem binary:
//! `GetConsoleWindow` can then distinguish an ordinary inherited/allocated
//! console from a child created with `CREATE_NO_WINDOW`. Optional modes also
//! provide a long-lived descendant and an exact exit code for native process-
//! containment regressions without relying on PowerShell or a shell shim.

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

#[link(name = "Kernel32")]
extern "system" {
    fn GetConsoleWindow() -> *mut core::ffi::c_void;
    fn FreeConsole() -> i32;
    fn AttachConsole(process_id: u32) -> i32;
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
    fn GenerateConsoleCtrlEvent(event: u32, process_group_id: u32) -> i32;
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--launch-new-console") => {
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            let program = args
                .next()
                .expect("--launch-new-console requires a program");
            let output = Command::new(program)
                .args(args)
                .creation_flags(CREATE_NEW_CONSOLE)
                .output()
                .expect("failed to launch positive-control console child");
            std::io::stdout()
                .write_all(&output.stdout)
                .expect("failed forwarding positive-control stdout");
            std::io::stderr()
                .write_all(&output.stderr)
                .expect("failed forwarding positive-control stderr");
            std::process::exit(output.status.code().unwrap_or(1));
        }
        Some("--launch-new-console-ctrl-c") => {
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            const CTRL_C_EVENT: u32 = 0;
            let ready_file = args
                .next()
                .expect("--launch-new-console-ctrl-c requires a ready file");
            let program = args
                .next()
                .expect("--launch-new-console-ctrl-c requires a program");
            let mut child = Command::new(program)
                .args(args)
                .creation_flags(CREATE_NEW_CONSOLE)
                .spawn()
                .expect("failed to launch console-control target");
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !std::path::Path::new(&ready_file).exists() {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("console-control target did not publish readiness");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            unsafe {
                let _ = FreeConsole();
                assert_ne!(
                    AttachConsole(child.id()),
                    0,
                    "failed to attach to positive-control console"
                );
                assert_ne!(
                    SetConsoleCtrlHandler(None, 1),
                    0,
                    "failed to ignore the helper's own Ctrl-C event"
                );
                assert_ne!(
                    GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0),
                    0,
                    "failed to generate Ctrl-C for the wrapper process group"
                );
                let _ = FreeConsole();
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().expect("failed waiting for Ctrl-C target") {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("console-control target did not exit after Ctrl-C");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            println!("ctrl-c-exit={}", status.code().unwrap_or(-1));
            std::process::exit(0);
        }
        Some("--spawn-descendant") => {
            let pid_file = args.next().expect("--spawn-descendant requires a pid file");
            let child = Command::new("ping.exe")
                .args(["-n", "120", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("failed to spawn descendant fixture");
            std::fs::write(pid_file, child.id().to_string())
                .expect("failed to write descendant pid");
            println!("descendant={}", child.id());
            std::io::stdout().flush().expect("failed to flush fixture output");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        Some("--ctrl-c-ready") => {
            let ready_file = args.next().expect("--ctrl-c-ready requires a ready file");
            let pid_file = args.next().expect("--ctrl-c-ready requires a pid file");
            // Ignore the console broadcast in the native fixture itself. The
            // Node wrapper must receive Ctrl-C and explicitly terminate this
            // child, proving forwarding rather than shared-console delivery.
            unsafe {
                assert_ne!(
                    SetConsoleCtrlHandler(None, 1),
                    0,
                    "failed to make native Ctrl-C fixture ignore broadcasts"
                );
            }
            std::fs::write(pid_file, std::process::id().to_string())
                .expect("failed to write Ctrl-C fixture pid");
            std::fs::write(ready_file, b"ready\n")
                .expect("failed to write Ctrl-C fixture readiness");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        Some("--exit-code") => {
            let code = args
                .next()
                .expect("--exit-code requires a value")
                .parse::<i32>()
                .expect("exit code must be an integer");
            std::process::exit(code);
        }
        Some("--echo-stdio-exit") => {
            let code = args
                .next()
                .expect("--echo-stdio-exit requires an exit code")
                .parse::<i32>()
                .expect("exit code must be an integer");
            let state = if unsafe { GetConsoleWindow() }.is_null() {
                "console=absent"
            } else {
                "console=present"
            };
            let mut input = Vec::new();
            std::io::stdin()
                .read_to_end(&mut input)
                .expect("failed reading forwarded stdin");
            println!("{state}");
            std::io::stdout()
                .write_all(&input)
                .expect("failed writing forwarded stdout");
            std::io::stdout()
                .flush()
                .expect("failed flushing forwarded stdout");
            eprintln!("stderr=forwarded");
            std::process::exit(code);
        }
        Some("--spam-stdout") => {
            let chunk = vec![b'x'; 64 * 1024];
            let mut stdout = std::io::stdout().lock();
            loop {
                if stdout.write_all(&chunk).is_err() || stdout.flush().is_err() {
                    break;
                }
            }
        }
        Some("--print-env") => {
            let name = args.next().expect("--print-env requires a name");
            let present = std::env::vars()
                .any(|(key, _)| key.eq_ignore_ascii_case(&name));
            println!("env={}", if present { "present" } else { "absent" });
        }
        Some("--record-argv-env") => {
            let argv_file = args
                .next()
                .expect("--record-argv-env requires an argv file");
            let env_file = args
                .next()
                .expect("--record-argv-env requires an env file");
            let remaining = args.collect::<Vec<_>>();
            std::fs::write(argv_file, remaining.join("\0")).expect("failed recording argv");
            let package_root = std::env::var("CODEX_MANAGED_PACKAGE_ROOT")
                .unwrap_or_else(|_| "<absent>".to_string());
            let managers = [
                "CODEX_MANAGED_BY_NPM",
                "CODEX_MANAGED_BY_BUN",
                "CODEX_MANAGED_BY_PNPM",
            ]
            .into_iter()
            .filter(|name| std::env::var(name).as_deref() == Ok("1"))
            .collect::<Vec<_>>()
            .join(",");
            std::fs::write(
                env_file,
                format!("package_root={package_root}\nmanagers={managers}\n"),
            )
            .expect("failed recording managed-package environment");
            let state = if unsafe { GetConsoleWindow() }.is_null() {
                "console=absent"
            } else {
                "console=present"
            };
            println!("{state}");
            return;
        }
        Some(other) => {
            if let Some(pid_file) = std::env::var_os("COVEN_TEST_WINDOWS_CODEX_DESCENDANT_PID_FILE")
            {
                let child = Command::new("ping.exe")
                    .args(["-n", "120", "127.0.0.1"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("failed to spawn Codex-style descendant fixture");
                std::fs::write(pid_file, child.id().to_string())
                    .expect("failed to write Codex-style descendant pid");
                println!("codex-descendant={}", child.id());
                std::io::stdout()
                    .flush()
                    .expect("failed to flush Codex-style fixture output");
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            panic!("unknown fixture mode: {other}");
        }
        None => {}
    }
    let state = if unsafe { GetConsoleWindow() }.is_null() {
        "console=absent"
    } else {
        "console=present"
    };
    println!("{state}");
}
