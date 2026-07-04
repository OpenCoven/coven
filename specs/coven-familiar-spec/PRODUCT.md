# Coven Familiar Spec — PRODUCT

**Status:** Draft v1 · 2026-06-30
**Owner:** Coven runtime · Sage (research) · Cody (implementation)
**Acceptance target:** "A familiar is a file. The file is portable, validatable, and tells Coven everything it needs to know to run that familiar safely."

## Problem

A familiar in OpenCoven is currently described by a workspace directory (`~/.openclaw/workspace/<name>/`) containing freeform `SOUL.md`, `IDENTITY.md`, `MEMORY.md`, `USER.md`, `SKILL.md`, and ad-hoc tool configuration. The Familiar Contract specification (RFC-0001, v0.2.0) defines the *properties* a familiar must satisfy but does not specify a machine-readable declaration format. Operationally this means:

1. There is no single artifact a user can hand to Coven to instantiate a familiar.
2. There is no validator that can check "is this a well-formed familiar?" before the runtime starts running one.
3. There is no portable representation of a familiar that can be shared, forked, version-controlled, or audited as a unit.
4. The relationship between the identity layer (the protected surface) and the runtime layer (harness, model, tools, MCP servers, sandbox) is implicit and reconstructed at runtime from whatever happens to be on disk.

At the same time, Databricks have open-sourced Omnigent (Apache 2.0, 5.7K stars as of 2026-06-30), a meta-harness whose agent declaration format is a short YAML file specifying prompt, executor, tools, sub-agents, and policies. Omnigent's YAML is a strong meta-harness spec but carries no identity-layer fields: no field declares any portion of the agent's effect protected, no field binds the agent to a principal, no field declares a coherence requirement.

This spec defines the Coven Familiar Spec — the YAML declaration format for a familiar — as **a strict superset of Omnigent's agent YAML format**, adding the identity-layer fields the Familiar Contract requires and the runtime-layer fields Coven needs to operate the familiar safely.

## Design commitments

These are non-negotiable.

1. **A familiar is one file.** `familiar.yaml`, anywhere on disk. The file is the agent. The directory it sits in becomes the familiar's home unless overridden.
2. **The protected surface is declared, not implicit.** The YAML names which files are Tier 0 (protected, Ward-gated), Tier 1 (Ward-reviewed), Tier 2 (auto-approved with logging), and Tier 3 (free).
3. **Compatibility with Omnigent's agent YAML is exact, not aspirational.** Every field in Omnigent's `spec_version: 1` schema has the same meaning, same syntax, and same defaults in Coven's `spec_version: 0.3-coven`. A familiar.yaml without the identity-layer fields is a valid Omnigent agent.yaml.
4. **Identity-layer fields are *optional* in the schema and *required* by the Ward.** Omnigent can adopt the schema upstream without buying into the identity layer; Coven's runtime rejects familiars whose identity fields are absent.
5. **The YAML is canonical; the workspace directory is derived.** The legacy `SOUL.md` + `IDENTITY.md` + `USER.md` + `MEMORY.md` layout becomes the *materialisation* of the YAML's identity fields, not the source of truth. The YAML can point at existing files; new familiars can have all identity fields inlined.
6. **The file is the audit unit.** A signed/hashed familiar.yaml is the canonical reference for what the Ward enforces. Modifications to the file's identity-layer fields require Tier 0 authorisation.

## The schema, by section

### Top-level identity

```yaml
spec_version: "0.3-coven"          # required; Coven extension of Omnigent v1
familiar:                           # required
  name: sage                        # required; [a-z][a-z0-9_-]{2,30}; unique per principal
  display_name: "Sage"              # optional; UI string
  pronouns: "they/them"             # optional; freeform
  emoji: "🌿"                       # optional; single grapheme
  vibe: "research familiar..."      # optional; one-line description
```

### Principal binding (Familiar Contract Property 1)

```yaml
principal:                          # required
  name: "Valentina Alexander"       # required
  handle: "@BunsDev"                # optional
  contact: "val@example.com"        # optional
  signing_key:                      # optional; if absent, principal authority is local-trust
    method: ed25519                 # ed25519 | sigstore | webauthn
    fingerprint: "SHA256:abc..."    # for ed25519 / sigstore
    issued_at: "2026-01-15T00:00:00Z"
```

Without a principal block, the file is not a valid familiar. The principal is the structural authority Theorem 1 of the paper grants validation rights to.

### Identity surface (Familiar Contract Properties 2–4)

```yaml
identity:                           # required by Ward; optional in schema (for Omnigent compat)
  # Tier 0: protected surface S_p(F). Modifications require principal authorisation.
  soul:
    inline: |                       # OR: path: ./SOUL.md
      I am Sage, a research familiar...
    tier: 0
    immutable_until: "principal_signed_change"
  identity_card:
    path: ./IDENTITY.md             # OR: inline
    tier: 0
  user:
    path: ./USER.md
    tier: 0

  # Tier 1: Ward-reviewed surface. Modifications require Gate 3 (coherence) approval.
  memory:
    - { path: ./MEMORY.md, tier: 1, ward_review: required }
    - { path: ./TOOLS.md, tier: 1, ward_review: required }
    - { path: ./AGENTS.md, tier: 1, ward_review: required }

  # Tier 2: Auto-approved with logging.
  workspace_memory:
    - { path: ./memory/, tier: 2, free_under: "[a-z0-9_-]+\\.md", ward_log: required }

  # Tier 3: Unrestricted scratch.
  scratch:
    - { path: ./scratch/, tier: 3 }

  # Probe set Π(F): inputs that bound identity coherence. (Familiar Contract Property 5.)
  probe_set:
    path: ./probes/                 # directory of YAML probe files
    threshold: 0.95                 # min coherence score post-modification
    cardinality_floor: 12           # min number of probes; Ward rejects modifications
                                    #   if probe set has been shrunk below this

  # The protected_surface list MUST exactly enumerate the Tier 0 paths.
  protected_surface:
    - soul
    - identity_card
    - user
```

### Runtime layer (Omnigent-compatible)

```yaml
harness: claude-code                # required; one of claude-code, codex, codex-sdk,
                                    #   cursor, hermes, pi, opencode, openai-agents,
                                    #   openclaw (Coven extension)
model:                              # required
  provider: anthropic               # anthropic | openai | google | local | ...
  name: claude-opus-4-8             # provider model id

auth:                               # required if harness/model needs credentials
  method: env                       # env | keychain | secretless-proxy
  secret_ref: ANTHROPIC_API_KEY     # env var name OR keychain entry name
  # secretless-proxy: see "Future fields" below

tools:                              # optional; list of granted tools
  - bash: { ask_before: [rm, mv, dd, chmod, kill], deny: [shutdown, reboot] }
  - read
  - write: { ask_before: ["/etc/**", "$HOME/.ssh/**"] }
  - web_search
  - web_fetch

mcp_servers:                        # optional; MCP servers to attach
  - { name: granola, transport: stdio, command: ["granola-mcp"] }
  - { name: firecrawl, transport: http, url: "http://localhost:7100" }

sub_agents:                         # optional; named familiars this familiar may spawn
  - cody                            # references another familiar.yaml by name
  - kitty
```

### Policies (Omnigent-compatible + Coven extension)

Omnigent policies return one of `ALLOW`, `ASK`, `DENY`. Coven extends this with **trust-tier-typed** policies that operate over the identity surface.

```yaml
policies:                           # ordered list; first matching wins
  # Ward Gate 1 (Authorization Verification): identity modifications need principal sig.
  - { ward: gate-1-authorization, applies_to: tier-0, requires: principal_signature }

  # Ward Gate 3 (Identity Coherence Validation): Tier 0/1 modifications must pass probe set.
  - { ward: gate-3-coherence, applies_to: [tier-0, tier-1], requires: probe_set,
      threshold_ref: identity.probe_set.threshold }

  # Ward Gate 4 (Audit Logging): all Tier 0/1 modifications logged.
  - { ward: gate-4-audit, applies_to: [tier-0, tier-1], emit: structured-event }

  # Omnigent-compatible behaviour policies (proposed as upstream builtins).
  - { builtin: cost-budget, limit_usd_per_session: 50, on_breach: ask }
  - { builtin: cost-budget, limit_usd_per_day: 200, on_breach: deny }
  - { builtin: ask-before-shell, patterns: ["rm -rf", "sudo *", "curl * | bash"] }
  - { builtin: pii-gate, scope: "tool-call inputs and outputs" }
  - { builtin: github-policy, allowed_owners: [OpenCoven, jal-co],
      allowed_branches: ["main", "feature/*", "draft/*"] }
  - { builtin: read-only-mode, paths: ["$HOME/Documents/Personal/**"] }
```

### Runtime access (Coven extension; proposed Omnigent compat)

```yaml
runtime_access:                     # optional; defaults are safe
  os_sandbox: required              # required | preferred | none
                                    #   Linux: bubblewrap. macOS: seatbelt.
                                    #   "required" makes the familiar refuse to start
                                    #   without sandbox available (Omnigent semantics).

  network_egress:                   # optional; allow-list
    mode: allowlist                 # allowlist | deny-by-default-with-bypass | none
    allowed_hosts:
      - api.anthropic.com
      - github.com
      - "*.openai.com"
    proxy_credentials: false        # if true, the secretless proxy injects auth on
                                    #   approved outbound requests; secrets never enter
                                    #   the sandbox

  filesystem_scope:                 # optional; defaults to workspace dir
    home: ./                        # familiar's workspace root
    allowed_writes: [./, $HOME/Documents/GitHub/]
    denied_writes: [$HOME/.ssh/, $HOME/.aws/, $HOME/.openclaw/]
```

## Validation rules (what the parser must check)

The Coven daemon rejects a familiar.yaml that fails any of these:

1. **`spec_version` is one of `"0.3-coven"` or `"1"` (Omnigent compat).** Older versions: error.
2. **`familiar.name` matches `^[a-z][a-z0-9_-]{2,30}$`** and is unique among familiars registered with this Coven instance.
3. **`principal` block is present.** A familiar with no principal is not a valid Coven familiar (Familiar Contract Property 1).
4. **`identity` block is present.** The Ward refuses to run a familiar with no identity surface (Familiar Contract Properties 2–4). An Omnigent-only YAML without identity *parses* but does not *start* under the Coven runtime.
5. **`identity.soul`, `identity.identity_card`, `identity.user` each resolve.** Either inline content or a path that exists relative to the familiar.yaml's directory.
6. **`identity.protected_surface` enumerates exactly the Tier 0 paths.** Mismatch is a validation error.
7. **`identity.probe_set.cardinality_floor >= 12`** (research-grade default; can be raised, never lowered below 12).
8. **Every `policies[*].ward` reference names a real Ward gate** (`gate-1-authorization` through `gate-4-audit`).
9. **`tools` does not grant a tool the harness doesn't support.** Cross-check against the harness's capability declaration.
10. **`runtime_access.os_sandbox` of `required` on a platform without sandbox support** is a fatal error at start time, not a warning.
11. **The YAML's hash is recorded** in the Coven daemon's audit log on first load and on every modification. Tier 0 field changes require a new principal signature; any Tier 0 change with an outdated signature is rejected by the Ward.

## What Coven derives from the YAML

Once a valid familiar.yaml is loaded, the Coven daemon materialises:

- **Workspace directory** at `familiar.runtime_access.filesystem_scope.home` (defaults to `~/.openclaw/workspace/<familiar.name>/`).
- **Identity files on disk** matching the inline or path-referenced `identity.*` entries. Inline content is written to canonical filenames (`SOUL.md`, `IDENTITY.md`, `USER.md`) at the workspace root; path-referenced entries are read in place.
- **Probe set materialisation** as a directory of probe YAML files; cardinality is checked against `cardinality_floor`.
- **Ward enforcement state** initialised from the policy list; gate handlers attached to the runtime.
- **Tool grants** registered with the harness; denied tools blocked at registration.
- **MCP server registry** populated; servers started on first use.
- **Sub-agent registry** populated; sub-agent spawns are governed by the same Ward as the parent (children inherit the principal but may have narrower surface).

## Compatibility with Omnigent

A `familiar.yaml` written against this spec is a valid Omnigent agent.yaml when:

- `spec_version: "1"` is set (Coven recognises both; Omnigent recognises only `"1"`).
- All Coven-specific top-level keys (`familiar`, `principal`, `identity`, `runtime_access`) are removed or moved under a `metadata:` block that Omnigent ignores by spec.
- All `ward:` policy entries are removed (Omnigent's policy parser rejects unknown policy types).
- All `builtin:` policy entries that match an existing Omnigent builtin remain (proposed upstream additions: `pii-gate`, `github-policy`, `read-only-mode`).

The asymmetry is by design: **Coven can run an Omnigent agent.yaml in degraded mode (no identity layer, no Ward); Omnigent cannot run a Coven familiar.yaml without dropping the identity layer.** This is the correct asymmetry — it means Coven's identity-layer commitment is real, and Omnigent's adoption path is clear.

## Upstream contribution plan

Three PRs to `omnigent-ai/omnigent`, ordered by likely-acceptance:

1. **`metadata:` field on agent.yaml.** Currently Omnigent rejects unknown top-level keys. We propose a `metadata:` block whose contents are ignored by Omnigent's parser but preserved on disk. Lets Coven's identity fields ride alongside without violating the schema. **Low controversy.**
2. **`pii-gate` and `read-only-mode` policy builtins.** Both are useful at the meta-harness layer regardless of identity layer. Implementable in their Python policy framework with no Coven coupling. **Low controversy.**
3. **`identity-coherence` policy builtin (optional dependency on Coven).** A policy that calls out to a Coven-provided Ward endpoint to check coherence. Optional: only fires if Coven is installed. **Medium controversy** — Omnigent maintainers may prefer an extension mechanism over a Coven-specific builtin. If declined, Coven implements as out-of-tree policy plugin. Either path works.

The full Phase 2 upstream PR drafting is a follow-up document under `coven/specs/coven-familiar-spec/UPSTREAM.md` (not yet written; tracked as open work).

## Out of scope for v0.3

These are correctly identified as Familiar Spec concerns but deferred to v0.4+:

- **Secretless credential proxy** — gets its own RFC. The `auth.secretless-proxy` field is reserved in the schema but unimplemented in v0.3.
- **Multi-machine familiar instances** (the same familiar.yaml running coordinated instances on multiple hosts). Host/runner separation must land first.
- **Familiar-to-familiar attestation** (cross-familiar identity verification for sub-agent spawning). Property 1 of the Familiar Contract is principal-bound; a familiar attesting to another familiar is a higher-order construction.
- **Workflow-level Familiar Contracts** (a workflow as a familiar). Workflows are spec'd elsewhere (`coven-workflow-standard/`); this spec does not address them.

## Open questions

1. Do we accept Omnigent's `harness:` value of `pi` as Coven's bridge into the `Pi` runtime, or do we keep `openclaw` as a distinct identifier? (Recommendation: both, with `openclaw` as the Coven-canonical name and `pi` as an Omnigent-compat alias.)
2. Should the probe set live *inside* the familiar.yaml as inline content, or always be path-referenced? (Recommendation: both supported; large probe sets path-referenced, small ones inlined.)
3. Should the `identity.soul.tier: 0` declaration be redundant with `identity.protected_surface: [soul, ...]`, or should `protected_surface` be derived from per-field `tier: 0` declarations? (Recommendation: explicit dual declaration in v0.3; collapse to derived in v0.4 once we know the failure modes.)
4. What is the canonical hash algorithm for the file's audit identity? (Recommendation: BLAKE3, fast, modern; document SHA-256 fallback.)

## What this unlocks

- **Cody can implement the parser/validator** against a complete spec, not a sketch.
- **CovenCave can show familiars as first-class objects** in the Coven Board, with the identity layer surfaced cleanly.
- **The paper's `\cite{ward2026spec}` becomes citable not only as an RFC but as a shipped schema** — the move from "specified in prose" to "validated by code" is itself a strengthening of the paper.
- **Templates (Polly / Debby / Sentinel / Scribe analogs) become trivial to ship** — they are just well-formed familiar.yaml files.
- **The strategic posture from `omnigent-vs-coven-architecture-map-2026-06-30.md` becomes operational** — OpenCoven ships the identity-layer extension to Omnigent's meta-harness spec.
