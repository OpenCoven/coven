# Retention checklist

Use this when user asks to clean up memory or during periodic maintenance.

## Labels

- `keep`: durable, high-signal, likely to matter later
- `review`: unclear value, revisit later
- `archive`: historical context, low day-to-day value
- `delete-candidate`: likely noise/duplicate, remove only with confirmation

## Review flow

1. Scan recent daily files (`memory/YYYY-MM-DD.md`).
2. Extract durable facts (preferences, recurring meetings, decisions, active projects).
3. Promote durable facts into `MEMORY.md` with concise wording.
4. Mark low-signal entries with `archive` or `delete-candidate`.
5. Ask before deleting anything.

## Safe prune policy (default)

- Never hard-delete without explicit user approval.
- Prefer moving content to an archive section/file first.
- Keep recent notes untouched unless requested.
