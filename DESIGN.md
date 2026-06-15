# OpenCoven — Brand & Design System

**Status:** Production-Ready | Last Updated: 2026-05-24 (Field manual redesign + no-gradient rule)

---

## 1. Brand Core

### Positioning
**OpenCoven** = collective intelligence + controlled power

A system where agents, tools, and workflows converge under intentional orchestration.

OpenCoven is an open ecosystem for persistent AI familiars: named agents with memory, tools, identity, roles, and continuity. It moves AI beyond disposable chat sessions and toward durable, personal, intelligible systems that people can own, customize, and collaborate with over time.

The value should be instantly clear: OpenCoven turns AI from a blank chatbox into a living workspace of agents that remember, coordinate, and belong to you.

### Vision
Most AI today feels temporary: a user opens a chat, explains context, gets a response, and starts over. OpenCoven is built around a different future: AI that can stay.

The next generation of AI should be durable, personal systems that remember what matters, understand their purpose, use tools, collaborate with other agents, and grow alongside the people and projects they serve.

OpenCoven brings structure to the magic by combining memory, identity, orchestration, local execution, tool access, and multi-agent collaboration into an open framework people can inspect, customize, and own.

### Philosophy
A familiar is not a faceless bot. It has a name, purpose, memory, toolset, voice, role, and place in a larger workflow. It should feel personal without pretending to be human, powerful without becoming opaque, and extensible without collapsing into chaos.

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

### Approved Logo

The approved public logo is the sigil as a white icon on a black square background. Use it for docs, README, package READMEs, landing chrome, favicons, avatars, and small identity surfaces.

Do not substitute gradient, mark-only, black-only, monoline, or external avatar images in public surfaces.

### Logo Files
```
/assets/opencoven/opencoven.svg       # Shared approved SVG source
/docs/assets/opencoven-icon.svg       # Public docs copy
/packages/cli/assets/opencoven.svg    # Package-local copy
/packages/openclaw-coven/assets/opencoven.svg
```

---

## 3. Color System

### Primary Palette

```css
/* Anchors */
--oc-black: #080808;
--oc-white: #ffffff;

/* Violet Spectrum (Signature) */
--oc-purple-1: #6a5fa0;      /* Dim / secondary */
--oc-purple-2: #9A8ECD;      /* Primary UI accent — canonical violet */
--oc-purple-3: #b8afdc;      /* Light / subtle */
--oc-purple-glow: #9A8ECD;   /* Hover states — same as primary, no glow shadow */
```

### Usage Guidelines
- **90% black / white** — compose UI on neutral foundation
- **10% violet accents** — hover states, active borders, labels, identity moments
- **No gradients. Ever.** — flat solid colors only; this rule has no exceptions
- **No glow shadows** — border-color changes on hover, not box-shadow glow
- **No blur/glass** — no backdrop-filter; panels are solid surface colors
- **Maintain contrast** — all text must meet WCAG AA minimums

### Surfaces
```css
--oc-surface-0: #080808;    /* Page background */
--oc-surface-1: #0f0f0f;    /* Cards, panels */
--oc-surface-2: #141414;    /* Elevated panels */
--oc-surface-3: #1a1a1a;    /* Hover surface */
--oc-border-subtle: rgba(255,255,255,0.06);
--oc-border-strong: rgba(255,255,255,0.10);
--oc-border-violet: rgba(154,142,205,0.18);  /* Accent borders */
```

---

## 4. Typography

### Primary Typefaces

**UI / Body (Default):**
- **Inter** — canonical cross-platform choice
- **Geist Sans** — acceptable on Vercel/Next surfaces (CovenCave default; already preloaded)
- **SF Pro Text** — Apple-platform fallback
- Fallback: system-ui stack

**Display / Brand (Headlines, Hero):**
- **Satoshi** — preferred; distinctive but technical
- **Neue Montreal** — alternative, modern, geometric
- **Geist** — fallback if Satoshi unavailable

**Mono / Code:**
- **JetBrains Mono** — canonical for all code, terminal output, session IDs, hashes, paths, labels, and badges
- **SF Mono** — Apple-platform fallback
- **Geist Mono** — acceptable on Vercel/Next surfaces (CovenCave preloaded default; swap to JetBrains Mono as user-selectable default)

### CovenCave Font Defaults
Cave exposes a user-selectable font catalog (`src/lib/font-catalog.ts`). Configured defaults:
- **Sans:** `geist` → `--font-geist-sans` (preloaded, renders immediately)
- **Mono:** `jetbrains-mono` → `--font-jetbrains-mono` (matches DESIGN.md canon)

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

## 6. Visual Style — Field Manual

OpenCoven UI uses a **field manual aesthetic**: high information density, monospace labels, ruled borders, structured grids, zero decoration. Think ops dashboard meets field report.

### Principles
- **Flat and solid** — no gradients, no blur, no glow shadows
- **Monospace for labels** — `JetBrains Mono` or `SF Mono` for all uppercase labels, badges, status text, nav items, and metadata
- **Ruled borders** — thin `rgba(255,255,255,0.06–0.10)` lines as structural dividers
- **Dense but readable** — generous line-height in body copy, tight in labels
- **Hover = border change** — `border-color` shifts on hover; never `box-shadow` glow, never `transform: scale`
- **Violet for identity** — `#9A8ECD` on labels, active states, accent borders only; not backgrounds

### Typography in UI
```
Display / headlines:  Inter, 800 weight, -0.025em tracking
Body copy:            Inter, 400 weight, 1.65 line-height
Labels / badges:      JetBrains Mono, 700, 0.14–0.18em letter-spacing, uppercase
Code / terminal:      JetBrains Mono, 400
```

### Button Style
- Sharp corners (border-radius: 4–5px)
- Monospace label, uppercase, 10–11px
- Primary: solid `#9A8ECD` fill, `#080808` text
- Secondary: transparent, subtle border
- **No pill shapes** on primary actions
- **No box-shadow on hover** — only border-color and background shift

---

## 7. Landing Page System

### Layout Structure

### Hero Section
- **Background:** `#080808` — pure flat black
- **No ambient wash divs, no radial halos, no grid overlays**
- **Headline:** Inter 800, flat `#9A8ECD` span (no gradient clip)
- **Kicker:** JetBrains Mono, 10px, 0.18em tracking, prefixed with a 20px violet rule
- **CTAs:** Sharp corners, monospace labels; primary is solid violet fill
- **Workspace card:** solid `#0f0f0f` surface, no backdrop-filter, no inner glow
- **Hero footer:** JetBrains Mono, 9px, ruled top border, muted

**Headline:**
```
OpenCoven turns AI into a living workspace.
```

**Sub-headline:**
```
Summon named agents with memory, tools, identity, roles, and continuity.
OpenCoven gives them a local runtime, shared project context,
and a place to coordinate without becoming opaque.
```

**CTAs:**
- **Primary:** "Join the Discord" (solid violet `#9A8ECD`)
- **Secondary:** "View on GitHub", "Read the field notes" (transparent, subtle border)

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
- **No glass, no blur** — solid surfaces only
- **Borders:** `rgba(255,255,255,0.06–0.10)` structural, `rgba(154,142,205,0.18)` violet accent
- **Hover states:** border-color shift only — no glow, no scale, no shadow
- **Spacing:** generous negative space between sections
- **Shadows:** none

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
- Same as X avatar

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
- Use negative space aggressively
- Let the logo carry the identity
- Maintain consistent violet accent usage (10% rule)
- Ensure all text is high contrast
- Use monospace for all labels, badges, nav items, metadata
- Sharp corners on interactive elements (4–5px radius)
- Hover = border-color change only
- Verify icon legibility at minimum 24px scale

### ❌ Don't
- **Use gradients anywhere** — no linear, no radial, no gradient text clips. Zero exceptions.
- Use `backdrop-filter` or glass blur effects
- Use `box-shadow` glow on hover or focus (use `outline` for focus rings only)
- Use pill-shaped buttons on primary actions
- Add ambient wash divs or decorative overlays
- Mix serif/sans fonts carelessly
- Use colored shadows or noise textures
- Scale logo below 24px without testing
- Create or publish alternate public logo variants
- Use the accent blue for general UI (reserve for actionable states only)

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
- [ ] Logo in header/footer: approved black-background, white-icon asset

### File Structure Template

```
/brand
  /logo
    └── source variants retained for asset work, not public docs/package usage

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
- **All public surfaces:** Approved black-background, white-icon asset
- **Small icons/badges:** Approved black-background, white-icon asset
- **Docs and diagrams:** Use the approved logo only when a logo is needed; diagrams otherwise use plain labels and lines

### Adding New Icons
1. Check `/brand/icons/` for existing similar icon
2. Match monoweight stroke (1.5–2px)
3. Align to 24px or 32px grid
4. Test at minimum 24px display size
5. Place in `/brand/icons/` with descriptive name

---

## 13. Versioning & Maintenance

**Current Version:** 1.1.0 (2026-05-24)

**Last Reviewed:** 2026-05-24
**Reviewed By:** Val + Nova

**Change Log:**
- **v1.1.0** – Field manual redesign: no-gradient rule (hard), flat violet `#9A8ECD`, JetBrains Mono for labels, solid surfaces, sharp corners, hover = border-color only. Removed all gradient/glow/glass patterns.
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
