# Coven Skills

Canonical Coven-wide skills live here. Each skill is a directory with a `SKILL.md` and any supporting scripts.

## Convention

- **Source of truth:** `coven/skills/<skill-name>/`
- **Harness consumers:** symlink from `<repo>/.agents/skills/`, `<repo>/.warp/skills/`, or `~/.openclaw/workspace/<familiar>/skills/`
- **Never duplicate:** edit the canonical source only; symlinks pick up changes automatically

## Skills

| Skill | Purpose |
|-------|---------|
| `coven-board-entry` | Create a new entry on the Coven task board programmatically |
| `coven-task-manager` | Keep Coven task boards fresh with scheduled stale/blocked/review task hygiene |
| `opencoven-design` | OpenCoven design system and visual language reference |

## Adding a new Coven skill

1. Create `coven/skills/<skill-name>/SKILL.md` + implementation files
2. Symlink into each harness that needs it:
   ```bash
   ln -s $(pwd)/skills/<skill-name> /path/to/harness/skills/<skill-name>
   ```
3. Document the symlinks in the skill's `SKILL.md`
