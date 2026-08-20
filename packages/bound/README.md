# Bound

**A spend cap no agent can raise.**

Bound is a signed spend-authority system for Coven familiars. It answers one
question: how do you set a billing cap that the agents subject to it cannot
edit, disable, argue with, or route around?

The design lives in
[`docs/superpowers/specs/2026-08-20-bound-spend-authority-design.md`](../../docs/superpowers/specs/2026-08-20-bound-spend-authority-design.md).

## Run it

No install step. Node 24 runs the TypeScript directly.

```bash
cd packages/bound
node bin/bound.ts dev
```

Then open the dashboard URL it prints (default <http://127.0.0.1:8787>). Dev
mode also binds the gateway on `:8788` and the demo provider stub on `:8789`,
so the gateway URL a familiar reads out of the signed `BOUNDS.md` is the URL
that actually enforces.

In a second terminal:

```bash
node demo/agent.ts      # honest familiars spending until the ceiling stops them
node demo/attacks.ts    # eleven bypass attempts, every one denied
node --test "test/*.test.ts"
```

`npm run typecheck` runs strict `tsc` over the package if you have TypeScript
available; it is a check only, never a build step — nothing here is compiled.

## Why a markdown file is not the control

`BOUNDS.md` and `familiars/<id>/BOUND.md` are ordinary agent files, peers to
`SOUL.md` and `IDENTITY.md`. A familiar can read them. A familiar can even
write to them — and gain nothing, because two mechanisms carry the authority:

1. **Val-only signing.** The fenced ` ```bound ` block is covered by a detached
   ed25519 signature from a key held outside this repository. Editing the block
   invalidates the signature, and an unverified policy grants nothing. Try it:

   ```bash
   node bin/bound.ts status                       # policy verified
   sed -i '' 's/ceiling_usd: 25.00/ceiling_usd: 9999.00/' BOUNDS.md
   node bin/bound.ts status                       # policy NOT verified — all paid actions denied
   git checkout BOUNDS.md
   ```

2. **Credential separation.** Familiars hold no provider credentials. The
   gateway holds them, and refuses to use them without a short-lived signed
   grant that only the authority can mint. An agent cannot exceed a budget it
   has no way to spend against.

## Shape

```
BOUNDS.md                 signed coven policy — $25.00 / UTC month
familiars/cody/BOUND.md   signed override — $10.00 sub-budget
bin/bound.ts              keygen · sign · verify · status · dev
src/canonical.ts          bound:v1 block parsing and canonical signing bytes
src/policy.ts             signature verification, global-plus-overrides merge
src/keystore.ts           key material, kept outside the repository
src/ledger.ts             reserve / settle accounting, one ledger per period
src/grant.ts              short-lived signed grants, nonce replay guard
src/authority.ts          the only minter of grants; owns the ledger
src/gateway.ts            the only holder of provider credentials
src/provider-stub.ts      fake metered vendor, so the demo costs nothing
src/client.ts             the surface an agent links against
src/dashboard.ts          live read-only view
demo/                     honest agent + adversarial suite
```

## Policy semantics

| Rule | Behaviour |
| --- | --- |
| Ceiling | USD 25.00 per UTC calendar month, coven-wide |
| Overrides | `BOUND.md` may only narrow; the coven ceiling is a hard clamp |
| Unsigned override | Zero paid budget — never silent inheritance |
| Breach | Paid actions denied; free and local work continues |
| Any failure | Deny immediately. No cache, no grace period, no fail-open |
| Subagents | One ledger per period, so sharding does not multiply budget |
| Denials | Terminal. No retry, no cheaper-model fallback |

## What the adversarial suite proves

`node demo/attacks.ts` runs each of these against a live stack:

| # | Attempt | Denied by |
| --- | --- | --- |
| 1 | Familiar raises its own cap in `BOUND.md` | signature no longer verifies |
| 2 | Familiar invents an unsigned override | unsigned override means zero |
| 3 | Familiar runs its own authority and self-issues grants | gateway pins the key from signed policy |
| 4 | Familiar calls the provider directly | it holds no credential |
| 5 | Hand-forged grant | grant signature check |
| 6 | Replaying a spent grant | nonce guard |
| 7 | Requesting more output than granted | gateway clamps to the grant |
| 8 | Ten subagents splitting the spend | one shared ledger |
| 9 | Spending while the authority is offline | fail closed |
| 10 | Editing the ledger or policy over HTTP | no write route exists |
| 11 | Reading the provider credential | it never crosses the wire |

## Local demo vs production

| | Local demo | Production |
| --- | --- | --- |
| Signing key | 0600 file outside the repo | Secure Enclave / hardware token |
| Authority | loopback Node process | Cloudflare Worker |
| Ledger | serialized in-process queue + JSON | Durable Object, one per period |
| Gateway credential | demo string | Workers Secrets Store |
| Provider | local stub, zero real cost | real metered vendors |

The trust model is identical in both. Only the hosting changes.

## Keys

```bash
node bin/bound.ts keygen                          # Val signing key
node bin/bound.ts keys                            # list trusted key ids
node bin/bound.ts sign BOUNDS.md coven            # re-sign after an authorized edit
node bin/bound.ts verify BOUNDS.md coven
```

Keys live in `$BOUND_KEYSTORE` (default: the familiar workspace's `.bound/keys`,
deliberately outside this repository). Private key material is never committed.
