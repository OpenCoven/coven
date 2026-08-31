---
title: "Coven release governance and shipped-truth attestation"
description: "Normative policy binding Coven publication to the exact source commit: required checks, signed tags, exact-source acceptance, receipts, repository rulesets, audited bypass, and break-glass rules."
---

# Release governance and shipped-truth attestation

Publication of a Coven release is an **authorization decision**, not a
consequence of pushing a tag. A release may ship one and only one thing: a
source commit that passed the repository's required quality and security
policy. Signed tags, provenance, and OIDC attestations prove origin; they do
not prove that the released commit passed CI. This document is the normative
contract that closes that gap and binds every release channel to the same
accepted source.

Normative language: **MUST**, **MUST NOT**, **MAY** follow RFC 2119.

## The invariant

> No artifact for version X may be published to any channel unless, at the
> moment of publication, the exact source commit `S` identified by the release
> tag has a completed CI run on `main` concluding `success`, and every required
> check for `S` reports `success`.

A green ancestor commit, a green run of an unrelated workflow, a re-run still
in flight, a cancelled or timed-out run, a required check that was skipped, and
check evidence from any check suite other than the selected run are all
**non-evidence**. The gate fails closed on each of them.

## Control map

| Control | Mechanism | Status |
| --- | --- | --- |
| Exact-source acceptance before publication | `scripts/verify-release-commit-gate.mjs` wired into `release-npm.yml` (`exact-source-gate` job → `npm-publish` needs, plus a point-of-mutation revalidation inside `npm-publish`) and `release-github.yml` (pre-publication verification job plus point-of-mutation revalidation in the publication job) | Enforced in this repository |
| Check evidence bound to the selected run/attempt | Evidence is the job list of the exact selected workflow-run attempt (`GET /actions/runs/{id}/attempts/{attempt}/jobs`); same-named check runs from other check suites cannot satisfy a required check | Added |
| Required-check names are stable, complete, and contract-tested | `scripts/release-required-checks.json` + `check-ci-workflow-test.py` and `verify-release-commit-gate-test.mjs` completeness contracts | Added |
| PR-merge policy kept separate from release policy | `pr_gate` (branch protection) vs `strict_checks`/`routed_checks` (release gate) in the same manifest | Added |
| Tag object + acceptance evidence persisted with the release | `coven-v<version>-release-evidence.json` (schema `coven.release-evidence/v1`), checksummed in `SHA256SUMS` | Added |
| Signed annotated tags from trusted signers | `release-npm.yml` `verify-tag` (GitHub verification + `NPM_RELEASE_ALLOWED_SIGNERS` + `git verify-tag`) | Existing, preserved |
| Tag target equals published commit; tag object immutable through the pipeline | `verify-tag` + `package-github-release.mjs` (`verifyAnnotatedTag`, `assertRemoteTagMatchesVerifiedContext`, `revalidateTag`, `revalidateRemoteTag`) at every mutation point | Existing, preserved |
| Immutable release assets (no overwrite) | `syncGitHubRelease` hash-verified asset sync; refuse duplicate/extra assets outside audited operator action | Existing, preserved |
| Stable-channel publication lock | `release-npm-stable-channel` and `release-github-stable-channel` concurrency groups serialize the complete npm-to-GitHub transaction across all tags | Changed (was per tag) |
| Read-only verification split from write-enabled publication | `release-github.yml` `verify-and-package` (`contents: read`, no persisted credentials) → `publish-release` (`contents: write` only) | Changed |
| Machine-readable acceptance receipt | `coven.release-commit-gate-receipt/v1` uploaded as workflow artifact `coven-release-commit-gate-<tag>` and packaged into the release evidence asset | Added |
| `main` branch protection incl. administrator enforcement | Repository ruleset (see below) | **Maintainer action required** (settings are not branch-editable) |
| `v*` tag ruleset (creation restricted, no update/deletion) | Repository ruleset (see below) | **Maintainer action required** (settings are not branch-editable) |
| Release immutability | No native GitHub control exists; bounded by `contents: write` ACL + audited asset-deletion procedure (see below) | **Maintainer action required** (ACL) + code-enforced refusal |
| SBOM, toolchain version pinning, security/support surface gate | — | **Remaining** (see "Remaining work") |

## Exact source acceptance

`release-npm.yml` resolves one immutable candidate SHA on `main`:

1. A `v*` tag push triggers the pipeline. The `verify-tag` job rejects
   lightweight tags, unsigned tags, tags not verified by GitHub, tags not signed
   by a key in `NPM_RELEASE_ALLOWED_SIGNERS`, tags whose annotated target is not
   a commit, and commits not contained in `main`. It emits the tagged commit as
   the `head_sha` output and the verified annotated tag object SHA as
   `tag_object_sha`.
2. The `exact-source-gate` job runs `verify-release-commit-gate.mjs verify`
   against that SHA before any publication step may run. The script:
   - queries `GET /repos/{owner}/{repo}/actions/runs?head_sha=<sha>` (REST) and
     refuses to decide if the results are paginated away;
   - requires a run of the CI workflow (path, event, and branch come from the
     required-checks manifest) whose `head_sha` equals the tagged commit — a
     green ancestor or unrelated run is not evidence;
   - requires that run to be `completed` with conclusion `success`; queued,
     in-flight, failed, cancelled, and timed-out runs refuse;
   - **binds evidence to the selected run and attempt**: the only accepted
     check evidence is the job list of that exact workflow-run attempt
     (`GET /actions/runs/{run_id}/attempts/{attempt}/jobs`, paginated to
     completion or refusal). Every job must carry the selected run id, the
     exact commit SHA, and the source workflow name, so a same-named check run
     from another check suite — a different workflow, a different attempt, or a
     third-party integration — can never satisfy a required check;
   - refuses any job that ran in the selected run but is not declared in
     `scripts/release-required-checks.json` (fail closed on manifest narrowing);
   - requires, for the exact SHA:
     - every **strict** required check present, `completed`, `success`
       (missing, skipped, cancelled, or failed refuses);
     - every **routed** required check absent-or-skipped (legitimate path
       classification) or `success` (any other conclusion refuses).
3. `npm-publish` declares `exact-source-gate` in its `needs` **and** re-proves
   authorization at the point of mutation: immediately before the first
   `npm publish` step it re-runs the gate on fresh check evidence and calls
   `package-github-release.mjs revalidate-tag`, which re-reads the remote tag
   from the GitHub API and refuses a moved, replaced, deleted, unsigned, or
   lightweight tag. A stale early authorization can never be spent.
4. The GitHub Release workflow (`release-github.yml`) is split:
   - `verify-and-package` is read-only (`contents: read`,
     `persist-credentials: false`): it re-verifies the source run, digests the
     required-checks manifest **from the verified source SHA as data**
     (`git show <release sha>:scripts/release-required-checks.json`) instead of
     trusting the default-branch working tree, re-runs the gate, verifies npm
     provenance and registry signatures, reconfirms the run attempt after
     artifact download, and packages the deterministic assets;
   - `publish-release` holds `contents: write` and is the only mutating job. It
     digests the manifest from the verified source SHA the same way, then
     immediately before the release mutation re-runs the exact-commit gate on
     fresh evidence, revalidates the remote signed tag, and reconfirms the
     source run attempt. Only then does it synchronize the already-verified
     assets. The whole `Release npm packages` workflow must have concluded
     `success` for this workflow to trigger at all.

## Required-check manifest governance

`scripts/release-required-checks.json` is the single source of truth for the
required-check names the release gate enforces, and it declares the **complete
expected sets**:

- `strict_checks` are jobs that run on **every** push to `main`. Renaming or
  removing one is a release-policy change.
- `routed_checks` are jobs skipped by `scripts/classify-ci-changes.py` when the
  commit does not touch their surface. Their absence is legitimate; their
  failure is not.
- `pr_only_checks` are jobs gated on the `pull_request` event. They never run
  for a release SHA, so they are never release evidence; they are listed so the
  manifest is a complete, auditable map of every CI job.
- `pr_gate` is the **pull-request merge contract** (branch protection): the
  `PR gate` aggregate plus the checks that run unconditionally on pull
  requests. This is deliberately a *different set* from the release policy:
  release-only jobs are conditioned on `github.event_name == 'push'` and never
  run on pull requests, so requiring them for merge would deadlock merges.
  Requiring every release check at merge time is also unnecessary — the
  release gate re-proves the full strict/routed set for the release SHA.
- Each entry binds `name` (the stable check-run name) to the workflow `job_id`
  it belongs to.

When routing or job names change, CI fails closed twice: contract tests
(`check-ci-workflow-test.py`, `verify-release-commit-gate-test.mjs`) re-parse
`.github/workflows/ci.yml` and require that **every CI job is claimed by
exactly the right policy dimension** with its real display name (including the
`npm onboarding smoke (<target>)` matrix expansion), and the release gate
itself refuses any job it observes in the selected run that the manifest does
not declare. The manifest MUST be updated in the same PR as the rename.

## Repository protection (maintainer action required)

The following is enforced by repository configuration, which cannot be set
from a branch. Maintainers MUST apply it to `OpenCoven/coven`. Until it lands,
the in-repo gates above hold even against an administrator push to `main`
(the release gate re-reads the world at publication time), but review bypass
and direct `v*` tag mutation remain possible — that residual risk is closed
only here.

### 1. Branch ruleset for `main`

```json
{
  "name": "main release policy",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    {
      "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 1,
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": false,
        "require_last_push_approval": true,
        "allowed_merge_methods": ["squash"]
      }
    }
  ],
  "bypass_actors": []
}
```

Settings that MUST also be enabled for `main`:

- **Require a pull request before merging** with 1 approving review, dismiss
  stale reviews on new pushes, and require approval of the most recent push.
- **Require status checks to pass**: `PR gate`, `Classify changes`, and
  `Policy guard` — exactly the `pr_gate` set declared in
  `scripts/release-required-checks.json` — marked as *required* so a missing
  check blocks merge even for a first contribution. (Release-only checks such
  as `CLI performance baseline` or the `npm onboarding smoke (…)` matrix are
  push-event jobs and MUST NOT be required here; they never run on pull
  requests.)
- **Enforce for administrators** (`enforcement: active` with an empty
  `bypass_actors` list): review and required-check policy applies to
  administrators too. This is the switch the review called out; without it the
  protection is advisory for admins.
- **Restrict creations and updates** of `main` to the allow-listed actors
  (nobody direct; everything through PRs).
- **Block deletions and non-fast-forward updates** of `main`.

### 2. Tag ruleset for `v*`

```json
{
  "name": "release tag policy",
  "target": "tag",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["v*"], "exclude": [] } },
  "rules": [
    { "type": "creation", "parameters": { "allow_creations": false } },
    { "type": "update" },
    { "type": "deletion" },
    { "type": "non_fast_forward" }
  ],
  "bypass_actors": []
}
```

Concretely, in the ruleset UI for target "Tags", pattern `v*`, enforcement
`Active`:

- **Restrict creations** to the release-signer role/team (the humans whose SSH
  keys are listed in `NPM_RELEASE_ALLOWED_SIGNERS`). Everyone else is blocked
  from creating `v*` tags at all.
- **Block updates, deletions, and non-fast-forward updates** for *everyone*,
  including administrators — a pushed and released tag is immutable. Recovery
  is forward-only (new patch version, new signed tag).
- Keep `bypass_actors` empty. The workflow-level `verify-tag` and
  `revalidate-tag` checks assume the tag cannot change underneath them.

### 3. GitHub Release mutability

GitHub has no native "lock release" control: any actor with `contents: write`
can edit a release body or delete an asset through the API/UI. The code-level
bounds are: `syncGitHubRelease` refuses to create, overwrite, or mismatch
assets outside the verified set; `publish-release` is the only `contents:
write` job; and asset deletion is only sanctioned as an audited operator
action (below). Maintainers MUST additionally limit who holds `contents:
write` on the repository (and, where available, restrict the GitHub App /
token scopes used for operations) so the release-mutation ACL is as small as
the signer list.

### 4. Bounded audited bypass process

Any bypass of the above — a ruleset temporarily switched to `evaluate`/
`disabled`, an administrator merge under enforcement-off, a manual release
mutation, or an audited asset deletion — MUST:

1. be performed by a repository administrator, with the organization audit log
   (and the security log for ruleset changes) as the record of who/when/what;
2. be **time-boxed and bounded** to the single change or release that needed
   it: enforcement is restored before the incident is closed, and the bypass
   MUST NOT be used for any second purpose;
3. open a post-use review issue within one business day stating the reason,
   the affected commit/tag, the exact bypass used, and the re-verification
   performed; the review MUST re-run the exact-commit gate against the shipped
   SHA before the incident is closed;
4. never move, reuse, or delete a released tag; where an asset is genuinely
   wrong, the only sanctioned mutation is deleting the specific GitHub release
   asset (recorded per step 3), then re-running the GitHub-only workflow.

A silent permanent bypass MUST NOT exist. If a bypass is needed more than
once for the same reason, the underlying policy MUST be changed by PR instead.

## Tag, signer, and publication controls

- Version tags MUST be **signed annotated** tags created with
  `git tag -s vX.Y.Z -m ...` and pushed by a signer registered in the
  repository variable `NPM_RELEASE_ALLOWED_SIGNERS`.
- Tags MUST NOT be moved, force-updated, replaced, or deleted once released.
  Recovery from a publishing error is forward-only: push a new patch version
  with a new signed tag. The release scripts treat a changed tag object as a
  hard refusal (`revalidateTag`, `revalidateRemoteTag`).
- Workflow permissions stay least privilege until publication:
  `contents: read` throughout verification, `actions: read`/`checks: read`
  only where the gate needs to read run/job evidence, `id-token: write` only
  on the npm publish job (OIDC trusted publishing), `contents: write` only in
  the GitHub Release publication job. Release checkouts set
  `persist-credentials: false` so no git credentials outlive checkout.
- Publication is serialized by one stable-channel lock per channel
  (`release-npm-stable-channel`, `release-github-stable-channel`) with
  `cancel-in-progress: false`: two stable versions can never interleave
  their package publications on the shared `latest` dist-tag, and the complete
  npm-to-GitHub release transaction is serialized across all tags. Pending
  releases queue behind an in-flight one rather than cancelling it.

## Release-channel synchronization

One public version, one accepted source, everywhere:

- npm packages and the GitHub Release are published from the same
  `verify-tag`-attested SHA; the GitHub Release is created only after the npm
  pipeline concluded success, and it re-proves the exact SHA immediately
  before its own mutation.
- `package-github-release.mjs` revalidates the remote tag object and head SHA
  before creating or uploading anything, and synchronizes assets
  content-addressed by SHA-256 (`SHA256SUMS`), refusing to overwrite an
  existing asset whose digest differs.
- The release itself carries its evidence: `coven-v<version>-release-evidence.json`
  (the verified tag object plus the exact-commit acceptance receipt) is
  checksummed in `SHA256SUMS` and uploaded as a release asset.
- `SOURCE_DATE_EPOCH` derived from the source run keeps packaging
  deterministic.
- Partial publication recovery is **forward-only and channel-scoped**:
  - npm versions are immutable. npm re-publication of an existing version
    fails at the registry by design — re-running the npm publish workflow for
    a version that already shipped is NOT a recovery path and MUST NOT be
    attempted. If npm publication itself failed part-way, push a new
    patch-bumped signed tag.
  - If only the GitHub Release failed after npm succeeded, the sanctioned
    recovery is re-running the GitHub-only workflow (`workflow_dispatch` of
    "Publish GitHub Release" with the same immutable tag and the same source
    run ID/attempt pair). All its steps are idempotent-or-refusing: existing
    assets are verified and skipped, never rewritten.

## Supply-chain evidence

- npm provenance (SLSA v1 statements via OIDC trusted publishing) is verified
  against the attested build inputs before a GitHub Release is produced.
  Preserved as-is.
- **Action pinning is enforced only for the release workflows**:
  `.github/workflows/release-npm.yml` and `.github/workflows/release-github.yml`
  pin every third-party action to a full commit SHA with the version noted in
  a comment, and `publish-npm-test.mjs` rejects any `uses:` line on the
  release workflow that is not a 40-character commit SHA. CI
  (`.github/workflows/ci.yml`) still references several actions by mutable tag
  (`actions/checkout@v7.0.1`, `dtolnay/rust-toolchain@stable`,
  `actions/setup-node@v7`, `actions/setup-python@v7`,
  `actions/upload-artifact@v7`); pinning those is a tracked follow-up, not a
  property to claim here.
- Known gap (tracked): the Rust toolchain is pinned via a SHA-pinned action but
  tracks the `stable` channel, a mutable ref. Pin to an exact toolchain
  version in a follow-up release-policy change.
- SBOM production/retention and a machine-readable security/support surface
  gate (which surfaces are publishable) remain open; see below.

## Release receipt

The exact-source gate writes a deterministic JSON receipt — no timestamps, no
secret values — uploaded as the workflow artifact
`coven-release-commit-gate-<tag>` and embedded, together with the verified
tag object, in the checksummed release asset
`coven-v<version>-release-evidence.json` (schema
`coven.release-evidence/v1`):

```json
{
  "schema": "coven.release-commit-gate-receipt/v1",
  "decision": "accepted",
  "repository": "OpenCoven/coven",
  "commit_sha": "<exact release commit>",
  "release_tag": "vX.Y.Z",
  "tag_object_sha": "<annotated tag object>",
  "required_checks_manifest": { "schema": "...", "sha256": "...", "strict_count": 10, "routed_count": 7 },
  "source_workflow": { "name": "CI", "path": ".github/workflows/ci.yml", "event": "push", "branch": "main" },
  "workflow_run": { "id": "...", "run_number": 12, "run_attempt": 1, "head_sha": "...", "conclusion": "success", "url": "..." },
  "checks": [ { "name": "Policy guard", "job_id": "policy-guard", "class": "strict", "status": "completed", "conclusion": "success" } ],
  "generated_from": "scripts/verify-release-commit-gate.mjs"
}
```

`checks` records the name, class, and conclusion of every required check for
the release commit — the "required check names/conclusions for that SHA"
evidence the release record must retain. `workflow_run` identifies the exact
selected run and attempt whose job list produced the evidence.

## Negative/mutation coverage

`scripts/verify-release-commit-gate-test.mjs` proves each failure class fails
closed: missing run for the exact SHA; green ancestor only; run payload SHA
mismatch; queued/in-flight (stale) run including a newer attempt over a green
older one; failed/cancelled/timed-out aggregate; non-`main` branch and
`pull_request` events as evidence; **cross-suite job binding** (a same-named
job from another run/attempt is refused outright and cannot satisfy a
required check); a job whose workflow name differs from the source workflow; a
job reporting a stale head SHA; an undeclared (unclassified) job in the
selected run; duplicate or unnamed jobs; missing/skipped/failed/
not-completed/cancelled strict check; failed/in-progress routed check;
malformed manifests (schema, empty strict list, strict∩routed overlap,
duplicates, bad path, missing `job_id`, PR-only/release job-id collisions);
**manifest completeness** (every `ci.yml` job claimed by exactly the right
policy dimension; PR-gate entries not bound to push-only jobs); incoherent
version metadata (non-`vX.Y.Z` tag); malformed tag object SHA;
paginated/truncated REST evidence (runs and jobs); and CLI misuse.

`scripts/package-github-release-test.mjs` proves the GitHub-only side:
`revalidateRemoteTag` refuses moved/deleted/unsigned/lightweight tags; the
release evidence bundle refuses incoherent receipt/tag combinations and is
checksummed with the release; `syncGitHubRelease` revalidates before create
and upload; and the workflow contract asserts the read-only/write split,
`persist-credentials: false`, stable-channel locks, manifest-from-verified-SHA
digestion, point-of-mutation revalidation placement, and the evidence asset
wiring. `check-ci-workflow-test.py` proves the workflows keep the gate wired
and the manifest complete.

A green happy path alone does not prove the gate; the negative classes above do.

## Remaining work (slice boundary)

1. Apply the branch ruleset, the `v*` tag ruleset, the `contents: write` ACL
   restriction, and the audited-bypass process above (maintainer action; not
   expressible from a branch).
2. Pin the Rust toolchain to an exact version instead of the `stable` channel,
   and pin the remaining mutable `uses:` tags in `ci.yml`.
3. Produce/retain SBOMs and add a security/support surface gate so a
   security-blocked surface cannot be re-enabled by packaging metadata alone.
4. Fold the remaining evidence (artifact digests, npm registry versions,
   signer verification output, generated-file cleanliness) into a single
   retained release receipt linked from the release body, extending
   `coven.release-evidence/v1`.
