# Coven Patch OpenClaw Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `coven patch openclaw` as a beginner-friendly and power-user-friendly rescue loop that launches a supervised Codex or Claude Code patch session against a local OpenClaw source checkout and reports verification-ready results without committing or pushing.

**Architecture:** Add a focused patch workflow layer inside `crates/coven-cli` that reuses Coven's existing project boundary, harness detection, PTY launching, and SQLite session store. Keep repo detection, git state inspection, repair brief construction, verification selection, and result summarization in small Rust modules so each piece is testable without spawning real harnesses. The CLI command orchestrates these modules and stores patch metadata as session events rather than widening the core session schema first.

**Tech Stack:** Rust 2021, `clap`, `anyhow`, `serde_json`, `rusqlite`, `portable-pty`, standard-library `std::process::Command`, existing Coven CLI modules.

---

## Spec source

Design spec: `docs/superpowers/specs/2026-05-04-coven-patch-openclaw-design.md`

This plan implements Phase 1 from the spec: CLI rescue loop for local OpenClaw source repos.

## Scope check

This plan intentionally implements only **local OpenClaw source repo patching**. It does not implement installed OpenClaw Gateway/config repair, recipe distribution, comux review surfaces, custom harnesses, PR creation, or OpenClaw plugin integration. Those stay deferred because each is an independent subsystem.

## File structure

### Create

- `crates/coven-cli/src/patch.rs`
  - Owns `coven patch openclaw` request/plan/result types.
  - Builds guided or fast repair plans.
  - Builds the harness prompt.
  - Produces dry-run and final report text.

- `crates/coven-cli/src/openclaw_repo.rs`
  - Detects and validates local OpenClaw source checkouts.
  - Reads safe repo facts from `package.json` and `.git`.
  - Inspects git state using `git` argv calls.

- `crates/coven-cli/src/verification.rs`
  - Resolves verification profile names.
  - Runs `git diff --check` and selected OpenClaw verification commands.
  - Returns structured command/status output for final reporting.

### Modify

- `crates/coven-cli/src/main.rs`
  - Add `mod openclaw_repo; mod patch; mod verification;`.
  - Add `Command::Patch { command: PatchCommand }`.
  - Add `PatchCommand::OpenClaw` with flags and optional issue text.
  - Route the command to `run_patch_openclaw(...)`.
  - Add small prompt helpers for interactive confirmation and input.

- `crates/coven-cli/src/store.rs`
  - Add generic event insertion helpers if needed for patch metadata events.
  - Avoid a schema migration in v0 unless a later task proves events are insufficient.

- `crates/coven-cli/src/pty_runner.rs`
  - No functional change expected. Use existing `build_harness_command` and `run_attached`.
  - Add tests only if patch flow reveals a required seam.

- `crates/coven-cli/src/harness.rs`
  - No functional change expected. Reuse `built_in_harnesses()` and `command_parts_for_harness()`.

- `docs/README.md`
  - Add a short public-facing entry for `coven patch openclaw`.

- `docs/MVP-PLAN.md`
  - Add the rescue loop to active MVP scope and acceptance criteria.

## Commit sequence

Use one signed commit per task unless a task explicitly says it is a verification-only task.

Recommended commit messages:

1. `feat: add openclaw repo detection for patch workflow`
2. `feat: build openclaw patch repair briefs`
3. `feat: add openclaw patch verification profiles`
4. `feat: wire openclaw patch cli flow`
5. `docs: document openclaw patch rescue loop`

---

## Task 1: OpenClaw repo detection and git state inspection

**Files:**
- Create: `crates/coven-cli/src/openclaw_repo.rs`
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Create failing tests for explicit OpenClaw repo detection**

Create `crates/coven-cli/src/openclaw_repo.rs` with this test scaffold first:

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawRepo {
    pub root: PathBuf,
    pub package_name: Option<String>,
}

pub fn detect_openclaw_repo(explicit_repo: Option<&Path>, start_dir: &Path) -> Result<OpenClawRepo> {
    let _ = (explicit_repo, start_dir);
    anyhow::bail!("OpenClaw repo detection intentionally fails before the implementation step")
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
            error.to_string().contains("does not look like an OpenClaw source checkout"),
            "unexpected error: {error:?}"
        );
        Ok(())
    }
}
```

- [ ] **Step 2: Run the new tests and verify they fail**

Run:

```bash
cargo test -p coven-cli openclaw_repo::tests::detects_explicit_openclaw_repo openclaw_repo::tests::rejects_explicit_non_openclaw_repo
```

Expected: compile or test failure because `openclaw_repo` is not yet imported from `main.rs`, or because detection returns the initial failing error.

- [ ] **Step 3: Import the module from `main.rs`**

Add this beside the existing module declarations in `crates/coven-cli/src/main.rs`:

```rust
mod openclaw_repo;
```

- [ ] **Step 4: Implement explicit repo detection**

Replace the initial failing `detect_openclaw_repo` implementation in `crates/coven-cli/src/openclaw_repo.rs` with:

```rust
pub fn detect_openclaw_repo(explicit_repo: Option<&Path>, start_dir: &Path) -> Result<OpenClawRepo> {
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
        .map(|name| name.eq_ignore_ascii_case("openclaw") || name.eq_ignore_ascii_case("@openclaw/openclaw"))
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
```

- [ ] **Step 5: Add ancestry detection test**

Add this test to the `tests` module in `crates/coven-cli/src/openclaw_repo.rs`:

```rust
#[test]
fn detects_openclaw_repo_from_child_directory() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("openclaw");
    let child = repo.join("src/agents");
    write_openclaw_fixture(&repo)?;

    let detected = detect_openclaw_repo(None, &child)?;

    assert_eq!(detected.root, repo.canonicalize()?);
    Ok(())
}
```

- [ ] **Step 6: Add git state types and tests**

Append this code above the `tests` module in `crates/coven-cli/src/openclaw_repo.rs`:

```rust
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
```

Add this test inside the `tests` module:

```rust
#[test]
fn git_state_reports_dirty_and_untracked_files() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("openclaw");
    write_openclaw_fixture(&repo)?;
    run_git_for_test(&repo, &["init"])?;
    run_git_for_test(&repo, &["config", "user.email", "test@example.com"])?;
    run_git_for_test(&repo, &["config", "user.name", "Test User"])?;
    run_git_for_test(&repo, &["add", "."])?;
    run_git_for_test(&repo, &["commit", "-m", "initial"])?;
    fs::write(repo.join("package.json"), r#"{"name":"openclaw","scripts":{"check":"changed"}}"#)?;
    fs::write(repo.join("new-file.txt"), "new")?;

    let state = inspect_git_state(&repo)?;

    assert!(!state.head.is_empty());
    assert!(state.dirty_files.contains(&"package.json".to_string()));
    assert!(state.untracked_files.contains(&"new-file.txt".to_string()));
    assert!(state.is_dirty());
    Ok(())
}

fn run_git_for_test(repo_root: &Path, args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git test command failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}
```

- [ ] **Step 7: Run task tests**

Run:

```bash
cargo test -p coven-cli openclaw_repo::tests
```

Expected: all `openclaw_repo` tests pass.

- [ ] **Step 8: Format and commit Task 1**

Run:

```bash
cargo fmt --check
cargo test -p coven-cli openclaw_repo::tests
```

Expected: both pass.

Commit:

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/openclaw_repo.rs
git commit -S -m "feat: add openclaw repo detection for patch workflow"
```

---

## Task 2: Repair brief builder and patch workflow types

**Files:**
- Create: `crates/coven-cli/src/patch.rs`
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Create failing tests for repair brief construction**

Create `crates/coven-cli/src/patch.rs`:

```rust
use std::path::PathBuf;

use crate::openclaw_repo::{GitState, OpenClawRepo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationProfile {
    Auto,
    PnpmCheck,
    TargetedTest,
    DiffOnly,
}

impl VerificationProfile {
    pub fn parse(value: Option<&str>) -> anyhow::Result<Self> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "pnpm-check" => Ok(Self::PnpmCheck),
            "targeted-test" => Ok(Self::TargetedTest),
            "diff-only" => Ok(Self::DiffOnly),
            other => anyhow::bail!("unknown verification profile `{other}`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOpenClawRequest {
    pub repo: OpenClawRepo,
    pub git_state: GitState,
    pub issue: String,
    pub harness_id: String,
    pub verification_profile: VerificationProfile,
    pub non_interactive: bool,
    pub dry_run: bool,
    pub keep_session: bool,
}

pub fn build_repair_brief(request: &PatchOpenClawRequest) -> String {
    let _ = request;
    String::new()
}

pub fn summarize_patch_plan(request: &PatchOpenClawRequest) -> String {
    let _ = request;
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(issue: &str) -> PatchOpenClawRequest {
        PatchOpenClawRequest {
            repo: OpenClawRepo {
                root: PathBuf::from("/repo/openclaw"),
                package_name: Some("openclaw".to_string()),
            },
            git_state: GitState {
                branch: "fix/auth".to_string(),
                head: "abc1234".to_string(),
                dirty_files: vec!["CHANGELOG.md".to_string()],
                untracked_files: vec![],
            },
            issue: issue.to_string(),
            harness_id: "codex".to_string(),
            verification_profile: VerificationProfile::Auto,
            non_interactive: false,
            dry_run: false,
            keep_session: false,
        }
    }

    #[test]
    fn repair_brief_requires_root_cause_tests_and_no_commits() {
        let brief = build_repair_brief(&request("fix invalidated Codex auth profile order"));

        assert!(brief.contains("fix invalidated Codex auth profile order"));
        assert!(brief.contains("Investigate root cause before changing code"));
        assert!(brief.contains("Do not commit"));
        assert!(brief.contains("Do not push"));
        assert!(brief.contains("Respect existing uncommitted changes"));
        assert!(brief.contains("CHANGELOG.md"));
        assert!(brief.contains("git diff --check"));
    }

    #[test]
    fn patch_plan_summary_names_repo_harness_and_verification() {
        let summary = summarize_patch_plan(&request("fix auth"));

        assert!(summary.contains("/repo/openclaw"));
        assert!(summary.contains("codex"));
        assert!(summary.contains("auto"));
        assert!(summary.contains("fix auth"));
    }

    #[test]
    fn parses_verification_profiles() -> anyhow::Result<()> {
        assert_eq!(VerificationProfile::parse(None)?, VerificationProfile::Auto);
        assert_eq!(VerificationProfile::parse(Some("pnpm-check"))?, VerificationProfile::PnpmCheck);
        assert_eq!(VerificationProfile::parse(Some("targeted-test"))?, VerificationProfile::TargetedTest);
        assert_eq!(VerificationProfile::parse(Some("diff-only"))?, VerificationProfile::DiffOnly);
        assert!(VerificationProfile::parse(Some("everything")).is_err());
        Ok(())
    }
}
```

- [ ] **Step 2: Import the module and run failing tests**

Add this beside the modules in `crates/coven-cli/src/main.rs`:

```rust
mod patch;
```

Run:

```bash
cargo test -p coven-cli patch::tests
```

Expected: tests fail because the brief and summary return empty strings.

- [ ] **Step 3: Implement display helpers and repair brief text**

Replace the empty `build_repair_brief` and `summarize_patch_plan` functions in `crates/coven-cli/src/patch.rs` with:

```rust
impl VerificationProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::PnpmCheck => "pnpm-check",
            Self::TargetedTest => "targeted-test",
            Self::DiffOnly => "diff-only",
        }
    }
}

pub fn build_repair_brief(request: &PatchOpenClawRequest) -> String {
    let dirty = if request.git_state.dirty_files.is_empty() {
        "none".to_string()
    } else {
        request.git_state.dirty_files.join(", ")
    };
    let untracked = if request.git_state.untracked_files.is_empty() {
        "none".to_string()
    } else {
        request.git_state.untracked_files.join(", ")
    };

    format!(
        "You are repairing a local OpenClaw source checkout through Coven.\n\n\
Repository: {repo}\n\
Branch: {branch}\n\
HEAD: {head}\n\
Existing modified files: {dirty}\n\
Existing untracked files: {untracked}\n\n\
Issue to repair:\n{issue}\n\n\
Instructions:\n\
- Investigate root cause before changing code.\n\
- Make the smallest targeted patch that fixes the root cause.\n\
- Add or update tests when meaningful.\n\
- Run `git diff --check` before reporting success.\n\
- Run targeted tests for touched behavior when possible.\n\
- Do not commit.\n\
- Do not push.\n\
- Do not run destructive git commands.\n\
- Respect existing uncommitted changes and do not clobber them.\n\
- Finish with a concise summary, changed files, and verification output.\n",
        repo = request.repo.root.display(),
        branch = request.git_state.branch,
        head = request.git_state.head,
        dirty = dirty,
        untracked = untracked,
        issue = request.issue.trim()
    )
}

pub fn summarize_patch_plan(request: &PatchOpenClawRequest) -> String {
    format!(
        "Coven will patch OpenClaw at {} using harness `{}` with verification `{}`.\nIssue: {}\nNothing will be committed or pushed.",
        request.repo.root.display(),
        request.harness_id,
        request.verification_profile.as_str(),
        request.issue.trim()
    )
}
```

- [ ] **Step 4: Run repair brief tests**

Run:

```bash
cargo test -p coven-cli patch::tests
```

Expected: all patch module tests pass.

- [ ] **Step 5: Format and commit Task 2**

Run:

```bash
cargo fmt --check
cargo test -p coven-cli patch::tests
```

Expected: both pass.

Commit:

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/patch.rs
git commit -S -m "feat: build openclaw patch repair briefs"
```

---

## Task 3: Verification profiles and command runner

**Files:**
- Create: `crates/coven-cli/src/verification.rs`
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Create failing tests for verification command selection**

Create `crates/coven-cli/src/verification.rs`:

```rust
use std::path::Path;

use anyhow::{Context, Result};

use crate::patch::VerificationProfile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub command: String,
    pub status: VerificationStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationStatus {
    Passed,
    Failed(i32),
}

pub fn commands_for_profile(profile: &VerificationProfile) -> Vec<VerificationCommand> {
    let _ = profile;
    Vec::new()
}

pub fn run_verification(repo_root: &Path, profile: &VerificationProfile) -> Result<Vec<VerificationResult>> {
    let _ = (repo_root, profile);
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_runs_diff_check_only_as_safe_default() {
        let commands = commands_for_profile(&VerificationProfile::Auto);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].program, "git");
        assert_eq!(commands[0].args, vec!["diff", "--check"]);
    }

    #[test]
    fn pnpm_check_runs_diff_check_then_pnpm_check() {
        let commands = commands_for_profile(&VerificationProfile::PnpmCheck);

        assert_eq!(commands[0].args, vec!["diff", "--check"]);
        assert_eq!(commands[1].program, "pnpm");
        assert_eq!(commands[1].args, vec!["check"]);
    }

    #[test]
    fn diff_only_runs_only_diff_check() {
        let commands = commands_for_profile(&VerificationProfile::DiffOnly);

        assert_eq!(commands, vec![VerificationCommand {
            program: "git".to_string(),
            args: vec!["diff".to_string(), "--check".to_string()],
        }]);
    }
}
```

- [ ] **Step 2: Import module and run failing tests**

Add this beside module declarations in `crates/coven-cli/src/main.rs`:

```rust
mod verification;
```

Run:

```bash
cargo test -p coven-cli verification::tests
```

Expected: command selection tests fail because no commands are returned.

- [ ] **Step 3: Implement verification command selection and runner**

Replace the initial empty functions in `crates/coven-cli/src/verification.rs` with:

```rust
pub fn commands_for_profile(profile: &VerificationProfile) -> Vec<VerificationCommand> {
    let diff_check = VerificationCommand {
        program: "git".to_string(),
        args: vec!["diff".to_string(), "--check".to_string()],
    };

    match profile {
        VerificationProfile::Auto | VerificationProfile::DiffOnly | VerificationProfile::TargetedTest => {
            vec![diff_check]
        }
        VerificationProfile::PnpmCheck => vec![
            diff_check,
            VerificationCommand {
                program: "pnpm".to_string(),
                args: vec!["check".to_string()],
            },
        ],
    }
}

pub fn run_verification(repo_root: &Path, profile: &VerificationProfile) -> Result<Vec<VerificationResult>> {
    commands_for_profile(profile)
        .into_iter()
        .map(|command| run_command(repo_root, command))
        .collect()
}

fn run_command(repo_root: &Path, command: VerificationCommand) -> Result<VerificationResult> {
    let output = std::process::Command::new(&command.program)
        .args(&command.args)
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("failed to run verification command `{}`", format_command(&command)))?;
    let code = output.status.code().unwrap_or(1);
    let status = if output.status.success() {
        VerificationStatus::Passed
    } else {
        VerificationStatus::Failed(code)
    };

    Ok(VerificationResult {
        command: format_command(&command),
        status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn format_command(command: &VerificationCommand) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}
```

- [ ] **Step 4: Add runner tests with a real git fixture**

Add this test to the `tests` module in `crates/coven-cli/src/verification.rs`:

```rust
#[test]
fn run_verification_reports_git_diff_check_failure() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path();
    run_git_for_test(repo, &["init"])?;
    std::fs::write(repo.join("bad.txt"), "trailing whitespace   \n")?;

    let results = run_verification(repo, &VerificationProfile::DiffOnly)?;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].command, "git diff --check");
    assert!(matches!(results[0].status, VerificationStatus::Failed(_)));
    assert!(results[0].stdout.contains("trailing whitespace") || results[0].stderr.contains("trailing whitespace"));
    Ok(())
}

fn run_git_for_test(repo_root: &Path, args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git test command failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}
```

- [ ] **Step 5: Run verification tests**

Run:

```bash
cargo test -p coven-cli verification::tests
```

Expected: all verification tests pass.

- [ ] **Step 6: Format and commit Task 3**

Run:

```bash
cargo fmt --check
cargo test -p coven-cli verification::tests
```

Expected: both pass.

Commit:

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/verification.rs
git commit -S -m "feat: add openclaw patch verification profiles"
```

---

## Task 4: CLI parser, dry-run, and non-interactive planning

**Files:**
- Modify: `crates/coven-cli/src/main.rs`
- Modify: `crates/coven-cli/src/patch.rs`

- [ ] **Step 1: Add failing CLI parser tests**

Add these tests to the `tests` module in `crates/coven-cli/src/main.rs`:

```rust
#[test]
fn cli_accepts_patch_openclaw_guided_command() {
    let cli = Cli::parse_from(["coven", "patch", "openclaw"]);

    match cli.command {
        Command::Patch { command: PatchCommand::OpenClaw { issue, repo, harness, verify, non_interactive, dry_run, keep_session } } => {
            assert!(issue.is_empty());
            assert!(repo.is_none());
            assert!(harness.is_none());
            assert!(verify.is_none());
            assert!(!non_interactive);
            assert!(!dry_run);
            assert!(!keep_session);
        }
        other => panic!("expected patch openclaw command, got {other:?}"),
    }
}

#[test]
fn cli_accepts_patch_openclaw_fast_command() {
    let cli = Cli::parse_from([
        "coven",
        "patch",
        "openclaw",
        "fix auth order",
        "--repo",
        "/repo/openclaw",
        "--harness",
        "codex",
        "--verify",
        "pnpm-check",
        "--non-interactive",
        "--dry-run",
        "--keep-session",
    ]);

    match cli.command {
        Command::Patch { command: PatchCommand::OpenClaw { issue, repo, harness, verify, non_interactive, dry_run, keep_session } } => {
            assert_eq!(issue, vec!["fix auth order".to_string()]);
            assert_eq!(repo.as_deref(), Some(Path::new("/repo/openclaw")));
            assert_eq!(harness.as_deref(), Some("codex"));
            assert_eq!(verify.as_deref(), Some("pnpm-check"));
            assert!(non_interactive);
            assert!(dry_run);
            assert!(keep_session);
        }
        other => panic!("expected patch openclaw command, got {other:?}"),
    }
}
```

- [ ] **Step 2: Add CLI enum variants**

Modify the `Command` enum in `crates/coven-cli/src/main.rs` to include:

```rust
    Patch {
        #[command(subcommand)]
        command: PatchCommand,
    },
```

Add this enum below `Command`:

```rust
#[derive(Subcommand, Debug)]
enum PatchCommand {
    OpenClaw {
        #[arg(num_args = 0..)]
        issue: Vec<String>,
        #[arg(long)]
        repo: Option<PathBuf>,
        #[arg(long)]
        harness: Option<String>,
        #[arg(long)]
        verify: Option<String>,
        #[arg(long)]
        non_interactive: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        keep_session: bool,
    },
}
```

Update `main()` match with a temporary route:

```rust
        Command::Patch { command } => run_patch_command(command),
```

Add this function near `run_daemon_command`:

```rust
fn run_patch_command(command: PatchCommand) -> Result<()> {
    match command {
        PatchCommand::OpenClaw {
            issue,
            repo,
            harness,
            verify,
            non_interactive,
            dry_run,
            keep_session,
        } => run_patch_openclaw(issue, repo, harness, verify, non_interactive, dry_run, keep_session),
    }
}
```

- [ ] **Step 3: Add an initial guarded `run_patch_openclaw` that supports parser tests**

Add this below `run_patch_command` in `crates/coven-cli/src/main.rs`:

```rust
fn run_patch_openclaw(
    _issue: Vec<String>,
    _repo: Option<PathBuf>,
    _harness: Option<String>,
    _verify: Option<String>,
    _non_interactive: bool,
    _dry_run: bool,
    _keep_session: bool,
) -> Result<()> {
    anyhow::bail!("coven patch openclaw requires the planning implementation in the next step")
}
```

- [ ] **Step 4: Run parser tests**

Run:

```bash
cargo test -p coven-cli cli_accepts_patch_openclaw
```

Expected: both parser tests pass.

- [ ] **Step 5: Implement non-interactive request construction and dry-run output**

Add this helper to `crates/coven-cli/src/main.rs` above `run_patch_openclaw`:

```rust
fn joined_optional_issue(issue: Vec<String>) -> Result<Option<String>> {
    if issue.is_empty() {
        return Ok(None);
    }
    let joined = issue.join(" ").trim().to_string();
    if joined.is_empty() {
        anyhow::bail!("issue text must not be empty when provided");
    }
    Ok(Some(joined))
}
```

Replace the initial guarded `run_patch_openclaw` with:

```rust
fn run_patch_openclaw(
    issue: Vec<String>,
    repo: Option<PathBuf>,
    harness: Option<String>,
    verify: Option<String>,
    non_interactive: bool,
    dry_run: bool,
    keep_session: bool,
) -> Result<()> {
    let start_dir = std::env::current_dir().context("failed to read current directory")?;
    let detected_repo = openclaw_repo::detect_openclaw_repo(repo.as_deref(), &start_dir)?;
    let git_state = openclaw_repo::inspect_git_state(&detected_repo.root)?;
    let issue = match joined_optional_issue(issue)? {
        Some(issue) => issue,
        None if non_interactive => anyhow::bail!("issue text is required with --non-interactive"),
        None => prompt_for_required_line("What is broken in OpenClaw? ")?,
    };
    let harness_id = match harness {
        Some(harness) => harness,
        None if non_interactive => anyhow::bail!("--harness is required with --non-interactive"),
        None => choose_default_harness()?,
    };
    let verification_profile = patch::VerificationProfile::parse(verify.as_deref())?;

    let request = patch::PatchOpenClawRequest {
        repo: detected_repo,
        git_state,
        issue,
        harness_id,
        verification_profile,
        non_interactive,
        dry_run,
        keep_session,
    };

    println!("{}", patch::summarize_patch_plan(&request));
    if dry_run {
        println!("\nRepair brief:\n{}", patch::build_repair_brief(&request));
        return Ok(());
    }

    anyhow::bail!("launching patch sessions is disabled until the launch task; rerun with --dry-run")
}
```

Add simple interactive helpers below it:

```rust
fn prompt_for_required_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    io::stdin().read_line(&mut line).context("failed to read input")?;
    let line = line.trim().to_string();
    if line.is_empty() {
        anyhow::bail!("a response is required")
    }
    Ok(line)
}

fn choose_default_harness() -> Result<String> {
    let harnesses = harness::built_in_harnesses();
    if harnesses.iter().any(|harness| harness.id == "codex" && harness.available) {
        return Ok("codex".to_string());
    }
    if harnesses.iter().any(|harness| harness.id == "claude" && harness.available) {
        return Ok("claude".to_string());
    }
    anyhow::bail!("no supported harness is available; run `coven doctor` for setup guidance")
}
```

- [ ] **Step 6: Add unit tests for optional issue parsing**

Add these tests to the `tests` module in `crates/coven-cli/src/main.rs`:

```rust
#[test]
fn joined_optional_issue_returns_none_for_guided_mode() -> Result<()> {
    assert_eq!(joined_optional_issue(vec![])?, None);
    Ok(())
}

#[test]
fn joined_optional_issue_joins_fast_issue_text() -> Result<()> {
    assert_eq!(
        joined_optional_issue(vec!["fix".to_string(), "auth".to_string()])?,
        Some("fix auth".to_string())
    );
    Ok(())
}
```

- [ ] **Step 7: Run Task 4 tests**

Run:

```bash
cargo test -p coven-cli cli_accepts_patch_openclaw joined_optional_issue
```

Expected: all selected tests pass.

- [ ] **Step 8: Manually smoke dry-run from a real OpenClaw checkout if available**

Run from the Coven repo using the known local OpenClaw checkout:

```bash
cargo run -p coven-cli -- patch openclaw "fix auth order" --repo /Users/buns/Documents/GitHub/openclaw/openclaw --harness codex --dry-run
```

Expected: command prints a plan and repair brief, then exits without launching a harness.

- [ ] **Step 9: Format and commit Task 4**

Run:

```bash
cargo fmt --check
cargo test -p coven-cli cli_accepts_patch_openclaw joined_optional_issue patch::tests openclaw_repo::tests verification::tests
```

Expected: all pass.

Commit:

```bash
git add crates/coven-cli/src/main.rs
git commit -S -m "feat: wire openclaw patch dry run"
```

---

## Task 5: Launch supervised patch sessions and final report

**Files:**
- Modify: `crates/coven-cli/src/main.rs`
- Modify: `crates/coven-cli/src/patch.rs`
- Modify: `crates/coven-cli/src/store.rs`

- [ ] **Step 1: Add patch result report types and tests**

Append to `crates/coven-cli/src/patch.rs` above the tests module:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOpenClawReport {
    pub status: String,
    pub session_id: String,
    pub changed_files: Vec<String>,
    pub verification: Vec<String>,
}

pub fn summarize_patch_report(report: &PatchOpenClawReport) -> String {
    let changed_files = if report.changed_files.is_empty() {
        "none".to_string()
    } else {
        report.changed_files.join("\n- ")
    };
    let verification = if report.verification.is_empty() {
        "not run".to_string()
    } else {
        report.verification.join("\n- ")
    };

    format!(
        "Coven patch status: {status}\nSession: {session_id}\nChanged files:\n- {changed_files}\nVerification:\n- {verification}\nNothing was committed or pushed.",
        status = report.status,
        session_id = report.session_id,
        changed_files = changed_files,
        verification = verification
    )
}
```

Add this test to `patch::tests`:

```rust
#[test]
fn patch_report_reminds_user_nothing_was_committed_or_pushed() {
    let report = PatchOpenClawReport {
        status: "patched".to_string(),
        session_id: "session-1".to_string(),
        changed_files: vec!["src/file.rs".to_string()],
        verification: vec!["git diff --check passed".to_string()],
    };

    let summary = summarize_patch_report(&report);

    assert!(summary.contains("patched"));
    assert!(summary.contains("src/file.rs"));
    assert!(summary.contains("git diff --check passed"));
    assert!(summary.contains("Nothing was committed or pushed"));
}
```

- [ ] **Step 2: Add store event helper for patch metadata**

Add this helper to `crates/coven-cli/src/store.rs` below `insert_event`:

```rust
pub fn insert_json_event(
    conn: &Connection,
    session_id: &str,
    kind: &str,
    payload: &serde_json::Value,
    created_at: &str,
) -> Result<()> {
    let record = EventRecord {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        payload_json: payload.to_string(),
        created_at: created_at.to_string(),
    };
    insert_event(conn, &record)
}
```

Add this test to the `tests` module in `store.rs`:

```rust
#[test]
fn inserts_json_event() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let conn = open_store(&temp_dir.path().join("coven.db"))?;
    let session = session_record("session-1", "2026-04-27T06:00:00Z");
    insert_session(&conn, &session)?;

    insert_json_event(
        &conn,
        "session-1",
        "patch_metadata",
        &serde_json::json!({"target":"openclaw"}),
        "2026-04-27T06:01:00Z",
    )?;

    let events = list_events(&conn, "session-1")?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "patch_metadata");
    assert!(events[0].payload_json.contains("openclaw"));
    Ok(())
}
```

- [ ] **Step 3: Add changed-file collection helper**

Add this helper to `crates/coven-cli/src/openclaw_repo.rs` below `inspect_git_state`:

```rust
pub fn changed_files(repo_root: &Path) -> Result<Vec<String>> {
    let porcelain = run_git(repo_root, &["status", "--porcelain"])?;
    Ok(porcelain
        .lines()
        .filter(|line| line.len() >= 4)
        .map(|line| line[3..].to_string())
        .collect())
}
```

Add this test to `openclaw_repo::tests`:

```rust
#[test]
fn changed_files_lists_modified_and_untracked_files() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("openclaw");
    write_openclaw_fixture(&repo)?;
    run_git_for_test(&repo, &["init"])?;
    run_git_for_test(&repo, &["config", "user.email", "test@example.com"])?;
    run_git_for_test(&repo, &["config", "user.name", "Test User"])?;
    run_git_for_test(&repo, &["add", "."])?;
    run_git_for_test(&repo, &["commit", "-m", "initial"])?;
    fs::write(repo.join("package.json"), r#"{"name":"openclaw","scripts":{"check":"changed"}}"#)?;
    fs::write(repo.join("untracked.txt"), "new")?;

    let files = changed_files(&repo)?;

    assert!(files.contains(&"package.json".to_string()));
    assert!(files.contains(&"untracked.txt".to_string()));
    Ok(())
}
```

- [ ] **Step 4: Replace the guarded launch error with supervised session launch**

In `run_patch_openclaw` in `crates/coven-cli/src/main.rs`, replace the final guarded launch error with:

```rust
    if request.git_state.is_dirty() && !request.non_interactive {
        println!("\nExisting changes were detected. Coven will not stash or overwrite them.");
        if !confirm_yes("Continue and ask the harness to preserve existing changes? [y/N] ")? {
            anyhow::bail!("cancelled before harness launch")
        }
    }

    if !request.non_interactive && !confirm_yes("Launch the harness now? [y/N] ")? {
        anyhow::bail!("cancelled before harness launch")
    }

    let session_id = launch_patch_session(&request)?;
    let verification_results = verification::run_verification(&request.repo.root, &request.verification_profile)?;
    let verification = verification_results
        .into_iter()
        .map(|result| match result.status {
            verification::VerificationStatus::Passed => format!("{} passed", result.command),
            verification::VerificationStatus::Failed(code) => format!("{} failed with exit code {}", result.command, code),
        })
        .collect::<Vec<_>>();
    let changed_files = openclaw_repo::changed_files(&request.repo.root)?;
    let status = if verification.iter().any(|line| line.contains(" failed ")) {
        "verification failed"
    } else if changed_files.is_empty() {
        "blocked"
    } else {
        "patched"
    };

    println!(
        "{}",
        patch::summarize_patch_report(&patch::PatchOpenClawReport {
            status: status.to_string(),
            session_id,
            changed_files,
            verification,
        })
    );
    Ok(())
```

Add these helpers below `choose_default_harness`:

```rust
fn confirm_yes(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    io::stdin().read_line(&mut line).context("failed to read input")?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

fn launch_patch_session(request: &patch::PatchOpenClawRequest) -> Result<String> {
    let selected_harness = selected_available_harness(&request.harness_id)?;
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let now = current_timestamp();
    let brief = patch::build_repair_brief(request);
    let record = store::SessionRecord {
        id: Uuid::new_v4().to_string(),
        project_root: request.repo.root.to_string_lossy().into_owned(),
        harness: selected_harness.id.to_string(),
        title: session_title(Some("Patch OpenClaw"), &brief),
        status: DEFAULT_SESSION_STATUS.to_string(),
        exit_code: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    store::insert_session(&conn, &record)?;
    store::insert_json_event(
        &conn,
        &record.id,
        "patch_metadata",
        &serde_json::json!({
            "patchTarget": "openclaw",
            "repoRoot": request.repo.root,
            "issue": request.issue,
            "harnessId": request.harness_id,
            "verificationProfile": request.verification_profile.as_str(),
            "status": "running"
        }),
        &now,
    )?;

    store::update_session_status(&conn, &record.id, RUNNING_SESSION_STATUS, None, &current_timestamp())?;
    let command = pty_runner::build_harness_command(selected_harness.id, &brief, &request.repo.root)?;
    let result = pty_runner::run_attached(&command)?;
    store::update_session_status(&conn, &record.id, result.status, result.exit_code, &current_timestamp())?;
    Ok(record.id)
}
```

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p coven-cli patch::tests store::tests::inserts_json_event openclaw_repo::tests::changed_files_lists_modified_and_untracked_files
```

Expected: selected tests pass.

- [ ] **Step 6: Run dry-run smoke again**

Run:

```bash
cargo run -p coven-cli -- patch openclaw "fix auth order" --repo /Users/buns/Documents/GitHub/openclaw/openclaw --harness codex --dry-run
```

Expected: dry-run still prints the plan and repair brief, with no harness launch.

- [ ] **Step 7: Run non-interactive missing-input smoke**

Run:

```bash
cargo run -p coven-cli -- patch openclaw --repo /Users/buns/Documents/GitHub/openclaw/openclaw --non-interactive
```

Expected: exits non-zero with `issue text is required with --non-interactive` and does not launch a harness.

- [ ] **Step 8: Format and commit Task 5**

Run:

```bash
cargo fmt --check
cargo test -p coven-cli patch::tests store::tests::inserts_json_event openclaw_repo::tests verification::tests
```

Expected: all selected tests pass.

Commit:

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/patch.rs crates/coven-cli/src/store.rs crates/coven-cli/src/openclaw_repo.rs
git commit -S -m "feat: launch openclaw patch rescue sessions"
```

---

## Task 6: Public docs and final verification

**Files:**
- Modify: `docs/README.md`
- Modify: `docs/MVP-PLAN.md`
- Modify: `README.md`

- [ ] **Step 1: Update `docs/README.md` with rescue loop documentation**

Add this section near the command overview in `docs/README.md`:

````markdown
## OpenClaw rescue loop

Coven can help repair a local OpenClaw source checkout without relying on a healthy OpenClaw runtime:

```sh
coven patch openclaw
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
coven patch openclaw --repo ~/Documents/GitHub/openclaw/openclaw --harness codex --dry-run
```

The guided flow detects the repo, asks what is broken, launches a supervised Codex or Claude Code session, runs verification, and reports changed files. Coven does not commit or push in v0.
````

- [ ] **Step 2: Update `docs/MVP-PLAN.md` scope**

In `docs/MVP-PLAN.md`, add this bullet under the MVP in-scope list:

```markdown
- `coven patch openclaw` as a CLI-first rescue loop for local OpenClaw source checkouts.
```

Add this success criterion after the existing CLI/session success criteria:

```markdown
11. A user can run `coven patch openclaw --dry-run --repo <openclaw-source>` and see a safe repair plan without launching a harness.
12. A user can run `coven patch openclaw "<issue>" --repo <openclaw-source> --harness codex` to launch a supervised patch session that leaves changes uncommitted.
```

- [ ] **Step 3: Update root `README.md` with one short teaser**

Add this under the primary Coven description:

````markdown
Coven also provides a rescue loop for OpenClaw contributors and users:

```sh
coven patch openclaw
```

If OpenClaw breaks, Coven gives you a predictable repair room: choose a repo, choose a harness, get a verified patch.
````

- [ ] **Step 4: Run full verification gate**

Run:

```bash
cargo fmt --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected:

- format check passes;
- all workspace tests pass;
- clippy reports no warnings;
- diff check reports no whitespace errors.

- [ ] **Step 5: Manual smoke commands**

Run:

```bash
cargo run -p coven-cli -- patch openclaw "fix fixture issue" --repo /Users/buns/Documents/GitHub/openclaw/openclaw --harness codex --dry-run
cargo run -p coven-cli -- patch openclaw --repo /Users/buns/Documents/GitHub/openclaw/openclaw --non-interactive
```

Expected:

- first command prints a dry-run plan and repair brief;
- second command fails closed because issue text and harness are missing;
- neither command commits, pushes, or launches a harness.

- [ ] **Step 6: Commit docs and final polish**

Commit:

```bash
git add README.md docs/README.md docs/MVP-PLAN.md
git commit -S -m "docs: document openclaw patch rescue loop"
```

- [ ] **Step 7: Final status report**

Run:

```bash
git status --short
git log --oneline -5
```

Expected:

- working tree clean;
- latest five commits correspond to the task commits from this plan.

Report to Val:

```text
Implemented `coven patch openclaw` Phase 1.
Verification passed: cargo fmt --check, cargo test --workspace --locked, cargo clippy --workspace --all-targets -- -D warnings, git diff --check.
Manual smoke passed: dry-run plan and non-interactive fail-closed.
No commit/push behavior was added to the rescue loop.
```

---

## Plan self-review

### Spec coverage

- Guided beginner flow: Task 4 adds parser, prompted issue, default harness choice, dry-run plan; Task 5 adds confirmations and launch.
- Fast advanced flow: Task 4 supports issue text, `--repo`, `--harness`, `--verify`, `--non-interactive`, `--dry-run`, and `--keep-session` storage field.
- Repo detection: Task 1 implements explicit and ancestry detection with OpenClaw package/directory signals.
- Git state handling: Task 1 records branch, HEAD, dirty, and untracked files; Task 5 asks before proceeding when dirty in interactive mode.
- Repair brief: Task 2 requires root cause investigation, tests, verification output, no commits, no pushes, and preservation of existing changes.
- Harness launch: Task 5 reuses existing selected harness and PTY command paths.
- Verification: Task 3 implements `auto`, `pnpm-check`, `targeted-test`, and `diff-only` command selection with `git diff --check` as the safe base.
- Result summary: Task 5 adds final report with status, changed files, verification, and no commit/push reminder.
- Safety: Tasks 2, 4, and 5 keep no commit/push, no shell interpolation, repo-root cwd, and fail-closed non-interactive behavior.
- Public docs: Task 6 documents the command and v0 behavior.

### Type consistency

- `VerificationProfile::as_str()` is defined in Task 2 before use in Tasks 4 and 5.
- `OpenClawRepo`, `GitState`, and `changed_files()` live in `openclaw_repo.rs` and are referenced consistently.
- `PatchOpenClawRequest` owns `repo`, `git_state`, `issue`, `harness_id`, `verification_profile`, `non_interactive`, `dry_run`, and `keep_session`; all later tasks use those exact field names.
- `VerificationResult` and `VerificationStatus` live in `verification.rs`; Task 5 matches those exact enum variants.

### Verification strategy

The plan uses TDD for each module, targeted tests after each task, smoke tests before launch behavior, and a full final gate before reporting completion.
