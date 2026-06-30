# Channel & Sender Policy Reference

## DM Policies

| Policy | Behavior | Risk |
|--------|----------|------|
| `pairing` | Requires pairing approval before DM access | **Lowest** — recommended default |
| `allowlist` | Only pre-approved sender IDs | Low — static but secure |
| `open` | Anyone can DM the agent | **High** — use only for public-facing bots with minimal tools |

**Rule**: Never use `dmPolicy: "open"` on an agent with exec/filesystem/browser tools.

## Group Policies

| Policy | Behavior | Risk |
|--------|----------|------|
| `allowlist` | Only explicitly listed groups | **Lowest** — recommended |
| `pairing` | Groups must be approved | Low |
| `open` | Any group can interact | **High** |

## `allowFrom` Verification

- Each channel should have explicit `allowFrom` sender IDs.
- Cross-reference with known/trusted user IDs.
- Telegram: numeric user IDs (e.g., `823292124`).
- Discord: snowflake user IDs.
- iMessage: phone numbers or email addresses.

## Audit Checklist

1. List all enabled channels from config.
2. For each channel, verify:
   - `dmPolicy` is `pairing` or `allowlist`
   - `groupPolicy` is `allowlist`
   - `allowFrom` contains only known user IDs
   - Group IDs in allowlists are verified
3. Flag any channel with `open` policy + tool access.
4. Check for channels enabled but not actively used (reduce surface).

## Cross-Channel Consistency

- All channels should use the same minimum policy level.
- If Telegram is `pairing` but Discord is `open`, that's inconsistent — flag it.
- Prefer uniform `pairing` + `allowlist` across all channels.

## Group-Specific Settings

- `requireMention: true` in groups reduces accidental triggering.
- Topic-level overrides (`"*": { "requireMention": false }`) expand exposure — use deliberately.
- Verify that `requireMention: false` groups are intentional.
