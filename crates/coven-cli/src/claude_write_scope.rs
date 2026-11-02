//! Explicit scoped edit authority, separate from unrestricted Full and #713.
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeWriteScope {
    pub version: u32,
    pub primary_root: String,
    pub writable_roots: Vec<String>,
    pub read_only_roots: Vec<String>,
    pub protected_roots: Vec<String>,
}

impl ClaudeWriteScope {
    pub fn from_value(value: Value, cwd: &Path) -> Result<Self> {
        let requested: Self = serde_json::from_value(value).context("invalid writeScope schema")?;
        requested.validate_for_cwd(cwd)
    }

    pub fn read(file: &Path, cwd: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        fs::File::open(file)
            .context("could not read writeScope file")?
            .take(65_537)
            .read_to_end(&mut bytes)
            .context("could not read writeScope file")?;
        ensure!(bytes.len() <= 65_536, "writeScope exceeds 65536 bytes");
        Self::from_value(serde_json::from_slice(&bytes)?, cwd)
    }

    pub fn validate_for_cwd(&self, cwd: &Path) -> Result<Self> {
        ensure!(self.version == 1, "unsupported writeScope version");
        let primary_root = canonical_root(&self.primary_root)?;
        ensure!(
            Path::new(&primary_root) == crate::project::canonical_project_root(cwd)?,
            "writeScope primaryRoot differs from launch cwd"
        );
        let canonical = |roots: &[String]| -> Result<Vec<String>> {
            roots
                .iter()
                .map(|root| canonical_root(root))
                .collect::<Result<BTreeSet<_>>>()
                .map(|roots| roots.into_iter().collect())
        };
        let writable_roots = canonical(&self.writable_roots)?;
        let read_only_roots = canonical(&self.read_only_roots)?;
        let protected_roots = canonical(&self.protected_roots)?;
        ensure!(
            writable_roots.contains(&primary_root),
            "writeScope must explicitly grant its primaryRoot write access"
        );
        for protected in &protected_roots {
            ensure!(
                !writable_roots.iter().any(|write| within(write, protected)),
                "writeScope cannot grant a protected runtime root or its descendants"
            );
        }
        for read in &read_only_roots {
            // A read-only parent outside all writable roots needs no deny:
            // acceptEdits grants only cwd and the explicit additional roots.
            // Inside a writable root, a native deny cannot be overridden by
            // a child allow. Refuse that ambiguous layout rather than widen it.
            if writable_roots.iter().any(|write| within(read, write)) {
                ensure!(
                    !writable_roots.iter().any(|write| within(write, read)),
                    "writeScope has unsupported overlapping read/write roots"
                );
            }
        }
        Ok(Self {
            version: 1,
            primary_root,
            writable_roots,
            read_only_roots,
            protected_roots,
        })
    }

    /// A protected or nested read-only object must not have a second name
    /// that a writable root could edit. Audit only those denied trees: their
    /// size is bounded by runtime resources, while writable project trees can
    /// hold millions of entries (pnpm stores hardlink every dependency file).
    /// Pre-existing aliases between a writable name and an ungranted object
    /// remain writable, as they were under unrestricted Full. The child tree
    /// also receives a kernel no-hardlink guard, so it cannot manufacture a
    /// new name after this admission.
    fn audit_denied_objects(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            const MAX_ENTRIES: usize = 200_000;
            let mut pending: Vec<PathBuf> = self
                .protected_roots
                .iter()
                .chain(
                    self.read_only_roots
                        .iter()
                        .filter(|read| self.writable_roots.iter().any(|write| within(read, write))),
                )
                .map(PathBuf::from)
                .collect();
            let mut visited = BTreeSet::new();
            while let Some(path) = pending.pop() {
                if !visited.insert(path.clone()) {
                    continue;
                }
                ensure!(
                    visited.len() <= MAX_ENTRIES,
                    "writeScope protected-resource audit exceeds 200000 entries"
                );
                let metadata =
                    fs::symlink_metadata(&path).context("writeScope filesystem audit failed")?;
                if metadata.is_dir() {
                    for entry in fs::read_dir(&path).context("writeScope directory audit failed")? {
                        pending.push(entry?.path());
                    }
                } else if metadata.is_file() {
                    ensure!(
                        metadata.nlink() == 1,
                        "writeScope protected or read-only files have hardlink aliases; remove the extra links or narrow the grants"
                    );
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        anyhow::bail!("writeScope filesystem-object qualification is unavailable on this platform")
    }

    pub fn native_args(&self) -> Result<Vec<String>> {
        let scope = self.validate_for_cwd(Path::new(&self.primary_root))?;
        scope.audit_denied_objects()?;
        let read_allowed: Vec<String> = scope
            .read_only_roots
            .iter()
            .chain(scope.protected_roots.iter())
            .flat_map(|root| {
                let root = root.replace('\\', "/");
                [format!("Read(/{root})"), format!("Read(/{root}/**)")]
            })
            .collect();
        let mut denied: BTreeSet<String> = scope.protected_roots.into_iter().collect();
        for root in scope.read_only_roots {
            if scope
                .writable_roots
                .iter()
                .any(|write| within(&root, write))
            {
                denied.insert(root);
            }
        }
        let deny: Vec<String> = denied
            .iter()
            .flat_map(|root| {
                let root = root.replace('\\', "/");
                [format!("Edit(/{root})"), format!("Edit(/{root}/**)")]
            })
            .collect();
        let mut args = vec![
            "--permission-mode".into(),
            "acceptEdits".into(),
            "--setting-sources".into(),
            String::new(),
            "--strict-mcp-config".into(),
            "--mcp-config".into(),
            "{\"mcpServers\":{}}".into(),
            "--settings".into(),
            json!({"permissions": {"allow": read_allowed, "deny": deny}}).to_string(),
        ];
        for root in scope.writable_roots {
            if root != scope.primary_root {
                args.extend(["--add-dir".into(), root]);
            }
        }
        Ok(args)
    }
}

/// This is a no-hardlink guard, not a claim of general shell containment.
/// Native permissions remain responsible for the admitted pathname boundary.
pub fn guard_native_command(program: String, args: Vec<String>) -> Result<(String, Vec<String>)> {
    ensure!(
        cfg!(target_os = "macos"),
        "Claude scoped writes require a qualified macOS hardlink guard"
    );
    let mut guarded = vec![
        "-p".into(),
        "(version 1) (allow default) (deny file-link)".into(),
        program,
    ];
    guarded.extend(args);
    Ok(("/usr/bin/sandbox-exec".into(), guarded))
}

fn within(candidate: &str, root: &str) -> bool {
    Path::new(candidate).starts_with(root)
}

fn canonical_root(root: &str) -> Result<String> {
    ensure!(
        !root.is_empty() && root.trim() == root && Path::new(root).is_absolute(),
        "writeScope roots must be nonempty absolute paths"
    );
    let canonical = crate::project::canonical_project_root(Path::new(root))
        .context("writeScope root is unavailable")?;
    ensure!(
        canonical.is_dir() && canonical.parent().is_some(),
        "writeScope root must be a directory narrower than the filesystem root"
    );
    let value = canonical.to_str().context("writeScope root is not UTF-8")?;
    ensure!(
        native_rule_path_is_supported(value, cfg!(windows)),
        "writeScope root contains unsupported native rule syntax"
    );
    Ok(value.to_owned())
}

fn native_rule_path_is_supported(value: &str, windows_separators: bool) -> bool {
    !value
        .chars()
        .any(|c| c.is_control() || "*?[]{}!()".contains(c) || (c == '\\' && !windows_separators))
}

/// The scoped policy must not inherit a second native authority source.
/// Refuse overrides rather than silently removing a possible administrator
/// restriction. Provider credentials and endpoint variables are unaffected.
pub fn validate_native_environment() -> Result<()> {
    validate_native_environment_names(std::env::vars_os().map(|(name, _)| name))
}

fn validate_native_environment_names(
    names: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<()> {
    for name in names {
        if let Some(name) = name.to_str() {
            ensure!(!(name.starts_with("CLAUDE_") &&
                (name.contains("PERMISSION") || name.contains("SETTINGS") || name.contains("ADD_DIR")
                    || name == "CLAUDE_CODE_SESSION_KIND")),
                "writeScope cannot use inherited Claude permission or settings overrides; restart Coven with a clean runtime environment");
        }
    }
    Ok(())
}

/// Qualification is deliberately exact; a new CLI must be qualified before
/// it can inherit this profile. Never infer enforcement from a flag's name.
pub fn verify_native_profile(program: &str) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Claude scoped writes are not qualified on this platform"
    );
    let request = crate::setup::LaunchRequest {
        executable: PathBuf::from(program),
        args: vec!["--version".into()],
        current_dir: None,
        env_overrides: Vec::new(),
    };
    let version = crate::setup::process::probe_version(&request, Duration::from_secs(2))?
        .context("could not establish the Claude scoped-write profile")?;
    ensure!(
        version.trim() == "2.1.316",
        "Claude scoped-write profile is unsupported; qualified version is 2.1.316"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Value) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for name in [
            "primary",
            "primary/skills",
            "read",
            "read/write",
            "ungranted",
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let value = json!({"version":1,"primaryRoot":root.join("primary"),
            "writableRoots":[root.join("primary"),root.join("read/write")],
            "readOnlyRoots":[root.join("read")],"protectedRoots":[root.join("primary/skills")]});
        (temp, value)
    }
    #[cfg(unix)]
    #[test]
    fn explicit_child_write_keeps_its_read_only_parent_ungranted() {
        let (_temp, value) = fixture();
        let cwd = PathBuf::from(value["primaryRoot"].as_str().unwrap());
        let scope = ClaudeWriteScope::from_value(value, &cwd).unwrap();
        let args = scope.native_args().unwrap();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "acceptEdits"]));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--add-dir" && pair[1].ends_with("/read/write")));
        assert!(!args
            .iter()
            .any(|arg| arg == "bypassPermissions" || arg == "--dangerously-skip-permissions"));
        let settings: Value = serde_json::from_str(
            &args[args.iter().position(|arg| arg == "--settings").unwrap() + 1],
        )
        .unwrap();
        let read_rules = settings["permissions"]["allow"].as_array().unwrap();
        assert!(read_rules
            .iter()
            .any(|rule| rule.as_str().unwrap().ends_with("/read/**)")));
        assert!(read_rules
            .iter()
            .any(|rule| rule.as_str().unwrap().ends_with("/primary/skills/**)")));
        assert!(!read_rules
            .iter()
            .any(|rule| rule.as_str().unwrap().starts_with("Edit(")));
        assert!(settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule.as_str().unwrap().ends_with("/primary/skills/**)")));
    }
    #[cfg(unix)]
    #[test]
    fn protected_hardlink_alias_cannot_be_admitted_as_writable() {
        let (_temp, value) = fixture();
        let cwd = PathBuf::from(value["primaryRoot"].as_str().unwrap());
        let protected = cwd.join("skills/source");
        fs::write(&protected, "protected sentinel").unwrap();
        fs::hard_link(&protected, cwd.join("allowed-alias")).unwrap();
        let scope = ClaudeWriteScope::from_value(value, &cwd).unwrap();
        assert!(
            scope.native_args().is_err(),
            "a protected object must not become writable through a hardlink name"
        );
        assert_eq!(fs::read_to_string(protected).unwrap(), "protected sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn hardlinks_entirely_inside_writable_roots_remain_writable() {
        let (_temp, value) = fixture();
        let cwd = PathBuf::from(value["primaryRoot"].as_str().unwrap());
        fs::write(cwd.join("first"), "writable").unwrap();
        fs::hard_link(cwd.join("first"), cwd.join("second")).unwrap();
        let scope = ClaudeWriteScope::from_value(value, &cwd).unwrap();
        scope.native_args().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_store_links_in_writable_trees_do_not_block_admission() {
        let (temp, value) = fixture();
        let cwd = PathBuf::from(value["primaryRoot"].as_str().unwrap());
        let store = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("ungranted/store-file");
        fs::write(&store, "dependency").unwrap();
        fs::create_dir_all(cwd.join("node_modules/dep")).unwrap();
        fs::hard_link(&store, cwd.join("node_modules/dep/index.js")).unwrap();
        let scope = ClaudeWriteScope::from_value(value, &cwd).unwrap();
        scope.native_args().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn nested_read_only_hardlink_alias_cannot_be_admitted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("primary/locked")).unwrap();
        fs::write(root.join("primary/locked/source"), "read-only sentinel").unwrap();
        fs::hard_link(
            root.join("primary/locked/source"),
            root.join("primary/alias"),
        )
        .unwrap();
        let value = json!({"version":1,"primaryRoot":root.join("primary"),
            "writableRoots":[root.join("primary")],
            "readOnlyRoots":[root.join("primary/locked")],"protectedRoots":[]});
        let cwd = root.join("primary");
        let scope = ClaudeWriteScope::from_value(value, &cwd).unwrap();
        assert!(scope.native_args().is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn kernel_guard_denies_new_links_without_disabling_ordinary_writes() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        fs::write(&source, "sentinel").unwrap();
        let (program, args) = guard_native_command(
            "/bin/ln".into(),
            vec![source.display().to_string(), alias.display().to_string()],
        )
        .unwrap();
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!alias.exists());
        assert_eq!(fs::read_to_string(source).unwrap(), "sentinel");
        let destination = root.path().join("ordinary");
        let (program, args) = guard_native_command(
            "/bin/sh".into(),
            vec![
                "-c".into(),
                "printf allowed > \"$1\"".into(),
                "probe".into(),
                destination.display().to_string(),
            ],
        )
        .unwrap();
        assert!(std::process::Command::new(program)
            .args(args)
            .status()
            .unwrap()
            .success());
        assert_eq!(fs::read_to_string(destination).unwrap(), "allowed");
    }

    #[cfg(windows)]
    #[test]
    fn scope_roots_use_the_existing_node_compatible_canonical_form() {
        let (_temp, value) = fixture();
        let cwd = PathBuf::from(value["primaryRoot"].as_str().unwrap());
        let scope = ClaudeWriteScope::from_value(value, &cwd).unwrap();
        assert_eq!(
            Path::new(&scope.primary_root),
            crate::project::canonical_project_root(&cwd).unwrap()
        );
        assert!(!scope.primary_root.starts_with(r"\\?\"));
    }

    #[test]
    fn native_rule_syntax_distinguishes_windows_separators_from_posix_characters() {
        assert!(native_rule_path_is_supported(r"C:\scoped\project", true));
        assert!(!native_rule_path_is_supported(
            r"/scoped/skills\private",
            false
        ));
        assert!(!native_rule_path_is_supported(r"C:\scoped\*", true));
        assert!(native_rule_path_is_supported("/scoped/project", false));
    }

    #[test]
    fn inherited_native_authority_is_refused_without_removing_provider_configuration() {
        for name in [
            "CLAUDE_RELAUNCH_SESSION_ADD_DIRS",
            "CLAUDE_BG_SESSION_PERMISSION_RULES",
            "CLAUDE_CODE_MANAGED_SETTINGS_PATH",
            "CLAUDE_CODE_REMOTE_SETTINGS_PATH",
            "CLAUDE_TEST_USER_SETTINGS",
            "CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS",
            "CLAUDE_CODE_SESSION_KIND",
        ] {
            assert!(
                validate_native_environment_names([name.into()]).is_err(),
                "{name}"
            );
        }
        assert!(validate_native_environment_names(
            [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_BASE_URL",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CONFIG_DIR"
            ]
            .map(Into::into)
        )
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn literal_backslash_roots_cannot_be_rewritten_into_other_paths() {
        let (_temp, value) = fixture();
        let cwd = PathBuf::from(value["primaryRoot"].as_str().unwrap());
        let unusual = cwd.join("skills\\private");
        fs::create_dir(&unusual).unwrap();
        for field in ["protectedRoots", "readOnlyRoots", "writableRoots"] {
            let mut requested = value.clone();
            requested[field]
                .as_array_mut()
                .unwrap()
                .push(json!(unusual));
            assert!(
                ClaudeWriteScope::from_value(requested, &cwd).is_err(),
                "{field}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_profile_uses_normalized_exact_version() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("claude");
        for (version, allowed) in [("2.1.316", true), ("2.1.315", false), ("2.1.317", false)] {
            fs::write(
                &executable,
                format!("#!/bin/sh\necho '{version} (Claude Code)'\n"),
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                verify_native_profile(executable.to_str().unwrap()).is_ok(),
                allowed,
                "{version}"
            );
        }
    }

    #[test]
    fn malformed_or_ambiguous_scopes_never_create_authority() {
        let (_temp, value) = fixture();
        let cwd = PathBuf::from(value["primaryRoot"].as_str().unwrap());
        for bad in [
            json!({}),
            {
                let mut v = value.clone();
                v["version"] = json!(2);
                v
            },
            {
                let mut v = value.clone();
                v["extra"] = json!(true);
                v
            },
            {
                let mut v = value.clone();
                v["writableRoots"] = json!([]);
                v
            },
            {
                let mut v = value.clone();
                v["protectedRoots"] = json!([cwd.clone()]);
                v
            },
            {
                let mut v = value.clone();
                v["readOnlyRoots"] = json!([cwd.clone()]);
                v
            },
        ] {
            assert!(ClaudeWriteScope::from_value(bad, &cwd).is_err());
        }
    }
}
