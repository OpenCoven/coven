# Coven Workflow Standard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement CWF-01 so users can create, validate, preview, share, and run the same workflow manifest from Coven Code and Cave.

**Architecture:** Start with the shared schema and validator in `OpenCoven/coven`, then expose daemon/CLI APIs, then integrate Coven Code and Cave as clients. The canonical workflow file remains the source of truth; Cave-only display state stays in sidecars.

**Tech Stack:** Rust Coven daemon/CLI, JSON Schema, YAML, Coven Code Rust TUI, Cave Next.js/TypeScript UI, tar.gz bundle import/export.

**Spec:** `specs/coven-workflow-standard/PRODUCT.md` and `specs/coven-workflow-standard/TECH.md`

---

## File Structure

`OpenCoven/coven`:

- Create `schemas/workflow/coven.workflow.v1.schema.json`: JSON Schema source of truth.
- Create `schemas/workflow/examples/cody-review-pr.workflow.yaml`: simple workflow fixture.
- Create `schemas/workflow/examples/sage-research-synthesis/WORKFLOW.md`: directory workflow fixture.
- Create `crates/coven-cli/src/workflows/manifest.rs`: typed manifest structs and normalization.
- Create `crates/coven-cli/src/workflows/validation.rs`: schema, semantic, and preflight validation entry points.
- Create `crates/coven-cli/src/workflows/discovery.rs`: workflow root discovery and duplicate handling.
- Create `crates/coven-cli/src/workflows/dry_run.rs`: dry-run plan builder.
- Create `crates/coven-cli/src/workflows/bundles.rs`: import/export bundle checks.
- Modify `crates/coven-cli/src/api.rs`: workflow API routes.
- Modify `crates/coven-cli/src/main.rs`: module declaration and `coven workflows ...` CLI group.

`OpenCoven/coven-code`:

- Modify `src-rust/crates/commands`: `/workflow` slash command group.
- Modify `src-rust/crates/tui`: workflow picker/editor and validation preview.
- Modify `docs/commands.md` and `docs/index.md`: user docs.

`OpenCoven/coven-cave`:

- Create `src/lib/workflows.ts`: client-side types and API helpers.
- Create `src/lib/workflow-sidecar.ts`: `WORKFLOW.cave.json` read/write helpers.
- Create `src/app/api/workflows/*`: Cave server proxy routes.
- Create `src/components/workflows-*`: list, builder, validator, dry-run, run-history components.
- Modify shell/navigation files to expose Workflows.

---

### Task 1: Add shared schema and examples

**Files:**
- Create: `schemas/workflow/coven.workflow.v1.schema.json`
- Create: `schemas/workflow/examples/cody-review-pr.workflow.yaml`
- Create: `schemas/workflow/examples/sage-research-synthesis/WORKFLOW.md`

- [ ] **Step 1: Create the schema**

Add `schemas/workflow/coven.workflow.v1.schema.json` with the required fields from the TECH spec. Include `if`/`then` clauses for:

```json
{
  "if": { "properties": { "pattern": { "const": "loop-until-done" } } },
  "then": { "required": ["exit_criteria"] }
}
```

and:

```json
{
  "if": { "properties": { "ward_gate": { "const": false } } },
  "then": { "required": ["ward_gate_reason"] }
}
```

Use `additionalProperties: false` at the top level and allow extension only through `custom_fields`.

- [ ] **Step 2: Add the simple YAML fixture**

Create `schemas/workflow/examples/cody-review-pr.workflow.yaml` using the example manifest from `specs/coven-workflow-standard/PRODUCT.md`.

- [ ] **Step 3: Add the WORKFLOW.md fixture**

Create `schemas/workflow/examples/sage-research-synthesis/WORKFLOW.md` with YAML front matter for a `fan-out-and-synthesize` research workflow and a short Markdown body describing when to use it.

- [ ] **Step 4: Sanity-check the schema and YAML fixtures**

Run:

```bash
cd ~/Documents/GitHub/OpenCoven/coven
jq empty schemas/workflow/coven.workflow.v1.schema.json
ruby -e 'require "yaml"; YAML.load_file(ARGV[0])' schemas/workflow/examples/cody-review-pr.workflow.yaml
ruby -e 'text = File.read(ARGV[0]); abort("missing front matter") unless text.start_with?("---\n"); YAML.safe_load(text.split(/^---\s*$/, 3)[1], permitted_classes: [], aliases: false)' schemas/workflow/examples/sage-research-synthesis/WORKFLOW.md
```

Expected: schema JSON parses, YAML fixture parses, and `WORKFLOW.md` has valid YAML front matter. Full JSON Schema validation is added as Rust tests in Task 2 so the repo does not depend on a global YAML-aware schema CLI.

---

### Task 2: Add manifest parsing and schema validation in Coven

**Files:**
- Create: `crates/coven-cli/src/workflows/mod.rs`
- Create: `crates/coven-cli/src/workflows/manifest.rs`
- Create: `crates/coven-cli/src/workflows/validation.rs`
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Write parser tests**

Add tests for:

```rust
#[test]
fn parses_yaml_workflow_file() {
    let source = include_str!("../../../../schemas/workflow/examples/cody-review-pr.workflow.yaml");
    let manifest = WorkflowManifest::from_yaml(source).unwrap();
    assert_eq!(manifest.schema_version, "coven.workflow.v1");
    assert_eq!(manifest.id, "cody-review-pr");
}

#[test]
fn parses_workflow_md_front_matter() {
    let source = include_str!("../../../../schemas/workflow/examples/sage-research-synthesis/WORKFLOW.md");
    let manifest = WorkflowManifest::from_workflow_md(source).unwrap();
    assert_eq!(manifest.schema_version, "coven.workflow.v1");
    assert_eq!(manifest.pattern, WorkflowPattern::FanOutAndSynthesize);
}
```

- [ ] **Step 2: Implement manifest structs**

Define `WorkflowManifest`, `WorkflowStep`, `WorkflowLimits`, `WorkflowPermissions`, `WorkflowInput`, `WorkflowOutput`, and `WorkflowPattern` with `serde` derives. Use strongly typed enums for `pattern` and `steps[].kind`.

- [ ] **Step 3: Implement front-matter parsing**

Parse `WORKFLOW.md` by requiring the file to start with `---`, reading until the next `---`, and feeding only that YAML block to `serde_yaml`.

- [ ] **Step 4: Write schema-validation tests**

Add tests for missing `schema_version`, missing `limits.max_agents`, `ward_gate: false` without `ward_gate_reason`, and `loop-until-done` without `exit_criteria`.

- [ ] **Step 5: Implement schema validation**

Use the existing Rust JSON Schema crate if present; otherwise add a small dependency such as `jsonschema` in `crates/coven-cli/Cargo.toml`. Return `WorkflowValidationResult` with issue objects matching the TECH spec.

- [ ] **Step 6: Run targeted tests**

Run:

```bash
cd ~/Documents/GitHub/OpenCoven/coven
cargo test -p coven-cli workflows::manifest workflows::validation
```

If Cargo rejects multiple test filters, run them separately:

```bash
cargo test -p coven-cli workflows::manifest
cargo test -p coven-cli workflows::validation
```

Expected: all new parser and schema-validation tests pass.

---

### Task 3: Add discovery, semantic lint, and dry-run

**Files:**
- Create: `crates/coven-cli/src/workflows/discovery.rs`
- Create: `crates/coven-cli/src/workflows/dry_run.rs`
- Modify: `crates/coven-cli/src/workflows/validation.rs`

- [ ] **Step 1: Write discovery tests**

Cover all three roots:

```rust
#[test]
fn discovers_project_coven_before_project_coven_code_before_user_coven() {
    let roots = WorkflowRoots::new(project.join(".coven/workflows"), project.join(".coven-code/workflows"), home.join(".coven/workflows"));
    let discovered = discover_workflows(&roots).unwrap();
    assert_eq!(discovered[0].source, WorkflowSource::ProjectCoven);
}
```

- [ ] **Step 2: Implement discovery**

Discover `*/WORKFLOW.md`, `*.workflow.yaml`, and `*.workflow.yml`. Store canonical path, source tier, ID, version, name, familiar, and validation state.

- [ ] **Step 3: Write semantic lint tests**

Test unknown familiar, unknown skill, bad `needs` reference, workflow cycle, and nesting depth over five. Use fake registries so tests do not depend on the user's machine.

- [ ] **Step 4: Implement semantic lint**

Add a `WorkflowReferenceRegistry` trait with methods for familiar, skill, tool, command, and workflow lookup. Production uses real registries; tests use fakes.

- [ ] **Step 5: Write dry-run tests**

Assert that dry-run returns estimates and never calls any executor. Use a fake executor with a counter and assert the counter stays zero.

- [ ] **Step 6: Implement dry-run**

Dry-run calls schema validation, semantic lint, and runtime preflight, then builds `WorkflowDryRunPlan` from limits, permissions, external accounts, capabilities, and human-gate steps.

- [ ] **Step 7: Run targeted tests**

```bash
cd ~/Documents/GitHub/OpenCoven/coven
cargo test -p coven-cli workflows::discovery workflows::validation workflows::dry_run
```

If Cargo rejects multiple test filters, run them separately:

```bash
cargo test -p coven-cli workflows::discovery
cargo test -p coven-cli workflows::validation
cargo test -p coven-cli workflows::dry_run
```

Expected: all discovery, semantic lint, and dry-run tests pass.

---

### Task 4: Add Coven API and CLI workflow commands

**Files:**
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `crates/coven-cli/src/main.rs` or existing CLI command module
- Modify: `docs/reference/cli.md`
- Modify: `docs/API.md`

- [ ] **Step 1: Add API tests**

Add tests for:

```rust
get_workflows_lists_discovered_workflows
post_workflows_validate_returns_grouped_issues
post_workflows_dry_run_does_not_execute_steps
post_workflows_run_rejects_invalid_workflow
post_workflows_import_rejects_path_traversal_bundle
```

- [ ] **Step 2: Implement API routes**

Add routes from `TECH.md`:

```text
GET /api/v1/workflows
GET /api/v1/workflows/:id
POST /api/v1/workflows/validate
POST /api/v1/workflows/dry-run
POST /api/v1/workflows/run
GET /api/v1/workflow-runs/:id
POST /api/v1/workflows/import
POST /api/v1/workflows/export
```

- [ ] **Step 3: Add CLI tests**

Test `coven workflows list`, `show`, `validate`, and `dry-run` against fixture roots.

- [ ] **Step 4: Implement CLI commands**

Wire commands to the same workflow service used by the API. Print validation issues grouped by tier and use JSON output when the existing CLI supports machine-readable output.

- [ ] **Step 5: Update Coven docs**

Document the API routes in `docs/API.md` and CLI commands in `docs/reference/cli.md`.

- [ ] **Step 6: Verify Coven**

```bash
cd ~/Documents/GitHub/OpenCoven/coven
cargo test -p coven-cli workflows
cargo fmt --check
cargo clippy -p coven-cli -- -D warnings
```

Expected: tests pass, formatting is clean, clippy has no warnings.

---

### Task 5: Add Coven Code `/workflow` and TUI surfaces

**Files:**
- Modify: `~/Documents/GitHub/OpenCoven/coven-code/src-rust/crates/commands/src/lib.rs`
- Modify: `~/Documents/GitHub/OpenCoven/coven-code/src-rust/crates/tui/src/app.rs`
- Modify: `~/Documents/GitHub/OpenCoven/coven-code/docs/commands.md`
- Modify: `~/Documents/GitHub/OpenCoven/coven-code/docs/index.md`

- [ ] **Step 1: Write command tests**

In `claurst-commands`, test:

```rust
workflow_list_shows_discovered_workflows
workflow_validate_prints_schema_and_semantic_issues
workflow_dry_run_refuses_invalid_workflow
workflow_run_requires_clean_dry_run
workflow_export_writes_bundle
```

- [ ] **Step 2: Implement `/workflow` command group**

Add subcommands:

```text
list
show <id>
new
validate <path-or-id>
dry-run <id>
run <id>
explain <id>
doctor
export <id>
```

When the Coven daemon is online, call its workflow API. When offline, support local list, show, new, validate, dry-run, explain, doctor, and export using the shared schema files vendored or generated from `OpenCoven/coven`.

- [ ] **Step 3: Add TUI picker/editor**

Add a workflow picker reachable from the command palette and `/coven` surfaces. Each row shows ID, familiar, pattern, validation state, and canonical path. The editor exposes required fields first and an advanced raw YAML pane second.

- [ ] **Step 4: Add dry-run preview**

Before run, render dry-run estimates: max agents, timeout, cost ceiling, external accounts, permissions, and human gates. Disable run when dry-run is not clean.

- [ ] **Step 5: Update Coven Code docs**

Document `/workflow` in `docs/commands.md` and link it from `docs/index.md`.

- [ ] **Step 6: Verify Coven Code**

```bash
cd ~/Documents/GitHub/OpenCoven/coven-code/src-rust
cargo test -p claurst-commands workflow
cargo check --workspace
cargo fmt --all --check
```

Expected: command tests pass, workspace checks, formatting is clean.

---

### Task 6: Add Cave Workflows interface

**Files:**
- Create: `coven-cave/src/lib/workflows.ts`
- Create: `coven-cave/src/lib/workflow-sidecar.ts`
- Create: `coven-cave/src/app/api/workflows/route.ts`
- Create: `coven-cave/src/app/api/workflows/validate/route.ts`
- Create: `coven-cave/src/app/api/workflows/dry-run/route.ts`
- Create: `coven-cave/src/components/workflows-view.tsx`
- Create: `coven-cave/src/components/workflow-builder.tsx`
- Create: `coven-cave/src/components/workflow-validator-panel.tsx`
- Create: `coven-cave/src/components/workflow-dry-run-preview.tsx`

- [ ] **Step 1: Write API helper tests**

Test that Cave forwards validation and dry-run requests to the daemon and preserves `WorkflowValidationIssue` shape exactly.

- [ ] **Step 2: Implement API helpers**

`src/lib/workflows.ts` defines shared TypeScript interfaces matching the TECH spec and functions for list, validate, dry-run, run, import, and export.

- [ ] **Step 3: Write sidecar tests**

Test that `WORKFLOW.cave.json` stores layout state without modifying `WORKFLOW.md`.

- [ ] **Step 4: Implement sidecar helpers**

`src/lib/workflow-sidecar.ts` reads and writes only Cave display fields: node positions, colors, collapsed groups, sidebar pins, and selected tab.

- [ ] **Step 5: Build Workflows view**

Create a dense operational view: list on the left, details/validator/preview on the right, builder accessible through a create button. Avoid marketing copy. The first screen is the usable workflow browser.

- [ ] **Step 6: Build builder form**

The form includes required fields, inputs, outputs, permissions, limits, Ward gate, steps, and `on_error`. Advanced YAML remains editable for power users.

- [ ] **Step 7: Build validator and dry-run panels**

Group issues by tier. Render dry-run estimates and blockers. Human gates appear as pending approval rows.

- [ ] **Step 8: Add navigation**

Expose Workflows in Cave's main navigation where roles, skills, plugins, board, and library surfaces already live.

- [ ] **Step 9: Verify Cave**

```bash
cd ~/Documents/GitHub/OpenCoven/coven-cave
pnpm test:app
pnpm test:api
pnpm tsc --noEmit
```

Expected: app tests, API tests, and TypeScript checks pass.

---

### Task 7: Cross-client round-trip verification

**Files:**
- Modify docs only if verification exposes mismatched user-facing behavior.

- [ ] **Step 1: Create from Coven Code**

Run `/workflow new`, create a workflow in `.coven/workflows/cody-review-pr/WORKFLOW.md`, validate it, and dry-run it.

- [ ] **Step 2: Open in Cave**

Open Cave Workflows and confirm the same workflow appears with the same ID, version, familiar, pattern, validation result, and dry-run estimates.

- [ ] **Step 3: Edit Cave sidecar**

Move graph nodes or change layout, save, and confirm only `WORKFLOW.cave.json` changes.

- [ ] **Step 4: Create from Cave**

Create a second workflow in Cave, save it, and validate it from Coven Code with `/workflow validate`.

- [ ] **Step 5: Export and import**

Export both workflows, import them into a clean temp `$COVEN_HOME`, and confirm validation passes or reports only missing local capabilities.

- [ ] **Step 6: Verify all repos**

Run:

```bash
cd ~/Documents/GitHub/OpenCoven/coven && cargo test -p coven-cli workflows
cd ~/Documents/GitHub/OpenCoven/coven-code/src-rust && cargo test -p claurst-commands workflow
cd ~/Documents/GitHub/OpenCoven/coven-cave && pnpm test:app && pnpm test:api && pnpm tsc --noEmit
```

Expected: all workflow-related tests and checks pass.

---

## Execution Notes

- Do not embed script bodies in workflow manifests.
- Do not store secrets in manifests, sidecars, examples, or bundles.
- Do not put Cave layout state in `WORKFLOW.md`.
- Do not commit until Val explicitly asks.
- Commit boundaries, once approved, should be one task per commit.

## Open Questions Deferred Past v1

- Scheduling grammar.
- Rollback/idempotency markers.
- Marketplace publication policy.
- Full graph-editor interactions.
- Signed external script artifacts.
