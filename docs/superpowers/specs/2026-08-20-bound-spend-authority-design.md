# Bound: Signed Spend Authority for Coven Agents

## Decision

Introduce **Bound**, a signed spend-authority system that enforces a hard
billing cap no familiar, agent, subagent, or automation can raise, disable, or
route around.

Bound has two faces:

- an **agent file convention** — `BOUNDS.md` (coven-wide) and `BOUND.md`
  (per-familiar override), living beside `SOUL.md` and `IDENTITY.md`;
- a **protocol and enforcement path** — a remote, Val-signed authority that
  issues short-lived spend grants, and a credential-holding gateway that is the
  only route to metered providers.

The two domains we own carry the two faces:

| Domain | Role |
| --- | --- |
| `bounds.md` | Human documentation: the format, examples, rationale, versioning. |
| `bound.md` | Machine surface: `/v1/schema.json`, `/.well-known/bound-keys.json`, format canonicalization rules. |

The authority and gateway for our own coven run on private OpenCoven endpoints.
`bound.md` and `bounds.md` publish the open format and our public verification
keys; they never hold budget state and are never in the enforcement path.

First rollout target: **OpenCoven agents** (`crates/coven-agents`).

## Why the file alone is not the control

A markdown file inside an agent's own workspace is advisory. An agent that can
write files can rewrite its own limit; an agent that holds a provider API key
can spend without asking anyone. So the file is the *statement of policy*, and
two mechanisms make it *authority*:

1. **Val-only hardware signing.** `BOUNDS.md` and `BOUND.md` are meaningless
   unless accompanied by a valid detached signature from a hardware-backed key
   that only Val holds (Secure Enclave or hardware token). No familiar, harness,
   CI job, or service holds the private key. An agent editing the file
   invalidates the signature, and an invalid file grants nothing.
2. **Signed remote authority with credential separation.** The ledger runs
   off-host. Agents never hold provider credentials; they hold short-lived,
   scoped, signed grants. The gateway holds the real keys and refuses any
   request without a valid grant.

An agent cannot exceed the cap because it never possesses the thing that spends
money.

## Approved parameters

| Parameter | Value |
| --- | --- |
| Cap | **USD 25.00** per UTC calendar month, coven-wide |
| Scope | **All metered costs** — model tokens, hosted tools, image/audio/video generation, TTS/STT, search APIs, render and compute minutes, paid storage/egress, paid infrastructure and domain actions |
| Enforcement mode | **Deny paid actions only** — free and local work continues |
| Policy layering | **Global plus overrides** — `BOUNDS.md` sets the ceiling; per-familiar `BOUND.md` sets sub-budgets |
| Trust root | **Val-only hardware signing** |
| Authority | **Signed remote authority**, not a local check |
| Failure posture | **Fail closed immediately** — no grace period, no cached allow, no fail-open |

### Global plus overrides

Effective policy for a familiar is computed as:

```
effective = merge(BOUNDS.md, BOUND.md[familiar])
```

Rules:

- Both files must carry a valid Val signature. An unsigned or stale-signed
  override is not "ignored in favour of the global" — it makes the familiar's
  effective policy **zero paid budget** until repaired.
- An override may set any sub-budget less than or equal to the global ceiling.
  A signed override *may* raise a familiar above another familiar's share, but
  never above the global `ceiling_usd`; the global ceiling is a hard clamp
  applied after merge.
- Absent override key means "inherit"; explicit `0` means "no paid actions".
- Sub-budgets do not need to sum to the ceiling. The ceiling is enforced
  independently, so one familiar cannot consume another's headroom past the
  global remaining balance.

### File format (`bound:v1`)

`BOUND.md` and `BOUNDS.md` are readable markdown with one fenced `bound` block
and one detached signature comment.

````markdown
# BOUNDS.md

Coven-wide spend authority. Edited and signed by Val only.

```bound
version: 1
scope: coven
issued_at: 2026-08-20T00:00:00Z
not_after: 2027-08-20T00:00:00Z
period: monthly-utc
ceiling_usd: 25.00
mode: deny-paid   # deny-paid | shadow — only Val's signature can change this
metered:
  - model.tokens
  - model.images
  - model.audio
  - tools.hosted
  - search.api
  - compute.render
  - storage.egress
  - infra.paid
authority: https://authority.opencoven.ai
authority_key: ed25519:BASE64URL
gateway: https://gateway.opencoven.ai
grant_ttl_seconds: 120
clock_skew_seconds: 60
```

<!-- bound:sig v=1 alg=ed25519 key=val-hw-2026-08 sig=BASE64URL -->
````

The signature covers the canonical serialization of the `bound` block bytes,
the document role (`coven` or `familiar:<id>`), and `issued_at`. Canonical
serialization rules are published at `https://bound.md/v1/canonical`. Anything
outside the fenced block is prose and is not signed, so editing the surrounding
documentation never invalidates authority — and never grants any.

## Architecture

Four layers, each with a single job:

1. **Policy layer** — signed `BOUNDS.md` / `BOUND.md` in git. Declarative,
   reviewable, diffable. Holds no state.
2. **Authority layer** — the Bound Authority service. Holds the ledger, verifies
   policy signatures, issues signed grants. Off-host, not writable by agents.
3. **Enforcement layer** — the Bound Gateway. The only process holding provider
   credentials. Accepts a request only with a valid, unexpired, in-scope,
   unreplayed grant.
4. **Audit layer** — append-only receipts and an operator surface in Cave.

The critical invariant: **the enforcement layer and the credential store are the
same process, and neither is reachable by agent-authored configuration.** The
authority URL, authority public key, and gateway URL are read from the *signed*
policy, not from environment variables an agent can set.

## Components

| Component | Home | Responsibility |
| --- | --- | --- |
| `bound-policy` | `crates/bound-policy` (Rust) | Parse `bound:v1`, verify signatures, compute effective policy, expose a digest. No I/O beyond reading given bytes. |
| `bound-authority` | Cloudflare Worker | `POST /v1/grant`, `POST /v1/settle`, `GET /v1/policy`, `GET /v1/ledger`. Verifies caller identity and policy digest before any grant. |
| `SpendLedger` | Durable Object, one per `(coven, period)` | Strongly consistent reserve/settle accounting. Single-threaded per period, so concurrent reserves cannot race past the cap. |
| `bound-gateway` | Cloudflare Worker + Secrets Store | Holds provider keys. Verifies grant signature, TTL, nonce, scope, and provider-side limits. Forwards, meters actual cost, calls `settle`. |
| `BoundGuardrail` | `crates/coven-agents` | Implements the existing `InputGuardrail`/tool-gate traits. Requests grants, converts denials into a structured `BoundDenied` outcome rather than a panic or retry loop. |
| Bound panel | Coven Cave | Shows period spend, remaining, per-familiar sub-budgets, recent denials, incidents. Read-only. |
| `bound.md` / `bounds.md` | Cloudflare Pages | Public spec, JSON schema, published verification keys. Static. |

## Data flow

Happy path for one paid action:

1. An agent reaches a metered action. `BoundGuardrail` estimates a bounded
   maximum cost (model price × `max_tokens`, or the published unit price).
2. The agent calls `POST /v1/grant` with its identity, the action class, the
   estimate, and the policy digest it believes is current.
3. The authority verifies the caller identity, verifies the Val signature over
   the stored policy, compares digests, and asks the `SpendLedger` DO to
   **reserve** the estimate against the remaining balance.
4. On success the authority returns a signed grant:
   `{grant_id, familiar, action_class, provider, model, max_usd, max_units,
   nonce, exp}` — TTL ≤ 120 s.
5. The agent calls the gateway with the grant. The gateway verifies signature,
   `exp`, clock skew, nonce freshness, and that the requested provider/model/
   limits are inside the grant. It clamps provider-side limits (for example
   `max_tokens`) to the grant so actual cost cannot exceed the reservation.
6. The gateway injects the real provider credential, forwards the call, and
   meters actual usage.
7. The gateway calls `POST /v1/settle` with `{grant_id, actual_usd, units}`.
   The ledger converts the reservation into a settled charge and appends a
   receipt. Settle is idempotent on `grant_id`.
8. Unsettled reservations expire at `exp` and release their hold, so a crashed
   agent cannot permanently consume budget.

An agent never sees a provider key, never talks to a provider directly, and
cannot mint a grant, because grant signing happens only inside the authority.

## Failure handling

Every failure resolves to **deny the paid action, immediately, with a reason**.
There is no fail-open branch anywhere in the design, and no code path that
treats "authority unavailable" as "assume allowed".

| Failure | Behaviour |
| --- | --- |
| Missing, malformed, or invalid policy signature | Authority refuses to issue any grant for that scope. All paid actions denied. |
| Policy `not_after` passed | Same as invalid signature. Policy must be re-signed. |
| Digest mismatch between the repo copy and the authority copy | Deny, and raise an integrity incident. Never auto-reconcile toward the more permissive copy. |
| Authority unreachable, timeout, or 5xx | Gateway denies. No cached grant, no offline allowance, no retry-until-allowed. |
| Ledger write failure | Reserve fails, therefore no grant, therefore deny. Accounting is the gate, not a side effect. |
| Clock skew beyond `clock_skew_seconds` | Grant rejected at the gateway. |
| Replayed nonce or reused `grant_id` | Rejected and recorded as an abuse incident. |
| Grant scope mismatch (different provider, model, or larger limits) | Rejected. |
| Cap reached | Paid actions denied; free and local actions continue; one notification to Val per period, not per attempt. |
| Actual cost exceeds reservation | Bounded by gateway-clamped provider limits. Any residual overage settles, marks an over-reservation incident, and tightens the estimator multiplier for that action class. |
| Malformed markdown in a policy file | Parse failure is a denial, never a fallback to defaults. |

Additional guarantees:

- **No cheaper-path escape.** Every metered provider is behind the same
  gateway, so "try a cheaper model instead" still requires a grant.
- **No sharding escape.** Subagents and spawned sessions get their own
  identities but share one `SpendLedger` per period. Splitting work across
  agents does not multiply the budget.
- **No retry storms.** `BoundDenied` is terminal for that action. The agent
  records a blocked status and continues free work or stops; it does not loop.
- **Kill switch.** Val can publish a signed policy with `ceiling_usd: 0.00`, or
  a signed revocation for a key id. Because grant TTL is ≤ 120 s, the maximum
  exposure window after revocation is one TTL.
- **Denials are visible.** Every denial carries a machine-readable reason code
  and surfaces in the Cave Bound panel, so a silent stall is never mistaken for
  a working system.
- **Shadow mode is not a failure mode.** `mode: shadow` (see Rollout) suppresses
  blocking only while the policy verifies. Every row of the table above still
  denies in shadow mode, because an unverifiable policy has no mode at all.

## Testing

Bound is a security control, so the test suite is written adversarially: the
default assumption is that an agent will try to get around it.

### Unit

- `bound-policy` parser: valid documents, missing block, two blocks, unknown
  keys, wrong version, non-UTF-8, truncated signature comment.
- Signature verification vectors: valid, tampered block, tampered role,
  tampered `issued_at`, wrong key id, revoked key, expired `not_after`.
- Effective-policy merge: inherit, explicit zero, sub-budget below ceiling,
  sub-budget above ceiling (clamped), unsigned override (zero budget).
- Cost estimator: per-provider unit pricing, bounded maximum derivation.

### Property and fuzz

- For arbitrary override inputs, `effective.ceiling_usd <= global.ceiling_usd`
  always holds.
- For arbitrary byte inputs, the parser either fails or yields a policy no more
  permissive than the signed source. No input produces a permissive default.

### Ledger concurrency

- N concurrent reserves against a remaining balance never exceed the cap.
- Settle is idempotent under duplicate delivery.
- Expired reservations release exactly once.
- Period rollover opens a fresh ledger without carrying reservations forward.

### Integration

Run against `wrangler dev` / Miniflare with the Worker test harness:

- Happy path: grant → gateway → provider stub → settle → receipt.
- One case per row of the failure table above, asserting denial *and* asserting
  that a free action issued immediately afterwards still succeeds.

### Adversarial suite

Each of these must produce a denial, and each is a permanent regression test:

1. Agent edits its own `BOUND.md` to raise the cap → signature invalid → denied.
2. Agent writes a brand-new `BOUND.md` for an unknown familiar → unsigned →
   zero budget.
3. Agent sets `BOUND_AUTHORITY` / `BOUND_GATEWAY` environment variables to a
   fake endpoint → endpoints come from signed policy, gateway holds the keys →
   denied.
4. Agent calls the provider API directly → holds no credential → denied.
5. Agent forges a grant → signature check fails → denied.
6. Agent replays a settled grant → nonce rejected → denied.
7. Agent requests a larger `max_tokens` than the grant allows → clamped at the
   gateway → cost cannot exceed reservation.
8. Agent spawns ten subagents to split the spend → shared ledger → denied at
   cap.
9. Agent takes the authority offline (simulated) and retries → denied, not
   allowed.
10. Agent attempts to edit the ledger or receipts → no write path exists.

### Chaos

Authority down, DO storage error, network partition, ±10 minute clock skew.
Assert deny-closed in all cases and assert free work is unaffected.

### Verification gates

```
cargo test -p bound-policy
cargo test -p coven-agents bound_guardrail
pnpm --filter bound-authority test
pnpm --filter bound-gateway test
```

The adversarial suite must be green before enforcement is enabled for any
familiar.

## Rollout

1. **Ship the format.** Publish `bound:v1` schema, canonicalization rules, and
   public keys on `bound.md`; publish documentation on `bounds.md`.
2. **Sign the first policy.** Val signs `BOUNDS.md` with `ceiling_usd: 25.00`.
3. **Shadow phase, scoped to `coven-agents` only.** The guardrail requests
   grants and records outcomes but does not block, for a fixed, explicitly
   ended window. Shadow is a rollout stage owned by Val, expressed in the signed
   policy as `mode: shadow`; an agent cannot re-enter it, because changing the
   mode requires Val's hardware key.
4. **Enforce for OpenCoven agents.** Flip the signed policy to
   `mode: deny-paid`. Verify a real denial at the cap and verify free work
   continues.
5. **Extend outward** to Cave-initiated work, research missions, and media
   generation once the adversarial suite covers each new metered class.

## Out of scope for v1

- Multi-tenant hosting of Bound for other people's covens.
- Per-action human approval prompts (Bound is a ceiling, not an approval queue).
- Cost forecasting or optimization.
- Non-USD currencies and non-monthly periods.
- Trusting any provider's own budget dashboard as an enforcement mechanism;
  provider caps are a backstop, not the control.
