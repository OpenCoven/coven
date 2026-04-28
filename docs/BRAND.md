# Coven Brand Assets

The canonical OpenCoven logo is [`assets/opencoven/opencoven.svg`](../assets/opencoven/opencoven.svg).
Raster PNGs in the same directory are derived package and platform preview assets.

## Logo Pack

| Asset | Size | Intended use |
| --- | --- | --- |
| [`opencoven.svg`](../assets/opencoven/opencoven.svg) | Vector, 2272 x 2272 viewBox | Canonical logo source |
| [`opencoven-16.png`](../assets/opencoven/opencoven-16.png) | 16 x 16 | Small favicon-scale surfaces |
| [`opencoven-29.png`](../assets/opencoven/opencoven-29.png) | 29 x 29 | Compact platform icon slots |
| [`opencoven-32.png`](../assets/opencoven/opencoven-32.png) | 32 x 32 | Standard favicon-scale surfaces |
| [`opencoven-40.png`](../assets/opencoven/opencoven-40.png) | 40 x 40 | Compact platform icon slots |
| [`opencoven-60.png`](../assets/opencoven/opencoven-60.png) | 60 x 60 | Platform icon slots |
| [`opencoven-64.png`](../assets/opencoven/opencoven-64.png) | 64 x 64 | Package and integration thumbnails |
| [`opencoven-87.png`](../assets/opencoven/opencoven-87.png) | 87 x 87 | Platform icon slots |
| [`opencoven-120.png`](../assets/opencoven/opencoven-120.png) | 120 x 120 | Package and integration thumbnails |
| [`opencoven-128.png`](../assets/opencoven/opencoven-128.png) | 128 x 128 | README, package, and integration thumbnails |
| [`opencoven-180.png`](../assets/opencoven/opencoven-180.png) | 180 x 180 | Apple touch icon-scale surfaces |
| [`opencoven-256.png`](../assets/opencoven/opencoven-256.png) | 256 x 256 | Repo README and docs previews |
| [`opencoven-512.png`](../assets/opencoven/opencoven-512.png) | 512 x 512 | High-resolution package and app previews |
| [`opencoven-1024.png`](../assets/opencoven/opencoven-1024.png) | 1024 x 1024 | Source-resolution icon asset |

## Package Copies

The npm package READMEs use package-local copies of `opencoven.svg` so package previews do not depend on files outside the package tarball:

- [`packages/cli/assets/opencoven.svg`](../packages/cli/assets/opencoven.svg)
- [`packages/openclaw-coven/assets/opencoven.svg`](../packages/openclaw-coven/assets/opencoven.svg)

Keep those copies in sync with [`assets/opencoven/opencoven.svg`](../assets/opencoven/opencoven.svg) when updating the logo. Keep the `opencoven-*.png` files in sync when raster previews are regenerated.
