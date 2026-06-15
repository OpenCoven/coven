---
name: opencoven-design
description: Agent-operable design constraints for OpenCoven, Coven, harness cockpits, docs, and integration surfaces
style: dark
accent: violet
source_of_truth: DESIGN.md
---

# OpenCoven Design Skill

Use this skill when designing or implementing OpenCoven/Coven interfaces, docs pages, diagrams, package README visuals, native companion surfaces, or agent-built UI components.

OpenCoven should feel like collective intelligence plus controlled power: arcane but precise, technical not gimmicky, powerful not loud, minimal but symbolic.

Core positioning: OpenCoven is an open ecosystem for persistent AI familiars: named agents with memory, tools, identity, roles, and continuity. It turns AI from a blank chatbox into a living workspace of agents that remember, coordinate, and belong to the user.

## Source Of Truth

Read these local files before changing brand or UI behavior:

- `DESIGN.md` - canonical brand and design system
- `docs/BRAND.md` - brand asset index and usage summary
- `docs/BRANDING-ADHERENCE.md` - current exceptions, risks, and hardening checklist
- `brand/docs/BRAND-USAGE.md` - contributor PR checklist
- `brand/ui/color-tokens.css` - canonical CSS color tokens
- `brand/ui/typography.css` - canonical typography tokens

Do not invent a separate design system. If this skill conflicts with `DESIGN.md`, follow `DESIGN.md` and update this skill.

## Brand Position

Build for harness operators and developers, not a marketing toy.

- The default feeling is a quiet command cockpit.
- The product promise is durable AI systems that can stay: remember what matters, understand their purpose, use tools, coordinate with other agents, and remain understandable over time.
- A familiar is not a faceless bot. It has a name, purpose, memory, toolset, voice, role, and place in a larger workflow.
- Keep the philosophy clear: powerful without becoming opaque, personal without pretending to be human, extensible without collapsing into chaos.
- Mystical symbolism is allowed only when it supports identity: sigil, trident, flame, crescents, nodes, execution paths.
- Interfaces should be dense, legible, and repeatable under daily use.
- Copy should be direct and capable. Avoid hype, jokes, and vague AI language.
- Use `OpenCoven` for the ecosystem/org and `Coven` for the CLI/daemon/product.
- The command is always `coven`, never `opencoven`.

## Color Tokens

Use the repo tokens, not arbitrary hex values.

```css
--oc-black: #000000;
--oc-white: #ffffff;

--oc-purple-1: #6E4BFF;
--oc-purple-2: #8A63FF;
--oc-purple-3: #A78BFF;
--oc-purple-glow: #7C5CFF;

--oc-accent-blue: #0A84FF;
--oc-danger: #FF3B30;
--oc-success: #30D158;

--oc-surface-0: var(--oc-black);
--oc-surface-1: #050507;
--oc-surface-2: #080812;
--oc-border-subtle: rgba(255, 255, 255, 0.08);
--oc-border-strong: rgba(255, 255, 255, 0.14);
--oc-text: rgba(255, 255, 255, 0.94);
--oc-text-muted: rgba(255, 255, 255, 0.64);
--oc-text-faint: rgba(255, 255, 255, 0.42);
```

### Color Rules

- Use black and white for roughly 90 percent of the composition.
- Reserve purple for identity, primary CTAs, focus, selected state, execution paths, and small highlights.
- Use `--oc-accent-blue` only for documented actionable/system states. Do not make blue the brand accent.
- Use `--oc-danger` for destructive actions and hard errors.
- Use `--oc-success` for completion, connected, passing, and healthy states.
- Keep text WCAG AA contrast on every surface.

## Signature Treatments

Gradients and glow are allowed, but only as controlled brand moments.

```css
--oc-gradient-signature: linear-gradient(135deg, var(--oc-purple-1), var(--oc-purple-3));
--oc-radial-glow: radial-gradient(circle, rgba(138, 99, 255, 0.28) 0%, rgba(138, 99, 255, 0) 68%);
--oc-focus-ring: 0 0 0 2px rgba(124, 92, 255, 0.52), 0 0 32px rgba(124, 92, 255, 0.28);
--oc-hover-glow: 0 0 36px rgba(124, 92, 255, 0.26);
```

Use signature treatment for:

- Primary logo and hero/OG assets
- Focus rings and interactive affordances
- Hover glow on CTAs or selected execution paths
- Subtle radial glow behind the sigil

Do not use gradients or glows for:

- General cards
- Tables
- Dense cockpit panels
- Documentation diagrams, except small path highlights
- Decorative page backgrounds

## Typography

Use the repo typography tokens.

```css
/* UI / body — Inter is canonical; Geist Sans is fine on Vercel/Next surfaces */
--oc-font-ui: Inter, "SF Pro Text", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;

/* Display — headlines and hero copy only */
--oc-font-display: Satoshi, "Neue Montreal", Geist, Inter, "SF Pro Display", -apple-system, BlinkMacSystemFont, system-ui, sans-serif;

/* Mono — JetBrains Mono is canonical; SF Mono is the Apple fallback */
--oc-font-mono: "JetBrains Mono", "SF Mono", SFMono-Regular, ui-monospace, Menlo, Monaco, Consolas, "Liberation Mono", monospace;

--oc-tracking-heading: -0.02em;
--oc-tracking-hero: -0.055em;
--oc-tracking-label: 0.14em;
--oc-line-body: 1.4;
--oc-line-heading: 1.2;
```

### CovenCave Defaults

Cave (`coven-cave`) uses a user-selectable font catalog. The configured defaults:
- **Sans:** `geist` — `--font-geist-sans`, preloaded, renders immediately
- **Mono:** `jetbrains-mono` — `--font-jetbrains-mono`, matches DESIGN.md canon

### Typography Rules

- Use UI font for product surfaces, controls, tables, body text, and docs prose.
- Use display font for hero headlines and major product headings only.
- Use mono font for ALL of: commands, session IDs, logs, event streams, diffs, paths, hashes, API payloads, terminal output, and uppercase labels / badges.
- JetBrains Mono is the canonical mono face — prefer it over SF Mono, Geist Mono, or system mono on any surface where you control the font.
- Keep body line height at 1.4 and headings at 1.2.
- Avoid all-caps except for compact labels, badges, and nav group headings.
- Do not add playful, cursive, serif, or decorative fonts.

## Spacing And Shape

- Base spacing is a 4px grid.
- Dense cockpit UIs should favor 4px, 8px, 12px, and 16px increments.
- Documentation and landing pages may use more breathing room, but still stay on grid.
- Cards and panels should use restrained radii: 4px to 8px unless a platform style requires otherwise.
- Do not nest cards inside cards.
- Use borders and alignment before shadows.

Recommended dimensions:

```
Button heights: 24px compact, 32px standard, 40px large
Input height: 32px standard
Table rows: 32px dense, 40px comfortable
Sidebar: 280px standard, 320px expanded
Modal: 400px compact, 500px standard, 600px wide
Icon grid: 24px or 32px
```

## Layout Patterns

### Harness Cockpit

Use for session managers, operator dashboards, run queues, agent control rooms, and debug tools.

- Left navigation or lane rail, main work area, optional right inspector.
- Dense rows with clear status badges.
- Mono identifiers for sessions, agents, tools, commits, ports, and paths.
- Explicit empty, loading, error, and disconnected states.
- No oversized marketing hero inside operational UI.

### Session Browser

Use tab-like navigation for long-lived work.

- Tabs or session pills show active work.
- Preserve browser-like history where relevant: back, forward, reopen, recent sessions.
- Active session should be obvious through border, text, and selected state, not color alone.
- Destructive session actions should be visible but gated.
- Use product ritual labels exactly where they exist: **Rejoin**, **View Log**, **Summon**, **Archive**, and **Sacrifice**.
- **Archive** is reversible and keeps the ledger.
- **Summon** restores an archived session to the active list.
- **Sacrifice** is permanent, refuses running sessions, and requires explicit confirmation such as `--yes` or typed confirmation.
- In terminal UIs, `coven sessions` should favor a human browser; `coven sessions --plain` stays scriptable.

### Documentation And Landing

- Landing hero: black foundation, centered sigil, restrained purple glow, literal OpenCoven/Coven offer.
- Docs: calm technical rhythm, readable code blocks, no decorative clutter.
- Diagrams: nodes, paths, monoline icons, purple execution highlights.
- Keep the next section visible below a landing hero on normal mobile and desktop viewports.
- Be honest about early MVP status where relevant. Do not imply production maturity beyond the docs.

### Repair And Trust Surfaces

- Repair flows should show selected repo root, branch, dirty files, untracked files, harness, and verification profile before launch.
- Dirty repos require a visible warning and confirmation.
- v0 repair flows never commit, push, or operate outside the selected repo root.
- Treat the Rust CLI/daemon as the authority boundary for project-root validation, PTY lifecycle, socket API, input/kill, and persistence.
- Clients may improve UX, but they must not visually imply authority they do not have.

## Components

### Buttons

- Primary: purple accent, white text, restrained hover glow.
- Secondary: dark surface, subtle border, white or muted text.
- Ghost: transparent, clear hover surface.
- Danger: danger token, explicit label, confirmation for irreversible actions.
- Icon buttons should use familiar icons and tooltips when meaning is not obvious.
- Hover should not scale layout.

### Inputs

- Dark surface, subtle border, clear placeholder color.
- Focus must use visible purple ring or equivalent tokenized focus treatment.
- Error state must include border and text message.
- Disabled and readonly states must be visually distinct.

### Panels And Cards

- Use for real grouped content, repeated items, modals, and framed tools.
- Background: `--oc-surface-1` or `--oc-surface-2`.
- Border: `--oc-border-subtle` or `--oc-border-strong`.
- Prefer dividers and alignment over shadows.
- Do not use glass-heavy blur.

### Tables And Lists

- Header labels should be compact, muted, and scannable.
- Rows should support hover, selected, focused, loading, and error states.
- Use mono for machine data.
- Avoid zebra striping unless density requires it.

### Badges

- Use badges for run status, agent state, verification result, branch state, auth state, and risk level.
- Keep badges compact.
- Pair color with text, never color alone.

### Modals And Confirmations

- Use modals for irreversible or high-risk choices.
- Dialog copy should name the action and affected resource.
- Prefer archive/stop/disable before delete where the product supports recovery.
- Danger action must not be the default focused action.

## Interaction States

Every interactive element needs these states:

- Default
- Hover
- Active or pressed
- Focus visible
- Disabled
- Loading or pending where async work exists
- Error where user recovery is possible

Keyboard navigation must remain visible. Never remove focus indicators without a tokenized replacement.

## Motion

- Motion should clarify state changes, not decorate the page.
- Standard transition: 150ms to 250ms.
- Page or path drawing motion may run 600ms to 1200ms when it explains graph/execution flow.
- Respect `prefers-reduced-motion`.
- Hover glow is preferred over scale for brand CTAs.
- Do not animate body text or cause layout shift.

## Iconography

- Use monoweight, geometric, slightly sharp icons.
- Use existing icons from `brand/icons/` when possible.
- Use trident tips, crescents, flame curves, nodes, and connection lines only as meaningful motifs.
- Avoid emoji and whimsical icons in system UI.
- Test logo and icon legibility at 24px minimum.

## Logo Rules

- Public surfaces use the approved black-background, white-icon logo.
- Use `assets/opencoven/opencoven.svg` as the shared SVG source.
- Use `docs/assets/opencoven-icon.svg` for the generated docs site.
- Package READMEs use package-local copies of the approved SVG.
- Preserve aspect ratio and clear space.
- Do not create, publish, or link alternate public logo variants.

## Accessibility

- Hit targets should be large enough for pointer use: 32px minimum for dense tools, 40px preferred for primary actions.
- Do not rely on color alone for status, risk, or selection.
- Keep focus order predictable through rails, tabs, tables, dialogs, and inspectors.
- Modals must trap focus while open and restore focus when closed.
- Every icon-only control needs an accessible name and tooltip where helpful.

## Verification

Before claiming a UI or brand change follows this skill:

- Check changed colors against `brand/ui/color-tokens.css`.
- Check typography against `brand/ui/typography.css`.
- Inspect desktop and mobile layouts when a visual surface changed.
- Verify focus, hover, disabled, loading, empty, and error states for touched interactive components.
- Run the smallest relevant project gate: typecheck, lint, build, screenshot, or direct visual inspection.
- Record intentional exceptions in `docs/BRANDING-ADHERENCE.md`.

## MUST

- Use `DESIGN.md` and `brand/ui/*.css` as source of truth.
- Build dark-first on pure black foundations.
- Keep UI technical, minimal, and symbolic.
- Use `OpenCoven`, `Coven`, and `coven` with their exact product meanings.
- Keep purple controlled and meaningful.
- Use tokenized colors and typography.
- Provide visible focus states.
- Provide empty, loading, disabled, error, and success states where applicable.
- Gate destructive actions.
- Preserve Archive/Summon/Sacrifice semantics in session UI.
- Respect the Rust daemon as the authority boundary in trust-sensitive UI.
- Keep dense operator surfaces scannable.
- Use mono for operational data.

## SHOULD

- Use borders and spacing before shadows.
- Prefer tabs, rails, split panes, inspectors, and command surfaces for harness workflows.
- Use status badges for session/agent/tool state.
- Keep page copy short and concrete.
- Use diagrams to explain orchestration only when they clarify relationships.
- Document any intentional exception in `docs/BRANDING-ADHERENCE.md`.

## NEVER

- Do not use light mode as the primary OpenCoven experience.
- Do not use random gradients, noise textures, bokeh, or decorative blobs.
- Do not overuse purple; it should feel intentional, not saturated.
- Do not use blue, green, or red as brand accents.
- Do not hide focus rings.
- Do not use heavy blur/glass effects.
- Do not use shadows as the main structure.
- Do not make marketing heroes inside operator tools.
- Do not add playful fonts, emoji UI, or novelty iconography.
- Do not place text over complex backgrounds.
- Do not create orphan color values when a token exists.

## Prompt Pattern

Use this pattern when assigning UI work to an agent:

```markdown
Build [surface/component] for OpenCoven.
Follow skills/opencoven-design/SKILL.md and the canonical brand files:
- DESIGN.md
- brand/ui/color-tokens.css
- brand/ui/typography.css

The surface must include:
- [required states]
- [required interactions]
- [data density / layout constraints]

Do not invent new colors, logo variants, or decorative effects.
Return changed files, verification run, and any brand exceptions.
```

## Review Checklist

- Colors come from `--oc-*` tokens or documented semantic aliases.
- Typography uses `--oc-font-ui`, `--oc-font-display`, or `--oc-font-mono`.
- Purple is limited to identity, focus, selected state, CTAs, and execution highlights.
- Hover states glow or change border/surface; they do not scale layout.
- Dense tools are scannable and not card-heavy.
- Destructive actions are explicit and gated.
- Focus, disabled, loading, empty, and error states are present.
- Logo usage stays on the approved black-background, white-icon asset.
- Any exception is documented in `docs/BRANDING-ADHERENCE.md`.

---

Last updated: 2026-06-13
Version: 1.2.0
License: MIT
