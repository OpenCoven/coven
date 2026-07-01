# Lit accessibility checklist

Sources checked 2026-04-29:
- W3C WCAG overview: https://www.w3.org/WAI/standards-guidelines/wcag/
- WAI-ARIA Authoring Practices patterns: https://www.w3.org/WAI/ARIA/apg/patterns/

Use WCAG 2.2 AA as the default target. WCAG is organized around perceivable, operable, understandable, and robust content with testable success criteria.

## Native-first rules

- Use `button` for actions, `a[href]` for navigation, `label` for controls, headings for structure, lists for lists, and tables for tabular data.
- Do not use `div role="button"` unless there is a compelling reason; if used, implement keyboard activation and focus manually.
- Prefer native `<dialog>` only when the repo/browser target supports the needed behavior; still verify focus trap, close behavior, and labelling.

## Keyboard

- Tab reaches every interactive control once, in visual/logical order.
- Shift+Tab works.
- Enter/Space activate buttons and toggles.
- Arrow keys match APG for tabs, menus, listboxes, grids, sliders, and trees.
- Escape closes popovers/menus/dialogs without destroying unrelated state.
- Focus is restored after closing transient UI.

## Names, roles, values

- Icon-only controls have `aria-label` or visible text via `aria-labelledby`.
- Toggles expose state with native checked state or `aria-pressed`/`aria-expanded`/`aria-selected` as appropriate.
- Menus, tabs, comboboxes, listboxes, grids, trees, and sliders follow APG patterns exactly.
- Live updates use `aria-live` only for information users need immediately; avoid noisy announcements.

## Shadow DOM checks

- Verify accessible names with actual browser accessibility snapshots when possible.
- Check label association for inputs inside and outside the shadow root.
- Use composed/bubbling events for parent-level form or app orchestration.
- If using slots, verify fallback content and slotted interactive children remain keyboard reachable.

## Visual accessibility

- Text and meaningful indicators meet AA contrast.
- Focus indicators are visible in light/dark themes.
- Hit targets are comfortably sized for touch where applicable.
- Layout supports zoom/reflow and does not rely on fixed pixel heights for text-heavy content.
- Motion respects `prefers-reduced-motion`.

## Review commands to prefer when available

- Existing repo a11y tests or Playwright accessibility tests.
- Browser screenshot plus keyboard walkthrough.
- `axe`/`@axe-core/playwright` if already in the repo; do not add dependencies without project approval.
