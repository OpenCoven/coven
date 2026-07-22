---
title: "Coven documentation maintenance and public-docs rules"
description: "Maintenance rules for the public Coven docs: safe examples, canonical names, what to keep private, when to update pages, and how to handle stale content."
---

# Documentation Maintenance

These rules keep the public docs useful, accurate, and free of private material.

## Public docs stance

Docs in this repository are public product docs. They should describe OpenCoven and Coven without depending on private workspaces, private chats, private infrastructure, or unreleased assumptions.

Use examples that are safe to publish:

- `/path/to/project`
- `/Users/example/.coven/coven.sock`
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
[Local API](API.md)
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
