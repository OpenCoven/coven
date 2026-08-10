use std::env;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn arg(index: usize, name: &str) -> String {
    env::args().nth(index).unwrap_or_else(|| panic!("missing {name}"))
}

fn main() {
    match arg(1, "mode").as_str() {
        "output-exit" => {
            let code = arg(2, "exit code").parse::<i32>().expect("exit code");
            print!("supervised-stdout");
            io::stdout().flush().expect("flush stdout");
            eprint!("supervised-stderr");
            io::stderr().flush().expect("flush stderr");
            std::process::exit(code);
        }
        "descendant" => {
            let pid_file = arg(2, "pid file");
            let child = Command::new(env::current_exe().expect("current executable"))
                .arg("worker")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn descendant");
            std::fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
            thread::sleep(Duration::from_secs(120));
        }
        "descendant-with-root" => {
            let root_pid_file = arg(2, "root pid file");
            let descendant_pid_file = arg(3, "descendant pid file");
            let child = Command::new(env::current_exe().expect("current executable"))
                .arg("worker")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn descendant");
            std::fs::write(root_pid_file, std::process::id().to_string())
                .expect("write root pid");
            std::fs::write(descendant_pid_file, child.id().to_string())
                .expect("write descendant pid");
            thread::sleep(Duration::from_secs(120));
        }
        "root-exit-descendant" => {
            let pid_file = arg(2, "pid file");
            let child = Command::new(env::current_exe().expect("current executable"))
                .arg("worker")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn descendant");
            std::fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
        }
        "worker" => thread::sleep(Duration::from_secs(120)),
        mode => panic!("unsupported mode {mode}"),
    }
}
