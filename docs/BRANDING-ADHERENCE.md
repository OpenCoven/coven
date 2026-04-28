# Branding Adherence Checklist, Progress, Exceptions, and Risks

**Status:** In progress
**Source of truth:** [`../DESIGN.md`](../DESIGN.md)
**Last updated:** 2026-04-28

## Progress

- [x] Added canonical design system to `DESIGN.md`.
- [x] Generated brand color tokens in `brand/ui/color-tokens.css`.
- [x] Generated typography tokens in `brand/ui/typography.css`.
- [x] Added canonical SVG logo variants in `brand/logo/`.
- [x] Generated social/OG assets in `brand/social/` and `web/og.png`.
- [x] Added landing-page brand override stylesheet at `web/brand.css`.
- [x] Updated landing hero to the production copy: “Orchestrate Intelligence.” / “Multi-agent systems. Unified control. Real execution.”
- [x] Added contributor usage checklist in `brand/docs/BRAND-USAGE.md`.
- [x] Updated `docs/BRAND.md` to point to the new system.

## Surface checklist

### Landing (`web/`)

- [x] Black foundation and strict tokenized palette.
- [x] Purple reserved for logo, CTAs, focus/hover, and small state accents.
- [x] Hover uses glow instead of scale.
- [x] Glass-heavy blur disabled by `web/brand.css` overrides.
- [x] OG metadata and image added.
- [x] Landing inline CSS now imports token files before page styles and uses the adherence override layer for strict interaction/typography rules.

### Documentation (`docs/`)

- [x] Brand docs now reference canonical design system and generated assets.
- [x] Checklist/progress/risks are documented here.
- [ ] Markdown diagrams are not yet visually restyled; current docs are mostly text/Rust/API and have limited brand surface.

### Packages (`packages/cli`, `packages/openclaw-coven`)

- [x] Existing package README icon copies remain documented.
- [ ] Package READMEs still use PNG logo copies for npm compatibility.
- [ ] If npm rendering allows, migrate package READMEs to canonical SVG variants later.

### Application / CLI

- [x] No separate graphical app surface exists in this repo today.
- [ ] Future TUI/web app surfaces must import `brand/ui/color-tokens.css` and `brand/ui/typography.css` or mirror them in platform-native constants.

## Exceptions

1. **Font files are not vendored.** `typography.css` uses local/system font stacks (`Satoshi`, `Neue Montreal`, `Geist`, `Inter`, SF Pro). This avoids licensing mistakes. If OpenCoven wants exact Satoshi rendering in production, add licensed font files under `brand/fonts/` with license notes.
2. **Landing still has inline page CSS.** It now imports token files and uses `web/brand.css` as the enforcement layer, but a later cleanup should split layout CSS into `web/page.css` for easier review.
3. **Prior “avoid gradients” preference is superseded by the latest production brand kit.** Gradients are now allowed only for primary logo/OG/signature glow moments and remain excluded from diagrams/general UI.
4. **OG image uses local generated raster output.** It should be regenerated whenever the logo or primary tagline changes.

## Risks

- **Social preview portability:** Some platforms cache OG images aggressively. If deployed, invalidate caches after updating `web/og.png`.
- **Font mismatch:** Without bundled licensed fonts, headings may render differently across machines. The stack is intentionally safe but not pixel-identical.
- **Token drift:** Because the landing is static HTML, future edits could bypass tokens unless reviewers enforce `brand/docs/BRAND-USAGE.md`.
- **Package README rendering:** npm/GitHub image handling differs; keeping PNG fallbacks avoids broken logos but duplicates assets.
- **Brand conflict memory:** Older notes requested no gradients. The new kit is more specific and newer; treat it as authoritative unless Val reverses it.

## Next hardening steps

- [ ] Refactor remaining landing layout CSS into `web/page.css` so future token drift is easier to audit.
- [ ] Add a lightweight script that fails on non-tokenized brand colors in web CSS.
- [ ] Generate a versioned asset manifest (`brand/manifest.json`).
- [ ] Produce platform icon exports from the canonical SVG mark instead of relying on the existing PNG set.
- [ ] Add README badges and GitHub banner once public profile surfaces are ready.
