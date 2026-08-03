# Universal Runtime Capability Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Rust-owned effective runtime capability surface for supported harnesses without changing the meaning of existing harness-scan routes or widening the supported harness set.

**Architecture:** Freeze the public `coven.daemon.v1` and session-lifecycle vocabulary first, then layer a new effective runtime-descriptor resolver on top of existing Rust launch specs and raw harness-scan manifests. Keep `/api/v1/capabilities/harnesses` as factual native scan output, add a distinct runtime descriptor read surface, keep `harness` required on session create, make `runtimeId` an optional consistency check, and enforce required capabilities in Rust before launch.

**Tech Stack:** Rust (`crates/coven-cli/src/api.rs`, `harness.rs`, `capabilities.rs`, `daemon.rs`, `store.rs`), the pinned shared schema types from `coven_runtime_spec` (`Capabilities`, `SandboxMapping`, `StreamArgs`), markdown API docs, existing Rust tests, secret/privacy guardrails.

## File map

- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/reference/api-contract.md`
- Modify: `docs/reference/api.md`
- Modify: `docs/sessions/lifecycle.md`
- Modify: `crates/coven-cli/src/api.rs`
- Test: `docs/ARCHITECTURE.md`
- Test: `docs/sessions/lifecycle.md`
- Test: `crates/coven-cli/src/api.rs`

The public docs in scope are the live pages that still carry stale version or lifecycle guidance. The freeze must update the version contract docs and the two user-facing anchors called out in the issue: `docs/ARCHITECTURE.md` and `docs/sessions/lifecycle.md`.

---

### Task 1: Freeze the O1 public contract vocabulary in the live docs

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/reference/api-contract.md`
- Modify: `docs/reference/api.md`
- Modify: `docs/sessions/lifecycle.md`
- Modify: `crates/coven-cli/src/api.rs`
- Test: `docs/ARCHITECTURE.md`
- Test: `docs/sessions/lifecycle.md`
- Test: `crates/coven-cli/src/api.rs`

- [ ] **Step 1:** Update the live public docs so they use one canonical external API version vocabulary: `coven.daemon.v1` for the contract name and `/api/v1` for the route prefix. Remove stale `supportedApiVersions` guidance from `docs/ARCHITECTURE.md`, and remove `pending`/`archived` from the authoritative lifecycle-status set in `docs/sessions/lifecycle.md` without deleting valid archive metadata or ritual documentation.
- [ ] **Step 2:** Enumerate the exact persisted session statuses used by Rust (`created`, `running`, `completed`, `failed`, `killed`, `idle`, `orphaned`), distinguish archive metadata from lifecycle status, and keep the lifecycle page aligned with the authoritative Rust store vocabulary.
- [ ] **Step 3:** Change `GET /api/v1/api-version` in `crates/coven-cli/src/api.rs` so the payload reports the named contract string instead of the bare `v1` token.
- [ ] **Step 4:** Add or update route tests in `crates/coven-cli/src/api.rs` covering the named version response and the current unknown-version failure path, plus exact docs assertions that keep `supportedApiVersions` documented for `GET /api/v1/api-version`, reject any claim that `GET /api/v1/health` returns it, and keep the stale lifecycle guidance out of the live pages.
- [ ] **Step 5:** Run focused tests for the version routes and these exact doc checks:

  ```sh
  rg -n 'supportedApiVersions' docs/reference/api-contract.md
  rg -n 'created|running|completed|failed|killed|idle|orphaned' docs/sessions/lifecycle.md
  rg -n 'archived_at|Archive / summon / sacrifice' docs/sessions/lifecycle.md
  ```

  then commit only the O1 contract-freeze changes.

### Task 2: Introduce Rust-owned effective runtime descriptor types

**Files:**
- Create: `crates/coven-cli/src/runtime_descriptors.rs`
- Modify: `crates/coven-cli/src/main.rs`
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `crates/coven-cli/src/harness.rs`
- Test: `crates/coven-cli/src/runtime_descriptors.rs`

- [ ] **Step 1:** Add `runtime_descriptors.rs` with the new derived authority types: `EffectiveRuntimeDescriptor`, `RuntimeSupportClass`, `RuntimeAdmission`, `AvailabilityDescriptor`, `CapabilityDescriptor`, `CapabilityState`, the closed v1 `CapabilityReason` enum, `NativeIntegrationSummary`, `RuntimeWarning`, `RuntimeWarningCode`, and `RuntimeWarningScope`.
- [ ] **Step 2:** Reuse the pinned shared-spec inputs already carried by `HarnessCommandSpec` (`coven_runtime_spec::Capabilities`, `SandboxMapping`, and `StreamArgs`) instead of redefining those schema structs locally; keep Coven's Rust adapter logic authoritative for evaluation and spawn decisions.
- [ ] **Step 3:** Expose the new module from `main.rs` or the existing module tree without moving launch authority out of `harness.rs`.
- [ ] **Step 4:** Add unit tests that construct descriptors for `codex`, `claude`, `copilot`, and harness-only `coven-code`, asserting that only the first three classify as `supported_runtime`.
- [ ] **Step 5:** Add a regression test proving unknown harness ids never synthesize a descriptor, that `CapabilityReason` serializes with the exact documented snake_case values, and that `RuntimeWarning` values serialize with the documented enum set and stable ordering.
- [ ] **Step 6:** Commit the type layer separately before wiring any API route.

### Task 3: Build the supported-runtime resolver from existing Rust facts

**Files:**
- Modify: `crates/coven-cli/src/runtime_descriptors.rs`
- Modify: `crates/coven-cli/src/harness.rs`
- Modify: `crates/coven-cli/src/capabilities.rs`
- Test: `crates/coven-cli/src/runtime_descriptors.rs`

- [ ] **Step 1:** Add a resolver that combines the bundled `HarnessCommandSpec` data with local availability and the existing `HarnessCapabilityManifest` scan results.
- [ ] **Step 2:** Map the current Rust-owned launch behaviors into the initial capability families from the design: `launch.text`, `model.selection`, `prompt.system`, `access.read_only`, `filesystem.additional_directories`, `conversation.stream`, `conversation.resume`, `conversation.preassigned_session_id`, `reasoning.think`, `reasoning.speed`, and `transport.local`.
- [ ] **Step 3:** Encode explicit `supported`, `unavailable`, `unverified`, and `unsupported` states with the exact v1 mapping rules: `supported -> reason null`; `unavailable -> harness_not_installed | version_too_old`; `unverified -> version_unknown`; `unsupported -> capability_not_advertised | policy_denied | platform_unsupported`.
- [ ] **Step 4:** Map raw `CapabilityWarning { kind, path, message }` values into the closed `RuntimeWarning` enum set (`parse_error` → `native_scan_parse_error`, `permission_denied` → `native_scan_permission_denied`) and sort warnings by the documented stable order.
- [ ] **Step 5:** Add unit tests covering each supported runtime, missing executables, missing sandbox mapping, conversation capability differences, deterministic warning ordering, and the exclusion of observational scan targets from the public list.
- [ ] **Step 6:** Commit the resolver once the unit tests pass.

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
- [ ] **Step 3:** Add a structured `404 descriptor_unavailable` path for unknown or unsupported public runtime ids, without implying that harness-only tools such as `coven-code` are public runtimes.
- [ ] **Step 4:** Document the new routes in `docs/API-CONTRACT.md`, `docs/reference/api.md`, `docs/reference/api-contract.md`, and the dedicated `docs/reference/api-runtimes.md` page, explicitly contrasting them with the raw harness-capability scan routes and using exact example error payloads.
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

- [ ] **Step 1:** Extend the session launch payload parsing so `harness` stays required, `runtimeId` is optional, and `requiredCapabilities` remains Rust-owned, while preserving today's harness-only launch path when `runtimeId` is absent.
- [ ] **Step 2:** If `runtimeId` is present, resolve it only against the public supported-runtime set; reject `coven-code`, observational-scan ids, and unknown ids with stable `404 descriptor_unavailable` before argv construction or spawn, and do not fall back to harness launch.
- [ ] **Step 3:** After a public `runtimeId` resolves, compare it to the canonical public runtime mapped from `harness`; when they differ, or when the harness has no canonical public runtime mapping, fail before spawn with stable `400 runtime_harness_mismatch`.
- [ ] **Step 4:** Reject unknown capability ids up front with `400 invalid_request` and structured details.
- [ ] **Step 5:** Evaluate the chosen runtime's descriptor before launch and reject any required capability whose state is not `supported`.
- [ ] **Step 6:** Add the stable `409 runtime_capability_not_met` error code, document it, and include `runtimeId`, `capability`, `state`, and `reason` in `details`.
- [ ] **Step 7:** Add focused API tests for: harness-only launches without `runtimeId`; matching `harness`/`runtimeId` launches; explicit non-public `runtimeId` requests returning `descriptor_unavailable`; explicit supported mismatches returning `runtime_harness_mismatch`; unsupported capability requests; unavailable capability requests; and accepted launches where every required capability is `supported` and therefore `reason` is `null`.
- [ ] **Step 8:** Commit the enforcement change separately from broader refactors.

### Task 6: Lock the contract with regression coverage and docs guardrails

**Files:**
- Modify: `crates/coven-cli/src/api.rs`
- Create: `crates/coven-cli/tests/runtime_descriptor_contract.rs`
- Verify: repository root

- [ ] **Step 1:** After Tasks 4 and 5 create the new `crates/coven-cli/tests/runtime_descriptor_contract.rs` integration test, reusing the existing binary-invocation style from current contract tests instead of assuming a pre-existing doctor-only harness.
- [ ] **Step 2:** Add regression assertions in that file that prove the daemon still advertises `coven.daemon.v1`, that the runtime routes remain separate from `/api/v1/capabilities/harnesses`, that explicit non-public `runtimeId` requests fail with `descriptor_unavailable` before spawn, that supported mismatches fail with `runtime_harness_mismatch`, and that harness-only `coven-code` launches remain governed by the current harness policy.
- [ ] **Step 3:** Add or update focused `api.rs` unit tests so the documented persisted lifecycle vocabulary in `docs/sessions/lifecycle.md` matches authoritative Rust states.
- [ ] **Step 4:** Run `cargo fmt --check`.
- [ ] **Step 5:** Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] **Step 6:** Run `cargo test --workspace --locked`.
- [ ] **Step 7:** Run `python scripts/check-secrets.py`.
- [ ] **Step 8:** Stage the planned implementation diff and run `python3 scripts/check-coven-privacy.py --staged`.
- [ ] **Step 9:** Inspect `git diff --cached --check` and the scoped diff before the final commit.
- [ ] **Step 10:** Commit with a conventional subject that stays scoped to runtime capability recovery.

## Self-review

- **Spec coverage:** This plan covers the design's required version freeze, Rust authority types, effective descriptor resolver, separation from `/api/v1/capabilities/harnesses`, exact `harness`/`runtimeId` session semantics, required-capability evaluation, and acceptance-level regression/documentation work.
- **Placeholder scan:** No task relies on `TODO`, `TBD`, or implied follow-up wording; each names exact files and verification commands.
- **Type consistency:** The plan uses one consistent vocabulary across tasks: `EffectiveRuntimeDescriptor`, `RuntimeSupportClass`, `CapabilityDescriptor`, `CapabilityState`, `CapabilityReason`, `RuntimeWarning`, `requiredCapabilities`, `descriptor_unavailable`, and `runtime_harness_mismatch`.
