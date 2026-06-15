#!/usr/bin/env bash
# branch-triage/triage.sh — classify, merge, and prune branches.
#
# Inputs (from env, set by action.yml):
#   BASE_BRANCH     — base branch to measure against (default: main)
#   MERGE_STRATEGY  — squash | merge | rebase (default: squash)
#   DRY_RUN         — true | false (default: false)
#   STALE_DAYS      — integer; 0 = disable age check (default: 30)
#   GH_TOKEN        — GitHub token (passed as GH_TOKEN for gh CLI)
#
# Outputs (written to $GITHUB_OUTPUT):
#   merged_count    — number of PRs merged
#   deleted_count   — number of branches deleted
#   kept_count      — number of REVIEW branches skipped

set -euo pipefail

BASE="${BASE_BRANCH:-main}"
STRATEGY="${MERGE_STRATEGY:-squash}"
DRY="${DRY_RUN:-false}"
STALE="${STALE_DAYS:-30}"

merged_count=0
deleted_count=0
kept_count=0

# ── colour helpers ────────────────────────────────────────────────────────────
bold()  { printf '\033[1m%s\033[0m' "$*"; }
green() { printf '\033[32m%s\033[0m' "$*"; }
yellow(){ printf '\033[33m%s\033[0m' "$*"; }
red()   { printf '\033[31m%s\033[0m' "$*"; }
cyan()  { printf '\033[36m%s\033[0m' "$*"; }
dim()   { printf '\033[2m%s\033[0m'  "$*"; }

log()  { echo "$(cyan '[triage]') $*"; }
warn() { echo "$(yellow '[warn]') $*"; }
err()  { echo "$(red '[error]') $*" >&2; }

# ── Step 0 — fetch ────────────────────────────────────────────────────────────
log "Fetching all remotes and pruning stale tracking refs…"
git fetch --all --prune -q
git checkout "$BASE" -q
git pull -q

# ── Step 1 — collect PR data ──────────────────────────────────────────────────
log "Loading open and merged PR data from GitHub…"
OPEN_PR_JSON=$(gh pr list --state open  --json number,title,headRefName --limit 200)
MRGD_PR_JSON=$(gh pr list --state merged --json headRefName            --limit 500)

open_branches()  { echo "$OPEN_PR_JSON" | jq -r '.[].headRefName'; }
merged_branches(){ echo "$MRGD_PR_JSON" | jq -r '.[].headRefName'; }

pr_number_for() {
  local branch="$1"
  echo "$OPEN_PR_JSON" | jq -r --arg b "$branch" '.[] | select(.headRefName==$b) | .number'
}

pr_title_for() {
  local branch="$1"
  echo "$OPEN_PR_JSON" | jq -r --arg b "$branch" '.[] | select(.headRefName==$b) | .title'
}

branch_is_open_pr()   { open_branches  | grep -qxF "$1"; }
branch_is_merged_pr() { merged_branches | grep -qxF "$1"; }

unique_commits() {
  git log --oneline "$1" ^"origin/$BASE" 2>/dev/null | wc -l | tr -d ' '
}

last_commit_days_ago() {
  local ts
  ts=$(git log -1 --format="%ct" "$1" 2>/dev/null || echo 0)
  local now
  now=$(date +%s)
  echo $(( (now - ts) / 86400 ))
}

# ── Step 2 — classify ─────────────────────────────────────────────────────────
declare -a OPEN_LIST=()      # branches with open PRs
declare -a MERGED_LIST=()    # branches with merged PRs
declare -a SUPERSEDED_LIST=()# no PR, 0 unique commits (or stale)
declare -a REVIEW_LIST=()    # no PR, >0 unique commits — need human decision

log "Classifying branches…"
echo ""
printf "%-52s %-12s %-8s %s\n" "BRANCH" "CATEGORY" "UNIQUE" "PR"
printf "%s\n" "$(printf '─%.0s' {1..80})"

while IFS= read -r branch; do
  [[ "$branch" == "$BASE" ]] && continue
  [[ -z "$branch" ]] && continue

  uniq=$(unique_commits "$branch")
  age=$(last_commit_days_ago "$branch")

  if branch_is_open_pr "$branch"; then
    cat="OPEN"
    pr=$(pr_number_for "$branch")
    OPEN_LIST+=("$branch")
    printf "%-52s %-12s %-8s %s\n" "$branch" "$(green $cat)" "$uniq" "#$pr"
  elif branch_is_merged_pr "$branch"; then
    cat="MERGED"
    MERGED_LIST+=("$branch")
    printf "%-52s %-12s %-8s %s\n" "$branch" "$(dim $cat)" "$uniq" "—"
  elif [[ "$uniq" -eq 0 ]] || { [[ "$STALE" -gt 0 ]] && [[ "$age" -gt "$STALE" ]] && [[ "$uniq" -eq 0 ]]; }; then
    cat="SUPERSEDED"
    SUPERSEDED_LIST+=("$branch")
    printf "%-52s %-12s %-8s %s\n" "$branch" "$(dim $cat)" "$uniq" "—"
  else
    cat="REVIEW"
    REVIEW_LIST+=("$branch")
    printf "%-52s %-12s %-8s %s\n" "$branch" "$(yellow $cat)" "$uniq" "—"
  fi
done < <(git branch -r | sed 's|^\s*origin/||' | grep -v '^HEAD' | sort)

echo ""

# ── Step 3 — delete MERGED + SUPERSEDED ──────────────────────────────────────
log "Deleting MERGED and SUPERSEDED branches…"
for branch in "${MERGED_LIST[@]}" "${SUPERSEDED_LIST[@]}"; do
  if [[ "$DRY" == "true" ]]; then
    log "[dry-run] would delete: $branch"
  else
    if git push origin --delete "$branch" -q 2>/dev/null; then
      log "  deleted remote: $(red $branch)"
    else
      warn "  remote already gone: $branch"
    fi
    git branch -D "$branch" 2>/dev/null || true
    (( deleted_count++ )) || true
  fi
done

# Prune gone tracking refs
git remote prune origin -q
git branch -v | grep '\[gone\]' | awk '{print $1}' | xargs -r git branch -D 2>/dev/null || true

# ── Step 4 — REVIEW branches — report, do not touch ──────────────────────────
if [[ ${#REVIEW_LIST[@]} -gt 0 ]]; then
  echo ""
  warn "REVIEW branches (unique commits, no PR) — skipped, need manual decision:"
  for branch in "${REVIEW_LIST[@]}"; do
    uniq=$(unique_commits "$branch")
    echo "  $(yellow '→') $branch  ($(bold $uniq) unique commit(s))"
    git log --oneline "$branch" ^"origin/$BASE" 2>/dev/null | head -5 | sed 's/^/      /'
  done
  (( kept_count += ${#REVIEW_LIST[@]} )) || true
fi

# ── Step 5 — merge OPEN PRs ───────────────────────────────────────────────────
echo ""
log "Rebasing and merging open PRs (one at a time)…"

for branch in "${OPEN_LIST[@]}"; do
  pr=$(pr_number_for "$branch")
  title=$(pr_title_for "$branch")
  log "  PR #$pr — $title"

  if [[ "$DRY" == "true" ]]; then
    log "  [dry-run] would rebase $branch onto $BASE and merge #$pr"
    continue
  fi

  # Checkout and rebase
  git checkout "$branch" -q 2>/dev/null || {
    git checkout -b "$branch" "origin/$branch" -q
  }
  git fetch origin "$BASE":$BASE -q 2>/dev/null || true

  if ! git rebase "origin/$BASE" -q 2>/dev/null; then
    # Auto-resolve: take base for files not owned by this branch
    owned=$(git diff --name-only "origin/$BASE"..."$branch" 2>/dev/null || true)
    git diff --name-only --diff-filter=U | while IFS= read -r conflict; do
      if echo "$owned" | grep -qxF "$conflict"; then
        warn "    conflict in owned file $conflict — leaving for manual resolution"
        git rebase --abort
        warn "    aborted rebase for #$pr; skipping"
        continue 2
      else
        git checkout "origin/$BASE" -- "$conflict"
        git add "$conflict"
      fi
    done
    git rebase --continue --no-edit -q 2>/dev/null || {
      git rebase --abort 2>/dev/null || true
      warn "  rebase failed for #$pr ($branch) — skipping"
      continue
    }
  fi

  git push --force-with-lease origin "$branch" -q

  # Merge
  merge_flag="--$STRATEGY"
  if gh pr merge "$pr" $merge_flag --delete-branch 2>&1; then
    log "  $(green '✓') merged #$pr"
    (( merged_count++ )) || true
  else
    warn "  merge failed for #$pr — check CI or conflicts"
  fi

  # Re-sync base before next iteration
  git checkout "$BASE" -q
  git pull -q
done

# ── Step 6 — final state ──────────────────────────────────────────────────────
echo ""
log "Final state:"
git fetch --prune -q
remaining_open=$(gh pr list --state open --json number --limit 50 | jq 'length')
remaining_branches=$(git branch -r | grep -v 'HEAD' | grep -v "origin/$BASE$" | wc -l | tr -d ' ')
echo "  Open PRs remaining : $remaining_open"
echo "  Remote branches remaining (excl. $BASE): $remaining_branches"

# ── Step 7 — GitHub Job Summary ───────────────────────────────────────────────
{
  echo "## 🌿 Branch Triage Summary"
  echo ""
  echo "| Metric | Count |"
  echo "|--------|-------|"
  echo "| PRs merged | $merged_count |"
  echo "| Branches deleted | $deleted_count |"
  echo "| REVIEW branches skipped | $kept_count |"
  echo "| Open PRs remaining | $remaining_open |"
  echo ""

  if [[ ${#REVIEW_LIST[@]} -gt 0 ]]; then
    echo "### ⚠️ REVIEW branches (need manual decision)"
    echo ""
    for branch in "${REVIEW_LIST[@]}"; do
      uniq=$(unique_commits "$branch")
      echo "- \`$branch\` — $uniq unique commit(s)"
    done
    echo ""
  fi

  if [[ "$DRY" == "true" ]]; then
    echo "> **Dry-run mode** — no branches were deleted and no PRs were merged."
  fi
} >> "${GITHUB_STEP_SUMMARY:-/dev/null}"

# ── Outputs ───────────────────────────────────────────────────────────────────
{
  echo "merged_count=$merged_count"
  echo "deleted_count=$deleted_count"
  echo "kept_count=$kept_count"
} >> "${GITHUB_OUTPUT:-/dev/null}"

log "Done. merged=$merged_count deleted=$deleted_count kept=$kept_count"
