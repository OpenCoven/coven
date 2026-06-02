//! Harness-native capability discovery.
//!
//! Scans well-known harness config directories and returns a
//! [`HarnessCapabilityManifest`] per installed harness.  Coven is a
//! *reader* only — no harness-native file is created, modified, or deleted
//! by this module.
//!
//! Route surface: `GET /capabilities`, `GET /capabilities/:harness_id`
//! (both accept `?refresh=1`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct GlobalInstructions {
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessSkill {
    pub id: String,
    pub name: String,
    pub source: &'static str,
    pub harness_id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessPlugin {
    pub id: String,
    pub name: String,
    pub source: &'static str,
    pub harness_id: String,
    pub kind: String,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityWarning {
    pub kind: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessCapabilityManifest {
    pub harness_id: String,
    pub scanned_at: String,
    pub global_instructions: GlobalInstructions,
    pub skills: Vec<HarnessSkill>,
    pub plugins: Vec<HarnessPlugin>,
    pub warnings: Vec<CapabilityWarning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesResponse {
    pub coven_skills: Vec<crate::cockpit_sources::SkillDto>,
    pub harness_capabilities: Vec<HarnessCapabilityManifest>,
    pub scanned_at: String,
}

// ── Cache ─────────────────────────────────────────────────────────────────────

const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

struct CapabilityCache {
    manifests: HashMap<String, HarnessCapabilityManifest>,
    built_at: Instant,
}

static CACHE: OnceLock<Mutex<Option<CapabilityCache>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CapabilityCache>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn utc_now_iso() -> String {
    // Use SystemTime → epoch seconds → ISO-8601 UTC without chrono.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Format as YYYY-MM-DDTHH:MM:SSZ (UTC, no sub-second precision).
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400; // days since 1970-01-01
                             // Civil calendar computation (Gregorian).
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns all harness manifests, using the cache when fresh.
/// Pass `refresh = true` to force a re-scan.
pub fn get_all(coven_home: &Path, refresh: bool) -> CapabilitiesResponse {
    let home = dirs_home();
    {
        let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
        if !refresh {
            if let Some(ref c) = *guard {
                if c.built_at.elapsed() < CACHE_TTL {
                    let manifests: Vec<_> = c.manifests.values().cloned().collect();
                    let coven_skills =
                        crate::cockpit_sources::scan_skills(coven_home).unwrap_or_default();
                    return CapabilitiesResponse {
                        coven_skills,
                        harness_capabilities: manifests,
                        scanned_at: utc_now_iso(),
                    };
                }
            }
        }
        // Re-scan.
        let codex = scan_codex_capabilities(&home);
        let claude = scan_claude_capabilities(&home);
        let mut manifests = HashMap::new();
        manifests.insert("codex".to_string(), codex);
        manifests.insert("claude".to_string(), claude);
        *guard = Some(CapabilityCache {
            manifests,
            built_at: Instant::now(),
        });
    }
    let guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    let manifests: Vec<_> = guard
        .as_ref()
        .unwrap()
        .manifests
        .values()
        .cloned()
        .collect();
    let coven_skills = crate::cockpit_sources::scan_skills(coven_home).unwrap_or_default();
    CapabilitiesResponse {
        coven_skills,
        harness_capabilities: manifests,
        scanned_at: utc_now_iso(),
    }
}

/// Returns a single harness manifest, or `None` if the harness id is unknown.
pub fn get_one(
    coven_home: &Path,
    harness_id: &str,
    refresh: bool,
) -> Option<HarnessCapabilityManifest> {
    // Ensure the cache is warm first.
    get_all(coven_home, refresh);
    let guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref()?.manifests.get(harness_id).cloned()
}

/// Invalidate the cache (e.g. on SIGHUP).
#[allow(dead_code)]
pub fn invalidate() {
    let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

// ── Scanners ──────────────────────────────────────────────────────────────────

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Scan `~/.codex/` for Codex-native capabilities.
pub fn scan_codex_capabilities(home_dir: &Path) -> HarnessCapabilityManifest {
    let base = home_dir.join(".codex");
    let now = utc_now_iso();
    let mut warnings: Vec<CapabilityWarning> = Vec::new();

    // Global instructions: ~/.codex/AGENTS.md
    let agents_md = base.join("AGENTS.md");
    let global_instructions = probe_instructions(&agents_md);

    // Automations: ~/.codex/automations/*/
    let mut skills: Vec<HarnessSkill> = Vec::new();
    let automations_dir = base.join("automations");
    if let Ok(entries) = fs::read_dir(&automations_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = entry.path();
            let id = entry.file_name().to_string_lossy().into_owned();
            let (name, description, version, tags) = parse_automation_toml(&dir, &mut warnings);
            skills.push(HarnessSkill {
                id,
                name,
                source: "harness-native",
                harness_id: "codex".to_string(),
                path: dir.to_string_lossy().into_owned(),
                description,
                version,
                tags,
            });
        }
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "codex".to_string(),
        scanned_at: now,
        global_instructions,
        skills,
        plugins: Vec::new(),
        warnings,
    }
}

/// Scan `~/.claude/` for Claude-native capabilities.
pub fn scan_claude_capabilities(home_dir: &Path) -> HarnessCapabilityManifest {
    let base = home_dir.join(".claude");
    let now = utc_now_iso();
    let mut warnings: Vec<CapabilityWarning> = Vec::new();

    // Global instructions: ~/.claude/CLAUDE.md
    let claude_md = base.join("CLAUDE.md");
    let global_instructions = probe_instructions(&claude_md);

    // MCP servers: ~/.claude/claude_desktop_config.json (XDG fallback:
    // ~/.config/claude/claude_desktop_config.json)
    let config_paths = [
        base.join("claude_desktop_config.json"),
        home_dir
            .join(".config")
            .join("claude")
            .join("claude_desktop_config.json"),
    ];
    let mut plugins: Vec<HarnessPlugin> = Vec::new();
    for config_path in &config_paths {
        if !config_path.exists() {
            continue;
        }
        match fs::read_to_string(config_path) {
            Ok(raw) => {
                match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(v) => {
                        if let Some(servers) = v.get("mcpServers").and_then(|s| s.as_object()) {
                            for (id, cfg) in servers {
                                let disabled = cfg
                                    .get("disabled")
                                    .and_then(|d| d.as_bool())
                                    .unwrap_or(false);
                                let command = cfg
                                    .get("command")
                                    .and_then(|c| c.as_str())
                                    .map(str::to_owned);
                                let args = cfg.get("args").and_then(|a| a.as_array()).map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(str::to_owned))
                                        .collect::<Vec<_>>()
                                });
                                plugins.push(HarnessPlugin {
                                    id: id.clone(),
                                    name: id.clone(),
                                    source: "harness-native",
                                    harness_id: "claude".to_string(),
                                    kind: "mcp".to_string(),
                                    enabled: !disabled,
                                    transport: Some("stdio".to_string()),
                                    command,
                                    args,
                                });
                            }
                        }
                        break; // Found and parsed; no need to try the fallback.
                    }
                    Err(err) => {
                        warnings.push(CapabilityWarning {
                            kind: "parse_error".to_string(),
                            path: config_path.to_string_lossy().into_owned(),
                            message: format!("could not parse claude_desktop_config.json: {err}"),
                        });
                        break;
                    }
                }
            }
            Err(err) => {
                warnings.push(CapabilityWarning {
                    kind: "permission_denied".to_string(),
                    path: config_path.to_string_lossy().into_owned(),
                    message: format!("could not read claude_desktop_config.json: {err}"),
                });
                break;
            }
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "claude".to_string(),
        scanned_at: now,
        global_instructions,
        skills: Vec::new(),
        plugins,
        warnings,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn probe_instructions(path: &Path) -> GlobalInstructions {
    match fs::metadata(path) {
        Ok(m) => GlobalInstructions {
            present: true,
            path: Some(path.to_string_lossy().into_owned()),
            byte_count: Some(m.len()),
        },
        Err(_) => GlobalInstructions {
            present: false,
            path: None,
            byte_count: None,
        },
    }
}

#[derive(Debug, Deserialize)]
struct AutomationToml {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn parse_automation_toml(
    dir: &Path,
    warnings: &mut Vec<CapabilityWarning>,
) -> (String, Option<String>, Option<String>, Vec<String>) {
    let toml_path = dir.join("automation.toml");
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());

    match fs::read_to_string(&toml_path) {
        Ok(raw) => match toml::from_str::<AutomationToml>(&raw) {
            Ok(t) => (
                t.name.unwrap_or_else(|| id.clone()),
                t.description,
                t.version,
                t.tags,
            ),
            Err(err) => {
                warnings.push(CapabilityWarning {
                    kind: "parse_error".to_string(),
                    path: toml_path.to_string_lossy().into_owned(),
                    message: format!("could not parse automation.toml: {err}"),
                });
                (id, None, None, Vec::new())
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // No automation.toml is expected and fine — use dir name as label.
            (id, None, None, Vec::new())
        }
        Err(err) => {
            warnings.push(CapabilityWarning {
                kind: "permission_denied".to_string(),
                path: toml_path.to_string_lossy().into_owned(),
                message: format!("could not read automation.toml: {err}"),
            });
            (id, None, None, Vec::new())
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // ── Codex ──────────────────────────────────────────────────────────────

    #[test]
    fn scan_codex_returns_empty_manifest_when_dir_missing() {
        let home = tmp();
        let m = scan_codex_capabilities(home.path());
        assert_eq!(m.harness_id, "codex");
        assert!(!m.global_instructions.present);
        assert!(m.skills.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_codex_detects_agents_md() {
        let home = tmp();
        let codex_dir = home.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(codex_dir.join("AGENTS.md"), "# Rules\n\nBe careful.").unwrap();
        let m = scan_codex_capabilities(home.path());
        assert!(m.global_instructions.present);
        assert!(m
            .global_instructions
            .path
            .as_deref()
            .unwrap()
            .ends_with("AGENTS.md"));
        assert_eq!(m.global_instructions.byte_count, Some(20));
    }

    #[test]
    fn scan_codex_scans_automations_subdirectories() {
        let home = tmp();
        let autos = home.path().join(".codex").join("automations");
        fs::create_dir_all(&autos).unwrap();
        // With automation.toml
        let a1 = autos.join("daily-bug-scan");
        fs::create_dir_all(&a1).unwrap();
        fs::write(
            a1.join("automation.toml"),
            r#"name = "Daily bug scan"
description = "Scans PRs each morning."
version = "1.0.0"
tags = ["bugs", "daily"]"#,
        )
        .unwrap();
        // Without automation.toml (should still be discovered)
        let a2 = autos.join("tidy-crates");
        fs::create_dir_all(&a2).unwrap();

        let m = scan_codex_capabilities(home.path());
        assert_eq!(m.skills.len(), 2);
        let bug_scan = m.skills.iter().find(|s| s.id == "daily-bug-scan").unwrap();
        assert_eq!(bug_scan.name, "Daily bug scan");
        assert_eq!(
            bug_scan.description.as_deref(),
            Some("Scans PRs each morning.")
        );
        assert_eq!(bug_scan.version.as_deref(), Some("1.0.0"));
        assert_eq!(bug_scan.tags, vec!["bugs", "daily"]);
        let tidy = m.skills.iter().find(|s| s.id == "tidy-crates").unwrap();
        // Falls back to dir name when no toml
        assert_eq!(tidy.name, "tidy-crates");
        assert!(tidy.description.is_none());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_codex_tolerates_malformed_automation_toml() {
        let home = tmp();
        let a = home
            .path()
            .join(".codex")
            .join("automations")
            .join("broken");
        fs::create_dir_all(&a).unwrap();
        fs::write(a.join("automation.toml"), "NOT VALID TOML [[[").unwrap();
        let m = scan_codex_capabilities(home.path());
        assert_eq!(m.skills.len(), 1);
        assert_eq!(m.skills[0].name, "broken"); // falls back to dir name
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].kind, "parse_error");
    }

    // ── Claude ─────────────────────────────────────────────────────────────

    #[test]
    fn scan_claude_returns_empty_manifest_when_dir_missing() {
        let home = tmp();
        let m = scan_claude_capabilities(home.path());
        assert_eq!(m.harness_id, "claude");
        assert!(!m.global_instructions.present);
        assert!(m.plugins.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_claude_detects_claude_md() {
        let home = tmp();
        let claude_dir = home.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(claude_dir.join("CLAUDE.md"), "# Global rules\n").unwrap();
        let m = scan_claude_capabilities(home.path());
        assert!(m.global_instructions.present);
        assert!(m
            .global_instructions
            .path
            .as_deref()
            .unwrap()
            .ends_with("CLAUDE.md"));
    }

    #[test]
    fn scan_claude_parses_mcp_servers() {
        let home = tmp();
        let claude_dir = home.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(
            claude_dir.join("claude_desktop_config.json"),
            r#"{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/alice"],
      "disabled": false
    },
    "sqlite": {
      "command": "uvx",
      "args": ["mcp-server-sqlite", "--db-path", "/tmp/test.db"],
      "disabled": true
    }
  }
}"#,
        )
        .unwrap();
        let m = scan_claude_capabilities(home.path());
        assert_eq!(m.plugins.len(), 2);
        let fs_plugin = m.plugins.iter().find(|p| p.id == "filesystem").unwrap();
        assert_eq!(fs_plugin.kind, "mcp");
        assert!(fs_plugin.enabled);
        assert_eq!(fs_plugin.command.as_deref(), Some("npx"));
        let sqlite = m.plugins.iter().find(|p| p.id == "sqlite").unwrap();
        assert!(!sqlite.enabled);
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_claude_tolerates_malformed_mcp_json() {
        let home = tmp();
        let claude_dir = home.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(
            claude_dir.join("claude_desktop_config.json"),
            "{{{INVALID}}}",
        )
        .unwrap();
        let m = scan_claude_capabilities(home.path());
        assert!(m.plugins.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].kind, "parse_error");
    }

    #[test]
    fn scan_claude_tries_xdg_fallback_path() {
        let home = tmp();
        // Only place the config at the XDG path, not the primary path.
        let xdg = home.path().join(".config").join("claude");
        fs::create_dir_all(&xdg).unwrap();
        fs::write(
            xdg.join("claude_desktop_config.json"),
            r#"{"mcpServers": {"test": {"command": "echo"}}}"#,
        )
        .unwrap();
        let m = scan_claude_capabilities(home.path());
        assert_eq!(m.plugins.len(), 1);
        assert_eq!(m.plugins[0].id, "test");
    }
}
