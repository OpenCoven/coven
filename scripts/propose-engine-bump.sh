#!/usr/bin/env bash
# Usage: scripts/propose-engine-bump.sh <engine-version>
# Proposes OpenCoven/coven-code v<version> as the pinned engine: regenerates
# crates/coven-cli/engine.lock with scripts/pin-engine.sh, pushes
# engine-bump/<version> and opens a pull request for a person to review and
# merge. Run by .github/workflows/engine-bump.yml. Requires gh and push access
# to origin.
#
# A run that finds the branch on origin without a pull request opens the pull
# request an earlier run could not open. A run that cannot leave a pull
# request with CI started fails and says what to configure.
#
# WORKFLOW_TOKEN_ONLY=true says GH_TOKEN is the workflow's own GITHUB_TOKEN.
# GitHub starts no pull_request workflow for a pull request that token opens,
# so the required PR gate would never report on it.
set -euo pipefail
VERSION="${1:?usage: propose-engine-bump.sh <version>}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: not a stable engine version: $VERSION" >&2
  exit 1
fi
cd "$(dirname "$0")/.."
LOCK=crates/coven-cli/engine.lock
BRANCH="engine-bump/${VERSION}"
TITLE="chore(engine): bump pinned engine to ${VERSION}"
TOKEN_HELP="Set the ENGINE_BUMP_TOKEN repository secret to a token with contents and pull-request write access to this repository."

bash scripts/pin-engine.sh "$VERSION"

if git ls-remote --exit-code --heads origin "$BRANCH" >/dev/null 2>&1; then
  if [ "$(gh pr list --head "$BRANCH" --state all --json number --jq length)" != "0" ]; then
    echo "a pull request for $BRANCH already exists; nothing to do"
    exit 0
  fi
  git fetch --quiet origin "refs/heads/$BRANCH"
  if ! git show "FETCH_HEAD:$LOCK" | cmp -s - "$LOCK"; then
    echo "::error::$BRANCH is on origin with an engine.lock that differs from the one v${VERSION} produces now. Delete the branch, then run this workflow again."
    exit 1
  fi
  echo "$BRANCH is on origin without a pull request; opening it"
else
  git config user.name "coven-bot"
  git config user.email "coven-bot@users.noreply.github.com"
  git checkout -b "$BRANCH"
  git add "$LOCK"
  # This commit is unsigned: coven-bot has no signing key in CI. It lands on a
  # throwaway engine-bump/* branch and is reviewed and merged by a person
  # through GitHub. The resulting merge or squash commit is what branch
  # protection governs; do not add signing config here.
  git commit -m "$TITLE"
  git push origin "$BRANCH"
fi

if ! gh pr create \
  --title "$TITLE" \
  --body "Automated engine pin bump to \`${VERSION}\` (from the latest OpenCoven/coven-code release). The \`engine-contract\` CI job verifies this coven build drives the pinned engine. Regenerated via \`scripts/pin-engine.sh ${VERSION}\`." \
  --base main \
  --head "$BRANCH"; then
  echo "::error::Could not open the pull request for $BRANCH, which is pushed. GITHUB_TOKEN may not open pull requests in this repository. $TOKEN_HELP The next run opens the pull request from the pushed branch."
  exit 1
fi

if [ "${WORKFLOW_TOKEN_ONLY:-false}" = "true" ]; then
  echo "::error::The pull request for $BRANCH was opened with GITHUB_TOKEN, so no CI started on it. Close and reopen it to start CI. $TOKEN_HELP"
  exit 1
fi
