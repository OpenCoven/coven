# OpenCoven and Omnigent: Positioning

**One sentence:** OpenCoven is the identity-layer specification for the meta-harness ecosystem; Omnigent is the strongest production meta-harness in that ecosystem; the two layers compose by architectural construction.

## What Omnigent is

Databricks open-sourced Omnigent (Apache 2.0, `omnigent-ai/omnigent`) in mid-2026 as a **meta-harness** — a layer above the execution runtimes (Claude Code, OpenAI Codex, Cursor, Hermes, Pi, OpenCode) that:

- Sandboxes each agent session at the OS level (`bubblewrap` on Linux, mandatory; `seatbelt` on macOS).
- Exposes a uniform API across heterogeneous execution backends.
- Evaluates every tool call and language-model request against a three-tier policy hierarchy (server-wide / agent-config / session) returning `ALLOW`, `ASK`, or `DENY`.
- Maintains stateful policy state across the session (cost budgets, tool counters, risk scores).
- Treats the agent itself as a short YAML declaration whose `harness:` field can be swapped in one line.
- Provides session sharing, forking, comments, and stream APIs.
- Ships an OpenAPI specification and a Python client SDK.
- Targets cloud sandboxes (Modal, Daytona, Fly.io, E2B, Databricks) alongside local execution.

The Databricks thesis sentence is: *"we believe people will soon work with agents through this new layer, the meta-harness."*

This is a strong claim about a real layer. The infrastructure is mature; the positioning is industrial.

## What Omnigent is not

The Omnigent agent YAML carries:

- A `prompt:` field — system-prompt text.
- An `executor:` field — which runtime to use.
- A `tools:` block — what tools are granted.
- A `sub_agents:` block — what other agents can be spawned.
- A `policies:` block — what behaviour is allowed.

It carries no field that:

- Declares any portion of the agent's effect **protected** from the agent's own self-modifications.
- Binds the agent to a **principal** who authorised its character.
- Declares a **coherence requirement** against which an updated version of the agent must be validated before replacing its predecessor.

The compilation target is behaviour. Identity is not part of what is composed.

This is not a defect in Omnigent. It is the layer Omnigent operates at. The meta-harness asks *what may this session do?* — and answers well. It does not ask *which portion of the agent itself is structurally not subject to its own self-modifications?* — because that is a different question, addressed at a different layer.

## What OpenCoven is

OpenCoven is the **identity layer** for agentic systems. It specifies, for a given agent (called a *familiar*), which portion of that agent's constitution is structurally reserved to the principal who deployed it and not subject to the agent's own self-modification proposals.

OpenCoven ships:

- **The Familiar Contract** (RFC-0001, v0.2.0) — a formal specification of five properties an agent must satisfy to be called a familiar: principal binding, protected surface declaration, structural authority above the protected surface, coherence validation under self-modification, and probe-set bounded identity.
- **The Ward** — the runtime enforcement layer with four gates: authorisation verification, surface discrimination, identity coherence validation, audit logging.
- **A workspace convention** — `SOUL.md`, `IDENTITY.md`, `USER.md`, `MEMORY.md` as the canonical files that materialise the protected surface, the role declaration, the principal binding, and the curated memory.
- **The Familiar Spec YAML** (v0.3, in draft as of 2026-06-30) — the machine-readable declaration format that turns the Familiar Contract from prose into a parseable, validatable schema.
- **A daemon** (the Coven runtime) that loads familiar.yaml files, materialises the workspace, attaches the Ward, and supervises the principal-agent boundary across sessions.
- **CovenCave** — the local-first cockpit for working with familiars.

OpenCoven is **local-first by core principle**. The familiar lives on the user's machine. The principal binding is real, not delegated. The protected surface is in the user's filesystem, under the user's control.

## How they compose

The Coven Familiar Spec YAML is **a strict superset of Omnigent's agent YAML format**.

- Every field in Omnigent's `spec_version: 1` schema has the same meaning, same syntax, and same defaults in Coven's `spec_version: 0.3-coven`.
- Identity-layer fields (`principal:`, `identity:`, `runtime_access:`) are *optional in the schema* and *required by the Ward*. A familiar.yaml without the identity fields parses as a valid Omnigent agent.yaml; a Coven familiar without the identity fields refuses to start.
- Ward gate policies (`ward: gate-1-authorization`, etc.) co-exist with Omnigent's behaviour policies (`builtin: cost-budget`, etc.) in the same `policies:` list. The Ward gates operate over identity modifications; the behaviour policies operate over tool calls.

The intended deployment pattern, end-state, is:

```
User
 └── Principal authority over their familiars
      └── Coven daemon (identity layer; Ward enforcement)
           └── Omnigent meta-harness (session sandbox; policy engine; runtime adapter)
                └── Execution runtime (Claude Code, Codex, Cursor, Hermes, Pi, etc.)
                     └── Language model
```

Coven runs *above* Omnigent, not parallel to it. Omnigent's policy engine evaluates behaviour-layer policies; Coven's Ward evaluates identity-layer policies; both sit between the principal and the runtime, at architecturally distinct layers, addressing structurally distinct questions.

## Why OpenCoven is not "Omnigent done locally"

The honest answer the meta-harness alone does not provide:

- **An Omnigent session has no principal binding declared.** Server-wide policies are set by administrators, agent-config by developers, session-level by users — but the *agent itself* has no declared principal who authored its character and retains structural authority over its identity. The three-tier policy hierarchy is real and well-designed at the behaviour layer; the principal-agent identity relationship is not modelled.
- **An Omnigent agent's identity is its system prompt.** Modifying the prompt is the same operation as modifying any other YAML field. The principal who deployed the agent has no structural recourse if the agent's behaviour over time drifts from the principal's intent — no protected surface declaration, no coherence requirement, no Ward to enforce them.
- **An Omnigent session is sandboxed; the agent is not.** Sandboxing operates over filesystem and network access during one session. It does not operate over the agent's identity across sessions, across self-improvement loops, across model upgrades, across sub-agent delegation.

These are not gaps Databricks has chosen to leave open arbitrarily. They are gaps that exist because the meta-harness layer does not *need* to address them in order to function as a meta-harness. They become urgent only when the question shifts from *what does this session do?* to *what is this agent, and what is it allowed to become?* — the question the Familiar Contract is specified to answer.

## Why Omnigent is not "the platform OpenCoven should be built on"

The honest answer the identity layer alone does not provide:

- **A clean session lifecycle, host/runner topology, and operational runtime.** Omnigent has shipped these as mature mechanics; Coven has them in spec and in early-stage CovenCave implementation.
- **An OpenAPI-stable client surface.** Omnigent ships `openapi.json` and a published SDK; Coven has REST endpoints without a formal schema.
- **A clean policy engine with stateful contextual policies.** Omnigent's policy engine is, at the behaviour layer, the strongest production analog of the Ward; Coven's runtime currently has Ward gates specified but unevenly implemented.
- **A clean host/runner split.** Omnigent has it; Coven currently uses an SSH-OpenClaw workaround that requires local patches and breaks codesigning.

These are real Coven gaps. The correct posture is not "build them ourselves from scratch" — Databricks has invested industrial engineering at the meta-harness layer; replicating that work is bad use of effort. The correct posture is to **adopt Omnigent at the meta-harness layer and contribute the identity-layer extension upstream**, so that:

1. Coven gains a mature meta-harness without re-implementing one.
2. Omnigent gains an identity layer it does not currently specify.
3. Users gain a complete principal-agent contract — behaviour-layer enforcement plus identity-layer enforcement — with one declarative file (`familiar.yaml`) describing both.

## The composition, stated formally

For a familiar $F$ deployed by principal $P$:

- The **Familiar Contract** $C(F, P)$ specifies which portion of $F$'s constitution is reserved to $P$.
- The **Ward** $W$ enforces $C(F, P)$ at the identity layer.
- The **Meta-harness** $M$ (e.g., Omnigent) enforces behaviour-layer policies during $F$'s execution.
- The **Runtime** $R$ (e.g., Claude Code) executes $F$'s decisions, mediated by $M$.

The principal-agent contract for $F$ is the composition:

$$ \text{Contract}(F, P) = C(F, P) \circ M(F) \circ R(F) $$

— enforced by the Ward at the identity layer, by Omnigent at the behaviour layer, by the runtime sandbox at the execution layer. Each layer addresses a structurally distinct question; the composition addresses the full principal-agent relationship.

This is not a slogan. It is the architecture the Familiar Contract paper specifies, the architecture the Coven Familiar Spec YAML implements, and the architecture Omnigent's policy hierarchy supports by construction.

## What this means operationally

For OpenCoven:

- **Tier 2 work (post-paper-submission) is the identity-layer extension.** Familiar Spec YAML, Ward as Omnigent policy builtin, host/runner registration through Omnigent's topology. Detailed roadmap in the synthesis doc.
- **CovenCave's UX commitments do not change.** Coven remains the local-first, identity-aware, familiar-first cockpit. Adopting Omnigent at the meta-harness layer does not collapse Coven into Omnigent's product surface.
- **OpenCoven's distinctive contribution is the identity layer.** Not the cockpit (CovenCave is a UX expression of that contribution), not the runtime (OpenClaw is a runtime adapter), not the daemon plumbing (necessary but not differentiating). The identity layer is what no one else has specified, and what the field will increasingly need as agents become more capable.

For external readers:

- **OpenCoven is not a less-mature Omnigent.** It is a more-specified identity layer.
- **CovenCave is not an early-stage cockpit competitor.** It is the local-first UX for the identity layer.
- **The Familiar Contract is not a hypothetical.** It is a published RFC (v0.2.0) with a Familiar Spec YAML schema, a Ward enforcement layer, and a peer-reviewable paper specifying the formal framework.

## Honest assessment of where Coven is, today

- **Identity layer specification:** Strong. RFC-0001 v0.2.0 published, peer-reviewable paper near arXiv submission, Familiar Spec YAML in draft.
- **Identity layer implementation:** Partial. Ward gates specified, parser/validator unwritten, runtime integration incomplete.
- **Meta-harness layer:** Early. Coven daemon has session lifecycle and OpenClaw-runtime integration; host/runner separation not yet implemented; OpenAPI schema not extracted.
- **Cockpit (CovenCave):** Shipping fast but young. The local-first UX commitment is real; the engineering discipline needed for fleet-scale use is uneven.
- **Runtime adapter (OpenClaw):** Mature for the harnesses it supports; the SSH-OpenClaw patch is the most visible brittleness.

The trajectory: **publish the Familiar Contract paper → ship Familiar Spec YAML v0.3 → propose identity-coherence policy builtin upstream to Omnigent → adopt Omnigent's host/runner topology for Coven daemon → restructure CovenCave on top of Omnigent's session model.**

The destination: **OpenCoven is the identity-layer extension of the meta-harness ecosystem.** Whoever owns that layer in the ecosystem owns the next generation of agentic principal-agent relationships. Coven is currently the only project specifying it.
