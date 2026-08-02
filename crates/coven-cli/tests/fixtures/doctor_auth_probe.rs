use std::fs::OpenOptions;
use std::io::Write;
use std::process::{self, Command};
use std::thread;
use std::time::Duration;

fn main() {
    let executable = std::env::current_exe().expect("resolve probe executable");
    let invoked_as = executable
        .file_stem()
        .and_then(|name| name.to_str())
        .expect("probe executable name")
        .to_ascii_lowercase();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--hold-stdout"] {
        thread::sleep(Duration::from_secs(60));
        return;
    }
    record_invocation(&invoked_as, &args);

    if invoked_as != "coven-code" {
        // Provider executables exist so Doctor can discover them, but Doctor
        // must never launch one. A launch is recorded above before failing.
        process::exit(91);
    }

    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--version"] => {
            println!("coven-code 0.6.1");
        }
        ["auth", "status", "--json"] => match std::env::var("COVEN_DOCTOR_PROBE_MODE")
            .as_deref()
            .unwrap_or("configured")
        {
            "configured" => println!(r#"{{"loggedIn":true}}"#),
            "unconfigured" => {
                println!(r#"{{"loggedIn":false}}"#);
                process::exit(1);
            }
            "exit2" => {
                println!(r#"{{"loggedIn":false}}"#);
                process::exit(2);
            }
            "timeout" => {
                thread::sleep(Duration::from_secs(60));
                record_timeout_completion();
                println!(r#"{{"loggedIn":true}}"#);
            }
            "descendant" => {
                let descendant = Command::new(&executable)
                    .arg("--hold-stdout")
                    .spawn()
                    .expect("spawn stdout-holding descendant");
                record_descendant_pid(descendant.id());
                println!(r#"{{"loggedIn":true}}"#);
            }
            other => panic!("unknown probe mode: {other}"),
        },
        _ => process::exit(92),
    }
}

fn record_descendant_pid(pid: u32) {
    let path = std::env::var_os("COVEN_DOCTOR_DESCENDANT_PID_FILE")
        .expect("descendant pid file path");
    std::fs::write(path, pid.to_string()).expect("record descendant pid");
}

fn record_invocation(invoked_as: &str, args: &[String]) {
    let log_path = std::env::var_os("COVEN_DOCTOR_PROBE_LOG").expect("probe log path");
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .expect("open probe log");
    writeln!(
        log,
        "{invoked_as}\t{}\t{}",
        process::id(),
        args.join("\u{1f}")
    )
    .expect("record probe invocation");
}

fn record_timeout_completion() {
    let log_path = std::env::var_os("COVEN_DOCTOR_PROBE_LOG").expect("probe log path");
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .expect("open probe log");
    writeln!(log, "timeout-completed\t{}\t", process::id())
        .expect("record unexpected timeout completion");
}
