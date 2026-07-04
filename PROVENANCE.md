# PROVENANCE.md — OpenCoven Origin Record

> This document exists to establish the clear, timestamped origin of the architectural ideas that define OpenCoven. It is a public record, not a legal claim. It is maintained so that future readers — contributors, users, researchers, patent examiners — can verify where these ideas came from and when.

---

## Origin

**OpenCoven** was created by **Valentina Alexander** (`@BunsDev`) and first published publicly on **April 27, 2026** under the MIT License.

- GitHub organization: https://github.com/OpenCoven
- Website: https://OpenCoven.ai
- Discord: https://discord.gg/OpenCoven
- X / Twitter: https://x.com/OpenCvn
- License: MIT (https://opensource.org/licenses/MIT)
- Repository creation date: **2026-04-27** (verifiable via GitHub API: `https://api.github.com/repos/OpenCoven/coven`)

This repository has no fork parent. It is an original work with no upstream source repository.

---

## Architectural Concepts and Their Origins

### 1. The Familiar Identity Model
**First appeared:** `coven-cli/src/familiar_identity.rs`, commit history from 2026-04-27

A named, role-scoped agent persona — a "familiar" — resolved from a configuration manifest (`familiars.toml`) and attached to a session via a CLI flag or runtime config. Each familiar has an `id`, `display_name`, and `role`. The familiar model is the foundational identity primitive for multi-agent systems in OpenCoven.

This concept was original to this repository. It has been independently acknowledged as prior art by third parties in their own published documentation.

### 2. The Agent Spawn Harness
**First appeared:** `pty_runner.rs:307` (`spawn_piped_with_observer`), commit history from 2026-04-27

A structured dispatch pattern for launching configured agent/harness processes. The pattern validates a harness allowlist, canonicalizes working directory, checks session state, then dispatches to one of a defined set of low-level spawn primitives. This is the root pattern from which single-chokepoint process execution architectures in agent systems derive.

### 3. The Multi-Agent Familiar Substrate
**First appeared:** 2026-04-27, OpenCoven core architecture

Composable, purpose-scoped agents ("familiars") that cooperate through structured routing, share session context, and are individually manageable. Each familiar has a defined lane; routing between familiars is explicit and traceable. This is not monolithic prompt-chaining — it is a hub-and-spoke orchestration model where each agent is a named, scoped participant.

### 4. The Graded Approval Tier Model
**First appeared:** The Familiar Contract RFC-0001 v0.2.0, authored by Valentina Alexander and Sage, **dated 2026-06-19**

Defines a `ward.toml`-like structure with a `[protected]` partition (files/invariants an agent may not modify autonomously) and `[editable]` partition, gated by approval tiers: `auto → familiar_review → human_review → human_required`. Enforced by an authority layer separate from the familiar itself.

### 5. Session Memory and Continuity Substrate (OpenTrust)
**First published:** 2026 (active development)

Durable, portable agent memory that persists across sessions, remains under user control, and maintains provenance of agent actions. Designed to be model-agnostic and provider-agnostic — memory is not stored on vendor servers unless the user chooses that. This is the trust layer that makes long-running agent systems auditable and recoverable.

---

## Third-Party Acknowledgments

The following third-party projects have independently documented OpenCoven as a source or ancestor of their architectural concepts. These acknowledgments are recorded here as additional evidence of the originality and priority of OpenCoven's contributions:

| Project | Documentation | Concepts acknowledged |
|---|---|---|
| `YogiSotho/warden` | `lineage/LINEAGE.md`, `docs/ops/patent/prior-art-search.md` | Spawn chokepoint (pty_runner.rs:307), familiar identity model (coven-cli/src/familiar_identity.rs:23-35), harness adapter contract, ledger shape, CLI skeleton. Warden's own prior-art search gives OpenCoven a "real, dated, public prior art" verdict for these elements. |

The presence of these acknowledgments in third-party repositories is noted for the record and does not constitute an endorsement of those projects by OpenCoven maintainers.

---

## Maintainer

**Valentina Alexander**
- GitHub: [@BunsDev](https://github.com/BunsDev)
- Role: Creator and Core Maintainer, OpenCoven
- Also: Core Maintainer, OpenClaw; Developer Relations Engineer, Ritual Foundation

---

## Contributing to the Record

If you are aware of prior art that predates any of the concepts listed here, please open an issue. We maintain this document honestly — if something was not original to us, we want to know and update the record accordingly.

If you are aware of a patent application or trademark filing that cites or conflicts with the concepts documented here, please notify the maintainers immediately via the Discord or by opening a GitHub issue.

---

*Last updated: 2026-07-04*
*This document does not constitute legal advice.*
