---
summary: "Release flow for @opencoven/cli and platform packages."
description: "Operator runbook for releasing Coven to npm and GitHub Releases: OIDC setup, signed-tag publication, deterministic assets, provenance checks, and recovery."
read_when:
  - Cutting a release
title: "Releasing Coven to npm and GitHub Releases"
---

Coven publishes the `@opencoven/cli` wrapper and its four native platform packages (`@opencoven/cli-macos`, `@opencoven/cli-macos-x64`, `@opencoven/cli-linux-x64`, `@opencoven/cli-windows`) automatically from the **Release npm packages** GitHub Actions workflow.

The release is **driven by a signed git tag**. No `workflow_dispatch`, no manual approval click, no long-lived npm token: a maintainer runs `git tag -s vX.Y.Z` + `git push`, and the workflow verifies the tag signature, runs the full gate matrix, dry-runs, then publishes using **npm trusted publishing over GitHub Actions OIDC**, attaching a provenance attestation to every package.

Source package versions stay `0.0.0` in the repo. The published version comes from the tag name (`v0.0.17` → `0.0.17`) and is stamped into the wrapper and native packages at publish time by `scripts/publish-npm.mjs`.

## One-time setup (per package, per fresh npm publisher)

### Bootstrap a new native package

Trusted publisher configuration is scoped to an existing npm package record. When a new native package such as `@opencoven/cli-macos-x64` has never been published, create that record first with a short-lived, package-scoped credential under a bootstrap version. npm assigns `latest` to an initial public publish even when the `bootstrap` dist-tag is requested, so complete the trusted-publisher setup and recovery before publishing a wrapper that can select the bootstrap artifact.

Build the Intel macOS binary from the audited recovery checkout, export the short-lived granular token as `NPM_GRANULAR_TOKEN`, then publish only the bootstrap artifact:

```bash
NPM_CONFIG_TAG=bootstrap \
COVEN_NPM_VERSION=0.0.0-bootstrap.1 \
NODE_AUTH_TOKEN="$NPM_GRANULAR_TOKEN" \
node scripts/publish-npm.mjs --target=macos-x64 --skip-build --publish --skip-wrapper
```

This only creates the npm package record. Do not publish a production version or publish the wrapper before recovery replaces the bootstrap artifact with the OIDC-published production version, and revoke the temporary credential immediately after the bootstrap publish succeeds.

After every package already has an npm record, configure trusted publishing for every package. With npm 11.16 or newer, an authenticated publisher can configure it from the CLI:

```sh
npm trust github @opencoven/cli --repository OpenCoven/coven --file release-npm.yml --allow-publish --yes
npm trust github @opencoven/cli-macos --repository OpenCoven/coven --file release-npm.yml --allow-publish --yes
npm trust github @opencoven/cli-macos-x64 --repository OpenCoven/coven --file release-npm.yml --allow-publish --yes
npm trust github @opencoven/cli-linux-x64 --repository OpenCoven/coven --file release-npm.yml --allow-publish --yes
npm trust github @opencoven/cli-windows --repository OpenCoven/coven --file release-npm.yml --allow-publish --yes
```

Leave the environment unset (`no environment`). Each package should list `OpenCoven/coven` as the source repository and `release-npm.yml` as the workflow file. Verify the result with `npm trust list <package>`; for example:

```sh
npm trust list @opencoven/cli-macos-x64
```

The npm website exposes the same configuration:

For each of `@opencoven/cli`, `@opencoven/cli-macos`, `@opencoven/cli-macos-x64`, `@opencoven/cli-linux-x64`, `@opencoven/cli-windows`:

1. Sign in to npmjs.com as an account with publish rights on `@opencoven`.
2. Open the package settings page (e.g. `https://www.npmjs.com/package/@opencoven/cli/access`).
3. Under **Trusted Publishers → Add a new trusted publisher**, choose **GitHub Actions** and fill in:
   - **Organization or user**: `OpenCoven`
   - **Repository**: `coven`
   - **Workflow filename**: `release-npm.yml`
   - **Environment name**: leave blank (the workflow no longer uses a GitHub environment for the publish step).
4. Save.

Once all five packages are configured, the legacy `NPM_ACCESS_TOKEN` secret on the `npm-publish` GitHub environment is no longer needed and should be deleted as the final step of cutover so it cannot be reused to bypass OIDC:

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

### Certification packet

Produce the redacted certification report for every supported built-in harness
against the exact commit you intend to tag. Each report is a one-provider
artifact and the destination must not already exist:

```sh
mkdir -p cert
coven setup codex   --verify-only --report-json cert/codex.json
coven setup claude  --verify-only --report-json cert/claude.json
coven setup copilot --verify-only --report-json cert/copilot.json
```

Create the destination directory first: publication is fail-if-exists on the
report file itself, but Coven does not create parent directories, so a missing
`cert/` fails the command.

Verification requires an interactive terminal and explicit network/cost
consent, so this cannot run in CI — it is an operator step on a machine with
all three providers authenticated. Run non-interactively, the command exits
nonzero and writes no report.

Each report must show `"completed": true` and a `candidate_commit` equal to the
commit being tagged:

```sh
jq -r '"\(.harness) \(.completed) \(.candidate_commit)"' cert/*.json
```

`candidate_commit` is baked into the binary at build time, so a report whose
value does not match the release commit was produced by a different build and
does not certify this release. Reports are success-only and published
atomically; if verification fails, no file appears and the CLI exits nonzero.

Keep the packet with the release record. It contains only `harness`,
`cli_version`, `platform`, `candidate_commit`, `duration`, `exit_class`, and
`completed` — no output, account data, tokens, or private paths — so it is safe
to retain and share.

### Tag and push

Release tags are **immutable**. A pushed release tag is never moved, deleted,
recreated, or reused for different content, and a version that has been
published to npm is never republished. Every recovery path in this document
bumps forward to a new version instead. This is not a style preference: the
tag is the object that the npm provenance attestation, the GitHub Release, and
the certification packet all attest to, so moving it silently invalidates all
three.

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
3. **Build platform binaries** — matrix builds the release binary for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, and `x86_64-pc-windows-msvc`, then uploads each as an artifact.
4. **npm publish dry-run** — repacks the wrapper and native packages at the tag version and runs `npm publish --dry-run` for each. This is the same code path as the real publish minus the registry write, so a failure here means the real publish would also fail.
5. **npm publish** — authenticates via GitHub Actions OIDC (`permissions: id-token: write`), then runs `npm publish --provenance --access public` for the four native packages and the wrapper. Each published tarball gets a provenance attestation linking it to this exact workflow run and commit SHA, visible on each package's npm page.

### Postflight

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-macos-x64 version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

All five should now show the tag's version as `latest`. The package pages on npmjs.com should display a **"Provenance"** badge with a link back to the GitHub Actions run.

After the npm workflow succeeds, **Publish GitHub Release** (`.github/workflows/release-github.yml`) runs automatically from the default-branch workflow code. It refuses to touch GitHub Releases until it re-verifies the successful source npm workflow run through the GitHub API, confirms the signed annotated tag still resolves to that exact commit on `origin/main`, and proves all five npm packages have trusted-publisher / SLSA provenance tied to `https://github.com/OpenCoven/coven`, `.github/workflows/release-npm.yml`, `refs/tags/vX.Y.Z`, the tagged commit, and the source workflow run attempt. Its signature-audit preflight also rebuilds a throwaway consumer `package-lock.json` and rejects any final lock whose root dependency set is not exactly those five canonical packages at the tag version. After the four artifact downloads complete, it rechecks that the latest run attempt still equals the selected `source_run_attempt`; if a rerun started during download, the workflow fails closed before packaging or any GitHub Release mutation. Existing Releases are only accepted for recovery when the tag, title `Coven vX.Y.Z`, `draft=false`, and `prerelease=false` already match the canonical public release. Only then does it package deterministic archives and reconcile the matching GitHub Release.

The workflow creates or repairs one GitHub Release titled `Coven vX.Y.Z` using the signed tag annotation notes, with exactly these assets:

- `coven-vX.Y.Z-macos-aarch64.tar.gz`
- `coven-vX.Y.Z-macos-x64.tar.gz`
- `coven-vX.Y.Z-linux-x64.tar.gz`
- `coven-vX.Y.Z-windows-x64.zip`
- `SHA256SUMS`

`SHA256SUMS` contains exactly four lexically ordered entries naming only those four archive filenames. The GitHub Release is the public binary/checksum surface; npm provenance remains the package-integrity surface.

Download the published assets and verify the checksums actually match, rather
than trusting that the file exists:

```sh
gh release download vX.Y.Z --repo OpenCoven/coven --dir release-check
cd release-check && shasum -a 256 -c SHA256SUMS
```

`shasum -c` is macOS / Linux. On Windows PowerShell, compare hashes against the
same file explicitly:

```powershell
gh release download vX.Y.Z --repo OpenCoven/coven --dir release-check
cd release-check
Get-Content SHA256SUMS | ForEach-Object {
  $expected, $name = $_ -split '\s+', 2
  $actual = (Get-FileHash -Algorithm SHA256 $name.Trim()).Hash.ToLower()
  "$name $(if ($actual -eq $expected) { 'OK' } else { "FAILED ($actual)" })"
}
```

All four archives must report `OK`. This is the one checksum surface, so a
single mismatch fails the release regardless of what npm reports.

### Verify a fresh consumer install

The dry-run and packed smoke prove the artifacts *build*; only a real install
from the registry proves what a user actually gets. Do this in an environment
that has never had Coven installed — a container, a fresh VM, or a throwaway
user account — because a stale global install on `PATH` will shadow the new one
and make a broken release look fine:

```sh
npm install -g @opencoven/cli@X.Y.Z
coven --version         # must print X.Y.Z
coven doctor            # exits 1 with setup guidance on a bare machine
```

Confirm the `coven` you just ran is the only one on `PATH`. More than one
result means you are not testing the version you installed, and you must
resolve that before trusting any other result in this section:

```sh
which -a coven                     # macOS / Linux
```

```powershell
Get-Command coven -All             # Windows PowerShell
```

Verify the install is provenance-backed and resolved to the expected packages:

```sh
npm audit signatures
npm ls -g --depth 1 @opencoven/cli
```

Expect exactly two entries: the `@opencoven/cli` wrapper and the one native
package matching the current platform (for example `@opencoven/cli-macos` on
Apple silicon), both at `X.Y.Z`. A native package for a *different* platform, a
version mismatch between wrapper and native, or a missing native package are
each a failed release — not a cosmetic difference. All five packages are never
installed together on one machine; they are selected per platform.

Repeat on each platform you can reach. The `npm-onboarding` CI matrix covers
packed tarballs on all four targets, but it never installs from the public
registry, so this step is the only check of the real dist-tag resolution and
optional-dependency selection a user experiences.

### Confirm the certification packet

Re-check the preflight certification reports against what actually shipped:

```sh
jq -r '"\(.harness) \(.cli_version) \(.completed) \(.candidate_commit)"' cert/*.json
git rev-list -n 1 vX.Y.Z
```

Every report's `candidate_commit` must equal the commit the tag resolves to,
and every `completed` must be `true`. A mismatch means the packet certifies a
different build than the one published, and the packet must be regenerated
against the released commit before the release is considered closed out. File
the reports with the release record alongside the checksum verification above.

### Recover GitHub Release assets without touching npm

If the automatic GitHub-only workflow fails after the npm publication succeeded, rerun **Publish GitHub Release** with **Run workflow** / `workflow_dispatch` from the default branch and provide:

- `release_tag`: the immutable signed tag (`vX.Y.Z`)
- `source_run_id`: the successful **Release npm packages** run ID for that tag
- `source_run_attempt`: the exact successful **Release npm packages** run attempt for that run ID

To collect the recovery inputs safely:

1. Open the successful **Release npm packages** run for `vX.Y.Z` in the GitHub Actions UI, or list recent runs and copy the run ID shown there.
2. Query that run's immutable metadata and copy **both** the run ID and run attempt:
   ```sh
   gh api /repos/OpenCoven/coven/actions/runs/<source_run_id> \
     --jq '{release_tag: .head_branch, source_run_id: .id, source_run_attempt: .run_attempt, conclusion: .conclusion}'
   ```
3. Confirm the output still shows `release_tag: "vX.Y.Z"` and `conclusion: "success"`, then pass all three values into the recovery dispatch.

The recovery path is deliberately narrow:

- A missing GitHub Release may be created.
- A missing canonical asset may be uploaded.
- A canonical asset that already exists is streamed to a local file and hash-checked first; it is skipped only on an exact byte match.
- Any mismatched canonical asset or any extra/renamed asset fails closed. The workflow never overwrites GitHub assets automatically, never moves or reuses the tag, and never republishes npm.
- The workflow rechecks the latest run attempt once before download and once again after all downloads. If either check sees a newer attempt than the supplied `source_run_attempt`, recovery fails closed before packaging or mutation because `actions/download-artifact` is keyed only by run ID. In that case, use the newer successful run's ID/attempt pair instead of reusing the old attempt.

When a mismatch blocks recovery, record **the observed hash, the expected hash, and the reason** in the release log or incident notes. Then delete **only** the mismatched GitHub asset through an audited operator action (for example the GitHub UI or `gh release delete-asset ...`), and rerun the GitHub-only workflow with the same immutable tag plus the same source run ID/attempt pair. Do not delete matching assets, do not retag, and do not rerun the npm publish workflow for the same version.

### Recover a partial npm publication

Bump forward. There is no recovery-release path, and there deliberately is not
one: a recovery tag re-published an already-tagged version through a second,
weaker gate — it skipped the `origin/main` ancestry check that every other
release must pass, and it accepted a commit that had never landed on `main`.

npm forbids overwriting a published version, so a partial publish is never
repaired in place. Inspect the failed job, land the fix on `main` through the
normal review path, then push a new patch-bumped signed tag:

```sh
git fetch origin main --tags
git switch main
git pull --ff-only origin main
git tag -s vX.Y.Z+1 -m "Release vX.Y.Z+1"
git push origin vX.Y.Z+1
```

The skipped version keeps whichever packages already published; they are
superseded by the new version and the wrapper never points at the incomplete
set. Never move or reuse a release tag.

If the failure was a missing npm package record — a brand-new native package
such as `@opencoven/cli-macos-x64` — create that record with the bootstrap
procedure above and configure its trusted publisher before the next tag.

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
   cargo build --release --package coven-cli --target aarch64-apple-darwin
   cargo build --release --package coven-cli --target x86_64-apple-darwin
   cargo build --release --package coven-cli --target x86_64-unknown-linux-gnu
   cargo build --release --package coven-cli --target x86_64-pc-windows-msvc
   ```
3. Authenticate to npm with a freshly issued, narrowly-scoped granular token that covers all five packages (delete it immediately after).
4. Run `scripts/publish-npm.mjs --publish` for each target with `COVEN_NPM_VERSION` set to the tag version. The script's fallback path accepts `NPM_TOKEN` / `NODE_AUTH_TOKEN` when OIDC is not detected.
5. Publish the wrapper last, after all native packages are live, so users do not see a wrapper that points at native packages that don't yet exist at that version.
6. Revoke the temporary token.

Manually-published releases are not provenance-attested; document why the manual path was needed in the next release notes.
