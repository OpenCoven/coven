---
name: project-manager
description: Manage project-scoped knowledge bases with notes, decisions, and bookmarks. Use when the user wants to create/manage projects, add/view/search notes or decisions for a project, manage bookmarks (add, list, tag to projects), switch project context for conversation, or search across projects. Triggers on phrases like "project", "bookmark", "add a note to", "let's talk about [project]", "switch to [project]", "list projects", "search projects".
---

# Project Manager

Manage project-scoped context in `projects/` within the workspace.

## File Structure

```
projects/
├── _bookmarks.json         ← global bookmarks store
├── <project-slug>/
│   ├── CONTEXT.md          ← project overview, status, key info
│   └── notes/
│       └── YYYY-MM-DD.md   ← dated notes (append-style)
```

## Bookmarks

Stored in `projects/_bookmarks.json` as an array:

```json
[
  {
    "id": "b1",
    "url": "https://example.com",
    "title": "Example Site",
    "description": "Optional description",
    "projects": ["project-slug"],
    "added": "2026-02-08"
  }
]
```

- Bookmarks exist independently and can be tied to **zero or more** projects
- Use short incremental IDs: b1, b2, b3...
- When adding, auto-fetch the page title if not provided

## Actions

### Create project
1. Slugify the name (lowercase, hyphens)
2. Create `projects/<slug>/CONTEXT.md` with project name, description, and any initial info
3. Create `projects/<slug>/notes/` directory

### Add note to project
1. Append to `projects/<slug>/notes/YYYY-MM-DD.md` (today's date)
2. Use `## HH:MM` headers for multiple entries per day
3. If the note contains a decision, prefix with **Decision:**

### Add bookmark
1. Read `projects/_bookmarks.json`
2. Generate next ID (b1, b2, ...)
3. Append new bookmark with optional project tags
4. Write back

### Link bookmark to project
1. Find bookmark by URL or ID
2. Add project slug to its `projects` array

### Switch context / Chat about project
1. Read `projects/<slug>/CONTEXT.md`
2. Read recent notes: `projects/<slug>/notes/` (last 3 files)
3. Read bookmarks tagged to this project from `_bookmarks.json`
4. Use this context to inform conversation

### List projects
1. List directories in `projects/` (excluding files starting with `_`)
2. Show name + first line of CONTEXT.md for each

### Search across projects
1. Use `grep -ri` across `projects/` for text search
2. Report which project(s) and file(s) matched

### List bookmarks
1. Read `_bookmarks.json`
2. Optionally filter by project tag
3. Display as a clean list with titles and URLs

## Context Loading

When chatting about a project, always load:
1. `CONTEXT.md` — the full project overview
2. Latest 3 note files — recent activity
3. Tagged bookmarks — relevant links

Keep context loading efficient. Only read what's needed for the conversation.
