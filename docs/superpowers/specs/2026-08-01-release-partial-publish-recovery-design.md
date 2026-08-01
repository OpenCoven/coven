# Signed-Tag Partial npm Release Recovery

## Context

The signed `v0.2.3` release passed every source, test, tag, and native build gate, then partially published. npm accepted `@opencoven/cli-linux-x64@0.2.3` and `@opencoven/cli-windows@0.2.3`, but rejected `@opencoven/cli-macos@0.2.3` because that package's trusted publisher was incorrectly constrained to a nonexistent environment. The wrapper `@opencoven/cli@0.2.3` was never attempted because the macOS publish failed first.

The trusted-publisher configuration is now corrected. Re-running the original job is still unsafe: `scripts/publish-npm.mjs` deliberately refuses to overwrite an existing package version, so the job would stop on Linux before reaching the missing packages. Publishing locally would complete the registry versions without GitHub OIDC provenance and would violate the repository's release contract.

## Decision

Extend the existing `.github/workflows/release-npm.yml` with a signed-tag-only recovery mode. A tag named `v<version>-recovery.<attempt>` runs through the same trusted workflow filename and publishes the base `<version>`, but only after proving that the recovery commit is an operational descendant of the original signed release tag.

For this incident, the recovery tag will be `v0.2.3-recovery.1` and the npm target version will remain `0.2.3`.

No `workflow_dispatch`, repository secret, long-lived npm token, version overwrite, or unsigned entry point is introduced.

## Safety Contract

The workflow must fail closed unless all of these conditions hold:

1. The pushed tag is either a normal stable release tag (`vX.Y.Z`) or a recovery tag (`vX.Y.Z-recovery.N`).
2. The pushed tag is annotated, GitHub-verified, locally verifiable against `NPM_RELEASE_ALLOWED_SIGNERS`, and points to a commit contained in `origin/main`.
3. For recovery mode, the base `vX.Y.Z` tag exists and independently passes the same annotated/signature/authorized-signer checks.
4. The base tagged commit is an ancestor of the recovery commit.
5. Every path changed after the base tag is operationally inert for the produced package: the release workflow, its focused tests, and this design/implementation documentation. Rust sources, npm package templates, lockfiles, build configuration, and dependencies are forbidden.
6. The expected partial registry state is present before publishing: Linux x64 and Windows already have the base version, while macOS and the wrapper do not.
7. Recovery mode skips Linux and Windows publication and invokes the existing macOS target once. That target publishes the macOS package first and the wrapper second.
8. If either missing package appears before the recovery publish begins, the existing publish script refuses to continue rather than trusting an unknown artifact.

## Workflow Shape

A preflight step derives these values from `GITHUB_REF_NAME`:

- `release_mode`: `normal` or `recovery`
- `release_tag`: the stable base tag, such as `v0.2.3`
- `npm_version`: the base version, such as `0.2.3`

The values are emitted as job outputs and consumed by dry-run and publish jobs instead of using `github.ref_name` directly.

Normal mode preserves the current sequence: Linux x64, Windows, then macOS plus wrapper. Recovery mode runs the same release and tag gates, builds the platform artifacts, checks the expected partial npm state, and runs only macOS plus wrapper with `COVEN_NPM_VERSION` set to the base release tag.

The recovery commit is intentionally the provenance source. It contains no product-code changes relative to the base tag, and the workflow records both the original release tag and recovery tag in its evidence.

## Failure Handling

- Malformed recovery tags fail before builds or registry access.
- Missing, lightweight, unverified, unauthorized, unrelated, or non-main tags fail before publication.
- A changed path outside the recovery allowlist fails with the exact offending path.
- Unexpected npm state fails before `npm publish`.
- Existing package versions continue to fail closed inside `scripts/publish-npm.mjs`.
- A failed recovery requires a new incremented signed recovery tag; recovery tags are never moved or reused.

## Verification

Focused tests must cover:

- normal stable-tag parsing and unchanged normal publish ordering;
- recovery-tag parsing and base-version derivation;
- rejection of malformed or prerelease-like tags that are not recovery tags;
- signed base-tag and ancestry checks represented in the workflow contract;
- recovery path allowlisting;
- exact expected npm-state checks;
- recovery mode skipping Linux and Windows while publishing macOS plus wrapper;
- continued absence of `workflow_dispatch` and long-lived npm credentials.

Before the PR is opened, run the repository release-facing tests plus all required Rust, secret, privacy, and diff gates. After merge, create and verify the signed recovery tag, wait for the complete GitHub Actions run, verify all four npm packages report `0.2.3` with provenance, then create the `v0.2.3` GitHub Release with the already-built native archives and `SHA256SUMS`.

## Non-Goals

- General retry of arbitrary release steps.
- Overwriting, unpublishing, or trusting pre-existing npm versions.
- Adding manual or token-authenticated publication.
- Changing runtime, CLI, npm wrapper, or native binary behavior.
- Retargeting either the original release tag or a recovery tag.
