# Final Documentation Single-Source Audit Design

## Goal

Complete the remaining documentation migration so public Coven guidance has one
authoritative home in `OpenCoven/coven-docs`, while `OpenCoven/coven` retains
only documentation that must evolve with source code or repository policy.

The initial migration in `coven-docs#49` and `coven#668` established the
ownership boundary, corrected major runtime guidance, removed empty
placeholders, and replaced broad duplicate entry pages. This design covers the
remaining substantive install, platform, onboarding, harness, help, daemon,
CLI, guide, and reference pages.

## Ownership Boundary

`coven-docs` owns public:

- installation, update, uninstall, and platform guidance;
- onboarding and supported harness setup;
- CLI and daemon operation;
- troubleshooting, diagnostics, community, and issue-filing guidance;
- public API usage, automation examples, and memory workflows.

`coven` retains:

- normative API, adapter, lifecycle, safety, and authority-boundary contracts;
- contributor, security, release, and repository policy;
- maintainer source maps and verification procedures;
- implementation specifications, plans, design records, and historical notes;
- package- and crate-specific READMEs.

A local page does not remain merely because it contains one implementation
detail. Mixed pages are split: public procedures move to `coven-docs`, while a
concise local contract or maintainer note remains only when it must change with
the source.

## Delivery Architecture

Deliver the audit in three focused waves. Each wave uses an ordered PR pair:

1. merge the canonical `coven-docs` additions and corrections;
2. rebase and merge the dependent `coven` pointer, deletion, link, and guard
   cleanup.

### Wave A: Install, Platform, and Onboarding

Audit:

- `docs/install/**`
- `docs/platforms/**`
- `docs/start/**`

Canonical targets include:

- `content/docs/guide/install.mdx`
- `content/docs/guide/getting-started.mdx`
- `content/docs/cli/install.mdx`
- `content/docs/cli/install-debugging.mdx`
- `content/docs/cli/uninstall.mdx`
- daemon lifecycle and configuration pages where service operation belongs

The initial inventory indicates that several platform and onboarding overview
pages can already become pointers, while richer package-manager, WSL2,
launchd, systemd, container, Raspberry Pi, update, and rollback procedures must
be upstreamed first.

### Wave B: Harness and Support

Audit:

- `docs/harnesses/**`
- `docs/help/**`
- directly related `docs/models/**` pages

Canonical targets include:

- `content/docs/harnesses/**`
- `content/docs/reference/troubleshooting.mdx`
- `content/docs/cli/doctor.mdx`
- `content/docs/daemon/observability.mdx`
- `content/docs/memory-models/**`

`docs/HARNESS-ADAPTERS.md` remains local as the normative adapter contract.
Public setup, missing-harness recovery, diagnostics, permissions, paths,
community, issue filing, and memory-import procedures move upstream.

### Wave C: Daemon, CLI, Guides, and API Reference

Audit:

- `docs/daemon/**`
- `docs/reference/**`
- `docs/guides/**`
- directly related top-level API, auth, safety, lifecycle, settings, client,
  and stream contracts

Canonical targets include:

- `content/docs/daemon/**`
- `content/docs/cli/**`
- `content/docs/reference/**`
- `content/docs/openapi/**`
- `content/docs/guide/**`

Normative API and authority documents remain local. Public daemon operations,
remote-access procedures, health interpretation, CLI command usage, automation
examples, and endpoint usage move upstream or become canonical pointers.

## Page Decision Matrix

Each wave maintains a page-level matrix with these fields:

| Field | Purpose |
| --- | --- |
| Local page | Repository path under audit |
| Canonical target | Existing or proposed stable `docs.opencoven.ai` route |
| Ownership | Public, normative/local, or mixed |
| Unique facts | Material not yet present in canonical docs |
| Verification source | Rust, TypeScript, package manifest, workflow, or policy source |
| Inbound links | Repository files and tests that reference the local page |
| Disposition | Keep, upstream then pointer, pointer/delete, or correct in place |
| Status | Audited, upstreamed, canonical merged, cleanup merged |

The matrix is an implementation artifact, not a new permanent documentation
system. Its final decisions should be reflected in the changed pages, PR
descriptions, and the persistent goal.

## Decision Rules

For every substantive page:

1. Classify the page by ownership rather than filename.
2. Verify version-sensitive commands, flags, paths, support claims, and
   platform behavior against current source.
3. Compare sections and facts against canonical docs; topical similarity alone
   is not sufficient evidence of duplication.
4. Choose one disposition:
   - **Keep local:** normative, policy, maintainer, implementation, or
     historical material.
   - **Upstream then pointer:** useful public facts are absent or materially
     weaker in canonical docs.
   - **Pointer/delete now:** canonical coverage is complete and stable.
   - **Correct in place:** retained local material contains stale or invalid
     claims.
5. Do not remove or reduce a page until its canonical target is merged.
6. Update every inbound link and documentation guard in the same cleanup PR.

Short local pointers are acceptable when repository readers need a stable
entry point. Delete pages only when no repository navigation, source contract,
or compatibility reason requires the path to remain.

## Canonical Content Rules

Add missing facts to the narrowest coherent existing page. Create a new
canonical page only when combining the topic with an existing page would make
that page ambiguous or difficult to navigate.

Canonical additions must:

- describe current supported behavior, not historical implementation intent;
- use cross-platform terminology where behavior differs;
- label Unix-only commands and examples;
- avoid version-sensitive package claims unless verified during the change;
- link back to local normative contracts when public guidance depends on them;
- avoid copying maintainer-only source maps or implementation history.

## Local Contract Reduction

When a mixed page is reduced:

- retain the invariant, compatibility rule, ownership boundary, or maintainer
  verification procedure;
- remove step-by-step public setup and troubleshooting prose after it is
  canonical;
- add a direct canonical link for operational examples;
- preserve phrases required by existing contract guards unless the guard is
  deliberately migrated to a more appropriate local contract;
- do not turn implementation plans or historical records into present-tense
  public guidance.

## Link and Guard Migration

Before deleting or replacing a local page:

1. enumerate Markdown, workflow, issue-template, script, and package links;
2. retarget public navigation to a stable canonical route;
3. retarget source-adjacent references to the retained local contract;
4. update tests that incorrectly require obsolete public pages;
5. keep guards that enforce normative phrases, but move those phrases to the
   correct local contract when necessary.

A cleanup PR must not reintroduce duplicate prose solely to satisfy a stale
test. The test should assert the new ownership boundary.

## Validation

For each `coven-docs` PR:

- run the existing production build;
- run link validation;
- run topic-specific documentation checks;
- regenerate and validate OpenAPI artifacts when the source schema or generated
  endpoint guidance changes.

For each `coven` cleanup PR:

- run targeted documentation and onboarding guards;
- run API-contract documentation checks when API pages change;
- run the secret scan and staged privacy guard;
- run the existing package smoke checks when their documentation assertions
  change;
- wait for the complete required CI matrix before merge.

Each PR review must also confirm:

- every removed page has no unresolved inbound link;
- every external pointer uses a stable canonical route;
- all verified unique public facts exist in the merged canonical page;
- retained local pages state a source-adjacent ownership reason.

## Failure Handling and Ordering

Canonical and cleanup PRs cannot merge out of order. If a canonical target is
missing, disputed, or fails validation, the local page remains unchanged.

If source verification disproves a public claim, correct the authoritative
documentation rather than upstreaming the stale text. If a topic has no clear
canonical home, record it as an explicit exception and retain the local page
until a coherent destination is approved.

Each wave is independently reviewable and reversible. A failure in one wave
does not block already-correct ownership decisions in another wave.

## Completion Criteria

The documentation single-source goal is complete when:

- every substantive page in the target directories has an explicit audited
  disposition;
- all accepted canonical additions and dependent cleanups are merged;
- no repository link or test treats a removed public page as canonical;
- remaining local pages have a clear normative, policy, maintainer,
  implementation, historical, or package-specific reason to exist;
- no known current behavior is documented differently across the two
  repositories.

## Non-Goals

- Rewriting implementation plans, historical release notes, or design records.
- Moving normative source contracts into the public docs repository.
- Adding speculative harnesses or unsupported platform behavior.
- Renaming product surfaces or redesigning the documentation site.
- Replacing all repository documentation with external links.
