# OpenCoven Orchestration — Quick Reference

## The Idea

Multiple harnesses on one task. Automatic handoffs when a harness hits a blocker.

---

## 60-Second Example

```bash
# Create a multi-harness task
coven task create "fix the auth bug" --harnesses openclaw,claude-code

# Monitor
coven task status <task-id>

# OpenClaw explores → finds blocker → hands off to Claude Code
# Claude Code edits file → completes

# See the result
coven task result <task-id>
```

That's it. Handoffs happen automatically.

---

## Key Commands

| Goal | Command |
|------|---------|
| **Create task** | `coven task create "<goal>" --harnesses h1,h2` |
| **Check status** | `coven task status <id>` |
| **View log** | `coven task log <id>` |
| **See handoffs** | `coven task trace <id>` |
| **Get result** | `coven task result <id>` |
| **Pause work** | `coven task pause <id>` |
| **Resume work** | `coven task resume <id>` |
| **Manual handoff** | `coven task handoff <id> --to claude-code` |
| **List tasks** | `coven task list` |

---

## Harnesses

| Harness | Best For |
|---------|----------|
| `openclaw` | Exploration, testing, refactoring, complex reasoning |
| `claude-code` | File editing, implementation, syntax |
| `codex` | Implementation, code generation |
| `hermes` | Research, pattern finding (future) |

---

## Common Workflows

### Explore → Edit
```bash
coven task create "fix the parser" --harnesses openclaw,claude-code
```
OpenClaw explores, Claude Code edits.

### Explore → Edit → Test
```bash
coven task create "implement feature" --harnesses openclaw,claude-code,openclaw
```
Explore → Implement → Verify loop.

### Code Review Chain
```bash
coven task create "security review" --harnesses openclaw,codex,claude-code
```
Multiple perspectives, each harness reviews in turn.

---

## What Transfers During Handoff

✅ Findings and blockers  
✅ Code diffs and file state  
✅ Previous decisions  
✅ Error messages  
✅ Context and analysis  

❌ Harness credentials (each harness uses its own)  
❌ Private configuration  

---

## Status Values

| Status | Meaning |
|--------|---------|
| `pending` | Waiting for harness to start |
| `running` | Harness is working |
| `handoff-pending` | Ready to hand off |
| `handoff-executing` | Mid-handoff, context transferring |
| `paused` | Manually paused |
| `completed` | Task done |
| `failed` | Harness failed or timeout |

---

## Troubleshooting

**Task not starting?**
```bash
coven doctor  # Check harness availability
```

**Handoff failed?**
```bash
coven task log <id> --last 10  # See the error
```

**Lost context?**
```bash
coven handoff show <handoff-id> --context  # View what transferred
```

**Manual recovery?**
```bash
coven task pause <id>
coven task handoff <id> --to openclaw  # Try different harness
```

---

## Configuration

Create `~/.coven/orchestration.toml`:

```toml
[harnesses]
default = ["openclaw", "claude-code"]

[harnesses.openclaw]
priority = 1
timeout_minutes = 30

[harnesses.claude-code]
priority = 2
timeout_minutes = 45
```

Then use defaults:
```bash
coven task create "fix bug"  # Uses configured harnesses
```

---

## Pro Tips

✨ **Exploration first.** Let OpenClaw/Codex explore, then hand off to specialists.  
✨ **Short timeouts.** Set `--timeout-minutes` to prevent runaway tasks.  
✨ **Check the trace.** Use `coven task trace <id>` to see what each harness did.  
✨ **Use titles.** Make tasks searchable: `--title "Auth refactor"`.  
✨ **Export audits.** `coven task export <id> --format json` for records.  

---

## More Info

- **Full guide:** [USER-GUIDE-ORCHESTRATION.md](USER-GUIDE-ORCHESTRATION.md)
- **Architecture:** [ARCHITECTURE.md](ARCHITECTURE.md)
- **API details:** [API-CONTRACT.md](API-CONTRACT.md)
- **Glossary:** [GLOSSARY.md](GLOSSARY.md)

---

**Start:** `coven task create "fix something" --harnesses openclaw,claude-code`
