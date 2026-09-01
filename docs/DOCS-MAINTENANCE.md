---
title: "Coven documentation maintenance and public-docs rules"
description: "Maintenance rules for the public Coven docs: safe examples, canonical names, what to keep private, when to update pages, and how to handle stale content."
---

# Documentation Maintenance

These rules keep repository documentation accurate and define the boundary
between this source repository and the canonical public documentation site.

## Documentation ownership

Public user documentation is canonical in
[`OpenCoven/coven-docs`](https://github.com/OpenCoven/coven-docs) and published
at [docs.opencoven.ai](https://docs.opencoven.ai/).

Keep documentation in this repository only when it must evolve with the code:

- normative API, adapter, lifecycle, or authority-boundary contracts;
- contributor, security, release, and repository policy;
- maintainer/development source maps and verification procedures;
- implementation specs, plans, design records, and historical notes;
- package- or crate-specific READMEs.

Installation, onboarding, CLI usage, daemon operation, harness setup, public API
guides, memory guides, and troubleshooting belong in `coven-docs`. Repository
entry points should link to the canonical page instead of copying its content.

When moving a topic:

1. Add any missing current behavior to `coven-docs`.
2. Verify the canonical page and stable URL.
3. Replace the repository copy with a short pointer or remove it after updating
   inbound links.
4. Keep normative details here only when the public page links back to the
   source contract.

## Public-doc directory boundary

The repository's public-doc directories may contain only two kinds of pages:

- **Canonical pointers** — a stable repository entry point whose body links to
  the canonical `docs.opencoven.ai` route. Use this shape:

  ```md
  ---
  title: "<existing page title>"
  description: "Pointer to the canonical <topic> guidance."
  ---

  Canonical <topic> guidance: **https://docs.opencoven.ai/docs/<route>**

  <optional one-line note retaining a source-adjacent contract link>
  ```

- **Source-adjacent exceptions** — a page that must evolve with the code
  (contracts, maintainer source maps, verification procedures). Every retained
  page changed after this policy landed states its ownership reason in
  frontmatter:

  ```yaml
  source_adjacent_reason: "Tracks the daemon API implemented in this repository."
  ```

  The ownership guard intentionally applies to changed pages. Historical
  public guidance remains migration debt until its canonical target is
  verified; touching one requires converting it to a pointer or declaring a
  truthful source-adjacent reason.

Public-doc directories today: `docs/install/`, `docs/platforms/`,
`docs/start/`, `docs/help/`, `docs/harnesses/`, `docs/models/`,
`docs/memory/`, `docs/guides/`, `docs/reference/`, and the public operation
pages of `docs/daemon/`. Source-adjacent trees (`docs/design/`,
`docs/development/`, `docs/superpowers/`, `docs/architecture/`,
`docs/security/`) and the top-level normative contracts are exempt.

Do not add a new public page to these directories, and do not restore
duplicated prose. If a canonical target is missing, the local page stays
unchanged until the canonical coverage lands in `coven-docs` — topical
similarity alone is not duplication, and an absent canonical target blocks
removal, never forces a rewrite here.

Public user guidance pages that remain because their canonical target is still
pending (for example the platform pages retained until
`scripts/onboarding-docs-test.mjs` is migrated to canonical-pointer
expectations) are listed as pending exceptions in the tracking issue, not
silently kept.

## Public content stance

All committed documentation is public. It should describe OpenCoven and Coven
without depending on private workspaces, private chats, private
infrastructure, or unreleased assumptions.

Use examples that are safe to publish:

- `/path/to/project`
- `~/.coven/coven.sock`
- `session-1`
- `intent-1`
- `https://github.com/OpenCoven/coven`

Do not include:

- private usernames unless they are already public project handles;
- personal chat excerpts;
- local absolute paths from a maintainer machine;
- tokens, keys, cookies, or credential names;
- private hostnames;
- private repo URLs;
- real session ids from a private machine;
- raw environment dumps;
- screenshots containing private data.

## Canonical names

- Ecosystem/org: **OpenCoven**
- Runtime/daemon/CLI: **Coven**
- Command: `coven`
- CLI package: `@opencoven/cli`
- OpenClaw plugin package: external OpenClaw bridge plugin
- Discord: `discord.gg/opencoven`
- X / Twitter: `@OpenCvn`

## When to update docs

Update docs in the same change when you modify:

- CLI commands or flags;
- daemon lifecycle behavior;
- session record shape;
- event record shape;
- socket API response fields;
- harness support;
- project-root or cwd policy;
- archive/summon/sacrifice behavior;
- release package names;
- security or secret-handling rules.

## Required checks

For docs-only changes:

```sh
python3 scripts/check-docs-ownership-test.py
python3 scripts/check-docs-ownership.py --range origin/main...HEAD
python scripts/check-secrets.py
git diff --check
```

For docs plus code:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
git diff --check
```

## Version-sensitive claims

Avoid claiming a package is "latest" unless you have just verified the registry or release source. Prefer stable phrasing:

- "The npm wrapper packages are published for early adopters."
- "As of this documentation pass, ..."
- "Check the registry before publishing release notes."

## Links

Prefer relative repo links for internal docs:

```md
[API contract](API-CONTRACT.md)
```

Use full URLs only for external resources and public community links.

## Diagrams

Mermaid diagrams are allowed. Keep them small enough to read in GitHub's Markdown renderer.

When a diagram is normative, mirror the important rule in prose nearby. A diagram alone is not a contract.

## Private research and planning notes

Private planning notes can inform docs, but do not paste them directly. Convert them into public, general product language and remove:

- names of private operators;
- personal memory details;
- non-public project state;
- machine-specific paths;
- credentials or token references;
- internal-only commitments.

Public docs should describe the product, not the private circumstances that produced the product.
