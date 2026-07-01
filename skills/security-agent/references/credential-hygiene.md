# Credential Hygiene Reference

## Principles

- **No plaintext secrets in config files.** Use `op://` references (1Password) or environment variables from a secret manager.
- **No secrets in chat output.** Ever. Not even partially. Not even "redacted after showing."
- **No secrets in memory files, commits, or PR descriptions.**
- **No infrastructure URLs** (gateway URL, Tailscale Funnel URL) in any output.

## Common Plaintext Locations to Scan

| Location | What to Check |
|----------|--------------|
| `~/.openclaw/openclaw.json` | `gateway.auth.token`, channel bot tokens, API keys |
| `~/.openclaw/identity/` | Device identity files (should exist but never display) |
| Environment variables | `GH_TOKEN`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc. |
| Shell history | `~/.zsh_history`, `~/.bash_history` — leaked tokens in commands |
| `.env` files | Project-level environment files |
| Git config | `~/.gitconfig` — tokens in remote URLs |

## Migration to 1Password (`op://`)

### Check if 1Password CLI is available
```bash
op --version 2>/dev/null && echo "available" || echo "not installed"
op account list 2>/dev/null | head -3
```

### Migration pattern
1. Store the secret in 1Password vault.
2. Replace plaintext value with `op://<vault>/<item>/<field>` reference.
3. Ensure the process that reads the config supports `op run` injection.
4. Verify the old plaintext value is fully removed (not just commented out).

### What OpenClaw supports
- Check if OpenClaw config supports `op://` references natively for the field in question.
- If not, use `op run` wrapper or environment variable injection.
- Gateway auth tokens: may need to remain in config — protect with file permissions instead.

## File Permission Checks

```bash
# Config file should be owner-readable only
ls -la ~/.openclaw/openclaw.json
# Expected: -rw------- or -rw-r----- (600 or 640)

# Identity directory
ls -la ~/.openclaw/identity/
# Expected: drwx------ (700)
```

## Shell History Scan

```bash
# Check for common token patterns in history (existence only, never display)
grep -c -E '(token|key|secret|password)=' ~/.zsh_history 2>/dev/null
grep -c -E 'Bearer [A-Za-z0-9]' ~/.zsh_history 2>/dev/null
```

Never display the actual matched lines — only counts.

## Rotation Checklist

When rotating credentials:
1. Generate new credential.
2. Update all locations that reference the old credential.
3. Verify functionality with new credential.
4. Revoke old credential.
5. Confirm old credential no longer works.
6. Update any stored references (1Password, env files).
