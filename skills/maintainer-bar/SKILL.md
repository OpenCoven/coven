---
name: maintainer-bar
description: >
  Operate the MaintainerBar SwiftUI menu bar app and its ghcrawl backend.
  Use for: refreshing issue clusters, searching issues semantically,
  checking repo pulse, managing notifications, building/running the app,
  and coordinating ghcrawl data pipelines.
tags: [maintainer, github, menu-bar, ghcrawl, macos, swiftui]
---

# MaintainerBar — Maintainer Menu Bar Skill

## Overview

MaintainerBar is a native macOS SwiftUI menu bar app for GitHub repo maintenance.
It's backed by **ghcrawl** (local SQLite + OpenAI embeddings) for issue/PR clustering
and semantic search.

**Repo:** `~/Documents/GitHub/OpenKnots/MaintainerBar`
**Stack:** SwiftUI (Swift Package Manager), macOS 14+
**Backend:** ghcrawl CLI (JSON mode) + GitHub API via `gh`
**Host:** MB Black (macOS node)

## Prerequisites

- ghcrawl installed: `~/.nvm/versions/node/v24.13.0/bin/ghcrawl`
- Wrapper: `~/.local/bin/ghcrawl-op` (injects GitHub token via `gh auth` + OpenAI key via 1Password)
- Xcode 26+ / Swift 6.0+
- `gh` CLI authenticated
- `op` CLI (1Password) for OpenAI key

## Build & Run

```bash
# Build
cd ~/Documents/GitHub/OpenKnots/MaintainerBar
swift build

# Run (debug)
swift run MaintainerBar

# Run built binary directly
.build/debug/MaintainerBar
```

## ghcrawl Operations (via wrapper)

All ghcrawl commands should use the `ghcrawl-op` wrapper or inject env vars:

```bash
# Refresh a repo (sync + embed + cluster)
~/.local/bin/ghcrawl-op refresh openclaw/openclaw

# List clusters (JSON)
~/.local/bin/ghcrawl-op clusters openclaw/openclaw --min-size 2 --limit 30 --json

# Semantic search
~/.local/bin/ghcrawl-op search openclaw/openclaw --query "download stalls" --json

# Cluster detail
~/.local/bin/ghcrawl-op cluster-detail openclaw/openclaw --id 123 --json

# Specific threads
~/.local/bin/ghcrawl-op threads openclaw/openclaw --numbers 42,43,44 --json

# Author's open issues
~/.local/bin/ghcrawl-op author openclaw/openclaw --login someone --json

# Health check
~/.local/bin/ghcrawl-op doctor --json
```

## Architecture

```
Sources/
├── MaintainerBarApp.swift      # @main entry, MenuBarExtra
├── Models/
│   ├── AppState.swift          # @MainActor ObservableObject
│   └── Models.swift            # IssueCluster, SearchResult, etc.
├── Services/
│   └── GhcrawlService.swift    # Actor wrapping ghcrawl CLI calls
└── Views/
    ├── MenuBarView.swift       # Main container with tabs
    ├── ClustersView.swift      # Issue cluster browser
    ├── SearchView.swift        # Semantic search
    ├── NotificationsView.swift # GitHub notifications (WIP)
    └── PulseView.swift         # Repo health dashboard (WIP)
```

## Key Behaviors

- **Secrets never stored on disk** — GitHub token from `gh auth token`, OpenAI key from `op read`
- **All ghcrawl calls use `--json`** for machine-readable output
- **Cluster members link to GitHub** — clicking opens in browser
- **Refresh is explicit** — user triggers via button (no auto-polling yet)

## Planned Features

- [ ] GitHub notifications via `gh api /notifications`
- [ ] PR dashboard with CI status
- [ ] AI-generated summaries (daily/weekly pulse)
- [ ] Keyboard command palette (⌘K)
- [ ] Bulk triage actions (label, close duplicates)
- [ ] Background auto-refresh on interval
- [ ] Native macOS notifications for review requests
- [ ] Multi-repo support
- [ ] Sparkline trends in Pulse view

## Troubleshooting

```bash
# Check ghcrawl health
~/.local/bin/ghcrawl-op doctor --json

# Check if refresh is running
ps aux | grep ghcrawl

# Check local DB
ls -la ~/.config/ghcrawl/ghcrawl.db

# Manual cluster inspect (TUI)
~/.local/bin/ghcrawl-op tui openclaw/openclaw
```

## Run Commands on MB Black

All build/run/ghcrawl commands must run on the macOS node (MB Black):
```
exec(host="node", command="...")
```
