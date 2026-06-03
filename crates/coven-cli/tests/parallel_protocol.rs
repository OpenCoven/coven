#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[test]
fn wt_creates_sibling_worktree_and_lists_protocol_state() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let output = repo.coven(["wt", "feature/demo"])?;
    assert_success("coven wt feature/demo", &output);

    let worktree = repo.path.with_extension("wt").join("feature-demo");
    assert!(
        worktree.join(".git").exists(),
        "expected worktree at {}",
        worktree.display()
    );
    assert_eq!(
        repo.git_in(&worktree, ["branch", "--show-current"])?,
        "feature/demo"
    );

    let list = repo.coven(["wt", "--list"])?;
    assert_success("coven wt --list", &list);
    assert_stdout_contains("coven wt --list", &list, "feature/demo");
    assert_stdout_contains("coven wt --list", &list, "feature-demo");
    Ok(())
}

#[test]
fn claim_acquire_blocks_other_agent_until_release() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;

    let acquired = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "cody")],
    )?;
    assert_success("claim acquire by cody", &acquired);
    assert_stdout_contains("claim acquire by cody", &acquired, "claimed feature/demo");

    let blocked = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_failure("claim acquire by sage", &blocked);
    assert_stderr_contains("claim acquire by sage", &blocked, "claimed by cody");

    let status = repo.coven(["claim", "status"])?;
    assert_success("claim status", &status);
    assert_stdout_contains("claim status", &status, "feature/demo");
    assert_stdout_contains("claim status", &status, "cody");

    let released = repo.coven_with_env(
        ["claim", "release", "feature/demo"],
        [("COVEN_AGENT_ID", "cody")],
    )?;
    assert_success("claim release by cody", &released);

    let reacquired = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_success("claim acquire by sage after release", &reacquired);
    Ok(())
}

#[test]
fn installed_hooks_block_primary_commits_and_claim_conflicts() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;

    let install = repo.coven(["hooks", "install"])?;
    assert_success("hooks install", &install);

    fs::write(repo.path.join("main.txt"), "blocked\n")?;
    let blocked_main = repo.git_output(["commit", "-am", "blocked on main"], [])?;
    assert_failure("commit on main", &blocked_main);
    assert_stderr_contains(
        "commit on main",
        &blocked_main,
        "Coven Parallel Work Protocol",
    );

    let allowed_main = repo.git_output(
        ["commit", "-am", "explicit main commit"],
        [("COVEN_ALLOW_PRIMARY_COMMIT", "1")],
    )?;
    assert_success("commit on main with override", &allowed_main);

    repo.git(["checkout", "-b", "feature/demo"])?;
    let claim = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "cody")],
    )?;
    assert_success("claim feature/demo", &claim);

    fs::write(repo.path.join("main.txt"), "conflict\n")?;
    let blocked_claim = repo.git_output(
        ["commit", "-am", "blocked by claim"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_failure("commit with another agent claim", &blocked_claim);
    assert_stderr_contains(
        "commit with another agent claim",
        &blocked_claim,
        "claimed by cody",
    );

    let allowed_claim = repo.git_output(
        ["commit", "-am", "allowed by owner"],
        [("COVEN_AGENT_ID", "cody")],
    )?;
    assert_success("commit with owning claim", &allowed_claim);
    Ok(())
}

#[test]
fn installed_pre_push_requires_merge_intent_for_primary_and_consumes_it() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let remote_dir = tempfile::tempdir()?;
    let remote = remote_dir.path().join("remote.git");
    Command::new("git")
        .args(["init", "--bare"])
        .arg(&remote)
        .output()?;
    repo.git_os(["remote", "add", "origin"], [&remote])?;

    let install = repo.coven(["hooks", "install"])?;
    assert_success("hooks install", &install);

    let blocked = repo.git_output(["push", "origin", "main"], [])?;
    assert_failure("push main without intent", &blocked);
    assert_stderr_contains("push main without intent", &blocked, "MERGE_INTENT");

    fs::write(
        repo.git_common_dir()?.join("MERGE_INTENT"),
        "Enchant merge to main.",
    )?;
    let allowed = repo.git_output(["push", "origin", "main"], [])?;
    assert_success("push main with intent", &allowed);
    assert!(
        !repo.git_common_dir()?.join("MERGE_INTENT").exists(),
        "successful protected push should consume MERGE_INTENT"
    );
    Ok(())
}

struct TestRepo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}

impl TestRepo {
    fn new() -> anyhow::Result<Self> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("project");
        fs::create_dir(&path)?;
        let repo = Self { _temp: temp, path };
        repo.git(["init", "--initial-branch=main"])?;
        repo.git(["config", "user.email", "coven@example.test"])?;
        repo.git(["config", "user.name", "Coven Test"])?;
        fs::write(repo.path.join("main.txt"), "initial\n")?;
        repo.git(["add", "main.txt"])?;
        repo.git(["commit", "-m", "initial"])?;
        Ok(repo)
    }

    fn coven<const N: usize>(&self, args: [&str; N]) -> anyhow::Result<Output> {
        self.coven_with_env(args, [])
    }

    fn coven_with_env<const N: usize, const M: usize>(
        &self,
        args: [&str; N],
        env: [(&str, &str); M],
    ) -> anyhow::Result<Output> {
        let mut command = Command::new(coven_bin());
        command
            .args(args)
            .current_dir(&self.path)
            .env("COVEN_HOME", self.path.join(".coven-home"))
            .env_remove("COVEN_AGENT_ID")
            .env_remove("COVEN_ALLOW_PRIMARY_COMMIT");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().map_err(Into::into)
    }

    fn git<const N: usize>(&self, args: [&str; N]) -> anyhow::Result<String> {
        self.git_in(&self.path, args)
    }

    fn git_in<const N: usize>(&self, cwd: &Path, args: [&str; N]) -> anyhow::Result<String> {
        let output = Command::new("git").args(args).current_dir(cwd).output()?;
        assert_success("git", &output);
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn git_os<const N: usize, const M: usize>(
        &self,
        args: [&str; N],
        os_args: [&Path; M],
    ) -> anyhow::Result<Output> {
        let mut command = Command::new("git");
        command.args(args).args(os_args).current_dir(&self.path);
        command.output().map_err(Into::into)
    }

    fn git_output<const N: usize, const M: usize>(
        &self,
        args: [&str; N],
        env: [(&str, &str); M],
    ) -> anyhow::Result<Output> {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(&self.path)
            .env_remove("COVEN_AGENT_ID")
            .env_remove("COVEN_ALLOW_PRIMARY_COMMIT");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().map_err(Into::into)
    }

    fn git_common_dir(&self) -> anyhow::Result<PathBuf> {
        Ok(self.path.join(self.git(["rev-parse", "--git-common-dir"])?))
    }
}

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(label: &str, output: &Output) {
    assert!(
        !output.status.success(),
        "{label} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_stdout_contains(label: &str, output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(needle),
        "{label} stdout did not contain {needle:?}\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_stderr_contains(label: &str, output: &Output, needle: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(needle),
        "{label} stderr did not contain {needle:?}\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
}
