---
title: "OpenCoven branding adherence checklist and progress"
description: "Tracks OpenCoven brand adherence across landing, docs, and packages: design tokens, logo usage, surface checklists, current progress, exceptions, and risks."
---

# Branding Adherence Checklist, Progress, Exceptions, and Risks

**Status:** In progress
**Source of truth:** [`DESIGN.md`](https://github.com/OpenCoven/coven/blob/main/DESIGN.md)
**Last updated:** 2026-05-17

## Progress

- [x] Added canonical design system to `DESIGN.md`.
- [x] Generated brand color tokens in `brand/ui/color-tokens.css`.
- [x] Generated typography tokens in `brand/ui/typography.css`.
- [x] Standardized public logo usage on the black-background, white-icon asset.
- [x] Generated social/OG assets in `brand/social/` and `web/og.png`.
- [x] Added landing-page brand override stylesheet at `web/brand.css`.
- [x] Updated landing hero to the production copy: “Orchestrate Intelligence.” / “Multi-agent systems. Unified control. Real execution.”
- [x] Added contributor usage checklist in `brand/docs/BRAND-USAGE.md`.
- [x] Updated `docs/BRAND.md` to point to the new system.

## Surface checklist

### Landing (`web/`)

- [x] Black foundation and strict tokenized palette.
- [x] Purple reserved for CTAs, focus/hover, and small state accents.
- [x] Hover uses glow instead of scale.
- [x] Glass-heavy blur disabled by `web/brand.css` overrides.
- [x] OG metadata and image added.
- [x] Landing inline CSS now imports token files before page styles and uses the adherence override layer for strict interaction/typography rules.

### Documentation (`docs/`)

- [x] Brand docs now reference the approved black-background, white-icon logo.
- [x] Checklist/progress/risks are documented here.
- [x] Markdown diagrams are syntax-checked by the docs smoke test.

### Packages (`packages/cli`, `packages/openclaw-coven`)

- [x] Existing package README SVG icon copies remain documented.
- [x] Package README logo copies are kept in sync with the shared SVG source.

### Application / CLI

- [x] No separate graphical app surface exists in this repo today.
- [ ] Future TUI/web app surfaces must import `brand/ui/color-tokens.css` and `brand/ui/typography.css` or mirror them in platform-native constants.

## Exceptions

1. **Font files are not vendored.** `typography.css` uses local/system font stacks (`EB Garamond`, `Inter`, `JetBrains Mono`, SF Pro Text, SF Mono). All three canonical faces are free/open-source on Google Fonts — Cave loads them at runtime via `next/font/google`, and other surfaces should self-host via `@font-face` from those sources. This avoids licensing mistakes while still giving pixel-identical rendering everywhere.
2. **Landing still has inline page CSS.** It now imports token files and uses `web/brand.css` as the enforcement layer, but a later cleanup should split layout CSS into `web/page.css` for easier review.
3. **Logo variants are not approved for public surfaces.** Public docs, README, packages, and landing chrome should use the black-background, white-icon logo only.
4. **OG image uses local generated raster output.** It should be regenerated whenever the logo or primary tagline changes.

## Risks

- **Social preview portability:** Some platforms cache OG images aggressively. If deployed, invalidate caches after updating `web/og.png`.
- **Font mismatch:** Without bundled licensed fonts, headings may render differently across machines. The stack is intentionally safe but not pixel-identical.
- **Token drift:** Because the landing is static HTML, future edits could bypass tokens unless reviewers enforce `brand/docs/BRAND-USAGE.md`.
- **Package README rendering:** npm/GitHub image handling differs; package-local SVG copies avoid cross-package path dependencies.
- **Brand drift:** Older notes allowed multiple logo variants. Treat the single black-background, white-icon asset as authoritative unless maintainers reverse it.

## Next hardening steps

- [ ] Refactor remaining landing layout CSS into `web/page.css` so future token drift is easier to audit.
- [ ] Add a lightweight script that fails on non-tokenized brand colors in web CSS.
- [ ] Generate a versioned asset manifest (`brand/manifest.json`) that identifies the approved logo.
- [ ] Produce platform icon exports from the approved SVG instead of relying on the existing PNG set.
- [ ] Add README badges and GitHub banner once public profile surfaces are ready.
