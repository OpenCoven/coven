# SKILL.md: Disk Space Optimization

**Purpose:** Intelligently free disk space by removing build artifacts, caches, and stale repositories without affecting active development.

**Scope:** Analyze disk usage, identify safe cleanup targets, preserve active projects, and report freed space.

---

## Overview

This skill helps manage constrained disk space by:
1. **Diagnosing** current usage hotspots
2. **Identifying** artifacts that can be safely removed (caches, builds, stale repos)
3. **Preserving** all active projects and development environments
4. **Reporting** what was freed and what remains

**Safe to use:** Does NOT delete source code, `.git` history, or active project dependencies.

---

## Usage

```bash
# Full diagnosis and cleanup
openclaw skill disk-space-optimizer analyze --aggressive
openclaw skill disk-space-optimizer cleanup --exclude="active,important"

# Report only (no changes)
openclaw skill disk-space-optimizer report

# Verify what's safe to delete
openclaw skill disk-space-optimizer candidates --days=30
```

---

## Core Procedures

### 1. Diagnosis: Identify Hotspots

**Goal:** Find what's consuming space, categorized by age and type.

```bash
# Overall disk usage
df -h ~

# Largest directories by category
du -sh ~/.npm ~/.cache ~/.openclaw/workspace ~/Documents/GitHub 2>/dev/null | sort -rh

# Repository sizes
cd ~/Documents/GitHub && for dir in */; do 
  du -sh "$dir" 2>/dev/null
done | sort -rh

# Build artifacts (largest by far)
find ~/Documents/GitHub -maxdepth 3 -type d \( -name ".next" -o -name "target" -o -name "build" -o -name "dist" \) -exec du -sh {} \; 2>/dev/null | sort -rh

# Git repositories by activity
cd ~/Documents/GitHub && find . -maxdepth 2 -name ".git" -type d | while read d; do
  repo=$(dirname "$d")
  commits=$(git -C "$repo" log --oneline --since='7 days ago' 2>/dev/null | wc -l)
  size=$(du -sh "$d" 2>/dev/null | cut -f1)
  echo "$commits commits | $size | $repo"
done | sort -rn
```

### 2. Categorize by Safety

**Safe to Remove (auto-regenerate):**
- `~/.npm/` — npm cache
- `~/.cache/zig`, `~/.cache/puppeteer`, `~/.cache/cmux`, etc. — dev tool caches
- `repo/.next/` — Next.js build (in stale repos, <1 commit/7 days)
- `repo/target/` — Rust build (in stale repos)
- `repo/build/`, `repo/dist/` — Build artifacts (in inactive repos)
- `repo/node_modules/` — Dependencies (in stale repos only)

**Dangerous (DO NOT REMOVE):**
- `.git/` directories (history loss)
- `node_modules/` in **active** repos
- Source code files
- `.env` or config files

### 3. Activity Check (Key Decision Point)

**For each candidate, check:**

```bash
# Last commit
git -C repo log --oneline --since='30 days ago' | wc -l

# Current status
git -C repo status

# Recent file modifications
find repo -type f -newer <7-day-marker> 2>/dev/null | wc -l
```

**Decision:**
- **>20 commits/month:** Keep all artifacts (likely active)
- **1-20 commits/month:** Safe to remove `.next`, `dist`, but keep `node_modules`
- **0 commits/month:** Safe to remove entirely or all build artifacts

### 4. Safe Cleanup Procedures

#### A. Clear npm Cache

```bash
# Size check
du -sh ~/.npm

# Clean
npm cache clean --force

# Or atomic remove
rm -rf ~/.npm && echo "✓ npm cache cleared"
```

**Impact:** ~10-15 GB freed. Auto-recreates on next `npm install`.

#### B. Clear Dev Tool Caches

```bash
# List
du -sh ~/.cache/* | sort -rh

# Remove unpopular ones
rm -rf ~/.cache/zig ~/.cache/puppeteer ~/.cache/cmux ~/.cache/uv ~/.cache/pre-commit

# Keep high-use ones
# - ~/.cache/codex-runtimes (active, >500 MB)
# - ~/.cache/node (frequent)
# - ~/.cache/gh (CLI)
```

**Impact:** ~2-3 GB freed. Caches regenerate on next use.

#### C. Remove Stale Build Artifacts

```bash
# Find candidates (no commits in 30 days)
cd ~/Documents/GitHub
for repo in */; do
  commits=$(git -C "$repo" log --oneline --since='30 days ago' 2>/dev/null | wc -l)
  if [ "$commits" -eq 0 ]; then
    echo "STALE: $repo"
    # Show size of build dirs
    du -sh "$repo/.next" "$repo/target" "$repo/build" 2>/dev/null | sort -rh
  fi
done

# Remove ONLY from confirmed stale repos
rm -rf repo-name/.next repo-name/target repo-name/build
```

**Impact:** Highly variable (50 MB - 10 GB depending on repo).

#### D. Remove Stale node_modules

```bash
# From inactive repos only (0 commits in 30 days)
cd ~/Documents/GitHub
for repo in */; do
  commits=$(git -C "$repo" log --oneline --since='30 days ago' 2>/dev/null | wc -l)
  if [ "$commits" -eq 0 ]; then
    if [ -d "$repo/node_modules" ]; then
      rm -rf "$repo/node_modules"
      echo "✓ Removed $repo/node_modules"
    fi
  fi
done
```

**Impact:** 500 MB - 2 GB per repo. Regenerates with `pnpm install`.

#### E. Archive or Remove Entire Stale Repos

```bash
# Only if repo hasn't been touched in 60+ days AND is not in active roadmap
git -C repo log --oneline | head -1  # Last commit info
rm -rf repo  # Only if truly obsolete

# Or keep but move to archive
mv repo ~/Archive/repo-name
```

**Impact:** Varies by repo size (10 MB - 100 MB typical).

---

## Decision Matrix

| Scenario | Action | Impact |
|----------|--------|--------|
| Disk >98%, urgent | Clear npm cache + dev caches | ~15 GB, immediate |
| Disk 95-97% | Remove stale `.next` / build artifacts | ~5-10 GB |
| Disk 90-95% | Remove stale `node_modules` | ~2-5 GB |
| Disk <90% | No action needed | — |
| Repo: 0 commits/60 days | Safe to archive/remove | ~50 MB - 5 GB |
| Repo: >5 commits/7 days | Keep all artifacts | — |

---

## Verification Steps

After cleanup, verify:

```bash
# 1. Check disk space
df -h ~

# 2. Verify active repos still have node_modules
cd ~/Documents/GitHub/OpenCoven/coven-dashboard
[ -d node_modules ] && echo "✓ node_modules present" || echo "❌ Missing!"

# 3. Test a build
cd ~/Documents/GitHub/OpenCoven/coven-dashboard
pnpm build --dry-run  # Or equivalent for the project

# 4. Confirm Git history is intact
git -C ~/Documents/GitHub/OpenCoven/coven-dashboard log --oneline | head -5
```

---

## Reporting

After cleanup, generate a report:

```markdown
# Disk Cleanup Report — $(date +%Y-%m-%d)

## Before & After
- Disk usage: XXX GB (XX%) → YYY GB (YY%)
- Freed: ZZ GB

## Cleanups Performed
- npm cache: XX GB
- Dev caches: YY GB
- Build artifacts: ZZ GB
- Stale repos: AA GB

## What Was Preserved
- All active repos: ✓
- All .git history: ✓
- Active node_modules: ✓
```

---

## Configuration

**Preserve List** (always keep):
- `~/Documents/GitHub/OpenCoven/**` (active ecosystem)
- `~/Documents/GitHub/OpenClaw/**` (core project)
- Any repo with >1 commit in past 7 days
- All `.git/` directories

**Safe to Remove**:
- Repos with 0 commits in 90+ days
- `.next` in repos with <1 commit/7 days
- `target/` in repos with <1 commit/7 days
- Dev tool caches (auto-regenerate)
- npm cache (auto-regenerate)

---

## Common Issues

### "I need this build artifact back"
**Solution:** Rebuild. All `.next`, `target`, `build`, `dist` directories auto-regenerate on `pnpm build`, `cargo build`, etc.

### "A cache was cleared but I need it"
**Solution:** Recreate. Next use of the tool (npm, zig, puppeteer, etc.) regenerates it.

### "I accidentally removed a repo's source code"
**Problem:** You removed the entire repo directory (not just artifacts). This skill does NOT do that.
**Solution:** Restore from Git remote: `git clone <repo-url>`

### "Cleanup didn't free much space"
**Diagnosis:** 
- Run `du -sh ~/Documents/GitHub/*` — if all repos are <500 MB, cleanup is done
- Check `df -h ~` — if >80% of space is system files (outside home), not applicable here
- Run full diagnostic again to find new hotspots

---

## Safety Principles

1. **Never delete source code** — only build artifacts and caches
2. **Always check activity first** — preserve anything recently touched
3. **Test after cleanup** — verify active projects still build
4. **Keep history** — all `.git` directories stay untouched
5. **Report what you did** — log every cleanup action for transparency

---

## Advanced: Scheduled Cleanup

To run cleanup automatically:

```bash
# Via cron (monthly)
0 2 1 * * ~/bin/disk-cleanup.sh >> ~/logs/disk-cleanup.log 2>&1

# Via cron (on-demand with threshold)
*/30 * * * * if [ $(df ~ | tail -1 | awk '{print $5}' | sed 's/%//') -gt 95 ]; then ~/bin/disk-cleanup.sh; fi
```

---

## Exit Codes

- `0` — Success, cleanup performed
- `1` — Error (invalid input, permission denied, etc.)
- `2` — No action needed (disk <90%)
- `3` — User abort (cleanup requires confirmation)

---

## Related Skills

- `git-maintenance` — Optimize `.git` directories
- `node-dependency-auditor` — Identify unused node_modules
- `docker-cleanup` — Clean Docker images/containers (if applicable)

---

**Last Updated:** 2026-05-15
**Tested On:** macOS (MB Black)
**Contributed By:** Nova
