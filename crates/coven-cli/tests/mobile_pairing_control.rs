#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

struct PairingChild(Option<Child>);

impl Drop for PairingChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn run_pairing_control(
    input: &str,
    cancellation_state: &str,
    interrupt_confirmation: bool,
    initial_state: &str,
) -> (Output, Vec<String>) {
    // Keep the Unix socket path short even on macOS.
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let listener = UnixListener::bind(home.path().join("coven.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = PairingChild(Some(
        Command::new(env!("CARGO_BIN_EXE_coven"))
            .env("COVEN_HOME", home.path())
            .args(["memory", "mobile", "pair"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    child
        .0
        .as_mut()
        .unwrap()
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let mut paths = Vec::new();
    let started = Instant::now();
    loop {
        if child.0.as_mut().unwrap().try_wait().unwrap().is_some() {
            return (child.0.take().unwrap().wait_with_output().unwrap(), paths);
        }
        // Hang guard, well above the five-second control-response deadline.
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "pairing CLI wedged after {:?}, requests: {paths:?}",
            started.elapsed()
        );
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => panic!("accept pairing control: {error}"),
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        let path = request.split_whitespace().nth(1).unwrap();
        paths.push(path.to_owned());
        let (status, body) = if path == "/api/v1/internal/mobile/pairings" {
            (
                201,
                json!({
                    "id": "11111111-1111-4111-8111-111111111111",
                    "terminalOutput": "Fixture invitation",
                    "expiresAt": chrono::Utc::now() + chrono::Duration::minutes(5),
                }),
            )
        } else if path.ends_with("/status") {
            (
                200,
                json!({
                    "state": initial_state,
                    "phrase": ["a", "b", "c", "d", "e", "f"],
                }),
            )
        } else if path.ends_with("/confirm") {
            if interrupt_confirmation {
                let pid = i32::try_from(child.0.as_ref().unwrap().id()).unwrap();
                // The request proves the CLI installed its handler. Deliver the
                // interrupt while the daemon's accepted response is still in flight.
                assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
            }
            (409, json!({"state": "waiting_for_confirmation"}))
        } else if path.ends_with("/cancel") {
            (200, json!({"state": cancellation_state, "replayed": false}))
        } else {
            panic!("unexpected pairing control path: {path}");
        };
        let body = body.to_string();
        write!(
            stream,
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }
}

#[test]
fn interrupt_during_accepted_host_confirmation_preserves_device_window() {
    let (output, paths) =
        run_pairing_control("confirm\n", "cancelled", true, "waiting_for_confirmation");
    assert!(
        !paths.iter().any(|path| path.ends_with("/cancel")),
        "accepted confirmation must not be cancelled: {paths:?}, {output:?}"
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Host confirmed."),
        "{output:?}"
    );
}

#[test]
fn decline_reports_expired_instead_of_successful_cancellation() {
    let (output, _) = run_pairing_control("no\n", "expired", false, "waiting_for_confirmation");
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expired"),
        "{output:?}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Mobile pairing cancelled."));
}

#[test]
fn decline_rejects_nonterminal_cancellation_response() {
    let (output, _) = run_pairing_control(
        "no\n",
        "waiting_for_confirmation",
        false,
        "waiting_for_confirmation",
    );
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("non-terminal"),
        "{output:?}"
    );
}

#[test]
fn unavailable_enrollment_exits_without_polling_until_expiry() {
    let (output, paths) = run_pairing_control("", "cancelled", false, "unavailable");
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unavailable after rejected enrollment"),
        "{output:?}"
    );
    assert_eq!(
        paths
            .iter()
            .filter(|path| path.ends_with("/status"))
            .count(),
        1
    );
    assert!(paths.iter().any(|path| path.ends_with("/cancel")));
}

#[test]
fn ordinary_decline_reports_successful_cancellation() {
    let (output, paths) =
        run_pairing_control("no\n", "cancelled", false, "waiting_for_confirmation");
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("Mobile pairing cancelled."));
    assert_eq!(
        paths
            .iter()
            .filter(|path| path.ends_with("/cancel"))
            .count(),
        1
    );
}
