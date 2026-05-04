use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawRepo {
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

pub fn detect_openclaw_repo(
    explicit_repo: Option<&Path>,
    start_dir: &Path,
) -> Result<OpenClawRepo> {
    if let Some(repo) = explicit_repo {
        return validate_openclaw_repo(repo);
    }

    let mut candidate = start_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve start directory {}", start_dir.display()))?;

    loop {
        if looks_like_openclaw_repo(&candidate)? {
            return validate_openclaw_repo(&candidate);
        }
        if !candidate.pop() {
            anyhow::bail!(
                "could not find an OpenClaw source checkout from {}; pass --repo <path>",
                start_dir.display()
            );
        }
    }
}

fn validate_openclaw_repo(path: &Path) -> Result<OpenClawRepo> {
    let root = path
        .canonicalize()
        .with_context(|| format!("failed to resolve repo path {}", path.display()))?;
    if !looks_like_openclaw_repo(&root)? {
        anyhow::bail!(
            "{} does not look like an OpenClaw source checkout",
            root.display()
        );
    }
    Ok(OpenClawRepo {
        package_name: package_name(&root)?,
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

    fn write_openclaw_fixture(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join(".git"))?;
        fs::create_dir_all(root.join("src/gateway"))?;
        fs::write(
            root.join("package.json"),
            r#"{"name":"openclaw","scripts":{"check":"node scripts/check.mjs"}}"#,
        )?;
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

        let detected = detect_openclaw_repo(Some(&repo), temp.path())?;

        assert_eq!(detected.root, repo.canonicalize()?);
        assert_eq!(detected.package_name.as_deref(), Some("openclaw"));
        Ok(())
    }

    #[test]
    fn rejects_explicit_non_openclaw_repo() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("not-openclaw");
        fs::create_dir_all(repo.join(".git"))?;
        fs::write(repo.join("package.json"), r#"{"name":"other"}"#)?;

        let error = detect_openclaw_repo(Some(&repo), temp.path()).unwrap_err();

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
        // Also create the child dir so ancestry search works
        fs::create_dir_all(&child)?;

        let detected = detect_openclaw_repo(None, &child)?;

        assert_eq!(detected.root, repo.canonicalize()?);
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
}
