# OpenCoven Brand System

The canonical OpenCoven brand system lives in [`../DESIGN.md`](../DESIGN.md). Implementation assets live in [`../brand`](../brand).

## Core rule

OpenCoven should feel like **collective intelligence + controlled power**: arcane but precise, technical not gimmicky, powerful not loud, minimal but symbolic.

## Brand asset pack

| Asset | Purpose |
| --- | --- |
| [`brand/logo/opencoven-logo.svg`](../brand/logo/opencoven-logo.svg) | Full-gradient primary logo for hero and social use |
| [`brand/logo/opencoven-mark.svg`](../brand/logo/opencoven-mark.svg) | Mark-only vector |
| [`brand/logo/opencoven-white.svg`](../brand/logo/opencoven-white.svg) | Solid white logo for small dark surfaces |
| [`brand/logo/opencoven-black.svg`](../brand/logo/opencoven-black.svg) | Solid black logo for light surfaces |
| [`brand/logo/opencoven-monoline.svg`](../brand/logo/opencoven-monoline.svg) | Technical diagrams and docs |
| [`brand/ui/color-tokens.css`](../brand/ui/color-tokens.css) | Canonical color tokens |
| [`brand/ui/typography.css`](../brand/ui/typography.css) | Canonical font stacks and tracking |
| [`brand/social/opencoven-og.png`](../brand/social/opencoven-og.png) | Social preview / OG image |
| [`brand/docs/BRAND-USAGE.md`](../brand/docs/BRAND-USAGE.md) | Contributor-facing usage checklist |

## Legacy raster icon pack

The existing raster icon pack remains available in [`assets/opencoven`](../assets/opencoven) for package README compatibility and platform slots. Treat `brand/logo` as canonical for new vector work.

## Package copies

The npm package READMEs use package-local copies of `opencoven.svg` so package previews do not depend on files outside the package tarball:

- [`packages/cli/assets/opencoven.svg`](../packages/cli/assets/opencoven.svg)
- [`packages/openclaw-coven/assets/opencoven.svg`](../packages/openclaw-coven/assets/opencoven.svg)

Keep those copies in sync with [`assets/opencoven/opencoven-128.png`](../assets/opencoven/opencoven-128.png) until package READMEs move to SVG rendering.
