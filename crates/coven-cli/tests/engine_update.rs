//! Real CLI update/activation journeys with offline release fixtures.
#[cfg(unix)]
#[test]
fn engine_update_cli_journeys() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/engine-update-test.py");
    let output = std::process::Command::new("python3")
        .arg(script)
        .args(["--binary", env!("CARGO_BIN_EXE_coven")])
        .output()
        .expect("python3 is required for the offline engine-update fixtures");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
