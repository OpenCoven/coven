---
summary: "GET /api/v1/capabilities discovery: control-plane catalog and harness manifests."
read_when:
  - Looking up the capabilities API
title: "Capabilities endpoint"
description: "Reference for the /api/v1/capabilities routes: the control-plane catalog on the bare path, the harness capability aggregate at /capabilities/harnesses, and single-harness manifests at /capabilities/:harnessId."
---

Coven exposes two capability concepts on adjacent paths. Do not confuse them:

| Method | Path | Concept |
|---|---|---|
| GET | `/api/v1/capabilities` | **Control-plane catalog** — what the daemon itself can do (feature ids, policy hints, routable action ids). |
| GET | `/api/v1/capabilities/harnesses` | **Harness capability aggregate** — what each installed harness brings (global instructions, skills, plugins), plus Coven-owned skills. |
| GET | `/api/v1/capabilities/:harnessId` | One harness's capability manifest. |

## Control-plane catalog

`GET /api/v1/capabilities` is the intake-client handshake for deciding which
actions to show or route through Coven. The full payload shape, known enum
values, and forward-compatibility rules are pinned in the
[API contract](/API-CONTRACT#capability-catalog-shape-v1):

```json
{ "capabilities": [ { "id", "label", "adapter", "status", "policy", "actions" } ] }
```

To refresh it, `POST /api/v1/actions` with action id `coven.capabilities.refresh`.

## Harness capability aggregate

`GET /api/v1/capabilities/harnesses` returns the union of Coven-owned skills
and one manifest per known harness scan target (`codex`, `claude`, `cursor`,
`gemini`, `opencode`, `coven-code`, `copilot`). Scans are read-only: Coven
never writes to harness-native config directories.

```json
{
  "coven_skills": [ /* SkillDto array, same shape as GET /api/v1/skills */ ],
  "harness_capabilities": [
    {
      "harness_id": "codex",
      "scanned_at": "2026-07-15T12:00:00Z",
      "global_instructions": { "present": true, "path": "~/.codex/AGENTS.md", "byte_count": 1240 },
      "skills": [],
      "plugins": [],
      "warnings": []
    }
  ],
  "scanned_at": "2026-07-15T12:00:00Z"
}
```

- A harness that is not installed (or has no config) still returns a manifest —
  empty arrays and `global_instructions.present: false` — never an error.
- Unparseable config files add structured entries to `warnings[]`; the scan
  does not fail.
- Results are cached for 5 minutes. Pass `?refresh=1` to invalidate the cache
  and re-scan before responding.
- This surface keeps the snake_case field names pinned by the
  [harness-capabilities spec](https://github.com/OpenCoven/coven/blob/main/specs/coven-harness-capabilities/PRODUCT.md)
  (`coven_skills`, `harness_capabilities`, `scanned_at`, `harness_id`).

## Single harness manifest

`GET /api/v1/capabilities/:harnessId` returns one manifest from the same scan
(also honoring `?refresh=1`). Unknown harness ids fail closed with the
structured error envelope:

```json
{ "error": { "code": "harness_not_found", "message": "Harness id is not a known capability scan target.", "details": { "harnessId": "warlock" } } }
```

`harnesses` is a reserved path segment and never a valid harness id.

## History

The harness-capabilities spec originally placed the aggregate at
`GET /api/v1/capabilities` itself, but that route was shadowed by the
control-plane catalog and never reachable
([#368](https://github.com/OpenCoven/coven/issues/368)). The catalog kept the
bare path — matching shipped behavior and the API contract — and the aggregate
landed at `/capabilities/harnesses`.
