# OpenCoven Orchestration — User Guide

**Use multiple harnesses seamlessly on a single task.**

This guide takes you from "I want to use two harnesses together" to a working multi-harness workflow.

---

## What Orchestration Does

Instead of:
- "Use OpenClaw to explore this, then switch to Claude Code to edit the file"
- Manual copy-paste of context between harnesses
- Separate session history for each harness

You now get:
- **Smart handoffs:** "Task this to the best harness"
- **Context transfer:** Full state passes automatically
- **Unified history:** See the whole conversation across harnesses
- **User choice:** You pick which harnesses participate

---

## The Promise

> One task. Any combination of harnesses. Transparent coordination.

---

## Quick Start (5 minutes)

### Prerequisites

- Coven daemon running
- At least 2 harnesses installed (e.g., OpenClaw + Claude Code)
- A project with code to work on

### Run a multi-harness task

```bash
# Start the daemon
coven daemon start

# Launch a task that can hand off
coven task create "fix the failing auth test" \
  --harnesses openclaw,claude-code \
  --priority high
```

Coven will:
1. Send to OpenClaw first (default order)
2. If OpenClaw hits a blocker, hand off to Claude Code
3. Claude Code completes the work
4. You see the full trace: OpenClaw → Claude Code

### Check progress

```bash
coven task status <task-id>
coven task log <task-id>          # See full conversation
coven task trace <task-id>        # See handoff events
```

### View handoff details

```bash
coven handoff show <handoff-id>   # See context that transferred
```

---

## Core Concepts

### Harness
An external coding-agent CLI (OpenClaw, Claude Code, Codex, Hermes, etc.) that Coven coordinates.

You choose which harnesses to use. We support any harness with an adapter.

### Handoff
Explicit transfer of a task from one harness to another.

When Harness A can't proceed, it hands off to Harness B with full context:
- What was done
- What blockers were hit
- What decisions were made
- Full code diffs and findings

### Task
A unit of work that can span multiple harnesses.

Example: "fix the auth test"
- OpenClaw explores, finds the issue
- Claude Code makes the fix
- Both efforts tracked in one task

### Context
The complete state passed during handoff.

Includes:
- Previous findings and work
- Code state (diffs, modified files)
- Error messages and blockers
- Decisions made so far
- Required capabilities for next harness

---

## Workflows

### Single Harness (Existing Behavior)

```bash
coven run openclaw "fix the tests"
```

Works exactly as before. No changes needed.

---

### Two-Harness Workflow (New)

**Scenario:** "Explore with OpenClaw, then edit with Claude Code"

```bash
# Create a task that allows handoff
coven task create "fix the parser bug" \
  --harnesses openclaw,claude-code \
  --title "Parser debugging"

# Check progress
coven task status <task-id>

# When ready, retrieve the result
coven task result <task-id>
```

**What happens:**
1. OpenClaw starts, explores the parser issue
2. OpenClaw finds the bug, makes notes
3. OpenClaw determines it needs file editing
4. OpenClaw hands off to Claude Code with full context
5. Claude Code receives the findings
6. Claude Code edits the file, adds test
7. Task completes
8. You see: OpenClaw's findings + Claude Code's fix in one trace

---

### Three+ Harness Workflow (Advanced)

```bash
coven task create "refactor the auth module" \
  --harnesses openclaw,claude-code,codex \
  --priority medium
```

Coven tries harnesses in order. When one reaches a natural stopping point, it hands off to the next.

---

### Manual Handoff (Explicit Control)

If you want precise control:

```bash
# Get the result so far
coven task pause <task-id>

# Get the context
coven handoff prep <task-id>

# Manually hand off to the next harness
coven task handoff <task-id> \
  --to claude-code \
  --reason "Need file editing expertise"
```

---

## CLI Commands

### Create a multi-harness task

```bash
coven task create "<goal>" \
  --harnesses <h1,h2,h3> \
  --priority <low|medium|high> \
  --title "<readable title>" \
  --timeout-minutes 60
```

Examples:
```bash
# Two harnesses, exploring then editing
coven task create "fix the race condition" \
  --harnesses openclaw,claude-code

# Three harnesses, complex workflow
coven task create "refactor the database layer" \
  --harnesses openclaw,claude-code,codex \
  --priority high \
  --timeout-minutes 120

# Single harness (traditional)
coven task create "update the docs" \
  --harnesses openclaw
```

### Check task status

```bash
coven task status <task-id>              # One-line status
coven task status <task-id> --json       # Machine-readable
```

Response:
```
Task: fix-auth-test-abc123
Status: handoff-executing
Started: 2026-05-14 14:30:00 UTC
Current harness: claude-code
Previous: openclaw (completed)
Next: (none)
ETA: 12 minutes
```

### View task log

```bash
coven task log <task-id>                 # Full conversation
coven task log <task-id> --from-harness  # Only from one harness
coven task log <task-id> --format json   # Structured log
```

### View handoff events

```bash
coven task trace <task-id>               # All handoff events
coven handoff show <handoff-id>          # Details of one handoff
coven handoff show <handoff-id> --context  # Context that transferred
```

### Pause and resume

```bash
coven task pause <task-id>               # Stop work, keep state
coven task resume <task-id>              # Resume from pause
```

### Manual handoff

```bash
coven task handoff <task-id> --to claude-code
coven task handoff <task-id> --to codex --reason "Need Codex's capabilities"
```

### Retrieve results

```bash
coven task result <task-id>              # Final result
coven task result <task-id> --full       # Full trace + result
```

---

## Understanding Handoffs

### What Transfers

When Harness A hands off to Harness B, Harness B receives:

```json
{
  "from_harness": "openclaw",
  "to_harness": "claude-code",
  "task_id": "fix-auth-test",
  "context": {
    "findings": "Auth token expires after 1 hour. Cause: missing refresh mechanism.",
    "files_examined": ["src/auth.ts", "src/token.ts"],
    "blockers": "Need to edit token.ts to add refresh logic",
    "code_state": { /* full diffs */ },
    "decisions": "Will add background refresh task"
  },
  "required_capabilities": ["file_editing", "testing"],
  "deadline_minutes": 45
}
```

Harness B starts with full context. No copy-paste needed.

### What Happens Next

Harness B:
1. Receives context from Harness A
2. Validates it has required capabilities
3. Confirms understanding
4. Continues from where A left off
5. Either completes or hands off again

All interactions are logged to the audit trail.

---

## Common Workflows

### Workflow: Explore → Edit → Test

Goal: Complex feature with exploration, editing, and testing phases.

```bash
coven task create "implement rate limiting" \
  --harnesses openclaw,claude-code,openclaw \
  --priority high
```

1. **OpenClaw (Phase 1):** Explore existing code, find where to add rate limiting
2. **Claude Code (Phase 2):** Edit, add rate limiting middleware
3. **OpenClaw (Phase 3):** Add tests, verify the implementation

---

### Workflow: Research → Implement

Goal: Research existing patterns, then implement.

```bash
coven task create "add caching layer to API" \
  --harnesses openclaw,claude-code \
  --timeout-minutes 120
```

1. **OpenClaw:** Research Redis integration patterns, existing cache code
2. **Claude Code:** Implement Redis integration based on findings

---

### Workflow: Code Review Chain

Goal: Multiple expert reviews.

```bash
coven task create "security audit of auth module" \
  --harnesses codex,openclaw,claude-code
```

Each harness reviews from a different perspective:
1. **Codex:** First pass (general patterns)
2. **OpenClaw:** Security-specific review
3. **Claude Code:** Implementation best practices

Each hands off with findings.

---

## Troubleshooting

### Handoff failed

```bash
coven task log <task-id> --last 20        # See the failure
coven handoff show <handoff-id> --error   # Error details
```

Common causes:
- Target harness offline
- Context too large (>10MB)
- Target harness lacks required capabilities
- Timeout exceeded

### Context lost during handoff

All context is logged. Retrieve it:

```bash
coven handoff show <handoff-id> --context  # See transferred context
coven task log <task-id> --format json     # Full audit trail
```

### Task stuck

Pause and check state:

```bash
coven task pause <task-id>
coven task status <task-id>
coven task log <task-id>
```

Then either resume or manually hand off:

```bash
coven task handoff <task-id> --to openclaw --reason "Try different approach"
```

### Harness unavailable

If a harness goes offline mid-task:

```bash
coven task status <task-id>          # Shows: "pending_harness_recovery"
```

Coven waits 30 seconds, then either:
- Tries the harness again
- Fails (if deadline exceeded)
- Hands off to next harness (if available)

---

## Examples

### Example 1: Fix a Failing Test

```bash
# Create the task
coven task create "fix the auth token test" \
  --harnesses openclaw,claude-code \
  --title "Token expiry test"

# Monitor
coven task status <task-id>

# After 5 minutes...
# Task: token-expiry-test-abc
# Status: handoff-executing
# From: openclaw (completed)
# To: claude-code (active)

# Check OpenClaw's findings
coven task log <task-id> --from-harness openclaw

# Result after 15 minutes total
coven task result <task-id>

# Output:
# ✅ Test fixed
# Changes:
#   - src/auth.ts: Added token refresh (5 lines)
#   - tests/auth.test.ts: Updated test (3 lines)
```

### Example 2: Implement a Feature

```bash
# Create multi-phase task
coven task create "add user profile page" \
  --harnesses openclaw,claude-code,openclaw \
  --priority high \
  --timeout-minutes 180

# OpenClaw explores existing profile patterns
# Claude Code implements the page
# OpenClaw adds tests and verifies

coven task result <task-id>

# See how each harness contributed
coven task trace <task-id>
```

### Example 3: Security Audit

```bash
# Multiple harnesses, different expertise
coven task create "audit password handling" \
  --harnesses openclaw,codex,claude-code

# OpenClaw: finds security patterns
# Codex: reviews for known vulnerabilities
# Claude Code: suggests improvements

coven task result <task-id>  # See combined audit
```

---

## Advanced: Configure Harness Preferences

Create `~/.coven/orchestration.toml`:

```toml
[harnesses]
default = ["openclaw", "claude-code"]

[harnesses.openclaw]
priority = 1
capabilities = ["exploration", "testing", "refactoring"]
timeout_minutes = 30

[harnesses.claude-code]
priority = 2
capabilities = ["file_editing", "implementation"]
timeout_minutes = 45

[harnesses.codex]
priority = 3
capabilities = ["research", "implementation"]
timeout_minutes = 60

[handoff]
max_context_size_mb = 10
timeout_minutes = 120
allow_manual_override = true
log_all_transfers = true
```

Then use defaults:

```bash
coven task create "implement feature"  # Uses configured harnesses
```

---

## Tips & Best Practices

### ✅ Do

- **Start with exploration.** Let OpenClaw or Codex explore first.
- **Hand off to specialists.** Let Claude Code handle file editing.
- **Chain workflows.** Explore → Edit → Test is natural.
- **Check the trace.** Understand what each harness did.
- **Set timeouts.** Prevent tasks from running forever.
- **Use titles.** Make tasks easy to find later.

### ❌ Don't

- **Expect magic.** Orchestration improves coordination, not capability.
- **Ignore blockers.** If a handoff fails, read the error.
- **Chain too many harnesses.** 3-4 is practical; 10 is chaos.
- **Large contexts.** If context > 5MB, something's wrong.
- **Manual copy-paste.** That's what orchestration prevents!

---

## Monitoring & Observability

### Query all active tasks

```bash
coven task list              # All tasks
coven task list --status running  # Running
coven task list --status handoff  # In handoff
coven task list --json       # Machine-readable
```

### Get task metrics

```bash
coven task metrics <task-id>

# Output:
# Task: fix-auth-test
# Total time: 18 minutes
# OpenClaw: 8 minutes (exploration)
# Handoff: 0.2 minutes (context transfer)
# Claude Code: 10 minutes (editing + testing)
# Context transferred: 240 KB
```

### Export audit trail

```bash
coven task export <task-id> --format json > audit.json
coven task export <task-id> --format markdown > report.md
```

---

## What's Next?

- [ARCHITECTURE.md](ARCHITECTURE.md) — How orchestration works internally
- [API-CONTRACT.md](API-CONTRACT.md) — HTTP API for programmatic access
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — More detailed diagnostics

---

**Questions?** See [GETTING-STARTED.md](GETTING-STARTED.md) for general Coven setup, or check [GLOSSARY.md](GLOSSARY.md) for terminology.
