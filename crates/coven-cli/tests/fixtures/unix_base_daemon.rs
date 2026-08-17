#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::Path;

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                write!(escaped, "\\u{:04x}", character as u32).expect("write JSON escape");
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn relative_path(base: &Path, target: &Path) -> std::path::PathBuf {
    let base = base.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();
    let shared = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = std::path::PathBuf::new();
    for _ in shared..base.len() {
        relative.push("..");
    }
    for component in &target[shared..] {
        relative.push(component.as_os_str());
    }
    relative
}

fn write_response(stream: &mut std::os::unix::net::UnixStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .expect("write BASE fixture response");
}

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let coven_home = arguments.next().expect("missing Coven home");
    let started_at = arguments
        .next()
        .expect("missing started-at identity")
        .into_string()
        .expect("started-at identity was not UTF-8");
    let reported_pid = match arguments
        .next()
        .expect("missing reported PID")
        .to_str()
        .expect("reported PID was not UTF-8")
    {
        "self" => std::process::id(),
        pid => pid.parse().expect("reported PID was invalid"),
    };
    let health_style = arguments
        .next()
        .expect("missing health style")
        .into_string()
        .expect("health style was not UTF-8");
    let coven_home = Path::new(&coven_home);
    fs::create_dir_all(coven_home).expect("create Coven home");
    fs::set_permissions(coven_home, fs::Permissions::from_mode(0o700))
        .expect("protect Coven home");
    let socket = coven_home.join("coven.sock");
    let status = coven_home.join("daemon.json");
    let requests = coven_home.join("base-requests.log");
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("bind BASE daemon socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("protect BASE daemon socket");
    let recorded_socket = match std::env::var("COVEN_TEST_BASE_RECORDED_SOCKET").as_deref() {
        Ok("relative") => relative_path(
            &std::env::current_dir().expect("read BASE fixture current directory"),
            &socket,
        ),
        Ok(other) => panic!("unknown recorded socket style {other}"),
        Err(std::env::VarError::NotPresent) => socket.clone(),
        Err(error) => panic!("invalid recorded socket style: {error}"),
    };
    let socket_json = json_string(recorded_socket.to_string_lossy().as_ref());
    let started_at_json = json_string(&started_at);
    let daemon = format!(
        r#"{{"pid":{reported_pid},"startedAt":{started_at_json},"socket":{socket_json}}}"#
    );
    let capabilities = match health_style.as_str() {
        "base" => r#"{"sessions":true,"events":true,"travel":true,"scheduler":true,"hub":true,"executorDispatch":true,"eventCursor":"sequence","structuredErrors":true,"sessionHandoff":true,"sessionLaunchPolicy":false,"afs":true,"afsMount":false,"afsCommit":true,"afsCommitDryRun":true,"executionBindingContracts":["psyche.execution_binding.v1"]}"#,
        "incompatible" => r#"{"structuredErrors":true}"#,
        other => panic!("unknown health style {other}"),
    };
    let health = format!(
        r#"{{"ok":true,"apiVersion":"coven.daemon.v1","covenVersion":"0.1.0","capabilities":{capabilities},"daemon":{daemon}}}"#
    );
    fs::write(&status, &daemon).expect("publish BASE daemon status");
    fs::set_permissions(&status, fs::Permissions::from_mode(0o600))
        .expect("protect BASE daemon status");

    for incoming in listener.incoming() {
        let mut stream = incoming.expect("accept BASE daemon connection");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read BASE daemon request");
        let request_line = request.lines().next().unwrap_or_default();
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&requests)
            .expect("open BASE request log");
        writeln!(log, "{request_line}").expect("record BASE request");
        if request_line == "GET /health HTTP/1.1" {
            write_response(&mut stream, 200, &health);
        } else {
            write_response(
                &mut stream,
                404,
                r#"{"error":{"code":"not_found","message":"route not found"}}"#,
            );
        }
    }
}
