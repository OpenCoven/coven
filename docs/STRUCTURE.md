---
title: "Coven docs structure, navigation, and source layout"
description: "How the Coven docs are organized: source layout under docs/, public navigation tabs driven by docs.json, excluded content, and the Mintlify-compatible build."
---

# Coven Documentation Structure

The docs app is Mintlify-compatible, but the local build is a small static renderer driven by `docs.json`.

## Source Layout

```text
docs/
├── docs.json                  # Public navigation and site metadata
├── index.md                   # Public landing page
├── GETTING-STARTED.md         # Public setup guide
├── ARCHITECTURE.md            # Runtime topology
├── API.md / API-CONTRACT.md   # Local socket API docs
├── start/                     # Small public start pages
├── reference/                 # Curated lookup pages
├── scripts/docs-site/         # Build, source index, smoke test
├── assets/                    # OpenCoven docs assets
└── ...                        # Additional repo-local docs and drafts
```

## Public Navigation

`docs.json` is the source of truth for the public app. The build scripts render and index only pages listed there.

Current public sections:

| Tab | Scope |
| --- | --- |
| Start | Install, first run, TUI, troubleshooting, concepts, roadmap |
| Runtime | Architecture, operational model, safety/auth, session lifecycle, harness adapter docs |
| Integrate | Local API, API contract, CastCodes integration, advanced clients, comux migration reference |
| Reference | CLI/API lookup, glossary, product spec |

## Excluded Content

Do not re-add broad generic docs tabs unless the pages are implemented and accurate.

Excluded categories include:

- old generated agent/profile docs;
- generic guides/examples/resources pages;
- `Stub - fill in` scaffold pages;
- future orchestration command guides;
- maintainer-only plans, verification notes, and scaffolding scripts.

## Build Commands

From `docs/`:

```sh
npm run docs:build
npm run docs:smoke
npm run docs:check
```

`docs:check` builds the curated app, creates `search-index.json`, runs Pagefind over `dist/docs-site`, and verifies that unrelated scaffold pages were not emitted.
