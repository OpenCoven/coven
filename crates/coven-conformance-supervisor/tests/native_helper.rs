#![cfg(windows)]
use std::{
    fs,
    io::{Read, Write},
    process::Command,
};

#[test]
fn node24_four_pipe_helper_success_and_refusal_matrix() {
    let directory = tempfile::tempdir().unwrap();
    let subject = directory.path().join("subject.exe");
    fs::copy(std::env::current_exe().unwrap(), &subject).unwrap();
    let driver = directory.path().join("driver.mjs");
    fs::write(&driver, include_str!("native_helper.mjs")).unwrap();
    let output = Command::new("node")
        .arg(&driver)
        .arg(env!("CARGO_BIN_EXE_coven-conformance-supervisor"))
        .arg(&subject)
        .arg(directory.path())
        .arg("matrix")
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/automations/runner/conformance.mjs"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "native driver failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("native-helper-matrix-passed"));
    // Raw fixed fixture records survive libtest print capture in successful CI.
    // They contain identities/counts/statuses, never subject payload or paths.
    std::io::stdout().write_all(&output.stdout).unwrap();
}

#[test]
#[ignore = "child entrypoint; exercised by the Node24 matrix parent"]
fn subject() {
    fs::write("entered.pending", std::process::id().to_string()).unwrap();
    fs::rename("entered.pending", "entered.marker").unwrap();
    let mode = fs::read_to_string("mode").unwrap();
    if mode == "hold" {
        loop {
            std::thread::park();
        }
    }
    if mode == "descendants" {
        // Observe target stdin EOF before root exit. This proves the helper's
        // exact body/EOF forwarding completed rather than racing job cleanup.
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap();
        assert_eq!(input, [0, 255, 13, 10, 128, 42]);
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "descendant", "--ignored", "--nocapture"])
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while fs::read_to_string("descendant.pid").ok().as_deref() != Some(&child.id().to_string())
        {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(60),
                "descendant readiness hang guard after {:?}",
                started.elapsed()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Retain the live child and its inherited pipes beyond root exit.
        std::process::exit(0);
    }
    if mode == "overflow" || mode == "stderr-overflow" {
        let bytes = vec![0x41; 2 * 1024 * 1024];
        if mode == "overflow" {
            let _ = std::io::stdout().write_all(&bytes);
        } else {
            let _ = std::io::stderr().write_all(&bytes);
        }
        std::thread::park();
    }
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    if mode == "consumer" {
        assert_eq!(bytes, [0, 255, 13, 10, 128, 42]);
        std::io::stdout().write_all(b"{\"inputBytes\":6}").unwrap();
        std::io::stdout().flush().unwrap();
        std::process::exit(0);
    }
    std::io::stdout().write_all(&bytes).unwrap();
    std::io::stdout().flush().unwrap();
    std::io::stderr().write_all(&[0, 255, 7]).unwrap();
    std::io::stderr().flush().unwrap();
    std::process::exit(if mode == "nonzero" { 23 } else { 0 });
}

#[test]
#[ignore = "descendant entrypoint; exercised by the Node24 matrix parent"]
fn descendant() {
    fs::write("descendant.pending", std::process::id().to_string()).unwrap();
    fs::rename("descendant.pending", "descendant.pid").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn standalone_panic_exit_identifies_compiled_site_without_payload_or_paths() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "panic_exit_fixture", "--ignored", "--nocapture"])
        .output()
        .unwrap();
    let code = output.status.code().expect("native fixture exit status") as u32;
    assert_eq!(code & 0xffff_0000, 0xe0ce_0000);
    assert_ne!(code & 0xffff, 0);
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-panic-payload"));
}

#[test]
#[ignore = "standalone panic entrypoint; exercised by its owning native test"]
fn panic_exit_fixture() {
    coven_conformance_supervisor::panic_site::install_hook();
    panic!("private-panic-payload must never be emitted");
}
