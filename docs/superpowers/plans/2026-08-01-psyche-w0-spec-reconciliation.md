# Psyche W0 Specification Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reconcile every Psyche companion specification with the approved surface-neutral familiar-runtime baseline and prove G1 specification coherence without creating implementation issues or production code.

**Architecture:** Treat `RUNTIME_DESIGN.md` and `DECISION_DOSSIER.md` as the approved sources of truth. Preserve each companion document's specialist detail, but replace Telegram-only product framing, incorrect authority assignments, and the superseded P0-P6 program with the approved surface-neutral ownership model, W0-W11 workstreams, and G0-G12 gates.

**Tech Stack:** Markdown specifications, repository documentation guardrails, Git diff validation, Rust workspace CI gates, Python secret/privacy scanners.

---

### Task 1: Freeze W0 decisions in the product specification

**Files:**
- Modify: `specs/psyche/PRODUCT.md`

- [x] **Step 1:** Replace the Telegram-only product decision with the approved surface-neutral familiar-runtime definition.
- [x] **Step 2:** Preserve Telegram as the first production adapter and parity target, not as Psyche's permanent boundary.
- [x] **Step 3:** Record the approved ownership matrix and the W0 decisions for service objectives, repository timing, first-release multi-agent scope, and OpenClaw compatibility.
- [x] **Step 4:** Replace the G0-G6 release table with G0-G12; make G1 the repository/W1 gate and G3 the implementation-planning gate.

### Task 2: Reconcile the technical architecture

**Files:**
- Modify: `specs/psyche/TECH.md`

- [x] **Step 1:** Add the surface-neutral intent, graph, attempt, verification, and surface contracts from the approved runtime design.
- [x] **Step 2:** Correct identity and authority ownership: Psyche resolves familiar identity and surface principals; Coven validates execution bindings and enforces only its versioned execution/resource contracts.
- [x] **Step 3:** Keep Telegram schemas and delivery state explicitly adapter-scoped.
- [x] **Step 4:** Align repository/crate boundaries, capability gating, retention formulas, cancellation acknowledgement, and artifact references with W0 decisions.

### Task 3: Expand the threat model and preserve Telegram parity as adapter evidence

**Files:**
- Modify: `specs/psyche/THREAT_MODEL.md`
- Modify: `specs/psyche/TELEGRAM_PARITY.md`

- [x] **Step 1:** Add graph, delegation, lease, budget, verifier, sealed-evidence, add-on metadata, principal-mapping, and multi-surface threats with testable controls.
- [x] **Step 2:** Remove any implication that a generator certifies itself or that capability metadata grants authority.
- [x] **Step 3:** Define the parity ledger as Telegram adapter conformance over common surface contracts.
- [x] **Step 4:** Map adapter evidence to G8-G11 while preserving every required/deferred/rejected Telegram classification.

### Task 4: Convert Coven prerequisites into an evidence-gated W1 audit contract

**Files:**
- Modify: `specs/psyche/COVEN_PREREQUISITES.md`

- [x] **Step 1:** Remove proposed capability names presented as already accepted implementation work.
- [x] **Step 2:** Classify prerequisite categories as current, current-but-undocumented, planned, optional, or rejected only through W1 code/test evidence.
- [x] **Step 3:** Separate the single-node and production multi-agent conformance profiles.
- [x] **Step 4:** Preserve fail-closed, fake/real-suite parity, adoption ambiguity, cancellation acknowledgement, artifact association, and restart evidence requirements.

### Task 5: Replace the superseded program plan

**Files:**
- Modify: `specs/psyche/PLAN.md`

- [x] **Step 1:** Replace P0-P6 with W0-W11 and the approved dependency graph.
- [x] **Step 2:** Replace G0-G6 with G0-G12 and capability-to-gate mapping.
- [x] **Step 3:** Preserve child-plan discipline, focused delivery, migration, canary, rollback, and release evidence.
- [x] **Step 4:** Mark W0 complete only when all six companions share the same product definition and ownership matrix; keep W1 evidence work separate and prohibit implementation issues before G3.

### Task 6: Self-review and verification

**Files:**
- Verify: `specs/psyche/*.md`
- Verify: repository root

- [x] **Step 1:** Scan for stale Telegram-only product claims, P0-P6 gates, contradictory ownership, placeholders, and broken relative links.
- [x] **Step 2:** Run `git diff --check` and inspect the complete scoped diff.
- [x] **Step 3:** Run `cargo fmt --check`.
- [x] **Step 4:** Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] **Step 5:** Run `cargo test --workspace --locked`.
- [x] **Step 6:** Run `python scripts/check-secrets.py`.
- [x] **Step 7:** Stage only the W0 documentation paths, run `python3 scripts/check-coven-privacy.py --staged`, and commit the verified changes.
