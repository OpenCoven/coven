# Professional Lit UI design quality

## Design principles

- Make the main job obvious within three seconds.
- Give every surface a hierarchy: title, primary content, primary action, secondary actions, supporting metadata.
- Use one spacing scale and one radius/shadow language per surface.
- Prefer fewer, stronger accents over many saturated colors.
- Design state, not just the happy path.
- Use motion to explain causality, not to decorate.

## State inventory

For every meaningful component, check:

- default
- hover
- active/pressed
- focus-visible
- selected/current
- disabled/read-only
- loading/skeleton
- empty
- error
- offline/unavailable
- permission denied
- reduced motion
- narrow viewport
- high contrast/dark mode if supported

## Anti-slop rules

- Do not ship generic gradients, random glassmorphism, meaningless glowing orbs, or placeholder dashboard cards unless the product language explicitly calls for them.
- Do not use “Lorem ipsum” in product UI examples; use realistic domain text.
- Do not hide broken hierarchy behind animation.
- Do not add ARIA roles to make inaccessible custom controls seem accessible.
- Do not change global design tokens casually; prefer component-level changes unless the task is system-wide design work.

## Lit-specific visual craft

- Surface theming via custom properties rather than one-off hardcoded overrides.
- Keep host display predictable (`:host { display: block; }` for block components, inline only when intentional).
- Use `box-sizing: border-box` inside component scopes.
- Ensure slotted content has sensible defaults but remains consumer-controlled.
- Expose `part` names for reusable internals that consumers reasonably need to style.

## Handoff format

When a UI change is done, summarize:

- what changed visually
- what changed behaviorally
- accessibility decisions
- verification run
- screenshots/previews if available
- follow-up polish that was intentionally left out
