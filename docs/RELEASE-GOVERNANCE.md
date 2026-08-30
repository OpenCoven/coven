---
title: "Coven release governance and shipped-truth attestation"
description: "Normative policy binding Coven publication to the exact source commit: required checks, signed tags, exact-source acceptance, receipts, and break-glass rules."
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
in flight, a cancelled or timed-out run, and a required check that was skipped
are all **non-evidence**. The gate fails closed on each of them.

## Control map

| Control | Mechanism | Status |
| --- | --- | --- |
| Exact-source acceptance before publication | `scripts/verify-release-commit-gate.mjs` wired into `release-npm.yml` (`exact-source-gate` job → `npm-publish` needs) and `release-github.yml` (pre-publication step) | Enforced in this repository |
| Required-check names are stable and contract-tested | `scripts/release-required-checks.json` + `scripts/check-ci-workflow-test.py::test_required_checks_manifest_matches_ci_job_names` | Enforced in this repository |
| Signed annotated tags from trusted signers | `release-npm.yml` `verify-tag` (GitHub verification + `NPM_RELEASE_ALLOWED_SIGNERS` + `git verify-tag`) | Existing, preserved |
| Tag target equals published commit; tag object immutable through the pipeline | `verify-tag` + `package-github-release.mjs` (`verifyAnnotatedTag`, `assertRemoteTagMatchesVerifiedContext`, `revalidateTag`) | Existing, preserved |
| Immutable release assets (no overwrite) | `syncGitHubRelease` hash-verified asset sync; refuse duplicate/extra assets outside audited operator action | Existing, preserved |
| Publication concurrency | `concurrency` groups with `cancel-in-progress: false` on both release workflows | Existing, preserved |
| Machine-readable acceptance receipt | `coven.release-commit-gate-receipt/v1` uploaded as workflow artifact `coven-release-commit-gate-<tag>` | Added |
| `main` branch protection incl. administrator enforcement | Repository ruleset (see below) | **Maintainer action required** (settings are not branch-editable) |
| SBOM, toolchain version pinning, security/support surface gate | — | **Remaining** (see "Remaining work") |

## Exact source acceptance

`release-npm.yml` resolves one immutable candidate SHA on `main`:

1. A `v*` tag push triggers the pipeline. The `verify-tag` job rejects
   lightweight tags, unsigned tags, tags not verified by GitHub, tags not signed
   by a key in `NPM_RELEASE_ALLOWED_SIGNERS`, tags whose annotated target is not
   a commit, and commits not contained in `main`. It emits the tagged commit as
   the `head_sha` output.
2. The `exact-source-gate` job runs `verify-release-commit-gate.mjs verify`
   against that SHA before any publication step may run. The script:
   - queries `GET /repos/{owner}/{repo}/actions/runs?head_sha=<sha>` (REST) and
     refuses to decide if the results are paginated away;
   - requires a run of the CI workflow (path, event, and branch come from the
     required-checks manifest) whose `head_sha` equals the tagged commit — a
     green ancestor or unrelated run is not evidence;
   - requires that run to be `completed` with conclusion `success`; queued,
     in-flight, failed, cancelled, and timed-out runs refuse;
   - queries `GET /repos/{owner}/{repo}/commits/<sha>/check-runs?filter=latest`
     (REST, paginated to completion or refusal) and requires, for the exact
     SHA:
     - every **strict** required check present, `completed`, `success`
       (missing, skipped, cancelled, or failed refuses);
     - every **routed** required check absent-or-skipped (legitimate path
       classification) or `success` (any other conclusion refuses);
   - accepts check-run evidence only from the `github-actions` app, so a
     third-party integration cannot satisfy a required check by name.
3. `npm-publish` declares `exact-source-gate` in its `needs`, so the registry
   mutation cannot start until the gate accepted the commit. The GitHub Release
   workflow re-runs the same gate against the same SHA before creating or
   updating the release, and the whole `Release npm packages` workflow must
   conclude `success` for the GitHub Release workflow to trigger at all.

## Required-check manifest governance

`scripts/release-required-checks.json` is the single source of truth for the
required-check names the release gate enforces:

- `strict_checks` are jobs that run on **every** push to `main`. Renaming or
  removing one is a release-policy change.
- `routed_checks` are jobs skipped by `scripts/classify-ci-changes.py` when the
  commit does not touch their surface. Their absence is legitimate; their
  failure is not.
- Each entry binds `name` (the stable check-run name) to the workflow `job_id`
  it belongs to.

When routing or job names change, CI fails closed: `check-ci-workflow-test.py`
re-parses `.github/workflows/ci.yml` and rejects any manifest entry whose
`name` no longer matches the job's display name (including the
`npm onboarding smoke (<target>)` matrix expansion) or whose `job_id` no longer
exists. The manifest MUST be updated in the same PR as the rename.

The same names are the ones repository branch protection should require (see
below); keeping them in one file is what makes the two sides agree.

## Branch and review protection (repository settings)

The following is enforced by repository configuration, which cannot be set
from a branch. Maintainers MUST apply it to `OpenCoven/coven`:

1. `main` requires the `PR gate` check (the canonical aggregate) plus the
   explicitly required non-routed checks that must never be skipped for a
   merge: `Classify changes` and `Policy guard`.
2. The ruleset MUST enable "Enforce for administrators" so review and required
   check policy applies to administrators as well as ordinary contributors.
3. Direct pushes to `main` MUST be restricted (`Restrict creations and updates`
   / require a pull request), so no accepted-review-bypassing push is possible.
4. Required checks MUST be configured as "expected before merge" with the
   strict-check names above so a missing check blocks merge even on first
   contribution.
5. Deletion of `main` and creation of matching heads MUST be blocked.

The equivalent REST sketch for maintainers (rulesets API):

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

`bypass_actors` MUST be empty in the default posture. Required checks are
attached to `main` through branch protection's required status checks using the
names in `scripts/release-required-checks.json`.

### Break-glass

Any bypass of the above (ruleset temporarily set to `evaluate`/`disabled`, an
administrator merge, or a manually re-run release) MUST:

1. be performed by a repository administrator with the organization audit log
   as the record of who/when/what;
2. be bounded to the single change or release that needed it;
3. open a post-use review issue within one business day stating the reason, the
   affected commit/tag, and the re-verification performed; the review MUST
   re-run the exact-commit gate against the shipped SHA before the incident is
   closed.

A silent permanent bypass MUST NOT exist.

## Tag, signer, and publication controls

- Version tags MUST be **signed annotated** tags created with
  `git tag -s vX.Y.Z -m ...` and pushed by a signer registered in the
  repository variable `NPM_RELEASE_ALLOWED_SIGNERS`.
- Tags MUST NOT be moved, force-updated, replaced, or deleted once released.
  Recovery from a publishing error is forward-only: push a new patch version
  with a new signed tag. The release scripts treat a changed tag object as a
  hard refusal (`revalidateTag`).
- Where a tag asset is genuinely wrong, the only sanctioned mutation is
  deleting the specific GitHub release asset through an audited operator
  action, then re-running the GitHub-only workflow. npm versions are immutable.
- Workflow permissions stay least privilege until publication:
  `contents: read` throughout, `actions: read`/`checks: read` only where the
  gate needs to read run/check evidence, `id-token: write` only on the npm
  publish job (OIDC trusted publishing), `contents: write` only in the
  GitHub Release job.
- Concurrency groups (`release-npm-<ref>`, `release-github-<tag>`) with
  `cancel-in-progress: false` prevent two mutations of the same
  version/channel racing.

## Release-channel synchronization

One public version, one accepted source, everywhere:

- npm packages and the GitHub Release are published from the same
  `verify-tag`-attested SHA; the GitHub Release is created only after the npm
  pipeline concluded success, and it re-checks the exact SHA first.
- `package-github-release.mjs` revalidates the remote tag object and head SHA
  before creating or uploading anything, and synchronizes assets
  content-addressed by SHA-256 (`SHA256SUMS`), refusing to overwrite an
  existing asset whose digest differs.
- The exact-source acceptance receipt (below) links the release to the check
  evidence for its commit; `SOURCE_DATE_EPOCH` derived from the source run
  keeps packaging deterministic.
- Partial publication recovery: re-run the failed workflow for the same tag.
  All steps are idempotent-or-refusing: existing assets are verified and
  skipped, never rewritten; npm re-publish of an existing version fails at the
  registry, which is the correct outcome.

## Supply-chain evidence

- npm provenance (SLSA v1 statements via OIDC trusted publishing) is verified
  against the attested build inputs before a GitHub Release is produced.
  Preserved as-is.
- Every GitHub Action used by CI and release workflows is pinned to a full
  commit SHA with the version noted in a comment.
- Known gap (tracked): the Rust toolchain is pinned via a SHA-pinned action but
  tracks the `stable` channel, a mutable ref. Pin to an exact toolchain
  version in a follow-up release-policy change.
- SBOM production/retention and a machine-readable security/support surface
  gate (which surfaces are publishable) remain open; see below.

## Release receipt

The exact-source gate writes a deterministic JSON receipt — no timestamps, no
secret values — uploaded as the workflow artifact
`coven-release-commit-gate-<tag>`:

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
evidence the release record must retain.

## Negative/mutation coverage

`scripts/verify-release-commit-gate-test.mjs` proves each failure class fails
closed: missing run for the exact SHA; green ancestor only; run payload SHA
mismatch; queued/in-flight (stale) run including a newer attempt over a green
older one; failed/cancelled/timed-out aggregate; non-`main` branch and
`pull_request` events as evidence; missing/skipped/failed/not-completed/cancelled
strict check; failed/in-progress routed check; third-party app evidence;
ambiguous duplicate check runs; mismatched check-run head SHA; malformed
manifests (schema, empty strict list, strict∩routed overlap, duplicates, bad
path, missing `job_id`); incoherent version metadata (non-`vX.Y.Z` tag);
malformed tag object SHA; paginated/truncated REST evidence; and CLI misuse.
`check-ci-workflow-test.py` proves the workflows keep the gate wired (gate job
present, `npm-publish` depends on it, receipt upload present, GitHub Release
re-validation present) and that manifest names stay bound to real CI jobs.

A green happy path alone does not prove the gate; the negative classes above do.

## Remaining work (slice boundary)

This change is the first coherent slice of issue #805. Explicitly remaining:

1. Apply the branch-protection/ruleset configuration above (maintainer action;
   not expressible from a branch).
2. Pin the Rust toolchain to an exact version instead of the `stable` channel.
3. Produce/retain SBOMs and add a security/support surface gate so a
   security-blocked surface cannot be re-enabled by packaging metadata alone.
4. Fold the remaining evidence (artifact digests, npm registry versions,
   signer verification output, generated-file cleanliness) into a single
   retained release receipt linked from the release body, extending
   `coven.release-commit-gate-receipt/v1`.
