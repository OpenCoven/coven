---
summary: "Release flow for @opencoven/cli and platform packages."
description: "Operator runbook for releasing Coven to npm: preflight checks, dry-run publishing, cutting native platform packages, and postflight verification steps."
read_when:
  - Cutting a release
title: "Releasing Coven to npm"
---

Coven publishes the `@opencoven/cli` wrapper and its native platform packages (`@opencoven/cli-macos`, `@opencoven/cli-linux-x64`, `@opencoven/cli-windows`) from the **Release npm packages** GitHub Actions workflow.

Use this page when you are cutting a release. The source package versions stay `0.0.0` in the repo; the workflow dispatch `version` input is the published npm version.

## Preflight

Before publishing:

1. Confirm there are no open PRs that must land in the release.
2. Confirm `main` CI is green for the exact commit you will release.
3. Check npm for the current `latest` versions:

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

4. Confirm the changelog, package README status copy, and brand assets match the release.

## Dry run

Run the workflow with `publish=false` first. This builds all platform binaries and runs npm publish dry-runs without requiring npm credentials:

```sh
gh workflow run release-npm.yml \
  --ref main \
  -f publish=false \
  -f version=0.0.12
```

Watch the run:

```sh
gh run list --workflow release-npm.yml --branch main --limit 1
gh run watch <run-id>
```

## Publish

Only publish after the dry-run succeeds and npm package versions are still available:

```sh
gh workflow run release-npm.yml \
  --ref main \
  -f publish=true \
  -f version=0.0.12
```

The publish job uses the `npm-publish` environment and `NPM_ACCESS_TOKEN`. It publishes native packages first (`@opencoven/cli-linux-x64`, `@opencoven/cli-windows`, `@opencoven/cli-macos`) and then the wrapper package (`@opencoven/cli`).

## Postflight

After the publish run completes:

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

If any package did not publish, do not re-run blindly. Inspect the failed job and confirm which package versions exist on npm. If npm has already accepted part of the release, rerun only with a new version.
