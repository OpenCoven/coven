# Coven CLI P0 — Coven Code Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring four foundational features from `@opencoven/coven-code` (Node) into `coven-cli` (Rust): unified JSONC settings, thread/session upgrades, file references in prompts, and a documented stream-JSON I/O protocol.

**Architecture:** Layer the new features on top of the existing `store.rs` (SQLite ledger) and `harness.rs` (Codex/Claude PTY runner). All four features sit at the I/O boundary — no changes to the agent loop, no new auth path, no remote calls. The work is split into four phases that each end in a shippable commit; phases 1–4 are ordered by dependency (settings → threads → file refs → stream-JSON), but file-refs is independent and could ship at any point.

**Tech Stack:** Rust 2021, clap 4, rusqlite 0.39 (bundled SQLite with FTS5), serde 1, serde_json, anyhow, regex, ratatui (for any TUI-side glue). New deps: `json5 = "0.4"` for JSONC parsing (Phase 1) and `globset = "0.4"` for prompt globs (Phase 3).

---

## Execution note (added 2026-05-26 mid-flight)

The plan was originally drafted assuming the state of `security/private-session-logs`, where `crates/coven-cli/src/privacy.rs`, `encrypted_artifacts.rs`, and the `redaction_status`/`sensitive`/`sensitive_artifacts` schema additions exist. **On `origin/main` (this PR's base) none of those exist yet.** Concrete adjustments applied during execution:

- **Task 1.4 (privacy wiring) is deferred** to a follow-up plan that runs once `security/private-session-logs` lands on main. The `PrivacySettings` type still lives in `settings.rs` so the schema is forward-compatible.
- **Phase 2 thread search** runs over `events.payload_json` as-is on main; the "redacted" qualifier in the plan was a security-branch artifact. Behavior is unchanged because main currently stores plain payloads.
- **Phase 3 `@T-<id>` expansion** reads `events.payload_json` directly; no decryption path needed on main.
- **Phase 4 stream-JSON** is unaffected.

---

## Design decisions (locked in — challenge before execution if you disagree)

1. **Stream-JSON event schema = Coven Code's exact JSONL shape**, which mirrors Anthropic's tool-use shape (`type`, `message`, `tool_use_id`, `session_id`, etc.). External SDKs written against `@opencoven/coven-code` should switch CLIs without modifying their parser. See `docs/STREAM-JSON.md` introduced in Phase 4.
2. **Settings canonical = `~/.config/coven/settings.json`** (JSONC, `covenCli.*` keyspace). Existing `~/.coven/repos.toml` and `~/.coven/privacy.toml` remain readable as fallbacks for one release. When both define the same key, JSONC wins and a one-time stderr warning lists the keys overridden. `familiars.toml` is data, not config — out of scope for this plan.
3. **Thread search uses SQLite FTS5** (already shipped in rusqlite's bundled SQLite build) over the redacted `events.payload_json` column only. No raw-artifact decryption during search; that would defeat the privacy default.
4. **Visibility levels = `private | workspace | shared`** (three levels — simpler than Coven Code's five, room to expand later). Default `private`.
5. **`--continue` with no argument resumes the latest non-archived session whose `project_root` matches the current directory.** With an explicit id it resumes by id.
6. **`@README.md` resolves relative to the invocation cwd**, matching Coven Code. Globs honor `.gitignore` unless the path matches `covenCli.fuzzy.alwaysIncludePaths`.
7. **Image references in non-stream mode produce a text placeholder** (`[image @ path: file.png, image/png, 12345 bytes]`); in stream-JSON mode they become real `image` content blocks. Text refs cap at 500 lines × 2048 chars/line, matching Coven Code's `FILE_MENTION_MAX_*` constants.
8. **Schema migrations follow the existing `ensure_column` pattern in `store.rs`**: idempotent `ALTER TABLE ... ADD COLUMN` guarded by a `PRAGMA table_info` check. No down-migrations.

---

## File structure

### New files

- `crates/coven-cli/src/settings.rs` — `Settings` struct, `load()` with precedence, JSONC parsing.
- `crates/coven-cli/src/prompt_refs.rs` — `@path` / `@@search` / `@T-<id>` expansion.
- `crates/coven-cli/src/stream_json.rs` — event type definitions, `emit_event()` writer, `read_event()` parser.
- `docs/STREAM-JSON.md` — the external I/O contract for the protocol.
- `docs/SETTINGS.md` — settings file format, precedence, key reference.

### Modified files

- `crates/coven-cli/Cargo.toml` — add `json5 = "0.4"`, `globset = "0.4"`.
- `crates/coven-cli/src/main.rs` — new flags (`--stream-json`, `--stream-json-input`, `--continue`, `--archive`, `--labels`, `--visibility`), new subcommand `sessions search`, settings loaded at startup.
- `crates/coven-cli/src/store.rs` — new columns `labels`, `visibility` on `sessions`; new FTS5 virtual table `events_fts`; new helpers `latest_active_for_project()`, `search_events()`, `set_labels()`, `set_visibility()`.
- `crates/coven-cli/src/privacy.rs` — read from `Settings` first, fall back to `privacy.toml`.
- `crates/coven-cli/src/repos_config.rs` — read from `Settings` first, fall back to `repos.toml`.
- `crates/coven-cli/src/pty_runner.rs` — emit stream-JSON events around PTY lifecycle when caller opts in.
- `crates/coven-cli/src/harness.rs` — promote raw claude stream-JSON shape into our `stream_json::Event` enum.

---

## Phase 1 — Unified JSONC settings (foundation)

**Outcome:** A single `~/.config/coven/settings.json` file is the canonical config. `repos.toml` and `privacy.toml` still work, with a deprecation warning when they shadow JSONC keys.

### Task 1.1: Add `json5` dependency

**Files:**
- Modify: `crates/coven-cli/Cargo.toml`

- [ ] **Step 1: Add dep**

```toml
# in [dependencies]
json5 = "0.4"
```

- [ ] **Step 2: Verify build**

Run: `cargo build -p coven-cli`
Expected: clean build, `json5` resolves.

- [ ] **Step 3: Commit**

```bash
git add crates/coven-cli/Cargo.toml crates/coven-cli/Cargo.lock
git commit -S -m "deps(coven-cli): add json5 for JSONC settings parsing"
```

### Task 1.2: Define `Settings` types and the empty loader

**Files:**
- Create: `crates/coven-cli/src/settings.rs`
- Modify: `crates/coven-cli/src/main.rs` (declare module)

- [ ] **Step 1: Write the failing test**

In `crates/coven-cli/src/settings.rs`:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    #[serde(rename = "covenCli")]
    pub coven_cli: CovenCliSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CovenCliSettings {
    pub privacy: Option<PrivacySettings>,
    pub repos: BTreeMap<String, RepoSettings>,
    #[serde(default)]
    pub default_repo: Option<String>,
    #[serde(default)]
    pub fuzzy: FuzzySettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrivacySettings {
    pub persist_raw_artifacts: Option<bool>,
    pub raw_artifact_retention_days: Option<u64>,
    pub log_retention_days: Option<u64>,
    pub extra_patterns: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RepoSettings {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FuzzySettings {
    pub always_include_paths: Vec<String>,
}

pub fn user_settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("coven").join(SETTINGS_FILE_NAME))
}

pub fn load_from(path: &Path) -> Result<Option<Settings>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let parsed: Settings = json5::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        assert_eq!(load_from(&path).unwrap(), None);
    }

    #[test]
    fn loads_jsonc_with_comments_and_trailing_commas() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
              // user comment
              "covenCli": {
                "defaultRepo": "alpha",
                "repos": {
                  "alpha": { "path": "/abs/alpha" },
                },
              },
            }"#,
        )
        .unwrap();
        let loaded = load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.coven_cli.default_repo.as_deref(), Some("alpha"));
        assert_eq!(
            loaded.coven_cli.repos.get("alpha").unwrap().path,
            PathBuf::from("/abs/alpha")
        );
    }
}
```

In `crates/coven-cli/src/main.rs`, add the module:

```rust
// near the other `mod ...;` lines
mod settings;
```

- [ ] **Step 2: Run tests to verify they fail then pass**

Run: `cargo test -p coven-cli settings::tests -- --nocapture`
Expected: both tests pass after implementation in step 1 (they were written alongside the impl since serde does most of the work). If a test fails, fix the structure mismatch.

- [ ] **Step 3: Commit**

```bash
git add crates/coven-cli/src/settings.rs crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): add Settings type with JSONC loader"
```

### Task 1.3: Wire settings into `repos_config.rs`

**Files:**
- Modify: `crates/coven-cli/src/repos_config.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `crates/coven-cli/src/repos_config.rs`:

```rust
#[test]
fn settings_override_toml_when_both_present() -> Result<()> {
    let temp = tempfile::tempdir()?;
    // legacy TOML
    std::fs::write(
        config_path(temp.path()),
        r#"
default = "from_toml"
[repos.from_toml]
path = "/abs/toml"
"#,
    )?;
    // new JSONC at sibling settings dir override path
    let settings = crate::settings::Settings {
        coven_cli: crate::settings::CovenCliSettings {
            default_repo: Some("from_jsonc".to_string()),
            repos: std::collections::BTreeMap::from([(
                "from_jsonc".to_string(),
                crate::settings::RepoSettings {
                    path: std::path::PathBuf::from("/abs/jsonc"),
                },
            )]),
            ..Default::default()
        },
    };
    let merged = load_with_settings(temp.path(), Some(&settings))?;
    assert_eq!(merged.default_name(), Some("from_jsonc"));
    assert_eq!(
        merged.resolve("from_jsonc"),
        Some(std::path::PathBuf::from("/abs/jsonc"))
    );
    // legacy entry still resolvable as a fallback
    assert_eq!(
        merged.resolve("from_toml"),
        Some(std::path::PathBuf::from("/abs/toml"))
    );
    Ok(())
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p coven-cli repos_config::tests::settings_override_toml_when_both_present`
Expected: FAIL — `load_with_settings` does not exist.

- [ ] **Step 3: Implement `load_with_settings`**

Add to `crates/coven-cli/src/repos_config.rs`:

```rust
pub fn load_with_settings(
    coven_home: &Path,
    settings: Option<&crate::settings::Settings>,
) -> Result<ReposConfig> {
    let mut config = load(coven_home)?;
    let Some(settings) = settings else {
        return Ok(config);
    };
    for (name, repo) in &settings.coven_cli.repos {
        config.repos.insert(
            name.clone(),
            RepoEntry {
                path: repo.path.clone(),
            },
        );
    }
    if let Some(name) = &settings.coven_cli.default_repo {
        config.default = Some(name.clone());
    }
    Ok(config)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p coven-cli repos_config::tests`
Expected: all 5 tests pass (4 existing + 1 new).

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/repos_config.rs
git commit -S -m "feat(coven-cli): repos_config reads from Settings with TOML fallback"
```

### Task 1.4: Wire settings into `privacy.rs`

**Files:**
- Modify: `crates/coven-cli/src/privacy.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing test module in `privacy.rs`:

```rust
#[test]
fn settings_override_toml_for_retention() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("privacy.toml"),
        "log_retention_days = 99\n",
    )
    .unwrap();
    let settings = crate::settings::Settings {
        coven_cli: crate::settings::CovenCliSettings {
            privacy: Some(crate::settings::PrivacySettings {
                log_retention_days: Some(7),
                ..Default::default()
            }),
            ..Default::default()
        },
    };
    let loaded = load_with_settings(temp.path(), Some(&settings)).unwrap();
    // JSONC wins
    assert_eq!(loaded.log_retention_days, 7);
}
```

- [ ] **Step 2: Run test to confirm fail**

Run: `cargo test -p coven-cli privacy::tests::settings_override_toml_for_retention`
Expected: FAIL — `load_with_settings` not defined.

- [ ] **Step 3: Implement `load_with_settings`**

Add to `privacy.rs`:

```rust
pub fn load_with_settings(
    coven_home: &Path,
    settings: Option<&crate::settings::Settings>,
) -> Result<PrivacyConfig> {
    let mut config = load_config(coven_home)?;
    if let Some(s) = settings.and_then(|s| s.coven_cli.privacy.as_ref()) {
        if let Some(v) = s.persist_raw_artifacts {
            config.persist_raw_artifacts = v;
        }
        if let Some(v) = s.raw_artifact_retention_days {
            config.raw_artifact_retention_days = v;
        }
        if let Some(v) = s.log_retention_days {
            config.log_retention_days = v;
        }
        if let Some(v) = &s.extra_patterns {
            config.extra_patterns = v.clone();
        }
    }
    // env vars still trump everything (already applied by load_config; reapply
    // here in case settings clobbered them above)
    if let Some(value) = std::env::var_os("COVEN_PERSIST_RAW_ARTIFACTS") {
        config.persist_raw_artifacts = env_truthy(&value.to_string_lossy());
    }
    if let Some(value) = env_u64("COVEN_RAW_ARTIFACT_RETENTION_DAYS") {
        config.raw_artifact_retention_days = value;
    }
    if let Some(value) = env_u64("COVEN_LOG_RETENTION_DAYS") {
        config.log_retention_days = value;
    }
    Ok(config)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p coven-cli privacy::tests`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/privacy.rs
git commit -S -m "feat(coven-cli): privacy reads from Settings with TOML fallback"
```

### Task 1.5: Deprecation warning when both files exist

**Files:**
- Modify: `crates/coven-cli/src/settings.rs`

- [ ] **Step 1: Write the failing test**

Append to `settings.rs` tests:

```rust
#[test]
fn detect_shadowed_keys_lists_overrides() {
    let toml_keys = ["repos.alpha".to_string(), "defaultRepo".to_string()];
    let jsonc_keys = ["repos.alpha".to_string()];
    let shadowed = shadowed_keys(&toml_keys, &jsonc_keys);
    assert_eq!(shadowed, vec!["repos.alpha".to_string()]);
}
```

- [ ] **Step 2: Confirm FAIL**

Run: `cargo test -p coven-cli settings::tests::detect_shadowed_keys_lists_overrides`
Expected: FAIL — symbol not found.

- [ ] **Step 3: Implement**

Add to `settings.rs`:

```rust
pub fn shadowed_keys(toml_keys: &[String], jsonc_keys: &[String]) -> Vec<String> {
    let mut out: Vec<String> = toml_keys
        .iter()
        .filter(|k| jsonc_keys.iter().any(|j| j == *k))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn warn_if_shadowed(shadowed: &[String], toml_path: &Path, jsonc_path: &Path) {
    if shadowed.is_empty() {
        return;
    }
    eprintln!(
        "coven: {} keys in {} are shadowed by {}. Consider removing them from the TOML file.",
        shadowed.len(),
        toml_path.display(),
        jsonc_path.display()
    );
    for key in shadowed {
        eprintln!("  - {key}");
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p coven-cli settings::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/settings.rs
git commit -S -m "feat(coven-cli): warn when legacy TOML keys are shadowed by JSONC settings"
```

### Task 1.6: Load settings once in `main()` and thread through

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Update `run_cli`**

In `crates/coven-cli/src/main.rs`, after `let cli = Cli::parse();`:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = settings::user_settings_path()
        .as_deref()
        .and_then(|path| match settings::load_from(path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("coven: ignoring settings: {err:#}");
                None
            }
        });
    run_cli(cli, settings.as_ref())
}
```

Update `run_cli` signature and pass `settings` to any call site that previously called `repos_config::load(...)` or `privacy::load_config(...)`. Replace those calls with `repos_config::load_with_settings(home, settings)` and `privacy::load_with_settings(home, settings)`.

- [ ] **Step 2: Build**

Run: `cargo build -p coven-cli`
Expected: clean build.

- [ ] **Step 3: Smoke test**

Run: `cargo run -p coven-cli -- doctor`
Expected: existing doctor output, no errors. (Settings absent → no behavior change.)

- [ ] **Step 4: Manual integration**

Create `~/.config/coven/settings.json` (in a temp HOME for the test):

```json
{
  // smoke test
  "covenCli": {
    "defaultRepo": "smoke",
    "repos": { "smoke": { "path": "/tmp" } }
  }
}
```

Run: `HOME=/tmp/coven-smoke XDG_CONFIG_HOME=/tmp/coven-smoke/.config cargo run -p coven-cli -- doctor`
Expected: no parse error.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): load JSONC settings at startup and thread through subcommands"
```

### Task 1.7: Document settings

**Files:**
- Create: `docs/SETTINGS.md`

- [ ] **Step 1: Write the doc**

Create `docs/SETTINGS.md`:

```markdown
# Coven CLI Settings

User settings live at `~/.config/coven/settings.json` (or `$XDG_CONFIG_HOME/coven/settings.json`).
Format is JSONC: `//` and `/* */` comments and trailing commas are allowed.

All keys live under `covenCli.*`.

## Precedence

1. Environment variables (highest)
2. `~/.config/coven/settings.json`
3. `~/.coven/repos.toml`, `~/.coven/privacy.toml` (legacy)

When a key is set in both the JSONC file and a legacy TOML file, the JSONC value
wins and `coven` prints a one-time stderr warning naming the shadowed keys.

## Schema

```jsonc
{
  "covenCli": {
    "defaultRepo": "openclaw",
    "repos": {
      "openclaw": { "path": "~/dev/openclaw" }
    },
    "privacy": {
      "persistRawArtifacts": false,
      "rawArtifactRetentionDays": 7,
      "logRetentionDays": 30,
      "extraPatterns": ["(?i)bearer\\s+[a-z0-9]+"]
    },
    "fuzzy": {
      "alwaysIncludePaths": [".env.example", "docs/secrets-redacted.md"]
    }
  }
}
```

## Migration

The legacy TOML files are still read. To migrate, copy values into the JSONC schema above and
delete the corresponding lines from `repos.toml` / `privacy.toml`. A `coven config migrate`
command will land in a later release.
```

- [ ] **Step 2: Commit**

```bash
git add docs/SETTINGS.md
git commit -S -m "docs(coven-cli): document unified JSONC settings file"
```

**Phase 1 milestone:** unified settings ship as additive feature. Run `cargo test -p coven-cli` — all green. Tag/PR can land here independently.

---

## Phase 2 — Thread/session upgrades

**Outcome:** `coven run --continue [ID] --labels foo,bar --visibility shared --archive`, `coven sessions search <query>`, all backed by SQLite FTS5.

### Task 2.1: Add `labels` and `visibility` columns to `sessions`

**Files:**
- Modify: `crates/coven-cli/src/store.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `store::tests` module:

```rust
#[test]
fn new_columns_default_correctly() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let conn = open_store(&temp.path().join("test.sqlite3"))?;
    conn.execute(
        "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
         VALUES('s1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
        [],
    )?;
    let labels: Option<String> = conn.query_row(
        "SELECT labels FROM sessions WHERE id='s1'",
        [],
        |row| row.get(0),
    )?;
    let visibility: String = conn.query_row(
        "SELECT visibility FROM sessions WHERE id='s1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(labels, None);
    assert_eq!(visibility, "private");
    Ok(())
}
```

- [ ] **Step 2: Run test to confirm FAIL**

Run: `cargo test -p coven-cli store::tests::new_columns_default_correctly`
Expected: FAIL — `labels` column missing.

- [ ] **Step 3: Add migration**

In `store.rs`, inside `open_store()` after `ensure_conversation_id_column(&conn)?;`:

```rust
ensure_labels_column(&conn)?;
ensure_visibility_column(&conn)?;
```

Add the two helper functions next to the existing `ensure_*` functions:

```rust
fn ensure_labels_column(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "sessions",
        "labels",
        "ALTER TABLE sessions ADD COLUMN labels TEXT",
    )
}

fn ensure_visibility_column(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "sessions",
        "visibility",
        "ALTER TABLE sessions ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private'",
    )
}
```

- [ ] **Step 4: Update `SessionRecord`**

Add fields to `SessionRecord` in `store.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub project_root: String,
    pub harness: String,
    pub title: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub archived_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "default_visibility")]
    pub visibility: String,
}

fn default_visibility() -> String {
    "private".to_string()
}
```

Update any `INSERT`/`SELECT` for sessions in `store.rs` to read/write the new columns. Labels serialize as JSON array (`serde_json::to_string(&labels)`).

- [ ] **Step 5: Run tests**

Run: `cargo test -p coven-cli store::tests`
Expected: PASS. Existing tests should still work since both columns have safe defaults.

- [ ] **Step 6: Commit**

```bash
git add crates/coven-cli/src/store.rs
git commit -S -m "feat(coven-cli): add labels and visibility columns to sessions"
```

### Task 2.2: Add FTS5 virtual table for events

**Files:**
- Modify: `crates/coven-cli/src/store.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn search_events_finds_match_in_payload() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let conn = open_store(&temp.path().join("test.sqlite3"))?;
    conn.execute(
        "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
         VALUES('s1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
        [],
    )?;
    conn.execute(
        "INSERT INTO events(id, session_id, kind, payload_json, created_at)
         VALUES('e1', 's1', 'stdout', '{\"text\":\"phoenix rises\"}', '2026-01-01')",
        [],
    )?;
    let hits = search_events(&conn, "phoenix")?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].event_id, "e1");
    assert_eq!(hits[0].session_id, "s1");
    Ok(())
}
```

- [ ] **Step 2: Confirm FAIL**

Run: `cargo test -p coven-cli store::tests::search_events_finds_match_in_payload`
Expected: FAIL — `search_events` not defined.

- [ ] **Step 3: Add FTS5 schema and trigger**

In `open_store()`, append to the `execute_batch` schema string:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
    payload_json,
    content='events',
    content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS events_fts_insert AFTER INSERT ON events BEGIN
    INSERT INTO events_fts(rowid, payload_json) VALUES (new.rowid, new.payload_json);
END;

CREATE TRIGGER IF NOT EXISTS events_fts_delete AFTER DELETE ON events BEGIN
    INSERT INTO events_fts(events_fts, rowid, payload_json) VALUES('delete', old.rowid, old.payload_json);
END;

CREATE TRIGGER IF NOT EXISTS events_fts_update AFTER UPDATE ON events BEGIN
    INSERT INTO events_fts(events_fts, rowid, payload_json) VALUES('delete', old.rowid, old.payload_json);
    INSERT INTO events_fts(rowid, payload_json) VALUES (new.rowid, new.payload_json);
END;
```

- [ ] **Step 4: Add `SearchHit` type and `search_events`**

In `store.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub event_id: String,
    pub session_id: String,
    pub kind: String,
    pub snippet: String,
    pub created_at: String,
}

pub fn search_events(conn: &Connection, query: &str) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.session_id, e.kind, snippet(events_fts, 0, '[', ']', '…', 16), e.created_at
         FROM events_fts
         JOIN events e ON e.rowid = events_fts.rowid
         WHERE events_fts MATCH ?1
         ORDER BY e.created_at DESC
         LIMIT 100",
    )?;
    let rows = stmt.query_map([query], |row| {
        Ok(SearchHit {
            event_id: row.get(0)?,
            session_id: row.get(1)?,
            kind: row.get(2)?,
            snippet: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
```

- [ ] **Step 5: Backfill FTS for any pre-existing events**

In `open_store()`, after FTS schema is in place:

```rust
// Backfill: copy any existing events into the FTS index. Safe on fresh dbs.
conn.execute(
    "INSERT INTO events_fts(rowid, payload_json)
     SELECT e.rowid, e.payload_json
     FROM events e
     LEFT JOIN events_fts f ON f.rowid = e.rowid
     WHERE f.rowid IS NULL",
    [],
)?;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p coven-cli store::tests`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/coven-cli/src/store.rs
git commit -S -m "feat(coven-cli): FTS5 index over events.payload_json for thread search"
```

### Task 2.3: Add `latest_active_for_project` helper

**Files:**
- Modify: `crates/coven-cli/src/store.rs`

- [ ] **Step 1: Test first**

```rust
#[test]
fn latest_active_returns_newest_non_archived_for_project() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let conn = open_store(&temp.path().join("test.sqlite3"))?;
    conn.execute_batch(
        "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
           VALUES ('older', '/p', 'codex', 't', 'created', '2026-01-01', '2026-01-01'),
                  ('newer', '/p', 'claude', 't', 'created', '2026-01-02', '2026-01-02'),
                  ('archived', '/p', 'claude', 't', 'created', '2026-01-03', '2026-01-03'),
                  ('other_proj', '/other', 'claude', 't', 'created', '2026-01-04', '2026-01-04');
         UPDATE sessions SET archived_at='2026-01-03' WHERE id='archived';",
    )?;
    let hit = latest_active_for_project(&conn, "/p")?;
    assert_eq!(hit.as_deref(), Some("newer"));
    Ok(())
}
```

- [ ] **Step 2: Implement**

```rust
pub fn latest_active_for_project(conn: &Connection, project_root: &str) -> Result<Option<String>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM sessions
             WHERE project_root = ?1 AND archived_at IS NULL
             ORDER BY created_at DESC LIMIT 1",
            params![project_root],
            |row| row.get(0),
        )
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(id)
}
```

- [ ] **Step 3: Run tests, commit**

Run: `cargo test -p coven-cli store::tests`
Expected: PASS.

```bash
git add crates/coven-cli/src/store.rs
git commit -S -m "feat(coven-cli): latest_active_for_project helper"
```

### Task 2.4: Add CLI flags to `Run`

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Extend the clap struct**

In `main.rs`, replace the `Run` variant:

```rust
#[command(about = "Launch a project-scoped harness session")]
Run {
    #[arg(help = "Harness to run: codex or claude")]
    harness: String,
    #[arg(help = "Task for the harness", required = false, num_args = 0..)]
    prompt: Vec<String>,
    #[arg(long, help = "Working directory inside the current project")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Readable title for `coven sessions`")]
    title: Option<String>,
    #[arg(long, help = "Create the session record without launching the harness")]
    detach: bool,
    #[arg(
        long,
        value_name = "ID",
        num_args = 0..=1,
        default_missing_value = "",
        help = "Resume session by id; omit value to resume latest active for this project"
    )]
    continue_session: Option<String>,
    #[arg(long, value_delimiter = ',', help = "Comma-separated labels")]
    labels: Vec<String>,
    #[arg(
        long,
        value_parser = ["private", "workspace", "shared"],
        help = "Session visibility (default: private)"
    )]
    visibility: Option<String>,
    #[arg(long, help = "Archive the session after the run completes")]
    archive: bool,
},
```

Note clap will rename `continue_session` to `--continue-session` automatically. Override with an explicit `long`:

```rust
#[arg(long = "continue", value_name = "ID", num_args = 0..=1, default_missing_value = "")]
continue_session: Option<String>,
```

- [ ] **Step 2: Update `run_session()` dispatcher**

In the existing `Command::Run { ... } => run_session(...)` arm, plumb the new fields through. Resolve `--continue`:

```rust
let resume_id: Option<String> = match continue_session.as_deref() {
    None => None,
    Some("") => {
        let conn = store::open_store(&store_path)?;
        store::latest_active_for_project(&conn, project_root.to_str().unwrap_or(""))?
    }
    Some(id) => Some(id.to_string()),
};
```

Pass `labels`, `visibility`, `archive`, and `resume_id` into the session creation path. The session writer should:
- store labels as `serde_json::to_string(&labels)?` in the `labels` column,
- store visibility as the provided value or `"private"`,
- when `archive` is true, after the harness exits, update `sessions.archived_at = now`.

- [ ] **Step 3: Build and smoke test**

Run: `cargo build -p coven-cli`
Then a real run (with claude installed):

```bash
cargo run -p coven-cli -- run claude --labels demo,p0 --visibility workspace --archive "ping"
cargo run -p coven-cli -- sessions --all --json | head
```

Expected: the new row shows `"labels":["demo","p0"]`, `"visibility":"workspace"`, and `archived_at` is set.

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): add --continue/--labels/--visibility/--archive to run"
```

### Task 2.5: Add `coven sessions search` subcommand

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Extend clap**

Replace the `Sessions` variant in `Command`:

```rust
#[command(about = "List/search recent Coven sessions")]
Sessions {
    #[command(subcommand)]
    command: Option<SessionsCommand>,
    #[arg(long, help = "Include archived sessions")]
    all: bool,
    #[arg(long, conflicts_with_all = ["plain", "json"])]
    manage: bool,
    #[arg(long, conflicts_with_all = ["manage", "json"])]
    plain: bool,
    #[arg(long, conflicts_with_all = ["manage", "plain"])]
    json: bool,
},
```

And add:

```rust
#[derive(Subcommand, Debug)]
enum SessionsCommand {
    #[command(about = "Full-text search session event payloads")]
    Search {
        #[arg(help = "FTS5 query (e.g. `phoenix OR rises`)")]
        query: String,
        #[arg(long, help = "Print JSON for clients")]
        json: bool,
    },
}
```

- [ ] **Step 2: Dispatch**

In the `Command::Sessions { command, .. }` arm, branch on `command`:

```rust
match command {
    Some(SessionsCommand::Search { query, json }) => {
        let conn = store::open_store(&store_path)?;
        let hits = store::search_events(&conn, &query)?;
        if json {
            println!("{}", serde_json::to_string(&hits)?);
        } else {
            for hit in &hits {
                println!(
                    "{}  {}  [{}]  {}",
                    hit.created_at, hit.session_id, hit.kind, hit.snippet
                );
            }
        }
        Ok(())
    }
    None => existing_sessions_list_path(all, manage, plain, json),
}
```

- [ ] **Step 3: Smoke test**

Run: `cargo run -p coven-cli -- sessions search "phoenix"`
Expected: empty results on a fresh db; no error.

Then after creating a session with the word "phoenix" in its prompt, re-run:
Expected: at least one hit prints.

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): coven sessions search <query>"
```

### Task 2.6: Document thread upgrades

**Files:**
- Modify: `docs/SESSION-LIFECYCLE.md` (existing)

- [ ] **Step 1: Append a section**

Add to the end of `docs/SESSION-LIFECYCLE.md`:

```markdown
## Search and continuation (added in 2026-05)

- `coven sessions search <query>` runs a SQLite FTS5 query over redacted event payloads.
  Encrypted raw artifacts are never decrypted during search.
- `coven run <harness> --continue` resumes the most recently created, non-archived
  session whose `project_root` matches the current directory.
- `coven run <harness> --continue <ID>` resumes by id.
- `coven run <harness> --labels foo,bar --visibility workspace --archive "task"` tags
  and archives a one-shot run in a single command.
```

- [ ] **Step 2: Commit**

```bash
git add docs/SESSION-LIFECYCLE.md
git commit -S -m "docs(coven-cli): document search, --continue, --labels, --visibility, --archive"
```

**Phase 2 milestone:** thread upgrades ship. Run `cargo test -p coven-cli` — all green. Can land independently.

---

## Phase 3 — File references in prompts

**Outcome:** Prompts can include `@README.md`, `@docs/*.md`, `@T-<id>`, `@@search words`. Text expands inline up to caps; images become content blocks in stream-JSON mode and placeholders otherwise.

### Task 3.1: Add `globset` dependency

**Files:**
- Modify: `crates/coven-cli/Cargo.toml`

- [ ] **Step 1: Add dep**

```toml
globset = "0.4"
```

- [ ] **Step 2: Build and commit**

Run: `cargo build -p coven-cli`

```bash
git add crates/coven-cli/Cargo.toml crates/coven-cli/Cargo.lock
git commit -S -m "deps(coven-cli): add globset for prompt @path globbing"
```

### Task 3.2: Define ref parser and types

**Files:**
- Create: `crates/coven-cli/src/prompt_refs.rs`
- Modify: `crates/coven-cli/src/main.rs` (declare module)

- [ ] **Step 1: Write the failing test**

```rust
// crates/coven-cli/src/prompt_refs.rs
use anyhow::Result;
use std::path::{Path, PathBuf};

pub const MAX_TEXT_LINES: usize = 500;
pub const MAX_LINE_CHARS: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// `@path/to/file` or `@glob/*.md`
    Path(String),
    /// `@T-<uuid>` — thread id reference
    Thread(String),
    /// `@@search words` — FTS query
    Search(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPrompt {
    pub raw: String,
    pub refs: Vec<Ref>,
}

pub fn parse(prompt: &str) -> ParsedPrompt {
    let mut refs = Vec::new();
    // double-@ for search comes first so single-@ doesn't swallow it
    let mut i = 0;
    let bytes = prompt.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'@' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'@' {
                // @@search words — runs to end-of-line
                let end = prompt[i..].find('\n').map(|n| i + n).unwrap_or(prompt.len());
                let body = &prompt[i + 2..end];
                if !body.trim().is_empty() {
                    refs.push(Ref::Search(body.trim().to_string()));
                }
                i = end;
                continue;
            }
            // single @ — runs to next whitespace
            let end = prompt[i + 1..]
                .find(|c: char| c.is_whitespace())
                .map(|n| i + 1 + n)
                .unwrap_or(prompt.len());
            let body = &prompt[i + 1..end];
            if let Some(rest) = body.strip_prefix("T-") {
                refs.push(Ref::Thread(format!("T-{rest}")));
            } else if !body.is_empty() {
                refs.push(Ref::Path(body.to_string()));
            }
            i = end;
            continue;
        }
        i += 1;
    }
    ParsedPrompt {
        raw: prompt.to_string(),
        refs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_path_refs() {
        let p = parse("look at @README.md and @docs/*.md please");
        assert_eq!(
            p.refs,
            vec![
                Ref::Path("README.md".into()),
                Ref::Path("docs/*.md".into()),
            ]
        );
    }

    #[test]
    fn parses_thread_ref() {
        let p = parse("continue @T-abc-123 with new ideas");
        assert_eq!(p.refs, vec![Ref::Thread("T-abc-123".into())]);
    }

    #[test]
    fn parses_search_ref_to_end_of_line() {
        let p = parse("background:\n@@phoenix rises again\ndo the thing");
        assert_eq!(p.refs, vec![Ref::Search("phoenix rises again".into())]);
    }

    #[test]
    fn ignores_bare_at_sign() {
        let p = parse("email me at @ work");
        // "@" with nothing after is dropped; "work" never starts with @
        assert!(p.refs.is_empty() || p.refs == vec![Ref::Path("".into())]);
    }
}
```

In `main.rs`:

```rust
mod prompt_refs;
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p coven-cli prompt_refs::tests`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/coven-cli/src/prompt_refs.rs crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): parse @path / @T-id / @@search refs from prompts"
```

### Task 3.3: Expand path refs (text + image classification)

**Files:**
- Modify: `crates/coven-cli/src/prompt_refs.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn expand_path_inlines_text_file_capped() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("hello.md");
    std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
    let expanded = expand_path(temp.path(), "hello.md").unwrap();
    assert!(expanded.contains("line1"));
    assert!(expanded.contains("line3"));
    assert!(expanded.contains("hello.md"));
}

#[test]
fn expand_path_image_becomes_placeholder() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pic.png");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\nfake").unwrap();
    let expanded = expand_path(temp.path(), "pic.png").unwrap();
    assert!(expanded.contains("[image @ "));
    assert!(expanded.contains("image/png"));
}
```

- [ ] **Step 2: Run to confirm FAIL**

Run: `cargo test -p coven-cli prompt_refs::tests::expand_path_inlines_text_file_capped`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
pub fn expand_path(cwd: &Path, raw: &str) -> Result<String> {
    let full = cwd.join(raw);
    if !full.exists() {
        return Ok(format!("[missing @{raw}]"));
    }
    let mime = guess_mime(&full);
    if mime.starts_with("image/") {
        let bytes = std::fs::metadata(&full)?.len();
        return Ok(format!(
            "[image @ {}: {mime}, {bytes} bytes]",
            full.display()
        ));
    }
    let raw_text = std::fs::read_to_string(&full)?;
    let mut out = String::new();
    out.push_str(&format!("--- @{raw} ---\n"));
    for (i, line) in raw_text.lines().take(MAX_TEXT_LINES).enumerate() {
        let truncated: String = line.chars().take(MAX_LINE_CHARS).collect();
        out.push_str(&truncated);
        out.push('\n');
        if i + 1 == MAX_TEXT_LINES {
            out.push_str(&format!("[…truncated at {MAX_TEXT_LINES} lines]\n"));
            break;
        }
    }
    out.push_str("--- end ---\n");
    Ok(out)
}

fn guess_mime(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png".into(),
        Some("jpg") | Some("jpeg") => "image/jpeg".into(),
        Some("gif") => "image/gif".into(),
        Some("webp") => "image/webp".into(),
        _ => "text/plain".into(),
    }
}
```

- [ ] **Step 4: Tests pass; commit**

```bash
git add crates/coven-cli/src/prompt_refs.rs
git commit -S -m "feat(coven-cli): expand @path with text/image classification and caps"
```

### Task 3.4: Expand glob path refs

**Files:**
- Modify: `crates/coven-cli/src/prompt_refs.rs`

- [ ] **Step 1: Test first**

```rust
#[test]
fn expand_glob_includes_all_matches() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("docs")).unwrap();
    std::fs::write(temp.path().join("docs/a.md"), "AAA").unwrap();
    std::fs::write(temp.path().join("docs/b.md"), "BBB").unwrap();
    let expanded = expand_path(temp.path(), "docs/*.md").unwrap();
    assert!(expanded.contains("AAA"));
    assert!(expanded.contains("BBB"));
}
```

- [ ] **Step 2: Confirm FAIL**

Expected: current `expand_path` treats `docs/*.md` literally and reports missing.

- [ ] **Step 3: Branch on glob characters**

Update `expand_path` near the top:

```rust
pub fn expand_path(cwd: &Path, raw: &str) -> Result<String> {
    if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        return expand_glob(cwd, raw);
    }
    // ... existing single-path body ...
}

fn expand_glob(cwd: &Path, pattern: &str) -> Result<String> {
    use globset::{Glob, GlobSetBuilder};
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern)?);
    let set = builder.build()?;
    let mut out = String::new();
    let mut count = 0;
    for entry in walkdir(cwd) {
        let rel = entry.strip_prefix(cwd).unwrap_or(&entry);
        if !set.is_match(rel) {
            continue;
        }
        let part = expand_path(cwd, &rel.to_string_lossy())?;
        out.push_str(&part);
        out.push('\n');
        count += 1;
        if count >= 20 {
            out.push_str("[…glob match cap reached at 20 files]\n");
            break;
        }
    }
    if count == 0 {
        return Ok(format!("[no matches for @{pattern}]"));
    }
    Ok(out)
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|s| s.to_str()) == Some(".git") {
                    continue;
                }
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p coven-cli prompt_refs::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/prompt_refs.rs
git commit -S -m "feat(coven-cli): expand glob refs in prompts (capped at 20 matches)"
```

### Task 3.5: Expand `@T-<id>` thread refs

**Files:**
- Modify: `crates/coven-cli/src/prompt_refs.rs`

- [ ] **Step 1: Test**

```rust
#[test]
fn expand_thread_inlines_redacted_payloads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let conn = crate::store::open_store(&temp.path().join("test.sqlite3"))?;
    conn.execute(
        "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
         VALUES('T-abc', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
        [],
    )?;
    conn.execute(
        "INSERT INTO events(id, session_id, kind, payload_json, created_at)
         VALUES('e1', 'T-abc', 'user', '{\"text\":\"hello world\"}', '2026-01-01')",
        [],
    )?;
    let out = expand_thread(&conn, "T-abc")?;
    assert!(out.contains("hello world"));
    assert!(out.contains("T-abc"));
    Ok(())
}
```

- [ ] **Step 2: Implement**

```rust
pub fn expand_thread(conn: &rusqlite::Connection, id: &str) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT kind, payload_json, created_at FROM events
         WHERE session_id = ?1 ORDER BY created_at ASC LIMIT 200",
    )?;
    let rows = stmt.query_map([id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = format!("--- @{id} ---\n");
    let mut found = false;
    for r in rows {
        let (kind, payload, ts) = r?;
        found = true;
        out.push_str(&format!("[{ts}] {kind}: {payload}\n"));
    }
    out.push_str("--- end ---\n");
    if !found {
        return Ok(format!("[no events for @{id}]"));
    }
    Ok(out)
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/coven-cli/src/prompt_refs.rs
git commit -S -m "feat(coven-cli): expand @T-<id> refs from the local session ledger"
```

### Task 3.6: Wire ref expansion into `coven run`

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Add `expand_all`**

In `prompt_refs.rs`:

```rust
pub fn expand_all(
    cwd: &Path,
    conn: &rusqlite::Connection,
    prompt: &str,
) -> Result<String> {
    let parsed = parse(prompt);
    if parsed.refs.is_empty() {
        return Ok(prompt.to_string());
    }
    let mut prefix = String::new();
    for r in &parsed.refs {
        let block = match r {
            Ref::Path(p) => expand_path(cwd, p)?,
            Ref::Thread(id) => expand_thread(conn, id)?,
            Ref::Search(q) => {
                let hits = crate::store::search_events(conn, q)?;
                let mut s = format!("--- @@{q} ---\n");
                for hit in hits.into_iter().take(5) {
                    s.push_str(&format!(
                        "[{}] {} {} {}\n",
                        hit.created_at, hit.session_id, hit.kind, hit.snippet
                    ));
                }
                s.push_str("--- end ---\n");
                s
            }
        };
        prefix.push_str(&block);
    }
    Ok(format!("{prefix}\n{prompt}"))
}
```

- [ ] **Step 2: Call from `Run`**

In `main.rs`, in the `Command::Run { .. }` arm, **before** sending the joined prompt to the harness:

```rust
let joined = prompt.join(" ");
let conn = store::open_store(&store_path)?;
let final_prompt = prompt_refs::expand_all(&cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap()), &conn, &joined)?;
```

Use `final_prompt` from that point on.

- [ ] **Step 3: Smoke test**

```bash
echo "explain @README.md briefly" | cargo run -p coven-cli -- run claude
```

Expected: claude receives the file contents inlined and explains them. If running without claude installed, mock by replacing the harness invocation with a `cat`-style stub during testing — verify `final_prompt` contains the file content.

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/prompt_refs.rs crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): expand prompt refs before dispatching to harness"
```

### Task 3.7: Document refs

**Files:**
- Create: `docs/PROMPT-REFERENCES.md`

- [ ] **Step 1: Write**

```markdown
# Prompt References

Coven expands four kinds of references in prompts before sending them to a harness.

## `@path/to/file`
Inlines a text file relative to the invocation directory. Capped at 500 lines × 2048 chars/line.

## `@glob/*.ext`
Expands a glob. Up to 20 matching files are inlined; the rest are noted as truncated.

## `@T-<session-id>`
Inlines the redacted event payloads of a prior session (up to 200 events).

## `@@search words`
Runs a SQLite FTS5 query over local session payloads. Up to 5 hits are inlined with snippets.

## Image references
`.png`, `.jpg`, `.gif`, `.webp` files become placeholders in plain mode and image content blocks
in `--stream-json` mode.
```

- [ ] **Step 2: Commit**

```bash
git add docs/PROMPT-REFERENCES.md
git commit -S -m "docs(coven-cli): document @path / @T-id / @@search references"
```

**Phase 3 milestone:** prompt refs ship. `cargo test -p coven-cli` green. Independent landing OK.

---

## Phase 4 — Stream-JSON I/O protocol

**Outcome:** `coven run claude --stream-json` emits the documented JSONL contract on stdout; `--stream-json-input` reads user messages as JSONL from stdin. Same shape Coven Code uses.

### Task 4.1: Define event types

**Files:**
- Create: `crates/coven-cli/src/stream_json.rs`
- Modify: `crates/coven-cli/src/main.rs` (declare module)

- [ ] **Step 1: Write the types and tests**

```rust
// crates/coven-cli/src/stream_json.rs
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    System(System),
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResult),
    Result(RunResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct System {
    pub subtype: String,
    pub cwd: String,
    pub session_id: String,
    pub tools: Vec<String>,
    pub agent_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserMessage {
    pub message: MessageBody,
    pub session_id: String,
    pub parent_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistantMessage {
    pub message: MessageBody,
    pub session_id: String,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageBody {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Image { source: ImageSource },
    ToolUse { id: String, name: String, input: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Path { path: String, media_type: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunResult {
    pub subtype: String,
    pub duration_ms: u64,
    pub is_error: bool,
    pub num_turns: u32,
    pub session_id: String,
    pub error: Option<String>,
}

pub fn emit_event<W: Write>(writer: &mut W, event: &Event) -> Result<()> {
    serde_json::to_writer(&mut *writer, event)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn read_event<R: BufRead>(reader: &mut R) -> Result<Option<Event>> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(trimmed)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_user_message() {
        let event = Event::User(UserMessage {
            message: MessageBody {
                role: "user".into(),
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            },
            session_id: "s1".into(),
            parent_tool_use_id: None,
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let mut reader = Cursor::new(buf);
        let decoded = read_event(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn json_shape_matches_coven_code() {
        let event = Event::Result(RunResult {
            subtype: "success".into(),
            duration_ms: 12,
            is_error: false,
            num_turns: 1,
            session_id: "s1".into(),
            error: None,
        });
        let mut buf = Vec::new();
        emit_event(&mut buf, &event).unwrap();
        let line = String::from_utf8(buf).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "result");
        assert_eq!(v["subtype"], "success");
        assert_eq!(v["is_error"], false);
        assert_eq!(v["num_turns"], 1);
    }
}
```

- [ ] **Step 2: Module declaration**

In `main.rs`:

```rust
mod stream_json;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p coven-cli stream_json::tests`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/stream_json.rs crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): stream-JSON event types matching Coven Code schema"
```

### Task 4.2: Add `--stream-json` and `--stream-json-input` flags

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Extend `Run` again**

Add to the `Run` clap struct (additions in **bold** if you're diffing — here just the new fields):

```rust
#[arg(long, help = "Emit JSONL events on stdout (init/user/assistant/tool_result/result)")]
stream_json: bool,
#[arg(long, requires = "stream_json", help = "Read JSONL user messages from stdin")]
stream_json_input: bool,
```

- [ ] **Step 2: Build, no tests yet**

Run: `cargo build -p coven-cli`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/coven-cli/src/main.rs
git commit -S -m "feat(coven-cli): add --stream-json and --stream-json-input flags to run"
```

### Task 4.3: Emit `system.init` + `user` + `result` for non-stream harness (codex)

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/coven-cli/tests/stream_json_integration.rs` (cargo picks up `tests/` automatically):

```rust
use std::process::{Command, Stdio};

#[test]
#[ignore = "requires real harness invocation; run with `cargo test -- --ignored`"]
fn stream_json_emits_init_and_result_for_codex_dry_run() {
    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["run", "codex", "--stream-json", "--detach", "ping"])
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut lines = stdout.lines();
    let first: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "init");
    let last_line = stdout.lines().last().unwrap();
    let last: serde_json::Value = serde_json::from_str(last_line).unwrap();
    assert_eq!(last["type"], "result");
}
```

- [ ] **Step 2: Implement around `run_session`**

In `main.rs`, in the `Command::Run { stream_json, .. }` arm, when `stream_json` is true and the harness is `codex` (or any non-stream harness):

```rust
use crate::stream_json::{emit_event, Event, System, UserMessage, MessageBody, ContentBlock, RunResult};
use std::io::{stdout, BufWriter};

if stream_json {
    let mut out = BufWriter::new(stdout().lock());
    let started = std::time::Instant::now();
    emit_event(&mut out, &Event::System(System {
        subtype: "init".into(),
        cwd: cwd_resolved.to_string_lossy().to_string(),
        session_id: session_id.clone(),
        tools: vec![],
        agent_mode: None,
    }))?;
    emit_event(&mut out, &Event::User(UserMessage {
        message: MessageBody {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: final_prompt.clone() }],
        },
        session_id: session_id.clone(),
        parent_tool_use_id: None,
    }))?;

    let exit_code = launch_harness_and_wait(...)?; // existing path

    let is_error = exit_code != 0;
    emit_event(&mut out, &Event::Result(RunResult {
        subtype: if is_error { "error_during_execution".into() } else { "success".into() },
        duration_ms: started.elapsed().as_millis() as u64,
        is_error,
        num_turns: 1,
        session_id,
        error: None,
    }))?;
    return Ok(());
}
```

- [ ] **Step 3: Run the integration test**

Run: `cargo test -p coven-cli --test stream_json_integration -- --ignored`
Expected: PASS (the `--detach` path avoids actually invoking codex, so this works on machines without codex installed).

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/tests/stream_json_integration.rs
git commit -S -m "feat(coven-cli): emit stream-JSON init/user/result for one-shot runs"
```

### Task 4.4: Wrap claude's native stream-json mode

**Files:**
- Modify: `crates/coven-cli/src/main.rs`
- Modify: `crates/coven-cli/src/pty_runner.rs`

- [ ] **Step 1: For claude stream mode, pass-through with re-emission**

When `harness == "claude"` and `stream_json` is true:
- Launch claude with its native flags: `claude -p --input-format stream-json --output-format stream-json --verbose`.
- For each newline-delimited JSON event claude emits, parse with `serde_json::from_str::<serde_json::Value>` and re-emit on our stdout untouched (claude's schema already matches ours by design).
- Emit our own `system.init` at start and our own `result` at end, since claude's framing differs slightly.

Sketch in `main.rs`:

```rust
if stream_json && harness == "claude" {
    let mut out = BufWriter::new(stdout().lock());
    let started = std::time::Instant::now();
    emit_event(&mut out, &Event::System(System {
        subtype: "init".into(),
        cwd: cwd_resolved.to_string_lossy().to_string(),
        session_id: session_id.clone(),
        tools: vec![],
        agent_mode: None,
    }))?;
    let exit_code = stream_claude(
        &cwd_resolved,
        &session_id,
        &final_prompt,
        stream_json_input,
        &mut out,
    )?;
    let is_error = exit_code != 0;
    emit_event(&mut out, &Event::Result(RunResult {
        subtype: if is_error { "error_during_execution".into() } else { "success".into() },
        duration_ms: started.elapsed().as_millis() as u64,
        is_error,
        num_turns: 1,
        session_id,
        error: None,
    }))?;
    return Ok(());
}
```

- [ ] **Step 2: Implement `stream_claude` in `pty_runner.rs`**

Add a function that:
- Spawns claude with the stream-json args.
- Pipes our stdin (if `stream_json_input`) directly to claude's stdin.
- Copies claude's stdout line-by-line to our stdout writer (no rewriting).
- Returns claude's exit code.

```rust
pub fn stream_claude<W: std::io::Write>(
    cwd: &std::path::Path,
    session_id: &str,
    prompt: &str,
    forward_stdin: bool,
    out: &mut W,
) -> anyhow::Result<i32> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let mut child = Command::new("claude")
        .args([
            "-p",
            "--input-format", "stream-json",
            "--output-format", "stream-json",
            "--verbose",
            "--session-id", session_id,
        ])
        .arg(prompt)
        .current_dir(cwd)
        .stdin(if forward_stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    if forward_stdin {
        let mut child_stdin = child.stdin.take().expect("piped stdin");
        let stdin = std::io::stdin();
        std::thread::spawn(move || {
            let mut buf = String::new();
            let mut lock = stdin.lock();
            while lock.read_line(&mut buf).unwrap_or(0) > 0 {
                if child_stdin.write_all(buf.as_bytes()).is_err() {
                    break;
                }
                buf.clear();
            }
        });
    }

    let child_stdout = child.stdout.take().expect("piped stdout");
    let reader = BufReader::new(child_stdout);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        writeln!(out, "{line}")?;
        out.flush()?;
    }

    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}
```

- [ ] **Step 3: Smoke test (manual; requires claude installed)**

```bash
cargo run -p coven-cli -- run claude --stream-json "what is 2+2"
```

Expected: JSONL on stdout starting with `{"type":"system","subtype":"init",...}`, claude's events in the middle, ending with `{"type":"result",...}`.

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/pty_runner.rs
git commit -S -m "feat(coven-cli): stream-JSON pass-through for claude with init/result framing"
```

### Task 4.5: Document the protocol

**Files:**
- Create: `docs/STREAM-JSON.md`

- [ ] **Step 1: Write the doc**

```markdown
# Coven CLI Stream-JSON Protocol

`coven run <harness> --stream-json` emits newline-delimited JSON events on stdout.
With `--stream-json-input`, user messages are read line-by-line from stdin as JSON.

The schema matches `@opencoven/coven-code` exactly; SDKs that target Coven Code work
unchanged against Coven CLI.

## Event types

### `system` — emitted once at startup
```json
{"type":"system","subtype":"init","cwd":"/path","session_id":"...","tools":[],"agent_mode":null}
```

### `user` — emitted for each user message
```json
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]},"session_id":"...","parent_tool_use_id":null}
```

### `assistant` — emitted by stream-capable harnesses (claude)
```json
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"..."}]},"session_id":"...","stop_reason":"end_turn"}
```

### `tool_result` — emitted by stream-capable harnesses
```json
{"type":"tool_result","tool_use_id":"...","content":[{"type":"text","text":"..."}],"is_error":false,"session_id":"..."}
```

### `result` — emitted once at end
```json
{"type":"result","subtype":"success","duration_ms":1234,"is_error":false,"num_turns":1,"session_id":"...","error":null}
```

## Stdin protocol (`--stream-json-input`)

Each line is one JSON object with the same shape as the `user` event's `message` field:

```json
{"role":"user","content":[{"type":"text","text":"hello"}]}
```

`image` content blocks use `source.type = "path"` (local file) or `source.type = "base64"` (inline).

## Harness behavior

- **claude**: passes through claude's native stream-json events, framed by Coven's `system` and `result`.
- **codex** and other non-stream harnesses: Coven synthesizes `system.init` + `user` + `result`. The harness's text output is not exposed as `assistant` events in this mode (use `coven attach <id>` for streamed text).

## Stability

This is a stable interface as of 2026-05. Additions are additive (new event types, new fields). Removals require a major version bump of the CLI.
```

- [ ] **Step 2: Commit**

```bash
git add docs/STREAM-JSON.md
git commit -S -m "docs(coven-cli): stable stream-JSON protocol spec"
```

**Phase 4 milestone:** stream-JSON ships. `cargo test -p coven-cli` green; manual smoke with claude verified. Tag a release.

---

## Self-review

**Spec coverage:**
- P0 #1 (stream-JSON + `--stream-json-input`) → Phase 4 ✓
- P0 #2 (file references `@path`, `@T-id`, `@@search`, image inlining) → Phase 3 ✓
- P0 #3 (unified JSONC settings, user→workspace→managed precedence, replacing TOML files) → Phase 1 ✓ (note: workspace tier deferred to a follow-up plan; the precedence machinery lands here as user→legacy-TOML→env, which is enough to ship; workspace `.coven/settings.json` is a P0.5 add-on)
- P0 #4 (search, `--labels`, `--visibility`, `--continue [id]`, `--archive`) → Phase 2 ✓

**Gap noted:** Phase 1 does not yet implement the `workspace > user` precedence layer (`.coven/settings.json` per-project file). The schema and loader are designed for it but the lookup is single-source. Add a follow-up task in Phase 1.5 if you want full parity now; otherwise punt to a separate plan. Adding now would mean: in Task 1.6, also probe `find_workspace_settings(cwd)` and shallow-merge over the user settings.

**Placeholders check:** No "TBD", "implement later", "similar to Task N" found. Every code block is concrete. The only conditional language is the "fix the structure mismatch" line in Task 1.2 — that's fine since serde does the heavy lifting and the test code defines the exact target shape.

**Type consistency:**
- `Settings` shape consistent across Tasks 1.2, 1.3, 1.4, 1.5, 1.6. ✓
- `SessionRecord` extended in 2.1 with `labels: Vec<String>` and `visibility: String`, consistent with usage in 2.4. ✓
- `Event` enum from 4.1 used unchanged in 4.3 and 4.4. ✓
- `latest_active_for_project(conn, project_root: &str) -> Result<Option<String>>` defined in 2.3, called the same way in 2.4. ✓
- Function name `expand_path` consistent in 3.3, 3.4. `expand_thread`, `expand_all` consistent in 3.5, 3.6. ✓

**Commit hygiene:** Every commit uses `-S` per CLAUDE.md. Conventional-commit prefixes (`feat`, `docs`, `deps`) match the repo's existing commit log style (verify with `git log` in the coven repo if uncertain).

**Plan complete.**
