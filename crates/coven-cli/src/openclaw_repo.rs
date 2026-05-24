use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

pub const OPENCLAW_REPO_NAME: &str = "openclaw";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoHandle {
    pub repo_name: String,
    pub root: PathBuf,
    pub package_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    pub branch: String,
    pub head: String,
    pub dirty_files: Vec<String>,
    pub untracked_files: Vec<String>,
}

impl GitState {
    pub fn is_dirty(&self) -> bool {
        !self.dirty_files.is_empty() || !self.untracked_files.is_empty()
    }
}

pub fn detect_repo(
    name: &str,
    explicit_repo: Option<&Path>,
    mapped_repo: Option<&Path>,
    start_dir: &Path,
    stored_repo: Option<&Path>,
) -> Result<RepoHandle> {
    if let Some(repo) = explicit_repo {
        return validate_repo(name, repo);
    }

    if let Some(repo) = mapped_repo {
        return validate_repo(name, repo).with_context(|| {
            format!(
                "repos.toml entry for \"{name}\" points to {} which is not a valid checkout",
                repo.display()
            )
        });
    }

    if name == OPENCLAW_REPO_NAME {
        if let Some(repo) = detect_openclaw_repo_from_ancestor(start_dir)? {
            return Ok(repo);
        }
    }

    if let Some(repo) = stored_repo {
        return validate_repo(name, repo).with_context(|| {
            format!(
                "stored \"{name}\" repo path {} is no longer valid; pass --repo <path>",
                repo.display()
            )
        });
    }

    if name == OPENCLAW_REPO_NAME {
        anyhow::bail!(
            "could not find an OpenClaw source checkout from {}; pass --repo <path> or add it to ~/.coven/repos.toml",
            start_dir.display()
        );
    }
    anyhow::bail!(
        "no path registered for repo \"{name}\"; add it to ~/.coven/repos.toml or pass --repo <path>"
    );
}

fn detect_openclaw_repo_from_ancestor(start_dir: &Path) -> Result<Option<RepoHandle>> {
    let mut candidate = start_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve start directory {}", start_dir.display()))?;

    loop {
        if looks_like_openclaw_repo(&candidate)? {
            return validate_openclaw_repo(&candidate).map(Some);
        }
        if !candidate.pop() {
            return Ok(None);
        }
    }
}

fn validate_repo(name: &str, path: &Path) -> Result<RepoHandle> {
    if name == OPENCLAW_REPO_NAME {
        return validate_openclaw_repo(path);
    }
    validate_git_checkout(name, path)
}

fn validate_openclaw_repo(path: &Path) -> Result<RepoHandle> {
    let root = path
        .canonicalize()
        .with_context(|| format!("failed to resolve repo path {}", path.display()))?;
    if !looks_like_openclaw_repo(&root)? {
        anyhow::bail!(
            "{} does not look like an OpenClaw source checkout",
            root.display()
        );
    }
    let package_name = package_name(&root)?;
    Ok(RepoHandle {
        repo_name: OPENCLAW_REPO_NAME.to_string(),
        package_name,
        root,
    })
}

fn validate_git_checkout(name: &str, path: &Path) -> Result<RepoHandle> {
    let root = path
        .canonicalize()
        .with_context(|| format!("failed to resolve repo path {}", path.display()))?;
    if !root.is_dir() {
        anyhow::bail!("{} is not a directory", root.display());
    }
    if !root.join(".git").exists() {
        anyhow::bail!(
            "{} does not contain a .git directory or file",
            root.display()
        );
    }
    let package_name = if root.join("package.json").is_file() {
        package_name(&root).ok().flatten()
    } else {
        None
    };
    Ok(RepoHandle {
        repo_name: name.to_string(),
        package_name,
        root,
    })
}

fn looks_like_openclaw_repo(root: &Path) -> Result<bool> {
    if !root.join(".git").exists() || !root.join("package.json").is_file() {
        return Ok(false);
    }

    let package_name = package_name(root)?;
    let has_openclaw_name = package_name
        .as_deref()
        .map(|name| {
            name.eq_ignore_ascii_case("openclaw") || name.eq_ignore_ascii_case("@openclaw/openclaw")
        })
        .unwrap_or(false);
    let has_expected_dirs = root.join("src/gateway").is_dir() || root.join("src/agents").is_dir();
    Ok(has_openclaw_name && has_expected_dirs)
}

fn package_name(root: &Path) -> Result<Option<String>> {
    let package_path = root.join("package.json");
    let raw = std::fs::read_to_string(&package_path)
        .with_context(|| format!("failed to read {}", package_path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", package_path.display()))?;
    Ok(value
        .get("name")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

pub fn inspect_git_state(repo_root: &Path) -> Result<GitState> {
    let branch = run_git(repo_root, &["branch", "--show-current"])?;
    let head = run_git(repo_root, &["rev-parse", "--short", "HEAD"])?;
    let porcelain = run_git(repo_root, &["status", "--porcelain"])?;
    let mut dirty_files = Vec::new();
    let mut untracked_files = Vec::new();

    for line in porcelain.lines() {
        if line.len() < 4 {
            continue;
        }
        let path = line[3..].to_string();
        if line.starts_with("??") {
            untracked_files.push(path);
        } else {
            dirty_files.push(path);
        }
    }

    Ok(GitState {
        branch: branch.trim().to_string(),
        head: head.trim().to_string(),
        dirty_files,
        untracked_files,
    })
}

pub fn changed_files(repo_root: &Path) -> Result<Vec<String>> {
    let porcelain = run_git(repo_root, &["status", "--porcelain"])?;
    Ok(porcelain
        .lines()
        .filter(|line| line.len() >= 4)
        .map(|line| line[3..].to_string())
        .collect())
}

fn run_git(repo_root: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["-c", "core.fsmonitor="])
        .args(args)
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn write_openclaw_fixture(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join(".git"))?;
        fs::create_dir_all(root.join("src/gateway"))?;
        fs::write(
            root.join("package.json"),
            r#"{"name":"openclaw","scripts":{"check":"node scripts/check.mjs"}}"#,
        )?;
        Ok(())
    }

    fn write_generic_git_fixture(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join(".git"))?;
        Ok(())
    }

    fn run_git_for_test(repo_root: &Path, args: &[&str]) -> Result<()> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git test command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    fn init_test_repo(repo: &Path) -> Result<()> {
        run_git_for_test(repo, &["init"])?;
        run_git_for_test(repo, &["config", "user.email", "test@example.com"])?;
        run_git_for_test(repo, &["config", "user.name", "Test User"])?;
        run_git_for_test(repo, &["config", "commit.gpgsign", "false"])?;
        Ok(())
    }

    #[test]
    fn detects_explicit_openclaw_repo() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("openclaw");
        write_openclaw_fixture(&repo)?;

        let detected = detect_repo(OPENCLAW_REPO_NAME, Some(&repo), None, temp.path(), None)?;

        assert_eq!(detected.root, repo.canonicalize()?);
        assert_eq!(detected.package_name.as_deref(), Some("openclaw"));
        assert_eq!(detected.repo_name, OPENCLAW_REPO_NAME);
        Ok(())
    }

    #[test]
    fn rejects_explicit_non_openclaw_repo() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("not-openclaw");
        fs::create_dir_all(repo.join(".git"))?;
        fs::write(repo.join("package.json"), r#"{"name":"other"}"#)?;

        let error =
            detect_repo(OPENCLAW_REPO_NAME, Some(&repo), None, temp.path(), None).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not look like an OpenClaw source checkout"),
            "unexpected error: {error:?}"
        );
        Ok(())
    }

    #[test]
    fn detects_openclaw_repo_from_child_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("openclaw");
        let child = repo.join("src/agents");
        write_openclaw_fixture(&repo)?;
        fs::create_dir_all(&child)?;

        let detected = detect_repo(OPENCLAW_REPO_NAME, None, None, &child, None)?;

        assert_eq!(detected.root, repo.canonicalize()?);
        Ok(())
    }

    #[test]
    fn detects_stored_openclaw_repo_when_current_directory_is_elsewhere() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("openclaw");
        let unrelated = temp.path().join("notes");
        write_openclaw_fixture(&repo)?;
        fs::create_dir(&unrelated)?;

        let detected = detect_repo(OPENCLAW_REPO_NAME, None, None, &unrelated, Some(&repo))?;

        assert_eq!(detected.root, repo.canonicalize()?);
        Ok(())
    }

    #[test]
    fn explicit_openclaw_repo_beats_other_sources() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let explicit = temp.path().join("explicit");
        let stored = temp.path().join("stored");
        let mapped = temp.path().join("mapped");
        write_openclaw_fixture(&explicit)?;
        write_openclaw_fixture(&stored)?;
        write_openclaw_fixture(&mapped)?;

        let detected = detect_repo(
            OPENCLAW_REPO_NAME,
            Some(&explicit),
            Some(&mapped),
            temp.path(),
            Some(&stored),
        )?;

        assert_eq!(detected.root, explicit.canonicalize()?);
        Ok(())
    }

    #[test]
    fn mapped_openclaw_repo_beats_ancestor_and_stored() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ancestor = temp.path().join("ancestor");
        let child = ancestor.join("src/gateway");
        let stored = temp.path().join("stored");
        let mapped = temp.path().join("mapped");
        write_openclaw_fixture(&ancestor)?;
        write_openclaw_fixture(&stored)?;
        write_openclaw_fixture(&mapped)?;
        fs::create_dir_all(&child)?;

        let detected = detect_repo(
            OPENCLAW_REPO_NAME,
            None,
            Some(&mapped),
            &child,
            Some(&stored),
        )?;

        assert_eq!(detected.root, mapped.canonicalize()?);
        Ok(())
    }

    #[test]
    fn ancestor_openclaw_repo_beats_stored_when_no_mapping() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ancestor = temp.path().join("ancestor");
        let child = ancestor.join("src/gateway");
        let stored = temp.path().join("stored");
        write_openclaw_fixture(&ancestor)?;
        write_openclaw_fixture(&stored)?;
        fs::create_dir_all(&child)?;

        let detected = detect_repo(OPENCLAW_REPO_NAME, None, None, &child, Some(&stored))?;

        assert_eq!(detected.root, ancestor.canonicalize()?);
        Ok(())
    }

    #[test]
    fn generic_repo_resolves_from_mapping_with_only_git_marker() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mapped = temp.path().join("other-repo");
        write_generic_git_fixture(&mapped)?;

        let detected = detect_repo("other-repo", None, Some(&mapped), temp.path(), None)?;

        assert_eq!(detected.repo_name, "other-repo");
        assert_eq!(detected.root, mapped.canonicalize()?);
        assert!(detected.package_name.is_none());
        Ok(())
    }

    #[test]
    fn generic_repo_rejects_path_missing_git_marker() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mapped = temp.path().join("plain-dir");
        fs::create_dir_all(&mapped)?;

        let error = detect_repo("plain", None, Some(&mapped), temp.path(), None).unwrap_err();
        let rendered = format!("{error:#}");

        assert!(
            rendered.contains("does not contain a .git"),
            "unexpected error: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn generic_repo_does_not_ancestor_walk() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ancestor = temp.path().join("openclaw");
        let child = ancestor.join("src/gateway");
        write_openclaw_fixture(&ancestor)?;
        fs::create_dir_all(&child)?;

        let error = detect_repo("other", None, None, &child, None).unwrap_err();

        assert!(
            error.to_string().contains("no path registered for repo"),
            "unexpected error: {error:?}"
        );
        Ok(())
    }

    #[test]
    fn unknown_generic_repo_errors_with_helpful_message() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let error = detect_repo("nope", None, None, temp.path(), None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("\"nope\""), "unexpected error: {message}");
        assert!(
            message.contains("repos.toml"),
            "unexpected error: {message}"
        );
        Ok(())
    }

    #[test]
    fn git_state_reports_dirty_and_untracked_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("openclaw");
        write_openclaw_fixture(&repo)?;
        init_test_repo(&repo)?;
        run_git_for_test(&repo, &["add", "."])?;
        run_git_for_test(&repo, &["commit", "-m", "initial"])?;
        fs::write(
            repo.join("package.json"),
            r#"{"name":"openclaw","scripts":{"check":"changed"}}"#,
        )?;
        fs::write(repo.join("new-file.txt"), "new")?;

        let state = inspect_git_state(&repo)?;

        assert!(!state.head.is_empty());
        assert!(state.dirty_files.contains(&"package.json".to_string()));
        assert!(state.untracked_files.contains(&"new-file.txt".to_string()));
        assert!(state.is_dirty());
        Ok(())
    }

    #[test]
    fn changed_files_lists_modified_and_untracked_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("openclaw");
        write_openclaw_fixture(&repo)?;
        init_test_repo(&repo)?;
        run_git_for_test(&repo, &["add", "."])?;
        run_git_for_test(&repo, &["commit", "-m", "initial"])?;
        fs::write(
            repo.join("package.json"),
            r#"{"name":"openclaw","scripts":{"check":"changed"}}"#,
        )?;
        fs::write(repo.join("untracked.txt"), "new")?;

        let files = changed_files(&repo)?;

        assert!(files.contains(&"package.json".to_string()));
        assert!(files.contains(&"untracked.txt".to_string()));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn git_state_ignores_repo_local_fsmonitor_config() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("openclaw");
        write_openclaw_fixture(&repo)?;
        init_test_repo(&repo)?;
        run_git_for_test(&repo, &["add", "."])?;
        run_git_for_test(&repo, &["commit", "-m", "initial"])?;
        let marker = temp.path().join("fsmonitor-ran");
        let fsmonitor = temp.path().join("fsmonitor-hook.sh");
        fs::write(
            &fsmonitor,
            format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
        )?;
        fs::set_permissions(&fsmonitor, fs::Permissions::from_mode(0o755))?;
        let fsmonitor_path = fsmonitor.to_string_lossy().into_owned();
        run_git_for_test(&repo, &["config", "core.fsmonitor", &fsmonitor_path])?;

        let state = inspect_git_state(&repo)?;
        let files = changed_files(&repo)?;

        assert!(!state.head.is_empty());
        assert!(files.is_empty());
        assert!(
            !marker.exists(),
            "repo-local core.fsmonitor hook was executed"
        );
        Ok(())
    }
}
