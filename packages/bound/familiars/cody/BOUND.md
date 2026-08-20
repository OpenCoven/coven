# BOUND.md — cody

Per-familiar spend override for the coding familiar.

An override may only narrow. The coven ceiling in `BOUNDS.md` is applied as a
hard clamp after this file is merged, so no signed override can raise a
familiar above the coven ceiling.

```bound
version: 1
scope: familiar:cody
issued_at: 2026-08-20T00:00:00Z
not_after: 2027-08-20T00:00:00Z
ceiling_usd: 10.00
```

Absent this file, cody would inherit the full coven ceiling. Present but
unsigned, cody would get **zero** paid budget — an unverifiable override is
never quietly ignored.

<!-- bound:sig v=1 alg=ed25519 key=val-hw-2026-08 sig=Lr1QKDVtwcrTcEoHmVVKrVHyjqJDvPv1O_WNWu7jvir-uGXsNHu6RNMaHjdAQfjJfzpSp9Vs46y_058U1aeUCg -->
