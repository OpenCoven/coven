# Coven Harness Capabilities — TECH

**Status:** Draft v1 · 2026-06-02  
**Owner:** Coven runtime  
**Depends on:** PRODUCT.md (this directory)

---

## Implementation plan

### 1. Capability manifest types (`crates/coven-cli/src/capabilities.rs`)

New file. No changes to existing files until the scanner and API handler are also ready.

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct GlobalInstructions {
    pub present: bool,
    pub path: Option<String>,
    pub byte_count: Option<u64>,
    pub excerpt_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessSkill {
    pub id: String,
    pub name: String,
    pub source: &'static str, // always "harness-native"
    pub harness_id: String,
    pub path: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessPlugin {
    pub id: String,
    pub name: String,
    pub source: &'static str, // "harness-native"
    pub harness_id: String,
    pub kind: String,         // "mcp" | "extension"
    pub enabled: bool,
    // MCP-specific (optional for non-mcp kinds)
    pub transport: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityWarning {
    pub kind: String,   // "parse_error" | "permission_denied" | "partial_scan"
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessCapabilityManifest {
    pub harness_id: String,
    pub scanned_at: String, // ISO-8601 UTC
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
```

### 2. Per-harness scanners

Each scanner lives in `capabilities.rs` as a free function.

#### `scan_codex_capabilities(home_dir: &Path) -> HarnessCapabilityManifest`

```
~/.codex/AGENTS.md          → global_instructions
~/.codex/automations/*/     → skills[] (reads automation.toml for name/description if present,
                               falls back to directory name as id)
```

Automation TOML shape (best-effort; missing fields degrade gracefully):

```toml
name = "Daily bug scan"
description = "Scans open issues and PRs for regressions each morning."
version = "1.0.0"
tags = ["bugs", "daily"]
```

#### `scan_claude_capabilities(home_dir: &Path) -> HarnessCapabilityManifest`

```
~/.claude/CLAUDE.md                           → global_instructions
~/.claude/claude_desktop_config.json          → plugins[] (mcpServers object)
  OR ~/.config/claude/claude_desktop_config.json (XDG fallback)
```

MCP server JSON shape (standard Claude config):

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path"],
      "disabled": false
    }
  }
}
```

Each key under `mcpServers` becomes a `HarnessPlugin` with `kind = "mcp"`.

### 3. Cache layer

Add to the daemon state (wherever global daemon state is held today):

```rust
struct CapabilityCache {
    manifests: HashMap<String, HarnessCapabilityManifest>,
    built_at: std::time::Instant,
}
```

- Cache is built on first `GET /v1/capabilities` request.
- Invalidated on `?refresh=1` or daemon `SIGHUP`.
- TTL: 5 minutes soft (stale data served with a `X-Capabilities-Stale: true` header until next refresh).
- No persistent storage — always rebuilt from disk.

### 4. API handler changes (`crates/coven-cli/src/api.rs`)

Add two new routes:

```rust
("GET", "/capabilities") => {
    let refresh = query.contains("refresh=1");
    let caps = build_capabilities_response(coven_home, &daemon_state, refresh)?;
    json_response(200, &caps)
}
("GET", p) if p.starts_with("/capabilities/") => {
    let harness_id = p.trim_start_matches("/capabilities/");
    let caps = build_single_harness_manifest(harness_id, coven_home, &daemon_state, false)?;
    match caps {
        Some(m) => json_response(200, &m),
        None => json_response(404, &serde_json::json!({"error": "unknown harness"})),
    }
}
```

`build_capabilities_response` calls `scan_codex_capabilities` and `scan_claude_capabilities` for every harness in `built_in_harness_specs()`, combines them with `cockpit_sources::scan_skills(coven_home)`, and returns a `CapabilitiesResponse`.

The existing `GET /skills` route is unchanged.

### 5. `coven doctor` output (`crates/coven-cli/src/control_plane.rs` or wherever doctor is implemented)

After the existing harness availability table, print a Capabilities section. Use the same manifest data — call the scanners directly (not through the HTTP path) so doctor works even when the daemon is not running.

Output format mirrors the existing doctor style (color-coded terminal rows). No new dependencies.

### 6. Tests

Tests live in `capabilities.rs` alongside the implementation (consistent with existing test placement in `cockpit_sources.rs`).

Required unit tests:
- `scan_codex_capabilities_returns_empty_manifest_when_dir_missing`
- `scan_codex_capabilities_parses_agents_md_metadata`
- `scan_codex_capabilities_scans_automations_subdirectories`
- `scan_codex_capabilities_tolerates_missing_automation_toml`
- `scan_claude_capabilities_returns_empty_manifest_when_dir_missing`
- `scan_claude_capabilities_parses_claude_md_metadata`
- `scan_claude_capabilities_parses_mcp_servers`
- `scan_claude_capabilities_tolerates_malformed_mcp_json` (warns, does not panic)
- `capabilities_response_combines_coven_skills_and_harness_manifests`

Required integration tests (in `api.rs` test block, consistent with existing tests):
- `get_capabilities_returns_200_with_empty_manifests_when_no_harness_config`
- `get_capabilities_refresh_param_invalidates_cache`
- `get_capabilities_harness_filter_returns_single_manifest`
- `get_capabilities_unknown_harness_returns_404`

### 7. No writes to harness-native directories

All scanner functions take `home_dir: &Path` as input and only call `fs::read_dir`, `fs::read_to_string`, and `fs::metadata`. No `fs::write`, `fs::create_dir`, or `OpenOptions` with write access. This must be enforced by code review — there is no runtime guard.

---

## File impact summary

| File | Change |
|---|---|
| `crates/coven-cli/src/capabilities.rs` | **New file** — types + scanners + cache |
| `crates/coven-cli/src/api.rs` | Add `GET /capabilities` and `GET /capabilities/:id` routes |
| `crates/coven-cli/src/control_plane.rs` | Add Capabilities section to `coven doctor` output |
| `crates/coven-cli/src/lib.rs` | `pub mod capabilities;` |

No changes to `harness.rs`, `cockpit_sources.rs`, or the session ledger.

---

## Gaps this tech spec defers

- **FSEvents/inotify watch** for automatic cache invalidation — use `notify` crate; deferred because TTL + `?refresh=1` covers the common case without the complexity.
- **Project-local config layering** — scanning a project's `AGENTS.md` or `.claude` and merging with global config. Requires Coven to know the current project root at scan time; deferred to the project-level capabilities follow-up.
- **Custom harness adapter capability scanner trait** — a `CapabilityScanner` trait that user-defined adapters implement. Deferred to the adapter maturity path.

---

## Acceptance for v1 (tech)

All acceptance items from PRODUCT.md, plus:

1. `cargo test -p coven-cli capabilities` passes with all unit tests listed above.
2. `cargo clippy -p coven-cli -- -D warnings` is clean on the new file.
3. `cargo fmt --check` passes.
4. No `unsafe` in `capabilities.rs`.
5. No file is opened with write access in any scanner function (verified by review).
