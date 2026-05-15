# OpenCoven Brand System

The canonical OpenCoven brand system lives in [`DESIGN.md`](https://github.com/OpenCoven/coven/blob/main/DESIGN.md). Implementation assets live in [`brand/`](https://github.com/OpenCoven/coven/tree/main/brand).

## Core rule

OpenCoven should feel like **collective intelligence + controlled power**: arcane but precise, technical not gimmicky, powerful not loud, minimal but symbolic.

## Positioning

OpenCoven is an open ecosystem for persistent AI familiars: named agents with memory, tools, identity, roles, and continuity.

Most AI today feels temporary. You open a chat, explain your context, get a response, and start over. OpenCoven is built around a different future: AI that can **stay**.

OpenCoven gives builders a way to create durable AI systems that remember what matters, understand their purpose, use tools, collaborate with other agents, and remain understandable over time. Each familiar can have a name, a voice, a memory, a toolset, a role, and a place in a larger workflow.

The philosophy is simple: AI should be powerful without becoming opaque, personal without pretending to be human, and extensible without collapsing into chaos. OpenCoven brings structure to the magic through memory, identity, orchestration, local execution, tool access, and multi-agent collaboration.

Use this as the high-level brand promise:

> OpenCoven turns AI from a blank chatbox into a living workspace of agents that remember, coordinate, and belong to you.

## Brand asset pack

| Asset | Purpose |
| --- | --- |
| [`brand/logo/opencoven-logo.svg`](https://github.com/OpenCoven/coven/blob/main/brand/logo/opencoven-logo.svg) | Full-gradient primary logo for hero and social use |
| [`brand/logo/opencoven-mark.svg`](https://github.com/OpenCoven/coven/blob/main/brand/logo/opencoven-mark.svg) | Mark-only vector |
| [`brand/logo/opencoven-white.svg`](https://github.com/OpenCoven/coven/blob/main/brand/logo/opencoven-white.svg) | Solid white logo for small dark surfaces |
| [`brand/logo/opencoven-black.svg`](https://github.com/OpenCoven/coven/blob/main/brand/logo/opencoven-black.svg) | Solid black logo for light surfaces |
| [`brand/logo/opencoven-monoline.svg`](https://github.com/OpenCoven/coven/blob/main/brand/logo/opencoven-monoline.svg) | Technical diagrams and docs |
| [`brand/ui/color-tokens.css`](https://github.com/OpenCoven/coven/blob/main/brand/ui/color-tokens.css) | Canonical color tokens |
| [`brand/ui/typography.css`](https://github.com/OpenCoven/coven/blob/main/brand/ui/typography.css) | Canonical font stacks and tracking |
| [`brand/social/opencoven-og.png`](https://github.com/OpenCoven/coven/blob/main/brand/social/opencoven-og.png) | Social preview / OG image |
| [`brand/docs/BRAND-USAGE.md`](https://github.com/OpenCoven/coven/blob/main/brand/docs/BRAND-USAGE.md) | Contributor-facing usage checklist |

## Legacy raster icon pack

The existing raster icon pack remains available in [`assets/opencoven`](https://github.com/OpenCoven/coven/tree/main/assets/opencoven) for package README compatibility and platform slots. Treat `brand/logo` as canonical for new vector work.

## Package copies

The npm package READMEs use package-local copies of `opencoven.svg` so package previews do not depend on files outside the package tarball:

- [`packages/cli/assets/opencoven.svg`](https://github.com/OpenCoven/coven/blob/main/packages/cli/assets/opencoven.svg)
- [`packages/openclaw-coven/assets/opencoven.svg`](https://github.com/OpenCoven/coven/blob/main/packages/openclaw-coven/assets/opencoven.svg)

Keep those copies in sync with [`assets/opencoven/opencoven.svg`](https://github.com/OpenCoven/coven/blob/main/assets/opencoven/opencoven.svg).
