# OpenCoven Documentation

This directory holds the product and architecture notes for Coven, the OpenCoven harness substrate.

OpenCoven is an open ecosystem for persistent AI familiars: named agents with memory, tools, identity, roles, and continuity. Coven is the local harness substrate that helps those systems run inside explicit project boundaries.

The guiding promise: OpenCoven turns AI from a blank chatbox into a living workspace of agents that remember, coordinate, and belong to the user.

## Two layouts in this folder

Coven's docs are in transition. Both layouts live here today:

1. **Mintlify-style scaffold** (new). The page tree under `docs.json` powers a future `docs.opencoven.ai` site that mirrors the OpenClaw docs structure. Entry points:
   - [`index.md`](index.md) — landing page.
   - [`docs.json`](docs.json) — navigation tabs and groups.
   - [`start/`](start/), [`install/`](install/), [`harnesses/`](harnesses/), [`familiars/`](familiars/), [`sessions/`](sessions/), [`memory/`](memory/), [`rituals/`](rituals/), [`capabilities/`](capabilities/), [`tools/`](tools/), [`automation/`](automation/), [`plugins/`](plugins/), [`models/`](models/), [`platforms/`](platforms/), [`daemon/`](daemon/), [`reference/`](reference/), [`help/`](help/).
   - [`AGENTS.md`](AGENTS.md) — contributor rules for the docs site.
2. **Flat canonical notes** (legacy, still authoritative for engineering). The all-caps files listed below remain the source-of-truth for the current Coven implementation. The Mintlify scaffold absorbs them over time.

## Start here

- [Getting started](GETTING-STARTED.md) — first install, first daemon, first session, and contributor checks.
- [Concepts](CONCEPTS.md) — core nouns: harness, project root, session, daemon, client, control plane, and rituals.
- [Glossary](GLOSSARY.md) — short definitions for recurring product and architecture terms.
- [Public roadmap](ROADMAP.md) — community-facing progress snapshot across Coven, comux, and integrations.
- [comux + Coven demo loop](COMUX-DEMO-LOOP.md) — Coven-side CLI/API contract for the visible comux cockpit flow.
- [Product spec](PRODUCT-SPEC.md) — what Coven is and what belongs in MVP.
- [Architecture diagrams](ARCHITECTURE.md) — runtime topology, session lifecycle, and authority boundary diagrams.
- [Session lifecycle](SESSION-LIFECYCLE.md) — launch, attach/replay, archive, summon, sacrifice, orphan recovery, and events.
- [Authentication and local access](AUTH.md) — current same-user Unix-socket access model, provider-auth boundary, and hardening gaps.
- [Local API](API.md) — versioned Unix-socket API contract for comux, OpenClaw, and other clients.
- [Operational model](OPERATIONAL-MODEL.md) — authority boundaries between Rust, comux, OpenClaw, and npm wrappers.
- [Safety model](SAFETY-MODEL.md) — local trust boundary, secret handling, socket posture, and automation approvals.
- [Client integration guide](CLIENT-INTEGRATION.md) — expectations for comux, OpenClaw, OpenMeow, desktop clients, and future control rooms.
- [Harness adapter guide](HARNESS-ADAPTERS.md) — current Codex/Claude adapter shape and the bar for future harness support.
- [Troubleshooting](TROUBLESHOOTING.md) — common setup, daemon, harness, session, API, and verification problems.
- [MVP plan](MVP-PLAN.md) — implementation plan and success criteria.
- [Future harnesses](FUTURE-HARNESSES.md) — adapter direction after Codex and Claude Code.
- [Brand assets](BRAND.md) — canonical logo, token, typography, and social asset pack.
- [Brand adherence checklist](BRANDING-ADHERENCE.md) — implementation progress, exceptions, and risks.
- [Documentation maintenance](DOCS-MAINTENANCE.md) — public-doc rules, canonical names, secret avoidance, and verification checks.

## OpenClaw rescue loop

Coven can help repair a local OpenClaw source checkout without relying on a healthy OpenClaw runtime:

```sh
coven patch openclaw
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
coven patch openclaw --repo ~/Documents/GitHub/openclaw/openclaw --harness codex --dry-run
```

The guided flow detects the repo, asks what is broken, launches a supervised Codex or Claude Code session, runs verification, and reports changed files. Coven does not commit or push in v0.

## Documentation stance

Keep these docs aligned with VMUX-style clarity while staying specific to OpenCoven:

- Short public-facing README first.
- Concrete quick-start commands.
- Explicit local trust boundary.
- Clear relationship to comux and OpenClaw.
- Consistent OpenCoven positioning: persistent familiars, user-owned systems, memory, identity, tools, orchestration, and multi-agent collaboration.
- Be precise about npm status: packages are published for early adopters, but Coven is still an early local-first MVP.

## Canonical language

- Ecosystem/org: **OpenCoven**
- Product/daemon/CLI: **Coven**
- Command: `coven`
- CLI package: `@opencoven/cli`
- OpenClaw plugin package: `@opencoven/coven`
- Community: `discord.gg/opencoven`
- X / Twitter: `@OpenCvn`
