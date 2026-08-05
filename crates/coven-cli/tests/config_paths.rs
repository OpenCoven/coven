use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

fn run_paths(temp: &TempDir, coven_home: &Path, extra: &[(&str, OsString)]) -> Output {
    let profile_home = temp.path().join("profile");
    let xdg_config_home = temp.path().join("xdg-config");
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    command
        .args(["config", "paths", "--json"])
        .current_dir(temp.path())
        .env_remove("COVEN_ENGINE_BIN")
        .env_remove("COVEN_HARNESS_ADAPTER_MANIFEST")
        .env_remove("COVEN_HARNESS_ADAPTER_DIRS")
        .env("COVEN_HOME", coven_home)
        .env("HOME", &profile_home)
        .env("USERPROFILE", &profile_home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env("XDG_CONFIG_HOME", &xdg_config_home);
    // `PATH` is also an engine source. Keep it empty so this isolated-process
    // fixture cannot resolve a developer or CI runner's installed engine.
    command.env("PATH", temp.path().join("empty-path"));
    for (name, value) in extra {
        command.env(name, value);
    }
    command.output().expect("run coven config paths")
}

fn report(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid JSON report")
}

fn surfaces(report: &Value) -> BTreeMap<&str, &Value> {
    report["surfaces"]
        .as_array()
        .expect("surfaces array")
        .iter()
        .map(|surface| (surface["id"].as_str().expect("surface id"), surface))
        .collect()
}

#[test]
fn paths_json_is_stable_and_creates_no_profile_state() {
    let temp = TempDir::new().expect("temporary directory");
    let coven_home = temp.path().join("isolated-coven-home");

    #[cfg(not(windows))]
    let expected_managed_engine_root = temp.path().join("profile").join(".coven").join("engine");
    #[cfg(windows)]
    let expected_managed_engine_root = dirs_next::home_dir()
        .expect("platform user home")
        .join(".coven")
        .join("engine");

    let first = run_paths(&temp, &coven_home, &[]);
    let second = run_paths(&temp, &coven_home, &[]);

    assert_eq!(first.stdout, second.stdout, "output must be byte-stable");
    assert!(
        first.stderr.is_empty(),
        "machine-readable diagnostic must not emit startup diagnostics: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report = report(&first);
    assert_eq!(report["schema"], "coven.config.paths");
    assert_eq!(report["version"], 1);
    assert!(
        !coven_home.exists(),
        "the diagnostic must not create COVEN_HOME"
    );
    assert!(
        !temp.path().join("xdg-config").exists(),
        "the diagnostic must not create a settings directory"
    );
    let surfaces = surfaces(&report);
    assert_eq!(
        surfaces["store.session_ledger"]["path"],
        coven_home.join("coven.sqlite3").display().to_string(),
        "missing state is still reported as a resolved prospective path"
    );
    assert_eq!(surfaces["store.session_ledger"]["status"], "resolved");
    assert_eq!(
        surfaces["state.executor_node_config"]["path"],
        coven_home.join("executor.json").display().to_string()
    );
    assert_eq!(
        surfaces["state.skills"]["path"],
        coven_home.join("skills").display().to_string()
    );
    assert_eq!(
        surfaces["state.memory_migrations"]["path"],
        coven_home.join("memory-migrations").display().to_string()
    );
    assert_eq!(
        surfaces["state.shared_lock"]["path"],
        coven_home.join("state.lock").display().to_string()
    );
    assert_eq!(
        surfaces["state.reset_transaction"]["path"],
        coven_home
            .join("reset-transaction.json")
            .display()
            .to_string()
    );
    assert_eq!(
        surfaces["state.daemon_lifecycle_lock"]["path"],
        coven_home.join("daemon.lock").display().to_string()
    );
    assert_eq!(
        surfaces["state.daemon_serve_lock"]["path"],
        coven_home.join("daemon-serve.lock").display().to_string()
    );
    assert_eq!(
        surfaces["state.pending_proposals"]["path"],
        coven_home.join("pending").display().to_string()
    );
    assert_eq!(
        surfaces["state.travel_profiles"]["path"],
        coven_home
            .join("travel")
            .join("profiles")
            .display()
            .to_string()
    );
    assert_eq!(
        surfaces["logs.daemon_recovery"]["path"],
        coven_home.join("daemon-recovery.log").display().to_string()
    );
    #[cfg(windows)]
    {
        let daemon_ipc = surfaces["state.daemon_ipc"]["path"]
            .as_str()
            .expect("Windows daemon IPC path");
        assert!(
            daemon_ipc.starts_with(r"\\.\pipe\coven-daemon-"),
            "expected fully qualified Coven named pipe, got {daemon_ipc}"
        );
        assert!(daemon_ipc.ends_with(".sock"));
        assert_eq!(surfaces["state.daemon_ipc"]["status"], "resolved");
        assert_eq!(surfaces["state.daemon_ipc"]["source"], "environment");
    }
    assert_eq!(
        surfaces["engine.managed_cache"]["path"],
        expected_managed_engine_root.display().to_string(),
        "the managed engine cache follows the platform user home, not COVEN_HOME"
    );
    #[cfg(unix)]
    assert_eq!(
        surfaces["engine.managed_cache"]["source"], "environment",
        "the managed engine cache inherits the isolated HOME override"
    );
    #[cfg(windows)]
    assert_eq!(
        surfaces["engine.managed_cache"]["source"], "default",
        "Windows uses the native profile resolver rather than HOME or USERPROFILE"
    );
    assert_eq!(
        surfaces["engine.resolved_binary"]["status"], "not_applicable",
        "an empty PATH and missing managed cache must not leak a runner engine"
    );
    assert_eq!(
        surfaces["dashboard.memory_companion_state"]["status"],
        "unsupported"
    );

    for surface in report["surfaces"].as_array().expect("surfaces array") {
        assert_eq!(surface["access"], "read_only");
        if let Some(path) = surface.get("path").and_then(Value::as_str) {
            assert!(Path::new(path).is_absolute(), "{path} must be absolute");
        }
        for path in surface["paths"].as_array().into_iter().flatten() {
            let path = path.as_str().expect("path string");
            assert!(Path::new(path).is_absolute(), "{path} must be absolute");
        }
    }
}

#[test]
fn paths_json_prefers_environment_roots_without_reading_secret_contents() {
    let temp = TempDir::new().expect("temporary directory");
    let coven_home = temp.path().join("selected-coven-home");
    let xdg_settings = temp.path().join("xdg-config/coven/settings.json");
    let manifest = temp.path().join("external/adapter-manifest.toml");
    let adapter_one = temp.path().join("external/adapter-one");
    let adapter_two = temp.path().join("external/adapter-two");
    let sensitive_content = "secret-value-that-must-never-appear-in-the-report";

    fs::create_dir_all(xdg_settings.parent().expect("settings parent"))
        .expect("create settings parent");
    fs::write(
        &xdg_settings,
        format!(r#"{{"ignoredValue":"{sensitive_content}"}}"#),
    )
    .expect("write settings");
    fs::create_dir_all(manifest.parent().expect("manifest parent"))
        .expect("create manifest parent");
    fs::write(
        &manifest,
        format!("ignored_value = {sensitive_content:?}\n"),
    )
    .expect("write manifest");
    let adapter_dirs =
        std::env::join_paths([&adapter_one, &adapter_two]).expect("join adapter directories");
    let output = run_paths(
        &temp,
        &coven_home,
        &[
            (
                "COVEN_HARNESS_ADAPTER_MANIFEST",
                manifest.clone().into_os_string(),
            ),
            ("COVEN_HARNESS_ADAPTER_DIRS", adapter_dirs),
        ],
    );

    let stdout = String::from_utf8(output.stdout.clone()).expect("UTF-8 report");
    assert!(
        !stdout.contains(sensitive_content),
        "the report must never include configuration contents"
    );
    let report = report(&output);
    let surfaces = surfaces(&report);
    assert_eq!(
        surfaces["coven.home"]["path"],
        coven_home.display().to_string()
    );
    assert_eq!(surfaces["coven.home"]["source"], "environment");
    assert_eq!(
        surfaces["settings.user"]["path"],
        temp.path()
            .join("xdg-config")
            .join("coven")
            .join("settings.json")
            .display()
            .to_string()
    );
    assert_eq!(surfaces["settings.user"]["source"], "environment");
    assert_eq!(
        surfaces["adapters.external_manifest"]["path"],
        manifest.display().to_string()
    );
    assert_eq!(
        surfaces["adapters.external_roots"]["paths"],
        serde_json::json!([
            adapter_one.display().to_string(),
            adapter_two.display().to_string(),
        ])
    );
}

#[test]
fn paths_json_reports_configured_familiar_workspaces() {
    let temp = TempDir::new().expect("temporary directory");
    let coven_home = temp.path().join("selected-coven-home");
    let external_workspace = temp.path().join("external").join("nova");
    fs::create_dir_all(&coven_home).expect("create COVEN_HOME");
    fs::write(
        coven_home.join("familiars.toml"),
        format!(
            r#"
[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Reads."
workspace = "relative/sage"

[[familiar]]
id = "nova"
display_name = "Nova"
role = "Orchestration"
description = "Coordinates."
workspace = {}

[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "Builds."
"#,
            serde_json::to_string(&external_workspace).expect("serialize workspace")
        ),
    )
    .expect("write familiar manifest");

    let report = report(&run_paths(&temp, &coven_home, &[]));
    let surfaces = surfaces(&report);
    let reported_paths: Vec<_> = surfaces["state.familiar_workspaces"]["paths"]
        .as_array()
        .expect("workspace paths")
        .iter()
        .map(|path| Path::new(path.as_str().expect("workspace path")).to_path_buf())
        .collect();
    assert_eq!(reported_paths.len(), 3);
    assert!(reported_paths[0].is_absolute());
    assert!(reported_paths[0].ends_with(Path::new("relative/sage")));
    assert_eq!(reported_paths[1], external_workspace);
    assert_eq!(reported_paths[2], coven_home.join("familiars/cody"));
    assert_eq!(
        surfaces["state.familiar_workspaces"]["source"],
        "configuration"
    );
    assert!(
        surfaces["state.familiar_workspaces"].get("path").is_none(),
        "configured workspaces must replace the misleading default root"
    );
}

#[test]
fn paths_json_does_not_guess_workspaces_when_familiar_manifest_is_invalid() {
    let temp = TempDir::new().expect("temporary directory");
    let coven_home = temp.path().join("selected-coven-home");
    fs::create_dir_all(&coven_home).expect("create COVEN_HOME");
    fs::write(coven_home.join("familiars.toml"), "[[familiar]\n")
        .expect("write invalid familiar manifest");

    let report = report(&run_paths(&temp, &coven_home, &[]));
    let surface = &surfaces(&report)["state.familiar_workspaces"];

    assert_eq!(surface["status"], "unresolved");
    assert!(surface.get("path").is_none());
    assert!(surface.get("paths").is_none());
}

#[test]
fn paths_requires_machine_readable_output_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["config", "paths"])
        .output()
        .expect("run coven config paths");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires `--json`"));
}

#[test]
fn paths_uses_coven_home_over_profile_home_and_ignores_an_invalid_engine_override() {
    let temp = TempDir::new().expect("temporary directory");
    let coven_home = temp.path().join("explicit-coven-home");
    let missing_engine = temp.path().join("missing-engine");
    let report = report(&run_paths(
        &temp,
        &coven_home,
        &[("COVEN_ENGINE_BIN", missing_engine.into_os_string())],
    ));
    let surfaces = surfaces(&report);

    assert_eq!(
        surfaces["coven.home"]["path"],
        coven_home.display().to_string(),
        "COVEN_HOME must win over the isolated profile home"
    );
    assert_eq!(surfaces["coven.home"]["source"], "environment");
    assert_eq!(
        surfaces["engine.resolved_binary"]["status"], "not_applicable",
        "an unusable COVEN_ENGINE_BIN must not be reported as a resolved binary"
    );
}
