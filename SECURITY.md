# Security Policy

Coven is an early local-first harness substrate for project-scoped coding-agent
sessions: a small Rust authority layer that launches supported harness CLIs
inside explicit local project boundaries, plus TypeScript integration packages
around it. This is pre-1.0 software. This document is the single normative
security policy for the `OpenCoven/coven` repository. It separates what is
**enforced today**, what is a **residual risk**, and what remains a **design
goal**.

> Scope note: this policy covers Coven the runtime/daemon/CLI and the code in
> this repository. Organization-wide OpenCoven reporting (protocol, memory
> substrate, other repositories) belongs in the
> [organization-level security policy](https://github.com/OpenCoven/.github/blob/main/SECURITY.md).
> The canonical public overviews live at
> [docs.opencoven.ai](https://docs.opencoven.ai/docs/reference/safety); this
> file stays beside the code as the source-adjacent contract.

## 1. Supported surfaces and security status

**Supported release family.** Security fixes land on the current minor release
line published in
[repository releases](https://github.com/OpenCoven/coven/releases) (v0.4.x as
of this update). Coven has no long-term-support commitment before 1.0; run the
latest release to pick up security fixes.

**Security-supported surfaces.**

- The Rust daemon authority boundary and its versioned local API,
  `coven.daemon.v1`, over same-user local IPC. See the
  [local API contract](docs/API-CONTRACT.md) and
  [authentication and local access](docs/AUTH.md).
- The bundled CLI and daemon lifecycle surfaces that drive the same boundary
  (see [README.md](README.md) and the
  [safety model](docs/SAFETY-MODEL.md)).
- Local session state: the SQLite event store, default event/log redaction, and
  artifact persistence defaults. See the
  [session artifacts spec](specs/coven-session-artifacts/TECH.md) and the
  [trust layer contract](specs/coven-trust-layer/PRODUCT.md).
- Repository content guards: the secret scan and the Coven privacy guard run in
  CI (`Policy guard`) and in managed local hooks.

**Experimental or disabled surfaces — not security-supported.**

- **AgentFS NFS mount backend.** The `coven-afs` storage engine is shipped and
  conformance-tested, but every mount backend sits behind the opt-in `mount`
  cargo feature and remains a spike. Loopback access control and single-writer
  SQLite remain open gates in
  [`specs/coven-agent-fs/MOUNT-SPIKE.md`](specs/coven-agent-fs/MOUNT-SPIKE.md),
  and the mount surface does not leave experimental status until the
  dedicated end-to-end certification gate (#779) passes. Do not expose a
  Coven AFS export beyond the local machine.
- **OpenClaw bridge plugin.** Disabled by default; it must be explicitly
  selected as the ACP backend. OpenClaw core is not a Coven trust root, and
  the plugin's client-side socket validation is defense in depth, not the
  enforcement boundary. See [authentication and local access](docs/AUTH.md).
- **Remote and tunnel transports.** The daemon does not bind TCP by default
  and has no remote authentication design yet. Only the documented remote
  access paths are supported; do not proxy the raw local IPC endpoint into a
  network or browser surface. A separate authenticated remote listener is
  drafted but unshipped (#463).

**Same-user trust is not sandboxing.** Coven's boundary assumes the operating
system separates users and that the person running `coven` controls the
machine. It distinguishes two different threats:

- *Same-user local trust* — what Coven relies on: OS-enforced local IPC
  permissions (a private Unix socket or owner-only named pipe) plus same-user
  process locality.
- *Sandboxing against hostile local processes, prompts, or providers* — what
  Coven does **not** provide. Harnesses run with your user's privileges. A
  malicious prompt, harness output, or provider response can steer a harness
  within those privileges; the daemon's checks validate requests against the
  local API contract, they do not contain a running harness. Never run
  untrusted harnesses or prompts in sensitive repositories.

Coven makes no absolute containment claim (such as "cannot escape") for any
surface. Where a property is enforced, it is tied to the named contracts and
verification families in the next section.

## 2. Enforced properties today

Each property below is backed by a normative source-adjacent contract and a
verification family. This table is the whole list; anything documented only as
a draft spec or design goal is in
[Design goals vs guarantees](#5-design-goals-vs-guarantees).

| Property | Normative contract | Verification |
|---|---|---|
| Rust-owned validation is authoritative over untrusted clients; every sensitive request is revalidated at the daemon and fails closed on unknown versions, action ids, harnesses, and session ids | [Safety model — trust boundary](docs/SAFETY-MODEL.md), [Authentication — Rust authority checks](docs/AUTH.md) | Rust workspace test suites (`cargo test --workspace`) run in CI on every PR |
| Capability advertisement never grants permission: `/api/v1/health` capabilities describe availability only, and clients must still pass every per-operation check | [API contract](docs/API-CONTRACT.md) (`Capabilities advertise availability and never grant permission`) | Health-negotiation contract tests (`crates/coven-client/tests/health.rs`) and daemon contract tests |
| Project, path, and session checks happen before effects: canonicalized `projectRoot`/`cwd`, allowlisted harness ids, live-session validation, and argv-only launch (never `sh -c`) | [Safety model — core rules](docs/SAFETY-MODEL.md), [API contract — error envelopes and fail-closed routes](docs/API-CONTRACT.md) | Rust workspace test suites, including daemon lifecycle and harness parity tests |
| Owner-protected local transport and peer negotiation: the daemon API travels only over same-user local IPC; the bundled Rust client discovers only the current user's private socket/pipe, binds health negotiation to a transport peer fingerprint, and never auto-replays a mutation | [Authentication and local access](docs/AUTH.md), [API contract — reusable Rust client](docs/API-CONTRACT.md) | Client transport and negotiation tests (`crates/coven-client/tests/health.rs`), Windows daemon lifecycle tests |
| Event/log redaction and sensitive-artifact defaults: event payloads are redacted before they are stored or returned by the API; raw sensitive artifact persistence is opt-in, off by default, and encrypted at rest with a private per-home key file | [Trust layer contract — defaults that must hold](specs/coven-trust-layer/PRODUCT.md), [Session artifacts spec](specs/coven-session-artifacts/TECH.md) | Redaction unit tests (`crates/coven-cli/src/privacy.rs`) and artifact store tests in the Rust workspace |
| Secret and privacy guards with a stated baseline: the secret scan covers the full tree and git history; the Coven privacy guard fails closed on new and PR-changed files; CI is the authoritative enforcement layer | [`scripts/check-secrets.py`](scripts/check-secrets.py), [`scripts/check-coven-privacy.py`](scripts/check-coven-privacy.py) | CI `Policy guard` job; managed hooks from `coven hooks install` |
| Mutation replay is explicit, never implicit: adopted launch/input operations use a normative replay-before-mutable ordering with exact first-adoption and exact-replay responses, and retained ambiguity is surfaced instead of silently resolved | [API contract — request ordering and durable side effects](docs/API-CONTRACT.md) | Adopted-route contract tests in the Rust workspace |

The privacy guard is deliberately a **baseline for new changes**: it applies to
newly staged and PR-changed files while the repository's legacy path examples
are inventoried and converted to placeholders. It rejects sensitive examples,
including invite/handoff URLs containing tokens. It does not claim that
historical commits satisfy the newer privacy rules, and rewriting public
history requires explicit maintainer approval.

Memory-layer code, tests, documentation, and PR discussion must describe memory
shape without including real memory content. Use synthetic placeholders such as
`FAMILIAR_ROOT`, `<familiar-id>`, and `01JEXAMPLE...`; never copy real
attestation prose, session identifiers, chat IDs, or local workspace paths into
the repository.

## 3. Residual risk and safe configuration

**What local-first does not protect against.** Local-first keeps data on your
machine and keeps the API off the network by default; it is not a defense
against software already running as your user, harness processes acting with
your privileges, or a hostile prompt/provider steering a harness. It also does
not yet harden daemon-side `COVEN_HOME` ownership and permission checks before
creating or removing daemon state; that remains a documented hardening priority
(see [authentication — current hardening gap](docs/AUTH.md)), so client-side
socket validation should be treated as defense in depth, not a complete
boundary.

**Raw-artifact opt-in risk.** Setting `persist_raw_artifacts = true` in
`privacy.toml` (or `COVEN_PERSIST_RAW_ARTIFACTS=1`) stores unredacted payload
artifacts. They are encrypted at rest with a key generated under
`<COVEN_HOME>/keys/session-artifacts.key` with private file permissions, and
the key is not stored in the repository or the database. The local key-file
provider is an MVP for local-first encryption: it protects raw artifact rows
from casual database inspection, but it is not OS keychain-backed key
management and is not intended for shared or higher-risk machines.

**Retention is data minimization, not secure deletion.** Raw encrypted
artifacts are retained for 7 days and redacted event logs for 30 days by
default, with manual pruning via `coven logs prune`. Retention bounds how long
sensitive rows persist in Coven's store; it does not overwrite database pages
or other copies your operating system or backup tooling may hold.

**Untrusted harnesses and prompts.** Supported harness CLIs (Codex, Claude
Code, GitHub Copilot CLI; opt-in recipes beyond that set) execute with your
user's privileges inside the project you point them at. Coven validates how
they are launched and which sessions they reach; it does not police what a
running harness does inside those privileges. Do not paste secrets into
prompts, do not ask a harness to dump environment variables, and use throwaway
projects for demos and smoke tests.

**AgentFS mount safe configuration.** Treat every AFS mount as experimental
scratch state for a single user on a single machine: loopback only, no
multi-user export, no exposure beyond localhost, and no use as durable
storage. The backend is feature-gated and uncertified; its remaining gates and
go/no-go status are tracked in
[`specs/coven-agent-fs/MOUNT-SPIKE.md`](specs/coven-agent-fs/MOUNT-SPIKE.md)
and the certification work under #779, which this section follows. If the
mount surface ships for real, this policy is updated before that release.

**Targets and design goals are not enforced properties.** Performance targets,
SLOs, and architectural goals — in this repository or the wider OpenCoven
protocol work — are not security properties of Coven until a corresponding
test suite and release prove them, and release-gating security claims are
recorded in the shipped-truth/certification evidence produced by the release
governance work (#779, #805).

## 4. Reporting a vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

- **Primary path:** open a private
  [GitHub Security Advisory](https://github.com/OpenCoven/coven/security/advisories/new)
  on this repository. This is the monitored intake for Coven.
- **Organization-wide findings** (protocol behavior, cross-repository issues,
  other OpenCoven repositories) belong in the
  [organization-level security policy](https://github.com/OpenCoven/.github/blob/main/SECURITY.md).
- If you cannot use Security Advisories, mark a related tracking issue private
  by contacting a maintainer through an organization-owned channel — please do
  not depend on any individual's personal account as the reporting path, and
  never post exploit details in public issues.

Coven deliberately publishes **no acknowledgment or remediation deadline**.
Maintainers triage advisories through normal repository maintenance. Adding
response-time commitments requires an accountable process that can meet and
measure them; until such a process exists, this policy does not promise one.
Researchers who responsibly disclose may request credit in a release note, with
their permission.

**Third-party dependencies and providers.** Findings that live purely inside a
third-party dependency or a model provider's API are best reported upstream to
that maintainer. Report them here as well — via a Security Advisory — when they
materially compromise Coven's supported behavior: bundled or pinned versions,
Coven's integration defaults, credential-handling boundaries, or anything that
turns a dependency flaw into a Coven compromise.

## 5. Design goals vs guarantees

The retired repository policy listed broad isolation properties beside enforced
behavior. They are **design goals of the OpenCoven protocol**, not enforced
Coven properties today, and they now live where they belong:

- **Session isolation** across users and agents, **memory ownership**, and
  **familiar identity integrity** are protocol-level goals described in the
  [trust layer contract](specs/coven-trust-layer/PRODUCT.md) and related
  OpenCoven protocol documents.
- **Agent-to-agent boundary policy and delegation** are active work in
  #803 (input-guardrail parity across handoffs) and #804 (invocation and
  delegation contracts). Coven's current local Runner does not implement A2A
  isolation; do not rely on it as if it did.
- **Execution boundaries** are enforced only to the extent of the properties in
  [Enforced properties today](#2-enforced-properties-today).

A property becomes a Coven guarantee when an executable acceptance or control
family tests it on a shipped release — the table in section 2 is that list.
Violating an enforced property is a security report. A path that would defeat a
design goal (for example, cross-user or cross-agent access) is also worth
reporting, but it should be described as a protocol-boundary finding, not as a
broken Coven guarantee.

## Policy maintenance

- This file is the single normative security policy for this repository; the
  organization-level default policy is not additive here.
- Update it in the same change as any security or secret-handling rule, per
  [documentation maintenance](docs/DOCS-MAINTENANCE.md).
- Internal links are relative repository links so they resolve in both the
  repository and deployed-doc contexts; docs link and freshness validation is
  tracked under #778.

*Last updated: 2026-08-30*
