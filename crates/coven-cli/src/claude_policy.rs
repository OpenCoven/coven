//! Coven's permission policy for the Claude Code processes it starts (#1738).
//!
//! Claude Code merges settings from the operator's `~/.claude`, the workspace
//! and the command line, so one chat behaved differently on each machine. A
//! headless launch has nobody to answer a prompt: whatever those files did not
//! allow was refused. Coven supplies the policy itself:
//!
//! - `policy/claude-settings.json` is compiled into the binary, so installing
//!   or updating Coven provisions it. It is written under the Coven home only
//!   when its content differs.
//! - `--settings <that file>` sits above every settings file, and no other
//!   source can undo one of its deny rules.
//! - `--setting-sources project` leaves out the operator's `~/.claude` and the
//!   workspace's `settings.local.json`. The workspace's instructions
//!   (`AGENTS.md`, `CLAUDE.md`) and its checked-in `.claude/settings.json`
//!   still load; an empty list would drop the instructions as well.
//! - Leaving out the user source also drops the MCP servers in the operator's
//!   Claude config file, so that file is named with `--mcp-config` instead.
//!
//! Administrator-managed settings stay above all of this, and Coven never
//! writes them. The policy covers headless launches, `--stream-json` and
//! non-interactive. The engine starts Claude Code itself, so a headless
//! engine launch receives the same policy through [`ENGINE_POLICY_ENV`],
//! with the permission Coven was asked for.
//!
//! Unchanged: interactive terminal launches, where a person can answer, and
//! `--write-scope` launches, which carry their own complete policy. An
//! interactive engine can change its permission while it runs, and only the
//! engine knows the current one, so that case is the engine's to cover.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::harness::{HarnessLaunchMode, HarnessLaunchOptions, LaunchPolicy, Permission};

/// The policy, as Claude Code settings.
const POLICY: &str = include_str!("../policy/claude-settings.json");
const POLICY_DIR: &str = "policy";
const POLICY_FILE: &str = "claude-settings.json";
/// The settings files Claude Code still reads beside the policy.
const SETTING_SOURCES: &str = "project";
/// Names a JSON object for the engine: `version`, `settingsFile`,
/// `settingSources`, `permissionMode`, `addDirs` and `mcpConfigFile`.
pub(crate) const ENGINE_POLICY_ENV: &str = "COVEN_CLAUDE_POLICY";
const ENGINE_POLICY_VERSION: u32 = 1;
/// Claude Code's config file runs to a few megabytes.
const CONFIG_FILE_LIMIT: u64 = 64 * 1024 * 1024;

/// What a launch gains from the policy.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LaunchAdditions {
    args: Vec<String>,
    pub(crate) env: Vec<(String, Option<String>)>,
}

impl LaunchAdditions {
    /// `args` with the policy's own arguments in front. They end in a flag
    /// that takes one value, so they cannot swallow what follows.
    pub(crate) fn prepend_args(
        &self,
        harness_id: &str,
        mode: HarnessLaunchMode,
        program: &str,
        args: Vec<String>,
    ) -> Vec<String> {
        if self.args.is_empty() {
            return args;
        }
        let mut all =
            crate::harness::sanitize_argv_for_program(harness_id, mode, program, self.args.clone());
        all.extend(args);
        all
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Claude Code as the harness: the policy travels in its arguments.
    Claude,
    /// The engine, which passes the policy on to its Claude Code backend.
    Engine,
}

/// The policy's part of one launch. A launch the policy does not cover gets
/// nothing, and nothing is written for it.
pub(crate) fn for_launch(
    harness_id: &str,
    mode: HarnessLaunchMode,
    options: HarnessLaunchOptions<'_>,
) -> Result<LaunchAdditions> {
    let Some(target) = target(harness_id, mode, options) else {
        return Ok(LaunchAdditions::default());
    };
    let (coven_home, claude_config) = launch_roots()?;
    additions(target, &coven_home, claude_config.as_deref(), options)
}

fn target(
    harness_id: &str,
    mode: HarnessLaunchMode,
    options: HarnessLaunchOptions<'_>,
) -> Option<Target> {
    if mode == HarnessLaunchMode::Interactive {
        return None;
    }
    let scoped_write = matches!(
        options.launch_policy,
        Some(LaunchPolicy::ClaudeScopedWrite(_))
    );
    if harness_id == crate::engine::ENGINE_HARNESS_ID {
        Some(Target::Engine)
    } else if harness_id == "claude" && !scoped_write {
        Some(Target::Claude)
    } else {
        None
    }
}

#[cfg(not(test))]
fn launch_roots() -> Result<(PathBuf, Option<PathBuf>)> {
    Ok((
        crate::paths::coven_home_dir()?,
        claude_config_file(std::env::var_os("CLAUDE_CONFIG_DIR"), dirs_next::home_dir()),
    ))
}

/// Unit tests build commands for every harness. None of them may write to the
/// developer's Coven home or depend on the developer's Claude config.
#[cfg(test)]
fn launch_roots() -> Result<(PathBuf, Option<PathBuf>)> {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| tempfile::tempdir().expect("temporary Coven home"));
    Ok((home.path().to_path_buf(), None))
}

fn additions(
    target: Target,
    coven_home: &Path,
    claude_config: Option<&Path>,
    options: HarnessLaunchOptions<'_>,
) -> Result<LaunchAdditions> {
    let settings = materialize(coven_home)?;
    let settings = settings
        .to_str()
        .context("the Coven home path is not UTF-8, so Claude Code cannot be given its policy")?;
    // The servers are a convenience: an unreadable config never fails a launch.
    let mcp_config = claude_config
        .filter(|file| defines_mcp_servers(file))
        .and_then(Path::to_str);
    Ok(match target {
        Target::Claude => {
            let mut args = Vec::new();
            if let Some(file) = mcp_config {
                args.extend(["--mcp-config".to_owned(), file.to_owned()]);
            }
            args.extend([
                "--setting-sources".to_owned(),
                SETTING_SOURCES.to_owned(),
                "--settings".to_owned(),
                settings.to_owned(),
            ]);
            LaunchAdditions {
                args,
                env: Vec::new(),
            }
        }
        Target::Engine => LaunchAdditions {
            args: Vec::new(),
            env: vec![(
                ENGINE_POLICY_ENV.to_owned(),
                Some(engine_policy(settings, mcp_config, options).to_string()),
            )],
        },
    })
}

fn engine_policy(
    settings: &str,
    mcp_config: Option<&str>,
    options: HarnessLaunchOptions<'_>,
) -> Value {
    let add_dirs: Vec<&str> = options
        .add_dirs
        .iter()
        .map(|dir| dir.trim())
        .filter(|dir| !dir.is_empty())
        .collect();
    json!({
        "version": ENGINE_POLICY_VERSION,
        "settingsFile": settings,
        "settingSources": SETTING_SOURCES,
        "permissionMode": options.permission.and_then(claude_permission_mode),
        "addDirs": add_dirs,
        "mcpConfigFile": mcp_config,
    })
}

/// Claude Code's name for a Coven permission, read from the bundled Claude
/// adapter so the engine and the harness cannot disagree.
fn claude_permission_mode(permission: Permission) -> Option<String> {
    crate::harness::built_in_harness_specs()
        .into_iter()
        .find(|spec| spec.id == "claude")?
        .sandbox_args(permission)
        .pop()
}

/// Write the compiled-in policy under `coven_home` unless that exact content
/// is already there, and return its path.
fn materialize(coven_home: &Path) -> Result<PathBuf> {
    let directory = coven_home.join(POLICY_DIR);
    let path = directory.join(POLICY_FILE);
    let current = fs::symlink_metadata(&path)
        .ok()
        .filter(fs::Metadata::is_file)
        .and_then(|_| fs::read(&path).ok());
    if current.as_deref() == Some(POLICY.as_bytes()) {
        return Ok(path);
    }
    let write = || -> std::io::Result<()> {
        fs::create_dir_all(&directory)?;
        // Renamed into place, so a reader sees the old policy or the new one.
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        file.write_all(POLICY.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&path).map_err(|error| error.error)?;
        Ok(())
    };
    write().with_context(|| {
        format!(
            "could not write Coven's Claude Code permission policy to {}",
            path.display()
        )
    })?;
    eprintln!(
        "coven: {} the Claude Code permission policy at {}",
        if current.is_some() {
            "updated"
        } else {
            "installed"
        },
        path.display()
    );
    Ok(path)
}

/// Where Claude Code keeps the operator's own MCP servers.
fn claude_config_file(config_dir: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let directory = config_dir
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .or(home)?;
    Some(directory.join(".claude.json"))
}

/// Whether `--mcp-config` accepts `file` and gains anything from it. Claude
/// Code tolerates a malformed server entry but refuses to start on a file with
/// no `mcpServers` object.
fn defines_mcp_servers(file: &Path) -> bool {
    let readable = fs::metadata(file)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= CONFIG_FILE_LIMIT);
    if !readable {
        return false;
    }
    fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|config| {
            config
                .get("mcpServers")
                .and_then(Value::as_object)
                .is_some_and(|servers| !servers.is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(permission: Option<Permission>) -> HarnessLaunchOptions<'static> {
        HarnessLaunchOptions {
            permission,
            ..HarnessLaunchOptions::default()
        }
    }

    fn rules(key: &str) -> Vec<String> {
        let policy: Value = serde_json::from_str(POLICY).unwrap();
        policy["permissions"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| rule.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn the_policy_allows_the_approved_commands_and_refuses_the_gated_ones() {
        assert_eq!(
            rules("allow"),
            [
                "Bash(trash:*)",
                "Bash(mkdir:*)",
                "Bash(touch:*)",
                "Bash(cp:*)",
                "Bash(mv:*)",
                "Bash(git add:*)",
                "Bash(git commit:*)"
            ]
        );
        assert_eq!(
            rules("deny"),
            ["Bash(rm:*)", "Bash(git push:*)", "Bash(git tag:*)"]
        );
        assert!(rules("ask").is_empty());
        assert!(rules("additionalDirectories").is_empty());
        // Only permissions: a mode, hook or environment here would apply to
        // every chat on every machine.
        let policy: Value = serde_json::from_str(POLICY).unwrap();
        assert_eq!(policy.as_object().unwrap().len(), 1);
        assert_eq!(policy["permissions"].as_object().unwrap().len(), 4);
    }

    #[test]
    fn the_policy_covers_headless_claude_and_engine_launches_only() {
        let none = options(None);
        let engine = crate::engine::ENGINE_HARNESS_ID;
        for mode in [HarnessLaunchMode::NonInteractive, HarnessLaunchMode::Stream] {
            assert_eq!(target("claude", mode, none), Some(Target::Claude));
            assert_eq!(target(engine, mode, none), Some(Target::Engine));
            assert_eq!(target("codex", mode, none), None);
            assert_eq!(target("copilot", mode, none), None);
        }
        for harness in ["claude", engine, "codex"] {
            assert_eq!(target(harness, HarnessLaunchMode::Interactive, none), None);
        }
    }

    #[test]
    fn a_write_scope_launch_keeps_its_own_policy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let scope = LaunchPolicy::ClaudeScopedWrite(crate::claude_write_scope::ClaudeWriteScope {
            version: 2,
            primary_root: root.to_str().unwrap().to_owned(),
            writable_roots: vec![root.to_str().unwrap().to_owned()],
            read_only_roots: Vec::new(),
            protected_roots: Vec::new(),
            mcp_config_file: None,
            mcp_server_names: Vec::new(),
        });
        let scoped = HarnessLaunchOptions {
            launch_policy: Some(&scope),
            ..HarnessLaunchOptions::default()
        };
        assert_eq!(target("claude", HarnessLaunchMode::Stream, scoped), None);
    }

    #[test]
    fn an_uncovered_launch_gains_nothing() {
        let additions = for_launch(
            "codex",
            HarnessLaunchMode::NonInteractive,
            options(Some(Permission::Full)),
        )
        .unwrap();
        assert_eq!(additions, LaunchAdditions::default());
        let args = vec!["exec".to_owned()];
        assert_eq!(
            additions.prepend_args(
                "codex",
                HarnessLaunchMode::NonInteractive,
                "codex",
                args.clone()
            ),
            args
        );
    }

    #[test]
    fn claude_gets_the_policy_file_and_only_the_project_source() {
        let home = tempfile::tempdir().unwrap();
        let additions = additions(Target::Claude, home.path(), None, options(None)).unwrap();
        let policy = home.path().join("policy").join("claude-settings.json");
        assert_eq!(
            additions.args,
            [
                "--setting-sources",
                "project",
                "--settings",
                policy.to_str().unwrap()
            ]
        );
        assert!(additions.env.is_empty());
        assert_eq!(fs::read_to_string(policy).unwrap(), POLICY);
        assert_eq!(
            additions.prepend_args(
                "claude",
                HarnessLaunchMode::Stream,
                "claude",
                vec!["--print".to_owned()]
            )[4..],
            ["--print"]
        );
    }

    #[test]
    fn the_operators_mcp_servers_are_named_only_when_the_config_defines_some() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".claude.json");
        for unusable in [
            r#"{"numStartups":3}"#,
            r#"{"mcpServers":{}}"#,
            r#"{"mcpServers":[]}"#,
            "not json",
        ] {
            fs::write(&config, unusable).unwrap();
            let additions =
                additions(Target::Claude, home.path(), Some(&config), options(None)).unwrap();
            assert_eq!(additions.args[0], "--setting-sources", "{unusable}");
        }
        let absent = home.path().join("absent.json");
        assert!(!defines_mcp_servers(&absent));

        fs::write(
            &config,
            r#"{"mcpServers":{"tracker":{"command":"tracker"}}}"#,
        )
        .unwrap();
        let additions =
            additions(Target::Claude, home.path(), Some(&config), options(None)).unwrap();
        // The flag takes several values, so one that takes a single value
        // follows it.
        assert_eq!(
            additions.args[..3],
            [
                "--mcp-config",
                config.to_str().unwrap(),
                "--setting-sources"
            ]
        );
    }

    #[test]
    fn the_policy_is_written_once_and_repaired_when_it_drifts() {
        let home = tempfile::tempdir().unwrap();
        let path = materialize(home.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), POLICY);

        // Unchanged content is left alone: the file is not replaced.
        let first = fs::metadata(&path).unwrap();
        assert_eq!(materialize(home.path()).unwrap(), path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&path).unwrap().ino(), first.ino());
        }
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            first.modified().unwrap()
        );

        fs::write(&path, r#"{"permissions":{"allow":["Bash"]}}"#).unwrap();
        materialize(home.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), POLICY);
        let entries: Vec<_> = fs::read_dir(home.path().join("policy"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["claude-settings.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_policys_place_is_replaced_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = home.path().join("elsewhere.json");
        fs::write(&elsewhere, POLICY).unwrap();
        fs::create_dir(home.path().join("policy")).unwrap();
        let path = home.path().join("policy").join("claude-settings.json");
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        materialize(home.path()).unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().is_file());
        assert_eq!(fs::read_to_string(&path).unwrap(), POLICY);
    }

    #[test]
    fn the_engine_gets_the_policy_with_coven_s_permission_and_directories() {
        let home = tempfile::tempdir().unwrap();
        let policy = home.path().join("policy").join("claude-settings.json");
        let dirs = [
            "/granted/one".to_owned(),
            "  ".to_owned(),
            " /granted/two ".to_owned(),
        ];
        for (permission, mode) in [
            (Some(Permission::Full), json!("bypassPermissions")),
            (Some(Permission::ReadOnly), json!("plan")),
            (None, Value::Null),
        ] {
            let launch = HarnessLaunchOptions {
                permission,
                add_dirs: &dirs,
                ..HarnessLaunchOptions::default()
            };
            let additions = additions(Target::Engine, home.path(), None, launch).unwrap();
            assert!(additions.args.is_empty());
            let [(name, Some(value))] = additions.env.as_slice() else {
                panic!("one variable: {:?}", additions.env);
            };
            assert_eq!(name, "COVEN_CLAUDE_POLICY");
            assert_eq!(
                serde_json::from_str::<Value>(value).unwrap(),
                json!({
                    "version": 1,
                    "settingsFile": policy.to_str().unwrap(),
                    "settingSources": "project",
                    "permissionMode": mode,
                    "addDirs": ["/granted/one", "/granted/two"],
                    "mcpConfigFile": null,
                })
            );
        }
    }

    #[test]
    fn the_operators_claude_config_follows_claude_config_dir() {
        let home = Some(PathBuf::from("/operator"));
        assert_eq!(
            claude_config_file(None, home.clone()),
            Some(PathBuf::from("/operator/.claude.json"))
        );
        assert_eq!(
            claude_config_file(Some(OsString::new()), home.clone()),
            Some(PathBuf::from("/operator/.claude.json"))
        );
        assert_eq!(
            claude_config_file(Some("/elsewhere/claude".into()), home),
            Some(PathBuf::from("/elsewhere/claude/.claude.json"))
        );
        assert_eq!(claude_config_file(None, None), None);
    }
}
