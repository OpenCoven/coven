---
title: "Coven repository documentation"
description: "Pointers to canonical public documentation and source-adjacent contracts retained with the Coven code."
---

# Coven repository documentation

Public product documentation is canonical at
[docs.opencoven.ai](https://docs.opencoven.ai/). Start with:

- [Getting started](https://docs.opencoven.ai/docs/guide/getting-started)
- [CLI reference](https://docs.opencoven.ai/docs/cli)
- [Daemon](https://docs.opencoven.ai/docs/daemon)
- [Harnesses](https://docs.opencoven.ai/docs/harnesses)
- [Local API](https://docs.opencoven.ai/docs/reference/api)
- [Memory](https://docs.opencoven.ai/docs/memory-models)
- [Troubleshooting](https://docs.opencoven.ai/docs/reference/troubleshooting)

This directory retains source-adjacent material that must change with the
implementation:

- [`API-CONTRACT.md`](API-CONTRACT.md) — normative `coven.daemon.v1` contract.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — crate ownership and authority boundaries.
- [`SESSION-LIFECYCLE.md`](SESSION-LIFECYCLE.md) — normative session state machine.
- [`HARNESS-ADAPTERS.md`](HARNESS-ADAPTERS.md) — adapter implementation contract.
- [`RELEASE-GOVERNANCE.md`](RELEASE-GOVERNANCE.md) — normative release policy binding publication to the exact source commit.
- [`development/`](development/) — maintainer source maps and verification loops.
- [`design/`](design/), [`superpowers/`](superpowers/), and repository specs —
  implementation decisions, plans, and historical records.

See [`DOCS-MAINTENANCE.md`](DOCS-MAINTENANCE.md) before adding or moving a page.
Its public-doc directory boundary defines which directories may contain only
canonical pointers or source-adjacent exceptions.
