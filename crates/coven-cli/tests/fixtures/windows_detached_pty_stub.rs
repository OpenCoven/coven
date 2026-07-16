#![cfg(windows)]

use std::io::{Read, Write};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(id: i32) -> isize;
    fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: isize, mode: u32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("queries");
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();

    if mode == "queries" {
        let trace_file = args.get(2).expect("query mode requires a trace file");
        let input_mode = set_raw_input_mode().expect("failed to enable raw VT input mode");
        std::fs::write(trace_file, format!("started mode={input_mode:#x}\n")).unwrap();
        output.write_all(b"\x1b[6").unwrap();
        output.flush().unwrap();
        thread::sleep(Duration::from_millis(30));
        output.write_all(b"n").unwrap();
        output.flush().unwrap();
        expect_reply(&mut input, b"\x1b[1;1R", Some(trace_file), "cpr");
        append_trace(trace_file, "cpr\n");

        output.write_all(b"\x1b[c").unwrap();
        output.flush().unwrap();
        expect_da_reply(&mut input, trace_file, "da");
        append_trace(trace_file, "da\n");

        output.write_all(b"\x1b[5n").unwrap();
        output.flush().unwrap();
        expect_reply(&mut input, b"\x1b[0n", Some(trace_file), "status");
        append_trace(trace_file, "status\n");

        output.write_all(b"\x1b[0").unwrap();
        output.flush().unwrap();
        thread::sleep(Duration::from_millis(30));
        output.write_all(b"c").unwrap();
        output.flush().unwrap();
        expect_da_reply(&mut input, trace_file, "da0");
        append_trace(trace_file, "da0\n");

        output.write_all("WINDOWS_PTY_STUB_OK_🎉".as_bytes()).unwrap();
        output.flush().unwrap();
        return;
    }

    let pid_file = args.get(2).expect("timeout mode requires a pid file");
    set_raw_input_mode().expect("failed to enable raw VT input mode");
    let mut descendant = Command::new("cmd.exe")
        .args(["/d", "/c", "ping 127.0.0.1 -n 120 >nul"])
        .spawn()
        .unwrap();
    std::fs::write(pid_file, descendant.id().to_string()).unwrap();
    output.write_all(b"\x1b[6n").unwrap();
    output.flush().unwrap();
    expect_reply(&mut input, b"\x1b[1;1R", None, "timeout-cpr");
    let _ = descendant.wait();
}

fn append_trace(path: &str, text: &str) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

fn expect_reply(input: &mut dyn Read, expected: &[u8], trace_file: Option<&str>, label: &str) {
    let mut seen = Vec::with_capacity(256);
    while seen.len() < 256 {
        let mut byte = [0];
        input.read_exact(&mut byte).unwrap();
        seen.push(byte[0]);
        if let Some(trace_file) = trace_file {
            append_trace(trace_file, &format!("{label} byte={:02x}\n", byte[0]));
        }
        if seen.ends_with(expected) {
            return;
        }
    }
    panic!(
        "expected terminal reply {:?} was absent from input {:?}",
        expected, seen
    );
}

fn expect_da_reply(input: &mut dyn Read, trace_file: &str, label: &str) {
    let mut seen = Vec::with_capacity(256);
    while seen.len() < 256 {
        let mut byte = [0];
        input.read_exact(&mut byte).unwrap();
        seen.push(byte[0]);
        append_trace(trace_file, &format!("{label} byte={:02x}\n", byte[0]));
        if byte[0] != b'c' {
            continue;
        }
        if let Some(start) = seen.windows(3).rposition(|window| window == b"\x1b[?") {
            let parameters = &seen[start + 3..seen.len() - 1];
            if !parameters.is_empty()
                && parameters
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || *byte == b';')
            {
                return;
            }
        }
    }
    panic!("valid terminal DA reply was absent from input {seen:?}");
}

fn set_raw_input_mode() -> Result<u32, &'static str> {
    // SAFETY: these calls only inspect and update the current process's
    // standard-input console mode. Invalid handles simply make this a no-op.
    unsafe {
        let handle = GetStdHandle(-10);
        let mut mode = 0;
        if handle == 0 || handle == -1 || GetConsoleMode(handle, &mut mode) == 0 {
            return Err("GetConsoleMode failed");
        }
        let raw_vt_mode = (mode & !0x7) | 0x200;
        if SetConsoleMode(handle, raw_vt_mode) == 0 {
            return Err("SetConsoleMode failed");
        }
        Ok(raw_vt_mode)
    }
}
