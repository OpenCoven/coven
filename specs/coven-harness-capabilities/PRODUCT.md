# Coven Harness Capabilities — PRODUCT

**Status:** Draft v1 · 2026-06-02  
**Owner:** Coven runtime  
**Acceptance target:** "Any harness-specific skill or plugin a user has installed is visible to Coven and surfaced to CastCodes without manual registration."

---

## Problem

Coven knows about its own `~/.coven/skills/` directory and serves them over `GET /v1/skills`. It does not know anything about the user's harness-native configuration: Codex automations, Claude MCP servers, `AGENTS.md` / `CLAUDE.md` global rules, or any future harness-specific skill surfaces.

This means:

- CastCodes cannot show the user what Codex or Claude brings to a session.
- Coven cannot route a request to the right harness based on what capabilities are available.
- Users who install a Codex automation or a Claude MCP server have no way to know whether Coven sees it.
- `coven doctor` cannot tell the user that their harness has unconfigured or conflicting instructions.

The gap creates a split world: Coven knows its own skills, each harness knows its own, and no layer sees the whole picture.

---

## Philosophy alignment

Coven's north star is: **One project. Any harness. Visible work.**

Capability visibility is part of visible work. A user should be able to open CastCodes and understand exactly what any lane session brings into a prompt: which Coven skills are active, which harness-native rules apply, which tools are registered. Opacity here defeats the purpose of a controlled local runtime.

At the same time, Coven's authority model is clear: the Rust daemon is Rank 0. Harness-native configuration is **read-only input** to Coven — Coven observes it, reports it, and can make routing decisions based on it, but never modifies it. The harness owns its config; Coven is a reader and reporter, not a manager.

---

## Scope of v1

v1 establishes:

1. **A capability manifest format** — a standard JSON shape every harness adapter can produce.
2. **A scan contract** — when and how Coven reads harness-native directories to build a capability manifest.
3. **An API surface** — `GET /v1/capabilities` extended to include harness-scoped capabilities alongside Coven skills.
4. **A `coven doctor` integration** — capability health is part of the doctor check.
5. **The authority boundary for harness config** — read-only; Coven never writes into `~/.codex/`, `~/.claude/`, or equivalent.

Out of scope for v1:
- Installing or modifying harness-native skills from within Coven or CastCodes.
- Cloud sync of harness capabilities.
- Cross-harness skill translation (a Codex automation exposed to Claude as an MCP server).
- User-authored custom harness adapters (those are the phase-2 adapter path; see `FUTURE-HARNESSES.md`).

---

## Capability manifest format

A capability manifest is a JSON object produced by a harness adapter's capability scanner. It is ephemeral — rebuilt on demand, never stored in the session ledger.

```json
{
  "harness_id": "codex",
  "scanned_at": "2026-06-02T18:00:00Z",
  "global_instructions": {
    "present": true,
    "path": "/Users/alice/.codex/AGENTS.md",
    "byte_count": 1240,
    "excerpt_lines": 5
  },
  "skills": [
    {
      "id": "daily-bug-scan",
      "name": "Daily bug scan",
      "source": "harness-native",
      "harness_id": "codex",
      "path": "/Users/alice/.codex/automations/daily-bug-scan",
      "description": "Scans open issues and PRs for regressions each morning.",
      "version": null,
      "tags": []
    }
  ],
  "plugins": [],
  "warnings": []
}
```

For Claude:

```json
{
  "harness_id": "claude",
  "scanned_at": "2026-06-02T18:00:00Z",
  "global_instructions": {
    "present": true,
    "path": "/Users/alice/.claude/CLAUDE.md",
    "byte_count": 3102,
    "excerpt_lines": 5
  },
  "skills": [],
  "plugins": [
    {
      "id": "filesystem",
      "name": "Filesystem MCP server",
      "source": "harness-native",
      "harness_id": "claude",
      "kind": "mcp",
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/alice/projects"],
      "enabled": true
    }
  ],
  "warnings": []
}
```

### Field semantics

| Field | Required | Notes |
|---|---|---|
| `harness_id` | Yes | Matches the Coven harness id (`codex`, `claude`, …) |
| `scanned_at` | Yes | ISO-8601 UTC timestamp |
| `global_instructions.present` | Yes | Whether the harness-global instruction file exists |
| `global_instructions.path` | If present | Absolute path to the file |
| `global_instructions.byte_count` | If present | Size in bytes; no content is stored |
| `global_instructions.excerpt_lines` | If present | Line count of a brief leading excerpt (for display only) |
| `skills[].id` | Yes | Unique within the harness |
| `skills[].source` | Yes | Always `"harness-native"` for harness-scanned entries; `"coven"` for Coven-owned skills |
| `plugins[].kind` | Yes | `"mcp"` today; `"extension"` reserved for future harness plugin types |
| `warnings[]` | Yes (may be empty) | Non-fatal scan problems (missing directory, unreadable file, parse error) |

---

## Scan contract

### When to scan

Coven builds a capability manifest for a harness **lazily, on first request** per daemon lifetime, then caches it. The cache is invalidated when:

- The client sends `GET /v1/capabilities?refresh=1`.
- The daemon receives a `SIGHUP`.
- The relevant harness config directory is modified (inotify/FSEvents watch, best-effort; degraded to manual refresh on platforms without reliable filesystem events).

Coven does **not** scan on every session launch. Capability discovery is a background/on-demand operation, not a hot path.

### What to scan per built-in harness

**Codex (`~/.codex/`)**

| Item | Scan target | Result field |
|---|---|---|
| Global instructions | `~/.codex/AGENTS.md` | `global_instructions` |
| Automations | `~/.codex/automations/*/automation.toml` | `skills[]` |

**Claude (`~/.claude/`)**

| Item | Scan target | Result field |
|---|---|---|
| Global instructions | `~/.claude/CLAUDE.md` | `global_instructions` |
| MCP servers | `~/.claude/claude_desktop_config.json` → `mcpServers` | `plugins[]` |

If a config file is missing: `global_instructions.present = false` or the relevant array is empty. Missing directories are not errors — they produce empty manifests with no warnings.

If a config file exists but cannot be parsed: a structured warning is added to `warnings[]`. The scan does not fail; the daemon continues with a partial manifest.

### Authority: read-only

Coven never writes to harness-native directories. No harness config file is created, modified, or deleted by the Coven daemon or CLI.

---

## API surface

### `GET /v1/capabilities`

Returns the union of Coven-owned skills and all available harness capability manifests.

```json
{
  "coven_skills": [ /* existing SkillDto array */ ],
  "harness_capabilities": [
    { /* capability manifest for codex */ },
    { /* capability manifest for claude */ }
  ],
  "scanned_at": "2026-06-02T18:00:00Z"
}
```

Query parameters:
- `?harness=codex` — filter to a single harness.
- `?refresh=1` — invalidate cache and re-scan before responding.

The existing `GET /v1/skills` endpoint is preserved unchanged for backward compatibility. `GET /v1/capabilities` is the new unified surface.

### `GET /v1/capabilities/:harness_id`

Returns the manifest for a single harness. Returns `404` if the harness id is unknown; returns a manifest with empty arrays and `global_instructions.present = false` if the harness is known but not installed or has no config.

---

## `coven doctor` integration

`coven doctor` gains a **Capabilities** section after the existing harness availability checks:

```
Capabilities
  codex   global instructions   ~/.codex/AGENTS.md (1240 bytes)   ✓
  codex   automations           7 found                            ✓
  claude  global instructions   ~/.claude/CLAUDE.md (3102 bytes)   ✓
  claude  mcp servers           3 enabled, 0 disabled              ✓
  coven   skills                2 installed                        ✓
```

Warnings from the scan appear as yellow rows. Missing optional config (no `AGENTS.md`, no automations) is shown as a neutral info row, not a warning.

`coven doctor --capabilities` runs only the capabilities section and exits.

---

## CastCodes integration

CastCodes reads `GET /v1/capabilities` at workspace open and displays a **Capabilities panel** in the session launcher or familiar detail view, showing:

- Which Coven skills are available globally.
- Which harness-native instructions are active for a given session's harness.
- Which automations or MCP plugins that harness brings.

CastCodes does not expose controls to modify harness-native config. The panel is read-only, same as the daemon boundary.

---

## Gaps this spec consciously defers

- **Project-level capability overrides** — project-local `AGENTS.md` or `.claude` config files layered on top of global config. This interacts with Coven's project-root scoping and is deferred to a follow-up spec.
- **Automation scheduling through Coven** — Codex automations today run outside Coven. Bringing them under Coven's session ledger is a separate feature.
- **Custom harness adapters** — user-defined harness adapters that ship their own capability scanner. Deferred to the adapter maturity path in `FUTURE-HARNESSES.md`.
- **Cross-harness capability advertisement** — exposing a Codex automation to Claude as an MCP tool. Interesting but out of scope until the base manifest format is stable.

---

## Acceptance for v1

1. `GET /v1/capabilities` returns a correct manifest for `codex` and `claude` on a machine where both are installed.
2. `GET /v1/capabilities` returns an empty-but-valid manifest (not an error) for a harness that is installed but has no config directory.
3. `GET /v1/capabilities` returns a manifest with a populated `warnings[]` and no panic when a config file exists but cannot be parsed.
4. `coven doctor` prints the Capabilities section with correct counts.
5. `GET /v1/capabilities?refresh=1` forces a re-scan and the response `scanned_at` is updated.
6. No scan writes any file into `~/.codex/`, `~/.claude/`, or any harness-native directory.
7. CastCodes reads `GET /v1/capabilities` and renders a read-only capability summary for any open session.
