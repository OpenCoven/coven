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
- Website favicon: `web/favicon.svg`
- Transparent composition mark: `brand/logo/opencoven-mark.svg`
- UI tokens: `brand/ui/color-tokens.css`
- Typography tokens: `brand/ui/typography.css`
- Social/OG assets: `brand/social/*`
- Landing copies: `web/og.png`, `web/brand.css`

## PR checklist

- [ ] Colors use `--oc-*` tokens or documented semantic aliases.
- [ ] Typography uses `--oc-font-ui`, `--oc-font-display`, or `--oc-font-mono`.
- [ ] Public exports use the approved black-background, white-crown asset; controlled dark compositions may use the transparent mark.
- [ ] Hover states glow; they do not scale layout.
- [ ] UI is mostly black/white with purple kept to accent and identity moments.
- [ ] Diagrams are clean lines/nodes, not decorative gradients.
- [ ] Any exception is recorded in `docs/BRANDING-ADHERENCE.md`.
