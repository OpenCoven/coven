// ── eval_loop_cli.rs ─────────────────────────────────────────────────────────
//
// `coven eval-loop …` — the harness-agnostic driver for the eval-loop skill
// (spec: 2026-10-06-eval-loop-v2-design.md).
//
// Division of labour: everything deterministic lives here (split, noise
// floor, headroom, accept rule, leak check, TSV row, lock). Everything that
// needs a model (propose, run a case, judge) lives in the calling harness and
// talks to this module only through files under `<workspace>/evals/<track>/`
// and `<workspace>/eval-loop/`.
//
// Safety contract: this module never mutates git. It runs only read-only git
// commands (`rev-parse`, `status --porcelain`, `branch --show-current`, `diff`).
// A REVERT is recorded with `revert_required: true` and the harness performs
// the reset; the next `begin` verifies HEAD against the recorded baseline.
//
// Exit codes (surfaced by main.rs): 0 ok · 3 refused · 1 error.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::eval_loop::{
    self, parse_results_tsv, validate_track, EVALS_DIR, EVAL_LOOP_DIR, NOISE_FILE, RESULTS_TSV,
    RUN_LOCK_FILE, RUN_SPEC_FILE, SKIPS_FILE,
};

pub const EXIT_REFUSED: i32 = 3;

const CASES_FILE: &str = "cases.jsonl";
const JUDGE_FILE: &str = "judge.md";
const CONFIG_FILE: &str = "config.json";
const BASELINE_FILE: &str = "baseline.json";
const NOISE_DIR: &str = "noise";
const ITERATIONS_DIR: &str = "iterations";
const CONTEXT_FILE: &str = "context.json";
const PROPOSAL_FILE: &str = "proposal.json";
const DECISION_FILE: &str = "decision.json";
const FINALIZATION_FILE: &str = "finalization.json";
const SCORING_FILE: &str = "scoring.json";
const SCORES_DIR: &str = "scores";
const TRACES_DIR: &str = "memory/eval-loop-traces";
const MAX_TRAIN_FAILURES_IN_CONTEXT: usize = 20;
const MAX_FILES_PER_PROPOSAL: usize = 3;
const STALL_REVERTS: usize = 3;
const NOISE_AGING_ACCEPTS: u32 = 5;
const LEAK_MIN_CHARS: usize = 40;
const LEAK_WINDOW_CHARS: usize = 60;
const LEAK_STRIDE_CHARS: usize = 20;

pub const TSV_HEADER_V2: &str = "timestamp\ttrack\titeration\tchange_summary\tmetric_before\tmetric_after\tdelta\toutcome\tbranch\tproposer_reasoning\tfailure_modes\tmetric_train_before\tmetric_train_after\tnoise_floor\teval_set_hash\tdecision_reason\trun_id";

// ── Outcome ──────────────────────────────────────────────────────────────────

/// A command either produced a JSON result or refused with a stable reason.
/// Refusals are a normal, expected outcome (exit 3), never an error.
#[derive(Debug)]
pub enum Outcome {
    Ok(Value),
    Refused { reason: &'static str, detail: Value },
}

impl Outcome {
    pub fn to_json(&self) -> Value {
        match self {
            Outcome::Ok(v) => v.clone(),
            Outcome::Refused { reason, detail } => json!({ "refused": reason, "detail": detail }),
        }
    }
    pub fn is_refused(&self) -> bool {
        matches!(self, Outcome::Refused { .. })
    }
    #[cfg(test)]
    fn refused_reason(&self) -> Option<&'static str> {
        match self {
            Outcome::Refused { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

// ── On-disk documents ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackConfig {
    pub salt: String,
    pub test_pct: u8,
    pub min_test: usize,
    pub min_train: usize,
    pub saturation: f64,
    pub noise_max_floor: f64,
    pub noise_max_age_days: i64,
}

impl TrackConfig {
    fn new(test_pct: u8) -> Self {
        Self {
            salt: Uuid::new_v4().simple().to_string(),
            test_pct,
            min_test: 10,
            min_train: 20,
            saturation: 0.95,
            noise_max_floor: 0.15,
            noise_max_age_days: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Noise {
    pub eval_set_hash: String,
    #[serde(default)]
    pub measurement_hash: String,
    pub runs: u32,
    pub mean: f64,
    pub stddev: f64,
    pub floor: f64,
    pub measured_at: String,
    #[serde(default)]
    pub accepts_since: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub eval_set_hash: String,
    #[serde(default)]
    pub measurement_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub test_mean: f64,
    pub train_mean: f64,
    /// Per-case train scores so `begin` can surface failures without re-scoring.
    /// Test per-case scores are deliberately NOT stored here (information barrier).
    pub train_cases: BTreeMap<String, f64>,
    pub measured_at: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    pub id: String,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_buckets: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_multi_file: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoisePlan {
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    result: Option<CalibrationResult>,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    pub generation: String,
    pub runs: u32,
    pub eval_set_hash: String,
    #[serde(default)]
    pub measurement_hash: String,
    pub test_ids: Vec<String>,
    pub train_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalibrationResult {
    noise: Noise,
    baseline: Baseline,
    pass_means: Vec<f64>,
}

fn calibration_pending(l: &Layout) -> Result<bool> {
    Ok(
        read_json_opt::<NoisePlan>(&l.noise_dir().join("plan.json"))?
            .is_some_and(|p| p.result.is_some() && !p.completed),
    )
}

fn publish_calibration(l: &Layout, plan: &mut NoisePlan) -> Result<Outcome> {
    let result = plan
        .result
        .as_ref()
        .context("calibration has no recorded result")?;
    if !plan.completed {
        write_json(&l.noise(), &result.noise)?;
        write_json(&l.baseline(), &result.baseline)?;
        plan.completed = true;
        write_json(&l.noise_dir().join("plan.json"), plan)?;
    }
    let result = plan
        .result
        .as_ref()
        .context("calibration has no recorded result")?;
    Ok(Outcome::Ok(
        json!({ "track": l.track, "noise": result.noise, "baseline": result.baseline, "pass_means": result.pass_means }),
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainFailure {
    pub id: String,
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPaths {
    pub proposal: PathBuf,
    pub scores_train: PathBuf,
    pub scores_test: PathBuf,
    pub judge: PathBuf,
}

/// What the proposer is allowed to see. Train data only — never a test row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationContext {
    pub run_id: String,
    pub track: String,
    pub iteration: u32,
    pub eval_set_hash: String,
    #[serde(default)]
    pub measurement_hash: String,
    pub baseline_test_mean: f64,
    pub baseline_train_mean: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_commit: Option<String>,
    /// HEAD at `begin`; the harness resets to this on REVERT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    pub noise_floor: f64,
    pub request_buckets: bool,
    pub train_failures: Vec<TrainFailure>,
    pub git: bool,
    pub paths: ContextPaths,
    pub warnings: Vec<String>,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionMetrics {
    pub test_before: f64,
    pub test_after: f64,
    pub train_before: f64,
    pub train_after: f64,
    pub delta_test: f64,
    pub delta_train: f64,
    pub noise_floor: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub run_id: String,
    pub track: String,
    pub iteration: u32,
    pub outcome: String,
    pub reason: String,
    pub metrics: DecisionMetrics,
    pub revert_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    /// "clean" | "partial" (some strings too short to check) | "leaked"
    pub leak_check: String,
    pub decided_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<String>,
}

// ── Workspace resolution ─────────────────────────────────────────────────────

/// `--workspace <path>` wins; `--familiar <id>` resolves through familiars.toml.
pub fn resolve_workspace(workspace: Option<PathBuf>, familiar: Option<String>) -> Result<PathBuf> {
    match (workspace, familiar) {
        (Some(ws), _) => Ok(ws),
        (None, Some(id)) => {
            let home = crate::paths::coven_home_dir()?;
            Ok(eval_loop::familiar_workspace(&home, &id))
        }
        (None, None) => bail!("pass --workspace <path> or --familiar <id>"),
    }
}

// ── Paths ────────────────────────────────────────────────────────────────────

struct Layout {
    ws: PathBuf,
    track: String,
}

impl Layout {
    fn new(ws: &Path, track: &str) -> Result<Self> {
        validate_track(track)?;
        Ok(Self {
            ws: ws.to_path_buf(),
            track: track.to_string(),
        })
    }
    fn evals(&self) -> PathBuf {
        self.ws.join(EVALS_DIR).join(&self.track)
    }
    fn cases(&self) -> PathBuf {
        self.evals().join(CASES_FILE)
    }
    fn judge(&self) -> PathBuf {
        self.evals().join(JUDGE_FILE)
    }
    fn config(&self) -> PathBuf {
        self.evals().join(CONFIG_FILE)
    }
    fn noise(&self) -> PathBuf {
        self.evals().join(NOISE_FILE)
    }
    fn baseline(&self) -> PathBuf {
        self.evals().join(BASELINE_FILE)
    }
    fn eval_loop(&self) -> PathBuf {
        self.ws.join(EVAL_LOOP_DIR)
    }
    fn noise_dir(&self) -> PathBuf {
        self.eval_loop().join(NOISE_DIR).join(&self.track)
    }
    fn iterations(&self) -> PathBuf {
        self.eval_loop().join(ITERATIONS_DIR).join(&self.track)
    }
    fn iteration(&self, n: u32) -> PathBuf {
        self.iterations().join(n.to_string())
    }
    fn skips(&self) -> PathBuf {
        self.eval_loop().join(SKIPS_FILE)
    }
    fn run_lock(&self) -> PathBuf {
        self.eval_loop().join(RUN_LOCK_FILE)
    }
    fn run_json(&self) -> PathBuf {
        self.eval_loop().join(RUN_SPEC_FILE)
    }
    fn results(&self) -> PathBuf {
        self.ws.join(RESULTS_TSV)
    }
}

// ── Small helpers ────────────────────────────────────────────────────────────

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn read_json_opt<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    read_json(path).map(Some)
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let parent = path.parent().context("output has no parent")?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, &serde_json::to_string_pretty(value)?)
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: T = serde_json::from_str(line).with_context(|| {
            format!(
                "{}:{} is not valid JSON for this file",
                path.display(),
                i + 1
            )
        })?;
        out.push(v);
    }
    Ok(out)
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

fn sanitize_tsv(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// Sample standard deviation (K−1). One or zero samples ⇒ 0.
fn sample_stddev(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64;
    var.sqrt()
}

pub fn noise_floor_from_stddev(stddev: f64) -> f64 {
    (2.0 * stddev).max(0.01)
}

fn eval_set_hash(cases_bytes: &[u8]) -> String {
    blake3::hash(cases_bytes).to_hex().to_string()
}

/// Deterministic split: `test iff blake3(salt ‖ id)[0..8] % 100 < test_pct`.
pub fn is_test_case(salt: &str, id: &str, test_pct: u8) -> bool {
    let mut hasher = blake3::Hasher::new();
    hasher.update(salt.as_bytes());
    hasher.update(id.as_bytes());
    let bytes = hasher.finalize();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes.as_bytes()[0..8]);
    (u64::from_le_bytes(buf) % 100) < test_pct as u64
}

// ── Git (read-only) ──────────────────────────────────────────────────────────

fn git_available(ws: &Path) -> bool {
    ws.join(".git").exists()
}

fn git_out(ws: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(ws)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_head(ws: &Path) -> Option<String> {
    if !git_available(ws) {
        return None;
    }
    git_out(ws, &["rev-parse", "HEAD"]).ok()
}

fn git_branch(ws: &Path) -> String {
    if !git_available(ws) {
        return String::new();
    }
    git_out(ws, &["branch", "--show-current"]).unwrap_or_default()
}

/// Paths the loop itself writes. They are expected to change between
/// iterations and must not count as a dirty tree.
const BOOKKEEPING_EXCLUDES: [&str; 4] = [
    ":(exclude)evals",
    ":(exclude)eval-loop",
    ":(exclude)results.tsv",
    ":(exclude)memory/eval-loop-traces",
];

fn git_implementation_changed(ws: &Path, commit: &str) -> Result<bool> {
    let mut args = vec!["diff", "--name-only", commit, "HEAD", "--", "."];
    args.extend(BOOKKEEPING_EXCLUDES);
    Ok(!git_out(ws, &args)?.is_empty())
}

fn git_dirty(ws: &Path) -> Result<bool> {
    let mut args = vec!["status", "--porcelain", "--untracked-files=all", "--", "."];
    args.extend(BOOKKEEPING_EXCLUDES);
    Ok(!git_out(ws, &args)?.is_empty())
}

/// `git merge-base --is-ancestor <commit> HEAD` — read-only.
fn git_is_ancestor(ws: &Path, commit: &str) -> Result<bool> {
    let out = Command::new("git")
        .args(["merge-base", "--is-ancestor", commit, "HEAD"])
        .current_dir(ws)
        .output()
        .context("failed to run git merge-base")?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "git merge-base failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

// ── Loading & validation ─────────────────────────────────────────────────────

struct LoadedSet {
    config: TrackConfig,
    cases: Vec<Case>,
    hash: String,
    measurement_hash: String,
    train: Vec<Case>,
    test: Vec<Case>,
}

/// Length-prefix each source so split and grader changes invalidate measurements
/// while eval_set_hash keeps its existing cases-only API meaning.
fn measurement_hash(l: &Layout) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    for path in [l.cases(), l.config(), l.judge()] {
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    Ok(hash.finalize().to_hex().to_string())
}

fn noise_scores(l: &Layout, plan: &NoisePlan) -> Result<PathBuf> {
    let generation = Uuid::parse_str(&plan.generation)
        .context("noise plan requires a fresh calibration; run noise start")?;
    Ok(l.noise_dir().join(generation.to_string()))
}

fn load_set(l: &Layout) -> Result<std::result::Result<LoadedSet, Outcome>> {
    let config: TrackConfig = match read_json_opt(&l.config())? {
        Some(c) => c,
        None => {
            return Ok(Err(Outcome::Refused {
                reason: "not_initialized",
                detail: json!({ "hint": format!("run `coven eval-loop init --track {}`", l.track), "config": l.config() }),
            }))
        }
    };
    let bytes = match fs::read(l.cases()) {
        Ok(b) => b,
        Err(_) => {
            return Ok(Err(Outcome::Refused {
                reason: "insufficient_cases",
                detail: json!({ "cases": 0, "path": l.cases() }),
            }))
        }
    };
    let hash = eval_set_hash(&bytes);
    let measurement_hash = measurement_hash(l)?;
    let cases: Vec<Case> = read_jsonl(&l.cases())?;

    let mut seen = BTreeSet::new();
    for c in &cases {
        if !seen.insert(c.id.clone()) {
            bail!("duplicate_case_id: {}", c.id);
        }
        if c.expected.is_none() && c.rubric.is_none() {
            bail!(
                "cases_invalid: case `{}` needs `expected` or `rubric`",
                c.id
            );
        }
    }

    let (test, train): (Vec<Case>, Vec<Case>) = cases
        .iter()
        .cloned()
        .partition(|c| is_test_case(&config.salt, &c.id, config.test_pct));

    if test.len() < config.min_test || train.len() < config.min_train {
        return Ok(Err(Outcome::Refused {
            reason: "insufficient_cases",
            detail: json!({
                "test": test.len(), "min_test": config.min_test,
                "train": train.len(), "min_train": config.min_train,
                "total": cases.len(),
            }),
        }));
    }
    Ok(Ok(LoadedSet {
        config,
        cases,
        hash,
        measurement_hash,
        train,
        test,
    }))
}

fn validate_scores(
    scores: &[Score],
    expected_ids: &[String],
    label: &str,
) -> Result<BTreeMap<String, f64>> {
    let mut map = BTreeMap::new();
    for s in scores {
        if !(0.0..=1.0).contains(&s.score) || s.score.is_nan() {
            bail!(
                "score_out_of_range: {label} case `{}` has score {}",
                s.id,
                s.score
            );
        }
        map.insert(s.id.clone(), s.score);
    }
    let missing: Vec<&String> = expected_ids
        .iter()
        .filter(|id| !map.contains_key(*id))
        .collect();
    if !missing.is_empty() {
        bail!(
            "scores_incomplete: {label} is missing {} case(s): {:?}",
            missing.len(),
            missing
        );
    }
    Ok(map)
}

fn mean_of(map: &BTreeMap<String, f64>, ids: &[String]) -> f64 {
    let xs: Vec<f64> = ids.iter().filter_map(|id| map.get(id).copied()).collect();
    mean(&xs)
}

fn ids(cases: &[Case]) -> Vec<String> {
    cases.iter().map(|c| c.id.clone()).collect()
}

// ── Iteration bookkeeping ────────────────────────────────────────────────────

fn iteration_dirs(l: &Layout) -> Vec<u32> {
    let Ok(rd) = fs::read_dir(l.iterations()) else {
        return Vec::new();
    };
    let mut ns: Vec<u32> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .collect();
    ns.sort_unstable();
    ns
}

fn track_rows(l: &Layout) -> Vec<eval_loop::LoopIterationDto> {
    fs::read_to_string(l.results())
        .map(|raw| parse_results_tsv(&raw))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.track == l.track)
        .collect()
}

/// The iteration that has a context.json but no decision.json yet.
fn active_iteration(l: &Layout) -> Option<u32> {
    iteration_dirs(l).into_iter().rev().find(|n| {
        let d = l.iteration(*n);
        d.join(CONTEXT_FILE).exists()
            && (!d.join(DECISION_FILE).exists() || d.join(FINALIZATION_FILE).exists())
    })
}

fn last_decision(l: &Layout) -> Result<Option<(u32, Decision)>> {
    for n in iteration_dirs(l).into_iter().rev() {
        let p = l.iteration(n).join(DECISION_FILE);
        if p.exists() && !l.iteration(n).join(FINALIZATION_FILE).exists() {
            return Ok(Some((n, read_json(&p)?)));
        }
    }
    Ok(None)
}

fn trailing_reverts(rows: &[eval_loop::LoopIterationDto]) -> usize {
    rows.iter()
        .rev()
        .take_while(|r| r.outcome == "REVERT")
        .count()
}

fn record_skip(l: &Layout, reason: &str, detail: &Value, run_id: Option<&str>) -> Result<()> {
    let line = json!({
        "timestamp": now(), "track": l.track, "reason": reason, "detail": detail, "runId": run_id,
    });
    append_line(&l.skips(), &line.to_string())
}

// ── Commands ─────────────────────────────────────────────────────────────────

pub fn init(ws: &Path, track: &str, test_pct: u8) -> Result<Outcome> {
    let _mutation = eval_loop::mutation_lock(ws)?;
    if !(1..=90).contains(&test_pct) {
        bail!("--test-pct must be between 1 and 90");
    }
    let l = Layout::new(ws, track)?;
    fs::create_dir_all(l.evals())?;
    let mut created = Vec::new();
    if !l.config().exists() {
        write_json(&l.config(), &TrackConfig::new(test_pct))?;
        created.push(l.config());
    }
    if !l.cases().exists() {
        fs::write(l.cases(), "")?;
        created.push(l.cases());
    }
    if !l.judge().exists() {
        fs::write(l.judge(), JUDGE_TEMPLATE)?;
        created.push(l.judge());
    }
    let cfg: TrackConfig = read_json(&l.config())?;
    let min_total = ((cfg.min_test as f64) / (cfg.test_pct as f64 / 100.0)).ceil() as usize;
    Ok(Outcome::Ok(json!({
        "track": track,
        "created": created,
        "config": cfg,
        "hint": format!(
            "append cases to {} as JSONL {{id, source, input, expected|rubric}}; with test_pct={} and min_test={} you need roughly ≥ {} cases, then run `noise start`",
            l.cases().display(), cfg.test_pct, cfg.min_test, min_total.max(cfg.min_test + cfg.min_train)
        ),
    })))
}

pub fn noise_start(ws: &Path, track: &str, runs: u32) -> Result<Outcome> {
    if runs == 0 {
        bail!("--runs must be ≥ 1");
    }
    let _mutation = eval_loop::mutation_lock(ws)?;
    let l = Layout::new(ws, track)?;
    if calibration_pending(&l)? {
        return Ok(Outcome::Refused {
            reason: "calibration_pending",
            detail: json!({ "hint": "retry noise finalize to finish the recorded calibration" }),
        });
    }
    if let Some(iteration) = active_iteration(&l) {
        return Ok(Outcome::Refused {
            reason: "iteration_open",
            detail: json!({ "iteration": iteration }),
        });
    }
    let set = match load_set(&l)? {
        Ok(s) => s,
        Err(refused) => {
            if let Outcome::Refused { reason, detail } = &refused {
                record_skip(&l, reason, detail, None)?;
            }
            return Ok(refused);
        }
    };
    let commit = git_head(ws);
    if git_available(ws) && (commit.is_none() || git_dirty(ws)?) {
        return Ok(Outcome::Refused {
            reason: "dirty_baseline",
            detail: json!({ "hint": "commit the implementation before calibration" }),
        });
    }
    let plan = NoisePlan {
        commit,
        result: None,
        completed: false,
        generation: Uuid::new_v4().to_string(),
        runs,
        eval_set_hash: set.hash.clone(),
        measurement_hash: set.measurement_hash.clone(),
        test_ids: ids(&set.test),
        train_ids: ids(&set.train),
        created_at: now(),
    };
    fs::create_dir_all(l.noise_dir())?;
    write_json(&l.noise_dir().join("plan.json"), &plan)?;
    let score_dir = noise_scores(&l, &plan)?;
    fs::create_dir_all(&score_dir)?;
    let write_to: Vec<PathBuf> = (1..=runs)
        .map(|k| score_dir.join(format!("pass-{k}.jsonl")))
        .collect();
    Ok(Outcome::Ok(json!({
        "track": track,
        "runs": runs,
        "eval_set_hash": set.hash,
        "instructions": "Score the TEST cases `runs` times with NO change applied, writing one {id, score, notes?} JSONL row per case to each pass file; score the TRAIN cases once into train.jsonl; then run `noise finalize`.",
        "test_cases": set.test,
        "train_cases": set.train,
        "write_test_passes_to": write_to,
        "write_train_to": noise_scores(&l, &plan)?.join("train.jsonl"),
        "judge": l.judge(),
    })))
}

pub fn noise_finalize(ws: &Path, track: &str) -> Result<Outcome> {
    let _mutation = eval_loop::mutation_lock(ws)?;
    let l = Layout::new(ws, track)?;
    if let Some(iteration) = active_iteration(&l) {
        return Ok(Outcome::Refused {
            reason: "iteration_open",
            detail: json!({ "iteration": iteration }),
        });
    }
    let mut plan: NoisePlan = read_json_opt(&l.noise_dir().join("plan.json"))?
        .ok_or_else(|| anyhow!("no noise plan; run `noise start` first"))?;
    // Replay the recorded publication; completed generations return their
    // receipt without overwriting a newer accepted baseline.
    if plan.result.is_some() {
        return publish_calibration(&l, &mut plan);
    }
    if git_available(ws) != plan.commit.is_some()
        || (plan.commit.is_some() && git_dirty(ws)?)
        || plan
            .commit
            .as_deref()
            .map(|commit| git_implementation_changed(ws, commit))
            .transpose()?
            .unwrap_or(false)
    {
        return Ok(Outcome::Refused {
            reason: "dirty_baseline",
            detail: json!({ "hint": "implementation changed during calibration; rerun noise start" }),
        });
    }
    let set = match load_set(&l)? {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    if set.hash != plan.eval_set_hash || set.measurement_hash != plan.measurement_hash {
        return Ok(Outcome::Refused {
            reason: "noise_stale",
            detail: json!({ "planned_hash": plan.eval_set_hash, "current_hash": set.hash, "hint": "evaluation cases, split, or judge changed since `noise start`; rerun it" }),
        });
    }
    let mut pass_means = Vec::with_capacity(plan.runs as usize);
    for k in 1..=plan.runs {
        let p = noise_scores(&l, &plan)?.join(format!("pass-{k}.jsonl"));
        let scores: Vec<Score> = read_jsonl(&p)?;
        let map = validate_scores(&scores, &plan.test_ids, &format!("pass-{k}"))?;
        pass_means.push(mean_of(&map, &plan.test_ids));
    }
    let train_scores: Vec<Score> = read_jsonl(&noise_scores(&l, &plan)?.join("train.jsonl"))?;
    let train_map = validate_scores(&train_scores, &plan.train_ids, "train")?;

    let stddev = sample_stddev(&pass_means);
    let noise = Noise {
        eval_set_hash: set.hash.clone(),
        measurement_hash: set.measurement_hash.clone(),
        runs: plan.runs,
        mean: mean(&pass_means),
        stddev,
        floor: noise_floor_from_stddev(stddev),
        measured_at: now(),
        accepts_since: 0,
    };
    let baseline = Baseline {
        eval_set_hash: set.hash.clone(),
        measurement_hash: set.measurement_hash.clone(),
        commit: plan.commit.clone(),
        test_mean: noise.mean,
        train_mean: mean_of(&train_map, &plan.train_ids),
        train_cases: train_map,
        measured_at: now(),
        source: "noise".to_string(),
    };
    plan.result = Some(CalibrationResult {
        noise,
        baseline,
        pass_means,
    });
    write_json(&l.noise_dir().join("plan.json"), &plan)?;
    publish_calibration(&l, &mut plan)
}

pub fn begin(ws: &Path, track: &str, from_run_json: bool, ack_unreviewed: bool) -> Result<Outcome> {
    let _mutation = eval_loop::mutation_lock(ws)?;
    let l = Layout::new(ws, track)?;
    if calibration_pending(&l)? {
        return Ok(Outcome::Refused {
            reason: "calibration_pending",
            detail: json!({ "hint": "retry noise finalize to finish the recorded calibration" }),
        });
    }
    let adopted_run_id = if from_run_json {
        let spec = read_json_opt::<eval_loop::RunSpec>(&l.run_json())?;
        match spec {
            Some(spec) if spec.track == track && run_owned_by(&l, &spec.run_id)? => {
                Some(spec.run_id)
            }
            _ => {
                return Ok(Outcome::Refused {
                    reason: "run_ownership_mismatch",
                    detail: json!({ "hint": "run.json and run.lock must identify this track's queued run" }),
                })
            }
        }
    } else {
        None
    };
    let refuse = |l: &Layout, reason: &'static str, detail: Value| -> Result<Outcome> {
        record_skip(l, reason, &detail, adopted_run_id.as_deref())?;
        Ok(Outcome::Refused { reason, detail })
    };

    // 1. eval set
    let set = match load_set(&l)? {
        Ok(s) => s,
        Err(Outcome::Refused { reason, detail }) => return refuse(&l, reason, detail),
        Err(other) => return Ok(other),
    };
    let mut warnings = Vec::new();
    let synthetic = set
        .cases
        .iter()
        .filter(|c| c.source.as_deref() == Some("synthetic"))
        .count();
    if synthetic * 2 > set.cases.len() {
        warnings.push(format!(
            "{synthetic}/{} cases are synthetic (> 50%)",
            set.cases.len()
        ));
    }

    // 2. noise
    let Some(noise) = read_json_opt::<Noise>(&l.noise())? else {
        return refuse(
            &l,
            "noise_missing",
            json!({ "hint": "run `noise start` then `noise finalize`" }),
        );
    };
    if noise.eval_set_hash != set.hash || noise.measurement_hash != set.measurement_hash {
        return refuse(
            &l,
            "noise_stale",
            json!({ "noise_hash": noise.eval_set_hash, "current_hash": set.hash }),
        );
    }
    let age_days = DateTime::parse_from_rfc3339(&noise.measured_at)
        .map(|dt| {
            Utc::now()
                .signed_duration_since(dt.with_timezone(&Utc))
                .num_days()
        })
        .unwrap_or(i64::MAX);
    if age_days > set.config.noise_max_age_days {
        return refuse(
            &l,
            "noise_stale",
            json!({ "age_days": age_days, "max_age_days": set.config.noise_max_age_days }),
        );
    }
    if noise.floor > set.config.noise_max_floor {
        return refuse(
            &l,
            "noise_too_high",
            json!({ "floor": noise.floor, "max_floor": set.config.noise_max_floor, "hint": "add cases or increase --runs" }),
        );
    }
    if noise.accepts_since >= NOISE_AGING_ACCEPTS {
        warnings.push(format!(
            "noise measured {} accepts ago; consider re-estimating",
            noise.accepts_since
        ));
    }

    // 3. baseline
    let Some(baseline) = read_json_opt::<Baseline>(&l.baseline())? else {
        return refuse(
            &l,
            "baseline_missing",
            json!({ "hint": "run `noise finalize`" }),
        );
    };
    if baseline.eval_set_hash != set.hash || baseline.measurement_hash != set.measurement_hash {
        return refuse(
            &l,
            "noise_stale",
            json!({ "baseline_hash": baseline.eval_set_hash, "current_hash": set.hash }),
        );
    }

    // 4. headroom
    if baseline.test_mean >= set.config.saturation {
        return refuse(
            &l,
            "saturated",
            json!({ "test_mean": baseline.test_mean, "saturation": set.config.saturation }),
        );
    }

    // 5. open iteration / human gate
    if let Some(n) = active_iteration(&l) {
        return refuse(
            &l,
            "iteration_open",
            json!({ "iteration": n, "hint": "run `decide` for it first" }),
        );
    }
    if let Some((n, prev)) = last_decision(&l)? {
        if prev.acknowledged_at.is_none() && !ack_unreviewed {
            return refuse(
                &l,
                "trace_unreviewed",
                json!({ "iteration": n, "hint": "read the trace, then `coven eval-loop ack` (or pass --ack-unreviewed)" }),
            );
        }
    }

    // 6. git baseline — ancestry, not SHA equality, so bookkeeping commits are fine.
    let git = git_available(ws);
    let head = git_head(ws);
    if git {
        if git_dirty(ws)? {
            return refuse(
                &l,
                "dirty_baseline",
                json!({ "hint": "working tree has uncommitted changes outside the loop's bookkeeping paths" }),
            );
        }
        if let Some((n, prev)) = last_decision(&l)? {
            if let Some(prev_head) = &prev.head_commit {
                let moved = prev.base_commit.as_deref() != Some(prev_head.as_str());
                if prev.revert_required && moved && git_is_ancestor(ws, prev_head)? {
                    return refuse(
                        &l,
                        "dirty_baseline",
                        json!({
                            "iteration": n, "reverted_commit": prev_head, "base_commit": prev.base_commit,
                            "hint": "the REVERTed commit is still in HEAD's history; reset to base_commit first",
                        }),
                    );
                }
                if !prev.revert_required && !git_is_ancestor(ws, prev_head)? {
                    return refuse(
                        &l,
                        "dirty_baseline",
                        json!({
                            "iteration": n, "accepted_commit": prev_head,
                            "hint": "the ACCEPTed commit is no longer in HEAD's history",
                        }),
                    );
                }
            }
        }
        let Some(commit) = baseline.commit.as_deref() else {
            return refuse(
                &l,
                "dirty_baseline",
                json!({ "hint": "calibrate the current Git implementation first" }),
            );
        };
        if git_implementation_changed(ws, commit)? {
            return refuse(
                &l,
                "dirty_baseline",
                json!({ "baseline_commit": commit, "hint": "implementation changed since calibration; recalibrate before begin" }),
            );
        }
    }

    // 7. lock
    let run_id = if l.run_lock().exists() {
        match &adopted_run_id {
            Some(id) => id.clone(),
            None => {
                let lock = eval_loop::eval_loop_lock(ws);
                return refuse(
                    &l,
                    "run_in_progress",
                    json!({ "lock": lock, "hint": "pass --from-run-json to adopt a daemon-enqueued run, or clear a stale lock" }),
                );
            }
        }
    } else {
        let spec = eval_loop::RunSpec {
            run_id: adopted_run_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            familiar_id: ws
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            track: track.to_string(),
            requested_at: now(),
        };
        fs::create_dir_all(l.eval_loop())?;
        write_json(&l.run_json(), &spec)?;
        fs::write(l.run_lock(), &spec.run_id)?;
        spec.run_id
    };

    // 8. context
    let rows = track_rows(&l);
    let iteration = rows
        .iter()
        .map(|r| r.iteration)
        .max()
        .unwrap_or(0)
        .max(iteration_dirs(&l).last().copied().unwrap_or(0))
        + 1;
    let request_buckets = trailing_reverts(&rows) >= STALL_REVERTS;
    let mut train_failures: Vec<TrainFailure> = set
        .train
        .iter()
        .filter_map(|c| {
            let score = *baseline.train_cases.get(&c.id)?;
            (score < 1.0).then(|| TrainFailure {
                id: c.id.clone(),
                input: c.input.clone(),
                expected: c.expected.clone(),
                rubric: c.rubric.clone(),
                score,
            })
        })
        .collect();
    train_failures.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    train_failures.truncate(MAX_TRAIN_FAILURES_IN_CONTEXT);

    let dir = l.iteration(iteration);
    fs::create_dir_all(dir.join(SCORES_DIR))?;
    let context = IterationContext {
        run_id,
        track: track.to_string(),
        iteration,
        eval_set_hash: set.hash,
        measurement_hash: set.measurement_hash,
        baseline_test_mean: baseline.test_mean,
        baseline_train_mean: baseline.train_mean,
        baseline_commit: baseline.commit.clone(),
        base_commit: head,
        noise_floor: noise.floor,
        request_buckets,
        train_failures,
        git,
        paths: ContextPaths {
            proposal: dir.join(PROPOSAL_FILE),
            scores_train: dir.join(SCORES_DIR).join("train.jsonl"),
            scores_test: dir.join(SCORES_DIR).join("test.jsonl"),
            judge: l.judge(),
        },
        warnings,
        started_at: now(),
    };
    write_json(&dir.join(CONTEXT_FILE), &context)?;
    Ok(Outcome::Ok(serde_json::to_value(&context)?))
}

pub fn cases(ws: &Path, track: &str, split: &str) -> Result<Outcome> {
    let _mutation = eval_loop::mutation_lock(ws)?;
    let l = Layout::new(ws, track)?;
    let Some(n) = active_iteration(&l) else {
        return Ok(Outcome::Refused {
            reason: "no_active_iteration",
            detail: json!({ "hint": "run `begin` first" }),
        });
    };
    let dir = l.iteration(n);
    let ctx: IterationContext = read_json(&dir.join(CONTEXT_FILE))?;
    if dir.join(FINALIZATION_FILE).exists() {
        return Ok(Outcome::Refused {
            reason: "finalization_pending",
            detail: json!({ "hint": "retry decide to finish the recorded decision" }),
        });
    }
    if !run_owned_by(&l, &ctx.run_id)? {
        return Ok(Outcome::Refused {
            reason: "run_ownership_mismatch",
            detail: json!({ "run_id": ctx.run_id }),
        });
    }
    let set = match load_set(&l)? {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    if set.hash != ctx.eval_set_hash || set.measurement_hash != ctx.measurement_hash {
        return Ok(Outcome::Refused {
            reason: "noise_stale",
            detail: json!({ "hint": "evaluation inputs changed after begin" }),
        });
    }
    let selected = match split {
        "train" => set.train,
        "test" => {
            if !l.iteration(n).join(PROPOSAL_FILE).exists() {
                return Ok(Outcome::Refused {
                    reason: "proposal_missing",
                    detail: json!({ "iteration": n, "hint": "commit your change and write proposal.json before scoring the test split", "path": l.iteration(n).join(PROPOSAL_FILE) }),
                });
            }
            if let Some(refused) = check_scoring(&l, &ctx, true)? {
                return Ok(refused);
            }
            set.test
        }
        other => bail!("--split must be `train` or `test`, got `{other}`"),
    };
    Ok(Outcome::Ok(
        json!({ "iteration": n, "split": split, "cases": selected }),
    ))
}

fn leak_windows(s: &str) -> Option<Vec<String>> {
    let norm: Vec<char> = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect();
    if norm.len() < LEAK_MIN_CHARS {
        return None;
    }
    if norm.len() <= LEAK_WINDOW_CHARS {
        return Some(vec![norm.iter().collect()]);
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start + LEAK_WINDOW_CHARS <= norm.len() {
        out.push(norm[start..start + LEAK_WINDOW_CHARS].iter().collect());
        start += LEAK_STRIDE_CHARS;
    }
    Some(out)
}

/// Returns ("clean"|"partial"|"leaked", offending case id if any).
pub fn leak_check(diff_text: &str, test_cases: &[Case]) -> (String, Option<String>) {
    let hay = diff_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut partial = false;
    for c in test_cases {
        for field in [Some(&c.input), c.expected.as_ref()].into_iter().flatten() {
            match leak_windows(field) {
                None => partial = true,
                Some(windows) => {
                    if windows.iter().any(|w| hay.contains(w.as_str())) {
                        return ("leaked".to_string(), Some(c.id.clone()));
                    }
                }
            }
        }
    }
    (
        (if partial { "partial" } else { "clean" }).to_string(),
        None,
    )
}

fn proposal_diff_text(ws: &Path, ctx: &IterationContext, proposal: &Proposal) -> Result<String> {
    if ctx.git {
        if let Some(base) = &ctx.base_commit {
            return git_out(ws, &["diff", base, "HEAD"]);
        }
        return git_out(ws, &["show", "HEAD"]);
    }
    let mut text = String::new();
    for f in &proposal.files {
        let p = ws.join(f);
        if let Ok(c) = fs::read_to_string(&p) {
            text.push_str(&c);
            text.push('\n');
        }
    }
    Ok(text)
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct ScoringBinding {
    commit: String,
    proposal_hash: String,
    measurement_hash: String,
}

/// Pin the committed proposal before exposing held-out cases. A changed input
/// needs a new iteration rather than silently reusing existing score files.
fn check_scoring(l: &Layout, ctx: &IterationContext, bind: bool) -> Result<Option<Outcome>> {
    if !ctx.git {
        return Ok(None);
    }
    if git_dirty(&l.ws)? {
        return Ok(Some(Outcome::Refused {
            reason: "dirty_proposal",
            detail: json!({ "hint": "commit the proposal before scoring or deciding" }),
        }));
    }
    let dir = l.iteration(ctx.iteration);
    let current = ScoringBinding {
        commit: git_out(&l.ws, &["rev-parse", "HEAD"])?,
        proposal_hash: blake3::hash(&fs::read(dir.join(PROPOSAL_FILE))?)
            .to_hex()
            .to_string(),
        measurement_hash: measurement_hash(l)?,
    };
    let path = dir.join(SCORING_FILE);
    if let Some(recorded) = read_json_opt::<ScoringBinding>(&path)? {
        if current != recorded {
            return Ok(Some(Outcome::Refused {
                reason: "proposal_changed",
                detail: json!({ "scored_commit": recorded.commit, "current_commit": current.commit }),
            }));
        }
    } else if bind {
        write_json(&path, &current)?;
    } else {
        return Ok(Some(Outcome::Refused {
            reason: "scoring_not_started",
            detail: json!({ "hint": "run cases --split test on the committed proposal before scoring" }),
        }));
    }
    Ok(None)
}

fn run_owned_by(l: &Layout, run_id: &str) -> Result<bool> {
    let lock_id = match fs::read_to_string(l.run_lock()) {
        Ok(id) => id,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    let spec = read_json_opt::<eval_loop::RunSpec>(&l.run_json())?;
    Ok(lock_id.trim() == run_id
        && spec.is_some_and(|spec| spec.run_id == run_id && spec.track == l.track))
}

pub fn decide(ws: &Path, track: &str) -> Result<Outcome> {
    let _mutation = eval_loop::mutation_lock(ws)?;
    let l = Layout::new(ws, track)?;
    let Some(n) = active_iteration(&l) else {
        return Ok(Outcome::Refused {
            reason: "no_active_iteration",
            detail: json!({ "hint": "run `begin` first" }),
        });
    };
    let dir = l.iteration(n);
    let ctx: IterationContext = read_json(&dir.join(CONTEXT_FILE))?;
    if let Some(pending) = read_json_opt::<Finalization>(&dir.join(FINALIZATION_FILE))? {
        return finish_decision(&l, &ctx, &pending);
    }
    if !run_owned_by(&l, &ctx.run_id)? {
        return Ok(Outcome::Refused {
            reason: "run_ownership_mismatch",
            detail: json!({ "run_id": ctx.run_id, "hint": "the active run has been cleared or replaced" }),
        });
    }
    let proposal: Proposal = read_json_opt(&dir.join(PROPOSAL_FILE))?.ok_or_else(|| {
        anyhow!(
            "proposal_missing: write {}",
            dir.join(PROPOSAL_FILE).display()
        )
    })?;
    if let Some(refused) = check_scoring(&l, &ctx, false)? {
        return Ok(refused);
    }
    let set = match load_set(&l)? {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    if set.hash != ctx.eval_set_hash || set.measurement_hash != ctx.measurement_hash {
        return Ok(Outcome::Refused {
            reason: "noise_stale",
            detail: json!({ "hint": "evaluation cases, split, or judge changed mid-iteration", "context_hash": ctx.eval_set_hash, "current_hash": set.hash }),
        });
    }
    let baseline: Baseline = read_json(&l.baseline())?;
    let mut noise: Noise = read_json(&l.noise())?;

    let train_scores: Vec<Score> = read_jsonl(&ctx.paths.scores_train)?;
    let test_scores: Vec<Score> = read_jsonl(&ctx.paths.scores_test)?;
    let train_ids = ids(&set.train);
    let test_ids = ids(&set.test);
    let train_map = validate_scores(&train_scores, &train_ids, "train")?;
    let test_map = validate_scores(&test_scores, &test_ids, "test")?;

    let metrics = DecisionMetrics {
        test_before: baseline.test_mean,
        test_after: mean_of(&test_map, &test_ids),
        train_before: baseline.train_mean,
        train_after: mean_of(&train_map, &train_ids),
        delta_test: 0.0,
        delta_train: 0.0,
        noise_floor: noise.floor,
    };
    let metrics = DecisionMetrics {
        delta_test: metrics.test_after - metrics.test_before,
        delta_train: metrics.train_after - metrics.train_before,
        ..metrics
    };

    let diff_text = proposal_diff_text(ws, &ctx, &proposal)?;
    let (leak_state, leaked_id) = leak_check(&diff_text, &set.test);
    let floor = noise.floor;

    let (outcome, reason) = if leak_state == "leaked" {
        ("REVERT", "leak_detected")
    } else if proposal.files.len() > MAX_FILES_PER_PROPOSAL
        && !proposal.allow_multi_file.unwrap_or(false)
    {
        ("REVERT", "too_many_files")
    } else if ctx.request_buckets
        && proposal
            .failure_buckets
            .as_ref()
            .map(|b| b.is_empty())
            .unwrap_or(true)
    {
        ("REVERT", "buckets_missing")
    } else if metrics.delta_test > floor && metrics.delta_train >= 0.0 {
        ("ACCEPT", "accepted")
    } else if metrics.delta_train > floor && metrics.delta_test <= floor {
        ("REVERT", "overfit")
    } else if metrics.delta_test < -floor {
        ("REVERT", "regression")
    } else {
        ("REVERT", "noise")
    };

    let head = git_head(ws);
    let decision = Decision {
        run_id: ctx.run_id.clone(),
        track: track.to_string(),
        iteration: n,
        outcome: outcome.to_string(),
        reason: reason.to_string(),
        metrics: metrics.clone(),
        revert_required: outcome == "REVERT",
        base_commit: ctx.base_commit.clone(),
        head_commit: head.clone(),
        leak_check: leak_state.clone(),
        decided_at: now(),
        acknowledged_at: None,
    };
    // results.tsv row (v2, 17 columns)
    let failure_modes = match (&proposal.failure_buckets, outcome) {
        (Some(b), _) if !b.is_empty() => b.join("; "),
        (_, "REVERT") => leaked_id
            .map(|id| format!("{reason} ({id})"))
            .unwrap_or_else(|| reason.to_string()),
        _ => String::new(),
    };
    let row = [
        decision.decided_at.clone(),
        track.to_string(),
        n.to_string(),
        sanitize_tsv(&proposal.summary),
        format!("{:.4}", metrics.test_before),
        format!("{:.4}", metrics.test_after),
        format!("{:.4}", metrics.delta_test),
        outcome.to_string(),
        sanitize_tsv(&git_branch(ws)),
        sanitize_tsv(proposal.reasoning.as_deref().unwrap_or("")),
        sanitize_tsv(&failure_modes),
        format!("{:.4}", metrics.train_before),
        format!("{:.4}", metrics.train_after),
        format!("{:.4}", floor),
        set.hash.clone(),
        reason.to_string(),
        ctx.run_id.clone(),
    ]
    .join("\t");
    // Persist an immutable plan before publishing any output. Replays use
    // absolute values, so a partially applied ACCEPT cannot count twice.
    let (new_baseline, new_noise) = if outcome == "ACCEPT" {
        let new_baseline = Baseline {
            eval_set_hash: set.hash.clone(),
            measurement_hash: set.measurement_hash.clone(),
            commit: head.clone(),
            test_mean: metrics.test_after,
            train_mean: metrics.train_after,
            train_cases: train_map,
            measured_at: now(),
            source: format!("iteration:{n}"),
        };
        noise.accepts_since += 1;
        (Some(new_baseline), Some(noise))
    } else {
        (None, None)
    };
    let trace = render_trace(&l, &decision, &set.test, &test_map, &test_scores);
    let pending = Finalization {
        decision,
        row,
        baseline: new_baseline,
        noise: new_noise,
        trace,
    };
    write_json(&dir.join(FINALIZATION_FILE), &pending)?;
    finish_decision(&l, &ctx, &pending)
}

#[derive(Serialize, Deserialize)]
struct Finalization {
    decision: Decision,
    row: String,
    baseline: Option<Baseline>,
    noise: Option<Noise>,
    trace: String,
}

fn finish_decision(l: &Layout, ctx: &IterationContext, pending: &Finalization) -> Result<Outcome> {
    let d = &pending.decision;
    if d.run_id != ctx.run_id || d.track != l.track || d.iteration != ctx.iteration {
        bail!("finalization does not match iteration context");
    }
    let dir = l.iteration(d.iteration);
    let decision_path = dir.join(DECISION_FILE);
    let trace_path = l.ws.join(TRACES_DIR).join(format!(
        "{}-{}-{}.md",
        DateTime::parse_from_rfc3339(&d.decided_at)?.format("%Y-%m-%d"),
        l.track,
        d.iteration
    ));
    if !decision_path.exists() {
        if !run_owned_by(l, &ctx.run_id)? {
            return Ok(Outcome::Refused {
                reason: "run_ownership_mismatch",
                detail: json!({ "run_id": ctx.run_id }),
            });
        }
        let mut results = match fs::read_to_string(l.results()) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => format!("{TSV_HEADER_V2}\n"),
            Err(err) => return Err(err.into()),
        };
        let present = results.lines().any(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            fields.len() >= 17 && fields[1] == d.track && fields[16] == d.run_id
        });
        if !present {
            if !results.ends_with('\n') {
                results.push('\n');
            }
            results.push_str(&pending.row);
            results.push('\n');
            write_atomic(&l.results(), &results)?;
        }
        if let Some(baseline) = &pending.baseline {
            write_json(&l.baseline(), baseline)?;
        }
        if let Some(noise) = &pending.noise {
            write_json(&l.noise(), noise)?;
        }
        write_atomic(&trace_path, &pending.trace)?;
        // Completion is visible only once every required artifact exists.
        write_json(&decision_path, d)?;
    }
    // A clear/re-enqueue between retries must not release the replacement run.
    // Removing the spec first permits recovery after either cleanup operation.
    let lock_id = match fs::read_to_string(l.run_lock()) {
        Ok(id) => Some(id),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    if lock_id.as_deref().map(str::trim) == Some(d.run_id.as_str()) {
        if let Some(spec) = read_json_opt::<eval_loop::RunSpec>(&l.run_json())? {
            if spec.run_id != d.run_id || spec.track != l.track {
                bail!("run spec changed during finalization cleanup");
            }
            fs::remove_file(l.run_json())?;
        }
        fs::remove_file(l.run_lock())?;
    }
    fs::remove_file(dir.join(FINALIZATION_FILE))?;
    Ok(Outcome::Ok(json!({
        "decision": d,
        "trace": trace_path,
        "next": if d.revert_required {
            format!("REVERT: reset the workspace to base_commit {} (harness-owned), then `coven eval-loop ack`", d.base_commit.as_deref().unwrap_or("<none>"))
        } else {
            "ACCEPT: keep the commit; read the trace and run `coven eval-loop ack`".to_string()
        },
    })))
}

fn render_trace(
    l: &Layout,
    d: &Decision,
    test_cases: &[Case],
    test_map: &BTreeMap<String, f64>,
    test_scores: &[Score],
) -> String {
    let notes: BTreeMap<&str, &str> = test_scores
        .iter()
        .filter_map(|s| s.notes.as_deref().map(|n| (s.id.as_str(), n)))
        .collect();
    let mut by_score: Vec<&Case> = test_cases.iter().collect();
    by_score.sort_by(|a, b| {
        test_map
            .get(&a.id)
            .partial_cmp(&test_map.get(&b.id))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    let mut sample: Vec<&Case> = by_score.iter().take(3).copied().collect();
    // two "random" picks, deterministic on run_id so the trace is reproducible
    let mut rest: Vec<&Case> = by_score.iter().skip(3).copied().collect();
    rest.sort_by_key(|c| {
        blake3::hash(format!("{}{}", d.run_id, c.id).as_bytes())
            .to_hex()
            .to_string()
    });
    sample.extend(rest.into_iter().take(2));

    let mut md = String::new();
    md.push_str(&format!(
        "# eval-loop trace — {} iteration {}\n\n",
        l.track, d.iteration
    ));
    md.push_str(&format!("- outcome: **{}** ({})\n- Δtest {:+.4} vs floor {:.4} · Δtrain {:+.4}\n- test {:.4} → {:.4} · train {:.4} → {:.4}\n- leak check: {}\n- run: `{}`\n\n",
        d.outcome, d.reason, d.metrics.delta_test, d.metrics.noise_floor, d.metrics.delta_train,
        d.metrics.test_before, d.metrics.test_after, d.metrics.train_before, d.metrics.train_after,
        d.leak_check, d.run_id));
    md.push_str("## Sampled test transcripts (3 lowest, 2 pseudo-random)\n\nRead these before believing the grader. Then run `coven eval-loop ack`.\n\n");
    for c in sample {
        let score = test_map.get(&c.id).copied().unwrap_or(0.0);
        let input: String = c.input.chars().take(400).collect();
        md.push_str(&format!(
            "### {} — score {:.2}\n\n**input:** {}{}\n\n**judge notes:** {}\n\n",
            c.id,
            score,
            input,
            if c.input.chars().count() > 400 {
                "…"
            } else {
                ""
            },
            notes.get(c.id.as_str()).unwrap_or(&"(none)")
        ));
    }
    md
}

pub fn ack(ws: &Path, track: &str) -> Result<Outcome> {
    let _mutation = eval_loop::mutation_lock(ws)?;
    let l = Layout::new(ws, track)?;
    let Some((n, mut d)) = last_decision(&l)? else {
        return Ok(Outcome::Refused {
            reason: "nothing_to_ack",
            detail: json!({}),
        });
    };
    if d.acknowledged_at.is_none() {
        d.acknowledged_at = Some(now());
        write_json(&l.iteration(n).join(DECISION_FILE), &d)?;
    }
    Ok(Outcome::Ok(
        json!({ "iteration": n, "acknowledged_at": d.acknowledged_at }),
    ))
}

pub fn status(ws: &Path, track: &str, last: usize) -> Result<Outcome> {
    let l = Layout::new(ws, track)?;
    let rows = track_rows(&l);
    let shown: Vec<_> = rows.iter().rev().take(last.max(1)).cloned().collect();
    let skips = eval_loop::read_skips(ws)?
        .into_iter()
        .filter(|s| s.track == track)
        .count();
    Ok(Outcome::Ok(json!({
        "track": track,
        "workspace": ws,
        "iterations_total": rows.len(),
        "accepted": rows.iter().filter(|r| r.outcome == "ACCEPT").count(),
        "reverted": rows.iter().filter(|r| r.outcome == "REVERT").count(),
        "skipped": skips,
        "trailing_reverts": trailing_reverts(&rows),
        "active_iteration": active_iteration(&l),
        "lock": eval_loop::eval_loop_lock(ws),
        "noise": read_json_opt::<Noise>(&l.noise())?,
        "baseline": read_json_opt::<Baseline>(&l.baseline())?.map(|b| json!({
            "eval_set_hash": b.eval_set_hash, "commit": b.commit, "test_mean": b.test_mean,
            "train_mean": b.train_mean, "measured_at": b.measured_at, "source": b.source,
        })),
        "last": shown,
    })))
}

const JUDGE_TEMPLATE: &str = "# Judge rubric\n\nYou are grading ONE case. Return a single JSON object: {\"id\": \"<case id>\", \"score\": <0.0–1.0>, \"notes\": \"<one sentence>\"}.\n\n- 1.0: fully meets `expected` / the rubric.\n- 0.5: partially correct or correct with a material omission.\n- 0.0: wrong, missing, or violates a hard constraint.\n\nGrade the output, not the effort. Do not reward length. If `expected` is given, compare semantically, not by exact string match, unless the case says otherwise.\n";

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str, expected: &str) -> String {
        json!({ "id": id, "source": "manual", "input": format!("input for {id} — a sentence long enough to matter for leak checks"), "expected": expected }).to_string()
    }

    /// Workspace with `count` cases; expected strings are long so the leak check can see them.
    fn fixture(count: usize) -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_path_buf();
        let out = init(&ws, "prompt", 30).unwrap();
        assert!(!out.is_refused());
        let l = Layout::new(&ws, "prompt").unwrap();
        // Keep fixture splits reproducible: a random salt can put fewer than the
        // required ten of sixty cases in the holdout set. This yields 19 test / 41 train.
        let mut config: TrackConfig = read_json(&l.config()).unwrap();
        config.salt = "eval-loop-test-fixture".into();
        write_json(&l.config(), &config).unwrap();
        let lines: Vec<String> = (0..count)
            .map(|i| {
                case(
                    &format!("case-{i:03}"),
                    &format!("the expected answer for case {i:03} is forty-plus characters long"),
                )
            })
            .collect();
        fs::write(l.cases(), lines.join("\n") + "\n").unwrap();
        (dir, l)
    }

    fn write_scores(path: &Path, ids: &[String], score: impl Fn(&str) -> f64) {
        let lines: Vec<String> = ids
            .iter()
            .map(|id| json!({ "id": id, "score": score(id), "notes": "n" }).to_string())
            .collect();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    /// Run noise start+finalize with constant pass scores → floor 0.01 and baseline means `base`.
    fn establish_baseline(l: &Layout, base_test: f64, base_train: f64) -> NoisePlan {
        let out = noise_start(&l.ws, &l.track, 3).unwrap();
        assert!(!out.is_refused(), "{:?}", out);
        let plan: NoisePlan = read_json(&l.noise_dir().join("plan.json")).unwrap();
        for k in 1..=3 {
            write_scores(
                &noise_scores(l, &plan)
                    .unwrap()
                    .join(format!("pass-{k}.jsonl")),
                &plan.test_ids,
                |_| base_test,
            );
        }
        write_scores(
            &noise_scores(l, &plan).unwrap().join("train.jsonl"),
            &plan.train_ids,
            |_| base_train,
        );
        let out = noise_finalize(&l.ws, &l.track).unwrap();
        assert!(!out.is_refused(), "{:?}", out);
        plan
    }

    fn begin_ok(l: &Layout) -> IterationContext {
        let out = begin(&l.ws, &l.track, false, true).unwrap();
        match out {
            Outcome::Ok(v) => serde_json::from_value(v).unwrap(),
            other => panic!("begin refused: {:?}", other),
        }
    }

    fn propose(ctx: &IterationContext, summary: &str, files: &[&str]) {
        write_json(
            &ctx.paths.proposal,
            &Proposal {
                summary: summary.into(),
                reasoning: Some("because".into()),
                files: files.iter().map(|s| s.to_string()).collect(),
                failure_buckets: None,
                allow_multi_file: None,
                judge: None,
            },
        )
        .unwrap();
    }

    fn decide_reason(l: &Layout) -> (String, String) {
        let out = decide(&l.ws, &l.track).unwrap();
        let Outcome::Ok(v) = out else {
            panic!("decide refused: {:?}", out)
        };
        let d: Decision = serde_json::from_value(v["decision"].clone()).unwrap();
        (d.outcome, d.reason)
    }

    #[test]
    fn begin_rejects_another_tracks_enqueued_run() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.5, 0.5);
        fs::create_dir_all(l.eval_loop()).unwrap();
        let spec = eval_loop::RunSpec {
            run_id: "other-run".into(),
            familiar_id: "fixture".into(),
            track: "other-track".into(),
            requested_at: now(),
        };
        write_json(&l.run_json(), &spec).unwrap();
        fs::write(l.run_lock(), &spec.run_id).unwrap();
        let out = begin(&l.ws, &l.track, true, true).unwrap();
        assert!(out.is_refused(), "must not adopt another track: {out:?}");
        assert_eq!(fs::read_to_string(l.run_lock()).unwrap(), "other-run");
        assert!(active_iteration(&l).is_none());
    }

    #[test]
    fn decide_preserves_a_replacement_runs_ownership() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.5, 0.5);
        let ctx = begin_ok(&l);
        propose(&ctx, "improve", &["prompt.md"]);
        let set = load_set(&l).unwrap().unwrap();
        write_scores(&ctx.paths.scores_train, &ids(&set.train), |_| 0.7);
        write_scores(&ctx.paths.scores_test, &ids(&set.test), |_| 0.7);
        let spec = eval_loop::RunSpec {
            run_id: "replacement".into(),
            familiar_id: "fixture".into(),
            track: "other-track".into(),
            requested_at: now(),
        };
        write_json(&l.run_json(), &spec).unwrap();
        fs::write(l.run_lock(), &spec.run_id).unwrap();
        let out = decide(&l.ws, &l.track).unwrap();
        assert!(
            out.is_refused(),
            "stale iteration must not finalize: {out:?}"
        );
        assert_eq!(fs::read_to_string(l.run_lock()).unwrap(), "replacement");
        assert_eq!(
            read_json::<eval_loop::RunSpec>(&l.run_json())
                .unwrap()
                .run_id,
            "replacement"
        );
        assert!(!l.iteration(ctx.iteration).join(DECISION_FILE).exists());
    }

    #[test]
    fn decide_recovers_interrupted_finalization_exactly_once() {
        for fail_trace in [false, true] {
            let (_d, l) = fixture(60);
            establish_baseline(&l, 0.5, 0.5);
            let ctx = begin_ok(&l);
            propose(&ctx, "improve", &["prompt.md"]);
            let set = load_set(&l).unwrap().unwrap();
            write_scores(&ctx.paths.scores_train, &ids(&set.train), |_| 0.7);
            write_scores(&ctx.paths.scores_test, &ids(&set.test), |_| 0.7);
            let obstacle = if fail_trace {
                l.ws.join(TRACES_DIR)
            } else {
                l.results()
            };
            if fail_trace {
                fs::create_dir_all(obstacle.parent().unwrap()).unwrap();
                fs::write(&obstacle, "injected failure").unwrap();
            } else {
                fs::create_dir(&obstacle).unwrap();
            }
            assert!(decide(&l.ws, &l.track).is_err());
            // Once publication starts, retries must use the saved decision even
            // when a harness overwrites its input artifacts in the meantime.
            fs::write(&ctx.paths.scores_test, "invalid replacement scores").unwrap();
            fs::write(&ctx.paths.proposal, "invalid replacement proposal").unwrap();
            if fail_trace {
                fs::remove_file(&obstacle).unwrap();
            } else {
                fs::remove_dir(&obstacle).unwrap();
            }
            let out = decide(&l.ws, &l.track).unwrap();
            assert!(
                !out.is_refused(),
                "retry must finish interrupted decision: {out:?}"
            );
            let rows = track_rows(&l);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].outcome, "ACCEPT");
            let noise: Noise = read_json(&l.noise()).unwrap();
            assert_eq!(noise.accepts_since, 1);
            let baseline: Baseline = read_json(&l.baseline()).unwrap();
            assert!((baseline.test_mean - 0.7).abs() < 1e-9);
            assert!(!l.run_lock().exists());
            assert!(!l.run_json().exists());
        }
    }

    #[test]
    fn concurrent_begins_publish_only_one_context() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.5, 0.5);
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let attempts: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        begin(&l.ws, &l.track, false, true).unwrap()
                    })
                })
                .collect();
            attempts
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|r| !r.is_refused()).count(), 1);
        assert_eq!(iteration_dirs(&l), vec![1]);
        let ctx: IterationContext = read_json(&l.iteration(1).join(CONTEXT_FILE)).unwrap();
        assert!(run_owned_by(&l, &ctx.run_id).unwrap());
    }

    #[test]
    fn noise_commands_cannot_overwrite_an_open_iterations_baseline() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.5, 0.5);
        begin_ok(&l);
        let baseline = fs::read(l.baseline()).unwrap();
        assert!(noise_start(&l.ws, &l.track, 3).unwrap().is_refused());
        assert!(noise_finalize(&l.ws, &l.track).unwrap().is_refused());
        assert_eq!(fs::read(l.baseline()).unwrap(), baseline);
    }

    fn commit_fixture(ws: &Path, message: &str) {
        assert!(git(ws, &["add", "SOUL.md"]));
        assert!(git(
            ws,
            &[
                "-c",
                "user.email=test@users.noreply.github.com",
                "-c",
                "user.name=Fixture",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                message
            ]
        ));
    }

    #[test]
    fn scoring_rejects_dirty_proposals_and_changed_commits() {
        let (_d, l) = fixture(60);
        assert!(git(&l.ws, &["init", "-q"]));
        fs::write(l.ws.join("SOUL.md"), "base").unwrap();
        commit_fixture(&l.ws, "base");
        establish_baseline(&l, 0.5, 0.5);
        let ctx = begin_ok(&l);
        propose(&ctx, "improve", &["SOUL.md"]);
        fs::write(l.ws.join("SOUL.md"), "uncommitted improvement").unwrap();
        assert_eq!(
            cases(&l.ws, &l.track, "test").unwrap().refused_reason(),
            Some("dirty_proposal")
        );
        let set = load_set(&l).unwrap().unwrap();
        write_scores(&ctx.paths.scores_train, &ids(&set.train), |_| 0.7);
        write_scores(&ctx.paths.scores_test, &ids(&set.test), |_| 0.7);
        assert_eq!(
            decide(&l.ws, &l.track).unwrap().refused_reason(),
            Some("dirty_proposal")
        );
        commit_fixture(&l.ws, "proposal");
        assert!(!cases(&l.ws, &l.track, "test").unwrap().is_refused());
        fs::write(
            l.ws.join("SOUL.md"),
            "different implementation after scoring",
        )
        .unwrap();
        commit_fixture(&l.ws, "changed");
        assert_eq!(
            decide(&l.ws, &l.track).unwrap().refused_reason(),
            Some("proposal_changed")
        );
        assert!(!l.iteration(ctx.iteration).join(DECISION_FILE).exists());
    }

    #[test]
    fn restarting_calibration_requires_new_scores() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.5, 0.5);
        let original = fs::read(l.noise()).unwrap();
        assert!(!noise_start(&l.ws, &l.track, 3).unwrap().is_refused());
        assert!(
            noise_finalize(&l.ws, &l.track).is_err(),
            "old score files must not refresh calibration"
        );
        assert_eq!(fs::read(l.noise()).unwrap(), original);
    }

    #[test]
    fn calibration_rejects_changed_split_or_judge() {
        for change_judge in [false, true] {
            let (_d, l) = fixture(300);
            establish_baseline(&l, 0.5, 0.5);
            if change_judge {
                fs::write(l.judge(), "A different grading rubric").unwrap();
            } else {
                let mut config: TrackConfig = read_json(&l.config()).unwrap();
                config.salt = "a different split".into();
                write_json(&l.config(), &config).unwrap();
            }
            assert_eq!(
                begin(&l.ws, &l.track, false, true)
                    .unwrap()
                    .refused_reason(),
                Some("noise_stale")
            );
        }
    }

    #[test]
    fn begin_rejects_committed_implementation_changes_since_calibration() {
        let (_d, l) = fixture(60);
        assert!(git(&l.ws, &["init", "-q"]));
        fs::write(l.ws.join("SOUL.md"), "calibrated implementation").unwrap();
        commit_fixture(&l.ws, "baseline");
        establish_baseline(&l, 0.5, 0.5);
        fs::write(l.ws.join("SOUL.md"), "unmeasured implementation").unwrap();
        commit_fixture(&l.ws, "changed");
        assert_eq!(
            begin(&l.ws, &l.track, false, true)
                .unwrap()
                .refused_reason(),
            Some("dirty_baseline")
        );
    }

    #[test]
    fn calibration_cannot_be_reapplied_over_an_accepted_baseline() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.5, 0.5);
        assert_eq!(run_iteration(&l, 0.7, 0.7).0, "ACCEPT");
        let baseline = fs::read(l.baseline()).unwrap();
        let noise = fs::read(l.noise()).unwrap();
        let _ = noise_finalize(&l.ws, &l.track);
        assert_eq!(
            fs::read(l.baseline()).unwrap(),
            baseline,
            "completed calibration must not rewind accepted baseline"
        );
        assert_eq!(
            fs::read(l.noise()).unwrap(),
            noise,
            "completed calibration must not reset its age/count"
        );
    }

    #[test]
    fn calibration_rejects_implementation_drift_during_scoring() {
        let (_d, l) = fixture(60);
        assert!(git(&l.ws, &["init", "-q"]));
        fs::write(l.ws.join("SOUL.md"), "scored implementation").unwrap();
        commit_fixture(&l.ws, "base");
        noise_start(&l.ws, &l.track, 1).unwrap();
        let plan: NoisePlan = read_json(&l.noise_dir().join("plan.json")).unwrap();
        let scores = noise_scores(&l, &plan).unwrap();
        write_scores(&scores.join("pass-1.jsonl"), &plan.test_ids, |_| 0.5);
        write_scores(&scores.join("train.jsonl"), &plan.train_ids, |_| 0.5);
        fs::write(l.ws.join("SOUL.md"), "unscored implementation").unwrap();
        commit_fixture(&l.ws, "changed");
        assert_eq!(
            noise_finalize(&l.ws, &l.track).unwrap().refused_reason(),
            Some("dirty_baseline")
        );
        assert!(!l.baseline().exists());
    }

    #[test]
    fn calibration_recovers_partial_publication_from_its_saved_result() {
        let (_d, l) = fixture(60);
        noise_start(&l.ws, &l.track, 1).unwrap();
        let plan: NoisePlan = read_json(&l.noise_dir().join("plan.json")).unwrap();
        let scores = noise_scores(&l, &plan).unwrap();
        write_scores(&scores.join("pass-1.jsonl"), &plan.test_ids, |_| 0.5);
        write_scores(&scores.join("train.jsonl"), &plan.train_ids, |_| 0.5);
        fs::create_dir(l.baseline()).unwrap();
        assert!(noise_finalize(&l.ws, &l.track).is_err());
        let noise = fs::read(l.noise()).unwrap();
        assert_eq!(
            noise_start(&l.ws, &l.track, 1).unwrap().refused_reason(),
            Some("calibration_pending")
        );
        assert_eq!(
            begin(&l.ws, &l.track, false, true)
                .unwrap()
                .refused_reason(),
            Some("calibration_pending")
        );
        fs::write(scores.join("pass-1.jsonl"), "invalid replacement scores").unwrap();
        fs::remove_dir(l.baseline()).unwrap();
        assert!(!noise_finalize(&l.ws, &l.track).unwrap().is_refused());
        assert_eq!(fs::read(l.noise()).unwrap(), noise);
        let baseline: Baseline = read_json(&l.baseline()).unwrap();
        assert_eq!(baseline.test_mean, 0.5);
        assert!(!calibration_pending(&l).unwrap());
        assert!(!begin(&l.ws, &l.track, false, true).unwrap().is_refused());
    }

    // ── split ──

    #[test]
    fn split_is_deterministic_and_roughly_proportional() {
        let a = is_test_case("salt", "case-001", 30);
        let b = is_test_case("salt", "case-001", 30);
        assert_eq!(a, b);
        let n = (0..1000)
            .filter(|i| is_test_case("salt", &format!("id-{i}"), 30))
            .count();
        assert!(
            (200..=400).contains(&n),
            "30% of 1000 should be ~300, got {n}"
        );
        assert!(is_test_case("s", "x", 100));
        assert!(!is_test_case("s", "x", 0));
    }

    // ── noise ──

    #[test]
    fn noise_floor_formula() {
        assert!((sample_stddev(&[0.70, 0.72, 0.74]) - 0.02).abs() < 1e-9);
        assert!((noise_floor_from_stddev(0.02) - 0.04).abs() < 1e-9);
        assert!((noise_floor_from_stddev(0.0) - 0.01).abs() < 1e-9);
    }

    #[test]
    fn noise_finalize_writes_noise_and_baseline() {
        let (_d, l) = fixture(60);
        let out = noise_start(&l.ws, "prompt", 3).unwrap();
        assert!(!out.is_refused());
        let plan: NoisePlan = read_json(&l.noise_dir().join("plan.json")).unwrap();
        for (k, m) in [(1, 0.70), (2, 0.72), (3, 0.74)] {
            write_scores(
                &noise_scores(&l, &plan)
                    .unwrap()
                    .join(format!("pass-{k}.jsonl")),
                &plan.test_ids,
                |_| m,
            );
        }
        write_scores(
            &noise_scores(&l, &plan).unwrap().join("train.jsonl"),
            &plan.train_ids,
            |_| 0.6,
        );
        let out = noise_finalize(&l.ws, "prompt").unwrap();
        assert!(!out.is_refused(), "{:?}", out);
        let noise: Noise = read_json(&l.noise()).unwrap();
        assert!((noise.stddev - 0.02).abs() < 1e-9);
        assert!((noise.floor - 0.04).abs() < 1e-9);
        assert!((noise.mean - 0.72).abs() < 1e-9);
        let b: Baseline = read_json(&l.baseline()).unwrap();
        assert!((b.test_mean - 0.72).abs() < 1e-9);
        assert!((b.train_mean - 0.6).abs() < 1e-9);
        assert_eq!(b.train_cases.len(), plan.train_ids.len());
        assert_eq!(b.source, "noise");
    }

    #[test]
    fn noise_finalize_rejects_incomplete_or_out_of_range_scores() {
        let (_d, l) = fixture(60);
        noise_start(&l.ws, "prompt", 1).unwrap();
        let plan: NoisePlan = read_json(&l.noise_dir().join("plan.json")).unwrap();
        let mut partial = plan.test_ids.clone();
        partial.pop();
        write_scores(
            &noise_scores(&l, &plan).unwrap().join("pass-1.jsonl"),
            &partial,
            |_| 0.5,
        );
        write_scores(
            &noise_scores(&l, &plan).unwrap().join("train.jsonl"),
            &plan.train_ids,
            |_| 0.5,
        );
        let err = noise_finalize(&l.ws, "prompt").unwrap_err().to_string();
        assert!(err.contains("scores_incomplete"), "{err}");

        write_scores(
            &noise_scores(&l, &plan).unwrap().join("pass-1.jsonl"),
            &plan.test_ids,
            |_| 1.2,
        );
        let err = noise_finalize(&l.ws, "prompt").unwrap_err().to_string();
        assert!(err.contains("score_out_of_range"), "{err}");
    }

    // ── begin refusals ──

    #[test]
    fn begin_refuses_insufficient_cases_and_logs_skip() {
        let (_d, l) = fixture(5);
        let out = begin(&l.ws, "prompt", false, false).unwrap();
        assert_eq!(out.refused_reason(), Some("insufficient_cases"));
        let skips = eval_loop::read_skips(&l.ws).unwrap();
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0].reason, "insufficient_cases");
        assert!(!l.results().exists(), "refusals never touch results.tsv");
    }

    #[test]
    fn begin_refuses_noise_missing_then_stale_then_too_high() {
        let (_d, l) = fixture(60);
        assert_eq!(
            begin(&l.ws, "prompt", false, false)
                .unwrap()
                .refused_reason(),
            Some("noise_missing")
        );

        establish_baseline(&l, 0.7, 0.6);
        // mutate cases → hash mismatch
        append_line(
            &l.cases(),
            &case(
                "case-999",
                "a brand new case appended after the noise measurement",
            ),
        )
        .unwrap();
        assert_eq!(
            begin(&l.ws, "prompt", false, false)
                .unwrap()
                .refused_reason(),
            Some("noise_stale")
        );

        // restore and age the measurement
        let (_d2, l2) = fixture(60);
        establish_baseline(&l2, 0.7, 0.6);
        let mut noise: Noise = read_json(&l2.noise()).unwrap();
        noise.measured_at = "2020-01-01T00:00:00Z".into();
        write_json(&l2.noise(), &noise).unwrap();
        assert_eq!(
            begin(&l2.ws, "prompt", false, false)
                .unwrap()
                .refused_reason(),
            Some("noise_stale")
        );

        noise.measured_at = now();
        noise.floor = 0.5;
        write_json(&l2.noise(), &noise).unwrap();
        assert_eq!(
            begin(&l2.ws, "prompt", false, false)
                .unwrap()
                .refused_reason(),
            Some("noise_too_high")
        );
    }

    #[test]
    fn begin_refuses_saturated() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.97, 0.9);
        assert_eq!(
            begin(&l.ws, "prompt", false, false)
                .unwrap()
                .refused_reason(),
            Some("saturated")
        );
    }

    #[test]
    fn begin_refuses_run_in_progress_unless_adopting_run_json() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.7, 0.6);
        fs::create_dir_all(l.eval_loop()).unwrap();
        fs::write(l.run_lock(), "other-run").unwrap();
        assert_eq!(
            begin(&l.ws, "prompt", false, false)
                .unwrap()
                .refused_reason(),
            Some("run_in_progress")
        );

        // daemon-enqueued run: run.json + run.lock exist → adopt its runId
        let spec = eval_loop::RunSpec {
            run_id: "daemon-run-1".into(),
            familiar_id: "x".into(),
            track: "prompt".into(),
            requested_at: now(),
        };
        write_json(&l.run_json(), &spec).unwrap();
        fs::write(l.run_lock(), "daemon-run-1").unwrap();
        let Outcome::Ok(v) = begin(&l.ws, "prompt", true, false).unwrap() else {
            panic!()
        };
        assert_eq!(v["run_id"], "daemon-run-1");
    }

    // ── cases gating ──

    #[test]
    fn test_cases_are_gated_on_proposal() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.7, 0.6);
        assert_eq!(
            cases(&l.ws, "prompt", "test").unwrap().refused_reason(),
            Some("no_active_iteration")
        );
        let ctx = begin_ok(&l);
        assert!(!cases(&l.ws, "prompt", "train").unwrap().is_refused());
        assert_eq!(
            cases(&l.ws, "prompt", "test").unwrap().refused_reason(),
            Some("proposal_missing")
        );
        propose(&ctx, "one change", &["SOUL.md"]);
        assert!(!cases(&l.ws, "prompt", "test").unwrap().is_refused());
        // context never contains a test row
        let test_ids: BTreeSet<String> = {
            let Outcome::Ok(v) = cases(&l.ws, "prompt", "test").unwrap() else {
                panic!()
            };
            v["cases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap().to_string())
                .collect()
        };
        for f in &ctx.train_failures {
            assert!(!test_ids.contains(&f.id));
        }
    }

    // ── accept rule ──

    fn run_iteration(l: &Layout, test_after: f64, train_after: f64) -> (String, String) {
        let ctx = begin_ok(l);
        propose(&ctx, "tweak", &["SOUL.md"]);
        let Outcome::Ok(tr) = cases(&l.ws, &l.track, "train").unwrap() else {
            panic!()
        };
        let Outcome::Ok(te) = cases(&l.ws, &l.track, "test").unwrap() else {
            panic!()
        };
        let tr_ids: Vec<String> = tr["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        let te_ids: Vec<String> = te["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        write_scores(&ctx.paths.scores_train, &tr_ids, |_| train_after);
        write_scores(&ctx.paths.scores_test, &te_ids, |_| test_after);
        decide_reason(l)
    }

    #[test]
    fn decide_accept_overfit_regression_noise() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.70, 0.60); // floor = 0.01 (constant passes)
        assert_eq!(
            run_iteration(&l, 0.80, 0.65),
            ("ACCEPT".into(), "accepted".into())
        );
        let b: Baseline = read_json(&l.baseline()).unwrap();
        assert!(
            (b.test_mean - 0.80).abs() < 1e-9,
            "baseline refreshed on ACCEPT"
        );
        assert_eq!(b.source, "iteration:1");
        let n: Noise = read_json(&l.noise()).unwrap();
        assert_eq!(n.accepts_since, 1);
        assert!(!l.run_lock().exists(), "lock released");

        assert_eq!(
            run_iteration(&l, 0.80, 0.85),
            ("REVERT".into(), "overfit".into())
        );
        let b2: Baseline = read_json(&l.baseline()).unwrap();
        assert!(
            (b2.test_mean - 0.80).abs() < 1e-9,
            "baseline unchanged on REVERT"
        );

        assert_eq!(
            run_iteration(&l, 0.60, 0.65),
            ("REVERT".into(), "regression".into())
        );
        assert_eq!(
            run_iteration(&l, 0.805, 0.65),
            ("REVERT".into(), "noise".into())
        );

        let rows = track_rows(&l);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].decision_reason.as_deref(), Some("accepted"));
        assert_eq!(rows[1].decision_reason.as_deref(), Some("overfit"));
        assert_eq!(rows[3].iteration, 4);
        assert!(rows
            .iter()
            .all(|r| r.noise_floor.is_some() && r.run_id.is_some()));
        let header = fs::read_to_string(l.results()).unwrap();
        assert!(header.starts_with(TSV_HEADER_V2));
    }

    #[test]
    fn decide_requires_buckets_after_three_reverts() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.70, 0.60);
        for _ in 0..3 {
            assert_eq!(run_iteration(&l, 0.70, 0.60).0, "REVERT");
        }
        let ctx = begin_ok(&l);
        assert!(ctx.request_buckets);
        propose(&ctx, "big fix", &["SOUL.md"]);
        let Outcome::Ok(tr) = cases(&l.ws, "prompt", "train").unwrap() else {
            panic!()
        };
        let Outcome::Ok(te) = cases(&l.ws, "prompt", "test").unwrap() else {
            panic!()
        };
        let tr_ids: Vec<String> = tr["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        let te_ids: Vec<String> = te["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        write_scores(&ctx.paths.scores_train, &tr_ids, |_| 0.9);
        write_scores(&ctx.paths.scores_test, &te_ids, |_| 0.9);
        assert_eq!(
            decide_reason(&l),
            ("REVERT".into(), "buckets_missing".into())
        );
    }

    #[test]
    fn decide_rejects_too_many_files_unless_allowed() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.70, 0.60);
        let ctx = begin_ok(&l);
        propose(&ctx, "sprawl", &["a", "b", "c", "d"]);
        let Outcome::Ok(tr) = cases(&l.ws, "prompt", "train").unwrap() else {
            panic!()
        };
        let Outcome::Ok(te) = cases(&l.ws, "prompt", "test").unwrap() else {
            panic!()
        };
        let tr_ids: Vec<String> = tr["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        let te_ids: Vec<String> = te["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        write_scores(&ctx.paths.scores_train, &tr_ids, |_| 0.9);
        write_scores(&ctx.paths.scores_test, &te_ids, |_| 0.9);
        assert_eq!(
            decide_reason(&l),
            ("REVERT".into(), "too_many_files".into())
        );
    }

    #[test]
    fn decide_errors_on_incomplete_scores() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.70, 0.60);
        let ctx = begin_ok(&l);
        propose(&ctx, "tweak", &["SOUL.md"]);
        write_scores(&ctx.paths.scores_train, &["case-000".to_string()], |_| 0.9);
        write_scores(&ctx.paths.scores_test, &[], |_| 0.9);
        let err = decide(&l.ws, "prompt").unwrap_err().to_string();
        assert!(err.contains("scores_incomplete"), "{err}");
    }

    // ── leak check ──

    #[test]
    fn leak_check_windows_and_states() {
        let cases = vec![Case {
            id: "t1".into(),
            source: None,
            input: "short".into(),
            expected: Some("the expected answer for case 017 is forty-plus characters long".into()),
            rubric: None,
            tags: None,
        }];
        let (state, id) = leak_check("nothing relevant here at all", &cases);
        assert_eq!(state, "partial"); // `input` too short to check
        assert!(id.is_none());
        let (state, id) = leak_check(
            "diff +  the   expected answer for case 017 is forty-plus characters long\n",
            &cases,
        );
        assert_eq!(state, "leaked");
        assert_eq!(id.as_deref(), Some("t1"));
        let long = vec![Case {
            id: "t2".into(),
            source: None,
            input: "x".repeat(50),
            expected: Some("y".repeat(50)),
            rubric: None,
            tags: None,
        }];
        assert_eq!(leak_check("zzz", &long).0, "clean");
    }

    #[test]
    fn decide_reverts_on_leak_in_non_git_workspace() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.70, 0.60);
        let ctx = begin_ok(&l);
        // leak a TEST expected string into a proposal file
        let Outcome::Ok(_) = cases(&l.ws, "prompt", "train").unwrap() else {
            panic!()
        };
        propose(&ctx, "cheat", &["SOUL.md"]);
        let Outcome::Ok(te) = cases(&l.ws, "prompt", "test").unwrap() else {
            panic!()
        };
        let first = &te["cases"][0];
        fs::write(
            l.ws.join("SOUL.md"),
            format!("Always answer: {}", first["expected"].as_str().unwrap()),
        )
        .unwrap();
        let Outcome::Ok(tr) = cases(&l.ws, "prompt", "train").unwrap() else {
            panic!()
        };
        let tr_ids: Vec<String> = tr["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        let te_ids: Vec<String> = te["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        write_scores(&ctx.paths.scores_train, &tr_ids, |_| 1.0);
        write_scores(&ctx.paths.scores_test, &te_ids, |_| 1.0);
        assert_eq!(decide_reason(&l), ("REVERT".into(), "leak_detected".into()));
    }

    // ── human gate & ack ──

    #[test]
    fn begin_refuses_trace_unreviewed_until_ack() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.70, 0.60);
        run_iteration(&l, 0.80, 0.65);
        assert_eq!(
            begin(&l.ws, "prompt", false, false)
                .unwrap()
                .refused_reason(),
            Some("trace_unreviewed")
        );
        assert!(!ack(&l.ws, "prompt").unwrap().is_refused());
        assert!(!begin(&l.ws, "prompt", false, false).unwrap().is_refused());
        assert_eq!(
            begin(&l.ws, "prompt", false, true)
                .unwrap()
                .refused_reason(),
            Some("iteration_open")
        );
    }

    #[test]
    fn status_reports_counts() {
        let (_d, l) = fixture(60);
        establish_baseline(&l, 0.70, 0.60);
        run_iteration(&l, 0.80, 0.65);
        run_iteration(&l, 0.80, 0.65); // noise → REVERT (ack_unreviewed=true in begin_ok)
        let Outcome::Ok(v) = status(&l.ws, "prompt", 5).unwrap() else {
            panic!()
        };
        assert_eq!(v["iterations_total"], 2);
        assert_eq!(v["accepted"], 1);
        assert_eq!(v["reverted"], 1);
        assert_eq!(v["trailing_reverts"], 1);
    }

    // ── git baseline (skipped when git is unavailable) ──

    fn git(ws: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(ws)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn begin_git_checks_use_ancestry_and_ignore_bookkeeping() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git unavailable; skipping");
            return;
        }
        let (_d, l) = fixture(60);
        let g = |args: &[&str]| {
            let mut full = vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ];
            full.extend_from_slice(args);
            assert!(git(&l.ws, &full), "git {:?}", args);
        };
        fs::write(l.ws.join("SOUL.md"), "v0\n").unwrap();
        g(&["init", "-q"]);
        // Keep the reset assertion independent of the host's checkout line endings.
        g(&["config", "--local", "core.autocrlf", "false"]);
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "base"]);
        establish_baseline(&l, 0.70, 0.60);

        // Untracked noise/baseline/eval-loop files are bookkeeping → not dirty.
        let ctx = begin_ok(&l);
        assert!(ctx.git);
        assert_eq!(ctx.base_commit, git_head(&l.ws));
        let base = ctx.base_commit.clone().unwrap();

        // Proposal commit, scored as noise → REVERT with head_commit != base_commit.
        fs::write(l.ws.join("SOUL.md"), "v1\n").unwrap();
        g(&["commit", "-q", "-am", "tweak"]);
        propose(&ctx, "tweak", &["SOUL.md"]);
        let Outcome::Ok(tr) = cases(&l.ws, "prompt", "train").unwrap() else {
            panic!()
        };
        let Outcome::Ok(te) = cases(&l.ws, "prompt", "test").unwrap() else {
            panic!()
        };
        let tr_ids: Vec<String> = tr["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        let te_ids: Vec<String> = te["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().into())
            .collect();
        write_scores(&ctx.paths.scores_train, &tr_ids, |_| 0.60);
        write_scores(&ctx.paths.scores_test, &te_ids, |_| 0.70);
        assert_eq!(decide_reason(&l), ("REVERT".into(), "noise".into()));
        let d: Decision = read_json(&l.iteration(1).join(DECISION_FILE)).unwrap();
        assert!(d.revert_required);
        assert_ne!(d.head_commit, d.base_commit);

        // Harness forgot to revert → the reverted commit is still an ancestor → refused.
        let out = begin(&l.ws, "prompt", false, true).unwrap();
        assert_eq!(out.refused_reason(), Some("dirty_baseline"));
        let Outcome::Refused { detail, .. } = out else {
            panic!()
        };
        assert!(detail.get("reverted_commit").is_some());

        // Harness reverts (its job, not ours) → clean begin.
        g(&["reset", "-q", "--hard", &base]);
        assert_eq!(fs::read_to_string(l.ws.join("SOUL.md")).unwrap(), "v0\n");
        // Committing bookkeeping between iterations is allowed.
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "bookkeeping"]);
        let ctx2 = begin_ok(&l);
        assert_eq!(ctx2.iteration, 2);

        // Uncommitted edit outside bookkeeping → dirty.
        write_json(
            &ctx2.paths.proposal,
            &Proposal {
                summary: "x".into(),
                reasoning: None,
                files: vec![],
                failure_buckets: None,
                allow_multi_file: None,
                judge: None,
            },
        )
        .unwrap();
        assert!(!cases(&l.ws, "prompt", "test").unwrap().is_refused());
        write_scores(&ctx2.paths.scores_train, &tr_ids, |_| 0.60);
        write_scores(&ctx2.paths.scores_test, &te_ids, |_| 0.70);
        decide_reason(&l);
        fs::write(l.ws.join("SOUL.md"), "uncommitted\n").unwrap();
        assert_eq!(
            begin(&l.ws, "prompt", false, true)
                .unwrap()
                .refused_reason(),
            Some("dirty_baseline")
        );
    }
}
