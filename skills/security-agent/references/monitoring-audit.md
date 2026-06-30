# Monitoring & Continuous Audit Reference

## Scheduled Audit Strategy

### Recommended Cron Jobs

| Job Name | Schedule | Command/Payload | Purpose |
|----------|----------|----------------|---------|
| `security-agent:weekly-audit` | Sunday 3:00 AM local | `openclaw security audit --deep --json` | Catch config drift |
| `security-agent:daily-version` | Daily 6:00 AM local | `openclaw update status` | Catch available security updates |

### Cron Setup Pattern

Before creating, always check for existing jobs:
```
cron list → search for name match → update if exists, create if not
```

Use `sessionTarget: "isolated"` with `payload.kind: "agentTurn"` for audit jobs.
Set delivery mode to `announce` so results reach the operator.

### Weekly Audit Job Template
```json
{
  "name": "security-agent:weekly-audit",
  "schedule": { "kind": "cron", "expr": "0 3 * * 0", "tz": "America/Chicago" },
  "payload": {
    "kind": "agentTurn",
    "message": "Run openclaw security audit --deep --json. Parse the JSON output. If any CRITICAL or new WARN findings exist, summarize them clearly. If all clear, report 'Security audit clean — no new findings.' Always include the finding count summary."
  },
  "delivery": { "mode": "announce" },
  "sessionTarget": "isolated"
}
```

### Daily Version Check Template
```json
{
  "name": "security-agent:daily-version",
  "schedule": { "kind": "cron", "expr": "0 6 * * *", "tz": "America/Chicago" },
  "payload": {
    "kind": "agentTurn",
    "message": "Run openclaw update status. If an update is available, notify that a new version is ready. If up to date, reply HEARTBEAT_OK."
  },
  "delivery": { "mode": "announce" },
  "sessionTarget": "isolated"
}
```

## Log & Memory Hygiene

### Secrets in Logs
Periodically scan memory/log files for accidentally persisted secrets:
```bash
# Count potential secret patterns in memory files (never display contents)
grep -rlc -E '(Bearer |token[=:]|apikey|secret[=:]|password[=:])' ~/.openclaw/workspace/memory/ 2>/dev/null
```

### Session Audit
Use `sessions_list` + `sessions_history` to review:
- Unknown or unexpected session sources.
- Unusual tool usage patterns (bulk exec, bulk file reads).
- Sessions from unrecognized channels or senders.

### Memory File Review
- Check `memory/*.md` files for accidentally stored credentials.
- Check `MEMORY.md` for infrastructure URLs that should be redacted.
- Verify `TOOLS.md` doesn't contain plaintext secrets.

## Anomaly Patterns to Watch

| Pattern | Indication | Response |
|---------|-----------|----------|
| Unknown sender in session list | Possible unauthorized access | Verify pairing, check channel policies |
| Burst of exec commands | Possible injection attack | Review session history, check for prompt injection |
| File reads outside workspace | Possible data exfiltration attempt | Review sandbox policy |
| Unusual web_fetch targets | Possible callback/exfiltration | Review what triggered the fetches |
| New skills installed | Supply chain risk | Verify skill source and contents |

## Post-Incident Checklist

After any security event:
1. [ ] Rotate all exposed credentials.
2. [ ] Review session logs for full scope.
3. [ ] Re-run `openclaw security audit --deep`.
4. [ ] Check for config modifications.
5. [ ] Verify channel policies unchanged.
6. [ ] Document in `memory/YYYY-MM-DD.md` (redacted).
7. [ ] Update MEMORY.md with lessons learned.
