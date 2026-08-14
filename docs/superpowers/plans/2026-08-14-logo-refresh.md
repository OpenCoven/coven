# OpenCoven Logo Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every repository-owned use of the previous OpenCoven emblem with the approved crown while preserving dimensions, layouts, package portability, and compatibility filenames.

**Architecture:** Treat `assets/opencoven/opencoven.svg` as the single canonical full-square logo. Derive package/docs copies, transparent brand variants, raster sizes, website chrome, and social exports from its `#mark` path; keep existing public filenames and compositions stable.

**Tech Stack:** SVG/XML, Node.js standard library, `rsvg-convert`, ImageMagick, HTML, Markdown, Git

---

## File Map

**Canonical source and brand variants**

- Modify: `assets/opencoven/opencoven.svg` — approved full-square source
- Modify: `brand/logo/opencoven-logo.svg` — full-square compatibility copy
- Modify: `brand/logo/opencoven-mark.svg` — transparent `currentColor` crown
- Modify: `brand/logo/opencoven-white.svg` — transparent white crown
- Modify: `brand/logo/opencoven-black.svg` — transparent black crown
- Modify: `brand/logo/opencoven-monoline.svg` — compatibility alias for the new silhouette

**Synchronized public/package assets**

- Modify: `docs/assets/opencoven-icon.svg`
- Modify: `packages/cli/assets/opencoven.svg`
- Modify: `packages/openclaw-coven/assets/opencoven.svg`
- Create: `web/favicon.svg`

**Raster derivatives**

- Modify: `assets/opencoven/opencoven-{16,29,32,40,60,64,87,120,128,180,256,512,1024}.png`
- Modify: `packages/cli/assets/opencoven-128.png`
- Modify: `packages/openclaw-coven/assets/opencoven-128.png`
- Modify: `web/apple-touch-icon.png`

**Website and social compositions**

- Modify: `web/index.html` — transparent inline crown symbol
- Modify: `brand/social/opencoven-og.svg`
- Modify: `brand/social/opencoven-og.png`
- Modify: `brand/social/github-banner.png`
- Modify: `brand/social/x-avatar.png`
- Modify: `brand/social/x-banner.png`
- Modify: `web/og.svg`
- Modify: `web/og.png`

**Documentation**

- Modify: `DESIGN.md`
- Modify: `docs/BRAND.md`
- Modify: `brand/docs/BRAND-USAGE.md`

### Task 1: Import the canonical crown and rebuild brand variants

**Files:**
- Modify: `assets/opencoven/opencoven.svg`
- Modify: `brand/logo/opencoven-logo.svg`
- Modify: `brand/logo/opencoven-mark.svg`
- Modify: `brand/logo/opencoven-white.svg`
- Modify: `brand/logo/opencoven-black.svg`
- Modify: `brand/logo/opencoven-monoline.svg`

- [ ] **Step 1: Verify the worktree still contains the previous canonical SVG**

Run:

```bash
cd /tmp/coven-logo-refresh
test "$(shasum -a 256 assets/opencoven/opencoven.svg | awk '{print $1}')" = \
  "23277948e41342302ea0f6e514a95aef59fb616f4457de63cd7ad7eba9d9ef2e"
```

Expected: exit status `1`, proving the approved crown has not yet been imported.

- [ ] **Step 2: Copy the user-approved canonical SVG into the worktree**

Run:

```bash
cd /tmp/coven-logo-refresh
primary_checkout="$(
  git worktree list --porcelain |
    awk '$1 == "worktree" { print $2; exit }'
)"
cp "$primary_checkout/assets/opencoven/opencoven.svg" \
  assets/opencoven/opencoven.svg
test "$(shasum -a 256 assets/opencoven/opencoven.svg | awk '{print $1}')" = \
  "23277948e41342302ea0f6e514a95aef59fb616f4457de63cd7ad7eba9d9ef2e"
```

Expected: exit status `0`.

- [ ] **Step 3: Create a temporary variant generator**

Create `/tmp/coven-logo-refresh-build-variants.mjs` with:

```js
import fs from "node:fs";

const canonicalPath = "/tmp/coven-logo-refresh/assets/opencoven/opencoven.svg";
const canonical = fs.readFileSync(canonicalPath, "utf8");
const pathElement = canonical.match(/<path id="mark"[\s\S]*?\/>/)?.[0];
const d = pathElement?.match(/\sd="([^"]+)"/s)?.[1];

if (!d) {
  throw new Error("canonical SVG does not contain a self-closing #mark path");
}

const svg = (title, fill) => `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="2047" height="2047" viewBox="0 0 2047 2047" preserveAspectRatio="xMidYMid meet">
  <title>${title}</title>
  <path id="mark" fill="${fill}" shape-rendering="geometricPrecision" fill-rule="evenodd" clip-rule="evenodd" d="${d}"/>
</svg>
`;

const root = "/tmp/coven-logo-refresh/brand/logo";
fs.copyFileSync(canonicalPath, `${root}/opencoven-logo.svg`);
fs.writeFileSync(`${root}/opencoven-mark.svg`, svg("OpenCoven crown mark", "currentColor"));
fs.writeFileSync(`${root}/opencoven-white.svg`, svg("OpenCoven white crown mark", "#ffffff"));
fs.writeFileSync(`${root}/opencoven-black.svg`, svg("OpenCoven black crown mark", "#000000"));
fs.writeFileSync(
  `${root}/opencoven-monoline.svg`,
  svg("OpenCoven crown mark compatibility alias", "currentColor"),
);
```

Expected: the script contains no external package imports.

- [ ] **Step 4: Generate and validate the variants**

Run:

```bash
node /tmp/coven-logo-refresh-build-variants.mjs
cd /tmp/coven-logo-refresh
xmllint --noout \
  assets/opencoven/opencoven.svg \
  brand/logo/opencoven-logo.svg \
  brand/logo/opencoven-mark.svg \
  brand/logo/opencoven-white.svg \
  brand/logo/opencoven-black.svg \
  brand/logo/opencoven-monoline.svg
cmp assets/opencoven/opencoven.svg brand/logo/opencoven-logo.svg
grep -q 'fill="currentColor"' brand/logo/opencoven-mark.svg
grep -q 'fill="#ffffff"' brand/logo/opencoven-white.svg
grep -q 'fill="#000000"' brand/logo/opencoven-black.svg
rm /tmp/coven-logo-refresh-build-variants.mjs
```

Expected: all commands exit `0`; `cmp` prints nothing.

- [ ] **Step 5: Commit the canonical source and variants**

Run:

```bash
cd /tmp/coven-logo-refresh
git add assets/opencoven/opencoven.svg brand/logo
git commit \
  -m "chore: install new OpenCoven logo source" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: one commit containing only the canonical SVG and five brand variants.

### Task 2: Synchronize public copies and regenerate square icons

**Files:**
- Modify: `docs/assets/opencoven-icon.svg`
- Modify: `packages/cli/assets/opencoven.svg`
- Modify: `packages/openclaw-coven/assets/opencoven.svg`
- Create: `web/favicon.svg`
- Modify: `assets/opencoven/opencoven-{16,29,32,40,60,64,87,120,128,180,256,512,1024}.png`
- Modify: `packages/cli/assets/opencoven-128.png`
- Modify: `packages/openclaw-coven/assets/opencoven-128.png`
- Modify: `web/apple-touch-icon.png`

- [ ] **Step 1: Verify synchronized SVG copies still fail**

Run:

```bash
cd /tmp/coven-logo-refresh
cmp -s assets/opencoven/opencoven.svg docs/assets/opencoven-icon.svg &&
cmp -s assets/opencoven/opencoven.svg packages/cli/assets/opencoven.svg &&
cmp -s assets/opencoven/opencoven.svg packages/openclaw-coven/assets/opencoven.svg
```

Expected: non-zero exit status because the three copies still contain the previous emblem.

- [ ] **Step 2: Synchronize SVG copies and add the website favicon**

Run:

```bash
cd /tmp/coven-logo-refresh
for target in \
  docs/assets/opencoven-icon.svg \
  packages/cli/assets/opencoven.svg \
  packages/openclaw-coven/assets/opencoven.svg \
  web/favicon.svg
do
  cp assets/opencoven/opencoven.svg "$target"
done
```

Expected: all four targets exist and are byte-identical to the canonical SVG.

- [ ] **Step 3: Regenerate every named root raster size**

Run:

```bash
cd /tmp/coven-logo-refresh
for size in 16 29 32 40 60 64 87 120 128 180 256 512 1024
do
  rsvg-convert \
    -w "$size" \
    -h "$size" \
    assets/opencoven/opencoven.svg \
    -o "assets/opencoven/opencoven-${size}.png"
done
```

Expected: thirteen PNG files are rewritten.

- [ ] **Step 4: Regenerate package and website raster copies**

Run:

```bash
cd /tmp/coven-logo-refresh
cp assets/opencoven/opencoven-128.png packages/cli/assets/opencoven-128.png
cp assets/opencoven/opencoven-128.png packages/openclaw-coven/assets/opencoven-128.png
rsvg-convert \
  -w 1254 \
  -h 1254 \
  assets/opencoven/opencoven.svg \
  -o web/apple-touch-icon.png
```

Expected: package icons are byte-identical and the touch icon remains `1254x1254`.

- [ ] **Step 5: Validate synchronization and dimensions**

Run:

```bash
cd /tmp/coven-logo-refresh
for target in \
  docs/assets/opencoven-icon.svg \
  packages/cli/assets/opencoven.svg \
  packages/openclaw-coven/assets/opencoven.svg \
  web/favicon.svg
do
  cmp assets/opencoven/opencoven.svg "$target"
done

for path in assets/opencoven/opencoven-*.png
do
  size="${path##*-}"
  size="${size%.png}"
  test "$(identify -format '%wx%h' "$path")" = "${size}x${size}"
done

test "$(identify -format '%wx%h' packages/cli/assets/opencoven-128.png)" = "128x128"
test "$(identify -format '%wx%h' packages/openclaw-coven/assets/opencoven-128.png)" = "128x128"
test "$(identify -format '%wx%h' web/apple-touch-icon.png)" = "1254x1254"
```

Expected: exit status `0` with no `cmp` output.

- [ ] **Step 6: Commit synchronized and raster assets**

Run:

```bash
cd /tmp/coven-logo-refresh
git add \
  assets/opencoven \
  docs/assets/opencoven-icon.svg \
  packages/cli/assets \
  packages/openclaw-coven/assets \
  web/favicon.svg \
  web/apple-touch-icon.png
git commit \
  -m "chore: regenerate OpenCoven icon assets" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: one commit containing synchronized SVGs and dimension-preserving raster exports.

### Task 3: Replace the website inline mark

**Files:**
- Modify: `web/index.html:943-951`

- [ ] **Step 1: Verify the old inline symbol is present**

Run:

```bash
cd /tmp/coven-logo-refresh
grep -q '<symbol id="opencoven-mark" viewBox="0 0 1024 1024">' web/index.html
grep -q 'M512 82c121 90' web/index.html
```

Expected: both commands exit `0`.

- [ ] **Step 2: Create a temporary website-symbol updater**

Create `/tmp/coven-logo-refresh-update-web-symbol.mjs` with:

```js
import fs from "node:fs";

const root = "/tmp/coven-logo-refresh";
const canonical = fs.readFileSync(`${root}/assets/opencoven/opencoven.svg`, "utf8");
const d = canonical
  .match(/<path id="mark"[\s\S]*?\/>/)?.[0]
  ?.match(/\sd="([^"]+)"/s)?.[1];

if (!d) {
  throw new Error("canonical SVG does not contain #mark geometry");
}

const path = `${root}/web/index.html`;
const html = fs.readFileSync(path, "utf8");
const symbol = `      <symbol id="opencoven-mark" viewBox="0 0 2047 2047">
        <path fill="currentColor" shape-rendering="geometricPrecision" fill-rule="evenodd" clip-rule="evenodd" d="${d}"/>
      </symbol>`;
const updated = html.replace(
  /      <symbol id="opencoven-mark"[\s\S]*?      <\/symbol>/,
  symbol,
);

if (updated === html) {
  throw new Error("existing #opencoven-mark symbol was not replaced");
}

fs.writeFileSync(path, updated);
```

Expected: the script replaces only the symbol definition.

- [ ] **Step 3: Update the website and remove the temporary script**

Run:

```bash
node /tmp/coven-logo-refresh-update-web-symbol.mjs
rm /tmp/coven-logo-refresh-update-web-symbol.mjs
```

Expected: `web/index.html` contains a single-path crown symbol.

- [ ] **Step 4: Verify website references and old geometry removal**

Run:

```bash
cd /tmp/coven-logo-refresh
grep -q '<symbol id="opencoven-mark" viewBox="0 0 2047 2047">' web/index.html
test "$(grep -c '<use href="#opencoven-mark"' web/index.html)" = "3"
! grep -q 'M512 82c121 90' web/index.html
grep -q 'href="https://opencoven.ai/favicon.svg"' web/index.html
```

Expected: exit status `0`; header, hero, and footer still use the same symbol ID.

- [ ] **Step 5: Commit the website mark**

Run:

```bash
cd /tmp/coven-logo-refresh
git add web/index.html
git commit \
  -m "chore: update website logo mark" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: one HTML-only commit.

### Task 4: Rebuild Open Graph vectors and PNG exports

**Files:**
- Modify: `brand/social/opencoven-og.svg`
- Modify: `brand/social/opencoven-og.png`
- Modify: `web/og.svg`
- Modify: `web/og.png`

- [ ] **Step 1: Verify the OG vectors still embed the oversized previous emblem**

Run:

```bash
cd /tmp/coven-logo-refresh
test "$(wc -c < brand/social/opencoven-og.svg)" -gt 400000
grep -q 'M715.769226,587.544373' brand/social/opencoven-og.svg
```

Expected: both commands exit `0`.

- [ ] **Step 2: Create a temporary concise OG generator**

Create `/tmp/coven-logo-refresh-build-og.mjs` with:

```js
import fs from "node:fs";

const root = "/tmp/coven-logo-refresh";
const canonical = fs.readFileSync(`${root}/assets/opencoven/opencoven.svg`, "utf8");
const d = canonical
  .match(/<path id="mark"[\s\S]*?\/>/)?.[0]
  ?.match(/\sd="([^"]+)"/s)?.[1];

if (!d) {
  throw new Error("canonical SVG does not contain #mark geometry");
}

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 630" role="img" aria-label="OpenCoven — Orchestrate Intelligence">
  <defs>
    <radialGradient id="glow" cx="50%" cy="45%" r="48%">
      <stop stop-color="#8A63FF" stop-opacity="0.34"/>
      <stop offset="1" stop-color="#8A63FF" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="1200" height="630" fill="#000000"/>
  <circle cx="600" cy="300" r="292" fill="url(#glow)"/>
  <g transform="translate(456 18) scale(0.14)">
    <path fill="#FFFFFF" shape-rendering="geometricPrecision" fill-rule="evenodd" clip-rule="evenodd" d="${d}"/>
  </g>
  <text x="600" y="360" text-anchor="middle" fill="#FFFFFF" font-family="Satoshi, Inter, system-ui, sans-serif" font-size="82" font-weight="700">OpenCoven</text>
  <text x="600" y="430" text-anchor="middle" fill="#A78BFF" font-family="Satoshi, Inter, system-ui, sans-serif" font-size="42" font-weight="600">Orchestrate Intelligence</text>
  <text x="600" y="488" text-anchor="middle" fill="rgba(255,255,255,.72)" font-family="Inter, system-ui, sans-serif" font-size="28">Multi-agent systems. Unified control. Real execution.</text>
</svg>
`;

fs.writeFileSync(`${root}/brand/social/opencoven-og.svg`, svg);
fs.writeFileSync(`${root}/web/og.svg`, svg);
```

Expected: the generated SVG preserves the existing canvas, glow, text, and placements.

- [ ] **Step 3: Generate vectors and raster exports**

Run:

```bash
node /tmp/coven-logo-refresh-build-og.mjs
cd /tmp/coven-logo-refresh
rsvg-convert \
  -w 1200 \
  -h 630 \
  brand/social/opencoven-og.svg \
  -o brand/social/opencoven-og.png
cp brand/social/opencoven-og.png web/og.png
rm /tmp/coven-logo-refresh-build-og.mjs
```

Expected: both SVG and PNG pairs are synchronized.

- [ ] **Step 4: Verify concise sources, dimensions, and copy equality**

Run:

```bash
cd /tmp/coven-logo-refresh
xmllint --noout brand/social/opencoven-og.svg web/og.svg
test "$(wc -c < brand/social/opencoven-og.svg)" -lt 20000
test "$(identify -format '%wx%h' brand/social/opencoven-og.png)" = "1200x630"
cmp brand/social/opencoven-og.svg web/og.svg
cmp brand/social/opencoven-og.png web/og.png
! grep -q 'M715.769226,587.544373' brand/social/opencoven-og.svg
```

Expected: exit status `0`; the vectors shrink from about 448 KB to under 20 KB.

- [ ] **Step 5: Commit Open Graph assets**

Run:

```bash
cd /tmp/coven-logo-refresh
git add brand/social/opencoven-og.svg brand/social/opencoven-og.png web/og.svg web/og.png
git commit \
  -m "chore: refresh OpenCoven social preview" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: one commit containing only synchronized OG vector/raster files.

### Task 5: Replace raster-only social emblems

**Files:**
- Modify: `brand/social/x-avatar.png`
- Modify: `brand/social/x-banner.png`
- Modify: `brand/social/github-banner.png`

- [ ] **Step 1: Save comparison copies and verify current banner equality**

Run:

```bash
cd /tmp/coven-logo-refresh
cmp brand/social/x-banner.png brand/social/github-banner.png
cp brand/social/x-banner.png /tmp/coven-logo-refresh-original-banner.png
```

Expected: `cmp` prints nothing.

- [ ] **Step 2: Render the approved avatar**

Run:

```bash
cd /tmp/coven-logo-refresh
rsvg-convert \
  -w 1024 \
  -h 1024 \
  assets/opencoven/opencoven.svg \
  -o brand/social/x-avatar.png
```

Expected: the avatar becomes the full white-crown-on-black-square treatment.

- [ ] **Step 3: Build a dark-violet transparent crown for banners**

Run:

```bash
cd /tmp/coven-logo-refresh
rsvg-convert \
  -w 512 \
  -h 512 \
  brand/logo/opencoven-white.svg \
  -o /tmp/coven-logo-refresh-banner-mark-white.png
magick \
  /tmp/coven-logo-refresh-banner-mark-white.png \
  -trim \
  +repage \
  -resize 300x300 \
  -fill '#171228' \
  -colorize 100 \
  /tmp/coven-logo-refresh-banner-mark.png
```

Expected: `/tmp/coven-logo-refresh-banner-mark.png` has a transparent background and a dark-violet crown.

- [ ] **Step 4: Replace only the left emblem region**

Run:

```bash
cd /tmp/coven-logo-refresh
banner_x=95
banner_y=100
magick \
  brand/social/x-banner.png \
  -fill black \
  -draw 'rectangle 0,0 430,499' \
  /tmp/coven-logo-refresh-banner-base.png
magick \
  /tmp/coven-logo-refresh-banner-base.png \
  /tmp/coven-logo-refresh-banner-mark.png \
  -geometry "+${banner_x}+${banner_y}" \
  -composite \
  brand/social/x-banner.png
cp brand/social/x-banner.png brand/social/github-banner.png
rm \
  /tmp/coven-logo-refresh-banner-mark-white.png \
  /tmp/coven-logo-refresh-banner-mark.png \
  /tmp/coven-logo-refresh-banner-base.png
```

Expected: text and network artwork remain unchanged; only the left emblem region changes.

- [ ] **Step 5: Verify dimensions and unchanged right-side pixels**

Run:

```bash
cd /tmp/coven-logo-refresh
test "$(identify -format '%wx%h' brand/social/x-avatar.png)" = "1024x1024"
test "$(identify -format '%wx%h' brand/social/x-banner.png)" = "1500x500"
test "$(identify -format '%wx%h' brand/social/github-banner.png)" = "1500x500"
cmp brand/social/x-banner.png brand/social/github-banner.png
metric="$(
  magick compare -metric AE \
    \( /tmp/coven-logo-refresh-original-banner.png -crop 1070x500+430+0 +repage \) \
    \( brand/social/x-banner.png -crop 1070x500+430+0 +repage \) \
    null: 2>&1
)"
test "${metric%% *}" = "0"
rm /tmp/coven-logo-refresh-original-banner.png
```

Expected: all commands exit `0`; the right `1070x500` pixels are identical to the original.

- [ ] **Step 6: Commit raster-only social assets**

Run:

```bash
cd /tmp/coven-logo-refresh
git add brand/social/x-avatar.png brand/social/x-banner.png brand/social/github-banner.png
git commit \
  -m "chore: update OpenCoven social logos" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: one commit containing the avatar and two banners.

### Task 6: Align brand documentation with the crown variants

**Files:**
- Modify: `DESIGN.md:45-72`
- Modify: `DESIGN.md:280-342`
- Modify: `docs/BRAND.md:38-64`
- Modify: `brand/docs/BRAND-USAGE.md:16-30`

- [ ] **Step 1: Verify outdated emblem terminology remains**

Run:

```bash
cd /tmp/coven-logo-refresh
grep -q 'Trident' DESIGN.md
grep -q 'Flame' DESIGN.md
grep -q 'inner flame tip' DESIGN.md
```

Expected: all commands exit `0`.

- [ ] **Step 2: Replace the symbolism and usage rules in `DESIGN.md`**

Replace the outdated symbolism and clear-space text with:

```markdown
**Symbolism:**
- **Crown silhouette** → governed authority and deliberate control
- **Mirrored wings** → coordinated agents working as a system
- **Rising center** → durable execution and forward motion

### Usage Rules
- **Default public treatment:** white crown on a black square (#000000)
- **Composition treatment:** transparent one-color crown inside controlled dark surfaces
- **Minimum scale:** 24px
- **Clear space:** at least 10% of the rendered logo width
- **Aspect ratio:** preserve exact proportions across all variants
```

Update the approved-logo paragraph to state that the website chrome is the controlled transparent-mark exception. Expand the logo file list to include `brand/logo/*` and `web/favicon.svg`.

Expected: the document describes the crown without referencing tridents, flames, hoods, crescents, or inner flame tips.

- [ ] **Step 3: Document canonical, transparent, and compatibility roles**

Add this rule to `docs/BRAND.md` after the approved-logo table:

```markdown
The full black-square logo is the default public export. The transparent
`brand/logo/opencoven-mark.svg` variant is reserved for composition inside
controlled dark surfaces, including the landing-page navigation, hero, and
footer. The black, white, and monoline filenames remain compatibility assets;
the monoline filename uses the same crown silhouette because the current
identity has no separate monoline drawing.
```

Add these required files to `brand/docs/BRAND-USAGE.md`:

```markdown
- Website favicon: `web/favicon.svg`
- Transparent composition mark: `brand/logo/opencoven-mark.svg`
```

Change its checklist logo item to:

```markdown
- [ ] Public exports use the approved black-background, white-crown asset; controlled dark compositions may use the transparent mark.
```

Expected: contributor documentation matches the implemented variant roles.

- [ ] **Step 4: Verify outdated terms and redundant source references are absent**

Run:

```bash
cd /tmp/coven-logo-refresh
! grep -Eiq 'trident|inner flame tip|side crescents' DESIGN.md docs/BRAND.md brand/docs/BRAND-USAGE.md
! grep -R 'brand-logo.png' DESIGN.md docs/BRAND.md brand/docs/BRAND-USAGE.md
grep -q 'web/favicon.svg' DESIGN.md
grep -q 'opencoven-mark.svg' docs/BRAND.md
```

Expected: exit status `0`.

- [ ] **Step 5: Commit documentation**

Run:

```bash
cd /tmp/coven-logo-refresh
git add DESIGN.md docs/BRAND.md brand/docs/BRAND-USAGE.md
git commit \
  -m "docs: update OpenCoven logo guidance" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: one documentation-only commit.

### Task 7: Perform visual and repository verification

**Files:**
- Verify: all files changed by Tasks 1-6
- Create temporarily: `/tmp/coven-logo-refresh-contact-sheet.png`

- [ ] **Step 1: Validate all tracked OpenCoven SVG files**

Run:

```bash
cd /tmp/coven-logo-refresh
git ls-files \
  'assets/opencoven/*.svg' \
  'brand/logo/*.svg' \
  'brand/social/*.svg' \
  'docs/assets/*.svg' \
  'packages/cli/assets/*.svg' \
  'packages/openclaw-coven/assets/*.svg' \
  'web/*.svg' |
while read -r path
do
  xmllint --noout "$path"
done
```

Expected: exit status `0` with no XML errors.

- [ ] **Step 2: Re-run synchronization and geometry checks**

Run:

```bash
cd /tmp/coven-logo-refresh
for target in \
  docs/assets/opencoven-icon.svg \
  packages/cli/assets/opencoven.svg \
  packages/openclaw-coven/assets/opencoven.svg \
  web/favicon.svg
do
  cmp assets/opencoven/opencoven.svg "$target"
done

! grep -R \
  -e 'M1139.3 100.4' \
  -e 'M512 82c121 90' \
  -e 'M715.769226,587.544373' \
  assets/opencoven brand/logo brand/social docs/assets packages/cli/assets packages/openclaw-coven/assets web
```

Expected: no copy drift and no known previous-emblem geometry.

- [ ] **Step 3: Build a visual contact sheet**

Run:

```bash
cd /tmp/coven-logo-refresh
magick montage \
  assets/opencoven/opencoven-16.png \
  assets/opencoven/opencoven-32.png \
  assets/opencoven/opencoven-64.png \
  assets/opencoven/opencoven-128.png \
  assets/opencoven/opencoven-1024.png \
  web/apple-touch-icon.png \
  brand/social/x-avatar.png \
  brand/social/opencoven-og.png \
  brand/social/x-banner.png \
  -thumbnail 360x220 \
  -background '#ece8f2' \
  -fill '#111111' \
  -stroke none \
  -tile 3x \
  -geometry 380x250+20+20 \
  /tmp/coven-logo-refresh-contact-sheet.png
```

Expected: a `1200px`-wide contact sheet showing small icons, large identity exports, OG art, and banner art.

- [ ] **Step 4: Inspect the contact sheet**

Open `/tmp/coven-logo-refresh-contact-sheet.png` and confirm:

- the crown remains recognizable at `16px`, `32px`, and `64px`;
- all square exports have black backgrounds and white crowns;
- the touch icon has no inherited old emblem or rounded-mask artifact;
- the OG crown is centered above the unchanged text hierarchy;
- the banner crown occupies the former left-emblem region without touching the center text.

Expected: all five checks pass. If one fails, return to the generating task, correct its deterministic transform, and repeat Tasks 7.1-7.4.

- [ ] **Step 5: Run repository diff and safety checks**

Run:

```bash
cd /tmp/coven-logo-refresh
git diff --check origin/main...HEAD
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
privacy_index="$(mktemp)"
rm "$privacy_index"
GIT_INDEX_FILE="$privacy_index" git read-tree origin/main
GIT_INDEX_FILE="$privacy_index" git add -A
GIT_INDEX_FILE="$privacy_index" python3 scripts/check-coven-privacy.py --staged
rm "$privacy_index"
git status --short
```

Expected:

- `git diff --check` prints nothing;
- formatting, Clippy, and workspace tests pass;
- secret and privacy scans report clean results;
- `git status --short` prints nothing because every intended change is committed.

- [ ] **Step 6: Remove the temporary contact sheet**

Run:

```bash
rm /tmp/coven-logo-refresh-contact-sheet.png
```

Expected: no temporary generation or review files remain.
