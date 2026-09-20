# Ordinary-chat context admission intent

An explicit context intent prevents a request for bounded conversation context
from silently running as legacy chat. This source-level contract is a
**fail-closed prerequisite**, not an enabled chat write or isolation profile.

```sh
printf 'null\n' > context-intent.json
coven run codex hello --context-admission context-intent.json
# Exits nonzero with chat_context_invalid. No harness session is launched.
```

Even a structurally valid intent with matching digests is refused. Coven does
not currently have a production consumer for trusted
`familiar.embodiment_binding.v1` evidence and current authoritative ledger
observations. It also has no qualified native context profile. Self-consistent
client data cannot fill either gap.

## Admission surfaces

Use `contextAdmission` on `POST /api/v1/sessions` or
`POST /api/v1/sessions/:id/input`. Unversioned aliases use the same gate.
The field is recognized before any session mutation or runtime call.
Its presence, including `null`, cannot fall through to ordinary launch/input.

The boundary requires owner-local IPC. TCP receives
`chat_context_forbidden`. Supplying the field on any other mutation, including
`/cast`, `/actions`, adopted launch/input, external registration, handoff,
or kill, is rejected. It does not extend those contracts. Mixing it with
`executionBinding` or `requestAdoption` is rejected without interpreting either
as chat authority.

Routes under `/api/v1/internal/` that the daemon dispatches *before* the API
handler -- lifecycle shutdown, and mobile local control, which ignores the
request body entirely -- are gated at that earlier dispatch point. Without that
gate an internal mutation would take effect while carrying an intent, returning
its own success instead of a refusal, so "any other mutation" would hold only
for the routes the API handler happens to observe.

For direct CLI launch, `--context-admission FILE` reads a bounded JSON intent.
The gate also applies with `--continue`, `--detach`, and streaming flags.
A rejected intent does not initialize the CLI store, acquire a session writer,
insert a session, deliver input, start a harness, or issue a receipt.

Omitting the new field or CLI option preserves legacy behavior, including
ordinary history recovery and prompt reference expansion. Existing malformed
body handling is unchanged when no complete JSON object carrying the new
member can be decoded.

## Closed intent shape

All listed members are required. `model` and `expectedReceipt` must be explicit
`null` when absent. Objects reject extra or duplicate members. Context requests
also reject duplicate top-level API request keys.

| Intent member | Meaning |
| --- | --- |
| `profile` | Exactly `coven.chat_context_admission.v1`. |
| `manifest` | The complete manifest described below. Its version is fixed by `profile`. |
| `manifestDigest` | Lowercase SHA-256 of RFC 8785/JCS bytes of the entire `manifest`. |
| `expectedReceipt` | `null`, or a reference containing `receiptId`, `sessionId`, `manifestDigest`, and `bindingDigest`. This is a claim to reconcile, not a supplied accepted receipt. |

| Manifest member | Meaning |
| --- | --- |
| `identity` | Reference containing `profile: "familiar.embodiment_binding.v1"`, `bindingId`, `bindingDigest`, `familiarRootId`, and `identityRevisionId`. These are unverified expectations, not a new identity schema or authority record. |
| `familiarId` | Exact requested roster slot. It does not confer root, revision, or scope authority. |
| `harness`, `model`, `adapterProfile` | Exact requested runtime selection. `model` is a string or explicit `null`. No `adapterProfile` is currently qualified. |
| `mode` | `continuity` or `fresh`. Neither value grants isolation. |
| `projectRoot`, `resourceRefs` | Absolute requested project path and explicit resource-reference set. Consistency checks are not proof of source authorization. |
| `selections` | Selected categories: `identity`, `policy`, `daily-memory`, `durable-memory`, `vault`, `history`, or `selected-text`. |
| `sources` | Each source has `sourceRef`, `sourceRevision`, `contentDigest`, `resourceRef`, `category`, and `truncation: {originalBytes, includedBytes}`. |
| `promptDigest` | Lowercase SHA-256 of the exact prompt bytes at the dispatch boundary. |
| `launchPolicyDigest` | Lowercase SHA-256 of the requested launch-policy representation described below. This is not proof of effective permission. |
| `retention` | `retained` or `temporary`. Neither value enables a custody guarantee. |
| `optionalMemoryPolicy` | `disabled` or `selected-only`. Automatic memory ingestion is not supported by this intent. |

Digests are 64 lowercase hexadecimal characters. Strings are nonempty, unpadded,
bounded to 2,048 UTF-8 bytes, and exclude control characters. The intent is at
most 65,536 UTF-8 bytes, with at most 64 distinct resources and 128 distinct
source references. Truncation counts are unsigned 32-bit byte counts; included
content must be nonempty and no larger than the original.

Both `identity` and `policy` are mandatory selections with explicit sources.
Every selected category needs a source; every source must belong to a selected
category and a declared resource. Duplicate selections and sources are invalid.
`disabled` forbids daily and durable memory selections. `fresh` permits only
mandatory identity/policy and explicitly selected text.

For API launch, `launchPolicyDigest` covers JCS bytes of `launchPolicy`, or JSON
`null` when omitted. For CLI launch it covers
`{"permission": ..., "addDirs": [...], "think": ..., "speed": ...}`, retaining
explicit `null` for omitted permission/speed. Requested model and familiar
values must match the manifest. An enabled future profile must additionally
bind resolved effective permission, canonical scope, and adapter configuration.

## Prompt expansion and retries

The API compares `promptDigest` with the launch prompt after the existing outer
whitespace trim, or with the exact `data` string for input.

The CLI refuses `@path`, `@T-session`, and `@@search` references *before reading*
their contents because no authoritative source verifier is available. For
reference-free prompts, existing expansion returns the original bytes. The
CLI checks those bytes without opening the store or running expansion.
A client-supplied source list does not authorize reference reads.
Native workspace files, caches, instructions, plugins, provider-side context,
and subsequent identity injection remain unqualified; matching prompt bytes
alone never permit launch.

Native resume requires a claimed receipt and cannot be combined with
`fresh`. Neither can input to an existing session. Input requires an exact
target `sessionId` in `expectedReceipt`. CLI `--continue ID` requires that
same exact session ID; implicit latest-session selection and native
conversation-ID aliases are not admitted. The API's `conversation.id` names
a native conversation, not a Coven session ID. Its relationship to a claimed
receipt cannot be established without trusted stored evidence, so a matching
client claim still receives `chat_context_receipt_unavailable`.
Changing a manifest or binding without matching its receipt commitment is a
conflict. Matching the client-supplied commitments still cannot resolve a
receipt: no accepted ordinary-chat context receipts exist in this source unit.
Repeated requests therefore remain refusals, not successful replay or lost-ack
reconciliation. Do not erase the intent and retry through a legacy path.

## Failure contract

Errors use the existing daemon error envelope. Field diagnostics expose paths,
not submitted identity/source values.

| Code | HTTP status | Result |
| --- | --- | --- |
| `chat_context_invalid` | 400 | Malformed, missing, duplicate, oversized, or inconsistent intent structure. |
| `chat_context_mismatch` | 409 | Digest, request selection, native resume, or receipt commitment mismatch. |
| `chat_context_forbidden` | 403 | Not owner-local IPC. |
| `chat_context_wrong_route` | 400 | Unsupported operation or mixed authority contract. |
| `chat_context_admission_unavailable` | 503 | No trusted authority/current-ledger consumer and qualified native profile. |
| `chat_context_receipt_unavailable` | 503 | Claimed receipt cannot be resolved; no successful retry is asserted. |

The CLI additionally reports `chat_context_implicit_refs_unsupported` before
implicit reference reads. All CLI refusals exit nonzero.
The `503` API details explicitly contain `accepted: false` and
`receiptIssued: false`. These are refusal details, not durable execution
receipts. No new capability is advertised or implied by the parser.

## Required owner integration and qualification

`familiar_identity.rs` currently resolves a roster ID, display name, and role.
It provides no canonical root/revision ledger. The automation
`AuthorityEvidenceVerifier` interface consumes a different, automation-specific
extension; its fixture implementations are not a production chat verifier.
Ward/Threads identity predicates and automation consumer projections must not
be substituted for the Familiar embodiment contract.

The canonical Familiar contract already defines `direct_session` in
[`RFC-0001`, section 10.2](https://github.com/OpenCoven/familiar-contract/blob/main/rfcs/RFC-0001-familiar-contract.md).
Its owner-side consumer needs a retained verified identity bundle, independently
trusted signer/key policy, current authoritative ledger generation/head/status,
freshness and revocation checks, and a final eligibility check atomic with
binding commitment. The canonical repository's JavaScript validator accepts
trusted evidence supplied by its caller; it is not an operational ledger or a
configured trusted service for this binary. This unit does not mint roots,
provision trust, shell out to an arbitrary verifier, or accept request-supplied
ledger observations.

### Checked authority sources

The available canonical implementation is
[`validators/validate.js`](https://github.com/OpenCoven/familiar-contract/blob/main/validators/validate.js).
`validateEmbodimentBinding` checks portable evidence. `verifyEd25519` verifies
against the public key carried by that evidence, and `--trusted-ledger` reads a
file supplied by its caller. Neither operation establishes that the signer is
trusted by Coven or that the ledger observation came from an authorized live
source. A passing canonical fixture therefore cannot enable dispatch.

Source inspection found no operational canonical issuer/current-ledger client
in the available Coven, Cave, Psyche, Coven Agents, or Threads runtime source.
The existing roster resolver and automation verifier interface cannot supply
it. No production trust configuration, live revocation source, or authorized
identity bundle was supplied for this admission boundary.

### Conditions for enabling acceptance

This implementation completes the unavailable capability and rejection
boundary. It contains no hidden success path, test-verifier switch, or accepted
receipt placeholder. To enable acceptance in a later change, the owning
authority must provide:

- An operational issuer/ledger integration for the *existing*
  `familiar.embodiment_binding.v1` contract, independently trusted signer policy,
  authenticated principal/project/scope mappings, retained bundle access, and
  current head, generation, freshness, and revocation observations.
- Authorized source bytes and revisions for each manifest reference, including
  mandatory identity/policy, with exact root/revision and target binding.
- An adapter profile qualified against an isolated native home, controlled
  workspace/configuration, forbidden-context canaries, and explicitly authorized
  provider tests. Native read-only flags do not establish hidden-context
  exclusion or Temporary custody.

With those inputs, a production consumer still needs source verification,
effective permission and canonical scope binding, and existing-store receipt
transactions atomic with final eligibility and dispatch consumption. That is
additional source work, not something a publication or environment toggle
completes. Until then, do not interpret a self-consistent digest, known roster
slot, healthy daemon, or successful parser test as accepted familiar continuity.

No retained-side lifecycle, import/writeback, memory ingestion, accepted
receipt store, or native isolation acceptance is implemented here.
