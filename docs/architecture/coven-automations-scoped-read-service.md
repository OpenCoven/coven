---
title: "Automations scoped read service"
summary: "Review proposal for daemon-owned filesystem enforcement under Runtime Authority."
read_when:
  - Implementing the trusted Runtime Authority adapter
  - Reviewing grant-scoped filesystem access
source_adjacent_reason: "Defines the replacement runtime envelope and authority enforcement seams for issue 857."
---

# Automations scoped read service

Status: **design for review, not implemented or certified**. On 2026-10-18,
Val selected a **Coven-owned scoped Rust read service** as the boundary to
prepare for review. This replaces the proposed reliance on unrestricted native
read tools. It requires a reviewed revision of the existing no-MCP envelope
contract. The selection does not satisfy the upstream Threads human-review,
merged-dependency-pin, adapter, receipt or certification gates.

## Problem and chosen boundary

The cancelled #1217 adapter selected an envelope from capability names but did
not enforce filesystem scopes. Its lifecycle fixture granted `notes/today.md`
while permitting a read of `note.txt`. Observation after a read cannot undo
that disclosure. `authority_projection.rs` currently carries capabilities and
binding identity, but no enforceable filesystem authorization.

The daemon will resolve and enforce the exact granted scopes in Rust. A
Coven-owned local service will provide the only model-accessible file tools
for this envelope. The harness remains a consumer, never the authorization
source. SDK/Cave submit commands and show results; neither authorizes reads.

The alternative, retaining native read tools, would require independent
process isolation for each exact grant and separation of harness/provider
credentials from accessible files. That is not the selected implementation
boundary. A permission prompt, read-only mode, tool-name allowlist or cwd
restriction alone does not prove filesystem containment.

## Replacement envelope contract

Replace `r0-read.1` with a new descriptor version; do not change the meaning of
an already-signed descriptor. The new descriptor binds:

- The exact harness and supported version, restricted launch configuration,
  structured output format and permission behavior.
- Only the Coven-owned scoped read service and its exact tool/schema version.
  Native Read/Glob/Grep, shell, arbitrary subprocess, network tools, user MCP
  servers, hooks and settings that add tools are unavailable.
- The service's enforcement-policy version and bounded resource limits.
- The validated execution binding and a daemon-private session-to-service
  association; the model cannot supply a principal, grant or root to replace it.

Tool restrictions are insufficient if startup/context loading can read around
them. The launch admission contract must enumerate every model-visible input:
owner-supplied prompt, approved familiar embodiment, system instructions and
any explicitly granted context. Disable ambient project/ancestor instructions,
skills, memory, attachments, resumed transcripts and auto-discovery. Use an
owned empty launch directory and isolated configuration; do not inherit the
project cwd as an implicit context grant. Provider credentials may be used only
by the reviewed authentication path and must never become model context. If a
pinned harness cannot exclude or exactly constrain automatic ingestion, refuse
launch. Admission tests use out-of-scope sentinel files and a test provider
capture to prove they never enter the model request, even without tool calls.

The old no-MCP rule becomes **no external or user-configured MCP servers;
only the daemon's scoped read service is admitted for this envelope**. The
exact provider CLI configuration must be proven against a pinned executable
before this descriptor becomes launchable. If settings, environment overrides,
startup plugins or tool aliases can widen it, refuse launch. Existing signed
`r0-read.1` observations remain verifiable as historical evidence but cannot
serve as proof of scoped enforcement or be silently relabeled.

## Authority and service lifetime

1. The trusted adapter resolves fresh principal/familiar revisions, exact
   definition revision/digest, Threads grant and revocation state, runtime
   descriptor, adoption and dispatch fence. Unknown scope kinds fail closed.
2. Rust normalizes scopes using the reviewed Threads vocabulary, preserving
   their exact resource identities and access restrictions. Do not invent a
   parallel permissive scope grammar or treat an absent scope as workspace-wide.
3. The daemon creates a private service instance bound to that attempt and
   binding digest. The harness reaches it through an owned local stdio
   transport. No generic HTTP endpoint or model-visible bearer token is needed.
   Any helper process receives only a private daemon channel and immutable
   correlation; it cannot self-issue grants or reopen arbitrary roots.
4. Each request rechecks session ownership, binding/fence validity, expiry,
   cancellation and revocation. It performs only an operation covered by the
   current grant. Retry and recovery get a new attempt association after fresh
   authorization; they cannot reuse the old service instance.
5. Cancellation, expiry, revocation, daemon loss or restart closes the channel
   and refuses further reads. Interrupted/ambiguous attempts remain held under
   existing recovery semantics. A restarted service cannot adopt an old channel.

Keep authority resolution in the trusted Rust path. The public privacy-minimized
consumer projection is not a credential and must not acquire secret grant
material just to launch this service. Add an internal validated scope handle
alongside the launch binding; bind its digest into the new descriptor/binding
contract through the reviewed authority profile.

## Filesystem enforcement

The service offers bounded file read, directory listing and content search.
All path interpretation and traversal occur inside Rust. Search never shells
out, and listings reveal only authorized entries. Absolute paths, parent
traversal, alternate-root encodings, unsupported access modes and unknown
request fields are refused before opening a resource.

Resolve reads relative to daemon-held authorized root handles, not a process
cwd. Enforce no-follow and beneath-root semantics during the actual open;
canonicalizing a string and later opening it is insufficient. Reject symlinks,
special files and mount crossings in the initial profile. Reject multiply
linked regular files until a reviewed resource-identity rule can prove that
aliases cannot disclose outside-scope content. Validate the opened object,
not just the input string. Directory rename/replacement and symlink races must
be exercised concurrently. A platform without an audited race-safe primitive
for an operation refuses that operation; it cannot fall back to string checks.

A file grant does not imply access to siblings or its whole parent directory.
A directory grant must explicitly authorize traversal/list/search according to
the upstream scope vocabulary. Configuration, provider credentials, service
state and authority keys are never implicitly included by a workspace grant.
Enforce those exclusions before disclosure, including aliases and case/Unicode
variants on filesystems that equate them.

Proposed initial limits for review: 1 MiB per file response, 8 MiB of disclosed
file content per attempt, 256 entries per listing page and 256 search results
per page. Bound scans to 64 MiB per attempt, traversal to 32 levels, service calls to
1,024 per attempt and each operation to five seconds or the remaining original
run deadline, whichever is shorter. These are descriptor-bound policy limits,
not caller-supplied unlimited values.
An oversized file read returns an explicit limit refusal. Paged results carry
an explicit continuation scoped to the same grant and attempt; a cursor grants
no additional authority. Buffer bounded file, listing and search responses. Revalidation and response
commitment must share the daemon's authority/fence serialization point with
revocation and cancellation. A response commitment authorizes one exact bounded
payload digest for that attempt; it cannot authorize subsequent chunks or
requests. If revocation/cancellation commits first, discard the pending payload
and refuse delivery. If response commitment wins first, those bytes are already
authorized and cannot be recalled; record that ordering explicitly. Do not
promise retroactive revocation of committed/in-flight content. Queued but
uncommitted responses must be discarded, including listing/search metadata.
A check followed by an unsynchronized channel write is insufficient. Test the
ordering deterministically by pausing after validation and before commitment.

## Observations and receipts

The daemon records service-issued operation IDs, binding/attempt correlation,
operation class, permitted/refused outcome and privacy-classified resource
references. Raw file contents, credentials and arbitrary full paths do not
enter general logs. The observer correlates these records with the signed
runtime stream. A model-authored tool-looking line cannot manufacture an
operation or receipt. Missing service or runtime evidence yields incomplete
or unknown evidence and preserves recovery hold; exit zero is not success.

A conclusive R0 receipt must identify the exact grant and descriptor, the
operations actually exercised, refusal/cancellation history and evidence
completeness. Historical records retain their original privacy and integrity
semantics. Retention and authorized disclosure follow #857's existing contract.

## Owning seams and review order

- `automations/runtime_envelope.rs`: new versioned descriptor and exact tools;
  preserve historical descriptor lookup.
- Trusted adapter restored from #1217 after prerequisite integration: turn
  validated upstream scopes into internal enforcement handles.
- New Rust scoped-read module beside the authority implementation: parsing,
  platform-safe opens, quotas, request authorization and observation records.
- Existing API/daemon runtime launch seams: create and close private service
  channels with attempt lifecycle; reject widened effective configuration.
- `automations/stream_observation.rs` and `terminal_observer.rs`: recognize only
  descriptor-bound service operations and correlate authenticated evidence.
- Existing receipt commitment/recovery transactions: consume complete evidence
  without duplicating lifecycle decisions in the service or clients.

Review the scope normalization and platform-open contract first, then the
service and harness admission tests, then the restored adapter/receipts, then
exact-release SDK/Cave/Psyche canaries. Threads #82/#84 review and final merged
pins remain prerequisites for production adapter integration.

## Required acceptance evidence

| Case | Required observation |
| --- | --- |
| Exact file grant | Only that file can be read; sibling and parent listing refused |
| Explicit directory grant | Only permitted entries/operations disclosed; no implicit credential access |
| Traversal/absolute/alternate encoding | Typed refusal before resource open or content disclosure |
| Symlink/hardlink/mount/special file | Refusal under the initial profile, including concurrent replacements |
| Rename/open race | No outside-root disclosure; unsupported platform operation refused |
| Expired/revoked/changed binding | No new response commitments; uncommitted buffered responses withheld |
| Wrong attempt/fence/replayed channel | Refusal; no grant reuse across retry/recovery/restart |
| User settings/MCP/hooks/environment widening | Launch refused before harness work |
| Ambient instructions/memory/transcripts/attachments | Excluded sentinel content never reaches the captured provider request |
| Revocation racing disclosure | Serialized commitment order determines release; queued uncommitted responses discarded |
| Direct native tool or shell invocation | Tool unavailable; attempt cannot bypass the service |
| Quota/overlarge response/search | Explicit bounded result or limit refusal; no unbounded allocation |
| Forged output or missing observations | No authenticated success receipt; unknown evidence held |
| Valid complete R0 execution | Independently verified binding, service evidence and terminal receipt |

Require deterministic Rust vectors, harness admission integration tests and
exact published runtime canaries. Test fixtures are not certification. Do not
advertise Runtime Authority or enable unattended execution until #857/#858
acceptance and #854's go/no-go are satisfied.
