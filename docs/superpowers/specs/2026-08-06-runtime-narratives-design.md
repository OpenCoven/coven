# Runtime narrative reconciliation design

## Goal

Reconcile current, user-facing Coven documentation with the runtime contracts
implemented by the locked source reviewed in issue #537. Preserve historical
documents as records rather than rewriting their release-time claims.

## Source-of-truth contracts

- `coven-code` is the managed, canonical interactive UI. The in-process TUI
  is available only through the explicitly set, deprecated
  `COVEN_LEGACY_TUI` escape hatch.
- Unix daemons use `<COVEN_HOME>/coven.sock`; Windows daemons use a
  per-profile, owner-only named pipe. Clients must use native local IPC and
  must not assume a Unix filesystem socket on Windows.
- Windows source support and the `@opencoven/cli-windows` package do not make
  every command implementation portable. Direct `coven kill` is Unix-only;
  Windows-capable integrations cancel live sessions through the daemon's
  `POST /api/v1/sessions/:id/kill` operation.
- Codex, Claude Code, and GitHub Copilot CLI are bundled compatibility
  defaults. Hermes 1.0.3 is a trusted, installable opt-in recipe, not a
  bundled default or a future-only integration. Grok Build and OpenCode retain
  their existing recipe maturity labels.

## Documentation changes

1. Replace prompt-first/legacy-TUI claims in entry, onboarding, Windows, and
   architecture guides with the managed-engine default. Mention the legacy
   fallback only where an operator needs the migration escape hatch.
2. Establish one cross-platform IPC vocabulary in daemon, API, state-layout,
   and architecture references: "Unix socket on Unix-like hosts; owner-only
   Windows named pipe on Windows." Keep `curl --unix-socket` examples, but
   label them Unix-only rather than implying their path is a Windows contract.
3. Correct the distribution snapshot to separate:
   - native Windows source/runtime capability;
   - the published Windows platform package;
   - release-specific wrapper resolution; and
   - intentionally unsupported direct CLI behavior.
4. Update adapter overviews, capability cards, and architecture diagrams to
   show bundled defaults, trusted opt-in recipes, and genuinely future
   adapters as separate groups.
5. Amend the `coven kill` reference with the Windows limitation and the daemon
   cancellation route, without suggesting an unavailable direct CLI fallback.

## Scope boundaries

Edit present-tense, canonical user documentation only. Do not change Rust,
npm-package behavior, historical release notes, MVP plans, archived design
documents, or generated-help claims. Do not promise a stable, hand-constructed
Windows pipe pathname: daemon status/metadata remains the discoverable endpoint
surface.

## Error handling and user guidance

Documentation must fail closed in its claims: unsupported direct Windows
operations are called out explicitly, Unix-only examples are labeled, and
adapter maturity never implies bundled support. Windows users are directed to
the daemon-owned cancellation endpoint when a compatible native IPC client is
available, rather than told that `coven kill` will work.

## Validation

Review each changed statement against the implementation and npm registry
state. Search current documentation for stale present-tense phrases such as
"prompt-first TUI," "Windows in progress," Windows package future-tense, and
Hermes-as-future claims. Run any existing documentation validation command
discovered in the repository; otherwise validate links, fenced examples, and
cross-references by inspection.
