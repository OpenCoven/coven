# Familiar Identity Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first-class familiar identity resolver so Coven turns `familiar.yaml` declarations into typed effective familiar identity for sessions.

**Architecture:** Introduce a schema-backed manifest and Rust resolver beside the existing familiar context path, then swap harness launch identity injection to use resolved `EffectiveFamiliar`. Keep governance and capability intent as policy metadata, not raw permission grants.

**Tech Stack:** Rust Coven CLI/daemon, Serde, JSON Schema, YAML/TOML compatibility, existing harness launch path, Markdown docs.

**Spec:** `docs/superpowers/specs/2026-06-15-familiar-identity-model.md`

---

## File Structure

- Create `schemas/familiar/coven.familiar.v1.schema.json`: manifest schema for declared familiar identity.
- Create `schemas/familiar/examples/nova.familiar.yaml`: representative fixture for a full identity declaration.
- Create `crates/coven-cli/src/familiar_identity/mod.rs`: module exports and public resolver entrypoint.
- Create `crates/coven-cli/src/familiar_identity/manifest.rs`: Serde structs and YAML loading.
- Create `crates/coven-cli/src/familiar_identity/effective.rs`: normalized `EffectiveFamiliar` structs and preamble rendering.
- Create `crates/coven-cli/src/familiar_identity/resolver.rs`: precedence, validation, and provenance assembly.
- Modify `crates/coven-cli/src/harness.rs`: replace shallow `FamiliarContext` construction with resolver output.
- Modify `crates/coven-cli/src/api.rs`: expose resolved identity fields in familiar/session launch responses where appropriate.
- Modify `crates/coven-cli/src/main.rs`: add `coven familiars resolve <id> --json`.
- Modify `docs/familiars/identity.md`: replace stub with public explanation.

---

### Task 1: Add schema and example manifest

**Files:**
- Create: `schemas/familiar/coven.familiar.v1.schema.json`
- Create: `schemas/familiar/examples/nova.familiar.yaml`

- [ ] **Step 1: Write the schema file**

Create `schemas/familiar/coven.familiar.v1.schema.json`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.opencoven.ai/familiar/coven.familiar.v1.schema.json",
  "title": "Coven Familiar Manifest",
  "type": "object",
  "additionalProperties": false,
  "required": ["schema_version", "id", "display_name", "identity"],
  "properties": {
    "schema_version": { "const": "coven.familiar.v1" },
    "id": {
      "type": "string",
      "pattern": "^[a-z][a-z0-9-]{0,63}$"
    },
    "display_name": {
      "type": "string",
      "minLength": 1,
      "maxLength": 80
    },
    "identity": {
      "type": "object",
      "additionalProperties": false,
      "required": ["purpose", "roles"],
      "properties": {
        "purpose": { "type": "string", "minLength": 1, "maxLength": 500 },
        "roles": {
          "type": "array",
          "minItems": 1,
          "items": { "type": "string", "minLength": 1, "maxLength": 80 }
        },
        "principles": {
          "type": "array",
          "items": { "type": "string", "minLength": 1, "maxLength": 180 }
        },
        "relationships": {
          "type": "object",
          "additionalProperties": true
        }
      }
    },
    "memory": {
      "type": "object",
      "additionalProperties": true
    },
    "skills": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "required": { "type": "array", "items": { "type": "string" } },
        "preferred": { "type": "array", "items": { "type": "string" } }
      }
    },
    "workflows": {
      "type": "object",
      "additionalProperties": true
    },
    "capability_intent": {
      "type": "object",
      "additionalProperties": { "type": "string" }
    },
    "governance": {
      "type": "object",
      "additionalProperties": true
    }
  }
}
```

- [ ] **Step 2: Add the fixture**

Create `schemas/familiar/examples/nova.familiar.yaml`:

```yaml
schema_version: coven.familiar.v1
id: nova
display_name: Nova
identity:
  purpose: Trusted companion for building, organizing, remembering, and moving OpenCoven forward.
  roles:
    - orchestrator
    - maintainer-companion
  principles:
    - Truth before confidence.
    - Prefer local execution when it protects privacy and agency.
  relationships:
    owner:
      kind: sovereign-source
      display_name: Valentina
memory:
  profile: continuity-curated
  scopes:
    - long_term
    - daily_notes
    - project_context
skills:
  required:
    - verification-before-completion
  preferred:
    - writing-plans
capability_intent:
  filesystem: local_workspace
  github: maintainer_assist
governance:
  autonomy: supervised
  external_actions: require_approval
```

- [ ] **Step 3: Validate basic file syntax**

Run:

```bash
jq empty schemas/familiar/coven.familiar.v1.schema.json
ruby -e 'require "yaml"; YAML.safe_load(File.read(ARGV[0]), permitted_classes: [], aliases: false)' schemas/familiar/examples/nova.familiar.yaml
```

Expected: both commands exit 0.

- [ ] **Step 4: Commit**

```bash
git add schemas/familiar/coven.familiar.v1.schema.json schemas/familiar/examples/nova.familiar.yaml
git commit -m "feat(familiars): add familiar manifest schema"
```

---

### Task 2: Parse declared familiar manifests

**Files:**
- Create: `crates/coven-cli/src/familiar_identity/mod.rs`
- Create: `crates/coven-cli/src/familiar_identity/manifest.rs`
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Add module declaration**

In `crates/coven-cli/src/main.rs`, add:

```rust
mod familiar_identity;
```

- [ ] **Step 2: Create module exports**

Create `crates/coven-cli/src/familiar_identity/mod.rs`:

```rust
pub mod manifest;

pub use manifest::{FamiliarIdentity, FamiliarManifest, FamiliarSkills};
```

- [ ] **Step 3: Write parser tests first**

Create `crates/coven-cli/src/familiar_identity/manifest.rs` with tests first:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FamiliarManifest {
    pub schema_version: String,
    pub id: String,
    pub display_name: String,
    pub identity: FamiliarIdentity,
    #[serde(default)]
    pub skills: FamiliarSkills,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FamiliarIdentity {
    pub purpose: String,
    pub roles: Vec<String>,
    #[serde(default)]
    pub principles: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FamiliarSkills {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub preferred: Vec<String>,
}

impl FamiliarManifest {
    pub fn from_yaml(source: &str) -> Result<Self> {
        let manifest: Self = serde_yaml::from_str(source).context("failed to parse familiar manifest YAML")?;
        anyhow::ensure!(
            manifest.schema_version == "coven.familiar.v1",
            "unsupported familiar schema version: expected \"coven.familiar.v1\", found \"{}\"",
            manifest.schema_version
        );
        anyhow::ensure!(!manifest.id.trim().is_empty(), "familiar id is required");
        anyhow::ensure!(!manifest.identity.roles.is_empty(), "familiar roles are required: at least one role must be declared");
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nova_fixture() {
        let source = include_str!("../../../../schemas/familiar/examples/nova.familiar.yaml");
        let manifest = FamiliarManifest::from_yaml(source).unwrap();
        assert_eq!(manifest.schema_version, "coven.familiar.v1");
        assert_eq!(manifest.id, "nova");
        assert!(manifest.identity.roles.contains(&"orchestrator".to_string()));
        assert!(manifest.skills.required.contains(&"verification-before-completion".to_string()));
    }

    #[test]
    fn rejects_missing_roles() {
        let err = FamiliarManifest::from_yaml(
            r#"
schema_version: coven.familiar.v1
id: nova
display_name: Nova
identity:
  purpose: Test
  roles: []
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("roles"));
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p coven-cli familiar_identity::manifest
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/familiar_identity
git commit -m "feat(familiars): parse familiar identity manifests"
```

---

### Task 3: Resolve effective familiar identity

**Files:**
- Create: `crates/coven-cli/src/familiar_identity/effective.rs`
- Create: `crates/coven-cli/src/familiar_identity/resolver.rs`
- Modify: `crates/coven-cli/src/familiar_identity/mod.rs`

- [ ] **Step 1: Export effective and resolver modules**

Update `crates/coven-cli/src/familiar_identity/mod.rs`:

```rust
pub mod effective;
pub mod manifest;
pub mod resolver;

pub use effective::{EffectiveFamiliar, EffectiveFamiliarProvenance};
pub use manifest::{FamiliarIdentity, FamiliarManifest, FamiliarSkills};
pub use resolver::{resolve_familiar_from_manifest, FamiliarResolveRequest};
```

- [ ] **Step 2: Add effective familiar structs**

Create `crates/coven-cli/src/familiar_identity/effective.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveFamiliar {
    pub schema_version: String,
    pub id: String,
    pub display_name: String,
    pub roles: Vec<String>,
    pub principles: Vec<String>,
    pub identity_preamble: String,
    pub provenance: EffectiveFamiliarProvenance,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveFamiliarProvenance {
    pub manifest_path: Option<String>,
    pub resolved_from: Vec<String>,
}
```

- [ ] **Step 3: Add resolver tests and implementation**

Create `crates/coven-cli/src/familiar_identity/resolver.rs`:

```rust
use super::effective::{EffectiveFamiliar, EffectiveFamiliarProvenance};
use super::manifest::FamiliarManifest;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FamiliarResolveRequest {
    pub manifest_path: Option<String>,
}

pub fn resolve_familiar_from_manifest(
    manifest: FamiliarManifest,
    request: FamiliarResolveRequest,
) -> EffectiveFamiliar {
    let role_summary = manifest.identity.roles.join(", ");
    let identity_preamble = format!(
        "[Identity: You are {name}, a familiar with roles: {roles}. Purpose: {purpose}. Respond as {name}, not as the underlying tool.]",
        name = manifest.display_name,
        roles = role_summary,
        purpose = manifest.identity.purpose,
    );

    EffectiveFamiliar {
        schema_version: "coven.effective_familiar.v1".to_string(),
        id: manifest.id,
        display_name: manifest.display_name,
        roles: manifest.identity.roles,
        principles: manifest.identity.principles,
        identity_preamble,
        provenance: EffectiveFamiliarProvenance {
            manifest_path: request.manifest_path,
            resolved_from: vec!["manifest".to_string()],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::familiar_identity::manifest::FamiliarManifest;

    #[test]
    fn resolves_effective_identity_with_preamble_and_provenance() {
        let manifest = FamiliarManifest::from_yaml(include_str!(
            "../../../../schemas/familiar/examples/nova.familiar.yaml"
        ))
        .unwrap();

        let effective = resolve_familiar_from_manifest(
            manifest,
            FamiliarResolveRequest {
                manifest_path: Some("schemas/familiar/examples/nova.familiar.yaml".to_string()),
            },
        );

        assert_eq!(effective.schema_version, "coven.effective_familiar.v1");
        assert_eq!(effective.id, "nova");
        assert!(effective.identity_preamble.contains("You are Nova"));
        assert!(effective.identity_preamble.contains("orchestrator"));
        assert_eq!(
            effective.provenance.manifest_path.as_deref(),
            Some("schemas/familiar/examples/nova.familiar.yaml")
        );
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p coven-cli familiar_identity
```

Expected: manifest and resolver tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/coven-cli/src/familiar_identity
git commit -m "feat(familiars): resolve effective familiar identity"
```

---

### Task 4: Wire resolved identity into harness launch

**Files:**
- Modify: `crates/coven-cli/src/harness.rs`

- [ ] **Step 1: Add an adapter from `EffectiveFamiliar` to existing launch context**

In `crates/coven-cli/src/harness.rs`, keep the public launch API stable by adding:

```rust
impl From<crate::familiar_identity::EffectiveFamiliar> for FamiliarContext {
    fn from(effective: crate::familiar_identity::EffectiveFamiliar) -> Self {
        Self {
            id: effective.id,
            name: effective.display_name,
            role: Some(effective.roles.join(", ")),
        }
    }
}
```

- [ ] **Step 2: Add a regression test**

Near the existing `FamiliarContext` tests in `harness.rs`, add:

```rust
#[test]
fn effective_familiar_converts_to_launch_context() {
    let manifest = crate::familiar_identity::FamiliarManifest::from_yaml(include_str!(
        "../../../../schemas/familiar/examples/nova.familiar.yaml"
    ))
    .unwrap();
    let effective = crate::familiar_identity::resolve_familiar_from_manifest(
        manifest,
        crate::familiar_identity::FamiliarResolveRequest::default(),
    );
    let context: FamiliarContext = effective.into();
    assert_eq!(context.id, "nova");
    assert_eq!(context.name, "Nova");
    assert!(context.identity_preamble().contains("Nova"));
    assert!(context.identity_preamble().contains("orchestrator"));
}
```

- [ ] **Step 3: Run targeted tests**

```bash
cargo test -p coven-cli harness::effective_familiar_converts_to_launch_context
```

If Cargo rejects multiple filters, run:

```bash
cargo test -p coven-cli effective_familiar_converts_to_launch_context
```

Expected: new test passes and existing harness identity tests continue to pass.

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/harness.rs
git commit -m "feat(familiars): use resolved identity for harness context"
```

---

### Task 5: Add CLI inspection command

**Files:**
- Modify: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Add CLI test or snapshot**

If the repo has CLI command tests, add a test that invokes:

```bash
coven familiars resolve nova --manifest schemas/familiar/examples/nova.familiar.yaml --json
```

Expected JSON contains:

```json
{
  "schemaVersion": "coven.effective_familiar.v1",
  "id": "nova",
  "displayName": "Nova"
}
```

If no CLI command test harness exists, add a unit test for the handler function before wiring the Clap command.

- [ ] **Step 2: Implement command handler**

Add a `familiars resolve` command that:

1. accepts `<id>`;
2. accepts optional `--manifest <path>`;
3. loads the manifest;
4. verifies the loaded manifest ID matches `<id>`;
5. prints `EffectiveFamiliar` as pretty JSON when `--json` is present.

- [ ] **Step 3: Run verification**

```bash
cargo test -p coven-cli familiar_identity
cargo run -p coven-cli -- familiars resolve nova --manifest schemas/familiar/examples/nova.familiar.yaml --json
```

Expected: tests pass and command prints `coven.effective_familiar.v1`.

- [ ] **Step 4: Commit**

```bash
git add crates/coven-cli/src/main.rs
git commit -m "feat(familiars): inspect resolved identity from the CLI"
```

---

### Task 6: Update public docs

**Files:**
- Modify: `docs/familiars/identity.md`
- Modify if needed: `docs/familiars/index.md`

- [ ] **Step 1: Replace identity stub**

Write `docs/familiars/identity.md` with:

```md
---
summary: "How familiar identity is declared, resolved, and carried across sessions."
read_when:
  - Understanding why a familiar is more than a harness prompt
  - Designing a familiar that survives model or provider changes
title: "Identity"
description: "Identity for OpenCoven familiars: declared familiar manifests, effective familiar resolution, relationships, memory profile, and governance."
---

Familiar identity is the layer that lets a named agent remain itself while its harness, model, tools, or client changes.

```text
familiar.yaml
  -> identity resolver
  -> effective familiar
  -> session
```

## Identity is not configuration

Configuration chooses commands, models, paths, and tools. Identity declares purpose, roles, principles, relationships, memory profile, and governance. Coven resolves identity before launching a session so clients and harnesses receive an explicit effective familiar instead of a loose prompt string.

## Soul, mind, hands

- Soul: purpose, roles, principles, relationships.
- Mind: skills, workflows, and memory profile.
- Hands: tools, harnesses, MCP servers, browser, desktop, and GitHub.

Hands can change without changing the familiar.

## Relationships matter

Relationship affects permissions, memory behavior, autonomy, communication style, and delegation. An engineer familiar acting as a maintainer should not resolve the same governance posture as the same engineer familiar acting as an apprentice.

## Governance

Capability intent is not a permission grant. It is resolved into policy metadata that the daemon, clients, approval gates, and repository rules can enforce.
```

- [ ] **Step 2: Run docs sanity checks**

```bash
node -e 'const fs=require("fs"); const text=fs.readFileSync("docs/familiars/identity.md","utf8"); const banned=["Stub " + "— fill in"]; for (const marker of banned) if (text.includes(marker)) { console.error(marker); process.exit(1); }'
git diff --check docs/familiars/identity.md
```

Expected: the placeholder scan exits 0 and `git diff --check` exits 0.

- [ ] **Step 3: Commit**

```bash
git add docs/familiars/identity.md docs/familiars/index.md
git commit -m "docs(familiars): document identity resolution"
```

---

## Final Verification

Run:

```bash
cargo test -p coven-cli familiar_identity
cargo test -p coven-cli harness
jq empty schemas/familiar/coven.familiar.v1.schema.json
ruby -e 'require "yaml"; YAML.safe_load(File.read(ARGV[0]), permitted_classes: [], aliases: false)' schemas/familiar/examples/nova.familiar.yaml
node -e 'const fs=require("fs"); const files=["docs/superpowers/plans/2026-06-15-familiar-identity-model.md","docs/superpowers/specs/2026-06-15-familiar-identity-model.md","docs/familiars/identity.md"]; const banned=["Stub " + "— fill in","TB" + "D","implement " + "later"]; for (const file of files) { const text=fs.readFileSync(file,"utf8"); for (const marker of banned) if (text.includes(marker)) { console.error(`${file}: ${marker}`); process.exit(1); } }'
git diff --check
```

Expected:

- Rust tests pass.
- Schema parses.
- YAML fixture parses.
- Placeholder scan returns no matches in the new docs.
- Diff whitespace check exits 0.
