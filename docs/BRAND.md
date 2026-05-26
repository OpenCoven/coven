---
summary: "Canonical OpenCoven brand rules, asset locations, package logo copies, and product positioning for Coven docs and public surfaces."
read_when:
  - Using OpenCoven brand assets
  - Checking Coven docs and package copy against the brand system
title: "OpenCoven brand system"
description: "Canonical OpenCoven brand rules, asset locations, package logo copies, and product positioning for Coven docs and public surfaces."
---

# OpenCoven Brand System

The canonical OpenCoven brand system lives in [`DESIGN.md`](https://github.com/OpenCoven/coven/blob/main/DESIGN.md). Implementation assets live in [`brand/`](https://github.com/OpenCoven/coven/tree/main/brand).

## Core rule

OpenCoven should feel like **collective intelligence + controlled power**: arcane but precise, technical not gimmicky, powerful not loud, minimal but symbolic.

## Positioning

OpenCoven is the umbrella behind CastCodes and Coven. CastCodes is the local-first workspace users open; Coven is the local runtime that gives agent work project boundaries, durable session records, and explicit authority.

The broader OpenCoven brand still points toward persistent AI familiars: named agents with memory, tools, identity, roles, and continuity. CastCodes is the primary place that story should become visible.

Most AI today feels temporary. You open a chat, explain your context, get a response, and start over. OpenCoven is built around a different future: AI that can **stay**.

OpenCoven gives builders a way to create durable AI systems that remember what matters, understand their purpose, use tools, collaborate with other agents, and remain understandable over time. Each familiar can have a name, a voice, a memory, a toolset, a role, and a place in a larger workflow.

The philosophy is simple: AI should be powerful without becoming opaque, personal without pretending to be human, and extensible without collapsing into chaos. OpenCoven brings structure to the magic through memory, identity, orchestration, local execution, tool access, and multi-agent collaboration.

Use this as the high-level umbrella promise:

> OpenCoven turns AI from a blank chatbox into a living workspace of agents that remember, coordinate, and belong to you.

Use this as the product/runtime framing:

> CastCodes is the product users open. Coven is the runtime that powers it.

## Approved logo

Use the black-background logo with the white icon on every public docs, README, package, and site surface. Do not swap in gradient, mark-only, black-only, monoline, or external avatar images.

| Asset | Purpose |
| --- | --- |
| [`docs/assets/opencoven-icon.svg`](https://github.com/OpenCoven/coven/blob/main/docs/assets/opencoven-icon.svg) | Public docs logo, favicon, and generated docs metadata |
| [`assets/opencoven/opencoven.svg`](https://github.com/OpenCoven/coven/blob/main/assets/opencoven/opencoven.svg) | Root README and shared package-logo source |
| [`brand/ui/color-tokens.css`](https://github.com/OpenCoven/coven/blob/main/brand/ui/color-tokens.css) | Canonical color tokens |
| [`brand/ui/typography.css`](https://github.com/OpenCoven/coven/blob/main/brand/ui/typography.css) | Canonical font stacks and tracking |
| [`brand/social/opencoven-og.png`](https://github.com/OpenCoven/coven/blob/main/brand/social/opencoven-og.png) | Social preview image; regenerate it when the approved logo changes |
| [`brand/docs/BRAND-USAGE.md`](https://github.com/OpenCoven/coven/blob/main/brand/docs/BRAND-USAGE.md) | Contributor-facing usage checklist |

## Raster icon pack

The existing raster icon pack remains available in [`assets/opencoven`](https://github.com/OpenCoven/coven/tree/main/assets/opencoven) for platform slots. The raster exports should depict the same black-background, white-icon logo.

## Package copies

The npm package READMEs use package-local copies of `opencoven.svg` so package previews do not depend on files outside the package tarball:

- [`packages/cli/assets/opencoven.svg`](https://github.com/OpenCoven/coven/blob/main/packages/cli/assets/opencoven.svg)
- [`packages/openclaw-coven/assets/opencoven.svg`](https://github.com/OpenCoven/coven/blob/main/packages/openclaw-coven/assets/opencoven.svg)

Keep those copies in sync with [`assets/opencoven/opencoven.svg`](https://github.com/OpenCoven/coven/blob/main/assets/opencoven/opencoven.svg).
