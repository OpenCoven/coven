---
name: prompt-vault
description: Save, search, tag, and manage reusable AI prompts via the `pv` CLI. Local SQLite-backed prompt library with version history, tag filtering, and JSON export/import.
---

# Prompt Vault

Local-first prompt manager. Use when asked to save a prompt, find a saved prompt, list prompts by tag, or export/import the prompt library.

## Prerequisites

- `pv` CLI installed: `npm install -g BunsDev/prompt-vault`
- Data stored at `~/.prompt-vault.db` (SQLite)

## Commands

### Save a prompt
```bash
pv add "prompt text here" --tags tag1,tag2
```
- Tags are comma-separated, no spaces
- Returns the prompt ID

### Search prompts
```bash
pv search "keyword"
```
- Substring match on prompt text

### List prompts
```bash
pv list              # all prompts, newest first
pv list --tag code   # filter by tag
```

### Export library
```bash
pv export            # JSON to stdout
pv export > prompts.json
```

## Workflow Patterns

### Save a prompt the user liked
When the user says "save that prompt" or "remember this prompt":
1. Identify the prompt text from conversation context
2. Ask for tags if not obvious (or infer from context)
3. `pv add "<text>" --tags <tags>`

### Find a prompt for reuse
When the user says "find my prompt about X" or "use that refactor prompt":
1. `pv search "X"` to find matches
2. Present the best match
3. Use it in the current task

### Suggest tags
Common useful tags: `system-prompt`, `code-review`, `refactor`, `summary`, `debug`, `explain`, `test`, `docs`, `creative`, `agent`, `tool-use`

## Notes

- Prompt text with quotes: escape inner quotes or use single quotes around the CLI arg
- The DB is local-only — no sync. Use `pv export` to back up.
- Search is basic substring matching (LIKE %query%)
