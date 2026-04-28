# OpenCoven — Brand & Design System

**Status:** Production-Ready | Last Updated: 2026-04-28

---

## 1. Brand Core

### Positioning
**OpenCoven** = collective intelligence + controlled power

A system where agents, tools, and workflows converge under intentional orchestration.

### Brand Traits
- **Arcane, but precise** — mystical without chaos
- **Technical, not gimmicky** — substance over flair
- **Powerful, not loud** — confidence without shouting
- **Minimal, but symbolic** — every element carries weight

### Tone
- Confident, restrained
- Slightly esoteric but grounded in capability
- No fluff, no hype language
- Direct, authoritative, intellectually rigorous

---

## 2. Logo System

### Primary Mark: The Trident-Flame Sigil

**Symbolism:**
- **Trident** → control, direction, execution
- **Flame** → intelligence, transformation, power
- **Hood/crest** → orchestration layer, system boundary
- **Side crescents** → awareness, sensing, reach

### Usage Rules
- **Default:** on black background (#000000)
- **Minimum scale:** 24px (avoid detail loss in flame detail)
- **Clear space:** margin = height of inner flame tip
- **Aspect ratio:** preserve exact proportions across all variants

### Logo Variants

| Variant | Use Case | Notes |
|---------|----------|-------|
| Full Gradient (Primary) | Landing pages, social banners, hero sections | #6E4BFF → #A78BFF linear |
| Solid White | Apple icons, small sizes, dark backgrounds | #ffffff only |
| Solid Black | Light backgrounds, print | #000000 only |
| Monoline (Optional) | Dev docs, diagrams, technical contexts | Single stroke, no fill |

### Logo Files (Canonical Locations)
```
/brand/logo/
  ├── opencoven-logo.svg          # Full gradient (primary)
  ├── opencoven-mark.svg          # Mark only (square crop)
  ├── opencoven-white.svg         # Solid white variant
  ├── opencoven-black.svg         # Solid black variant
  └── opencoven-monoline.svg      # Outline / technical variant
```

---

## 3. Color System

### Primary Palette

```css
/* Anchors */
--oc-black: #000000;
--oc-white: #ffffff;

/* Purple Spectrum (Signature) */
--oc-purple-1: #6E4BFF;      /* Deep / rich */
--oc-purple-2: #8A63FF;      /* Mid / primary UI accent */
--oc-purple-3: #A78BFF;      /* Light / subtle backgrounds */
--oc-purple-glow: #7C5CFF;   /* Glow / hover states */
```

### Accent Palette (Controlled Use)
```css
/* System / Actionable */
--oc-accent-blue: #0A84FF;

/* Semantic */
--oc-danger: #FF3B30;
--oc-success: #30D158;
```

### Gradients (Signature)

**Linear (primary):**
```
#6E4BFF → #A78BFF
```

**Radial Glow (halos, accents):**
```
#8A63FF → transparent
```

### Usage Guidelines
- **90% black / white** — compose UI on neutral foundation
- **10% purple accents** — hover states, highlights, borders, glows
- **Never over-saturate** — avoid multiple gradients or over-layered purples
- **Glow only on interaction** — breathing, hover, focus states
- **Maintain contrast** — all text must meet WCAG AA minimums

### Implementation Files
```
/brand/ui/
  └── color-tokens.css    # CSS custom properties + gradients
```

---

## 4. Typography

### Primary Typefaces

**UI / System (Default):**
- **Inter** (web, cross-platform)
- **SF Pro** (Apple environments: iOS, macOS, watchOS)
- Fallback: system-ui stack

**Display / Brand (Headlines, Hero):**
- **Satoshi** (preferred, distinctive but technical)
- **Neue Montreal** (alternative, modern, geometric)
- **Geist** (optional, if Satoshi unavailable)

### Rules
- **Tight tracking** on headers (letterspacing: -0.02em)
- **No playful fonts** — maintain authority
- **Avoid all-caps** except for labels / badges
- **Line height:** 1.4 for body, 1.2 for headers
- **Font weights:** Regular (400) for body, Semi-bold (600) for emphasis, Bold (700) for headers

### Implementation Files
```
/brand/ui/
  └── typography.css    # Font stack, scales, weights
```

---

## 5. Iconography

### Style
- **Monoweight** — consistent stroke weight throughout
- **Geometric** — clean, rational shapes
- **Slightly sharp** — not rounded (avoid Material Design style)
- **Grid-aligned** — 24px or 32px base grids

### Reusable Motifs
- **Trident tips** — agency, direction
- **Crescent arcs** — sensing, awareness, connectivity
- **Flame curves** — intelligence, transformation
- **Node points** — agents, distributed actors
- **Connection lines** — workflows, orchestration

### Icon Library Location
```
/brand/icons/
  ├── agent-node.svg
  ├── tool-execute.svg
  ├── gateway.svg
  ├── workflow.svg
  ├── trident.svg
  └── ...
```

---

## 6. Landing Page System

### Layout Structure

#### Hero Section
- **Background:** Pure black (#000000)
- **Content:** Centered glowing sigil (logo)
- **Glow effect:** Subtle radial purple (#8A63FF with opacity gradient)
- **Breathing animation:** 2–3s loop, soft pulse

**Headline:**
```
"Orchestrate Intelligence."
```

**Sub-headline:**
```
Multi-agent systems. Unified control. Real execution.
```

**CTAs:**
- **Primary:** "Get Started" (purple glow on hover)
- **Secondary:** "View Docs" (subtle border, no fill)

#### Sections

**1. Capabilities Grid**
- Agent orchestration
- Tool execution
- Gateway abstraction
- Workflow memory
- 2×2 or 1×4 layout, black backgrounds with subtle purple accents

**2. Visual System Diagram**
- Nodes (agents) shown as small circles
- Lines (execution paths) in purple
- Central knot/sigil (OpenCoven core) prominent
- Interactive hover states (node glow, path highlight)

**3. Developer Experience**
- CLI showcase
- API compatibility
- Containerization highlights
- Code blocks with syntax highlighting

**4. Social Proof / Community**
- GitHub stars
- Discord members
- Top contributors
- Testimonials (if any)

### UI Style
- **Glass:** None (no heavy blur effects)
- **Borders:** Subtle, `rgba(255,255,255,0.08)`
- **Hover states:** Glow effect, NOT scale/grow
- **Spacing:** Generous negative space
- **Shadows:** Minimal, only for depth where necessary

---

## 7. Social Media

### X (Twitter)

**Avatar**
- Centered white logo on pure black background
- No gradient (loses clarity at small size)
- Dimensions: 1024×1024 px
- File: `/brand/social/x-avatar.png`

**Banner (1500×500)**
```
[Left]     [Center]      [Right]
Logo       Tagline       Subtle node
(faded)    (centered)    pattern
```

**Header Text:**
```
OpenCoven
Orchestrate Intelligence
```

### GitHub

**Profile Avatar**
- Same as X avatar (white logo on black)

**README Structure**
```markdown
# OpenCoven

Orchestrate intelligence across agents, tools, and systems.

## Features
- OpenAI-compatible gateway
- Multi-agent execution
- Tool-aware runtime
- Container-native

## Quickstart
[...]
```

**Repo Branding**
- Dark mode optimized badges (purple for OpenCoven, neutral for dependencies)
- Purple accent shields
- Clean diagrams (lines, nodes, no gradients)
- Logo at top of README

**Files:**
```
/brand/social/
  ├── x-avatar.png
  ├── x-banner.png
  └── github-banner.png
```

---

## 8. Apple Ecosystem (iOS + macOS)

### App Icon (General)

**Rules**
- Use solid white mark (#ffffff)
- Background: pure black (#000000) OR subtle radial purple glow
- No text
- Must pass squircle crop safely (avoid edge clipping)

**Master Size:** 1024×1024 px
- Export down to all smaller sizes (iOS: 180×180, 120×120, etc.)
- Use appropriate transparency/anti-aliasing

**Files:**
```
/brand/icons/
  ├── app-icon-1024.png          # Master
  ├── app-icon-ios-set/
  │   ├── 180.png
  │   ├── 120.png
  │   └── ...
  └── app-icon-macos-set/
      ├── 512.png
      ├── 256.png
      └── ...
```

### macOS

**Dock Icon**
- Slight inner glow (optional, subtle)
- Slight depth perception (shadows underneath, not skeuomorphic)
- Remain recognizable at 32×32 px minimum

**Menu Bar Icon**
- Monochrome white only (#ffffff)
- Consistent with system aesthetic
- No gradients or colors

### iOS

**Splash Screen**
- Black background (#000000)
- Centered logo with soft glow pulse (2–3s loop)
- Optional animation on app launch

---

## 9. Motion System

### Principles
- **Subtle** — not distracting
- **Intentional** — every movement serves purpose
- **Never decorative** — functional elegance only

### Standard Animations

**Glow Breathing**
```css
animation: breathe 2.5s ease-in-out infinite;
@keyframes breathe {
  0%, 100% { opacity: 0.7; }
  50% { opacity: 1; }
}
```

**Node Connection**
- Lines draw on interaction or page load
- Duration: 0.6–1.2s depending on path length
- Easing: ease-out (smooth deceleration)

**Gradient Shift**
- Hover states: subtle opacity/hue shift
- Duration: 200ms
- Easing: ease-out

**Focus Ring**
- Purple glow on interactive elements
- Width: 2px
- Color: `--oc-purple-glow` with 0.5 opacity

---

## 10. Design Checklist: Do / Don't

### ✅ Do
- Keep compositions centered and balanced
- Use negative space aggressively (breathing room)
- Let the logo carry the identity (it's strong enough alone)
- Maintain consistent purple accent usage (10% rule)
- Ensure all text is high contrast on backgrounds
- Use motion to clarify interaction, not distract
- Test all designs in both light and dark contexts
- Verify icon legibility at minimum 24px scale

### ❌ Don't
- Overuse gradients (90% black/white minimum)
- Add neon, cyberpunk clutter, or decorative flourishes
- Introduce randomness or asymmetry without reason
- Mix serif/sans fonts carelessly
- Use colored shadows or noise textures
- Scale logo below 24px without testing
- Create multiple variants of logo (stick to the 4 defined ones)
- Add transparency overlays that muddy the palette
- Use the accent blue for general UI (reserve for actionable states)

---

## 11. Implementation Audit

### Core Repos to Audit & Update

#### OpenCoven/coven
- [ ] Update README with brand guidelines reference
- [ ] Add `/brand` directory with logo variants
- [ ] Implement `brand/ui/color-tokens.css`
- [ ] Update docs site colors to match palette
- [ ] Replace any non-brand assets with OpenCoven sigil

#### OpenClaw Integration (openclaw-coven)
- [ ] Update plugin README
- [ ] Ensure consistent color usage in UI
- [ ] Update badge / shield colors

#### OpenSorceryAI/open-meow
- [ ] Update app icon to OpenCoven sigil (if applicable)
- [ ] Apply color palette to UI
- [ ] Update hero section imagery
- [ ] Ensure landing page follows layout structure

#### Landing Page (docs.opencoven.ai)
- [ ] Hero section: centered sigil + glow
- [ ] Capabilities grid: 90% black/white, 10% purple
- [ ] Diagram: nodes + paths in brand colors
- [ ] All CTAs: purple glow on hover, no scale
- [ ] Footer: logo variants, social links

#### Social Media Assets
- [ ] X avatar: white logo on black
- [ ] X banner: logo + tagline layout
- [ ] GitHub profile: updated README + banner
- [ ] All badges: purple accent (custom)

#### Documentation Sites
- [ ] Apply color tokens to all `<code>`, `<pre>` blocks
- [ ] Update button styles: purple accents, no gradients
- [ ] Ensure all headings use tight tracking
- [ ] Logo in header/footer: correct variant for background

### File Structure Template

```
/brand
  /logo
    ├── opencoven-logo.svg          # Full gradient primary
    ├── opencoven-mark.svg          # Mark only
    ├── opencoven-white.svg         # Solid white
    ├── opencoven-black.svg         # Solid black
    └── opencoven-monoline.svg      # Outline variant

  /icons
    ├── agent-node.svg
    ├── tool-execute.svg
    ├── gateway.svg
    ├── workflow.svg
    ├── trident.svg
    ├── app-icon-1024.png           # Master
    ├── app-icon-ios-set/
    │   ├── 180.png
    │   ├── 120.png
    │   └── ...
    └── app-icon-macos-set/
        ├── 512.png
        ├── 256.png
        └── ...

  /social
    ├── x-avatar.png                # 1024×1024
    ├── x-banner.png                # 1500×500
    └── github-banner.png           # 1280×640

  /ui
    ├── color-tokens.css            # CSS vars + gradients
    └── typography.css              # Font stacks + scales

  /docs
    └── BRAND-USAGE.md              # Quick-start for contributors
```

---

## 12. Quick-Start for Contributors

### Using Colors
```css
/* Always use these variables, never hardcode hex */
color: var(--oc-purple-2);
background: var(--oc-black);
border: 1px solid rgba(255, 255, 255, 0.08);
```

### Using Fonts
```css
/* UI */
font-family: 'Inter', 'SF Pro', system-ui, sans-serif;

/* Headers */
font-family: 'Satoshi', 'Neue Montreal', sans-serif;
letter-spacing: -0.02em;
```

### Logo Usage
- **Landing hero:** Full gradient variant
- **Small icons/badges:** White solid variant
- **Dark backgrounds:** White solid variant
- **Light backgrounds:** Black solid variant
- **Diagrams:** Monoline variant

### Adding New Icons
1. Check `/brand/icons/` for existing similar icon
2. Match monoweight stroke (1.5–2px)
3. Align to 24px or 32px grid
4. Test at minimum 24px display size
5. Place in `/brand/icons/` with descriptive name

---

## 13. Versioning & Maintenance

**Current Version:** 1.0.0 (2026-04-28)

**Last Reviewed:** 2026-04-28
**Reviewed By:** OpenCoven Brand Core

**Change Log:**
- **v1.0.0** – Initial production-ready system

**Next Review:** Quarterly or when major product change occurs

---

## 📞 Questions / Clarifications

For brand guidance, deviations, or approvals:
1. Check this document first
2. Reference `/brand/docs/BRAND-USAGE.md` for quick answers
3. Open issue in core repo with `[brand]` tag
4. Request review from @opencoven-team

---

**Remember:** The brand system is a constraint that frees us. Within these rules, every implementation should feel unmistakably OpenCoven.
