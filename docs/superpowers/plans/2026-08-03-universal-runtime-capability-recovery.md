# Universal Runtime Capability Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Rust-owned effective runtime capability surface for supported harnesses without changing the meaning of existing harness-scan routes or widening the supported harness set.

**Architecture:** Freeze the public `coven.daemon.v1` and session-lifecycle vocabulary first, then layer a new effective runtime-descriptor resolver on top of existing Rust launch specs and raw harness-scan manifests. Keep `/api/v1/capabilities/harnesses` as factual native scan output, add a distinct runtime descriptor read surface, and enforce required capabilities in Rust before launch.

**Tech Stack:** Rust (`crates/coven-cli/src/api.rs`, `harness.rs`, `capabilities.rs`, `daemon.rs`, `store.rs`), markdown API docs, existing Rust tests, secret/privacy guardrails.

---

### Task 1: Freeze the O1 public contract vocabulary

**Files:**
- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/reference/api-contract.md`
- Modify: `docs/reference/api.md`
- Modify: `docs/SESSION-LIFECYCLE.md`
- Modify: `crates/coven-cli/src/api.rs`
- Test: `crates/coven-cli/src/api.rs`

- [ ] **Step 1:** Update the public docs so they use one canonical external API version vocabulary: `coven.daemon.v1` for the contract name and `/api/v1` for the route prefix.
- [ ] **Step 2:** Enumerate the exact persisted session statuses used by Rust (`created`, `running`, `completed`, `failed`, `killed`, `idle`, `orphaned`) and clearly distinguish archive state from terminal state.
- [ ] **Step 3:** Change `GET /api/v1/api-version` in `crates/coven-cli/src/api.rs` so the payload reports the named contract string instead of the bare `v1` token.
- [ ] **Step 4:** Add or update route tests in `crates/coven-cli/src/api.rs` covering the named version response and the current unknown-version failure path.
- [ ] **Step 5:** Run focused tests for the version routes and session-lifecycle docs assertions, then commit only the O1 contract-freeze changes.

### Task 2: Introduce Rust-owned effective runtime descriptor types

**Files:**
- Create: `crates/coven-cli/src/runtime_descriptors.rs`
- Modify: `crates/coven-cli/src/main.rs`
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `crates/coven-cli/src/harness.rs`
- Test: `crates/coven-cli/src/runtime_descriptors.rs`

- [ ] **Step 1:** Add `runtime_descriptors.rs` with the new derived authority types: `EffectiveRuntimeDescriptor`, `RuntimeSupportClass`, `RuntimeAdmission`, `AvailabilityDescriptor`, `CapabilityDescriptor`, `CapabilityState`, `CapabilityReason`, and `NativeIntegrationSummary`.
- [ ] **Step 2:** Expose the new module from `main.rs` or the existing module tree without moving launch authority out of `harness.rs`.
- [ ] **Step 3:** Add unit tests that construct descriptors for `codex`, `claude`, `copilot`, and `coven-code`, asserting that only the first three classify as `supported_runtime`.
- [ ] **Step 4:** Add a regression test proving unknown harness ids never synthesize a descriptor.
- [ ] **Step 5:** Commit the type layer separately before wiring any API route.

### Task 3: Build the supported-runtime resolver from existing Rust facts

**Files:**
- Modify: `crates/coven-cli/src/runtime_descriptors.rs`
- Modify: `crates/coven-cli/src/harness.rs`
- Modify: `crates/coven-cli/src/capabilities.rs`
- Test: `crates/coven-cli/src/runtime_descriptors.rs`

- [ ] **Step 1:** Add a resolver that combines the bundled `HarnessCommandSpec` data with local availability and the existing `HarnessCapabilityManifest` scan results.
- [ ] **Step 2:** Map the current Rust-owned launch behaviors into the initial capability families from the design: `launch.text`, `model.selection`, `prompt.system`, `access.read_only`, `filesystem.additional_directories`, `conversation.stream`, `conversation.resume`, `conversation.preassigned_session_id`, `reasoning.think`, `reasoning.speed`, and `transport.local`.
- [ ] **Step 3:** Encode explicit `supported`, `unavailable`, `unverified`, and `unsupported` states with stable reason values rather than client-generated prose.
- [ ] **Step 4:** Add unit tests covering each supported runtime, missing executables, missing sandbox mapping, conversation capability differences, and the exclusion of observational scan targets from the public list.
- [ ] **Step 5:** Commit the resolver once the unit tests pass.

### Task 4: Add a distinct runtime descriptor read surface

**Files:**
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/reference/api.md`
- Modify: `docs/reference/api-contract.md`
- Create: `docs/reference/api-runtimes.md`
- Test: `crates/coven-cli/src/api.rs`

- [ ] **Step 1:** Add `GET /api/v1/runtimes` and `GET /api/v1/runtimes/:runtimeId` to `api.rs`, keeping `/api/v1/capabilities/harnesses` and `/api/v1/capabilities/:harnessId` unchanged.
- [ ] **Step 2:** Return payloads that include `descriptorVersion: "coven.runtime.descriptor.v1"` and the effective descriptor fields from Task 2.
- [ ] **Step 3:** Add a structured `404 runtime_not_found` path for unknown or unsupported public runtime ids.
- [ ] **Step 4:** Document the new routes in `docs/API-CONTRACT.md`, `docs/reference/api.md`, `docs/reference/api-contract.md`, and the dedicated `docs/reference/api-runtimes.md` page, explicitly contrasting them with the raw harness-capability scan routes.
- [ ] **Step 5:** Add focused API tests proving the new runtime routes work, that the old harness-capability routes are unchanged, and that unsupported observational scan targets do not appear in the public runtime listing.
- [ ] **Step 6:** Commit the read-surface work only after the docs and tests match.

### Task 5: Enforce required capabilities before launch

**Files:**
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `crates/coven-cli/src/runtime_descriptors.rs`
- Modify: `crates/coven-cli/src/session_launch.rs`
- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/reference/api.md`
- Test: `crates/coven-cli/src/api.rs`

- [ ] **Step 1:** Extend the session launch payload parsing to accept a Rust-owned required-capability field (for example `requiredCapabilities`).
- [ ] **Step 2:** Reject unknown capability ids up front with `400 invalid_request` and structured details.
- [ ] **Step 3:** Evaluate the chosen runtime's descriptor before launch and reject any required capability whose state is not `supported`.
- [ ] **Step 4:** Add the stable `409 runtime_capability_not_met` error code, document it, and include `runtimeId`, `capability`, `state`, and `reason` in `details`.
- [ ] **Step 5:** Add focused API tests for unknown capability ids, unsupported capability requests, unavailable capability requests, and an accepted launch where every required capability is `supported`.
- [ ] **Step 6:** Commit the enforcement change separately from broader refactors.

### Task 6: Lock the contract with regression coverage and docs guardrails

**Files:**
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `crates/coven-cli/tests/doctor_prose_contract.rs`
- Modify: `crates/coven-cli/tests/doctor_json_contract.rs`
- Verify: repository root

- [ ] **Step 1:** Add regression tests that prove the daemon still advertises `coven.daemon.v1`, that the runtime routes remain separate from `/api/v1/capabilities/harnesses`, and that session-lifecycle documentation matches authoritative Rust states.
- [ ] **Step 2:** Run `cargo fmt --check`.
- [ ] **Step 3:** Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] **Step 4:** Run `cargo test --workspace --locked`.
- [ ] **Step 5:** Run `python scripts/check-secrets.py`.
- [ ] **Step 6:** Stage the planned implementation diff and run `python3 scripts/check-coven-privacy.py --staged`.
- [ ] **Step 7:** Inspect `git diff --cached --check` and the scoped diff before the final commit.
- [ ] **Step 8:** Commit with a conventional subject that stays scoped to runtime capability recovery.

## Self-review

- **Spec coverage:** This plan covers the design's required version freeze, Rust authority types, effective descriptor resolver, separation from `/api/v1/capabilities/harnesses`, required-capability evaluation, and acceptance-level regression/documentation work.
- **Placeholder scan:** No task relies on `TODO`, `TBD`, or implied follow-up wording; each names exact files and verification commands.
- **Type consistency:** The plan uses one consistent vocabulary across tasks: `EffectiveRuntimeDescriptor`, `RuntimeSupportClass`, `CapabilityDescriptor`, `CapabilityState`, `CapabilityReason`, and `requiredCapabilities`.
