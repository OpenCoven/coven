---
name: lit-ui-designer
description: Design, build, review, or polish world-class Lit/LitElement web component UIs. Use for Lit components, custom elements, Web Components, shadow DOM styling, design systems/tokens, professional UI polish, accessibility/WCAG reviews, keyboard interactions, ARIA patterns, responsive states, and production-quality TypeScript/CSS implementations.
---

# Lit UI Designer

Use this skill to create or improve Lit web component interfaces that feel professional, accessible, robust, and production-ready.

## Workflow

1. **Inspect before inventing.** Read the existing component, styles, tokens, tests/stories, screenshots, and surrounding UX. Preserve repo conventions unless they are clearly broken.
2. **Define the UX contract.** Identify primary user goal, component states, keyboard/mouse/touch behavior, responsive behavior, empty/loading/error/disabled states, and integration boundaries.
3. **Choose native semantics first.** Prefer real HTML controls (`button`, `a`, `input`, `dialog`, headings, lists, tables) before ARIA. Add ARIA only when native semantics cannot express the pattern.
4. **Implement Lit idiomatically.** Keep public properties as external API, private `@state` for internals, declarative `render()`, `classMap`/`styleMap` when helpful, and custom events for user-driven state changes.
5. **Design with craft.** Use clear hierarchy, spacing rhythm, restrained color, meaningful motion, strong focus states, and polished empty/error states. Avoid generic “AI dashboard” slop.
6. **Verify.** Run the smallest meaningful gates: typecheck/test/build, accessibility checks if present, and browser/screenshot review for meaningful visual work.

## Lit implementation standards

- Use TypeScript and decorators when the repo already uses them; otherwise follow local Lit style.
- Keep reactive public props as inputs. Do not silently mutate owner-controlled props except for user-driven control patterns; emit a composed/bubbling event to announce changes.
- Use `@state()` or `state: true` for private reactive state.
- Keep expensive derived values outside hot render paths or memoize when needed.
- Use stable keys/repeat directives for lists that reorder or update often.
- Use `static styles = css\`...\`` for component styles. Prefer CSS custom properties for theming and per-instance variation.
- Avoid dynamic `<style>` blocks and `unsafeCSS`; use them only when the value is fully trusted and there is no safer design-token/custom-property option.
- Use `:host`, `:host([attr])`, `::slotted`, and CSS parts intentionally. Document exported parts and CSS custom properties when creating reusable components.
- Dispatch events with `bubbles: true` and `composed: true` when parents outside the shadow root must handle them.

## Accessibility bar

Target WCAG 2.2 AA unless the repo specifies stricter requirements.

- Keyboard: every interactive path works without a mouse; focus order is logical; Escape/Enter/Space/Arrow behavior matches the widget pattern.
- Focus: visible, high-contrast focus indicators; never remove outlines without replacement.
- Names: every icon-only control has an accessible name; form inputs have labels; status regions are announced when appropriate.
- Contrast: meet AA for text and meaningful UI indicators; do not encode meaning by color alone.
- Motion: respect `prefers-reduced-motion`; keep transitions purposeful and short.
- Shadow DOM: remember labels, focus delegation, slots, composed events, and ARIA relationships can behave differently across shadow boundaries. Test actual behavior.
- ARIA: if implementing a complex widget, follow WAI-ARIA APG keyboard and role guidance exactly; do not sprinkle roles as decoration.

## Professional UI checklist

- Layout has a clear information hierarchy: title, primary action, secondary actions, metadata, content.
- Spacing follows a consistent scale; alignment is deliberate; density matches the product surface.
- Loading, empty, error, offline, disabled, hover, active, selected, and focus states are designed.
- Copy is specific and calm; labels use verbs for actions and nouns for destinations.
- Components are responsive by design, not patched after the fact.
- Visual effects support comprehension; avoid gratuitous blur/glow/gradient noise.
- Dark mode and high-contrast modes are considered when the product supports them.

## Review protocol

When reviewing or polishing Lit UI, report findings in this order:

1. **Blocking accessibility or correctness issues**
2. **Lit/web-component architecture issues**
3. **Visual craft and UX polish**
4. **Nice-to-have improvements**

For implementation work, prefer a small, shippable slice over a sprawling redesign. If a full redesign is needed, create a plan and one representative component first.

## References

Read only as needed:

- `references/lit-patterns.md` — Lit architecture, properties, events, styles, and component APIs.
- `references/accessibility-checklist.md` — accessibility checks for shadow DOM, custom controls, keyboard UX, and ARIA.
- `references/design-quality.md` — professional visual/UI quality standards and anti-slop guidance.
