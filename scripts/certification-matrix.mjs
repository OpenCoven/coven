// Single source of truth for the Coven end-to-end certification matrix.
//
// Issue OpenCoven/coven#779 is the integration-certification authority: it
// consumes focused implementation evidence from #777/#778 and release
// governance evidence from #805 rather than duplicating them. This module is
// that matrix in machine-readable form. docs/reference/certification.md
// renders it; scripts/certification-receipt.mjs turns it into the structured
// certification receipt; scripts/certification-receipt-test.mjs keeps the
// doc, the matrix, and the CI wiring from drifting apart.
//
// Every row carries exactly one outcome:
//
//   required-passed        supported and proven on the exact candidate
//   required-failed        supported claim proven broken (release blocker)
//   required-unknown       support claim without sufficient evidence (open
//                          blocker — explicit, never hidden)
//   not-applicable         excluded by the support contract
//   experimental-disabled  visible as experimental and incapable of becoming
//                          supported through packaging drift
//   deferred               named owner issue, absent from current claims
//
// `Skipped` is not in the vocabulary on purpose: it is not a terminal outcome
// for a required certification row.

export const SUPPORT_MATRIX_VERSION = '1.1.0';
export const RECEIPT_VERSION = 1;

// Canonical support inventory: the machine-readable support contract that row
// applicability is derived from. A row may never claim coverage for a platform
// that is not declared here, and every platform declared here must be covered
// by at least one certification row (both enforced by the test suite).
//
// `releaseBuildRunner` is the runner that compiles the tag-built package for
// the platform in `.github/workflows/release-npm.yml`. `releaseOnboardingRunner`
// is the runner that *executes* the packaged onboarding verification of that
// tag-built package at tag time; `null` means no such leg exists in the release
// workflow, so release-time coverage for the platform cannot be claimed — rows
// covering it must stay `required-unknown` at the release channel instead of
// asserting coverage that does not run.
export const SUPPORT_INVENTORY = {
  version: 1,
  wrapperPackage: '@opencoven/cli',
  channels: ['source-checkout', 'release-tag'],
  platforms: [
    {
      id: 'linux-x64-gnu',
      package: '@opencoven/cli-linux-x64',
      ciRunner: 'ubuntu-latest',
      releaseBuildRunner: 'ubuntu-latest',
      releaseOnboardingRunner: 'ubuntu-latest'
    },
    {
      id: 'windows-x64',
      package: '@opencoven/cli-windows',
      ciRunner: 'windows-latest',
      releaseBuildRunner: 'windows-latest',
      // The release workflow's npm-dry-run leg executes on ubuntu-latest only;
      // no Windows runner exercises the tag-built Windows package.
      releaseOnboardingRunner: null
    },
    {
      id: 'macos-arm64',
      package: '@opencoven/cli-macos',
      ciRunner: 'macos-26',
      releaseBuildRunner: 'macos-26',
      // The release workflow builds macOS packages on macOS runners but the
      // onboarding/dry-run verification leg runs on ubuntu-latest only.
      releaseOnboardingRunner: null
    },
    {
      id: 'macos-x64',
      package: '@opencoven/cli-macos-x64',
      ciRunner: 'macos-15-intel',
      releaseBuildRunner: 'macos-15-intel',
      releaseOnboardingRunner: null
    }
  ]
};

export const SOURCE_CHECKOUT_CHANNEL = 'source-checkout';
export const RELEASE_TAG_CHANNEL = 'release-tag';

export const OUTCOMES = {
  REQUIRED_PASSED: 'required-passed',
  REQUIRED_FAILED: 'required-failed',
  REQUIRED_UNKNOWN: 'required-unknown',
  NOT_APPLICABLE: 'not-applicable',
  EXPERIMENTAL_DISABLED: 'experimental-disabled',
  DEFERRED: 'deferred'
};

export const ALL_OUTCOMES = new Set(Object.values(OUTCOMES));

// Outcomes that close a certification row. `required-unknown` is deliberately
// absent: it is an explicit, open blocker — never a terminal state.
export const TERMINAL_OUTCOMES = new Set([
  OUTCOMES.REQUIRED_PASSED,
  OUTCOMES.REQUIRED_FAILED,
  OUTCOMES.NOT_APPLICABLE,
  OUTCOMES.EXPERIMENTAL_DISABLED,
  OUTCOMES.DEFERRED
]);

// Evidence kinds and what they point at. Every kind is resolved by the test
// suite: repo paths must exist, CI jobs must be declared in ci.yml, and issue
// refs must name an upstream issue.
export const EVIDENCE_KINDS = {
  'ci-job': { description: 'GitHub Actions job declared in ci.yml' },
  workflow: { repoPath: true },
  test: { repoPath: true },
  script: { repoPath: true },
  docs: { repoPath: true },
  spec: { repoPath: true },
  package: { repoPath: true },
  runbook: { repoPath: true },
  issue: { repoPath: false }
};

export const LANES = [
  {
    id: 'A',
    title: 'Hermetic packaged first-session E2E',
    ownerIssue: 777,
    description:
      'Packaged-artifact first-session journey with a fresh COVEN_HOME and no developer-checkout reliance.'
  },
  {
    id: 'B',
    title: 'Supported platform/package matrix',
    ownerIssue: null,
    description:
      'Required packaged/source-equivalent checks on the declared support matrix; real-hardware confirmation is an external operator lane.'
  },
  {
    id: 'C',
    title: 'Real harness/provider certification',
    ownerIssue: 805,
    description:
      'Codex, Claude Code, and GitHub Copilot CLI support; real credentials stay in the operator lane, hermetic subsets in CI.'
  },
  {
    id: 'D',
    title: 'Lifecycle, restart, and recovery',
    ownerIssue: 807,
    description: 'Fault-oriented cases, not only normal shutdown.'
  },
  {
    id: 'E',
    title: 'Events, backpressure, and evidence integrity',
    ownerIssue: null,
    description: 'Event cursors, bounded writers, truncation, redaction, and evidence receipts.'
  },
  {
    id: 'F',
    title: 'AgentFS security and installed-artifact gate',
    ownerIssue: null,
    description: 'Mount surfaces stay experimental/disabled until their dedicated gate passes.'
  },
  {
    id: 'G',
    title: 'coven-agents/A2A boundary',
    ownerIssue: 804,
    description: 'Ingress parity and executor conformance gated by #803/#804.'
  },
  {
    id: 'H',
    title: 'Public client/API compatibility',
    ownerIssue: null,
    description: 'Version negotiation, contract fixtures, structured denial, fail-before-effects mutations.'
  },
  {
    id: 'I',
    title: 'Docs and first-response correctness',
    ownerIssue: 778,
    description: 'Canonical docs, browser journey, help contract, security/support text truth.'
  },
  {
    id: 'J',
    title: 'Release authorization and exact artifact evidence',
    ownerIssue: 805,
    description: 'Consumed from #805: tag-time gates, protection evidence, digests, fail-closed mutations.'
  },
  {
    id: 'K',
    title: 'Device/mobile trust expansion',
    ownerIssue: null,
    description: 'Issues #785–#788 program; outside shipped certification until support claims exist.'
  }
];

export const CERTIFICATION_MATRIX = [
  // ---------------------------------------------------------------- Lane A
  {
    id: 'A1',
    lane: 'A',
    claim:
      'The packaged artifact is built and packed from the exact candidate source, not an arbitrary developer checkout.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-pr' },
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-main' },
      { kind: 'script', ref: 'scripts/test-cli-prepublish.mjs' },
      { kind: 'script', ref: 'scripts/publish-npm.mjs' }
    ]
  },
  {
    id: 'A2',
    lane: 'A',
    claim: 'The produced tarball installs and runs in an isolated environment with a fresh COVEN_HOME.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' },
      { kind: 'script', ref: 'scripts/test-cli-prepublish.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' }
    ]
  },
  {
    id: 'A3',
    lane: 'A',
    claim:
      'Progressive command discovery holds on the installed artifact: curated default help, complete help contract, internals hidden.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/help_disclosure.rs' },
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' },
      { kind: 'script', ref: 'scripts/export-cli-help-contract.mjs' }
    ]
  },
  {
    id: 'A4',
    lane: 'A',
    claim:
      'Install/doctor/health, first project/session, deterministic fake harness, first output, inspect/events/log/status, input, kill/terminal disposition, and cleanup all work through the packaged CLI.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' },
      { kind: 'script', ref: 'scripts/fixtures/fake-codex.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' }
    ]
  },
  {
    id: 'A5',
    lane: 'A',
    claim:
      'The journey does not rely on repository-relative files, undeclared developer tools, global state, or preexisting user configuration.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/config_paths.rs' }
    ]
  },
  {
    id: 'A6',
    lane: 'A',
    claim: 'Uninstall/cleanup removes test state without deleting unrelated user or workspace data.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [
      { kind: 'docs', ref: 'docs/install/uninstall.md' },
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' }
    ],
    ownerIssue: 807,
    justification:
      'The hermetic journey asserts its own daemon cleanup and the uninstall contract is documented, but no automated check exercises uninstall on a populated COVEN_HOME or asserts unrelated sibling data survives.'
  },
  {
    id: 'A7',
    lane: 'A',
    claim: 'Failure output names the failed operation and the safe next action without leaking sensitive payloads.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/doctor_prose_contract.rs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/doctor_json_contract.rs' },
      { kind: 'test', ref: 'crates/coven-cli/src/privacy.rs' },
      { kind: 'script', ref: 'scripts/check-coven-privacy.py' }
    ]
  },

  // ---------------------------------------------------------------- Lane B
  {
    id: 'B1',
    lane: 'B',
    claim: 'Linux x64: packaged tarball onboarding plus source-equivalent checks pass in CI.',
    platforms: ['linux-x64-gnu'],
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-pr' },
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-main' },
      { kind: 'ci-job', ref: 'ci.yml#rust-test-linux' }
    ]
  },
  {
    id: 'B2',
    lane: 'B',
    claim: 'Windows x64: packaged tarball onboarding plus the Rust suite pass in CI.',
    platforms: ['windows-x64'],
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-pr' },
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-main' },
      { kind: 'ci-job', ref: 'ci.yml#rust-test-windows' }
    ]
  },
  {
    id: 'B3',
    lane: 'B',
    claim: 'macOS Apple Silicon: Rust suite, packaged onboarding, and AFS-mount legs run per push and at release tags.',
    platforms: ['macos-arm64'],
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'ci-job', ref: 'ci.yml#rust-test-macos' },
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-main' },
      { kind: 'ci-job', ref: 'ci.yml#afs-mount-macos' },
      { kind: 'workflow', ref: '.github/workflows/release-npm.yml' }
    ]
  },
  {
    id: 'B4',
    lane: 'B',
    claim: 'macOS Intel x64: a distinct public package path exists and its CI leg runs per push and at release tags.',
    platforms: ['macos-x64'],
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'ci-job', ref: 'ci.yml#npm-onboarding-main' },
      { kind: 'workflow', ref: '.github/workflows/release-npm.yml' },
      { kind: 'docs', ref: 'README.md' }
    ]
  },
  {
    id: 'B5',
    lane: 'B',
    claim: 'Real-hardware confirmation per platform: registry install, doctor, and a first session on end-user machines.',
    platforms: ['linux-x64-gnu', 'windows-x64', 'macos-arm64', 'macos-x64'],
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'runbook', ref: 'docs/reference/releasing.md' },
      { kind: 'issue', ref: 'OpenCoven/coven#805' }
    ],
    ownerIssue: 805,
    justification:
      'GitHub-hosted runners prove the declared OS images; end-user hardware variation (OEM images, real HOME profiles, sandboxing) is an operator release step in the runbook and stays external to the per-PR CI lane by design.'
  },
  {
    id: 'B6',
    lane: 'B',
    claim: 'Any additional architecture/platform beyond the declared support matrix.',
    outcome: OUTCOMES.NOT_APPLICABLE,
    evidence: [{ kind: 'docs', ref: 'README.md' }],
    justification:
      'The support contract claims macOS arm64/x64, glibc Linux x64, and Windows x64 only; Alpine and arm64 Linux are explicitly not support claims.'
  },

  // ---------------------------------------------------------------- Lane C
  {
    id: 'C1',
    lane: 'C',
    claim: 'A clean authentication/setup path is documented and exercised for every supported provider.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/setup_cli.rs' },
      { kind: 'docs', ref: 'docs/reference/cli-setup.md' },
      { kind: 'test', ref: 'crates/coven-cli/tests/doctor_auth_boundary.rs' }
    ]
  },
  {
    id: 'C2',
    lane: 'C',
    claim:
      'Launch, first output, input/continuation, termination, and final status work through the public interface.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' },
      { kind: 'script', ref: 'scripts/fixtures/fake-codex.mjs' },
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/harness_parity.rs' }
    ]
  },
  {
    id: 'C3',
    lane: 'C',
    claim: 'Missing/expired/refused credentials fail closed with actionable normalized errors.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/doctor_auth_boundary.rs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/setup_cli.rs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/doctor_prose_contract.rs' }
    ]
  },
  {
    id: 'C4',
    lane: 'C',
    claim: 'Credentials never appear in event, log, or evidence output.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/src/privacy.rs' },
      { kind: 'script', ref: 'scripts/check-coven-privacy.py' },
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/setup_cli.rs' }
    ]
  },
  {
    id: 'C5',
    lane: 'C',
    claim: 'Provider disappearance/timeout produces bounded state rather than indefinite ambiguity.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/process_supervisor.rs' },
      { kind: 'script', ref: 'scripts/release-stress.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' }
    ]
  },
  {
    id: 'C6',
    lane: 'C',
    claim: 'Unsupported providers never become implicitly supported because an executable happens to exist on PATH.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/src/harness.rs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/harness_parity.rs' }
    ]
  },
  {
    id: 'C7',
    lane: 'C',
    claim: 'Real-credential certification per supported provider (verify-only packet with real turns).',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'script', ref: 'scripts/certify-release.sh' },
      { kind: 'runbook', ref: 'docs/reference/releasing.md' },
      { kind: 'issue', ref: 'OpenCoven/coven#805' }
    ],
    ownerIssue: 805,
    justification:
      'Real provider credentials, an interactive TTY with explicit network/cost consent, and real usage spend keep this an operator certification-packet step; the hermetic PR lane stays credential-free.'
  },

  // ---------------------------------------------------------------- Lane D
  {
    id: 'D1',
    lane: 'D',
    claim: 'Daemon restart preserves durable sessions and state, including the managed identity contract.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/windows_daemon_lifecycle.rs' }
    ]
  },
  {
    id: 'D2',
    lane: 'D',
    claim: 'Harness/process crash before and after first output reaches bounded terminal-or-unknown state.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [{ kind: 'test', ref: 'crates/coven-cli/tests/process_supervisor.rs' }],
    ownerIssue: 807,
    justification:
      'Supervisor-level termination and stress cleanup are proven, but no hermetic test injects a harness crash in both the before-first-output and after-first-output windows and asserts the recorded disposition.'
  },
  {
    id: 'D3',
    lane: 'D',
    claim: 'Client disconnect/reconnect and event-cursor continuation work.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-client/tests/health.rs' },
      { kind: 'test', ref: 'crates/coven-cli/src/api.rs' },
      { kind: 'test', ref: 'crates/coven-client/src/lifecycle.rs' }
    ]
  },
  {
    id: 'D4',
    lane: 'D',
    claim:
      'Endpoint/peer replacement honors the typed client safety contract and never auto-replays a consequential mutation merely because transport changed.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [
      { kind: 'test', ref: 'crates/coven-client/src/error.rs' },
      { kind: 'test', ref: 'crates/coven-client/src/lifecycle.rs' }
    ],
    ownerIssue: 807,
    justification:
      'The typed client and its version-mismatch errors exist, but no test pins the no-auto-replay rule for consequential mutations across a transport replacement.'
  },
  {
    id: 'D5',
    lane: 'D',
    claim: 'Kill/cancel during active work reaches an authoritative terminal or explicit unknown/recovery state.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/process_supervisor.rs' },
      { kind: 'script', ref: 'scripts/user-journey-e2e.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' }
    ]
  },
  {
    id: 'D6',
    lane: 'D',
    claim:
      'Duplicate/retried requests use the operation idempotency/adoption semantics or are refused where the outcome is ambiguous.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [{ kind: 'test', ref: 'crates/coven-cli/tests/parallel_protocol.rs' }],
    ownerIssue: 807,
    justification:
      'Concurrent request safety is exercised, but no test submits a duplicate consequential mutation and asserts adoption-or-refusal semantics.'
  },
  {
    id: 'D7',
    lane: 'D',
    claim: 'Interrupted cleanup/orphan reconciliation is visible and deterministic.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [
      { kind: 'script', ref: 'scripts/release-stress.mjs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' }
    ],
    ownerIssue: 807,
    justification:
      'Process-cleanup stress and journey daemon cleanup are proven, but no focused test reconciles an orphan left by an interrupted cleanup.'
  },
  {
    id: 'D8',
    lane: 'D',
    claim: 'Corrupted state fails visibly and deterministically instead of silently succeeding.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [{ kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' }]
  },
  {
    id: 'D9',
    lane: 'D',
    claim: 'Unwritable state fails visibly and preserves recoverable evidence.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [
      { kind: 'issue', ref: 'OpenCoven/coven#807' },
      { kind: 'test', ref: 'crates/coven-cli/tests/smoke.rs' }
    ],
    ownerIssue: 807,
    justification:
      'No automated unwritable-state test exists; the gap is surfaced for the reliability scorecard program (#807).'
  },

  // ---------------------------------------------------------------- Lane E
  {
    id: 'E1',
    lane: 'E',
    claim: 'Event cursor continuation and paging semantics are exercised against the daemon contract.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-client/tests/health.rs' },
      { kind: 'test', ref: 'crates/coven-cli/src/api.rs' }
    ]
  },
  {
    id: 'E2',
    lane: 'E',
    claim: 'Event-writer pressure preserves lifecycle/tool/error/exit capacity according to contract.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/src/event_writer.rs' },
      { kind: 'script', ref: 'scripts/release-stress.mjs' }
    ]
  },
  {
    id: 'E3',
    lane: 'E',
    claim: 'Raw output loss/truncation is explicit, bounded, and observable rather than silent.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [{ kind: 'test', ref: 'crates/coven-cli/src/api.rs' }]
  },
  {
    id: 'E4',
    lane: 'E',
    claim: 'Oversized/malformed event/request bodies fail through structured errors.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/src/api.rs' },
      { kind: 'docs', ref: 'docs/API-CONTRACT.md' }
    ]
  },
  {
    id: 'E5',
    lane: 'E',
    claim: 'Redaction remains applied to default persisted/returned evidence.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/src/privacy.rs' },
      { kind: 'test', ref: 'crates/coven-cli/tests/setup_cli.rs' },
      { kind: 'script', ref: 'scripts/check-coven-privacy.py' }
    ]
  },
  {
    id: 'E6',
    lane: 'E',
    claim: 'Raw sensitive artifact opt-in remains separately protected/encrypted/retained according to current security policy.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/src/encrypted_artifacts.rs' },
      { kind: 'issue', ref: 'OpenCoven/coven#808' }
    ],
    ownerIssue: 808,
    justification:
      'Encrypted artifact storage exists in code, but the security-policy consolidation (#808) must first declare the retention/encryption contract this row certifies against.'
  },
  {
    id: 'E7',
    lane: 'E',
    claim:
      'Evidence receipts contain digests/references and sanitized outcomes rather than prompts, credentials, or unrestricted terminal output.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'script', ref: 'scripts/certify-release.sh' },
      { kind: 'runbook', ref: 'docs/reference/releasing.md' },
      { kind: 'script', ref: 'scripts/check-coven-privacy.py' }
    ]
  },

  // ---------------------------------------------------------------- Lane F
  {
    id: 'F1',
    lane: 'F',
    claim:
      'AFS mount stays experimental/disabled until its dedicated gate passes; it cannot become supported through packaging drift.',
    outcome: OUTCOMES.EXPERIMENTAL_DISABLED,
    evidence: [
      { kind: 'spec', ref: 'specs/coven-agent-fs/DESIGN.md' },
      { kind: 'spec', ref: 'specs/coven-agent-fs/MOUNT-SPIKE.md' },
      { kind: 'ci-job', ref: 'ci.yml#afs-mount-linux' },
      { kind: 'ci-job', ref: 'ci.yml#afs-mount-macos' }
    ],
    justification:
      'Mount is a cargo feature outside default builds and the spec scopes productionizing the mount spike out, so no mount capability is a current support claim.'
  },
  {
    id: 'F2',
    lane: 'F',
    claim: 'Installed-artifact mount gate exercised per platform before any mount support claim.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'script', ref: 'scripts/afs-mount-e2e.sh' },
      { kind: 'script', ref: 'scripts/afs-mount-smoke.sh' }
    ],
    ownerIssue: 805,
    justification:
      'The `--installed` gate is a manual post-release verification that needs a platform with local mount support; the v0.3.0 incident (published package omitted the mount helper) is why it must stay artifact-level and external to CI.'
  },
  {
    id: 'F3',
    lane: 'F',
    claim:
      'Pre-enablement gate (helper availability, credential observation, case-insensitive .git, handle reuse/gate-root enforcement, access-control boundaries, crash/restart/unmount recovery, concurrency, platform behavior, safe-disabled behavior) stays closed while the surface is experimental.',
    outcome: OUTCOMES.EXPERIMENTAL_DISABLED,
    evidence: [
      { kind: 'spec', ref: 'specs/coven-agent-fs/DESIGN.md' },
      { kind: 'script', ref: 'scripts/afs-mount-e2e.sh' }
    ],
    justification:
      'The gate checklist is defined but unclaimed; the mount capability stays experimental/disabled and cannot become supported through packaging drift because it is a feature-gated cargo feature outside default builds.'
  },

  // ---------------------------------------------------------------- Lane G
  {
    id: 'G1',
    lane: 'G',
    claim: 'Direct/handoff target ingress-policy parity is proven before stronger cross-agent safety claims.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [{ kind: 'issue', ref: 'OpenCoven/coven#803' }],
    ownerIssue: 803,
    justification: '#803 owns target-ingress-policy parity; stronger cross-agent claims stay blocked behind it.'
  },
  {
    id: 'G2',
    lane: 'G',
    claim: 'Local coven-agents behavior is certified as local/in-process semantics under the invocation/delegation contracts.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-agents/tests/runner.rs' },
      { kind: 'test', ref: 'crates/coven-agents/tests/loop_runner.rs' },
      { kind: 'issue', ref: 'OpenCoven/coven#804' }
    ],
    ownerIssue: 804,
    justification:
      'Local runner behavior is tested today, but certifying it now would pin a contract that #804 is migrating; the row follows that migration.'
  },
  {
    id: 'G3',
    lane: 'G',
    claim: 'No certification row describes the legacy handoff pointer-swap as durable distributed A2A request/response.',
    outcome: OUTCOMES.NOT_APPLICABLE,
    evidence: [{ kind: 'spec', ref: 'specs/coven-handoff-packet/TECH.md' }],
    justification:
      'The legacy handoff remains a local pointer swap; no distributed A2A request/response claim exists in docs or specs to certify.'
  },
  {
    id: 'G4',
    lane: 'G',
    claim:
      'Local and Coven-backed executors share one conformance suite (authorization, stable invocation ID, events, timeout/cancel, ambiguous adoption, duplicate submission, interruption, cleanup, secret-free evidence).',
    outcome: OUTCOMES.DEFERRED,
    evidence: [{ kind: 'issue', ref: 'OpenCoven/coven#804' }],
    ownerIssue: 804,
    justification: 'The shared conformance suite is part of the #804 architecture migration.'
  },
  {
    id: 'G5',
    lane: 'G',
    claim: 'Remote/multi-host placement reuses the existing hub and stays below agent-visible APIs.',
    outcome: OUTCOMES.NOT_APPLICABLE,
    evidence: [{ kind: 'spec', ref: 'specs/coven-multi-host-daemon/TECH.md' }],
    justification: 'No remote/multi-host placement support claim exists; the hub-based design remains below agent-visible APIs.'
  },

  // ---------------------------------------------------------------- Lane H
  {
    id: 'H1',
    lane: 'H',
    claim: 'Named version/capability negotiation works on the exact candidate.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'docs', ref: 'docs/API-CONTRACT.md' },
      { kind: 'test', ref: 'crates/coven-client/src/http.rs' },
      { kind: 'test', ref: 'crates/coven-cli/src/api.rs' }
    ]
  },
  {
    id: 'H2',
    lane: 'H',
    claim: 'Published client/package fixtures match the actual daemon contract.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-client/tests/health.rs' },
      { kind: 'script', ref: 'scripts/test-cli-prepublish.mjs' },
      { kind: 'script', ref: 'scripts/publish-npm-test.mjs' }
    ]
  },
  {
    id: 'H3',
    lane: 'H',
    claim: 'Unsupported version/capability denial is structured and deterministic.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'docs', ref: 'docs/API-CONTRACT.md' },
      { kind: 'test', ref: 'crates/coven-client/src/error.rs' }
    ]
  },
  {
    id: 'H4',
    lane: 'H',
    claim: 'Authentication/peer binding remains separate from capability advertisement.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [
      { kind: 'docs', ref: 'docs/AUTH.md' },
      { kind: 'docs', ref: 'docs/design/remote-listener-auth.md' }
    ],
    ownerIssue: 807,
    justification:
      'The separation is documented policy, but no hermetic test pins it on the running daemon; gap surfaced for the reliability scorecard program (#807).'
  },
  {
    id: 'H5',
    lane: 'H',
    claim: 'Malformed/oversized/unauthorized mutation paths fail before effects.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/src/api.rs' },
      { kind: 'docs', ref: 'docs/API-CONTRACT.md' }
    ]
  },
  {
    id: 'H6',
    lane: 'H',
    claim: 'Package consumers never rely on unpublished repository internals.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'package', ref: 'npm/coven/package.json' },
      { kind: 'script', ref: 'scripts/test-cli-prepublish.mjs' },
      { kind: 'script', ref: 'scripts/publish-npm-test.mjs' }
    ]
  },

  // ---------------------------------------------------------------- Lane I
  {
    id: 'I1',
    lane: 'I',
    claim: 'Canonical docs build and link checks pass in repository and deployed-site contexts.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'issue', ref: 'OpenCoven/coven#778' },
      { kind: 'docs', ref: 'docs/DOCS-MAINTENANCE.md' }
    ],
    ownerIssue: 778,
    justification: 'Docs build/link/browser-journey tooling is owned by #778, which is still open.'
  },
  {
    id: 'I2',
    lane: 'I',
    claim: 'Browser journey from install/discovery through first session/recovery uses the shipped commands/contracts.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [{ kind: 'issue', ref: 'OpenCoven/coven#778' }],
    ownerIssue: 778,
    justification: 'The deployed-site browser journey is #778 scope.'
  },
  {
    id: 'I3',
    lane: 'I',
    claim: 'Duplicate local public docs are removed or explicitly source-adjacent.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [{ kind: 'docs', ref: 'docs/DOCS-MAINTENANCE.md' }],
    ownerIssue: 778,
    justification:
      'The ownership rules are normative, but no automated duplicate-docs check runs; #778 owns the docs tooling that would enforce it.'
  },
  {
    id: 'I4',
    lane: 'I',
    claim: 'Help contract remains complete while default help is progressively disclosed.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'test', ref: 'crates/coven-cli/tests/help_disclosure.rs' },
      { kind: 'script', ref: 'scripts/export-cli-help-contract.mjs' },
      { kind: 'script', ref: 'scripts/cli-docs-test.mjs' }
    ]
  },
  {
    id: 'I5',
    lane: 'I',
    claim: 'Security/support text matches the security-policy truth and current capability state.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'issue', ref: 'OpenCoven/coven#808' },
      { kind: 'docs', ref: 'docs/SAFETY-MODEL.md' }
    ],
    ownerIssue: 808,
    justification: '#808 consolidates security-policy truth; this row certifies text against that outcome.'
  },
  {
    id: 'I6',
    lane: 'I',
    claim: 'No stale product/version/support claim survives after a certification state changes.',
    outcome: OUTCOMES.REQUIRED_UNKNOWN,
    evidence: [{ kind: 'docs', ref: 'docs/DOCS-MAINTENANCE.md' }],
    ownerIssue: 778,
    justification:
      'The stale-content rule is documented, but no check binds support-claim text to the certification state recorded here; #778 owns the docs tooling.'
  },

  // ---------------------------------------------------------------- Lane J
  {
    id: 'J1',
    lane: 'J',
    claim: 'Exact release/tag target has every required check success.',
    // Release-only requirement: applicable as soon as the candidate carries a
    // release tag, regardless of the statically declared outcome below.
    applicableWhen: { channels: [RELEASE_TAG_CHANNEL] },
    outcome: OUTCOMES.NOT_APPLICABLE,
    evidence: [
      { kind: 'workflow', ref: '.github/workflows/release-npm.yml' },
      { kind: 'workflow', ref: '.github/workflows/release-github.yml' }
    ],
    justification: 'No release tag is in flight for this candidate; the release gates run on the exact tag commit when a signed tag is pushed.'
  },
  {
    id: 'J2',
    lane: 'J',
    claim: 'Branch/ruleset/review protection evidence includes administrators.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [{ kind: 'issue', ref: 'OpenCoven/coven#805' }],
    ownerIssue: 805,
    justification: 'Protection-scope evidence including administrators needs admin-surface attestation outside this token\'s measured permissions.'
  },
  {
    id: 'J3',
    lane: 'J',
    claim: 'Signed tag/trusted signer/provenance/SBOM or declared dependency receipt passes.',
    applicableWhen: { channels: [RELEASE_TAG_CHANNEL] },
    outcome: OUTCOMES.NOT_APPLICABLE,
    evidence: [
      { kind: 'workflow', ref: '.github/workflows/release-npm.yml' },
      { kind: 'workflow', ref: '.github/workflows/release-github.yml' },
      { kind: 'runbook', ref: 'docs/reference/releasing.md' }
    ],
    justification: 'Proven per release by the signed-tag verification, OIDC provenance, and signature-audit preflight; no tag is in flight for this candidate.'
  },
  {
    id: 'J4',
    lane: 'J',
    claim: 'Generated/version state is coherent and clean.',
    outcome: OUTCOMES.REQUIRED_PASSED,
    evidence: [
      { kind: 'script', ref: 'scripts/publish-npm.mjs' },
      { kind: 'script', ref: 'scripts/publish-npm-test.mjs' },
      { kind: 'script', ref: 'scripts/package-github-release-test.mjs' }
    ]
  },
  {
    id: 'J5',
    lane: 'J',
    claim: 'Release channels agree on public version/support state.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'issue', ref: 'OpenCoven/coven#805' },
      { kind: 'runbook', ref: 'docs/reference/releasing.md' }
    ],
    ownerIssue: 805,
    justification: 'Channel agreement is a release-time attestation owned by #805.'
  },
  {
    id: 'J6',
    lane: 'J',
    claim: 'Artifact/package digests used by E2E match the published artifacts.',
    applicableWhen: { channels: [RELEASE_TAG_CHANNEL] },
    outcome: OUTCOMES.NOT_APPLICABLE,
    evidence: [
      { kind: 'runbook', ref: 'docs/reference/releasing.md' },
      { kind: 'workflow', ref: '.github/workflows/release-github.yml' }
    ],
    justification: 'No published artifact exists for this untagged candidate; the SHA256SUMS surface plus fresh-install verification binds digests at release.'
  },
  {
    id: 'J7',
    lane: 'J',
    claim: 'Mutation tests prove failed/missing checks, tag mismatch, signer failure, or security-disabled surfaces fail closed.',
    outcome: OUTCOMES.DEFERRED,
    evidence: [
      { kind: 'workflow', ref: '.github/workflows/release-github.yml' },
      { kind: 'script', ref: 'scripts/release-stress.mjs' }
    ],
    ownerIssue: 805,
    justification: 'The release workflows fail closed by design; adversarial mutation-test evidence is #805 scope.'
  },

  // ---------------------------------------------------------------- Lane K
  {
    id: 'K1',
    lane: 'K',
    claim:
      'Device-bound/QR/reconnection/recovery capabilities stay outside shipped certification until they become support claims.',
    outcome: OUTCOMES.NOT_APPLICABLE,
    evidence: [
      { kind: 'issue', ref: 'OpenCoven/coven#785' },
      { kind: 'spec', ref: 'spec/device-pairing/v1/README.md' },
      { kind: 'docs', ref: 'docs/design/mobile-pairing-protocol-v2.md' }
    ],
    justification:
      'No device/QR/biometric capability is a current support claim. Promotion requires cryptographic peer/device identity with scoped grants, QR replay/expiry/refusal coverage, biometric/passkey semantics where claimed, discovery/relay fallback without widened authority, device revocation/recovery/rotation, and cross-device continuity that preserves familiar/session authority.'
  }
];

// Human-readable labels used in docs/reference/certification.md tables. The
// parity test in certification-receipt-test.mjs keeps these in lockstep.
export const OUTCOME_LABELS = {
  'required-passed': 'required / passed',
  'required-failed': 'required / failed',
  'required-unknown': 'required / unknown (open blocker)',
  'not-applicable': 'not applicable',
  'experimental-disabled': 'experimental / disabled',
  deferred: 'deferred'
};

function laneTitle(laneId) {
  const lane = LANES.find((entry) => entry.id === laneId);
  if (!lane) {
    throw new Error(`matrix row references unknown lane ${laneId}`);
  }
  return lane.title;
}

export function laneOf(laneId) {
  return LANES.find((entry) => entry.id === laneId) ?? null;
}

// Structural validation of the matrix itself. Returns a list of problems; an
// empty list means the matrix is internally consistent and every
// non-terminal row is accountable (owner issue and/or justification where the
// outcome demands one).
export function validateMatrix(rows = CERTIFICATION_MATRIX) {
  const errors = [];
  const seen = new Set();
  const laneIds = new Set(LANES.map((lane) => lane.id));
  const platformIds = new Set(SUPPORT_INVENTORY.platforms.map((platform) => platform.id));

  for (const row of rows) {
    const where = `row ${row.id ?? '<missing id>'}`;
    if (!/^[A-K]\d+$/.test(row.id ?? '')) {
      errors.push(`${where}: id must look like <lane letter><digits>`);
      continue;
    }
    if (seen.has(row.id)) {
      errors.push(`${where}: duplicate id`);
    }
    seen.add(row.id);
    if (!laneIds.has(row.lane)) {
      errors.push(`${where}: lane '${row.lane}' is not declared`);
    }
    if (!ALL_OUTCOMES.has(row.outcome)) {
      errors.push(`${where}: unknown outcome '${row.outcome}'`);
    }
    if (!row.claim || row.claim.length < 16) {
      errors.push(`${where}: claim is missing or too short to be a certification row`);
    }
    if (!Array.isArray(row.evidence) || row.evidence.length === 0) {
      errors.push(`${where}: every row needs at least one evidence reference`);
    }
    if (row.outcome === OUTCOMES.DEFERRED && !row.ownerIssue) {
      errors.push(`${where}: deferred rows must name an owner issue`);
    }
    if (
      (row.outcome === OUTCOMES.NOT_APPLICABLE ||
        row.outcome === OUTCOMES.EXPERIMENTAL_DISABLED ||
        row.outcome === OUTCOMES.REQUIRED_UNKNOWN ||
        row.outcome === OUTCOMES.REQUIRED_FAILED) &&
      !row.justification
    ) {
      errors.push(`${where}: outcome '${row.outcome}' requires a justification`);
    }
    if (row.applicableWhen !== undefined) {
      const channels = row.applicableWhen?.channels;
      if (
        !Array.isArray(channels) ||
        channels.length === 0 ||
        !channels.every((channel) => SUPPORT_INVENTORY.channels.includes(channel))
      ) {
        errors.push(
          `${where}: applicableWhen.channels must be a non-empty subset of ${SUPPORT_INVENTORY.channels.join(', ')}`
        );
      }
    }
    if (row.platforms !== undefined) {
      if (
        !Array.isArray(row.platforms) ||
        row.platforms.length === 0 ||
        !row.platforms.every((platform) => platformIds.has(platform))
      ) {
        errors.push(
          `${where}: platforms must be a non-empty subset of the support inventory (${[...platformIds].join(', ')})`
        );
      }
    }
    for (const ref of row.evidence ?? []) {
      if (!EVIDENCE_KINDS[ref.kind]) {
        errors.push(`${where}: evidence kind '${ref.kind}' is not defined`);
      }
      if (!ref.ref) {
        errors.push(`${where}: evidence entry is missing ref`);
      }
    }
  }
  return errors;
}

// The candidate context drives row applicability: the channel comes from how
// the candidate is being certified (source checkout vs. release tag) and the
// tag exists exactly when the channel is the release channel. This replaces
// the old model where release-only rows were statically not-applicable and
// could silently vanish from go/no-go even when a tag was in flight.
export function resolveCandidateContext({ channel, tag = null } = {}) {
  if (!SUPPORT_INVENTORY.channels.includes(channel)) {
    throw new Error(
      `unknown certification channel '${channel}'; expected one of ${SUPPORT_INVENTORY.channels.join(', ')}`
    );
  }
  const releaseChannel = channel === RELEASE_TAG_CHANNEL;
  if (releaseChannel) {
    if (typeof tag !== 'string' || !/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag)) {
      throw new Error(`release-tag candidates require a tag named vX.Y.Z[-suffix]; got '${tag ?? ''}'`);
    }
  } else if (tag != null) {
    throw new Error(`channel '${channel}' must not carry a release tag; got '${tag}'`);
  }
  return Object.freeze({ channel, tag: releaseChannel ? tag : null });
}

// Whether a row applies to the candidate context, and why. Rows marked
// `applicableWhen` apply whenever the candidate channel matches, which
// overrides a statically declared not-applicable outcome (the release rows in
// lane J). Everything else applies per its declared outcome.
export function resolveApplicability(row, context) {
  const declaredChannels = row.applicableWhen?.channels;
  if (Array.isArray(declaredChannels)) {
    if (declaredChannels.includes(context.channel)) {
      return { applicable: true, basis: 'channel-matches-applicableWhen' };
    }
    return { applicable: false, basis: 'channel-not-in-applicableWhen' };
  }
  if (row.outcome === OUTCOMES.NOT_APPLICABLE) {
    return { applicable: false, basis: 'declared-not-applicable' };
  }
  return { applicable: true, basis: 'declared-applicable' };
}

// The certification rule from the issue, executable: skipped/unknown are not
// terminal outcomes for a required row, a proven-failed required row is a
// release blocker, deferred rows are pending at the source channel and become
// blockers at the release channel, and release-only rows apply as soon as the
// candidate carries a tag. Returns the open blockers for the given context.
export function certificationBlockers(rows = CERTIFICATION_MATRIX, context = null) {
  const candidateContext = context ?? resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL });
  const releaseChannel = candidateContext.channel === RELEASE_TAG_CHANNEL;
  const blockers = [];
  for (const row of rows) {
    const applicability = resolveApplicability(row, candidateContext);
    if (!applicability.applicable) {
      continue;
    }
    if (row.outcome === OUTCOMES.NOT_APPLICABLE) {
      // A release-only row whose channel arrived while the row is still
      // statically not-applicable is exactly the drift this function exists to
      // catch: the requirement is live but no evidence obligation is recorded.
      if (applicability.basis === 'channel-matches-applicableWhen') {
        blockers.push({
          id: row.id,
          lane: row.lane,
          reason: `release-only row became applicable for channel '${candidateContext.channel}' but is still statically not-applicable with no evidence binding`,
          ownerIssue: row.ownerIssue ?? null
        });
      }
      continue;
    }
    if (row.outcome === OUTCOMES.REQUIRED_FAILED) {
      blockers.push({
        id: row.id,
        lane: row.lane,
        reason: `required row proven failed: ${row.justification ?? row.claim}`,
        ownerIssue: row.ownerIssue ?? null
      });
    } else if (row.outcome === OUTCOMES.REQUIRED_UNKNOWN) {
      blockers.push({
        id: row.id,
        lane: row.lane,
        reason: `required row has an explicit unknown disposition: ${
          row.justification ?? 'no justification recorded'
        }`,
        ownerIssue: row.ownerIssue ?? null
      });
    } else if (row.outcome === OUTCOMES.DEFERRED && releaseChannel) {
      // Deferred rows must never disappear from a release go/no-go: at tag
      // time each of them is an open requirement owned by its issue.
      blockers.push({
        id: row.id,
        lane: row.lane,
        reason: `release candidate still carries a deferred requirement owned by #${row.ownerIssue}: ${
          row.justification ?? 'no justification recorded'
        }`,
        ownerIssue: row.ownerIssue ?? null
      });
    }
  }
  return blockers;
}

// Deferred rows stay visible in every go/no-go: at the source channel they are
// pending (owned, not blocking), at the release channel they are blockers.
export function pendingRows(rows = CERTIFICATION_MATRIX, context = null) {
  const candidateContext = context ?? resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL });
  if (candidateContext.channel === RELEASE_TAG_CHANNEL) {
    return [];
  }
  return rows
    .filter((row) => row.outcome === OUTCOMES.DEFERRED)
    .map((row) => ({ id: row.id, lane: row.lane, ownerIssue: row.ownerIssue ?? null }));
}

// The go/no-go verdict for a candidate context. `go` requires zero blockers,
// zero pending rows at the release channel, and — at the release channel — an
// approved reviewer decision recorded by release authorization (#805). The
// receipt never self-certifies: without an approved decision record the
// release channel cannot be `go`.
export function goNoGo(rows = CERTIFICATION_MATRIX, context = null, { reviewerDecision = null } = {}) {
  const candidateContext = context ?? resolveCandidateContext({ channel: SOURCE_CHECKOUT_CHANNEL });
  const releaseChannel = candidateContext.channel === RELEASE_TAG_CHANNEL;
  const blockers = certificationBlockers(rows, candidateContext);
  const pending = pendingRows(rows, candidateContext);
  const verdict =
    blockers.length === 0 && (!releaseChannel || (pending.length === 0 && reviewerDecision === 'approved'))
      ? 'go'
      : 'no-go';
  return {
    channel: candidateContext.channel,
    tag: candidateContext.tag,
    verdict,
    blockers,
    pending,
    reviewerDecision: releaseChannel ? reviewerDecision : null,
    summary: matrixSummary(rows)
  };
}

export function matrixSummary(rows = CERTIFICATION_MATRIX) {
  const summary = {
    requiredPassed: 0,
    requiredFailed: 0,
    requiredUnknown: 0,
    notApplicable: 0,
    experimentalDisabled: 0,
    deferred: 0,
    total: rows.length
  };
  for (const row of rows) {
    if (row.outcome === OUTCOMES.REQUIRED_PASSED) summary.requiredPassed += 1;
    else if (row.outcome === OUTCOMES.REQUIRED_FAILED) summary.requiredFailed += 1;
    else if (row.outcome === OUTCOMES.REQUIRED_UNKNOWN) summary.requiredUnknown += 1;
    else if (row.outcome === OUTCOMES.NOT_APPLICABLE) summary.notApplicable += 1;
    else if (row.outcome === OUTCOMES.EXPERIMENTAL_DISABLED) summary.experimentalDisabled += 1;
    else if (row.outcome === OUTCOMES.DEFERRED) summary.deferred += 1;
  }
  return summary;
}

export function receiptLanes(rows = CERTIFICATION_MATRIX) {
  return LANES.map((lane) => ({
    id: lane.id,
    title: lane.title,
    ownerIssue: lane.ownerIssue ?? null,
    rows: rows
      .filter((row) => row.lane === lane.id)
      .map((row) => ({ ...row, laneTitle: laneTitle(row.lane) }))
  }));
}
