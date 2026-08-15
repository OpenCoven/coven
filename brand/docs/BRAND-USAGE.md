# OpenCoven Brand Usage

This is the implementation companion to [`../../DESIGN.md`](../../DESIGN.md).

## Required imports

For web surfaces, import both token files before component styles:

```css
@import "../brand/ui/color-tokens.css";
@import "../brand/ui/typography.css";
```

The static landing page uses `web/brand.css`, which mirrors these tokens and overrides page styles for strict adherence.

## Required files

- Approved logo: `assets/opencoven/opencoven.svg`
- Public docs logo: `docs/assets/opencoven-icon.svg`
- Full black-square compatibility export: `brand/logo/opencoven-logo.svg`
- Website favicon: `web/favicon.svg`
- Website touch icon: `web/apple-touch-icon.png`
- Transparent composition mark: `brand/logo/opencoven-mark.svg`
- Transparent white compatibility mark: `brand/logo/opencoven-white.svg`
- Transparent black compatibility mark: `brand/logo/opencoven-black.svg`
- Monoline compatibility alias: `brand/logo/opencoven-monoline.svg`
- UI tokens: `brand/ui/color-tokens.css`
- Typography tokens: `brand/ui/typography.css`
- Social/OG assets: `brand/social/*`
- Landing copies: `web/og.png`, `web/brand.css`

## Logo rules

- The full black-square logo is the default public export for docs, README, package, avatar, and other public identity surfaces.
- Controlled dark compositions, including website chrome, may use `brand/logo/opencoven-mark.svg`.
- `brand/logo/opencoven-white.svg`, `brand/logo/opencoven-black.svg`, and `brand/logo/opencoven-monoline.svg` are compatibility assets; the monoline filename aliases the current crown because no separate monoline drawing exists.
- Render the logo at 24px minimum, preserve at least 10% clear space, and keep exact proportions.
- If `assets/opencoven/opencoven.svg` changes, keep `docs/assets/opencoven-icon.svg`, `packages/cli/assets/opencoven.svg`, and `packages/openclaw-coven/assets/opencoven.svg` in sync.

## PR checklist

- [ ] Colors use `--oc-*` tokens or documented semantic aliases.
- [ ] Typography uses `--oc-font-ui`, `--oc-font-display`, or `--oc-font-mono`.
- [ ] Public exports use the approved black-background, white-crown asset; controlled dark compositions may use the transparent mark.
- [ ] Website favicon/touch exports are regenerated from the canonical SVG when the approved logo changes.
- [ ] Hover states glow; they do not scale layout.
- [ ] UI is mostly black/white with purple kept to accent and identity moments.
- [ ] Diagrams are clean lines/nodes, not decorative gradients.
- [ ] Any exception is recorded in `docs/BRANDING-ADHERENCE.md`.
