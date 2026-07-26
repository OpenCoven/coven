use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, SecondsFormat};
const CLAIM_TTL_SECONDS: u64 = 60 * 60;
/// Passes allowed through the acquire loop. Each pass either settles the claim
/// or loses a race to another agent, and a loser only re-runs after the winner
/// has published its claim, so a couple of passes is plenty.
const CLAIM_ACQUIRE_ATTEMPTS: usize = 8;
/// A takeover holds its lock for two syscalls, so anything older than this was
/// abandoned by a process that died mid-takeover.
const TAKEOVER_LOCK_STALE: Duration = Duration::from_secs(30);
/// Initial claim writes normally finish within one syscall-sized critical
/// window. An incomplete file older than this was abandoned by a process that
/// died after `create_new` published the directory entry.
const INCOMPLETE_CLAIM_STALE: Duration = Duration::from_secs(30);
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
enum ClaimFileState {
    Missing,
    Incomplete,
    Parsed(Claim),
}

#[derive(Debug)]
struct Worktree {
    path: PathBuf,
    branch: Option<String>,
}

#[derive(Debug)]
struct WorktreeRow {
    branch: Option<String>,
    dirty: bool,
    claimed_by: Option<String>,
    path: PathBuf,
}

pub(crate) fn run_wt_command(
    branch: Option<&str>,
    list: bool,
    json: bool,
    doctor: bool,
    prune_merged: bool,
    prune_stale: Option<u64>,
) -> Result<()> {
    if json && !(list || doctor) {
        anyhow::bail!("--json requires --list or --doctor");
    }
    let repo = Repo::discover()?;
    match (branch, list, doctor, prune_merged, prune_stale) {
        (Some(branch), false, false, false, None) => wt_enter_or_create(&repo, branch),
        (None, true, false, false, None) => wt_list(&repo, json),
        (None, false, true, false, None) => wt_doctor(&repo, json),
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

    // Creating the claim file is the only step that decides the race, so it
    // has to be the same step that tests for a free slot: a separate
    // read-then-write lets two agents both observe "free" and both write,
    // which is how one issue ends up with two agents and two PRs.
    for _ in 0..CLAIM_ACQUIRE_ATTEMPTS {
        let now = unix_now();
        let claim = Claim {
            branch: branch.to_string(),
            agent_id: agent_id.clone(),
            acquired_at: now,
            expires_at: now + claim_ttl_seconds(),
            head: current_head().ok(),
        };

        if create_claim_exclusive(&repo, &claim)? {
            println!("claimed {branch} for {agent_id} until {}", claim.expires_at);
            return Ok(());
        }

        // Someone holds the slot. Re-read to decide whether it is ours to
        // refresh, someone else's to respect, or expired and up for grabs.
        match read_claim(&repo, branch)? {
            ClaimFileState::Missing => {
                // A takeover lock may temporarily protect a missing slot, or
                // the owner may change between the failed create and this
                // read. Retry without treating either state as free.
                std::thread::sleep(Duration::from_millis(20));
            }
            ClaimFileState::Incomplete => {
                // A fresh partial file belongs to a writer still publishing
                // its exclusive-create win. Once stale, recover it under the
                // same lock used for expired-claim takeover.
                if incomplete_claim_is_stale(&repo, branch) && try_takeover(&repo, branch, &claim)?
                {
                    println!("claimed {branch} for {agent_id} until {}", claim.expires_at);
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            ClaimFileState::Parsed(existing)
                if existing.is_active(now) && existing.agent_id != agent_id =>
            {
                anyhow::bail!(
                    "{} is already claimed by {} until {}",
                    branch,
                    existing.agent_id,
                    existing.expires_at
                );
            }
            ClaimFileState::Parsed(existing) if existing.is_active(now) => {
                // Our own live claim: extend it in place rather than
                // reporting a conflict against ourselves.
                debug_assert_eq!(existing.agent_id, agent_id);
                replace_claim(&repo, &claim)?;
                println!("claimed {branch} for {agent_id} until {}", claim.expires_at);
                return Ok(());
            }
            // An expired claim is eligible for takeover. Contend under a
            // lock: unlike a free slot, this transition cannot be arbitrated
            // by `create_new` (the file already exists), and it must never
            // leave the slot empty or a concurrent creator could slip in.
            ClaimFileState::Parsed(_) => {
                if try_takeover(&repo, branch, &claim)? {
                    println!("claimed {branch} for {agent_id} until {}", claim.expires_at);
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    anyhow::bail!("could not acquire claim for {branch}: contended by other agents, retry")
}

pub(crate) fn claim_release(branch: &str) -> Result<()> {
    let repo = Repo::discover()?;
    let agent_id = agent_id();
    match read_claim(&repo, branch)? {
        ClaimFileState::Parsed(existing) => {
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
        }
        ClaimFileState::Incomplete => {
            anyhow::bail!(
                "claim file for {branch} is incomplete; refusing to release ambiguous ownership"
            );
        }
        ClaimFileState::Missing => {
            println!("no claim for {branch}");
        }
    }
    Ok(())
}

pub(crate) fn claim_heartbeat(branch: &str) -> Result<()> {
    let repo = Repo::discover()?;
    let agent_id = agent_id();
    let now = unix_now();
    let mut claim = match read_claim(&repo, branch)? {
        ClaimFileState::Parsed(claim) => claim,
        ClaimFileState::Missing => Claim {
            branch: branch.to_string(),
            agent_id: agent_id.clone(),
            acquired_at: now,
            expires_at: now,
            head: current_head().ok(),
        },
        ClaimFileState::Incomplete => {
            anyhow::bail!(
                "claim file for {branch} is incomplete; refusing to heartbeat ambiguous ownership"
            );
        }
    };
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

pub(crate) fn claim_status(json: bool) -> Result<()> {
    let repo = Repo::discover()?;
    let claims_dir = repo.common_dir.join("agent-claims");
    let now = unix_now();
    let mut claims = if claims_dir.exists() {
        fs::read_dir(&claims_dir)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                read_claim_file(&path)
                    .ok()
                    .and_then(ClaimFileState::into_parsed)
                    .filter(|claim| claim_path(&repo, &claim.branch) == path)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    claims.sort_by(|a, b| a.branch.cmp(&b.branch));
    if json {
        println!("{}", render_claim_status_json(&claims, now)?);
        return Ok(());
    }
    if claims.is_empty() {
        println!("No claims.");
        return Ok(());
    }
    println!("{:<32} {:<20} {:<10} expires", "branch", "agent", "state");
    for claim in claims {
        println!("{}", format_claim_status_row(&claim, now));
    }
    Ok(())
}

fn format_claim_status_row(claim: &Claim, now: u64) -> String {
    let state = if claim.is_active(now) {
        "active"
    } else {
        "expired"
    };
    format!(
        "{:<32} {:<20} {:<10} {}",
        claim.branch,
        claim.agent_id,
        state,
        format_epoch_utc(claim.expires_at)
    )
}

fn render_claim_status_json(claims: &[Claim], now: u64) -> Result<String> {
    let value = serde_json::json!({
        "claims": claims
            .iter()
            .map(|claim| {
                serde_json::json!({
                    "branch": claim.branch,
                    "agent_id": claim.agent_id,
                    "state": if claim.is_active(now) { "active" } else { "expired" },
                    "acquired_at": claim.acquired_at,
                    "acquired_at_rfc3339": format_epoch_utc(claim.acquired_at),
                    "expires_at": claim.expires_at,
                    "expires_at_rfc3339": format_epoch_utc(claim.expires_at),
                    "head": claim.head,
                })
            })
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).context("failed to serialize claims as JSON")
}

/// Render an epoch-seconds timestamp as RFC 3339 UTC (e.g. `2026-01-01T00:00:00Z`).
/// Falls back to the raw epoch value if it does not fit a chrono timestamp.
fn format_epoch_utc(epoch_seconds: u64) -> String {
    i64::try_from(epoch_seconds)
        .ok()
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| epoch_seconds.to_string())
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

fn wt_list(repo: &Repo, json: bool) -> Result<()> {
    let now = unix_now();
    let mut rows = Vec::new();
    for worktree in list_worktrees()? {
        let dirty = worktree_dirty(&worktree.path)?;
        let claimed_by = worktree
            .branch
            .as_deref()
            .and_then(|branch| read_claim(repo, branch).ok())
            .and_then(ClaimFileState::into_parsed)
            .filter(|claim| claim.is_active(now))
            .map(|claim| claim.agent_id);
        rows.push(WorktreeRow {
            branch: worktree.branch,
            dirty,
            claimed_by,
            path: worktree.path,
        });
    }
    if json {
        println!("{}", render_wt_list_json(&rows)?);
        return Ok(());
    }
    println!("{:<32} {:<8} {:<20} path", "branch", "dirty", "claim");
    for row in rows {
        println!(
            "{:<32} {:<8} {:<20} {}",
            row.branch.as_deref().unwrap_or("(detached)"),
            if row.dirty { "dirty" } else { "clean" },
            row.claimed_by.as_deref().unwrap_or("-"),
            row.path.display()
        );
    }
    Ok(())
}

fn render_wt_list_json(rows: &[WorktreeRow]) -> Result<String> {
    let value = serde_json::json!({
        "worktrees": rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "branch": row.branch,
                    "dirty": row.dirty,
                    "claimed_by": row.claimed_by,
                    "path": row.path.to_string_lossy(),
                })
            })
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).context("failed to serialize worktrees as JSON")
}

/// Findings gathered by `coven wt --doctor`, rendered as prose or JSON.
struct WtDoctorReport {
    repo: PathBuf,
    worktree_root: PathBuf,
    claims_dir: PathBuf,
    /// `(hook name, managed)` for each protocol hook.
    hooks: Vec<(&'static str, bool)>,
    /// Worktrees found outside the expected root: `(path, expected root)`.
    layout_warnings: Vec<(PathBuf, PathBuf)>,
}

impl WtDoctorReport {
    fn hooks_missing(&self) -> bool {
        self.hooks.iter().any(|(_, managed)| !managed)
    }

    fn ok(&self) -> bool {
        !self.hooks_missing() && self.layout_warnings.is_empty()
    }
}

fn gather_wt_doctor(repo: &Repo) -> Result<WtDoctorReport> {
    let worktree_root = worktree_root(repo)?;
    let mut hooks = Vec::new();
    for hook in ["pre-commit", "pre-push"] {
        let path = repo.common_dir.join("hooks").join(hook);
        hooks.push((hook, hook_is_managed(&path)?));
    }
    let mut layout_warnings = Vec::new();
    for worktree in list_worktrees()? {
        if worktree.path != repo.root && !worktree.path.starts_with(&worktree_root) {
            layout_warnings.push((worktree.path, worktree_root.clone()));
        }
    }
    Ok(WtDoctorReport {
        repo: repo.root.clone(),
        worktree_root,
        claims_dir: repo.common_dir.join("agent-claims"),
        hooks,
        layout_warnings,
    })
}

fn render_wt_doctor_json(report: &WtDoctorReport) -> Result<String> {
    let mut checks = Vec::new();
    for (hook, managed) in &report.hooks {
        checks.push(if *managed {
            crate::DoctorCheck::pass(
                format!("hook:{hook}"),
                format!("managed {hook} hook installed"),
            )
        } else {
            crate::DoctorCheck::fail(
                format!("hook:{hook}"),
                format!("managed {hook} hook missing"),
                Some("install the managed hooks with `coven hooks install`".to_string()),
            )
        });
    }
    checks.push(if report.layout_warnings.is_empty() {
        crate::DoctorCheck::pass(
            "layout",
            format!("all worktrees are under {}", report.worktree_root.display()),
        )
    } else {
        let offenders = report
            .layout_warnings
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        crate::DoctorCheck::fail(
            "layout",
            format!(
                "{} worktree(s) outside {}: {}",
                report.layout_warnings.len(),
                report.worktree_root.display(),
                offenders
            ),
            None,
        )
    });

    let ok = report.ok();
    let value = serde_json::json!({
        "ok": ok,
        "blocking": !ok,
        "repo": report.repo.to_string_lossy(),
        "worktree_root": report.worktree_root.to_string_lossy(),
        "claims_dir": report.claims_dir.to_string_lossy(),
        "checks": checks,
    });
    serde_json::to_string_pretty(&value).context("failed to serialize wt doctor report as JSON")
}

fn wt_doctor(repo: &Repo, json: bool) -> Result<()> {
    let report = gather_wt_doctor(repo)?;
    if json {
        println!("{}", render_wt_doctor_json(&report)?);
        if !report.ok() {
            crate::exit_checks_failed();
        }
        return Ok(());
    }

    println!("Coven Parallel Work Protocol doctor");
    println!("repo: {}", report.repo.display());
    println!("worktree root: {}", report.worktree_root.display());
    println!("claims: {}", report.claims_dir.display());
    for (hook, managed) in &report.hooks {
        let status = if *managed { "OK" } else { "missing" };
        println!("hook {hook}: {status}");
    }
    if report.hooks_missing() {
        println!("install the managed hooks with `coven hooks install`");
    }
    for (path, expected_root) in &report.layout_warnings {
        println!(
            "layout warning: {} is outside {}",
            path.display(),
            expected_root.display()
        );
    }
    if !report.ok() {
        crate::exit_checks_failed();
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

fn render_claim(claim: &Claim) -> String {
    let mut rendered = String::new();
    rendered.push_str(&format!("branch={}\n", claim.branch));
    rendered.push_str(&format!("agent_id={}\n", claim.agent_id));
    rendered.push_str(&format!("acquired_at={}\n", claim.acquired_at));
    rendered.push_str(&format!("expires_at={}\n", claim.expires_at));
    if let Some(head) = &claim.head {
        rendered.push_str(&format!("head={head}\n"));
    }
    rendered
}

/// Create the claim file, failing rather than overwriting when one exists.
///
/// `create_new` is the whole guarantee: the kernel resolves the O_CREAT|O_EXCL
/// open, so exactly one of any number of racing agents is told it created the
/// file. Returns `false` when the slot was already taken.
fn create_claim_exclusive(repo: &Repo, claim: &Claim) -> Result<bool> {
    let claims_dir = repo.common_dir.join("agent-claims");
    fs::create_dir_all(&claims_dir)?;
    let path = claim_path(repo, &claim.branch);
    let takeover_lock = takeover_lock_path(&path);
    if takeover_lock.try_exists().with_context(|| {
        format!(
            "failed to inspect takeover lock {}",
            takeover_lock.display()
        )
    })? {
        if path_is_stale(&takeover_lock, TAKEOVER_LOCK_STALE) {
            let _ = fs::remove_file(&takeover_lock);
        } else {
            // A releaser can remove an expired claim while its winner holds
            // the takeover lock. Do not let that temporary empty slot admit
            // a second winner before the takeover publishes its replacement.
            return Ok(false);
        }
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(render_claim(claim).as_bytes())
                .with_context(|| format!("failed to write claim {}", path.display()))?;
            Ok(true)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to create claim {}", path.display())),
    }
}

/// Overwrite an existing claim through a temp file plus rename, so a reader
/// never observes a half-written claim and two concurrent writers cannot
/// interleave their lines into one file.
fn replace_claim(repo: &Repo, claim: &Claim) -> Result<()> {
    let claims_dir = repo.common_dir.join("agent-claims");
    fs::create_dir_all(&claims_dir)?;
    let path = claim_path(repo, &claim.branch);
    let staging = temp_claim_path(&path, "write");
    fs::write(&staging, render_claim(claim))
        .with_context(|| format!("failed to stage claim {}", staging.display()))?;
    replace_claim_file(&staging, &path)
        .inspect_err(|_| {
            let _ = fs::remove_file(&staging);
        })
        .with_context(|| format!("failed to publish claim {}", path.display()))?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_claim_file(staging: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(staging, path)
}

#[cfg(windows)]
fn replace_claim_file(staging: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "Kernel32")]
    extern "system" {
        #[link_name = "MoveFileExW"]
        fn move_file_ex_w(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let existing: Vec<u16> = staging
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let new: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        move_file_ex_w(
            existing.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Sibling path for staged and cleared claim files. Kept in the same directory
/// so the rename stays within one filesystem. The `@` prefix is reserved for
/// protocol internals because `branch_slug` never preserves `@`, so no valid
/// claim filename can collide with staging or lock state.
fn temp_claim_path(path: &Path, kind: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "claim".to_string());
    let unique = format!(
        "@{name}.{kind}.{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
    );
    path.with_file_name(unique)
}

/// Take over an expired claim, serialised so exactly one agent can do it.
///
/// Replacing an existing claim cannot be arbitrated by `create_new`, so this
/// is the one transition that needs a lock. The replacement is published with
/// an atomic rename rather than a delete-then-create, so the slot is never
/// observed empty: agents that are not holding the lock keep failing their own
/// `create_new`, re-read, and see the new owner.
///
/// Returns `true` when `claim` was published.
fn try_takeover(repo: &Repo, branch: &str, claim: &Claim) -> Result<bool> {
    let Some(_lock) = TakeoverLock::try_acquire(repo, branch)? else {
        return Ok(false);
    };

    // Re-read under the lock: the state that justified the takeover may have
    // changed while we were contending for it.
    let now = unix_now();
    match read_claim(repo, branch)? {
        ClaimFileState::Parsed(existing)
            if existing.is_active(now) && existing.agent_id != claim.agent_id =>
        {
            Ok(false)
        }
        ClaimFileState::Parsed(_) => {
            replace_claim(repo, claim)?;
            Ok(true)
        }
        ClaimFileState::Incomplete if incomplete_claim_is_stale(repo, branch) => {
            replace_claim(repo, claim)?;
            Ok(true)
        }
        // A concurrent release may remove the expired claim after we decide
        // to take it over. With no file left to replace, drop the lock and let
        // the next free-slot acquisition use `create_new`; recreating it here
        // would race that path and could report two winners.
        ClaimFileState::Missing | ClaimFileState::Incomplete => Ok(false),
    }
}

/// Exclusive lock guarding the expired-claim takeover, released on drop.
struct TakeoverLock {
    path: PathBuf,
}

impl TakeoverLock {
    fn try_acquire(repo: &Repo, branch: &str) -> Result<Option<Self>> {
        let path = takeover_lock_path(&claim_path(repo, branch));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(Some(Self { path })),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // The critical section is two syscalls long, so a lock this
                // old belongs to a process that died holding it. Breaking it
                // only returns us to contending for the same lock.
                if path_is_stale(&path, TAKEOVER_LOCK_STALE) {
                    let _ = fs::remove_file(&path);
                }
                Ok(None)
            }
            Err(err) => {
                Err(err).with_context(|| format!("failed to lock takeover {}", path.display()))
            }
        }
    }
}

impl Drop for TakeoverLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn path_is_stale(path: &Path, stale_after: Duration) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|meta| meta.modified()) else {
        return false;
    };
    // A clock skew that puts the lock in the future reads as not-stale, which
    // is the safe direction: we wait instead of breaking someone's lock.
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age > stale_after)
        .unwrap_or(false)
}

fn takeover_lock_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "claim".to_string());
    path.with_file_name(format!("@{name}.takeover"))
}

fn write_claim(repo: &Repo, claim: &Claim) -> Result<()> {
    replace_claim(repo, claim)
}

fn incomplete_claim_is_stale(repo: &Repo, branch: &str) -> bool {
    path_is_stale(&claim_path(repo, branch), INCOMPLETE_CLAIM_STALE)
}

fn read_claim(repo: &Repo, branch: &str) -> Result<ClaimFileState> {
    read_claim_file(&claim_path(repo, branch))
}

fn read_claim_file(path: &Path) -> Result<ClaimFileState> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ClaimFileState::Missing);
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read claim {}", path.display()));
        }
    };
    let value = |key: &str| -> Option<String> {
        contents.lines().find_map(|line| {
            line.split_once('=')
                .filter(|(found, _)| *found == key)
                .map(|(_, value)| value.to_string())
        })
    };
    let Some(branch) = value("branch").filter(|value| !value.trim().is_empty()) else {
        return Ok(ClaimFileState::Incomplete);
    };
    let Some(agent_id) = value("agent_id").filter(|value| !value.trim().is_empty()) else {
        return Ok(ClaimFileState::Incomplete);
    };
    let Some(acquired_at) = value("acquired_at").and_then(|value| value.parse().ok()) else {
        return Ok(ClaimFileState::Incomplete);
    };
    let Some(expires_at) = value("expires_at").and_then(|value| value.parse().ok()) else {
        return Ok(ClaimFileState::Incomplete);
    };
    Ok(ClaimFileState::Parsed(Claim {
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

impl ClaimFileState {
    fn into_parsed(self) -> Option<Claim> {
        match self {
            Self::Parsed(claim) => Some(claim),
            Self::Missing | Self::Incomplete => None,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-01-01T00:00:00Z
    const NEW_YEAR_2026_EPOCH: u64 = 1_767_225_600;

    fn sample_claim() -> Claim {
        Claim {
            branch: "feat/sample".to_string(),
            agent_id: "agent-a".to_string(),
            acquired_at: NEW_YEAR_2026_EPOCH,
            expires_at: NEW_YEAR_2026_EPOCH + 3600,
            head: Some("abc123".to_string()),
        }
    }

    #[test]
    fn format_epoch_utc_renders_rfc3339() {
        assert_eq!(
            format_epoch_utc(NEW_YEAR_2026_EPOCH),
            "2026-01-01T00:00:00Z"
        );
    }

    #[test]
    fn claim_status_row_renders_human_readable_expiry() {
        let claim = sample_claim();
        let row = format_claim_status_row(&claim, NEW_YEAR_2026_EPOCH);

        assert!(row.contains("feat/sample"));
        assert!(row.contains("agent-a"));
        assert!(row.contains("active"));
        assert!(row.contains("2026-01-01T01:00:00Z"));
        assert!(!row.contains(&claim.expires_at.to_string()));
    }

    #[test]
    fn claim_status_row_reports_expired_claims() {
        let claim = sample_claim();
        let row = format_claim_status_row(&claim, claim.expires_at + 1);

        assert!(row.contains("expired"));
    }

    #[test]
    fn claim_status_json_includes_epoch_and_rfc3339_fields() {
        let claims = vec![sample_claim()];
        let body = render_claim_status_json(&claims, NEW_YEAR_2026_EPOCH).expect("render");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        let claim = &value["claims"][0];
        assert_eq!(claim["branch"], "feat/sample");
        assert_eq!(claim["agent_id"], "agent-a");
        assert_eq!(claim["state"], "active");
        assert_eq!(claim["acquired_at"], NEW_YEAR_2026_EPOCH);
        assert_eq!(claim["acquired_at_rfc3339"], "2026-01-01T00:00:00Z");
        assert_eq!(claim["expires_at"], NEW_YEAR_2026_EPOCH + 3600);
        assert_eq!(claim["expires_at_rfc3339"], "2026-01-01T01:00:00Z");
        assert_eq!(claim["head"], "abc123");
    }

    #[test]
    fn claim_status_json_renders_empty_claims_list() {
        let body = render_claim_status_json(&[], NEW_YEAR_2026_EPOCH).expect("render");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        assert_eq!(value["claims"], serde_json::json!([]));
    }

    #[test]
    fn replace_claim_overwrites_existing_claim() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repo = Repo {
            root: temp.path().to_path_buf(),
            common_dir: temp.path().to_path_buf(),
        };
        let original = sample_claim();
        assert!(create_claim_exclusive(&repo, &original).expect("create original claim"));

        let replacement = Claim {
            agent_id: "agent-b".to_string(),
            expires_at: original.expires_at + 3600,
            ..original.clone()
        };
        replace_claim(&repo, &replacement).expect("replace existing claim");

        let stored = read_claim(&repo, &replacement.branch)
            .expect("read replacement")
            .into_parsed()
            .expect("claim exists");
        assert_eq!(stored.agent_id, replacement.agent_id);
        assert_eq!(stored.expires_at, replacement.expires_at);
    }

    #[test]
    fn takeover_does_not_recreate_a_released_claim() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repo = Repo {
            root: temp.path().to_path_buf(),
            common_dir: temp.path().to_path_buf(),
        };
        fs::create_dir_all(repo.common_dir.join("agent-claims")).expect("claims dir");
        let claim = sample_claim();

        assert!(!try_takeover(&repo, &claim.branch, &claim).expect("try takeover"));
        assert!(!claim_path(&repo, &claim.branch).exists());
    }

    #[test]
    fn internal_claim_paths_use_unrepresentable_prefix() {
        let claim = PathBuf::from("feature-demo");
        let staged = temp_claim_path(&claim, "write");
        let lock = takeover_lock_path(&claim);

        for internal in [staged, lock] {
            let name = internal.file_name().unwrap().to_string_lossy();
            assert!(name.starts_with('@'));
            assert_ne!(branch_slug(&name), name);
        }
    }

    #[test]
    fn wt_list_json_includes_expected_fields() {
        let rows = vec![
            WorktreeRow {
                branch: Some("feat/sample".to_string()),
                dirty: true,
                claimed_by: Some("agent-a".to_string()),
                path: PathBuf::from("/tmp/repo.wt/feat-sample"),
            },
            WorktreeRow {
                branch: None,
                dirty: false,
                claimed_by: None,
                path: PathBuf::from("/tmp/repo"),
            },
        ];
        let body = render_wt_list_json(&rows).expect("render");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        let worktrees = value["worktrees"].as_array().expect("worktrees array");
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0]["branch"], "feat/sample");
        assert_eq!(worktrees[0]["dirty"], true);
        assert_eq!(worktrees[0]["claimed_by"], "agent-a");
        assert_eq!(worktrees[0]["path"], "/tmp/repo.wt/feat-sample");
        assert_eq!(worktrees[1]["branch"], serde_json::Value::Null);
        assert_eq!(worktrees[1]["dirty"], false);
        assert_eq!(worktrees[1]["claimed_by"], serde_json::Value::Null);
    }

    fn sample_wt_doctor_report() -> WtDoctorReport {
        WtDoctorReport {
            repo: PathBuf::from("/tmp/repo"),
            worktree_root: PathBuf::from("/tmp/repo.wt"),
            claims_dir: PathBuf::from("/tmp/repo/.git/agent-claims"),
            hooks: vec![("pre-commit", true), ("pre-push", true)],
            layout_warnings: Vec::new(),
        }
    }

    #[test]
    fn wt_doctor_json_reports_healthy_repo() {
        let report = sample_wt_doctor_report();
        let body = render_wt_doctor_json(&report).expect("render");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        assert_eq!(value["ok"], true);
        assert_eq!(value["blocking"], false);
        assert_eq!(value["repo"], "/tmp/repo");
        assert_eq!(value["worktree_root"], "/tmp/repo.wt");
        assert_eq!(value["claims_dir"], "/tmp/repo/.git/agent-claims");

        let checks = value["checks"].as_array().expect("checks array");
        assert_eq!(checks.len(), 3);
        assert_eq!(checks[0]["id"], "hook:pre-commit");
        assert_eq!(checks[0]["status"], "pass");
        assert_eq!(checks[1]["id"], "hook:pre-push");
        assert_eq!(checks[1]["status"], "pass");
        assert_eq!(checks[2]["id"], "layout");
        assert_eq!(checks[2]["status"], "pass");
    }

    #[test]
    fn wt_doctor_json_flags_missing_hook_with_install_hint() {
        let mut report = sample_wt_doctor_report();
        report.hooks = vec![("pre-commit", true), ("pre-push", false)];
        let body = render_wt_doctor_json(&report).expect("render");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        assert_eq!(value["ok"], false);
        assert_eq!(value["blocking"], true);
        let checks = value["checks"].as_array().expect("checks array");
        assert_eq!(checks[1]["id"], "hook:pre-push");
        assert_eq!(checks[1]["status"], "fail");
        assert_eq!(
            checks[1]["hint"],
            "install the managed hooks with `coven hooks install`"
        );
    }

    #[test]
    fn wt_doctor_json_flags_layout_offenders() {
        let mut report = sample_wt_doctor_report();
        report.layout_warnings = vec![(
            PathBuf::from("/elsewhere/stray"),
            PathBuf::from("/tmp/repo.wt"),
        )];
        let body = render_wt_doctor_json(&report).expect("render");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        assert_eq!(value["ok"], false);
        assert_eq!(value["blocking"], true);
        let layout = &value["checks"][2];
        assert_eq!(layout["id"], "layout");
        assert_eq!(layout["status"], "fail");
        let message = layout["message"].as_str().expect("message");
        assert!(message.contains("/elsewhere/stray"));
        assert!(message.contains("/tmp/repo.wt"));
    }

    #[test]
    fn wt_json_flag_requires_list_or_doctor() {
        let err = run_wt_command(Some("feat/x"), false, true, false, false, None)
            .expect_err("must reject --json without --list/--doctor");
        assert!(err.to_string().contains("--json requires"));
    }
}
