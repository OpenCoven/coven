# Tool & Exec Policy Reference

## Sandbox Modes

| Mode | Behavior | Risk |
|------|----------|------|
| `all` | All tool calls sandboxed | **Lowest** — recommended for shared/exposed agents |
| `elevated` | Only elevated commands sandboxed | Medium |
| `off` | No sandboxing | **Highest** — acceptable only for single-operator personal assistant |

For personal assistant model with one trusted operator: `off` is acceptable but documented as higher risk.
For any multi-user or exposed setup: `all` is required.

## Filesystem Policy

### `workspaceOnly`
- `true`: Agent can only read/write within its workspace directory.
- `false`: Agent has full filesystem access.
- **Personal assistant**: `false` is common (agent needs to access repos, configs, etc.).
- **Shared agent**: Must be `true`.

### Workspace isolation
- Verify workspace path is set and doesn't overlap with sensitive system directories.
- Agent should not have write access to `~/.openclaw/openclaw.json` via normal operations.

## Exec Approval Policy

| Setting | Behavior |
|---------|----------|
| `ask` | Agent must request approval for each exec command |
| `allowlist` | Pre-approved command patterns; others blocked |
| `bypass` | No approval required — **dangerous for exposed agents** |

### Allowlist patterns
- Should be specific: `["git *", "pnpm *", "openclaw *"]` not `["*"]`.
- Review periodically for stale/overly-broad patterns.
- Never allowlist `rm -rf`, `sudo`, or `curl | sh` patterns.

## Browser Control

When browser control is enabled:
- Agent can navigate, click, fill forms, evaluate JavaScript.
- **Risk**: If agent is steerable by untrusted input, browser actions can be induced.
- **Mitigation**: Ensure browser profile doesn't have sensitive logged-in sessions.
- **2FA recommendation**: All important accounts should have hardware-key 2FA.

## Elevated Commands

- Review what's in the elevated allowlist.
- Elevated commands run with host privileges — minimize this surface.
- Prefer specific commands over broad patterns.

## Node Command Security

- Verify `nodes.run` command allowlists per paired node.
- Node commands execute on remote paired devices — highest-privilege capability.
- Require approval gates for node commands.
- Audit declared vs actually-needed commands.

## Audit Procedure

1. `gateway config.get` → extract `agents.defaults` section.
2. Check sandbox mode, fs policy, exec policy.
3. List enabled tool categories.
4. Cross-reference tool exposure with channel exposure.
5. Flag: exposed channel + powerful tools + no sandbox = critical risk.
