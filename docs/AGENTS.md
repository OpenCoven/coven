# AGENTS.MD

Source-of-truth for the Coven docs site (`docs.opencoven.ai`, eventual destination).

## Rule

- Author English docs in this directory.
- Locale pages and search bundles are generated artifacts; do not hand-edit when a sync workflow is in place.
- Emergency generated-output edits need explicit owner approval and a follow-up source/workflow fix.

## Editable Here

- `index.md` and every page under the tabs declared in `docs.json`.
- `docs.json` — but only as part of a documented navigation change.
- `style.css` and `nav-tabs-underline.js` (visual treatment).
- `AGENTS.md`, `README.md`.
- `assets/`, `images/`, `snippets/`.

## Do Not Edit Here

- Generated locale output under `docs/<locale>/**` once translation workflows ship.
- Generated search bundle output (Pagefind / equivalent).
- Sync metadata files (e.g., `.opencoven-sync/source.json`).

## Style

- Frontmatter: `summary`, `read_when` (bulleted), and `title` for every page.
- Lead with a one-paragraph orientation; then components, then deep reference.
- Mintlify components are preferred over raw HTML: `<Columns>`, `<Card>`, `<Steps>`, `<Step>`, `<Tabs>`, `<Tab>`, `<Accordion>`, `<Tip>`, `<Note>`, `<Info>`, `<Warning>`, `<Frame>`.
- Mermaid is fine for runtime topology, lifecycle, and authority diagrams.
- Code blocks: prefer real CLI commands and real socket calls. Avoid placeholders that look like real syntax.

## Canonical language

- Ecosystem/org: **OpenCoven**.
- Product/daemon/CLI: **Coven**.
- Command: `coven`.
- CLI package: `@opencoven/cli`.
- OpenClaw plugin package: `@opencoven/coven`.
- Trust posture: **same-user local access** to the Unix socket. No OAuth/JWT/cookies on the daemon.
- Familiar vs. harness: a **familiar** is the product concept (named agent with memory/identity/role); a **harness** is the technical concept (a coding-agent CLI).

## Workflow

- Land docs changes in the same PR as the code that changes.
- Run `coven doctor` examples against the latest CLI before pasting their output.
- When in doubt, link forward to the canonical page rather than re-stating fields.
