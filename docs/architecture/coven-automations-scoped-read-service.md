---
title: "Automations scoped read service"
summary: "Review proposal for daemon-owned filesystem enforcement under Runtime Authority."
read_when:
  - Implementing the trusted Runtime Authority adapter
  - Reviewing grant-scoped filesystem access
source_adjacent_reason: "Defines the replacement runtime envelope and authority enforcement seams for issue 857."
---

# Automations scoped read service

Status: **design for review, partly implemented, not certified**. On
2026-10-18, Val selected a **Coven-owned scoped Rust read service** as the
boundary to prepare for review. This replaces the proposed reliance on
unrestricted native read tools. It requires a reviewed revision of the existing
no-MCP envelope contract. The selection does not satisfy the upstream Threads
human-review, merged-dependency-pin, adapter, receipt or certification gates.

## Implementation status

The service's operations are in `crates/coven-cli/src/automations/scoped_read/`:
reading a file, listing a directory and searching the files beneath one. Their
deterministic vectors are in `vectors.json` beside them. Nothing calls them
yet. They cover:

- the lexical rules for grant and request paths, and authorization before any
  filesystem access;
- the open walk on Unix;
- protected resources, held by identity;
- the recheck from the root after the open;
- the `.git` and `.env` exclusions beneath recursive scopes;
- listings and searches that leave out every entry a read would refuse;
- the per-file, per-attempt, scan, call, page, depth and path-length limits.

The service the harness talks to is beside them, in `service.rs` and
`channel.rs`:

- **An MCP server**, `coven`, with three tools: `read_file`, `list_directory`
  and `search` (`coven.scoped-read.tools.v1`).
  - Every `tools/call` reaches the reader, an unknown tool included, so every
    call counts against the attempt.
  - Each call is admitted by the attempt's gate before it runs, and its
    result, refusals included, is committed by digest before a byte is
    written. The text the harness receives is exactly the committed JCS text.
  - Each operation runs under the 5-second limit, or less when the run's
    deadline is nearer. One that overruns is refused (`limit_time`), and the
    service stops.
- **A private channel per attempt**, in a new directory under
  `<COVEN_HOME>/runtime/scoped-read`.
  - The directory is readable only by the owner. It holds a Unix socket and
    the MCP configuration the harness is pointed at.
  - That configuration names a hidden relay, `coven automations
    scoped-read-relay`, as the only MCP server. It passes the socket and a
    random token in the relay's environment, so neither appears in process
    arguments.
  - The channel serves one connection that presents the token, then closes
    its listener.
  - Closing the channel refuses every later call (`attempt_closed`) and
    withholds the result of one already running. It then removes only the
    files it created.

Two opt-in tests launch the installed Claude Code through the real relay with
a dummy key and a local server standing in for the model, so nothing is
billed. On 2.1.305:
- the init event and every model request listed exactly the three service
  tools;
- a scripted read of a granted file returned its content;
- a sibling the grant does not name was refused (`scope_missing`) in the same
  run;
- no memory file from the home folder, the launch folder or its parent reached
  the model.

**The replacement envelope, `r0-read.2`,** is described in
`automations/runtime_envelope.rs`:

- It launches claude with `--bare`, `--tools ""`, `--setting-sources ""`,
  `--strict-mcp-config`, no session persistence and no slash commands. Only the
  service's three tools are allowed, and the launch adds the attempt's
  `--mcp-config`.
- Its descriptor uses the `coven.automations.runtime-envelope.v2` profile. It
  binds:
  - the exact Claude Code versions proven so far, which is 2.1.305;
  - the service's server, transport, tools version and policy, including the
    5-second operation limit;
  - the launch: an empty working directory, an isolated home, the owner's API
    key, and the only environment variables passed.
- The observer reads its stream as complete only when the init event lists
  exactly the `coven` server, connected, and one of those versions. It also
  knows the `ui_invalidate` event that Claude Code 2.1.304 and later emit
  before init.
- The opt-in claude tests launch with these very arguments. The observer reads
  the scripted run's real stream as complete evidence of `analysis.read`.
- **It is not launchable yet.** `for_projection` and `for_grant` still find
  only `r0-read.1`, whose descriptor and digest are unchanged. Val decided on
  2026-10-20 that missions, the catalog and the mission-commands conformance
  vectors move to `r0-read.2` in a separate change, coordinated with Chat and
  the SDK.

**Admission for `r0-read.2`** is in `admission.rs` and `credential.rs`.
`prepare` admits one launch and returns what to start it with:

- **Version.** It runs the installed harness's `--version` with an empty
  environment apart from its own `PATH`, an isolated `HOME` and no
  auto-update. The version must be in the envelope's list, or the launch is
  refused with the version found and the versions admitted.
- **Key.** It reads the owner's saved API key. The hidden
  `coven automations api-key set|status|remove` command manages it, reading it
  only from standard input, never from a terminal. The key lives in
  `<COVEN_HOME>/credentials/anthropic-api-key`, 0600 in a 0700 folder. A key
  file that is a link, owned by someone else or readable by others is
  refused.
- **Launch folder.** It creates the launch folder, an empty working folder and
  a home for the harness. These sit under the system's temporary folder,
  never under `COVEN_HOME`, because the harness writes into its home and the
  whole folder is removed when the launch ends. A temporary folder that
  resolves inside `COVEN_HOME` is refused.
- **Environment.** It builds the harness's whole environment from exactly the
  envelope's variable names. A name the daemon can't fill refuses the launch.
- **Reader.** It builds the reader with `COVEN_HOME`, the launch folder and the
  owner's harness configuration and credential folders protected:
  - `.aws`, `.claude`, `.claude.json`, `.codex`, `.config/claude` and
    `.config/gh`;
  - `.docker`, `.gemini`, `.gnupg`, `.netrc` and `.ssh`.

  A project folder inside `COVEN_HOME` is therefore refused (`root_protected`).
- **Channel.** It opens the attempt's channel and adds `--mcp-config`.

An opt-in test runs the installed Claude Code 2.1.305 through `prepare`, with
exactly the environment and arguments it returns. The granted read is served,
the sibling is refused, the stream is complete evidence, and the launch folder
and channel are gone afterwards.

**The restored adapter supplies the scopes** (#1603). It pins `r0-read.2`
for a routine that declares authority, and refuses any declared scope the
service cannot enforce as written. `automations/trusted_launch.rs` takes a
launch's scopes from the signed Threads request its consumed decision names,
never from the stored definition, and admits the launch through `prepare`
over the routine's project folder. Its tests use the `notes/today.md` grant
from #1217:
- the granted note is served, and `note.txt` is refused (`scope_missing`);
- traversal, absolute paths and symbolic links out of the project are
  refused;
- a widened definition, a different launch folder, another binding and a
  tampered request are refused.

Still open: wiring `prepare` into the daemon's launch, so the channel opens and
closes with the session, and the production gate for response commitment
under the daemon's revocation fence. Also open: the mission switch, receipts
of the service's operations, and Windows. The vectors are engineering
evidence, not containment certification.

### Scope operations

Val decided the mapping on 2026-10-20 (#857). OpenCoven/coven-threads#90
records it in the profile as an additive consumer note.

| Scope | `read_file` | `list_directory` | `search` |
| --- | --- | --- | --- |
| Recursive read scope on directory D | Any file at or beneath D | D or any directory beneath it | D or any directory beneath it |
| Non-recursive read scope on path P | P, if it is a file | P, if it is a directory | Refused (`scope_operation_denied`) |
| Write scope only | Refused (`scope_access_denied`) | Refused | Refused |
| No scope covers the path | Refused (`scope_missing`) | Refused | Refused |

A non-recursive scope on a directory lets the model see the directory's entry
names but read none of them. Recursion stops 32 components from the root.

**Excluded names.** Val decided on 2026-10-20 (#857) that a recursive scope
leaves out every `.git` folder and every `.env` or `.env.*` file beneath its
directory, in any ASCII case, because project secrets usually sit there:
- A read, listing or search whose path passes through one beneath the scope's
  directory is refused before any filesystem access (`scope_excluded`).
- A scope whose own path names one covers it. A non-recursive scope on
  `.env` can read it, and a recursive scope on `repo/.git` covers what is
  beneath.
- Listings and searches never show an excluded name, even when another scope
  names it.

**Listings and searches show only what a read would admit.** They leave out:
- links, special files and hard-linked files;
- entries on another filesystem and protected resources;
- excluded names;
- names a request could never spell.

**A listing** returns entry names and kinds, never sizes. Each page holds up to
256 entries in byte order, and `after` resumes after a name.

**A search** matches a literal pattern of 1 to 256 bytes against each line, and
never shells out. It walks in path order and reads every file through the same
checked read, from the root. A file that cannot be read, is too large or is not
text is counted in `skipped`. Matched lines are cut to 512 bytes and count
against the attempt's disclosure total. Each page holds up to 256 matches, and
`after` resumes after a file and line.

A continuation only says where to resume. Every page is authorized afresh.
Entry names and searched file content count against a 64 MiB scan total per
attempt. When the total runs out, a search returns what it found with
`complete: false`, and later listings and searches are refused (`limit_scan`).

Every response is one closed JSON object with a digest of its JCS text, ready
for the commitment step.

### Points this implementation settles

These are the points the design left open. Each is for review:

| Question | Answer |
| --- | --- |
| What a retained root means | The directory opened when the reader is built. A request is served only while the root's path still names that directory. A root renamed away or replaced refuses every request (`root_moved`). |
| A granted filesystem scope the service cannot enforce as written | The whole grant is refused (`grant_unsupported`), not silently narrowed. This covers backslashes, repeated separators, `.` components, and anything the profile's breadth rule refuses. |
| Spellings that platforms read differently | Refused in grants and requests on every platform: backslash, colon (drive or data stream), control characters, a trailing dot or space, and Windows device names. |
| How protected resources match | By the identity of what each protected path names, so a case or Unicode alias still matches. A root that is, or is inside, a protected resource refuses the reader (`root_protected`). The daemon supplies the list; choosing it belongs to the service step. |
| Mount crossings | Refused by device on Unix. On Linux the mount id is also compared, which catches a bind mount from the same filesystem. A kernel that cannot report the mount id refuses (`platform_unsupported`). |
| Content that is not text | `read_file` refuses it (`resource_not_text`); a search skips it. |
| Platforms without a reviewed walk | Every request is refused (`platform_unsupported`). Today that is Windows. |
| What counts against the call limit | Every call, refused ones included. Paths are limited to 32 components and 4,096 bytes. |
| How the harness reaches the service | Through a per-attempt Unix socket in a private directory, by a relay the harness launches as its only MCP server. The socket path and token travel in the relay's environment through a 0600 configuration file, never in process arguments. One connection is served. |
| Whether a refusal is committed | Yes. A refusal can disclose that something exists, so it passes the same gate as content. Only the gate's own `attempt_closed` is written uncommitted. |
| What an overrun does | It refuses the call (`limit_time`) and stops the service. The worker cannot be interrupted, so no later call may share the reader with it. |

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

## Scope interpretation and opened-resource review contract

This section is a review proposal. It adds no accepted grant vocabulary and
provides no executed containment evidence. Threads #84 merged as `c8e67f7`
and its erratum, #89, as `7b5a9e6`; Coven's merged pin remains
`78e47c9d2430582128679fd4a54f9966f2f4f976`. The production adapter pins
`7b5a9e6` or later. Reconcile this contract with that dependency before
integrating it.

### Preserve authenticated bytes

The inspected filesystem scope has five fields: `kind`, `root`, `path`,
`access`, and `recursive`. The pending validator uses a temporary backslash
to slash replacement for breadth checks. It does not rewrite the signed scope.
Keep the authenticated JSON and its digests unchanged. Derive a separate
internal enforcement representation only after upstream validation, using
identical lexical interpretation for grant paths and request paths.

Refuse unsupported encodings before opening a resource. Include backslashes,
repeated separators, dot components, drive-relative paths, embedded NUL,
platform aliases, and case/Unicode aliases in the review vectors. A spelling
accepted by shape validation does not establish containment or permission.
Do not reinterpret a backslash as a literal filename on one platform while
another treats it as a separator.

### Separate capability, access, resource, and operation

`analysis.read` alone does not supply a filesystem scope. Missing applicable
read scope refuses every file operation; `access: write` does not imply read.
`recursive` does not establish resource type or authorize listing/search.
Determine type from the safely opened object, never its extension or a trailing
separator. Do not add private fields to the closed upstream scope object.

| Grant and opened resource | Proposed operation disposition |
| --- | --- |
| Applicable read scope, nonrecursive regular file | Bounded read of that exact opened file after all containment checks |
| Same grant, parent or sibling request | Refuse, including parent listing and metadata disclosure |
| Directory or recursive scope without reviewed operation mapping | Refuse; no inferred traversal, listing, or search |
| Missing scope, write-only scope, or unknown kind | Refuse before disclosure |
| Symlink, multiply linked file, special file, or mount crossing | Refuse under the initial profile |

An exact-file slice is only an implementation increment. It cannot complete
the required service: reviewed directory traversal, listing, and search remain
mandatory. Before implementing those operations, the upstream review must map
scope fields to each operation, its depth, accessible entries, protected
exclusions, and continuation semantics. Keep unsupported operations closed
until that mapping is reviewed; do not advertise partial service as complete.
Val decided the mapping on 2026-10-20. *Scope operations* above states it, and
OpenCoven/coven-threads#90 records it upstream.

### Review the actual open and disclosure order

Bind each root to a daemon-held resource identity and the validated grant.
Requests cannot select or reopen a root. Define what a retained root means
when its directory is renamed or replaced. Compare the opened resource to
that binding, including protected-resource aliases. A device identifier alone
is not proof that every mount crossing was excluded.

For each supported platform, review the actual traversal/open primitive and
its guarantees under concurrent rename, symlink replacement, mount changes,
and file-to-directory replacement. Require a deterministic race test and
an explicit unsupported disposition where a guarantee cannot be established.
A normalized string, canonical path, or successful metadata precheck cannot
substitute for the opened-object proof.

Buffer the complete bounded response, then revalidate and commit its exact
payload digest through the same authority/fence serialization point as
revocation and cancellation. Test both orders: revocation first withholds the
payload; response commitment first authorizes only those committed bytes.
Apply this rule to listing/search metadata and continuation responses too.
Retry, recovery, and restart require a fresh private channel and authorization;
a retained cursor or old channel cannot recreate authority.

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
