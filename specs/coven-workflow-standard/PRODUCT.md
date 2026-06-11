# Coven Workflow Standard - PRODUCT

**Status:** Draft v1 - 2026-06-11
**Owner:** Coven runtime, Coven Code, Cave
**Acceptance target:** "A Coven user can create, validate, preview, share, and run a workflow from either Coven Code or Cave, with the same manifest and the same safety result."

---

## Problem

Coven has several useful building blocks already: familiars, skills, tools,
hooks, sessions, standing orders, Coven Code slash commands, and Cave surfaces
for roles, boards, chat, and library context. What it lacks is a shared
workflow contract.

Without that contract:

- A repeated task is trapped inside one harness, one Cave affordance, or one
  agent's prose instructions.
- Cave and Coven Code can drift into different workflow shapes.
- Users cannot inspect the required inputs, optional knobs, limits,
  permissions, and approval gates before a workflow runs.
- Import/export is risky because executable behavior can be hidden inside an
  opaque script body.
- There is no shared dry-run or preview surface for estimating agents, cost,
  permissions, and human review points before execution.

The workflow standard gives Coven a portable recipe format, not an arbitrary
automation runtime.

---

## Philosophy alignment

Coven workflows must stay local-first, readable, inspectable, and user-owned.
A workflow is a recipe for orchestration across existing Coven capabilities. It
can call a familiar, skill, tool, command, prompt, or another workflow, but the
manifest itself is declarative in v1.

The important guarantee is that Cave and Coven Code do not have secret
parallel semantics. They read and write the same canonical file on disk, run
the same validator, show the same dry-run plan, and enforce the same safety
rules.

---

## Scope of v1

v1 establishes:

1. A portable `coven.workflow.v1` manifest format.
2. Two canonical file forms:
   - `workflows/<id>/WORKFLOW.md` with YAML front matter plus narrative docs.
   - `<id>.workflow.yaml` for simple workflows with five or fewer steps.
3. JSON Schema validation that runs in editors, Coven Code, Cave, and the
   daemon.
4. Semantic lint for references, pattern rules, cycle checks, and registry
   availability.
5. Runtime preflight for limits, installed dependencies, Ward gates, and
   permission posture.
6. Mandatory dry-run/plan mode before execution.
7. Import/export bundles for sharing without secrets.
8. A Coven Code TUI and slash-command surface for fast terminal use.
9. A Cave workflow browser, builder, validator, preview, and run-history
   surface for friendly authoring.

Out of scope for v1:

- Embedded arbitrary script bodies.
- General scheduling grammar beyond `trigger: null` or `trigger.kind: manual`.
- Cloud sync.
- Automatic rollback of external side effects.
- A visual graph editor as the only authoring surface.
- Model-specific routing. Workflows declare capabilities, not model names.

---

## User outcomes

### Create

A user can scaffold a workflow from either client:

- Coven Code: `/workflow new` or command-palette/TUI picker.
- Cave: Workflows view, template gallery, or builder form.

The scaffold must include every required field and validation-safe defaults
where defaults are allowed.

### Inspect

A user can see:

- Required inputs and optional configuration.
- Step list and step kinds.
- Permissions and external touch points.
- Limits for agents, time, and cost.
- Human-gate points.
- Outputs and artifacts.
- Error handling behavior.
- Which familiar owns or recommends the workflow.

### Validate

A user gets deterministic errors with:

- `code`
- `path` as JSON Pointer
- `message`
- `suggestion`
- `tier`: `schema`, `semantic`, or `preflight`

### Preview

Dry-run produces a non-executing plan that estimates:

- Agent count.
- Tool and skill dependencies.
- Required human gates.
- Permission prompts.
- Token/cost budget range.
- Runtime limit posture.
- Output artifacts.
- Reasons the run would be blocked.

### Run

Running a workflow creates observable run state:

- Status.
- Started and ended timestamps.
- Step statuses.
- Logs or linked session events.
- Artifacts.
- Final result.
- Validation and dry-run result used for the run.

---

## Canonical manifest

The manifest is YAML. `WORKFLOW.md` stores the manifest in front matter and
allows human narrative below it. `<id>.workflow.yaml` stores only the manifest.

Required fields:

```yaml
schema_version: coven.workflow.v1
id: cody-review-pr
version: 1.0.0
name: Review PR
summary: Review a GitHub pull request with a verification pass.
description: >
  Runs a structured code review, collects findings, and gates any suggested
  changes behind human approval.
pattern: sequential
familiar: cody
requires:
  - reasoning
inputs:
  pr_url:
    type: string
    required: true
    description: GitHub pull request URL.
outputs:
  review:
    type: markdown
    description: Ordered review findings and residual risk.
permissions:
  filesystem: read
  network: true
  external_accounts:
    - github
  approval: before_external_write
limits:
  max_agents: 2
  timeout_s: 1800
  cost_ceiling_usd: 3.00
ward_gate: true
steps:
  - id: inspect-pr
    kind: tool
    uses: github.pr.inspect
    with:
      pr_url: ${inputs.pr_url}
  - id: review
    kind: agent
    uses: familiar:cody
    needs:
      - inspect-pr
  - id: approval
    kind: human-gate
    prompt: Approve posting or implementing review feedback?
on_error:
  strategy: stop
  notify: true
```

---

## Required field semantics

| Field | Requirement |
|---|---|
| `schema_version` | Must be `coven.workflow.v1` for v1 workflows. Missing schema is a hard error. |
| `id` | Kebab-case stable slug. Recommended namespace is `<familiar>-<name>`. |
| `version` | Semver. |
| `name` | Short display name. |
| `summary` | One-sentence summary for lists and previews. |
| `description` | Human-readable explanation. May be front matter or narrative body in `WORKFLOW.md`. |
| `pattern` | Closed enum: `fan-out-and-synthesize`, `classify-and-act`, `adversarial-verification`, `generate-and-filter`, `tournament`, `loop-until-done`, `sequential`, `custom`. |
| `custom_pattern_name` | Required only when `pattern: custom`. |
| `custom_pattern_description` | Required only when `pattern: custom`. |
| `familiar` | Familiar ID, not instance path or running session ID. |
| `requires` | Capability requirements such as `reasoning`, `code-edit`, `web`, `vision`, or `long-context`; no model names. |
| `inputs` | Map of user-provided inputs, each with type and required/default rules. |
| `outputs` | Map of expected outputs or artifacts. |
| `permissions` | Filesystem, network, external account, tool, and approval posture. |
| `limits.max_agents` | Required integer. No default. |
| `limits.timeout_s` | Required integer. No default. |
| `limits.cost_ceiling_usd` | Required number. No default. |
| `ward_gate` | Defaults to `true`. If `false`, `ward_gate_reason` is required. |
| `steps` | Non-empty ordered list. |
| `on_error` | Explicit failure strategy. |

`exit_criteria` is required when `pattern: loop-until-done`.

---

## Optional configuration

Optional fields may be omitted without changing safety posture:

| Field | Default | Notes |
|---|---|---|
| `trigger` | `null` | v1 is manual-first. `trigger.kind: manual` is equivalent to null. |
| `visibility.coven_code` | `true` | Whether Coven Code lists the workflow. |
| `visibility.coven_cave` | `true` | Whether Cave lists the workflow. |
| `tags` | `[]` | Search and gallery labels. |
| `roles` | `[]` | Role affinities by ID. |
| `display_name` | `name` | Alternate UI label. |
| `examples` | `[]` | Example inputs and expected outputs. |
| `custom_fields` | `{}` | Extension map ignored by validators except for reserved-key checks. |

Cave-only display state never goes in the manifest. Step graph layout, panel
widths, color overrides, collapsed groups, pins, and draft cursor state live in
`WORKFLOW.cave.json`.

---

## Step kinds

| Kind | Purpose |
|---|---|
| `agent` | Run a familiar/subagent by stable familiar ID. |
| `skill` | Invoke a named skill by `skill-id@semver-range`, never by local path. |
| `tool` | Invoke a registered tool by stable ID. |
| `command` | Invoke a Coven or Coven Code command by stable command ID. |
| `prompt` | Run a prompt-only step against the selected familiar/runtime. |
| `human-gate` | Pause for user approval. Cave shows a pending gate; Coven Code shows an interactive prompt. |
| `workflow` | Invoke another workflow by ID and semver range. |

Nested workflows must pass cycle detection. v1 maximum nesting depth is five.

External scripts are not embedded. If an existing script must be called, wrap it
as a `tool` or `command` capability and preflight that capability before run.

---

## Validation tiers

Validation fails early at the cheapest tier.

### Tier 1: Schema

JSON Schema checks:

- Required fields.
- Types.
- Closed enums.
- Semver shape.
- Kebab-case ID.
- Conditional requirements such as `exit_criteria` and `custom_pattern_name`.
- Required limits.

### Tier 2: Semantic lint

Semantic lint checks:

- Familiar ID exists.
- Skill, tool, command, and workflow references resolve.
- Nested workflows do not cycle and stay within depth five.
- `needs` references point to earlier step IDs.
- Pattern-specific rules are satisfied.
- No model names or internal executor fields are used.
- `custom_fields` does not shadow reserved fields.

### Tier 3: Runtime preflight

Preflight checks:

- Required skills, plugins, tools, and external accounts are available.
- Filesystem and network permissions can be requested under the current safety
  posture.
- Ward gates are present for protected tiers.
- `limits` are within local policy.
- Dry-run plan can be produced without execution.

---

## Ward and approval policy

Ward compliance is part of v1, not a later retrofit.

Protected workflows must include `val_review_required: true` when they touch:

- SOUL or identity files.
- MEMORY or daily memory files.
- Ward policy files.
- Plugin or skill approval state.
- Secrets, tokens, or account credentials.
- Public posting or external writes.

If `ward_gate: false`, the manifest must include `ward_gate_reason`. The
validator emits a Tier-1 Cave event and a Coven Code warning so the bypass is
auditable.

---

## Storage and discovery

Clients discover workflows from these roots, in order:

1. Project `.coven/workflows/`.
2. Project `.coven-code/workflows/` for Coven Code compatibility.
3. User `~/.coven/workflows/`.

The canonical file on disk is the source of truth. Cave may keep sidecars and
run history, but it does not fork the manifest.

Recommended complex layout:

```text
workflows/
  cody-review-pr/
    WORKFLOW.md
    WORKFLOW.cave.json
    examples/
      happy-path.yaml
```

Recommended simple layout:

```text
workflows/
  cody-review-pr.workflow.yaml
```

---

## Import and export

Shared bundles use:

```text
<id>-v<version>.coven-workflow.tar.gz
```

The bundle includes:

- `MANIFEST.json` with bundle metadata and file checksums.
- The canonical `WORKFLOW.md` or `.workflow.yaml`.
- Optional examples and docs.
- No secrets.
- No runtime-local paths unless explicitly marked as examples.

Import validates before install and shows a dry-run preview before enabling.

---

## Cave surface

Cave gets the friendly authoring path:

- Workflows top-level section or Library subsection.
- Template gallery.
- Builder form for required fields.
- Text editor for advanced users.
- Validator panel with grouped schema, semantic, and preflight errors.
- Preview button backed by dry-run.
- Run history with step status, logs, artifacts, and gates.
- Sidecar-backed graph/layout view.
- Dirty-state protection when the file changes on disk.

Cave writes only canonical manifest fields to `WORKFLOW.md` or
`.workflow.yaml`. UI display state goes to `WORKFLOW.cave.json`.

---

## Coven Code surface

Coven Code gets the fast terminal path:

```text
/workflow list
/workflow show <id>
/workflow new
/workflow validate <path-or-id>
/workflow dry-run <id>
/workflow run <id>
/workflow explain <id>
/workflow doctor
/workflow export <id>
```

The TUI also gets a workflow picker/editor reachable from the command palette
and relevant `/coven` surfaces. The picker uses the same validator and dry-run
planner as Cave.

---

## Migration

Existing skill prose does not need forced migration. Keep `## Steps` in skills
until the behavior needs parallelism, gates, data binding, or cross-familiar
coordination. Then extract a workflow and reference the skill from a workflow
step.

Existing dynamic workflow scripts should be wrapped as external tools or
commands. The workflow manifest references the capability; it does not embed
script source.

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Cave and Coven Code diverge | Canonical file is source of truth; shared schema and validator. |
| Lock-in to current harness internals | Use familiar IDs, capability requirements, and stable tool/skill IDs. |
| Missing schema from early workflows | Hard fail in validator; scaffold always stamps `schema_version`. |
| Unbounded execution | Require `max_agents`, `timeout_s`, and `cost_ceiling_usd`. |
| Hidden Ward bypass | `ward_gate: false` requires `ward_gate_reason` and emits audit events. |
| Graph editor pollutes portable manifest | Cave layout sidecar only. |

---

## Acceptance for v1

1. A workflow scaffold created in Cave validates and runs from Coven Code.
2. A workflow scaffold created in Coven Code validates and opens in Cave.
3. The shared validator returns the same error list for the same invalid file
   in both clients.
4. Dry-run produces a plan without executing steps.
5. A workflow with missing limits fails schema validation.
6. A workflow with `pattern: loop-until-done` and no `exit_criteria` fails
   schema validation.
7. A workflow with `ward_gate: false` and no `ward_gate_reason` fails schema
   validation.
8. Cave sidecar edits do not modify the canonical workflow manifest.
9. Import rejects bundles containing secrets or embedded executable script
   bodies.
10. Exported bundles re-import on a clean machine with only missing-capability
    warnings, not parse errors.
