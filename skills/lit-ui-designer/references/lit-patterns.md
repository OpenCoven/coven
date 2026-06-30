# Lit patterns reference

Sources checked 2026-04-29:
- Lit components overview: https://lit.dev/docs/components/overview/
- Lit reactive properties: https://lit.dev/docs/components/properties/
- Lit styles: https://lit.dev/docs/components/styles/
- Lit templates overview: https://lit.dev/docs/templates/overview/

## Component model

A Lit component is a custom element with state, rendering, styles, lifecycle, and standard element APIs. Design reusable components as real elements with clear public API boundaries.

## Properties and state

- Public reactive properties are component API/input. Treat them as owner-controlled unless the component is explicitly a controlled/uncontrolled form-like control.
- Internal reactive state should be private/protected and marked with `@state()` or `state: true`.
- When user interaction changes a meaningful value, dispatch an event rather than only mutating internal state.
- Reflect attributes sparingly. Reflect when CSS/selectors, SSR, accessibility, or external consumers need the attribute; avoid reflecting rich objects/arrays.
- Boolean attributes follow HTML semantics: attribute presence means true.

## Rendering

- Keep `render()` declarative and readable. Break out private render helpers for complex regions.
- Prefer conditionals and directives over manual DOM mutation.
- Use stable keys when rendering reorderable lists.
- Do not read layout in render; use lifecycle callbacks or observers carefully.

## Styles and theming

- Prefer `static styles = css\`...\`` for performance and shadow DOM scoping.
- Share reusable style modules as exported `css` results.
- Use CSS custom properties for themes, density, color, radius, shadow, and per-instance customization.
- Export `part`s deliberately when consumers need styling hooks; document them.
- Avoid `unsafeCSS` except for fully trusted constants. Never feed it user, URL, database, or remote content.
- Avoid dynamic `<style>` in templates except for narrow cases; it can force CSS re-parsing and make performance worse.

## Events

- Use semantic event names (`selection-change`, `request-close`, `value-change`) instead of implementation names (`clicked-row`).
- Use `CustomEvent` with a typed `detail` object.
- Set `bubbles: true` and `composed: true` when the event must cross the shadow root.
- For cancelable workflows, set `cancelable: true` and respect `event.defaultPrevented`.

## Shadow DOM gotchas

- Test focus behavior; consider `delegatesFocus` only when it improves real keyboard UX.
- IDs inside shadow roots do not work as document-global references for outside elements.
- Slotted content is owned by the light DOM; style it with `::slotted()` only for shallow selectors.
- ARIA relationships across shadow boundaries require hands-on verification.

## Minimal component shape

```ts
import { LitElement, css, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';

@customElement('x-command-card')
export class CommandCard extends LitElement {
  static styles = css`
    :host { display: block; }
    button:focus-visible { outline: 2px solid var(--focus-ring, CanvasText); outline-offset: 3px; }
  `;

  @property({ type: String }) title = '';
  @property({ type: Boolean, reflect: true }) selected = false;
  @state() private _busy = false;

  render() {
    return html`
      <article class="card" aria-busy=${this._busy ? 'true' : 'false'}>
        <h3>${this.title}</h3>
        <button @click=${this.#select} ?disabled=${this._busy}>Select</button>
      </article>
    `;
  }

  #select() {
    this.dispatchEvent(new CustomEvent('selection-change', {
      detail: { selected: true },
      bubbles: true,
      composed: true,
    }));
  }
}
```
