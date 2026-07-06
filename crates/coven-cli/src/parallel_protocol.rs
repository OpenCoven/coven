use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
const CLAIM_TTL_SECONDS: u64 = 60 * 60;
const DEFAULT_PRIMARY_BRANCH: &str = "main";
const MANAGED_HOOK_MARKER: &str = "Coven Parallel Work Protocol managed hook";

#[derive(Debug, Clone)]
struct Repo {
    root: PathBuf,
    common_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct Claim {
    branch: String,
    agent_id: String,
    acquired_at: u64,
    expires_at: u64,
    head: Option<String>,
}

#[derive(Debug)]
struct Worktree {
    path: PathBuf,
    branch: Option<String>,
}

pub(crate) fn run_wt_command(
    branch: Option<&str>,
    list: bool,
    doctor: bool,
    prune_merged: bool,
    prune_stale: Option<u64>,
) -> Result<()> {
    let repo = Repo::discover()?;
    match (branch, list, doctor, prune_merged, prune_stale) {
        (Some(branch), false, false, false, None) => wt_enter_or_create(&repo, branch),
        (None, true, false, false, None) => wt_list(&repo),
        (None, false, true, false, None) => wt_doctor(&repo),
        (None, false, false, true, None) => wt_prune_merged(&repo),
        (None, false, false, false, Some(days)) => wt_prune_stale(&repo, days),
        // Unreachable today (clap requires one action), but fail loudly if
        // that constraint ever loosens instead of printing usage with exit 0.
        (None, false, false, false, None) => anyhow::bail!(
            "usage: coven wt <branch> | --list | --doctor | --prune-merged | --prune-stale DAYS"
        ),
        _ => anyhow::bail!("choose exactly one `coven wt` action"),
    }
}

pub(crate) fn claim_acquire(branch: &str) -> Result<()> {
    let repo = Repo::discover()?;
    let agent_id = agent_id();
    let now = unix_now();
    if let Some(existing) = read_claim(&repo, branch)? {
        if existing.is_active(now) && existing.agent_id != agent_id {
            anyhow::bail!(
                "{} is already claimed by {} until {}",
                branch,
                existing.agent_id,
                existing.expires_at
            );
        }
    }

    let claim = Claim {
        branch: branch.to_string(),
        agent_id: agent_id.clone(),
        acquired_at: now,
        expires_at: now + claim_ttl_seconds(),
        head: current_head().ok(),
    };
    write_claim(&repo, &claim)?;
    println!("claimed {branch} for {agent_id} until {}", claim.expires_at);
    Ok(())
}

pub(crate) fn claim_release(branch: &str) -> Result<()> {
    let repo = Repo::discover()?;
    let agent_id = agent_id();
    if let Some(existing) = read_claim(&repo, branch)? {
        if existing.is_active(unix_now()) && existing.agent_id != agent_id {
            anyhow::bail!(
                "{} is claimed by {}; {} cannot release it",
                branch,
                existing.agent_id,
                agent_id
            );
        }
        fs::remove_file(claim_path(&repo, branch))
            .with_context(|| format!("failed to release claim for {branch}"))?;
        println!("released {branch}");
    } else {
        println!("no claim for {branch}");
    }
    Ok(())
}

pub(crate) fn claim_heartbeat(branch: &str) -> Result<()> {
    let repo = Repo::discover()?;
    let agent_id = agent_id();
    let now = unix_now();
    let mut claim = read_claim(&repo, branch)?.unwrap_or_else(|| Claim {
        branch: branch.to_string(),
        agent_id: agent_id.clone(),
        acquired_at: now,
        expires_at: now,
        head: current_head().ok(),
    });
    if claim.is_active(now) && claim.agent_id != agent_id {
        anyhow::bail!(
            "{} is claimed by {}; {} cannot heartbeat it",
            branch,
            claim.agent_id,
            agent_id
        );
    }
    claim.agent_id = agent_id.clone();
    claim.expires_at = now + claim_ttl_seconds();
    write_claim(&repo, &claim)?;
    println!(
        "heartbeat {branch} for {agent_id} until {}",
        claim.expires_at
    );
    Ok(())
}

pub(crate) fn claim_canary(branch: &str) -> Result<()> {
    let repo = Repo::discover()?;
    let head = current_head()?;
    let canary_path = repo.common_dir.join("AGENT_HEAD_AT_START");
    fs::write(&canary_path, format!("branch={branch}\nhead={head}\n"))
        .with_context(|| format!("failed to write {}", canary_path.display()))?;
    println!("recorded canary for {branch} at {head}");
    Ok(())
}

pub(crate) fn claim_status() -> Result<()> {
    let repo = Repo::discover()?;
    let claims_dir = repo.common_dir.join("agent-claims");
    if !claims_dir.exists() {
        println!("No claims.");
        return Ok(());
    }
    let now = unix_now();
    let mut claims = fs::read_dir(&claims_dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| read_claim_file(&entry.path()).ok().flatten())
        .collect::<Vec<_>>();
    claims.sort_by(|a, b| a.branch.cmp(&b.branch));
    if claims.is_empty() {
        println!("No claims.");
        return Ok(());
    }
    println!("{:<32} {:<20} {:<10} expires", "branch", "agent", "state");
    for claim in claims {
        let state = if claim.is_active(now) {
            "active"
        } else {
            "expired"
        };
        println!(
            "{:<32} {:<20} {:<10} {}",
            claim.branch, claim.agent_id, state, claim.expires_at
        );
    }
    Ok(())
}

pub(crate) fn hooks_install() -> Result<()> {
    let repo = Repo::discover()?;
    let hooks_path = git_config("core.hooksPath")?;
    if hooks_path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
    {
        anyhow::bail!(
            "core.hooksPath is set to {}. Coven will not auto-modify tracked hook directories.\n\
             Integration options:\n\
             1. Run the Coven checks from that tracked hook directory.\n\
             2. Move the tracked hook to .git/hooks/<hook>.local and unset core.hooksPath.",
            hooks_path.unwrap()
        );
    }

    let hooks_dir = repo.common_dir.join("hooks");
    fs::create_dir_all(&hooks_dir)?;
    install_hook(&hooks_dir, "pre-commit", PRE_COMMIT_HOOK)?;
    install_hook(&hooks_dir, "pre-push", PRE_PUSH_HOOK)?;
    println!(
        "installed Coven Parallel Work Protocol hooks in {}",
        hooks_dir.display()
    );
    Ok(())
}

fn wt_enter_or_create(repo: &Repo, branch: &str) -> Result<()> {
    let path = worktree_path(repo, branch)?;
    if path.exists() {
        println!("{}", path.display());
        return Ok(());
    }
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("invalid worktree path {}", path.display()))?,
    )?;
    let branch_exists = git_success(["show-ref", "--verify", &format!("refs/heads/{branch}")]);
    if branch_exists {
        run_git(["worktree", "add"], [&path, Path::new(branch)])?;
    } else {
        run_git(["worktree", "add", "-b", branch], [&path])?;
    }
    println!("{}", path.display());
    Ok(())
}

fn wt_list(repo: &Repo) -> Result<()> {
    let worktrees = list_worktrees()?;
    println!("{:<32} {:<8} {:<20} path", "branch", "dirty", "claim");
    for worktree in worktrees {
        let branch = worktree.branch.as_deref().unwrap_or("(detached)");
        let dirty = if worktree_dirty(&worktree.path)? {
            "dirty"
        } else {
            "clean"
        };
        let claim = worktree
            .branch
            .as_deref()
            .and_then(|branch| read_claim(repo, branch).ok().flatten())
            .filter(|claim| claim.is_active(unix_now()))
            .map(|claim| claim.agent_id)
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<32} {:<8} {:<20} {}",
            branch,
            dirty,
            claim,
            worktree.path.display()
        );
    }
    Ok(())
}

fn wt_doctor(repo: &Repo) -> Result<()> {
    println!("Coven Parallel Work Protocol doctor");
    println!("repo: {}", repo.root.display());
    println!("worktree root: {}", worktree_root(repo)?.display());
    println!("claims: {}", repo.common_dir.join("agent-claims").display());
    for hook in ["pre-commit", "pre-push"] {
        let path = repo.common_dir.join("hooks").join(hook);
        let status = if hook_is_managed(&path)? {
            "OK"
        } else {
            "missing"
        };
        println!("hook {hook}: {status}");
    }
    for worktree in list_worktrees()? {
        let expected_root = worktree_root(repo)?;
        if worktree.path != repo.root && !worktree.path.starts_with(&expected_root) {
            println!(
                "layout warning: {} is outside {}",
                worktree.path.display(),
                expected_root.display()
            );
        }
    }
    Ok(())
}

fn wt_prune_merged(repo: &Repo) -> Result<()> {
    let primary = primary_branch();
    let merged = git_stdout(["branch", "--merged", &primary])?;
    let merged = merged
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .filter(|branch| !branch.is_empty() && *branch != primary)
        .map(str::to_string)
        .collect::<Vec<_>>();
    prune_worktrees(repo, |worktree| {
        Ok(worktree
            .branch
            .as_deref()
            .is_some_and(|branch| merged.iter().any(|merged| merged == branch)))
    })
}

fn wt_prune_stale(repo: &Repo, days: u64) -> Result<()> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(days.saturating_mul(24 * 60 * 60)))
        .unwrap_or(UNIX_EPOCH);
    prune_worktrees(repo, |worktree| {
        Ok(fs::metadata(&worktree.path)
            .and_then(|metadata| metadata.modified())
            .map(|modified| modified < cutoff)
            .unwrap_or(false))
    })
}

fn prune_worktrees(repo: &Repo, should_prune: impl Fn(&Worktree) -> Result<bool>) -> Result<()> {
    let mut pruned = 0usize;
    for worktree in list_worktrees()? {
        if worktree.path == repo.root || !should_prune(&worktree)? {
            continue;
        }
        if worktree_dirty(&worktree.path)? {
            println!("skip dirty {}", worktree.path.display());
            continue;
        }
        run_git(["worktree", "remove"], [&worktree.path])?;
        println!("removed {}", worktree.path.display());
        pruned += 1;
    }
    println!("pruned {pruned} worktree(s)");
    Ok(())
}

fn list_worktrees() -> Result<Vec<Worktree>> {
    let output = git_stdout(["worktree", "list", "--porcelain"])?;
    let mut worktrees = Vec::new();
    let mut path = None;
    let mut branch = None;
    for line in output.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(path) = path.take() {
                worktrees.push(Worktree {
                    path,
                    branch: branch.take(),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
        } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
            branch = Some(value.to_string());
        }
    }
    Ok(worktrees)
}

fn worktree_dirty(path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if !output.status.success() {
        anyhow::bail!("git status failed in {}", path.display());
    }
    Ok(!output.stdout.is_empty())
}

fn worktree_root(repo: &Repo) -> Result<PathBuf> {
    let name = repo
        .root
        .file_name()
        .ok_or_else(|| anyhow!("repo root has no file name: {}", repo.root.display()))?;
    Ok(repo
        .root
        .with_file_name(format!("{}.wt", name.to_string_lossy())))
}

fn worktree_path(repo: &Repo, branch: &str) -> Result<PathBuf> {
    Ok(worktree_root(repo)?.join(branch_slug(branch)))
}

fn write_claim(repo: &Repo, claim: &Claim) -> Result<()> {
    let claims_dir = repo.common_dir.join("agent-claims");
    fs::create_dir_all(&claims_dir)?;
    let path = claim_path(repo, &claim.branch);
    let mut file = fs::File::create(&path)
        .with_context(|| format!("failed to create claim {}", path.display()))?;
    writeln!(file, "branch={}", claim.branch)?;
    writeln!(file, "agent_id={}", claim.agent_id)?;
    writeln!(file, "acquired_at={}", claim.acquired_at)?;
    writeln!(file, "expires_at={}", claim.expires_at)?;
    if let Some(head) = &claim.head {
        writeln!(file, "head={head}")?;
    }
    Ok(())
}

fn read_claim(repo: &Repo, branch: &str) -> Result<Option<Claim>> {
    read_claim_file(&claim_path(repo, branch))
}

fn read_claim_file(path: &Path) -> Result<Option<Claim>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read claim {}", path.display()))?;
    let value = |key: &str| -> Option<String> {
        contents.lines().find_map(|line| {
            line.split_once('=')
                .filter(|(found, _)| *found == key)
                .map(|(_, value)| value.to_string())
        })
    };
    let branch = value("branch").unwrap_or_else(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });
    let Some(agent_id) = value("agent_id") else {
        return Ok(None);
    };
    let acquired_at = value("acquired_at")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let expires_at = value("expires_at")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    Ok(Some(Claim {
        branch,
        agent_id,
        acquired_at,
        expires_at,
        head: value("head"),
    }))
}

fn claim_path(repo: &Repo, branch: &str) -> PathBuf {
    repo.common_dir
        .join("agent-claims")
        .join(branch_slug(branch))
}

impl Claim {
    fn is_active(&self, now: u64) -> bool {
        self.expires_at > now
    }
}

impl Repo {
    fn discover() -> Result<Self> {
        if !git_success(["rev-parse", "--is-inside-work-tree"]) {
            let cwd = std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "<unknown>".to_string());
            anyhow::bail!(
                "this command needs a git repository; run it inside one (current directory: {cwd})"
            );
        }
        let root = PathBuf::from(git_stdout(["rev-parse", "--show-toplevel"])?.trim());
        let common = PathBuf::from(git_stdout(["rev-parse", "--git-common-dir"])?.trim());
        let common_dir = if common.is_absolute() {
            common
        } else {
            root.join(common)
        };
        Ok(Self { root, common_dir })
    }
}

fn install_hook(hooks_dir: &Path, hook: &str, contents: &str) -> Result<()> {
    let path = hooks_dir.join(hook);
    let local = hooks_dir.join(format!("{hook}.local"));
    if path.exists() && !hook_is_managed(&path)? {
        if local.exists() {
            anyhow::bail!(
                "{} already exists and {} is not Coven-managed; refusing to overwrite either hook",
                path.display(),
                local.display()
            );
        }
        fs::rename(&path, &local)
            .with_context(|| format!("failed to move existing hook to {}", local.display()))?;
    }
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    set_executable(&path)?;
    Ok(())
}

fn hook_is_managed(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    Ok(fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?
        .contains(MANAGED_HOOK_MARKER))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn current_head() -> Result<String> {
    Ok(git_stdout(["rev-parse", "HEAD"])?.trim().to_string())
}

fn git_config(key: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .output()
        .with_context(|| format!("failed to read git config {key}"))?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ));
    }
    Ok(None)
}

fn git_stdout<const N: usize>(args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        anyhow::bail!(
            "git failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr).trim_end(),
            String::from_utf8_lossy(&output.stdout).trim_end()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_success<const N: usize>(args: [&str; N]) -> bool {
    Command::new("git")
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_git<const N: usize, const M: usize>(args: [&str; N], path_args: [&Path; M]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .args(path_args.iter().map(|path| path.as_os_str()))
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        anyhow::bail!(
            "git failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr).trim_end(),
            String::from_utf8_lossy(&output.stdout).trim_end()
        );
    }
    Ok(())
}

fn branch_slug(branch: &str) -> String {
    let mut slug = String::with_capacity(branch.len());
    for ch in branch.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            slug.push(ch);
        } else {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

fn primary_branch() -> String {
    std::env::var("COVEN_PRIMARY_BRANCH").unwrap_or_else(|_| DEFAULT_PRIMARY_BRANCH.to_string())
}

fn agent_id() -> String {
    std::env::var("COVEN_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown-agent".to_string())
}

fn claim_ttl_seconds() -> u64 {
    std::env::var("COVEN_CLAIM_TTL_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(CLAIM_TTL_SECONDS)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

const PRE_COMMIT_HOOK: &str = r#"#!/bin/sh
# Coven Parallel Work Protocol managed hook
set -eu

slug_branch() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '-' | sed -e 's/^-*//' -e 's/-*$//'
}

claim_value() {
  key="$1"
  file="$2"
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "$file"
}

branch="$(git symbolic-ref --quiet --short HEAD || true)"
primary="${COVEN_PRIMARY_BRANCH:-main}"
agent="${COVEN_AGENT_ID:-${USER:-unknown-agent}}"
common_dir="$(git rev-parse --git-common-dir)"

if [ "$branch" = "$primary" ] && [ "${COVEN_ALLOW_PRIMARY_COMMIT:-}" != "1" ]; then
  echo "Coven Parallel Work Protocol: refusing commit on protected primary branch '$primary'." >&2
  echo "Set COVEN_ALLOW_PRIMARY_COMMIT=1 only for explicit human-approved primary commits." >&2
  exit 1
fi

if [ -n "$branch" ]; then
  claim_file="$common_dir/agent-claims/$(slug_branch "$branch")"
  if [ -f "$claim_file" ]; then
    claim_agent="$(claim_value agent_id "$claim_file")"
    expires_at="$(claim_value expires_at "$claim_file")"
    now="$(date +%s)"
    if [ -n "$claim_agent" ] && [ "${expires_at:-0}" -gt "$now" ] && [ "$claim_agent" != "$agent" ]; then
      echo "Coven Parallel Work Protocol: branch '$branch' is claimed by $claim_agent; current agent is $agent." >&2
      exit 1
    fi
  fi
fi

canary="$common_dir/AGENT_HEAD_AT_START"
if [ -f "$canary" ] && [ -n "$branch" ]; then
  canary_branch="$(claim_value branch "$canary")"
  canary_head="$(claim_value head "$canary")"
  if [ "$canary_branch" = "$branch" ] && [ -n "$canary_head" ] && git cat-file -e "$canary_head^{commit}" 2>/dev/null; then
    if ! git merge-base --is-ancestor "$canary_head" HEAD; then
      echo "Coven Parallel Work Protocol: HEAD canary tripped for '$branch'." >&2
      echo "Current HEAD is not a descendant of $canary_head." >&2
      exit 1
    fi
  fi
fi

if [ -x "$common_dir/hooks/pre-commit.local" ]; then
  "$common_dir/hooks/pre-commit.local" "$@"
fi
"#;

const PRE_PUSH_HOOK: &str = r#"#!/bin/sh
# Coven Parallel Work Protocol managed hook
set -eu

primary="${COVEN_PRIMARY_BRANCH:-main}"
protected_regex="${COVEN_PROTECTED_REGEX:-^(release|hotfix)/}"
merge_phrase="${COVEN_MERGE_PHRASE:-Enchant merge to main.}"
common_dir="$(git rev-parse --git-common-dir)"
intent_file="$common_dir/MERGE_INTENT"
consume_intent=0

is_zero() {
  case "$1" in
    0000000000000000000000000000000000000000) return 0 ;;
    *) return 1 ;;
  esac
}

is_protected_branch() {
  branch="$1"
  if [ "$branch" = "$primary" ]; then
    return 0
  fi
  printf '%s\n' "$branch" | grep -Eq "$protected_regex"
}

while read -r local_ref local_sha remote_ref remote_sha
do
  case "$remote_ref" in
    refs/heads/*) branch="${remote_ref#refs/heads/}" ;;
    *) continue ;;
  esac

  if ! is_protected_branch "$branch"; then
    continue
  fi

  if ! is_zero "$remote_sha" && ! is_zero "$local_sha" && ! git merge-base --is-ancestor "$remote_sha" "$local_sha"; then
    echo "Coven Parallel Work Protocol: refusing force-push to protected branch '$branch'." >&2
    exit 1
  fi

  if [ ! -f "$intent_file" ] || [ "$(cat "$intent_file")" != "$merge_phrase" ]; then
    echo "Coven Parallel Work Protocol: protected branch '$branch' requires $intent_file containing exactly:" >&2
    echo "$merge_phrase" >&2
    exit 1
  fi
  consume_intent=1
done

if [ -x "$common_dir/hooks/pre-push.local" ]; then
  "$common_dir/hooks/pre-push.local" "$@"
fi

if [ "$consume_intent" = "1" ]; then
  rm -f "$intent_file"
fi
"#;
