# Coven Workflow Standard - TECH

**Status:** Draft v1 - 2026-06-11
**Owner:** Coven runtime, Coven Code, Cave
**Depends on:** PRODUCT.md (this directory)

---

## Architecture

The workflow standard is implemented as a shared contract first, then consumed
by clients.

Core pieces:

1. Schema package in `OpenCoven/coven`.
2. Validator and dry-run planner in the Coven daemon/runtime layer.
3. Coven Code command and TUI integration in `OpenCoven/coven-code`.
4. Cave browser, builder, preview, sidecar, and run-history integration in
   `OpenCoven/coven-cave`.

The canonical manifest stays a file on disk. Cave and Coven Code are clients of
that file and the shared validator.

---

## Shared schema files

Create:

```text
schemas/workflow/coven.workflow.v1.schema.json
schemas/workflow/examples/cody-review-pr.workflow.yaml
schemas/workflow/examples/sage-research-synthesis/WORKFLOW.md
```

The JSON Schema must validate:

- `schema_version: coven.workflow.v1`
- Required scalar fields.
- Kebab-case `id`.
- Semver `version`.
- Closed `pattern` enum.
- Required `limits.max_agents`, `limits.timeout_s`,
  `limits.cost_ceiling_usd`.
- Required non-empty `steps`.
- Conditional `exit_criteria` for `loop-until-done`.
- Conditional `custom_pattern_name` and `custom_pattern_description` for
  `custom`.
- Conditional `ward_gate_reason` when `ward_gate` is `false`.
- Step kind enum.
- Step `needs` as an array of strings.
- `custom_fields` as an object.

Do not allow these reserved internal fields anywhere in the manifest:

- `executor`
- `session_id`
- `process_id`
- `workspace_path`
- `comux_channel`
- `model`
- `api_key`
- `token`

---

## Validator response contract

All validator and preflight errors use:

```json
{
  "code": "workflow.required_limits",
  "path": "/limits/max_agents",
  "message": "limits.max_agents is required.",
  "suggestion": "Set limits.max_agents to the maximum number of agents this workflow may spawn.",
  "tier": "schema"
}
```

Types:

```ts
type WorkflowValidationTier = "schema" | "semantic" | "preflight";

interface WorkflowValidationIssue {
  code: string;
  path: string;
  message: string;
  suggestion: string;
  tier: WorkflowValidationTier;
}

interface WorkflowValidationResult {
  ok: boolean;
  schemaVersion: "coven.workflow.v1" | string | null;
  workflowId: string | null;
  issues: WorkflowValidationIssue[];
}
```

Use the same shape over the daemon API, Coven Code command output, and Cave API
responses.

---

## Dry-run plan contract

Dry-run never executes steps. It resolves references and estimates posture.

```ts
interface WorkflowDryRunPlan {
  ok: boolean;
  workflowId: string;
  version: string;
  steps: Array<{
    id: string;
    kind: string;
    uses?: string;
    status: "ready" | "blocked";
    blockers: WorkflowValidationIssue[];
  }>;
  estimates: {
    maxAgents: number;
    timeoutS: number;
    costCeilingUsd: number;
    requiredCapabilities: string[];
    requiredExternalAccounts: string[];
    humanGates: string[];
  };
  issues: WorkflowValidationIssue[];
}
```

If validation fails, dry-run returns `ok: false` and the validation issues. It
must not continue to runtime preflight after a schema failure.

---

## Daemon and CLI API

Add workflow routes to the local daemon API:

| Route | Purpose |
|---|---|
| `GET /api/v1/workflows` | List discovered workflows. |
| `GET /api/v1/workflows/:id` | Return manifest metadata and canonical path. |
| `POST /api/v1/workflows/validate` | Validate raw manifest content or a path. |
| `POST /api/v1/workflows/dry-run` | Return a dry-run plan for a workflow and inputs. |
| `POST /api/v1/workflows/run` | Start a workflow run after validation and dry-run. |
| `GET /api/v1/workflow-runs/:id` | Return run state. |
| `POST /api/v1/workflows/import` | Validate and stage an import bundle. |
| `POST /api/v1/workflows/export` | Create an export bundle. |

CLI commands:

```text
coven workflows list
coven workflows show <id>
coven workflows validate <path-or-id>
coven workflows dry-run <id> --input key=value
coven workflows run <id> --input key=value
coven workflows import <bundle>
coven workflows export <id>
```

Coven Code may call daemon APIs when available and fall back to local shared
library validation when the daemon is offline.

---

## Discovery order

Workflow roots:

1. `<project>/.coven/workflows/`
2. `<project>/.coven-code/workflows/`
3. `$COVEN_HOME/workflows/`, defaulting to `~/.coven/workflows/`

Discovery rules:

- A directory containing `WORKFLOW.md` is one workflow.
- A file matching `*.workflow.yaml` or `*.workflow.yml` is one workflow.
- Duplicate IDs resolve by discovery order.
- Duplicate IDs at the same priority are validation warnings in list views and
  hard errors when selecting by ID.
- File watchers may refresh lists, but editors must use dirty-state checks
  before reloading.

---

## File parsing

`WORKFLOW.md` parsing:

- Read YAML front matter between the first `---` and the next `---`.
- Treat the Markdown body as human narrative.
- If `description` is missing in front matter, derive it from the first
  paragraph after the front matter for display only; validation still requires
  a description field in the normalized manifest.

`.workflow.yaml` parsing:

- Read the whole file as YAML.
- No narrative body is available.

Both forms normalize to the same in-memory `WorkflowManifest`.

---

## Runtime execution model

v1 execution is mediated by stable step kinds.

Step execution mapping:

| Step kind | Executor |
|---|---|
| `agent` | Spawn or delegate to familiar by ID through Coven harness routing. |
| `skill` | Resolve skill ID and semver range through Coven skill registry. |
| `tool` | Resolve registered tool/capability ID. |
| `command` | Resolve stable Coven or Coven Code command ID. |
| `prompt` | Send prompt to resolved familiar/runtime. |
| `human-gate` | Pause run and await explicit user input. |
| `workflow` | Load nested workflow, validate, preflight, and execute within depth limit. |

The executor writes run events to the session/event ledger so both Cave and
Coven Code can display the same run state.

---

## Coven Code integration

Repository: `OpenCoven/coven-code`

Expected file impact:

| File area | Change |
|---|---|
| `src-rust/crates/core` | Shared workflow manifest types if not consumed from a generated package. |
| `src-rust/crates/commands` | `/workflow` slash command group and command tests. |
| `src-rust/crates/tui` | Workflow picker/editor, validation panel, dry-run preview. |
| `docs/commands.md` | Document `/workflow` commands. |
| `docs/index.md` | Add workflow feature entry. |

Command behavior:

- `/workflow list` shows ID, version, familiar, pattern, visibility, validation
  state.
- `/workflow show <id>` shows normalized manifest summary and canonical path.
- `/workflow new` scaffolds `WORKFLOW.md` or `.workflow.yaml`.
- `/workflow validate <path-or-id>` prints grouped issues.
- `/workflow dry-run <id>` prints the dry-run plan.
- `/workflow run <id>` refuses to run if validation or dry-run is not clean.
- `/workflow explain <id>` renders user-facing docs from manifest metadata.
- `/workflow doctor` checks roots, duplicates, schema availability, and daemon
  integration.
- `/workflow export <id>` writes a `.coven-workflow.tar.gz` bundle.

---

## Cave integration

Repository: `OpenCoven/coven-cave`

Expected file impact:

| File area | Change |
|---|---|
| `src/lib` | Workflow API client, parser helpers, sidecar helpers. |
| `src/app/api` | Server routes proxying daemon workflow APIs where needed. |
| `src/components` | Workflow list, builder, validator panel, dry-run preview, run history. |
| `docs/superpowers/specs` | Cave UX spec for the Workflows section. |
| `docs/superpowers/plans` | Cave implementation plan. |

Cave must:

- Read canonical workflow files.
- Write only manifest fields to canonical files.
- Write display state to `WORKFLOW.cave.json`.
- Protect unsaved edits from file watcher reloads.
- Show schema, semantic, and preflight errors separately.
- Show human-gate steps as pending approvals.
- Show run history from daemon run state, not from local UI state alone.

---

## Import/export bundle

Bundle name:

```text
<id>-v<version>.coven-workflow.tar.gz
```

`MANIFEST.json`:

```json
{
  "schema_version": "coven.workflow.bundle.v1",
  "workflow_id": "cody-review-pr",
  "workflow_version": "1.0.0",
  "created_at": "2026-06-11T00:00:00Z",
  "files": [
    {
      "path": "WORKFLOW.md",
      "sha256": "hex-encoded-sha256",
      "kind": "workflow"
    }
  ]
}
```

Import checks:

- Bundle manifest exists.
- Checksums match.
- Exactly one canonical workflow exists.
- No file path escapes the target directory.
- Secret-pattern scan passes.
- No embedded script body fields exist.
- Workflow validates before install.

---

## Versioning policy

Schema version policy:

- Additive optional fields are minor-compatible.
- New required fields require a new major schema version.
- Field removal requires a new major schema version and a migration document.
- Validators must ignore unknown `custom_fields`.
- Validators must reject unknown top-level fields unless the schema explicitly
  allows them.

`schema_version` is always required. There is no v0 or inferred schema.

---

## Acceptance for v1 (tech)

1. `coven.workflow.v1.schema.json` validates both example workflows.
2. Schema rejects missing `schema_version`.
3. Schema rejects missing required limit fields.
4. Schema rejects `ward_gate: false` without `ward_gate_reason`.
5. Schema rejects `pattern: loop-until-done` without `exit_criteria`.
6. Semantic lint catches missing familiar, missing skill, bad `needs`
   references, cycles, and nesting depth over five.
7. Preflight catches missing installed dependencies and policy-blocked
   permissions.
8. Dry-run returns a plan without executing any step.
9. Coven Code and Cave display identical validation issues for the same bad
   manifest.
10. Cave sidecar changes do not alter the canonical manifest file.
11. Import rejects path traversal, secret-like content, and embedded script
    bodies.
12. Exported bundle imports on a clean local Coven install.

---

## Deferred work

v1.1 candidates:

- Scheduling grammar.
- Rollback/idempotency markers.
- Visual graph editing as a full authoring surface.
- Cloud marketplace publication.
- Remote workflow runs.
- Embedded script support through signed, separately reviewed artifacts.
