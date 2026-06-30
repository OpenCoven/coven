# Disk Space Optimizer Skill

Intelligently manage constrained disk space by removing build artifacts, caches, and stale repositories without affecting active development.

## Quick Start

```bash
# Analyze what can be cleaned
openclaw agent kitty --message "optimize disk analyze"

# See cleanup candidates
openclaw agent kitty --message "optimize disk candidates"

# Perform cleanup
openclaw agent kitty --message "optimize disk cleanup"

# Get current status
openclaw agent kitty --message "optimize disk report"
```

## What It Does

✅ **Safely Removes:**
- npm cache (~10-15 GB)
- dev tool caches (~2-3 GB)
- Build artifacts from stale repos (.next, target, build, dist)
- node_modules from repos inactive 30+ days

❌ **Never Touches:**
- Active project source code
- `.git/` directories (preserves history)
- node_modules in active repos
- Config/env files

## How It Works

1. **Analyzes** disk usage, categorizes by repository activity
2. **Identifies** candidates for cleanup (repos with 0 commits in 30+ days)
3. **Removes** only safe artifacts from confirmed stale repos
4. **Reports** what was freed and what remains

## Usage Examples

### Analyze Current State
```bash
kitty optimize disk analyze
```
Shows all space consumers, repository activity, and cleanup candidates.

### See What Can Be Removed
```bash
kitty optimize disk candidates [--days 30]
```
Lists stale repos and their removable artifacts (no user action required).

### Perform Cleanup
```bash
kitty optimize disk cleanup
```
Removes identified artifacts (asks for confirmation first).

### Get Status Report
```bash
kitty optimize disk report
```
Shows current disk usage and active repositories.

## Safety Guarantees

- **Activity-Based:** Only removes artifacts from repos with 0 commits in 30+ days
- **Regenerable:** All removed items auto-recreate on next use (npm install, cargo build, etc.)
- **Reversible:** Source code and git history always preserved
- **Transparent:** Reports exactly what was removed and why

## Typical Results

- **Before:** 99% full (5% available)
- **After:** 96% full (36% available)
- **Freed:** 20-30 GB typically
- **Time:** ~5-10 minutes (depends on disk I/O)

## What Gets Cleaned

| Item | Size | Removed When | Regenerates |
|------|------|-----|---|
| npm cache | 10-15 GB | Always | Next `npm install` |
| Dev caches | 2-3 GB | Selective | On next tool use |
| `.next` builds | 50-600 MB | Stale repos only | Next `pnpm build` |
| `target/` (Rust) | 100 MB - 5 GB | Stale repos only | Next `cargo build` |
| `node_modules` | 500 MB - 2 GB | Stale repos only | Next `pnpm install` |

## Advanced

### Manual Script Use
```bash
# Run script directly
~/.openclaw/workspace/skills/disk-space-optimizer/bin/disk-space-optimizer analyze
~/.openclaw/workspace/skills/disk-space-optimizer/bin/disk-space-optimizer cleanup
~/.openclaw/workspace/skills/disk-space-optimizer/bin/disk-space-optimizer candidates
```

### Configure Automation
```bash
# Add to launchd/cron to run monthly
0 2 1 * * ~/.openclaw/workspace/skills/disk-space-optimizer/bin/disk-space-optimizer cleanup
```

## When to Use

- **Disk >97%:** Run cleanup immediately
- **Disk 90-97%:** Can wait, but good to run preemptively
- **Disk <90%:** No action needed
- **After archiving a repo:** Always run cleanup

## Need Help?

Check the full SKILL.md for:
- Decision matrix for cleanup safety
- Troubleshooting common issues
- Configuration options
- Verification steps
- Related skills

---

**Integrated with:** Kitty (code agent), Nova (main assistant)  
**Last Updated:** 2026-05-15  
**Status:** Production-ready
