---
summary: "End-to-end certification matrix for Coven: lanes, rows, outcomes, and evidence."
description: "The certification rule, lane-by-lane row tables with evidence links, candidate and receipt generation, and the external operator lanes for real hardware and real credentials."
read_when:
  - Certifying a release candidate
  - Checking whether a support claim is proven
  - Reporting release readiness
title: "Coven end-to-end certification matrix"
---

This is the integration-certification authority for Coven
([OpenCoven/coven#779](https://github.com/OpenCoven/coven/issues/779), child of
[#670](https://github.com/OpenCoven/coven/issues/670)). It consumes focused
implementation evidence from [#777](https://github.com/OpenCoven/coven/issues/777)
and [#778](https://github.com/OpenCoven/coven/issues/778) and release-governance
evidence from [#805](https://github.com/OpenCoven/coven/issues/805) instead of
duplicating their work. Test totals, spec counts, and source-level unit coverage
alone do not establish release readiness: every support claim below maps to a
certification row with an explicit outcome and evidence, or it is recorded as an
open blocker.

The rows live once, in
[`scripts/certification-matrix.mjs`](../../scripts/certification-matrix.mjs).
This page renders them;
[`scripts/certification-receipt-test.mjs`](../../scripts/certification-receipt-test.mjs)
fails when the two drift apart.

## Certification rule

Every row has exactly one outcome:

| Outcome | Meaning |
| --- | --- |
| `required / passed` | Supported and proven on the exact candidate/artifact. |
| `required / failed` | A supported claim proven broken: a release/support blocker. |
| `required / unknown (open blocker)` | A support claim without sufficient evidence yet. Recorded explicitly, never hidden; it blocks release certification until resolved. |
| `not applicable` | Excluded by the support contract, with the justification stated. |
| `experimental / disabled` | Visible as experimental and incapable of becoming supported through packaging drift. |
| `deferred` | Named owner issue and absent from current support claims. |

`Skipped`, `unknown`, and "unit tests passed" are **not** terminal outcomes for
a required row: `scripts/certification-receipt.mjs --strict` fails closed on
every `required / unknown (open blocker)` and `required / failed` row.

## Candidate, receipts, and proof triggers

A **candidate** is a source commit plus its tree digest (and, once one exists, a
signed tag and published artifact digests). The receipt is generated with:

```sh
node scripts/certification-receipt.mjs            # print the receipt
node scripts/certification-receipt.mjs --strict   # fail closed on open blockers
```

The receipt is deterministic and keyed by the candidate digests, carries the
support-matrix version (`1.1.0`), one entry per row, the summary counts, the
open blockers, and a `reviewerDecision` that stays `null` until release
authorization ([#805](https://github.com/OpenCoven/coven/issues/805)) sets it
after human review. The receipt never self-certifies.

Rows are proven on different triggers, and the receipt records which:

- **per pull request** — the lanes CI runs on every PR merge commit (policy
  guard, Rust lint/tests on Linux and Windows, npm packaged-journey legs on
  Linux x64 and Windows x64, AFS-mount where routed);
- **per push to `main` and at release tags** — the legs that need dedicated
  runners (macOS suites, full four-platform npm onboarding);
- **at tag time** — the release gates in
  [`release-npm.yml`](../../.github/workflows/release-npm.yml) and
  [`release-github.yml`](../../.github/workflows/release-github.yml);
- **operator lanes (external)** — real hardware, real provider credentials, and
  the deployed docs site. These are documented below as external by design and
  are deliberately not CI lanes; see the issue's non-goal about not blocking
  every PR on real cloud/provider/device tests.

## Lane A — hermetic packaged first-session E2E

Owner: [#777](https://github.com/OpenCoven/coven/issues/777) (closed — the
journey exists and runs in CI). Fresh `COVEN_HOME`, isolated environment, fake
deterministic harness, no developer-checkout reliance.

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| A1 | The packaged artifact is built and packed from the exact candidate source, not an arbitrary developer checkout. | [`npm-onboarding-pr`](../../.github/workflows/ci.yml) · [`npm-onboarding-main`](../../.github/workflows/ci.yml) · [`test-cli-prepublish.mjs`](../../scripts/test-cli-prepublish.mjs) · [`publish-npm.mjs`](../../scripts/publish-npm.mjs) | required / passed |
| A2 | The produced tarball installs and runs in an isolated environment with a fresh COVEN_HOME. | [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) · [`test-cli-prepublish.mjs`](../../scripts/test-cli-prepublish.mjs) · [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) | required / passed |
| A3 | Progressive command discovery holds on the installed artifact: curated default help, complete help contract, internals hidden. | [`help_disclosure.rs`](../../crates/coven-cli/tests/help_disclosure.rs) · [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) · [`export-cli-help-contract.mjs`](../../scripts/export-cli-help-contract.mjs) | required / passed |
| A4 | Install/doctor/health, first project/session, deterministic fake harness, first output, inspect/events/log/status, input, kill/terminal disposition, and cleanup all work through the packaged CLI. | [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) · [`fake-codex.mjs`](../../scripts/fixtures/fake-codex.mjs) · [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) | required / passed |
| A5 | The journey does not rely on repository-relative files, undeclared developer tools, global state, or preexisting user configuration. | [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) · [`config_paths.rs`](../../crates/coven-cli/tests/config_paths.rs) | required / passed |
| A6 | Uninstall/cleanup removes test state without deleting unrelated user or workspace data. | [`uninstall.md`](../../docs/install/uninstall.md) · [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) | required / unknown (open blocker) |
| A7 | Failure output names the failed operation and the safe next action without leaking sensitive payloads. | [`doctor_prose_contract.rs`](../../crates/coven-cli/tests/doctor_prose_contract.rs) · [`doctor_json_contract.rs`](../../crates/coven-cli/tests/doctor_json_contract.rs) · [`privacy.rs`](../../crates/coven-cli/src/privacy.rs) · [`check-coven-privacy.py`](../../scripts/check-coven-privacy.py) | required / passed |

A6 is open: the journey asserts its own daemon cleanup and the uninstall
contract is documented, but no automated check exercises uninstall on a
populated `COVEN_HOME` or asserts unrelated sibling data survives. Owner:
[#807](https://github.com/OpenCoven/coven/issues/807).

## Lane B — supported platform/package matrix

The support contract
([README](../../README.md), npm platform packages) claims macOS arm64/x64,
glibc Linux x64, and Windows x64 via `@opencoven/cli` plus its four native
packages.

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| B1 | Linux x64: packaged tarball onboarding plus source-equivalent checks pass in CI. | [`npm-onboarding-pr`](../../.github/workflows/ci.yml) · [`npm-onboarding-main`](../../.github/workflows/ci.yml) · [`rust-test-linux`](../../.github/workflows/ci.yml) | required / passed |
| B2 | Windows x64: packaged tarball onboarding plus the Rust suite pass in CI. | [`npm-onboarding-pr`](../../.github/workflows/ci.yml) · [`npm-onboarding-main`](../../.github/workflows/ci.yml) · [`rust-test-windows`](../../.github/workflows/ci.yml) | required / passed |
| B3 | macOS Apple Silicon: Rust suite, packaged onboarding, and AFS-mount legs run per push and at release tags. | [`rust-test-macos`](../../.github/workflows/ci.yml) · [`npm-onboarding-main`](../../.github/workflows/ci.yml) · [`afs-mount-macos`](../../.github/workflows/ci.yml) · [`release-npm.yml`](../../.github/workflows/release-npm.yml) | required / passed |
| B4 | macOS Intel x64: a distinct public package path exists and its CI leg runs per push and at release tags. | [`npm-onboarding-main`](../../.github/workflows/ci.yml) · [`release-npm.yml`](../../.github/workflows/release-npm.yml) · [README](../../README.md) | required / passed |
| B5 | Real-hardware confirmation per platform: registry install, doctor, and a first session on end-user machines. | [`releasing.md`](../../docs/reference/releasing.md) · [#805](https://github.com/OpenCoven/coven/issues/805) | deferred |
| B6 | Any additional architecture/platform beyond the declared support matrix. | [README](../../README.md) | not applicable |

B1–B4 are proven by GitHub-hosted CI on the declared OS images. **B5 is the
external real-hardware lane**: the release runbook's fresh-consumer install
verification (`npm install -g @opencoven/cli@X.Y.Z`, `coven --version`,
`coven doctor` on a machine that never had Coven) is an operator step attached
to the release record, owned by #805. B6 is justified by the support contract:
Alpine and arm64 Linux are explicitly not claims.

## Lane C — real harness/provider certification

Supported harness set: Codex, Claude Code, GitHub Copilot CLI. The hermetic PR
lane stays credential-free; real credentials are the external operator lane.

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| C1 | A clean authentication/setup path is documented and exercised for every supported provider. | [`setup_cli.rs`](../../crates/coven-cli/tests/setup_cli.rs) · [`cli-setup.md`](../../docs/reference/cli-setup.md) · [`doctor_auth_boundary.rs`](../../crates/coven-cli/tests/doctor_auth_boundary.rs) | required / passed |
| C2 | Launch, first output, input/continuation, termination, and final status work through the public interface. | [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) · [`fake-codex.mjs`](../../scripts/fixtures/fake-codex.mjs) · [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) · [`harness_parity.rs`](../../crates/coven-cli/tests/harness_parity.rs) | required / passed |
| C3 | Missing/expired/refused credentials fail closed with actionable normalized errors. | [`doctor_auth_boundary.rs`](../../crates/coven-cli/tests/doctor_auth_boundary.rs) · [`setup_cli.rs`](../../crates/coven-cli/tests/setup_cli.rs) · [`doctor_prose_contract.rs`](../../crates/coven-cli/tests/doctor_prose_contract.rs) | required / passed |
| C4 | Credentials never appear in event, log, or evidence output. | [`privacy.rs`](../../crates/coven-cli/src/privacy.rs) · [`check-coven-privacy.py`](../../scripts/check-coven-privacy.py) · [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) · [`setup_cli.rs`](../../crates/coven-cli/tests/setup_cli.rs) | required / passed |
| C5 | Provider disappearance/timeout produces bounded state rather than indefinite ambiguity. | [`process_supervisor.rs`](../../crates/coven-cli/tests/process_supervisor.rs) · [`release-stress.mjs`](../../scripts/release-stress.mjs) · [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) | required / passed |
| C6 | Unsupported providers never become implicitly supported because an executable happens to exist on PATH. | [`harness.rs`](../../crates/coven-cli/src/harness.rs) · [`harness_parity.rs`](../../crates/coven-cli/tests/harness_parity.rs) | required / passed |
| C7 | Real-credential certification per supported provider (verify-only packet with real turns). | [`certify-release.sh`](../../scripts/certify-release.sh) · [`releasing.md`](../../docs/reference/releasing.md) · [#805](https://github.com/OpenCoven/coven/issues/805) | deferred |

C1–C6 are proven hermetically (the fake deterministic harness, the auth-boundary
and report-redaction contracts). **C7 is the external real-credential lane**:
`coven setup <provider> --verify-only` requires an interactive TTY, explicit
network/cost consent, and real provider usage, so it runs as the operator
certification packet ([`scripts/certify-release.sh`](../../scripts/certify-release.sh))
attached to the release record. Provider-specific model latency is not conflated
with Coven control-plane latency: C2/C5 assert bounded control-plane behavior
against deterministic fixtures.

## Lane D — lifecycle, restart, and recovery

Fault-oriented cases, not only normal shutdown. This lane has the most open
blockers, which is exactly the point of the matrix: recovery gaps are surfaced,
not averaged away. Owner for the open rows:
[#807](https://github.com/OpenCoven/coven/issues/807).

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| D1 | Daemon restart preserves durable sessions and state, including the managed identity contract. | [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) · [`windows_daemon_lifecycle.rs`](../../crates/coven-cli/tests/windows_daemon_lifecycle.rs) | required / passed |
| D2 | Harness/process crash before and after first output reaches bounded terminal-or-unknown state. | [`process_supervisor.rs`](../../crates/coven-cli/tests/process_supervisor.rs) | required / unknown (open blocker) |
| D3 | Client disconnect/reconnect and event-cursor continuation work. | [`health.rs`](../../crates/coven-client/tests/health.rs) · [`api.rs`](../../crates/coven-cli/src/api.rs) · [`lifecycle.rs`](../../crates/coven-client/src/lifecycle.rs) | required / passed |
| D4 | Endpoint/peer replacement honors the typed client safety contract and never auto-replays a consequential mutation merely because transport changed. | [`error.rs`](../../crates/coven-client/src/error.rs) · [`lifecycle.rs`](../../crates/coven-client/src/lifecycle.rs) | required / unknown (open blocker) |
| D5 | Kill/cancel during active work reaches an authoritative terminal or explicit unknown/recovery state. | [`process_supervisor.rs`](../../crates/coven-cli/tests/process_supervisor.rs) · [`user-journey-e2e.mjs`](../../scripts/user-journey-e2e.mjs) · [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) | required / passed |
| D6 | Duplicate/retried requests use the operation idempotency/adoption semantics or are refused where the outcome is ambiguous. | [`parallel_protocol.rs`](../../crates/coven-cli/tests/parallel_protocol.rs) | required / unknown (open blocker) |
| D7 | Interrupted cleanup/orphan reconciliation is visible and deterministic. | [`release-stress.mjs`](../../scripts/release-stress.mjs) · [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) | required / unknown (open blocker) |
| D8 | Corrupted state fails visibly and deterministically instead of silently succeeding. | [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) | required / passed |
| D9 | Unwritable state fails visibly and preserves recoverable evidence. | [#807](https://github.com/OpenCoven/coven/issues/807) · [`smoke.rs`](../../crates/coven-cli/tests/smoke.rs) | required / unknown (open blocker) |

Explicit uncertainty is acceptable; false success is not. D2/D4/D6/D7/D9 are
open because supervisor-level termination, typed-client errors, concurrent
request safety, and cleanup stress are proven while the specific
crash-window/replay/idempotency/orphan/unwritable contracts are not yet pinned
by tests.

## Lane E — events, backpressure, and evidence integrity

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| E1 | Event cursor continuation and paging semantics are exercised against the daemon contract. | [`health.rs`](../../crates/coven-client/tests/health.rs) · [`api.rs`](../../crates/coven-cli/src/api.rs) | required / passed |
| E2 | Event-writer pressure preserves lifecycle/tool/error/exit capacity according to contract. | [`event_writer.rs`](../../crates/coven-cli/src/event_writer.rs) · [`release-stress.mjs`](../../scripts/release-stress.mjs) | required / passed |
| E3 | Raw output loss/truncation is explicit, bounded, and observable rather than silent. | [`api.rs`](../../crates/coven-cli/src/api.rs) | required / passed |
| E4 | Oversized/malformed event/request bodies fail through structured errors. | [`api.rs`](../../crates/coven-cli/src/api.rs) · [`API-CONTRACT.md`](../../docs/API-CONTRACT.md) | required / passed |
| E5 | Redaction remains applied to default persisted/returned evidence. | [`privacy.rs`](../../crates/coven-cli/src/privacy.rs) · [`setup_cli.rs`](../../crates/coven-cli/tests/setup_cli.rs) · [`check-coven-privacy.py`](../../scripts/check-coven-privacy.py) | required / passed |
| E6 | Raw sensitive artifact opt-in remains separately protected/encrypted/retained according to current security policy. | [`encrypted_artifacts.rs`](../../crates/coven-cli/src/encrypted_artifacts.rs) · [#808](https://github.com/OpenCoven/coven/issues/808) | deferred |
| E7 | Evidence receipts contain digests/references and sanitized outcomes rather than prompts, credentials, or unrestricted terminal output. | [`certify-release.sh`](../../scripts/certify-release.sh) · [`releasing.md`](../../docs/reference/releasing.md) · [`check-coven-privacy.py`](../../scripts/check-coven-privacy.py) | required / passed |

E6 waits on [#808](https://github.com/OpenCoven/coven/issues/808): encrypted
artifact storage exists in code, but the retention/encryption contract it must
certify against is being consolidated there.

## Lane F — AgentFS security and installed-artifact gate

A storage benchmark or a generic workspace green run never promotes a
mount/security surface automatically.

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| F1 | AFS mount stays experimental/disabled until its dedicated gate passes; it cannot become supported through packaging drift. | [AgentFS design](../../specs/coven-agent-fs/DESIGN.md) · [mount spike](../../specs/coven-agent-fs/MOUNT-SPIKE.md) · [`afs-mount-linux`](../../.github/workflows/ci.yml) · [`afs-mount-macos`](../../.github/workflows/ci.yml) | experimental / disabled |
| F2 | Installed-artifact mount gate exercised per platform before any mount support claim. | [`afs-mount-e2e.sh`](../../scripts/afs-mount-e2e.sh) · [`afs-mount-smoke.sh`](../../scripts/afs-mount-smoke.sh) | deferred |
| F3 | Pre-enablement gate (helper availability, credential observation, case-insensitive .git, handle reuse/gate-root enforcement, access-control boundaries, crash/restart/unmount recovery, concurrency, platform behavior, safe-disabled behavior) stays closed while the surface is experimental. | [AgentFS design](../../specs/coven-agent-fs/DESIGN.md) · [`afs-mount-e2e.sh`](../../scripts/afs-mount-e2e.sh) | experimental / disabled |

F1/F3: mount is a cargo feature outside default builds and the spec scopes
productionizing the mount spike out, so no mount capability is a support claim.
**F2 is the external artifact-level gate**:
[`scripts/afs-mount-e2e.sh --installed`](../../scripts/afs-mount-e2e.sh) is a
manual post-release verification — the v0.3.0 incident (published package
omitted the mount helper) is why it must stay artifact-level.

## Lane G — coven-agents/A2A boundary

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| G1 | Direct/handoff target ingress-policy parity is proven before stronger cross-agent safety claims. | [#803](https://github.com/OpenCoven/coven/issues/803) | deferred |
| G2 | Local coven-agents behavior is certified as local/in-process semantics under the invocation/delegation contracts. | [`runner.rs`](../../crates/coven-agents/tests/runner.rs) · [`loop_runner.rs`](../../crates/coven-agents/tests/loop_runner.rs) · [#804](https://github.com/OpenCoven/coven/issues/804) | deferred |
| G3 | No certification row describes the legacy handoff pointer-swap as durable distributed A2A request/response. | [handoff spec](../../specs/coven-handoff-packet/TECH.md) | not applicable |
| G4 | Local and Coven-backed executors share one conformance suite (authorization, stable invocation ID, events, timeout/cancel, ambiguous adoption, duplicate submission, interruption, cleanup, secret-free evidence). | [#804](https://github.com/OpenCoven/coven/issues/804) | deferred |
| G5 | Remote/multi-host placement reuses the existing hub and stays below agent-visible APIs. | [multi-host spec](../../specs/coven-multi-host-daemon/TECH.md) | not applicable |

G1 is blocked behind [#803](https://github.com/OpenCoven/coven/issues/803);
G2/G4 follow the [#804](https://github.com/OpenCoven/coven/issues/804)
architecture migration so the suite does not certify a moving contract. G3/G5
record the boundaries that keep A2A claims from outrunning the architecture.

## Lane H — public client/API compatibility

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| H1 | Named version/capability negotiation works on the exact candidate. | [`API-CONTRACT.md`](../../docs/API-CONTRACT.md) · [`http.rs`](../../crates/coven-client/src/http.rs) · [`api.rs`](../../crates/coven-cli/src/api.rs) | required / passed |
| H2 | Published client/package fixtures match the actual daemon contract. | [`health.rs`](../../crates/coven-client/tests/health.rs) · [`test-cli-prepublish.mjs`](../../scripts/test-cli-prepublish.mjs) · [`publish-npm-test.mjs`](../../scripts/publish-npm-test.mjs) | required / passed |
| H3 | Unsupported version/capability denial is structured and deterministic. | [`API-CONTRACT.md`](../../docs/API-CONTRACT.md) · [`error.rs`](../../crates/coven-client/src/error.rs) | required / passed |
| H4 | Authentication/peer binding remains separate from capability advertisement. | [`AUTH.md`](../../docs/AUTH.md) · [`remote-listener-auth.md`](../../docs/design/remote-listener-auth.md) | required / unknown (open blocker) |
| H5 | Malformed/oversized/unauthorized mutation paths fail before effects. | [`api.rs`](../../crates/coven-cli/src/api.rs) · [`API-CONTRACT.md`](../../docs/API-CONTRACT.md) | required / passed |
| H6 | Package consumers never rely on unpublished repository internals. | [npm/coven/package.json](../../npm/coven/package.json) · [`test-cli-prepublish.mjs`](../../scripts/test-cli-prepublish.mjs) · [`publish-npm-test.mjs`](../../scripts/publish-npm-test.mjs) | required / passed |

H4 is open: the separation is documented policy, but no hermetic test pins it on
the running daemon. Owner: [#807](https://github.com/OpenCoven/coven/issues/807).

## Lane I — docs and first-response correctness

Owners: [#775](https://github.com/OpenCoven/coven/issues/775),
[#776](https://github.com/OpenCoven/coven/issues/776),
[#778](https://github.com/OpenCoven/coven/issues/778).

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| I1 | Canonical docs build and link checks pass in repository and deployed-site contexts. | [#778](https://github.com/OpenCoven/coven/issues/778) · [`DOCS-MAINTENANCE.md`](../../docs/DOCS-MAINTENANCE.md) | deferred |
| I2 | Browser journey from install/discovery through first session/recovery uses the shipped commands/contracts. | [#778](https://github.com/OpenCoven/coven/issues/778) | deferred |
| I3 | Duplicate local public docs are removed or explicitly source-adjacent. | [`DOCS-MAINTENANCE.md`](../../docs/DOCS-MAINTENANCE.md) | required / unknown (open blocker) |
| I4 | Help contract remains complete while default help is progressively disclosed. | [`help_disclosure.rs`](../../crates/coven-cli/tests/help_disclosure.rs) · [`export-cli-help-contract.mjs`](../../scripts/export-cli-help-contract.mjs) · [`cli-docs-test.mjs`](../../scripts/cli-docs-test.mjs) | required / passed |
| I5 | Security/support text matches the security-policy truth and current capability state. | [#808](https://github.com/OpenCoven/coven/issues/808) · [`SAFETY-MODEL.md`](../../docs/SAFETY-MODEL.md) | deferred |
| I6 | No stale product/version/support claim survives after a certification state changes. | [`DOCS-MAINTENANCE.md`](../../docs/DOCS-MAINTENANCE.md) | required / unknown (open blocker) |

I1/I2 belong to the deployed-site program in
[#778](https://github.com/OpenCoven/coven/issues/778) — external to this
repository's CI by design. I3/I6 are open because the docs rules are normative
but not yet machine-checked.

## Lane J — release authorization and exact artifact evidence

Owned by [#805](https://github.com/OpenCoven/coven/issues/805), consumed here.
No release tag is in flight for the current candidate, so tag-time rows are
recorded as not applicable for this candidate with their standing gates cited.

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| J1 | Exact release/tag target has every required check success. | [`release-npm.yml`](../../.github/workflows/release-npm.yml) · [`release-github.yml`](../../.github/workflows/release-github.yml) | not applicable |
| J2 | Branch/ruleset/review protection evidence includes administrators. | [#805](https://github.com/OpenCoven/coven/issues/805) | deferred |
| J3 | Signed tag/trusted signer/provenance/SBOM or declared dependency receipt passes. | [`release-npm.yml`](../../.github/workflows/release-npm.yml) · [`release-github.yml`](../../.github/workflows/release-github.yml) · [`releasing.md`](../../docs/reference/releasing.md) | not applicable |
| J4 | Generated/version state is coherent and clean. | [`publish-npm.mjs`](../../scripts/publish-npm.mjs) · [`publish-npm-test.mjs`](../../scripts/publish-npm-test.mjs) · [`package-github-release-test.mjs`](../../scripts/package-github-release-test.mjs) | required / passed |
| J5 | Release channels agree on public version/support state. | [#805](https://github.com/OpenCoven/coven/issues/805) · [`releasing.md`](../../docs/reference/releasing.md) | deferred |
| J6 | Artifact/package digests used by E2E match the published artifacts. | [`releasing.md`](../../docs/reference/releasing.md) · [`release-github.yml`](../../.github/workflows/release-github.yml) | not applicable |
| J7 | Mutation tests prove failed/missing checks, tag mismatch, signer failure, or security-disabled surfaces fail closed. | [`release-github.yml`](../../.github/workflows/release-github.yml) · [`release-stress.mjs`](../../scripts/release-stress.mjs) | deferred |

## Lane K — device/mobile trust expansion

Issues [#785](https://github.com/OpenCoven/coven/issues/785)–[#788](https://github.com/OpenCoven/coven/issues/788)
define the device-bound/QR/reconnection/recovery program. Those capabilities
remain outside the current shipped certification until they are support claims.

| ID | Certification row | Evidence | Outcome |
| --- | --- | --- | --- |
| K1 | Device-bound/QR/reconnection/recovery capabilities stay outside shipped certification until they become support claims. | [#785](https://github.com/OpenCoven/coven/issues/785) · [pairing spec](../../spec/device-pairing/v1/README.md) · [`mobile-pairing-protocol-v2.md`](../../docs/design/mobile-pairing-protocol-v2.md) | not applicable |

When the program is promoted, certification must require: cryptographic
peer/device identity and scoped grants; QR/bootstrap replay/expiry/refusal
coverage; biometric/passkey authorization semantics where claimed; local
discovery and relay fallback without widening authority; device
revocation/recovery/rotation; and cross-device continuity evidence that
preserves familiar/session authority rather than introducing a client-side
source of truth.

## Current state and how to update this matrix

Generate the live counts with
`node scripts/certification-receipt.mjs`; the summary partitions every row into
`requiredPassed` / `requiredFailed` / `requiredUnknown` / `notApplicable` /
`experimentalDisabled` / `deferred`, and `releaseBlockers` lists exactly the
rows that keep certification open. As of support-matrix version `1.1.0` the
open blockers are the recovery/API-boundary/docs gaps named in lanes A, D, H,
and I — all owned by [#807](https://github.com/OpenCoven/coven/issues/807) and
[#778](https://github.com/OpenCoven/coven/issues/778) rather than silently
skipped.

Rules for changing the matrix:

1. Edit [`scripts/certification-matrix.mjs`](../../scripts/certification-matrix.mjs)
   and the tables on this page in the same commit — the test suite fails on
   drift.
2. A required row never moves to `required / unknown` silently: the
   justification and owner issue must land with the change.
3. Evidence references must resolve: repo paths must exist, CI jobs must be
   declared in [`ci.yml`](../../.github/workflows/ci.yml), and deferred rows
   must name their owner issue.
4. Bump `SUPPORT_MATRIX_VERSION` when the row set or the support contract it
   reflects changes, and state the change in the PR description.
