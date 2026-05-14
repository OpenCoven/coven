# OpenCoven Orchestration — Common Workflows

Step-by-step guides for real-world multi-harness scenarios.

---

## Workflow 1: Fix a Failing Test

**Goal:** Use OpenClaw to diagnose, Claude Code to fix.

### Setup

```bash
cd your-project
coven daemon start
```

### Run

```bash
# Create the task
coven task create "fix the failing auth test" \
  --harnesses openclaw,claude-code \
  --title "Auth test fix" \
  --priority high

# Note the task ID (e.g., auth-test-abc123)
```

### Monitor Phase 1 (Exploration)

```bash
# Wait 2-3 seconds for OpenClaw to start
coven task status <task-id>

# Check OpenClaw's findings
coven task log <task-id> --from-harness openclaw
```

Expected output:
```
OpenClaw analysis:
- Test fails on line 42
- Auth token not being refreshed
- Refresh mechanism is missing from TokenManager
```

### Monitor Phase 2 (Handoff)

```bash
# Watch for handoff
coven task status <task-id>
# Should show: "handoff-executing" → "Status: claude-code (active)"

# View handoff details
coven handoff show <last-handoff-id>
```

### Monitor Phase 3 (Implementation)

```bash
# Check Claude Code's work
coven task log <task-id> --from-harness claude-code

# Expected: Claude Code edits TokenManager, adds refresh logic, runs test
```

### Get Result

```bash
coven task result <task-id>

# Expected:
# ✅ Test now passes
# Changes:
#   - src/token-manager.ts: Added refresh logic (12 lines)
#   - tests/auth.test.ts: Updated (1 line)
```

---

## Workflow 2: Implement a Feature

**Goal:** Explore existing patterns, then implement, then verify.

### Setup

```bash
coven daemon start
cd your-project
```

### Create Task

```bash
coven task create "implement user profile page" \
  --harnesses openclaw,claude-code,openclaw \
  --title "User profile feature" \
  --priority high \
  --timeout-minutes 120
```

### Phase 1: Exploration (OpenClaw)

```bash
# Monitor
coven task log <task-id> --from-harness openclaw | head -50
```

Expected findings:
```
- Existing profile pages: settings.tsx, dashboard.tsx
- Profile route: /profile/:id
- Database schema for users in User table
- API endpoint already exists: GET /api/users/:id
- Recommend: Create new ProfilePage component, use existing API
```

### Phase 2: Implementation (Claude Code)

```bash
# Check when handoff happens
coven task status <task-id>
# Should show: "to: claude-code"

# Monitor implementation
coven task log <task-id> --from-harness claude-code
```

Expected work:
```
Claude Code:
- Creates ProfilePage.tsx component
- Fetches user data from /api/users/:id
- Renders profile with photo, bio, stats
- Adds responsive styling
- 145 lines of code
```

### Phase 3: Verification (OpenClaw)

```bash
# Check when second OpenClaw starts
coven task status <task-id>
# Should show: "to: openclaw (phase 2)"

# Monitor tests
coven task log <task-id> --from-harness openclaw | tail -50
```

Expected:
```
OpenClaw:
- Adds ProfilePage.test.tsx
- Tests: renders profile, fetches data, handles errors
- Runs existing test suite
- All tests pass ✅
```

### Final Result

```bash
coven task result <task-id>

# Expected:
# ✅ Feature complete
# Files created: ProfilePage.tsx, ProfilePage.test.tsx
# Tests: 12 new, all passing
# Lines of code: 200
# Time: 45 minutes
```

---

## Workflow 3: Code Review Chain

**Goal:** Multiple harnesses review the same code from different angles.

### Setup

```bash
# Create a feature branch first
git checkout -b feature/rate-limiting
# ... make some changes ...
git add .

coven daemon start
cd your-project
```

### Create Task

```bash
coven task create "review rate limiting implementation" \
  --harnesses openclaw,codex,claude-code \
  --title "Rate limiting code review" \
  --priority medium
```

### Phase 1: OpenClaw Review

```bash
coven task log <task-id> --from-harness openclaw
```

Expected findings:
```
OpenClaw Review (Patterns & Architecture):
- Rate limiting middleware is well-placed
- Uses Redis for state (good choice)
- Complexity: moderate, clear
- Suggestion: Move Redis client to singleton
```

### Phase 2: Codex Review

```bash
coven task log <task-id> --from-harness codex
```

Expected findings:
```
Codex Review (Security & Performance):
- Rate limits are reasonable (1000 req/min)
- No timing attack vulnerability
- Memory efficient
- Warning: Test with high concurrency
```

### Phase 3: Claude Code Review

```bash
coven task log <task-id> --from-harness claude-code
```

Expected findings:
```
Claude Code Review (Implementation):
- Error handling is good
- TypeScript types are correct
- Logging is adequate
- Suggestion: Add JSDoc comments
- Suggestion: Use const instead of let (1 place)
```

### Combine Findings

```bash
coven task result <task-id> --full

# Expected: Combined review with all three perspectives
```

---

## Workflow 4: Refactor a Module

**Goal:** Understand existing code, plan changes, implement, test.

### Setup

```bash
coven daemon start
cd your-project
```

### Create Task

```bash
coven task create "refactor authentication module to reduce complexity" \
  --harnesses openclaw,openclaw,claude-code,openclaw \
  --title "Auth module refactor" \
  --timeout-minutes 180 \
  --priority high
```

### Phase 1: Analysis (OpenClaw)

```bash
coven task log <task-id> --step 1
```

OpenClaw:
- Maps existing auth code
- Identifies complexity hotspots
- Suggests refactoring approach (extract functions, separate concerns)

### Phase 2: Dry Run (OpenClaw)

```bash
coven task log <task-id> --step 2
```

OpenClaw:
- Plans the refactor
- Creates TODO list
- Estimates 40 lines reduction

### Phase 3: Implementation (Claude Code)

```bash
coven task log <task-id> --step 3
```

Claude Code:
- Executes the refactor
- Extracts helper functions
- Improves naming
- Adds JSDoc

### Phase 4: Verification (OpenClaw)

```bash
coven task log <task-id> --step 4
```

OpenClaw:
- Runs test suite
- Compares complexity metrics (before/after)
- Confirms no regressions
- Validates refactoring goals met

---

## Workflow 5: Debug Complex Issue

**Goal:** When one harness gets stuck, hand off to specialist.

### Setup

```bash
# Issue: Race condition in database writes
coven daemon start
cd your-project
```

### Create Task

```bash
coven task create "fix race condition in user creation" \
  --harnesses openclaw \
  --title "Race condition debug"
```

### Phase 1: Initial Investigation

```bash
coven task status <task-id>

# OpenClaw works for a while...
# Maybe 10-15 minutes...

coven task log <task-id>
# Output: "Complex concurrency issue. May need specialized approach."
```

### Phase 2: Manual Handoff to Specialist

```bash
# When OpenClaw hits a wall, manually hand off
coven task handoff <task-id> --to claude-code \
  --reason "Need implementation-level concurrency expertise"

# Or use Codex:
coven task handoff <task-id> --to codex \
  --reason "Research race condition patterns in databases"
```

### Phase 3: Specialist Works

```bash
# Claude Code or Codex continues with full context
coven task log <task-id>

# Expected: Uses promises, locks, or transactions to fix the issue
```

### Result

```bash
coven task result <task-id>

# Race condition fixed with proper locking mechanism
```

---

## Workflow 6: Documentation Sprint

**Goal:** One harness explores, another writes docs.

### Create Task

```bash
coven task create "document the API endpoints" \
  --harnesses openclaw,claude-code \
  --title "API documentation" \
  --priority medium
```

### Phase 1: Analysis (OpenClaw)

```bash
coven task log <task-id> --from-harness openclaw

# Expected:
# - Maps all endpoints
# - Extracts parameters, response types
# - Finds examples
```

### Phase 2: Writing (Claude Code)

```bash
coven task log <task-id> --from-harness claude-code

# Expected:
# - Writes API.md
# - Formats as OpenAPI spec
# - Adds usage examples
# - Adds error codes
```

---

## Monitoring Across All Tasks

### See all active orchestration tasks

```bash
coven task list --status running
coven task list --status handoff
```

### Export metrics

```bash
coven task metrics --from 2026-05-14 --to 2026-05-15

# Output:
# Total tasks: 23
# Completed: 19
# Failed: 1
# Avg time: 34 minutes
# Avg handoffs per task: 1.8
# Most common pair: openclaw → claude-code
```

### Build a dashboard

```bash
coven task list --json | jq '.[] | {id, title, status, time_minutes}'
```

---

## Tips for Each Workflow Type

### Exploration + Implementation
✅ Start with OpenClaw or Codex  
✅ Hand off to Claude Code for editing  
✅ Minimal context loss  

### Multi-Expert Review
✅ Put explorers first  
✅ Put security specialists second  
✅ Put implementation experts last  

### Iterative Refactoring
✅ Analyze → Plan → Implement → Verify loop works  
✅ Multiple OpenClaw passes is fine  

### Recovery from Blocker
✅ Pause, analyze, then manually hand off  
✅ Check context to understand what blocked  

---

## Common Issues & Fixes

| Issue | Solution |
|-------|----------|
| Handoff takes too long | Reduce context size, check network |
| Harness unavailable | Run `coven doctor`, check harness is installed |
| Task stuck | `coven task pause`, then manually `coven task handoff` |
| Wrong harness order | Check `~/.coven/orchestration.toml` or use `--harnesses` explicitly |
| No output from harness | `coven task log <id> --last 50` to see last messages |

---

## More Resources

- [USER-GUIDE-ORCHESTRATION.md](USER-GUIDE-ORCHESTRATION.md) — Full guide
- [ORCHESTRATION-QUICK-REFERENCE.md](ORCHESTRATION-QUICK-REFERENCE.md) — Command reference
- [GETTING-STARTED.md](GETTING-STARTED.md) — General Coven setup
- [ARCHITECTURE.md](ARCHITECTURE.md) — How it works internally

---

**Ready?** Pick a workflow above that matches your need, follow the steps, and let the harnesses coordinate.
