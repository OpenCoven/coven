# Bound Spend Authority Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a runnable v1 of Bound — a signed, agent-unforgeable USD 25.00/month spend ceiling for OpenCoven familiars — that Val can start in dev mode on this machine and demo end to end, including live bypass attempts that all deny.

**Architecture:** Three loopback HTTP services plus a signed policy file. The **authority** owns the ledger and mints short-lived ed25519-signed grants; the **gateway** is the only holder of provider credentials and refuses any request without a valid grant; a **provider stub** stands in for a metered vendor so the demo costs no real money. Policy lives in `BOUNDS.md` / `BOUND.md` and is authoritative only when a detached Val signature verifies. Design source: `docs/superpowers/specs/2026-08-20-bound-spend-authority-design.md`.

**Tech Stack:** Node 24 with native TypeScript type stripping, zero runtime dependencies, `node:crypto` ed25519, `node:http`, `node:test`. No install step, so `node packages/bound/bin/bound.ts dev` works immediately.

---

## File Structure

```
packages/bound/
  package.json              # scripts only: dev, demo, test, keygen, sign
  README.md                 # what Bound is, how to run the demo
  BOUNDS.md                 # signed coven policy (ceiling 25.00)
  familiars/cody/BOUND.md   # signed per-familiar override
  bin/bound.ts              # CLI: keygen | sign | verify | serve | dev | demo | status
  src/
    canonical.ts            # bound block subset parser + canonical bytes for signing
    policy.ts               # document parse, signature verify, effective policy merge
    keystore.ts             # key material outside the repo; load/generate/list
    pricing.ts              # metered action classes and bounded cost estimation
    ledger.ts               # SpendLedger: reserve, settle, expire, receipts, persistence
    grant.ts                # grant mint + verify, nonce replay guard
    authority.ts            # HTTP: /v1/grant /v1/settle /v1/policy /v1/ledger /v1/receipts
    gateway.ts              # HTTP: /v1/proxy — credential holder, clamp, meter, settle
    provider-stub.ts        # HTTP: fake metered vendor, returns usage
    dashboard.ts            # HTML + /v1/state feed for the live demo surface
    client.ts               # BoundClient an agent uses: grant -> gateway -> BoundDenied
    http.ts                 # tiny shared route/json helpers
  demo/
    agent.ts                # honest familiar burning budget until the cap denies
    attacks.ts              # the 10 adversarial bypass attempts from the spec
  test/
    canonical.test.ts
    policy.test.ts
    ledger.test.ts
    grant.test.ts
    adversarial.test.ts
```

Boundaries: `policy.ts` does no I/O, `ledger.ts` knows nothing about HTTP, `gateway.ts` is the only module that reads a provider credential, and `client.ts` is the only module an agent links against.

---

## Task 1 — Canonical form and policy parsing

- [ ] Write `test/canonical.test.ts`: a `bound` block parses to the expected object; key order changes produce identical canonical bytes; prose outside the block does not change canonical bytes; a second `bound` block is an error; unknown version is an error.
- [ ] Run it, confirm it fails.
- [ ] Implement `src/canonical.ts`: extract the single fenced `bound` block, parse the restricted `key: value` / `- item` subset, reject duplicates and tabs, emit canonical bytes as `bound:v1\n<role>\n<issued_at>\n<sorted-json>`.
- [ ] Run the tests, confirm they pass.
- [ ] Commit.

## Task 2 — Signature verification and effective policy

- [ ] Write `test/policy.test.ts`: valid signature verifies; tampered block, tampered role, tampered `issued_at`, wrong key id, unknown key, and expired `not_after` all fail with distinct reason codes; merge inherits, honours explicit `0`, clamps an override above the ceiling, and gives an unsigned override zero budget.
- [ ] Run it, confirm it fails.
- [ ] Implement `src/policy.ts` — `parseDocument`, `verifyDocument`, `computeEffectivePolicy(global, overrides)` — returning `{ ok, reason }` results and never throwing on bad input.
- [ ] Add the fast-check style property assertion: for arbitrary overrides, `effective.ceilingUsd <= global.ceilingUsd`.
- [ ] Run the tests, confirm they pass.
- [ ] Commit.

## Task 3 — Keystore and signing CLI

- [ ] Implement `src/keystore.ts`: keys live under `BOUND_KEYSTORE` (default `<familiar-workspace>/.bound/keys`, deliberately outside the repo), `0600` files, `val-hw-*` signing key plus an authority key.
- [ ] Implement `bin/bound.ts keygen` and `bound sign <file> --role <role>` writing the `<!-- bound:sig ... -->` trailer, and `bound verify <file>`.
- [ ] Generate the demo keys, write and sign `BOUNDS.md` (ceiling 25.00) and `familiars/cody/BOUND.md`.
- [ ] Run `bound verify` on both; confirm that hand-editing a ceiling makes verify fail.
- [ ] Commit (policy files yes, key material never).

## Task 4 — SpendLedger

- [ ] Write `test/ledger.test.ts`: reserve then settle reduces remaining by the actual amount; N parallel reserves never exceed the ceiling; settle is idempotent per `grantId`; expired reservations release exactly once; a familiar sub-budget denies before the global ceiling; period rollover starts clean.
- [ ] Run it, confirm it fails.
- [ ] Implement `src/ledger.ts` with a serialized async queue per `(coven, period)`, atomic JSON persistence, and append-only receipts.
- [ ] Run the tests, confirm they pass.
- [ ] Commit.

## Task 5 — Grants

- [ ] Write `test/grant.test.ts`: a minted grant verifies; tampered field, expired `exp`, skew beyond tolerance, replayed nonce, and out-of-scope provider/model/limits all reject with reason codes.
- [ ] Run it, confirm it fails.
- [ ] Implement `src/grant.ts` (`mintGrant`, `verifyGrant`, `NonceGuard`) and `src/pricing.ts` bounded estimation per action class.
- [ ] Run the tests, confirm they pass.
- [ ] Commit.

## Task 6 — Authority, gateway, provider stub

- [ ] Implement `src/http.ts`, then `src/authority.ts` (`/v1/grant`, `/v1/settle`, `/v1/policy`, `/v1/ledger`, `/v1/receipts`, `/v1/denials`), loading and verifying policy on every grant and failing closed on any verification, digest, or ledger error.
- [ ] Implement `src/provider-stub.ts` returning usage for a fake model, rejecting a missing or wrong credential.
- [ ] Implement `src/gateway.ts`: verify grant, clamp requested limits down to the grant, inject the credential, meter actual usage, settle, and return a receipt. Deny when the authority is unreachable.
- [ ] Implement `src/client.ts` exposing `spend()` that returns either a result or a structured `BoundDenied` with a reason code, and never retries a denial.
- [ ] Add `bin/bound.ts dev` to start all three on loopback and print the dashboard URL.
- [ ] Smoke test by hand with curl: one successful paid call end to end.
- [ ] Commit.

## Task 7 — Dashboard

- [ ] Implement `src/dashboard.ts`: server-rendered HTML at `/` plus a `/v1/state` JSON poll showing period, ceiling, settled, reserved, remaining, per-familiar sub-budgets, recent receipts, and recent denials with reason codes.
- [ ] Verify with curl that `/` returns 200 and `/v1/state` reflects a real settled charge.
- [ ] Commit.

## Task 8 — Demo and adversarial suite

- [ ] Implement `demo/agent.ts`: an honest familiar that spends repeatedly until it is denied at the cap, then proves a free local action still succeeds.
- [ ] Implement `demo/attacks.ts` and `test/adversarial.test.ts` covering all ten spec scenarios: self-edited `BOUND.md`, unsigned new override, spoofed authority/gateway env vars, direct provider call, forged grant, replayed grant, oversized `max_tokens`, ten subagents splitting spend, authority offline, ledger write attempt. Each must assert a denial with a specific reason code.
- [ ] Run the whole suite, confirm green.
- [ ] Commit.

## Task 9 — Wire-up, docs, verification

- [ ] Write `packages/bound/README.md`: what Bound is, the trust model, `node bin/bound.ts dev`, `demo`, `test`, and the production differences (Secure Enclave signing, Workers deployment, real provider keys).
- [ ] Add `packages/bound/package.json` scripts `dev`, `demo`, `attacks`, `test`.
- [ ] Run the full suite plus a live dev-mode run; capture the denial at the cap.
- [ ] Commit.

---

## Verification gates

```
node --test packages/bound/test          # all suites, adversarial included
node packages/bound/bin/bound.ts dev     # loopback services + dashboard
node packages/bound/demo/agent.ts        # honest spend to the cap
node packages/bound/demo/attacks.ts      # 10 bypass attempts, all denied
```

Enforcement is not considered working until every adversarial case denies **and** a free action immediately afterwards still succeeds.

## Out of scope for this plan

Cloudflare deployment, Durable Object ledger, Secure Enclave signing, the public `bound.md` / `bounds.md` sites, the Cave panel, and real provider credentials. Those follow once the local product is proven.
