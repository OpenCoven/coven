---
title: "Release governance"
summary: "Required branch, tag, and repository protection for Coven releases, the current-versus-required matrix, and the break-glass procedure."
description: "Administrator, review, release-tag, and break-glass policy that Coven publication enforces, with the live protection snapshot it was drafted from."
source_adjacent_reason: "Defines the policy file, ruleset bodies, and evaluator that release automation in this repository enforces."
read_when:
  - Changing branch protection, rulesets, or repository settings
  - Using an emergency bypass
  - Cutting a release that the governance check refuses
---

# Release governance

Coven publishes only from a commit that went through the same checks and
pull-request path as every other change, administrators included. Release
tags cannot be moved or deleted. Emergency bypasses are explicit, short, and
reviewed afterwards. This page states that policy. The files below make it
executable, and [issue #805](https://github.com/OpenCoven/coven/issues/805)
tracks delivery.

| File | Role |
| --- | --- |
| `.github/release-governance.json` | The required state, reviewed like code. |
| `.github/rulesets/*.json` | The exact ruleset bodies submitted to the GitHub rulesets API. |
| `scripts/release-governance.mjs` | Snapshots live protection over REST and evaluates it against the policy. |

## Decisions

Val chose these on 2026-10-18; they are recorded on #805.

- **Pull request required, zero approvals.** Every change to `main`,
  administrators' included, arrives through a pull request, the merge queue,
  and the `PR gate` check. No second approver is required, because the
  project has one maintainer and agents push as that maintainer. This is not
  independent review: the policy prevents unreviewed *paths* to `main`, not
  self-merging. Raising `required_approving_review_count` is a one-line policy
  change once a second reviewer exists.
- **No standing bypass.** No ruleset lists a bypass actor, except that tag
  creation is limited to repository administrators. An emergency uses the
  [break-glass procedure](#break-glass) instead of a permanent exemption.
- **A dedicated read credential.** The workflow `GITHUB_TOKEN` cannot read
  ruleset bypass actors, classic protection, or repository Actions settings, so
  a check run with it can never prove administrators are bound. The release
  check reads with `RELEASE_GOVERNANCE_READ_TOKEN`, a fine-grained credential
  limited to this repository with **Administration: Read-only**, and fails
  closed without it.
- **Immutable releases, published draft-first.** GitHub immutable releases
  lock a published release's assets and tag. The GitHub Release workflow
  therefore creates a draft, uploads and verifies every asset, then publishes.
  A published release with a wrong asset is superseded by a new version, never
  edited.

## Required state

Rulesets, not classic branch protection, are the enforcement layer. Rulesets
apply to administrators unless they list a bypass actor, and their rules are
readable by any reader. Classic protection may remain, but it must not exempt
administrators, because it can still be the most permissive layer a reader
sees.

The evaluator reports one control per row. A control passes only when an
**active** ruleset covering the ref enforces the rule with **zero** bypass
actors (tag creation excepted), with parameters at least as strict as the
policy. Hidden bypass actors, an unreadable endpoint, an unknown ref-pattern
token, or a ruleset in `evaluate` mode all fail.

| Control | Requirement |
| --- | --- |
| `branch.pull_request` | `main` changes only through a pull request; review threads must be resolved. |
| `branch.required_status_checks` | `PR gate` is required, and only from the GitHub Actions app (`15368`). |
| `branch.merge_queue` | Merges go through the merge queue, which tests each group against current `main`. |
| `branch.non_fast_forward` | No force pushes to `main`. |
| `branch.deletion` | `main` cannot be deleted. |
| `branch.classic-protection` | Classic protection is absent or has `enforce_admins` enabled. |
| `tags.update` | Release tags (`v*`, `coven-client-v*`) cannot be moved. |
| `tags.deletion` | Release tags cannot be deleted. |
| `tags.creation` | Only repository administrators (role `5`, mode `always`) can create release tags. |
| `repo.immutable-releases` | Published releases and their tags are immutable. |
| `repo.actions-sha-pinning` | Every action a workflow uses is pinned to a full commit SHA. |
| `repo.default-workflow-permissions` | Workflow tokens default to read-only. |
| `repo.workflow-review-approval` | Workflows cannot approve pull requests. |

Strict ("require branches to be up to date") status checks are not required:
the merge queue builds every merge group on the current tip of `main`, which is
the freshness strict checks would otherwise provide.

A bypass mode of `exempt` is treated like any other bypass, and stricter in
practice: GitHub skips the rules and writes no bypass audit entry, so it is a
silent bypass and never allowed.

## Current versus required

Snapshot taken 2026-10-19T02:26:51Z by a repository administrator, retained as
`scripts/fixtures/release-governance/snapshot-2026-10-19.json`. The test suite
asserts this table's pass/fail column against that fixture.

| Control | Observed | Status |
| --- | --- | --- |
| `branch.pull_request` | No ruleset requires a pull request. | Fail |
| `branch.required_status_checks` | `PR gate` is required only by classic protection, which exempts administrators. | Fail |
| `branch.merge_queue` | Ruleset `merge-queue-main` (#23575627), no bypass actors. | Pass |
| `branch.non_fast_forward` | Only classic protection blocks force pushes, and it exempts administrators. | Fail |
| `branch.deletion` | Only classic protection blocks deletion, and it exempts administrators. | Fail |
| `branch.classic-protection` | `enforce_admins=false`; the branch summary reports `enforcement_level: non_admins`. | Fail |
| `tags.update` | No tag ruleset exists. | Fail |
| `tags.deletion` | No tag ruleset exists. | Fail |
| `tags.creation` | No tag ruleset exists, so any writer can create a release tag. | Fail |
| `repo.immutable-releases` | `enabled: false`. | Fail |
| `repo.actions-sha-pinning` | `sha_pinning_required: false`; `ci.yml` uses tag-pinned actions. | Fail |
| `repo.default-workflow-permissions` | `read`. | Pass |
| `repo.workflow-review-approval` | `false`. | Pass |

The snapshot also records `strict: false` on classic required checks and no
required pull-request review, as #805 reported. No setting was changed to take
it.

## Check live protection

```sh
node scripts/release-governance.mjs check \
  --repository OpenCoven/coven \
  --snapshot-output governance-snapshot.json \
  --output governance-evaluation.json
```

It reads with `RELEASE_GOVERNANCE_READ_TOKEN`, then `GH_TOKEN`, then
`GITHUB_TOKEN`, then `gh auth token`. The snapshot records which source it
used, never the value. It exits non-zero when any control fails, after writing
both files. `evaluate --snapshot FILE --output FILE` re-evaluates a stored
snapshot without network access.

## Apply the policy

Administrator-only, and order matters. Each step is reversible by editing or
deleting what it created, but every one of them changes who can do what on
`main`, so run them deliberately and re-run the check after each.

1. Create the rulesets and bind administrators to classic protection. Neither
   depends on unreleased code:

   ```sh
   gh api -X POST repos/OpenCoven/coven/rulesets --input .github/rulesets/main-protection.json
   gh api -X POST repos/OpenCoven/coven/rulesets --input .github/rulesets/release-tags-immutable.json
   gh api -X POST repos/OpenCoven/coven/rulesets --input .github/rulesets/release-tags-creation.json
   gh api -X POST repos/OpenCoven/coven/branches/main/protection/enforce_admins
   ```

2. Require SHA-pinned actions only after every workflow on `main` pins its
   actions to commit SHAs; before that, the setting fails every CI run:

   ```sh
   gh api -X PUT repos/OpenCoven/coven/actions/permissions \
     -F enabled=true -f allowed_actions=all -F sha_pinning_required=true
   ```

3. Enable immutable releases only after the GitHub Release workflow publishes
   draft-first; the current create-then-upload flow cannot add assets to a
   published immutable release:

   ```sh
   gh api -X PUT repos/OpenCoven/coven/immutable-releases
   ```

4. Create the read credential: a fine-grained personal access token with
   repository access to `OpenCoven/coven` only and **Administration:
   Read-only**, stored as the repository secret
   `RELEASE_GOVERNANCE_READ_TOKEN`. Record its expiry on #805 and rotate it
   before then; an expired credential fails every release closed.

## Break-glass

Use this only when the normal path cannot serve, for example a broken `PR gate`
blocking the fix for that same gate. There is no standing exemption to use
instead.

1. **Open the record first.** File an issue titled `break-glass: <reason>`
   naming the ruleset, the rule, the exact change, why the normal path cannot
   serve, and the restore deadline. The window is at most 24 hours.
2. **Make the narrowest change.** Prefer adding one bypass actor to one
   ruleset, `{"actor_type": "User", "actor_id": <your user id>, "bypass_mode":
   "pull_request"}` on `main-protection`, so the bypass still goes through a
   pull request and GitHub records it in rule insights. Never use `exempt`,
   never disable a whole ruleset when one actor will do, and never touch the
   tag rulesets to repair a release: bump forward instead.
3. **Do only the recorded action.**
4. **Restore from the reviewed file**, not by hand:

   ```sh
   gh api -X PUT repos/OpenCoven/coven/rulesets/<ruleset id> --input .github/rulesets/<file>.json
   ```

5. **Prove the restore.** Run the check above and attach
   `governance-evaluation.json` (it must report `"compliant": true`) and the
   bypass record to the issue:

   ```sh
   gh api 'repos/OpenCoven/coven/rulesets/rule-suites?rule_suite_result=bypass&time_period=week'
   ```

6. **Review after use.** The issue stays open until a review comment records
   what was bypassed, whether the restore matched, and any follow-up that
   removes the need next time.

Release publication refuses while the check fails, so no release can publish
during a break-glass window or after an incomplete restore.

## Release tags

Release tags are immutable: never moved, deleted, recreated, or reused. With
the tag rulesets applied, that is enforced rather than conventional. A refused
tag (lightweight, unsigned, wrong signer, wrong commit) is abandoned and the
release bumps forward to the next patch version. Removing the stray tag is a
break-glass action and is not needed to proceed.

The crate release tags (`coven-client-v*`) receive the same protection. No
crate tag has been published yet.
