//! Harness-native capability discovery.
//!
//! Scans well-known harness config directories and returns a
//! [`HarnessCapabilityManifest`] per installed harness.  Coven is a
//! *reader* only — no harness-native file is created, modified, or deleted
//! by this module.
//!
//! Route surface: `GET /capabilities/harnesses`,
//! `GET /capabilities/:harness_id` (both accept `?refresh=1`).
//! `GET /capabilities` itself is the control-plane capability catalog
//! (`control_plane::capabilities`), not this module.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    coven_home: PathBuf,
    harness_home: PathBuf,
}

impl CacheKey {
    fn new(coven_home: PathBuf, harness_home: PathBuf) -> Self {
        Self {
            coven_home,
            harness_home,
        }
    }
}

#[derive(Clone)]
struct CapabilitySnapshot {
    response: CapabilitiesResponse,
    built_at: Instant,
}

static CACHE: OnceLock<RwLock<HashMap<CacheKey, CapabilitySnapshot>>> = OnceLock::new();

fn cache() -> &'static RwLock<HashMap<CacheKey, CapabilitySnapshot>> {
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn get_or_refresh(
    key: CacheKey,
    refresh: bool,
    build: impl FnOnce() -> CapabilitiesResponse,
) -> CapabilitiesResponse {
    if !refresh {
        let guard = cache()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(snapshot) = guard
            .get(&key)
            .filter(|snapshot| snapshot.built_at.elapsed() < CACHE_TTL)
        {
            return snapshot.response.clone();
        }
    }

    // Filesystem discovery must happen outside the shared cache lock. Two
    // concurrent refreshes may duplicate work, but readers retain access to a
    // complete prior snapshot throughout either scan.
    let response = build();
    let built_at = Instant::now();
    cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            key,
            CapabilitySnapshot {
                response: response.clone(),
                built_at,
            },
        );
    response
}

#[cfg(test)]
fn clear_cache_for_tests() {
    cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
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
    let harness_home = dirs_home();
    get_all_for_home(coven_home, &harness_home, refresh)
}

fn get_all_for_home(coven_home: &Path, harness_home: &Path, refresh: bool) -> CapabilitiesResponse {
    let key = CacheKey::new(coven_home.to_path_buf(), harness_home.to_path_buf());
    get_or_refresh(key, refresh, || build_response(coven_home, harness_home))
}

fn build_response(coven_home: &Path, harness_home: &Path) -> CapabilitiesResponse {
    CapabilitiesResponse {
        coven_skills: crate::cockpit_sources::scan_skills(coven_home).unwrap_or_default(),
        harness_capabilities: vec![
            scan_codex_capabilities(harness_home),
            scan_claude_capabilities(harness_home),
            scan_cursor_capabilities(harness_home),
            scan_gemini_capabilities(harness_home),
            scan_opencode_capabilities(harness_home),
            scan_coven_code_capabilities(harness_home),
            scan_copilot_capabilities(harness_home),
        ],
        scanned_at: utc_now_iso(),
    }
}

/// Returns a single harness manifest, or `None` if the harness id is unknown.
pub fn get_one(
    coven_home: &Path,
    harness_id: &str,
    refresh: bool,
) -> Option<HarnessCapabilityManifest> {
    let harness_home = dirs_home();
    get_one_for_home(coven_home, &harness_home, harness_id, refresh)
}

fn get_one_for_home(
    coven_home: &Path,
    harness_home: &Path,
    harness_id: &str,
    refresh: bool,
) -> Option<HarnessCapabilityManifest> {
    get_all_for_home(coven_home, harness_home, refresh)
        .harness_capabilities
        .into_iter()
        .find(|manifest| manifest.harness_id == harness_id)
}

/// Invalidate the cache (e.g. on SIGHUP).
#[allow(dead_code)]
pub fn invalidate() {
    cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
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
            let dir = entry.path();
            if !is_directory_entry(&dir) {
                continue;
            }
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

/// Parse a JSON object whose values each contain an MCP server definition.
/// Supports both `stdio` (has `command` field) and `sse` (has `url` field)
/// transports.  Used by multiple scanners.
fn parse_mcp_servers_object(
    servers: &serde_json::Map<String, serde_json::Value>,
    harness_id: &str,
) -> Vec<HarnessPlugin> {
    let mut plugins = Vec::new();
    for (id, cfg) in servers {
        let disabled = cfg
            .get("disabled")
            .and_then(|d| d.as_bool())
            .unwrap_or(false);
        // Detect transport: prefer `url` (SSE) then fall back to `command` (stdio).
        let (transport, command) = if let Some(url) = cfg.get("url").and_then(|u| u.as_str()) {
            // SSE server — store the URL in the `command` field for display,
            // but mark transport as `sse`.
            (Some("sse".to_string()), Some(url.to_owned()))
        } else {
            let cmd = cfg
                .get("command")
                .and_then(|c| c.as_str())
                .map(str::to_owned);
            (Some("stdio".to_string()), cmd)
        };
        let args = cfg.get("args").and_then(|a| a.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        });
        plugins.push(HarnessPlugin {
            id: id.clone(),
            name: id.clone(),
            source: "harness-native",
            harness_id: harness_id.to_string(),
            kind: "mcp".to_string(),
            enabled: !disabled,
            transport,
            command,
            args,
        });
    }
    plugins
}

/// Try a list of candidate paths in order; return the raw JSON `Value` from
/// the first file that exists and parses, pushing a warning on parse failure.
/// Returns `(value, path_used)` or `None` if no candidate was found.
fn load_first_json(
    candidates: &[PathBuf],
    warnings: &mut Vec<CapabilityWarning>,
    label: &str,
) -> Option<(serde_json::Value, PathBuf)> {
    for path in candidates {
        if !path.exists() {
            continue;
        }
        match fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(v) => return Some((v, path.clone())),
                Err(err) => {
                    warnings.push(CapabilityWarning {
                        kind: "parse_error".to_string(),
                        path: path.to_string_lossy().into_owned(),
                        message: format!("could not parse {label}: {err}"),
                    });
                    return None; // Stop after first found file, even if malformed.
                }
            },
            Err(err) => {
                warnings.push(CapabilityWarning {
                    kind: "permission_denied".to_string(),
                    path: path.to_string_lossy().into_owned(),
                    message: format!("could not read {label}: {err}"),
                });
                return None;
            }
        }
    }
    None
}

// ── New harness scanners ──────────────────────────────────────────────────────

/// Scan `~/.cursor/mcp.json` for Cursor MCP servers.
///
/// Format: `{"mcpServers": {...}}` — same top-level shape as Claude desktop.
/// Transport can be `stdio` (has `command`) or `sse` (has `url`).
pub fn scan_cursor_capabilities(home_dir: &Path) -> HarnessCapabilityManifest {
    let now = utc_now_iso();
    let mut warnings: Vec<CapabilityWarning> = Vec::new();
    let mut plugins: Vec<HarnessPlugin> = Vec::new();

    let config_path = home_dir.join(".cursor").join("mcp.json");
    if let Some((v, _)) = load_first_json(&[config_path], &mut warnings, "mcp.json") {
        if let Some(servers) = v.get("mcpServers").and_then(|s| s.as_object()) {
            plugins = parse_mcp_servers_object(servers, "cursor");
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "cursor".to_string(),
        scanned_at: now,
        global_instructions: GlobalInstructions {
            present: false,
            path: None,
            byte_count: None,
        },
        skills: Vec::new(),
        plugins,
        warnings,
    }
}

/// Scan `~/.gemini/settings.json` for Gemini CLI MCP servers.
///
/// Format: `{"mcpServers": {"name": {"command": "...", "args": [...]}}}` —
/// the `mcpServers` key is optional and may be absent.
pub fn scan_gemini_capabilities(home_dir: &Path) -> HarnessCapabilityManifest {
    let now = utc_now_iso();
    let mut warnings: Vec<CapabilityWarning> = Vec::new();
    let mut plugins: Vec<HarnessPlugin> = Vec::new();

    let config_path = home_dir.join(".gemini").join("settings.json");
    if let Some((v, _)) = load_first_json(&[config_path], &mut warnings, "settings.json") {
        if let Some(servers) = v.get("mcpServers").and_then(|s| s.as_object()) {
            plugins = parse_mcp_servers_object(servers, "gemini");
        }
        // If `mcpServers` is absent, that's expected — no warning needed.
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "gemini".to_string(),
        scanned_at: now,
        global_instructions: GlobalInstructions {
            present: false,
            path: None,
            byte_count: None,
        },
        skills: Vec::new(),
        plugins,
        warnings,
    }
}

/// Scan OpenCode config for MCP servers.
///
/// Tries `~/.opencode/config.json` then `~/.config/opencode/config.json`.
/// Format: `{"mcp": {"servers": {...}}}` — nested under `mcp.servers`.
pub fn scan_opencode_capabilities(home_dir: &Path) -> HarnessCapabilityManifest {
    let now = utc_now_iso();
    let mut warnings: Vec<CapabilityWarning> = Vec::new();
    let mut plugins: Vec<HarnessPlugin> = Vec::new();

    let candidates = [
        home_dir.join(".opencode").join("config.json"),
        home_dir
            .join(".config")
            .join("opencode")
            .join("config.json"),
    ];
    if let Some((v, _)) = load_first_json(&candidates, &mut warnings, "config.json") {
        if let Some(servers) = v
            .get("mcp")
            .and_then(|m| m.get("servers"))
            .and_then(|s| s.as_object())
        {
            plugins = parse_mcp_servers_object(servers, "opencode");
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "opencode".to_string(),
        scanned_at: now,
        global_instructions: GlobalInstructions {
            present: false,
            path: None,
            byte_count: None,
        },
        skills: Vec::new(),
        plugins,
        warnings,
    }
}

/// Scan `~/.coven-code/settings.json` for coven-code MCP servers.
///
/// Format: `{"mcp_servers": [{"name": "...", "command": "...", "args": [],
/// "type": "stdio"}]}` — **array**, not object.
pub fn scan_coven_code_capabilities(home_dir: &Path) -> HarnessCapabilityManifest {
    let now = utc_now_iso();
    let mut warnings: Vec<CapabilityWarning> = Vec::new();
    let mut plugins: Vec<HarnessPlugin> = Vec::new();

    let config_path = home_dir.join(".coven-code").join("settings.json");
    if let Some((v, _)) = load_first_json(&[config_path], &mut warnings, "settings.json") {
        if let Some(arr) = v.get("mcp_servers").and_then(|s| s.as_array()) {
            for entry in arr {
                let name = entry
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_owned();
                let disabled = entry
                    .get("disabled")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false);
                let transport_str = entry
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("stdio")
                    .to_owned();
                let command = entry
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(str::to_owned);
                let args = entry.get("args").and_then(|a| a.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                });
                plugins.push(HarnessPlugin {
                    id: name.clone(),
                    name,
                    source: "harness-native",
                    harness_id: "coven-code".to_string(),
                    kind: "mcp".to_string(),
                    enabled: !disabled,
                    transport: Some(transport_str),
                    command,
                    args,
                });
            }
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "coven-code".to_string(),
        scanned_at: now,
        global_instructions: GlobalInstructions {
            present: false,
            path: None,
            byte_count: None,
        },
        skills: Vec::new(),
        plugins,
        warnings,
    }
}

/// Scan GitHub Copilot config for MCP servers.
///
/// On macOS, tries `~/Library/Application Support/GitHub Copilot/mcp.json`
/// first, then `~/.config/github-copilot/mcp.json`.
/// On other platforms, only the XDG path is tried.
/// Format: `{"mcpServers": {...}}` — same shape as Claude desktop.
pub fn scan_copilot_capabilities(home_dir: &Path) -> HarnessCapabilityManifest {
    let now = utc_now_iso();
    let mut warnings: Vec<CapabilityWarning> = Vec::new();
    let mut plugins: Vec<HarnessPlugin> = Vec::new();

    // Build candidate list. On macOS the Library path is preferred.
    let xdg_candidate = home_dir
        .join(".config")
        .join("github-copilot")
        .join("mcp.json");
    #[cfg(target_os = "macos")]
    let candidates: Vec<PathBuf> = vec![
        home_dir
            .join("Library")
            .join("Application Support")
            .join("GitHub Copilot")
            .join("mcp.json"),
        xdg_candidate,
    ];
    #[cfg(not(target_os = "macos"))]
    let candidates: Vec<PathBuf> = vec![xdg_candidate];

    if let Some((v, _)) = load_first_json(&candidates, &mut warnings, "mcp.json") {
        if let Some(servers) = v.get("mcpServers").and_then(|s| s.as_object()) {
            plugins = parse_mcp_servers_object(servers, "copilot");
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "copilot".to_string(),
        scanned_at: now,
        global_instructions: GlobalInstructions {
            present: false,
            path: None,
            byte_count: None,
        },
        skills: Vec::new(),
        plugins,
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
                            let mut new_plugins = parse_mcp_servers_object(servers, "claude");
                            plugins.append(&mut new_plugins);
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

    let mut skills = scan_claude_skills(&base, &mut warnings);
    skills.sort_by(|a, b| a.id.cmp(&b.id));

    HarnessCapabilityManifest {
        harness_id: "claude".to_string(),
        scanned_at: now,
        global_instructions,
        skills,
        plugins,
        warnings,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn is_directory_entry(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        Err(_) => false,
    }
}

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

#[derive(Debug)]
struct ClaudeSkillMetadata {
    name: String,
    description: Option<String>,
    version: Option<String>,
    tags: Vec<String>,
}

fn scan_claude_skills(
    claude_dir: &Path,
    warnings: &mut Vec<CapabilityWarning>,
) -> Vec<HarnessSkill> {
    let mut skills = Vec::new();
    let skills_dir = claude_dir.join("skills");
    let entries = match fs::read_dir(&skills_dir) {
        Ok(entries) => entries,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !is_directory_entry(&dir) {
            continue;
        }

        let skill_path = dir.join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }

        let id = entry.file_name().to_string_lossy().into_owned();
        let metadata = parse_claude_skill_md(&skill_path, &id, warnings);
        skills.push(HarnessSkill {
            id,
            name: metadata.name,
            source: "harness-native",
            harness_id: "claude".to_string(),
            path: dir.to_string_lossy().into_owned(),
            description: metadata.description,
            version: metadata.version,
            tags: metadata.tags,
        });
    }

    skills
}

fn parse_claude_skill_md(
    path: &Path,
    fallback_id: &str,
    warnings: &mut Vec<CapabilityWarning>,
) -> ClaudeSkillMetadata {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) => {
            warnings.push(CapabilityWarning {
                kind: "permission_denied".to_string(),
                path: path.to_string_lossy().into_owned(),
                message: format!("could not read SKILL.md: {err}"),
            });
            return ClaudeSkillMetadata {
                name: fallback_id.to_string(),
                description: None,
                version: None,
                tags: Vec::new(),
            };
        }
    };

    let mut metadata = ClaudeSkillMetadata {
        name: fallback_id.to_string(),
        description: None,
        version: None,
        tags: Vec::new(),
    };

    let mut lines = raw.lines();
    if lines.next().map(str::trim) != Some("---") {
        return metadata;
    }

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" => {
                if let Some(name) = parse_frontmatter_string(value) {
                    metadata.name = name;
                }
            }
            "description" => {
                metadata.description = parse_frontmatter_string(value);
            }
            "version" => {
                metadata.version = parse_frontmatter_string(value);
            }
            "tags" => {
                metadata.tags = parse_frontmatter_string_list(value);
            }
            _ => {}
        }
    }

    metadata
}

fn parse_frontmatter_string(value: &str) -> Option<String> {
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value)
        .trim();
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

fn parse_frontmatter_string_list(value: &str) -> Vec<String> {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Vec::new();
    };
    inner
        .split(',')
        .filter_map(|item| parse_frontmatter_string(item.trim()))
        .collect()
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
    use std::{
        fs,
        sync::{mpsc, Mutex},
        time::Duration,
    };
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn cache_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn fixture_response(label: &str) -> CapabilitiesResponse {
        CapabilitiesResponse {
            coven_skills: vec![crate::cockpit_sources::SkillDto {
                id: label.to_string(),
                name: label.to_string(),
                owner: "test".to_string(),
                category: "test".to_string(),
                tags: vec![],
                score: 0.0,
                effective_rate: 0.0,
                applied_rate: 0.0,
                completion_rate: 0.0,
                fallback_rate: 0.0,
                version: "1.0.0".to_string(),
                description: "test fixture".to_string(),
            }],
            harness_capabilities: vec![HarnessCapabilityManifest {
                harness_id: "codex".to_string(),
                scanned_at: label.to_string(),
                global_instructions: GlobalInstructions {
                    present: false,
                    path: None,
                    byte_count: None,
                },
                skills: vec![],
                plugins: vec![],
                warnings: vec![],
            }],
            scanned_at: label.to_string(),
        }
    }

    #[test]
    fn fresh_cache_hit_returns_the_complete_snapshot_without_building() {
        let _serial = cache_test_lock().lock().unwrap();
        clear_cache_for_tests();
        let key = CacheKey::new(PathBuf::from("/coven"), PathBuf::from("/harness"));
        let initial = fixture_response("initial");

        let first = get_or_refresh(key.clone(), false, || initial.clone());
        let hit = get_or_refresh(key, false, || panic!("fresh cache must not build"));

        assert_eq!(first.scanned_at, "initial");
        assert_eq!(hit.scanned_at, "initial");
        assert_eq!(hit.coven_skills[0].id, "initial");
        assert_eq!(hit.harness_capabilities[0].scanned_at, "initial");
    }

    #[test]
    fn refresh_does_not_block_fresh_reader() {
        let _serial = cache_test_lock().lock().unwrap();
        clear_cache_for_tests();
        let key = CacheKey::new(PathBuf::from("/coven"), PathBuf::from("/harness"));
        let cached = fixture_response("cached");
        get_or_refresh(key.clone(), false, || cached);

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let refresh_key = key.clone();
        let refresh = std::thread::spawn(move || {
            get_or_refresh(refresh_key, true, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                fixture_response("refreshed")
            })
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let (reader_tx, reader_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            reader_tx
                .send(get_or_refresh(key, false, || {
                    panic!("fresh cache must not build")
                }))
                .unwrap();
        });
        let response = reader_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(response.scanned_at, "cached");

        release_tx.send(()).unwrap();
        reader.join().unwrap();
        assert_eq!(refresh.join().unwrap().scanned_at, "refreshed");
    }

    #[test]
    fn snapshot_ttl_starts_when_discovery_finishes() {
        let _serial = cache_test_lock().lock().unwrap();
        clear_cache_for_tests();
        let key = CacheKey::new(PathBuf::from("/coven"), PathBuf::from("/harness"));

        let write_guard = cache().write().unwrap();
        let (built_tx, built_rx) = mpsc::channel();
        let refresh_key = key.clone();
        let refresh = std::thread::spawn(move || {
            get_or_refresh(refresh_key, true, || {
                built_tx.send(()).unwrap();
                fixture_response("refreshed")
            })
        });
        built_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(50));

        let lock_released_at = Instant::now();
        drop(write_guard);
        refresh.join().unwrap();

        let snapshot = cache().read().unwrap().get(&key).cloned().unwrap();
        assert!(snapshot.built_at <= lock_released_at);
    }

    #[test]
    fn get_one_reads_the_published_response_snapshot() {
        let _serial = cache_test_lock().lock().unwrap();
        clear_cache_for_tests();
        let coven_home = PathBuf::from("/coven");
        let harness_home = PathBuf::from("/harness");
        get_or_refresh(
            CacheKey::new(coven_home.clone(), harness_home.clone()),
            false,
            || fixture_response("published"),
        );

        let manifest = get_one_for_home(&coven_home, &harness_home, "codex", false)
            .expect("published codex manifest");
        assert_eq!(manifest.scanned_at, "published");
    }

    #[test]
    fn snapshots_are_scoped_to_coven_and_harness_homes() {
        let _serial = cache_test_lock().lock().unwrap();
        clear_cache_for_tests();
        let first = CacheKey::new(PathBuf::from("/coven-one"), PathBuf::from("/harness"));
        let second = CacheKey::new(PathBuf::from("/coven-two"), PathBuf::from("/harness"));

        get_or_refresh(first.clone(), false, || fixture_response("first"));
        get_or_refresh(second, false, || fixture_response("second"));

        let first_hit = get_or_refresh(first, false, || panic!("first cache must remain scoped"));
        assert_eq!(first_hit.scanned_at, "first");
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

    #[cfg(unix)]
    #[test]
    fn scan_codex_follows_symlinked_automation_dirs() {
        let home = tmp();
        let real_auto = home.path().join("real-automations").join("nightly-triage");
        fs::create_dir_all(&real_auto).unwrap();
        fs::write(
            real_auto.join("automation.toml"),
            r#"name = "Nightly triage"
description = "Reviews stale branches."
tags = ["triage"]"#,
        )
        .unwrap();

        let autos = home.path().join(".codex").join("automations");
        fs::create_dir_all(&autos).unwrap();
        std::os::unix::fs::symlink(&real_auto, autos.join("nightly-triage")).unwrap();

        let m = scan_codex_capabilities(home.path());

        assert_eq!(m.skills.len(), 1);
        assert_eq!(m.skills[0].id, "nightly-triage");
        assert_eq!(m.skills[0].name, "Nightly triage");
        assert_eq!(
            m.skills[0].description.as_deref(),
            Some("Reviews stale branches.")
        );
        assert_eq!(m.skills[0].tags, vec!["triage"]);
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

    #[test]
    fn scan_claude_scans_skill_frontmatter() {
        let home = tmp();
        let skill = home.path().join(".claude").join("skills").join("reviewer");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            r#"---
name: "Review Helper"
description: "Reviews code changes."
version: "1.2.3"
tags: ["review", "code"]
---

# Review Helper
"#,
        )
        .unwrap();

        let m = scan_claude_capabilities(home.path());

        assert_eq!(m.skills.len(), 1);
        let skill = &m.skills[0];
        assert_eq!(skill.id, "reviewer");
        assert_eq!(skill.name, "Review Helper");
        assert_eq!(skill.description.as_deref(), Some("Reviews code changes."));
        assert_eq!(skill.version.as_deref(), Some("1.2.3"));
        assert_eq!(skill.tags, vec!["review", "code"]);
        assert!(m.warnings.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn scan_claude_follows_symlinked_skill_dirs() {
        let home = tmp();
        let real_skill = home.path().join("real-skills").join("brainstorming");
        fs::create_dir_all(&real_skill).unwrap();
        fs::write(
            real_skill.join("SKILL.md"),
            r#"---
name: Brainstorming
description: Explore before building.
---
"#,
        )
        .unwrap();

        let skills = home.path().join(".claude").join("skills");
        fs::create_dir_all(&skills).unwrap();
        std::os::unix::fs::symlink(&real_skill, skills.join("brainstorming")).unwrap();

        let m = scan_claude_capabilities(home.path());

        assert_eq!(m.skills.len(), 1);
        assert_eq!(m.skills[0].id, "brainstorming");
        assert_eq!(m.skills[0].name, "Brainstorming");
        assert_eq!(
            m.skills[0].description.as_deref(),
            Some("Explore before building.")
        );
    }

    // ── Cursor ─────────────────────────────────────────────────────────────────

    #[test]
    fn scan_cursor_returns_empty_when_dir_missing() {
        let home = tmp();
        let m = scan_cursor_capabilities(home.path());
        assert_eq!(m.harness_id, "cursor");
        assert!(m.plugins.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_cursor_parses_stdio_mcp_servers() {
        let home = tmp();
        let dir = home.path().join(".cursor");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("mcp.json"),
            r#"{"mcpServers": {"fs": {"command": "npx", "args": ["-y", "@mcp/fs"]}}}"#,
        )
        .unwrap();
        let m = scan_cursor_capabilities(home.path());
        assert_eq!(m.plugins.len(), 1);
        let p = &m.plugins[0];
        assert_eq!(p.id, "fs");
        assert_eq!(p.transport.as_deref(), Some("stdio"));
        assert_eq!(p.command.as_deref(), Some("npx"));
        assert!(p.enabled);
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_cursor_parses_sse_mcp_servers() {
        let home = tmp();
        let dir = home.path().join(".cursor");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("mcp.json"),
            r#"{"mcpServers": {"remote": {"url": "http://localhost:3000/mcp"}}}"#,
        )
        .unwrap();
        let m = scan_cursor_capabilities(home.path());
        assert_eq!(m.plugins.len(), 1);
        let p = &m.plugins[0];
        assert_eq!(p.id, "remote");
        assert_eq!(p.transport.as_deref(), Some("sse"));
        // URL is stored in the `command` field for display.
        assert_eq!(p.command.as_deref(), Some("http://localhost:3000/mcp"));
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_cursor_records_warning_on_malformed_json() {
        let home = tmp();
        let dir = home.path().join(".cursor");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mcp.json"), "{{{BAD}}}").unwrap();
        let m = scan_cursor_capabilities(home.path());
        assert!(m.plugins.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].kind, "parse_error");
    }

    // ── Gemini CLI ─────────────────────────────────────────────────────────

    #[test]
    fn scan_gemini_returns_empty_when_dir_missing() {
        let home = tmp();
        let m = scan_gemini_capabilities(home.path());
        assert_eq!(m.harness_id, "gemini");
        assert!(m.plugins.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_gemini_parses_mcp_servers() {
        let home = tmp();
        let dir = home.path().join(".gemini");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            r#"{
  "theme": "dark",
  "mcpServers": {
    "search": {"command": "uvx", "args": ["mcp-search"]}
  }
}"#,
        )
        .unwrap();
        let m = scan_gemini_capabilities(home.path());
        assert_eq!(m.plugins.len(), 1);
        let p = &m.plugins[0];
        assert_eq!(p.id, "search");
        assert_eq!(p.transport.as_deref(), Some("stdio"));
        assert_eq!(p.command.as_deref(), Some("uvx"));
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_gemini_tolerates_absent_mcp_servers_key() {
        let home = tmp();
        let dir = home.path().join(".gemini");
        fs::create_dir_all(&dir).unwrap();
        // Valid JSON but no mcpServers key.
        fs::write(dir.join("settings.json"), r#"{"theme": "light"}"#).unwrap();
        let m = scan_gemini_capabilities(home.path());
        assert!(m.plugins.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_gemini_records_warning_on_malformed_json() {
        let home = tmp();
        let dir = home.path().join(".gemini");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), "NOT_JSON").unwrap();
        let m = scan_gemini_capabilities(home.path());
        assert!(m.plugins.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].kind, "parse_error");
    }

    // ── OpenCode ───────────────────────────────────────────────────────────

    #[test]
    fn scan_opencode_returns_empty_when_dir_missing() {
        let home = tmp();
        let m = scan_opencode_capabilities(home.path());
        assert_eq!(m.harness_id, "opencode");
        assert!(m.plugins.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_opencode_parses_mcp_servers_primary_path() {
        let home = tmp();
        let dir = home.path().join(".opencode");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{
  "mcp": {
    "servers": {
      "sqlite": {"command": "uvx", "args": ["mcp-sqlite", "--db", "/tmp/a.db"]}
    }
  }
}"#,
        )
        .unwrap();
        let m = scan_opencode_capabilities(home.path());
        assert_eq!(m.plugins.len(), 1);
        let p = &m.plugins[0];
        assert_eq!(p.id, "sqlite");
        assert_eq!(p.command.as_deref(), Some("uvx"));
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_opencode_tries_xdg_fallback() {
        let home = tmp();
        // Only place the config at the XDG path.
        let xdg = home.path().join(".config").join("opencode");
        fs::create_dir_all(&xdg).unwrap();
        fs::write(
            xdg.join("config.json"),
            r#"{"mcp": {"servers": {"echo": {"command": "echo"}}}}"#,
        )
        .unwrap();
        let m = scan_opencode_capabilities(home.path());
        assert_eq!(m.plugins.len(), 1);
        assert_eq!(m.plugins[0].id, "echo");
    }

    #[test]
    fn scan_opencode_records_warning_on_malformed_json() {
        let home = tmp();
        let dir = home.path().join(".opencode");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.json"), "[[[BROKEN").unwrap();
        let m = scan_opencode_capabilities(home.path());
        assert!(m.plugins.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].kind, "parse_error");
    }

    // ── coven-code ──────────────────────────────────────────────────────────

    #[test]
    fn scan_coven_code_returns_empty_when_dir_missing() {
        let home = tmp();
        let m = scan_coven_code_capabilities(home.path());
        assert_eq!(m.harness_id, "coven-code");
        assert!(m.plugins.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_coven_code_parses_array_mcp_servers() {
        let home = tmp();
        let dir = home.path().join(".coven-code");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            r#"{
  "mcp_servers": [
    {"name": "filesystem", "command": "npx", "args": ["-y", "@mcp/fs"], "type": "stdio"},
    {"name": "search",     "command": "uvx", "args": ["mcp-search"],      "type": "stdio", "disabled": true}
  ]
}"#,
        )
        .unwrap();
        let m = scan_coven_code_capabilities(home.path());
        assert_eq!(m.plugins.len(), 2);
        let fs_p = m.plugins.iter().find(|p| p.id == "filesystem").unwrap();
        assert_eq!(fs_p.transport.as_deref(), Some("stdio"));
        assert!(fs_p.enabled);
        let search = m.plugins.iter().find(|p| p.id == "search").unwrap();
        assert!(!search.enabled);
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_coven_code_records_warning_on_malformed_json() {
        let home = tmp();
        let dir = home.path().join(".coven-code");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), "NOTJSON").unwrap();
        let m = scan_coven_code_capabilities(home.path());
        assert!(m.plugins.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].kind, "parse_error");
    }

    // ── Copilot ───────────────────────────────────────────────────────────

    #[test]
    fn scan_copilot_returns_empty_when_dir_missing() {
        let home = tmp();
        let m = scan_copilot_capabilities(home.path());
        assert_eq!(m.harness_id, "copilot");
        assert!(m.plugins.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_copilot_parses_xdg_mcp_json() {
        let home = tmp();
        let xdg = home.path().join(".config").join("github-copilot");
        fs::create_dir_all(&xdg).unwrap();
        fs::write(
            xdg.join("mcp.json"),
            r#"{"mcpServers": {"tools": {"command": "npx", "args": ["@github/copilot-tools"]}}}"#,
        )
        .unwrap();
        let m = scan_copilot_capabilities(home.path());
        assert_eq!(m.plugins.len(), 1);
        let p = &m.plugins[0];
        assert_eq!(p.id, "tools");
        assert_eq!(p.transport.as_deref(), Some("stdio"));
        assert_eq!(p.command.as_deref(), Some("npx"));
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn scan_copilot_records_warning_on_malformed_json() {
        let home = tmp();
        let xdg = home.path().join(".config").join("github-copilot");
        fs::create_dir_all(&xdg).unwrap();
        fs::write(xdg.join("mcp.json"), "{BAD JSON}").unwrap();
        let m = scan_copilot_capabilities(home.path());
        assert!(m.plugins.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].kind, "parse_error");
    }
}
