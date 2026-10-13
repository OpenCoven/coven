//! `coven familiar create` end to end (coven#1282), always against a
//! temporary COVEN_HOME and user home.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    coven_home: PathBuf,
}

impl Sandbox {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().to_path_buf();
        for name in ["user-home", "xdg-config", "project"] {
            fs::create_dir(root.join(name))?;
        }
        Ok(Self {
            coven_home: root.join("coven-home"),
            _dir: dir,
            root,
        })
    }

    fn coven(&self, args: &[&str]) -> anyhow::Result<Output> {
        self.coven_with_path(
            args,
            &std::env::var_os("PATH").unwrap_or_default(),
            &self.root,
        )
    }

    fn coven_with_path(
        &self,
        args: &[&str],
        path: &OsString,
        cwd: &Path,
    ) -> anyhow::Result<Output> {
        let user_home = self.root.join("user-home");
        Ok(Command::new(env!("CARGO_BIN_EXE_coven"))
            .args(args)
            .current_dir(cwd)
            .env("COVEN_HOME", &self.coven_home)
            .env("HOME", &user_home)
            .env("USERPROFILE", &user_home)
            .env("XDG_CONFIG_HOME", self.root.join("xdg-config"))
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("COVEN_SETTINGS_PATH")
            .env_remove("COVEN_HARNESS_ADAPTER_DIRS")
            .env_remove("COVEN_HARNESS_ADAPTER_MANIFEST")
            .output()?)
    }

    fn roster(&self) -> PathBuf {
        self.coven_home.join("familiars.toml")
    }
}

fn describe(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not one JSON document: {error}\n{}",
            describe(output)
        )
    })
}

#[test]
fn create_lists_the_familiar_and_refuses_a_second_identical_create() -> anyhow::Result<()> {
    let sandbox = Sandbox::new()?;

    let created = sandbox.coven(&["familiar", "create", "Astra", "--json"])?;
    assert!(created.status.success(), "{}", describe(&created));
    let workspace = sandbox
        .coven_home
        .join("workspaces")
        .join("familiars")
        .join("astra");
    let workspace_text = workspace.to_str().unwrap();
    assert_eq!(
        stdout_json(&created),
        json!({
            "id": "astra",
            "displayName": "Astra",
            "workspace": workspace_text,
            "created": true,
        })
    );
    assert!(workspace.join("SOUL.md").is_file());
    assert!(workspace.join("IDENTITY.md").is_file());

    let listed = sandbox.coven(&["familiars", "--json"])?;
    assert!(listed.status.success(), "{}", describe(&listed));
    let roster = stdout_json(&listed);
    let astra = roster
        .as_array()
        .and_then(|familiars| familiars.iter().find(|familiar| familiar["id"] == "astra"))
        .unwrap_or_else(|| panic!("astra is not listed: {roster}"));
    assert_eq!(astra["display_name"], "Astra");
    assert_eq!(astra["role"], "Familiar");
    assert_eq!(astra["workspace"], workspace_text);

    let before = fs::read(sandbox.roster())?;
    let duplicate = sandbox.coven(&["familiar", "create", "Astra", "--json"])?;
    assert_eq!(duplicate.status.code(), Some(1), "{}", describe(&duplicate));
    let refusal = stdout_json(&duplicate);
    assert_eq!(refusal["created"], false);
    assert_eq!(refusal["error"]["code"], "familiar_exists");
    assert!(
        refusal["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("`astra` already exists")),
        "{refusal}"
    );
    assert!(duplicate.stderr.is_empty(), "{}", describe(&duplicate));
    assert_eq!(fs::read(sandbox.roster())?, before);

    let duplicate_text = sandbox.coven(&["familiar", "create", "Astra"])?;
    assert_eq!(duplicate_text.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&duplicate_text.stderr)
            .contains("Error: familiar `astra` already exists"),
        "{}",
        describe(&duplicate_text)
    );
    assert_eq!(fs::read(sandbox.roster())?, before);

    let second = sandbox.coven(&[
        "familiars",
        "create",
        "Night Owl",
        "--role",
        "Research",
        "--emoji",
        "🦉",
    ])?;
    assert!(second.status.success(), "{}", describe(&second));
    let text = String::from_utf8_lossy(&second.stdout);
    assert!(
        text.starts_with("Created familiar `night-owl` (Night Owl).\n"),
        "{text}"
    );
    assert!(
        fs::read(sandbox.roster())?.starts_with(&before),
        "existing roster bytes changed"
    );
    Ok(())
}

#[test]
fn json_errors_are_reported_on_stdout() -> anyhow::Result<()> {
    let sandbox = Sandbox::new()?;
    fs::create_dir_all(&sandbox.coven_home)?;
    fs::write(sandbox.roster(), "not = [valid")?;

    let broken = sandbox.coven(&["familiar", "create", "Astra", "--json"])?;
    assert_eq!(broken.status.code(), Some(1), "{}", describe(&broken));
    assert_eq!(stdout_json(&broken)["error"]["code"], "roster_invalid");
    assert_eq!(fs::read_to_string(sandbox.roster())?, "not = [valid");

    let invalid = sandbox.coven(&["familiar", "create", "Astra", "--id", "Bad Id", "--json"])?;
    assert_eq!(invalid.status.code(), Some(1), "{}", describe(&invalid));
    assert_eq!(stdout_json(&invalid)["error"]["code"], "invalid_id");
    assert!(!sandbox.coven_home.join("workspaces").exists());
    Ok(())
}

/// The acceptance path from the issue: a created familiar resolves for
/// `coven run --familiar`. `--detach` records the session without spawning
/// the (fake) harness.
#[cfg(unix)]
#[test]
fn coven_run_resolves_a_created_familiar() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new()?;
    let bin = sandbox.root.join("bin");
    fs::create_dir(&bin)?;
    let codex = bin.join("codex");
    fs::write(&codex, "#!/bin/sh\nprintf 'fake codex: %s\\n' \"$*\"\n")?;
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755))?;
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))?;
    let project = sandbox.root.join("project");
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .output()?;
    assert!(init.status.success(), "{}", describe(&init));

    let unknown = sandbox.coven_with_path(
        &["run", "codex", "hi", "--familiar", "astra", "--detach"],
        &path,
        &project,
    )?;
    assert!(!unknown.status.success(), "{}", describe(&unknown));
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("unknown familiar `astra`"),
        "{}",
        describe(&unknown)
    );

    let created = sandbox.coven(&["familiar", "create", "Astra"])?;
    assert!(created.status.success(), "{}", describe(&created));

    let run = sandbox.coven_with_path(
        &["run", "codex", "hi", "--familiar", "astra", "--detach"],
        &path,
        &project,
    )?;
    assert!(run.status.success(), "{}", describe(&run));
    Ok(())
}
