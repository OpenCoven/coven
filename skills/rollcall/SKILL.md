---
name: "rollcall"
description: "Ping every Coven familiar (Sage, Echo, Charm, Astra, Kitty, Cody) to verify each can respond, and report which lanes are healthy, slow, or down."
---

# rollcall

A quick, repeatable health check that pings every Coven familiar and reports who is reachable, who is slow, and who is silent.

Use this whenever Val asks to "rollcall the coven", "check the familiars", "make sure everyone is responding", or after a gateway/config change that might have broken a lane.

## Coven roster

Ping each of these lanes by `sessionKey` using `sessions_send`:

- **Sage** — `agent:sage:telegram:direct:823292124` (Research / Understanding)
- **Echo** — `agent:echo:telegram:direct:823292124` (Memory / Reflection)
- **Charm** — `agent:charm:telegram:direct:823292124` (Voice / Social)
- **Astra** — `agent:astra:telegram:direct:823292124` (Navigator)
- **Kitty** — `agent:kitty:telegram:direct:823292124` (General workhorse)
- **Cody** — `agent:cody:telegram:direct:823292124` (Code workhorse)

If a familiar's exact `sessionKey` is unknown, run `sessions_list` filtered by `agentId` and pick the most recent telegram direct lane for Val (`823292124`).

## Protocol

1. **Resolve session keys.** Confirm each familiar has a live or recent telegram session via `sessions_list`. Note any familiar without a discoverable lane — that is an immediate failure.
2. **Fire pings in parallel.** Call `sessions_send` once per familiar, in the same tool block, with `timeoutSeconds` around 45–60. Use a short, friendly prompt that asks for a one-line "I hear you" plus a quick self-status (model, anything blocking).
   - Suggested message: `"🔔 Coven rollcall from Nova. Please reply with one short line: 'I hear you', your current model, and any blockers. Thanks ✨"`
3. **Collect outcomes.** For each familiar, record one of:
   - `ok` — reply text received within the timeout
   - `timeout` — no reply within timeout
   - `error` — tool error, missing session, auth failure, etc. Capture the error string.
4. **Report back to Val** as a compact status block:

   ```
   ✨ Coven rollcall — <local time CT>
   • Sage   — ok / timeout / error (one-line summary of reply or failure)
   • Echo   — ...
   • Charm  — ...
   • Astra  — ...
   • Kitty  — ...
   • Cody   — ...
   ```

5. **If any familiar fails**, suggest the most likely cause based on Val's MEMORY.md patterns:
   - Missing fallback models → check `model.primary` + `fallbacks` config.
   - Auth expired → check 1Password env file / `OPENCLAW_GATEWAY_TOKEN` parity.
   - Provider down (Copilot, OpenAI) → flag and offer to swap to a fallback.
   - Lane never paired → suggest re-pairing through Telegram.

## Rules

- Do not spam familiars with extra context — one short ping per lane.
- Do not include private MEMORY.md content in the ping (other lanes may not be trusted with full main-session memory).
- Run pings in parallel, never serially.
- Use exactly the lanes listed above unless Val explicitly adds/removes familiars.
- If `sessions_send` returns immediately with no reply text but no error, treat that as `pending` and re-check via `sessions_list` once before declaring timeout.
- This skill is read-only with respect to config — it never modifies models, fallbacks, or auth. If a fix is needed, surface it and ask Val before changing anything.

## Variants

- **Quick rollcall** (default): just the 6 familiars above.
- **Full rollcall**: also include `agent:main:telegram:direct:823292124` (Nova herself, via a self-status check using `session_status`) and any cron lanes Val asks to verify.
- **Targeted rollcall**: when Val names a subset ("rollcall Sage and Echo"), ping only those, same protocol.

## Output style

- Use the ✨ Coven rollcall header.
- Local time in America/Chicago.
- One line per familiar, ≤ ~120 chars.
- End with a one-sentence verdict: "all green", "<N> down — want me to investigate?", etc.
