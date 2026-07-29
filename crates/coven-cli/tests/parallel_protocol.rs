#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

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
fn default_claim_identity_blocks_same_user_in_another_worktree() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let first_worktree = repo.add_worktree("first-session", "feature/first-session")?;
    let second_worktree = repo.add_worktree("second-session", "feature/second-session")?;

    let acquired = repo.coven_in_with_env(
        &first_worktree,
        ["claim", "acquire", "issue-demo"],
        [("USER", "val")],
    )?;
    assert_success("claim acquire from first worktree", &acquired);

    let blocked = repo.coven_in_with_env(
        &second_worktree,
        ["claim", "acquire", "issue-demo"],
        [("USER", "val")],
    )?;
    assert_failure("claim acquire from second worktree", &blocked);
    assert_stderr_contains(
        "claim acquire from second worktree",
        &blocked,
        "already claimed by val@first-session",
    );
    assert_eq!(
        repo.claim_field("issue-demo", "agent_id")?,
        "val@first-session"
    );
    Ok(())
}

#[test]
fn default_claim_identity_supports_same_worktree_lifecycle() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let worktree = repo.add_worktree("one-session", "feature/one-session")?;
    let env = [("USER", "val")];

    let acquired = repo.coven_in_with_env(&worktree, ["claim", "acquire", "issue-demo"], env)?;
    assert_success("claim acquire with fallback identity", &acquired);
    assert_eq!(
        repo.claim_field("issue-demo", "agent_id")?,
        "val@one-session"
    );

    let heartbeat = repo.coven_in_with_env(&worktree, ["claim", "heartbeat", "issue-demo"], env)?;
    assert_success("claim heartbeat with fallback identity", &heartbeat);

    let released = repo.coven_in_with_env(&worktree, ["claim", "release", "issue-demo"], env)?;
    assert_success("claim release with fallback identity", &released);
    assert!(
        !repo.claim_path("issue-demo")?.exists(),
        "release should remove the claim"
    );
    Ok(())
}

#[test]
fn default_claim_identity_handles_blank_user() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let worktree = repo.add_worktree("one-session", "feature/one-session")?;
    let env = [("USER", "   "), ("COVEN_AGENT_ID", "   ")];

    let acquired = repo.coven_in_with_env(&worktree, ["claim", "acquire", "issue-demo"], env)?;
    assert_success("claim acquire with blank user", &acquired);
    assert_eq!(
        repo.claim_field("issue-demo", "agent_id")?,
        "unknown-agent@one-session"
    );

    let heartbeat = repo.coven_in_with_env(&worktree, ["claim", "heartbeat", "issue-demo"], env)?;
    assert_success("claim heartbeat with blank user", &heartbeat);

    let released = repo.coven_in_with_env(&worktree, ["claim", "release", "issue-demo"], env)?;
    assert_success("claim release with blank user", &released);
    assert!(
        !repo.claim_path("issue-demo")?.exists(),
        "release should remove the claim"
    );
    Ok(())
}

#[test]
fn explicit_agent_identity_remains_authoritative_across_worktrees() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let first_worktree = repo.add_worktree("first-session", "feature/first-session")?;
    let second_worktree = repo.add_worktree("second-session", "feature/second-session")?;
    let env = [("COVEN_AGENT_ID", "cody")];

    let acquired =
        repo.coven_in_with_env(&first_worktree, ["claim", "acquire", "issue-demo"], env)?;
    assert_success("claim acquire with explicit identity", &acquired);

    let refreshed =
        repo.coven_in_with_env(&second_worktree, ["claim", "acquire", "issue-demo"], env)?;
    assert_success("claim refresh with explicit identity", &refreshed);
    assert_eq!(repo.claim_field("issue-demo", "agent_id")?, "cody");
    Ok(())
}

#[test]
fn concurrent_claim_acquire_yields_exactly_one_winner() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;

    // A read-then-write acquire lets every racer observe a free slot and
    // then overwrite the others, so each one is told it holds the claim.
    let results = repo.race_acquire("feature/demo", &["cody", "sage", "nova", "kitty"], [])?;

    let winners: Vec<&str> = results
        .iter()
        .filter(|(_, output)| output.status.success())
        .map(|(agent, _)| *agent)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "expected exactly one winner, got {winners:?}"
    );

    // The agent that was told it won must be the one recorded on disk.
    assert_eq!(repo.claim_field("feature/demo", "agent_id")?, winners[0]);

    for (agent, output) in results.iter().filter(|(agent, _)| *agent != winners[0]) {
        assert_failure(&format!("claim acquire by {agent}"), output);
        assert_stderr_contains(
            &format!("claim acquire by {agent}"),
            output,
            "already claimed by",
        );
    }
    Ok(())
}

#[test]
fn concurrent_takeover_of_expired_claim_yields_exactly_one_winner() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;

    // Seed a claim that is already expired, so every racer is entitled to
    // take it over. Replacing an existing file cannot be arbitrated by an
    // exclusive create, which makes this the path most likely to double-win.
    let seeded = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [
            ("COVEN_AGENT_ID", "ghost"),
            ("COVEN_CLAIM_TTL_SECONDS", "1"),
        ],
    )?;
    assert_success("claim acquire by ghost", &seeded);
    std::thread::sleep(std::time::Duration::from_millis(1_200));

    let results = repo.race_acquire("feature/demo", &["cody", "sage", "nova", "kitty"], [])?;

    let winners: Vec<&str> = results
        .iter()
        .filter(|(_, output)| output.status.success())
        .map(|(agent, _)| *agent)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "expected exactly one winner taking over the expired claim, got {winners:?}"
    );
    assert_eq!(repo.claim_field("feature/demo", "agent_id")?, winners[0]);
    Ok(())
}

#[test]
fn claim_acquire_by_the_same_agent_extends_its_own_claim() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;

    let first = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [
            ("COVEN_AGENT_ID", "cody"),
            ("COVEN_CLAIM_TTL_SECONDS", "60"),
        ],
    )?;
    assert_success("first claim acquire by cody", &first);
    let first_expiry: u64 = repo.claim_field("feature/demo", "expires_at")?.parse()?;

    let again = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [
            ("COVEN_AGENT_ID", "cody"),
            ("COVEN_CLAIM_TTL_SECONDS", "6000"),
        ],
    )?;
    assert_success("re-acquire by the same agent", &again);

    let second_expiry: u64 = repo.claim_field("feature/demo", "expires_at")?.parse()?;
    assert!(
        second_expiry > first_expiry,
        "re-acquiring should extend the owner's claim: {first_expiry} -> {second_expiry}"
    );
    assert_eq!(repo.claim_field("feature/demo", "agent_id")?, "cody");
    Ok(())
}

#[test]
fn claim_acquire_does_not_steal_an_incomplete_claim() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let claims_dir = repo.claims_dir()?;
    fs::create_dir_all(&claims_dir)?;
    let claim_path = claims_dir.join("feature-demo");
    fs::write(&claim_path, "")?;

    let acquire = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_failure("claim acquire while initial write is incomplete", &acquire);
    assert_stderr_contains(
        "claim acquire while initial write is incomplete",
        &acquire,
        "contended",
    );
    assert_eq!(fs::read(&claim_path)?, b"");
    Ok(())
}

#[test]
fn claim_acquire_recovers_an_abandoned_incomplete_claim() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let claims_dir = repo.claims_dir()?;
    fs::create_dir_all(&claims_dir)?;
    let claim_path = claims_dir.join("feature-demo");
    fs::write(&claim_path, "")?;
    fs::File::options()
        .write(true)
        .open(&claim_path)?
        .set_times(
            fs::FileTimes::new()
                .set_accessed(SystemTime::UNIX_EPOCH)
                .set_modified(SystemTime::UNIX_EPOCH),
        )?;

    let acquire = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_success("claim acquire after abandoned initial write", &acquire);
    assert_eq!(repo.claim_field("feature/demo", "agent_id")?, "sage");
    Ok(())
}

#[test]
fn claim_release_rejects_an_incomplete_claim() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    fs::create_dir_all(repo.claims_dir()?)?;
    fs::write(repo.claims_dir()?.join("feature-demo"), "")?;

    let release = repo.coven_with_env(
        ["claim", "release", "feature/demo"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_failure("claim release while initial write is incomplete", &release);
    assert_stderr_contains(
        "claim release while initial write is incomplete",
        &release,
        "incomplete",
    );
    Ok(())
}

#[test]
fn claim_heartbeat_rejects_an_incomplete_claim() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    fs::create_dir_all(repo.claims_dir()?)?;
    fs::write(repo.claims_dir()?.join("feature-demo"), "")?;

    let heartbeat = repo.coven_with_env(
        ["claim", "heartbeat", "feature/demo"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_failure(
        "claim heartbeat while initial write is incomplete",
        &heartbeat,
    );
    assert_stderr_contains(
        "claim heartbeat while initial write is incomplete",
        &heartbeat,
        "incomplete",
    );
    Ok(())
}

#[test]
fn claim_acquire_respects_live_takeover_lock_when_claim_is_missing() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let claims_dir = repo.claims_dir()?;
    fs::create_dir_all(&claims_dir)?;
    fs::write(claims_dir.join("@feature-demo.takeover"), "")?;

    let acquire = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "sage")],
    )?;
    assert_failure("claim acquire during takeover", &acquire);
    assert_stderr_contains("claim acquire during takeover", &acquire, "contended");
    assert!(!claims_dir.join("feature-demo").exists());
    Ok(())
}

#[test]
fn claim_status_ignores_leftover_internal_files() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let acquired = repo.coven_with_env(
        ["claim", "acquire", "feature/demo"],
        [("COVEN_AGENT_ID", "cody")],
    )?;
    assert_success("claim acquire by cody", &acquired);

    // Staging and lock files use the reserved `@`-prefixed namespace; a crash
    // mid-takeover can leave one behind and it must not read as a claim.
    let claims_dir = repo.claims_dir()?;
    fs::write(claims_dir.join("@feature-demo.takeover"), "")?;
    fs::write(
        claims_dir.join("@feature-demo.write.1.2"),
        "branch=feature/demo\nagent_id=stale\nacquired_at=0\nexpires_at=0\n",
    )?;

    let status = repo.coven(["claim", "status"])?;
    assert_success("claim status", &status);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        !stdout.contains("stale"),
        "claim status must ignore internal files, got:\n{stdout}"
    );
    assert_eq!(
        stdout.lines().filter(|line| line.contains("cody")).count(),
        1,
        "expected exactly one claim row, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn claim_status_lists_real_dot_prefixed_claims() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    fs::create_dir_all(repo.claims_dir()?)?;
    fs::write(
        repo.claims_dir()?.join(".foo"),
        "branch=.foo\nagent_id=dotty\nacquired_at=0\nexpires_at=9999999999\n",
    )?;

    let status = repo.coven(["claim", "status"])?;
    assert_success("claim status", &status);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains(".foo"),
        "claim status should list real dot-prefixed claims, got:\n{stdout}"
    );
    assert!(
        stdout.contains("dotty"),
        "claim status should show the dot-prefixed claim owner, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn claim_status_lists_real_claim_matching_staging_filename_shape() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let acquired = repo.coven_with_env(
        ["claim", "acquire", ".feature.write.123.456"],
        [("COVEN_AGENT_ID", "dotty")],
    )?;
    assert_success("claim acquire by dotty", &acquired);

    let status = repo.coven(["claim", "status"])?;
    assert_success("claim status", &status);
    assert_stdout_contains("claim status", &status, ".feature.write.123.456");
    assert_stdout_contains("claim status", &status, "dotty");
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
fn managed_hook_uses_worktree_scoped_default_identity() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let install = repo.coven(["hooks", "install"])?;
    assert_success("hooks install", &install);

    repo.git(["checkout", "-b", "feature/demo"])?;
    let owner_env = [("USER", "val")];
    let claim = repo.coven_with_env(["claim", "acquire", "feature/demo"], owner_env)?;
    assert_success("claim feature/demo with fallback identity", &claim);
    assert_eq!(repo.claim_field("feature/demo", "agent_id")?, "val@project");

    fs::write(repo.path.join("main.txt"), "allowed\n")?;
    let allowed = repo.git_output(["commit", "-am", "allowed by fallback owner"], owner_env)?;
    assert_success("commit with fallback owning identity", &allowed);

    let released = repo.coven_with_env(["claim", "release", "feature/demo"], owner_env)?;
    assert_success("release fallback-owned claim", &released);
    let other_worktree = repo.add_worktree("other-session", "feature/other-session")?;
    let other_claim = repo.coven_in_with_env(
        &other_worktree,
        ["claim", "acquire", "feature/demo"],
        owner_env,
    )?;
    assert_success("claim from another worktree", &other_claim);

    fs::write(repo.path.join("main.txt"), "blocked\n")?;
    let blocked = repo.git_output(["commit", "-am", "blocked by other worktree"], owner_env)?;
    assert_failure("commit against another worktree's claim", &blocked);
    assert_stderr_contains(
        "commit against another worktree's claim",
        &blocked,
        "claimed by val@other-session",
    );
    Ok(())
}

#[test]
fn managed_hook_treats_blank_explicit_agent_id_as_unset() -> anyhow::Result<()> {
    let repo = TestRepo::new()?;
    let install = repo.coven(["hooks", "install"])?;
    assert_success("hooks install", &install);

    repo.git(["checkout", "-b", "feature/demo"])?;
    let env = [("USER", "val"), ("COVEN_AGENT_ID", "   ")];
    let claim = repo.coven_with_env(["claim", "acquire", "feature/demo"], env)?;
    assert_success("claim with blank explicit identity", &claim);
    assert_eq!(repo.claim_field("feature/demo", "agent_id")?, "val@project");

    fs::write(repo.path.join("main.txt"), "allowed\n")?;
    let allowed = repo.git_output(["commit", "-am", "allowed by fallback owner"], env)?;
    assert_success("commit with blank explicit identity", &allowed);
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
        self.coven_in_with_env(&self.path, args, env)
    }

    fn coven_in_with_env<const N: usize, const M: usize>(
        &self,
        cwd: &Path,
        args: [&str; N],
        env: [(&str, &str); M],
    ) -> anyhow::Result<Output> {
        let mut command = Command::new(coven_bin());
        command
            .args(args)
            .current_dir(cwd)
            .env("COVEN_HOME", self.path.join(".coven-home"))
            .env_remove("COVEN_AGENT_ID")
            .env_remove("COVEN_ALLOW_PRIMARY_COMMIT");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().map_err(Into::into)
    }

    fn add_worktree(&self, name: &str, branch: &str) -> anyhow::Result<PathBuf> {
        let path = self._temp.path().join(name);
        let output = self.git_os(["worktree", "add", "-b", branch], [&path])?;
        assert_success("git worktree add", &output);
        Ok(path)
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

    fn claims_dir(&self) -> anyhow::Result<PathBuf> {
        Ok(self.git_common_dir()?.join("agent-claims"))
    }

    fn claim_path(&self, branch: &str) -> anyhow::Result<PathBuf> {
        let slug: String = branch
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                    ch
                } else {
                    '-'
                }
            })
            .collect();
        Ok(self.claims_dir()?.join(slug.trim_matches('-')))
    }

    fn claim_field(&self, branch: &str, key: &str) -> anyhow::Result<String> {
        let path = self.claim_path(branch)?;
        let contents = fs::read_to_string(&path)
            .map_err(|err| anyhow::anyhow!("reading {}: {err}", path.display()))?;
        contents
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("no {key} in {}:\n{contents}", path.display()))
    }

    /// Launch one `claim acquire` per agent as concurrently as processes
    /// allow, so the acquire path is exercised as a real race rather than a
    /// sequence. Every child is spawned before any of them is waited on.
    fn race_acquire<'a, const M: usize>(
        &self,
        branch: &str,
        agents: &[&'a str],
        env: [(&str, &str); M],
    ) -> anyhow::Result<Vec<(&'a str, Output)>> {
        let children = agents
            .iter()
            .map(|agent| {
                let mut command = Command::new(coven_bin());
                command
                    .args(["claim", "acquire", branch])
                    .current_dir(&self.path)
                    .env("COVEN_HOME", self.path.join(".coven-home"))
                    .env("COVEN_AGENT_ID", agent)
                    .env_remove("COVEN_ALLOW_PRIMARY_COMMIT")
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                for (key, value) in env {
                    command.env(key, value);
                }
                command.spawn().map(|child| (*agent, child))
            })
            .collect::<Result<Vec<_>, _>>()?;

        children
            .into_iter()
            .map(|(agent, child)| child.wait_with_output().map(|output| (agent, output)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
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
