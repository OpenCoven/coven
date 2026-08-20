# BOUNDS.md

Coven-wide spend authority for OpenCoven.

This file is **readable by every familiar and writable by none**. It is only
authoritative while the signature below verifies against Val's key. A familiar
that edits the block invalidates the signature, and an unverified policy grants
nothing — it denies every paid action.

```bound
version: 1
scope: coven
issued_at: 2026-08-20T00:00:00Z
not_after: 2027-08-20T00:00:00Z
period: monthly-utc
ceiling_usd: 25.00
mode: deny-paid
metered:
  - model.tokens
  - model.images
  - model.audio
  - tools.hosted
  - search.api
  - compute.render
  - storage.egress
  - infra.paid
authority: http://127.0.0.1:8787
authority_key: ed25519:qKnyQMcdaW96J2WGc43uJp2RRTOOg7FrO97E4rXhc0A
gateway: http://127.0.0.1:8788
grant_ttl_seconds: 120
clock_skew_seconds: 60
```

## What this means in practice

- Paid work stops at USD 25.00 per UTC calendar month across the whole coven.
- Free and local work — reading files, editing code, running tests — is never
  gated by Bound and continues after the cap is reached.
- Every paid action must first obtain a short-lived signed grant. The gateway
  holds the provider credentials; familiars hold none.
- Splitting work across subagents does not multiply the budget. One ledger
  serves the whole period.

## Changing this file

Only Val can change it, and only by re-signing:

```
node bin/bound.ts sign BOUNDS.md coven
```

The signing key lives outside this repository. There is no path by which a
familiar can produce a valid signature.

<!-- bound:sig v=1 alg=ed25519 key=val-hw-2026-08 sig=pG5Kb3Ug_pXCxRsE6JA20gTJ5uF8DsEUkFOGUFarVFRMEus8JVINGA9R-ESAldelDvYhX1x8U_oIJwyPrB80AA -->
