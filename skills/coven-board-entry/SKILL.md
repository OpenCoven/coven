# Skill: coven-board-entry

**Purpose:** Create a new task on the Coven task board programmatically. Any familiar can use this without knowing task service internals.

## Input

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `title` | string | required | Task title |
| `description` | string | optional | Full description |
| `status` | TaskStatus | `"inbox"` | inbox / started / completed / blocked |
| `priority` | TaskPriority | `"medium"` | critical / high / medium / low |
| `familiar` | string | optional | Familiar slug (e.g. `"echo"`) |
| `project` | string | optional | Project name (e.g. `"CovenCave"`) |
| `tags` | string[] | optional | Array of tags |

## Output

Returns the created `CovenTask` object as JSON.

## How it works

1. **Try gateway API first** — POST to `http://localhost:3000/api/gateway/tasks` with task payload  
2. **Fallback** — If gateway unavailable or fails, write to `~/.openclaw/coven-tasks-pending.json` for later sync

## Usage (from a familiar / subagent)

```bash
# Direct call via the skill script
node /Users/buns/.openclaw/workspace/echo/skills/coven-board-entry/create-task.mjs \
  --title "My new task" \
  --description "Details here" \
  --priority high \
  --familiar echo \
  --project CovenCave \
  --tags memory,ui
```

Or via the JS API (ESM):

```js
import { createCovenTask } from './create-task.mjs';

const task = await createCovenTask({
  title: "My task",
  priority: "high",
  familiar: "echo",
  project: "CovenCave",
  tags: ["memory"]
});
console.log(task);
```

## Notes

- IDs are auto-generated as `<familiar>-<slug>-<timestamp>` or `task-<uuid>`
- `createdAt` / `updatedAt` are set automatically to current UTC time
- `createdBy` defaults to `familiar` if provided, else `"unknown"`
- Pending tasks file: `~/.openclaw/coven-tasks-pending.json`

## Location & Symlink Convention

**Canonical source:** `OpenCoven/coven/skills/coven-board-entry/`

This skill is symlinked into harnesses and familiar workspaces from the canonical location. Do not edit copies — edit the source.

**Active symlinks:**
- `~/.openclaw/workspace/echo/skills/coven-board-entry` → canonical
- `cast-codes/.agents/skills/coven-board-entry` → canonical (Codex/Claude harness)
- `cast-codes/.warp/skills/coven-board-entry` → canonical (Warp harness)
- `~/.openclaw/skills/coven-board-entry` → canonical (global OpenClaw)

**Adding a new harness symlink:**
```bash
ln -s /Users/buns/Documents/GitHub/OpenCoven/coven/skills/coven-board-entry \
  /path/to/harness/skills/coven-board-entry
```
