---
summary: "Release flow for @opencoven/cli and platform packages."
description: "Operator runbook for releasing Coven to npm: one-time OIDC setup, signed-tag release, automated publish with provenance, and how to recover from a refused or failed release."
read_when:
  - Cutting a release
title: "Releasing Coven to npm"
---

Coven publishes the `@opencoven/cli` wrapper and its three native platform packages (`@opencoven/cli-macos`, `@opencoven/cli-linux-x64`, `@opencoven/cli-windows`) automatically from the **Release npm packages** GitHub Actions workflow.

The release is **driven by a signed git tag**. No `workflow_dispatch`, no manual approval click, no long-lived npm token: a maintainer runs `git tag -s vX.Y.Z` + `git push`, and the workflow verifies the tag signature, runs the full gate matrix, dry-runs, then publishes using **npm trusted publishing over GitHub Actions OIDC**, attaching a provenance attestation to every package.

Source package versions stay `0.0.0` in the repo. The published version comes from the tag name (`v0.0.17` → `0.0.17`) and is stamped into the wrapper and native packages at publish time by `scripts/publish-npm.mjs`.

## One-time setup (per package, per fresh npm publisher)

Before the first OIDC release, configure trusted publishing for every package on npmjs.com. This is a manual web-only step — there is no npm CLI for it today.

For each of `@opencoven/cli`, `@opencoven/cli-macos`, `@opencoven/cli-linux-x64`, `@opencoven/cli-windows`:

1. Sign in to npmjs.com as an account with publish rights on `@opencoven`.
2. Open the package settings page (e.g. `https://www.npmjs.com/package/@opencoven/cli/access`).
3. Under **Trusted Publishers → Add a new trusted publisher**, choose **GitHub Actions** and fill in:
   - **Organization or user**: `OpenCoven`
   - **Repository**: `coven`
   - **Workflow filename**: `release-npm.yml`
   - **Environment name**: leave blank (the workflow no longer uses a GitHub environment for the publish step).
4. Save.

Once all four packages are configured, the legacy `NPM_ACCESS_TOKEN` secret on the `npm-publish` GitHub environment is no longer needed and should be deleted as the final step of cutover so it cannot be reused to bypass OIDC:

```sh
gh secret delete NPM_ACCESS_TOKEN --env npm-publish --repo OpenCoven/coven
```

You can leave the `npm-publish` environment itself in place or remove it — the new workflow does not reference it.

## Cut a release

### Preflight

1. Confirm `main` CI is green for the exact commit you intend to release.
2. Run the local pre-publish smoke test from a clean checkout:
   ```sh
   node scripts/test-cli-prepublish.mjs
   ```
   This re-runs the secret-guard scan, the `publish-npm.mjs` unit tests, a full `npm publish --dry-run`, and a tarball pack + install that confirms the wrapper resolves and starts the native binary.
3. Check the current `latest` tag on npm so you pick a strictly-higher version:
   ```sh
   npm view @opencoven/cli version
   ```
4. Confirm the changelog and any README / brand updates have already landed on `main`.

### Tag and push

The tag must be **annotated and cryptographically signed**. Lightweight tags (`git tag vX.Y.Z`) are refused by the workflow.

```sh
git fetch origin main
git checkout main
git pull --ff-only
git tag -s v0.0.17 -m "Coven v0.0.17"
git push origin v0.0.17
```

That single push is the entire release. The workflow takes over from there.

### What the workflow does

1. **Release gates** — `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace --locked`, `python3 scripts/check-secrets.py`.
2. **Verify signed release tag** — confirms the pushed ref is an annotated tag (not lightweight) and that GitHub has cryptographically verified the maintainer's signature. The workflow consults `gh api /repos/{owner}/{repo}/git/tags/{sha}` and requires `.verification.verified == true`. Any other state aborts the release.
3. **Build platform binaries** — matrix builds the release binary for `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, and `x86_64-pc-windows-msvc`, then uploads each as an artifact.
4. **npm publish dry-run** — repacks the wrapper and native packages at the tag version and runs `npm publish --dry-run` for each. This is the same code path as the real publish minus the registry write, so a failure here means the real publish would also fail.
5. **npm publish** — authenticates via GitHub Actions OIDC (`permissions: id-token: write`), then runs `npm publish --provenance --access public` for the three native packages and the wrapper. Each published tarball gets a provenance attestation linking it to this exact workflow run and commit SHA, visible on each package's npm page.

### Postflight

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

All four should now show the tag's version as `latest`. The package pages on npmjs.com should display a **"Provenance"** badge with a link back to the GitHub Actions run.

Create or update the matching GitHub Release from the successful workflow artifacts. Package the downloaded binaries with the same public asset names as prior releases:

- `coven-vX.Y.Z-macos-aarch64.tar.gz`
- `coven-vX.Y.Z-linux-x64.tar.gz`
- `coven-vX.Y.Z-windows-x64.zip`
- `SHA256SUMS`

The release body should include the npm install command, published package list, action run URL, tagged commit, and compare link. This GitHub Release is the public binary/checksum surface; npm provenance remains the package-integrity surface.

If any package did not publish, do not re-tag with the same version (npm forbids overwrite). Inspect the failed job, fix the underlying cause, then push a new patch-bumped signed tag.

## Recovering from a refused release

### Tag was lightweight, not signed, or signed with a key GitHub does not recognise

`verify-tag` fails with one of:

- `Refusing to release: vX.Y.Z is a lightweight tag` — re-tag with `git tag -s` and force-replace:
  ```sh
  git tag -d v0.0.17
  git push origin :refs/tags/v0.0.17
  git tag -s v0.0.17 -m "Coven v0.0.17"
  git push origin v0.0.17
  ```
- `Tag vX.Y.Z does not have a GitHub-verified signature (reason=...)` — the signing key isn't registered against your GitHub account. Add it under [GitHub → Settings → SSH and GPG keys → Signing keys](https://github.com/settings/keys), then re-tag.

### Build matrix failure on a single platform

The platform matrix uses `fail-fast: false`, so the other targets still attempt to build. Look at the failed job's logs, fix the cause on `main`, and push a new signed tag with a bumped patch number.

### Dry-run shows a version conflict

`npm publish --dry-run` returns an error like *"previously published version X is higher than the new version Y"*. The tag is below the registry's current `latest`. Delete the tag and push a higher one.

### Real publish fails with `403 Forbidden` (or anything OIDC-related)

If the publish job authenticated cleanly under the old NPM_TOKEN model but now fails on OIDC, the trusted-publisher configuration is missing or scoped to the wrong workflow. Re-check the npmjs.com trusted-publisher settings for the failing package: **Organization/User = `OpenCoven`**, **Repository = `coven`**, **Workflow filename = `release-npm.yml`**, **Environment = blank**. The workflow filename and environment must match exactly — even a `releases-npm.yml` typo will cause npm to refuse the OIDC handshake.

## Emergency manual publish (last resort)

The new workflow does not expose a manual publish path. If you ever need to publish without going through CI (broken Actions runners, npm trusted-publishing outage, etc.):

1. Cut a signed tag locally as above so the artifact you publish is reproducible.
2. Build the native binaries:
   ```sh
   cargo build --release --target aarch64-apple-darwin
   cargo build --release --target x86_64-unknown-linux-gnu
   cargo build --release --target x86_64-pc-windows-msvc
   ```
3. Authenticate to npm with a freshly issued, narrowly-scoped granular token that covers all four packages (delete it immediately after).
4. Run `scripts/publish-npm.mjs --publish` for each target with `COVEN_NPM_VERSION` set to the tag version. The script's fallback path accepts `NPM_TOKEN` / `NODE_AUTH_TOKEN` when OIDC is not detected.
5. Publish the wrapper last, after all native packages are live, so users do not see a wrapper that points at native packages that don't yet exist at that version.
6. Revoke the temporary token.

Manually-published releases are not provenance-attested; document why the manual path was needed in the next release notes.
