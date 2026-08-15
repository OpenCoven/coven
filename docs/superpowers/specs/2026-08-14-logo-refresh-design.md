# OpenCoven Logo Refresh Design

## Goal

Replace every repository-owned use of the previous OpenCoven emblem with the
approved crown while preserving each surface's existing purpose, dimensions,
layout, and packaging behavior.

The approved source is the staged
`assets/opencoven/opencoven.svg` from the primary checkout:

- Canvas: `2047 x 2047`
- Treatment: white crown on a black square
- SHA-256: `23277948e41342302ea0f6e514a95aef59fb616f4457de63cd7ad7eba9d9ef2e`

This SVG becomes the sole canonical identity source. The separately staged
redundant raster compatibility copies are not retained.

## Asset Architecture

### Canonical and synchronized copies

`assets/opencoven/opencoven.svg` is authoritative. The following public and
package-local files must be byte-identical copies:

- `docs/assets/opencoven-icon.svg`
- `packages/cli/assets/opencoven.svg`
- `packages/openclaw-coven/assets/opencoven.svg`

Package-local copies remain because npm README rendering must not depend on
files outside the package tarball.

### Brand variants

The existing compatibility filenames under `brand/logo/` remain available:

- `opencoven-logo.svg`: full white-crown-on-black-square treatment
- `opencoven-mark.svg`: transparent standalone crown using `currentColor`
- `opencoven-white.svg`: transparent white crown
- `opencoven-black.svg`: transparent black crown
- `opencoven-monoline.svg`: compatibility alias using the new crown silhouette

The new identity has no separate monoline drawing. Retaining the filename as an
alias avoids breaking consumers while ensuring no old geometry remains.

## Derived Surfaces

### Raster icons

Regenerate every tracked square raster directly from the canonical SVG:

- `assets/opencoven/opencoven-{16,29,32,40,60,64,87,120,128,180,256,512,1024}.png`
- `packages/cli/assets/opencoven-128.png`
- `packages/openclaw-coven/assets/opencoven-128.png`
- `web/apple-touch-icon.png`

Each file keeps its current filename and required dimensions. Rendering must
preserve the canonical square canvas and must not add cropping, rounded corners,
or platform-specific padding.

### Website chrome

The website keeps its existing dark layout. Its inline `#opencoven-mark`
symbol will be replaced with the transparent crown geometry, not the full black
square. Navigation, hero accents, and footer references continue to use the
same symbol ID and markup so this remains a visual-only change.

### Social and Open Graph graphics

Preserve the existing dimensions, typography, copy, colors, and layout of:

- `brand/social/opencoven-og.svg` and `brand/social/opencoven-og.png`
- `brand/social/github-banner.png`
- `brand/social/x-avatar.png`
- `brand/social/x-banner.png`
- `web/og.svg` and `web/og.png`

The reviewed banner exception keeps the black background, network art, and
layout intact; it replaces only the old low-contrast emblem with the canonical
crown in the approved dim violet `#6A5FA0`, so the mark stays subdued but
recognizable.

The OG composition keeps its existing white crown/text/purple-glow palette.
SVG source files are updated before their PNG exports so the committed vector
and raster forms agree.

## Generation

Use the locally available `rsvg-convert` and ImageMagick tools. No repository
dependency or build-system change is required.

Generation must stop with a non-zero exit status if:

- the canonical SVG is missing or malformed;
- an SVG or raster conversion fails;
- an output has an unexpected size;
- a required package or docs copy cannot be synchronized.

Generated files are committed; consumers do not need image tooling at runtime
or during package installation.

## Documentation

Update `DESIGN.md`, `docs/BRAND.md`, and
`brand/docs/BRAND-USAGE.md` only where their asset descriptions no longer
match the new variant roles. The public rule remains: README, docs, package,
favicon, avatar, and other identity exports use the full black-square logo;
the transparent mark is reserved for composition inside controlled dark
surfaces such as the website chrome.

## Verification

The implementation is complete when:

1. Canonical, docs, and package SVG copies are byte-identical.
2. Every raster dimension matches its filename or documented platform size.
3. Every SVG parses as XML and every PNG is readable.
4. No tracked brand or website asset contains the previous emblem geometry.
5. Website header, hero, and footer still reference `#opencoven-mark`.
6. Social and Open Graph files retain their previous dimensions and layouts.
7. A contact sheet confirms legibility at small icon sizes and correct emblem
   placement in social and banner graphics.
8. `git diff --check`, the repository secret scan, and the staged privacy guard
   pass.

## Out of Scope

- Changing OpenCoven typography, colors, messaging, or website layout
- Introducing a new wordmark or alternate crown drawing
- Changing package structure or external social-platform configuration
- Adding a permanent asset-generation dependency or CI pipeline
