# Mobile Pairing Retry Recovery Design

**Issue:** [#570](https://github.com/OpenCoven/coven/issues/570)
**Parent recovery track:** [#541](https://github.com/OpenCoven/coven/issues/541)

## Goal

Make completed mobile pairing confirmations idempotent for the rest of the
existing pairing window. A host or device that retries the final confirmation
must receive the same completed pairing result — the identical
`MobilePairedDevice` data, even if transport-level `request_id` values differ —
instead of a duplicate registration attempt or a terminal
`pairing_consumed` failure.

## Scope

This change is limited to the mobile pairing flow in
`crates/coven-cli/src/mobile_memory/pairing.rs` and the two confirmation
callers in `crates/coven-cli/src/mobile_memory/gateway.rs`.

In scope:

- retrying `confirm_host` after the device already completed pairing;
- retrying `confirm_device` after the host already completed pairing;
- preserving strict phrase validation on every retry;
- keeping completed-state retention bounded by the same expiry already used for
  pending pairings;
- preventing duplicate `PairingCompleted` audit entries caused by replayed host
  confirmations.

Out of scope:

- persisting pending or completed pairing retries across daemon restart;
- changing the pairing phrase derivation, nonce handling, or device registry
  schema;
- redesigning mobile auth, transport, or post-pairing device scopes.

## Current behavior

`PairingManager::confirm` currently removes the `PendingPairing` entry as soon
as both sides confirm (`pairing.rs:239-294`). That means the first successful
second confirmation registers the device and returns `PairingProgress::Complete`,
but any later retry for the same pairing id immediately fails because the entry
is gone.

The current mismatch path also removes the pairing on any wrong phrase before
returning `PairingPhraseMismatch`. That eviction is useful while the pairing is
still incomplete, because it shuts the window after a bad confirmation attempt.
However, once a device has already been registered, removing the completed entry
on a typo breaks idempotent recovery for the legitimate client without adding
meaningful registration safety.

The preserved recovery patch at
`.git/agent-recovery/issue-541/dirty/mobile-memory-gateway/worktree.patch`
proves the intended direction: cache the completed `MobilePairedDevice` inside
`PendingPairing` and return it on later matching confirmations. That patch is a
useful signal, but it should not be applied verbatim against current `main`
because it leaves two gaps:

1. replayed completions are indistinguishable from the first completion, so the
   internal host confirmation route would append duplicate
   `pairing_completed` audit events;
2. the retention contract is implicit rather than spelled out against the
   existing five-minute pairing expiry.

## Requirements

1. A completed retry with the correct phrase returns the same completed result
   as the first successful completion: identical `MobilePairedDevice` data,
   while outer response metadata such as `request_id` may differ.
2. The device registry is written exactly once per pairing, regardless of
   replayed confirmations.
3. Phrase validation is not weakened: a wrong phrase still returns
   `PairingPhraseMismatch` and never returns a device.
4. Incomplete pairings keep the current single-mismatch invalidation behavior.
5. Completed-state retention is bounded by the pairing's existing `expires_at`
   window and uses the same expiry checks already enforced by `phrase`,
   `confirm_host`, and `confirm_device`.
6. Host-side replays must not append duplicate `PairingCompleted` audit lines.

## Approaches considered

### Option A — Cache the completed device inside `PendingPairing` until expiry (recommended)

Add a completed snapshot to the existing in-memory pairing entry, validate the
phrase before every replay, and return the cached device on any matching retry.
Expose replay metadata from `PairingManager` so `gateway.rs` can preserve audit
and HTTP semantics.

**Pros**
- Small, surgical change in the existing Rust authority layer.
- Reuses the current expiry model and cleanup behavior.
- Avoids a second registry write entirely instead of depending on registry
  uniqueness errors.
- Keeps all phrase validation logic in one place.

**Cons**
- The retry window remains in-memory only and is lost on daemon restart.
- `PairingProgress` must grow enough metadata for callers to distinguish first
  completion from replay.

### Option B — Remove the pairing entry and reconstruct the device from `DeviceRegistry`

Delete the pending entry as today, then try to find the paired device by public
key or another derived identifier when a confirmation retries.

**Why not**
- The current registry has no lookup by pairing transcript or public key.
- Removing the pairing entry also removes the transcript hash needed to keep
  phrase validation authoritative.
- It would conflate retry recovery with persistent device management and would
  still need extra state to distinguish first completion from replay.

### Option C — Add a separate completed-pairing retry map

Move completed pairings into a second in-memory structure with its own cleanup.

**Why not**
- It duplicates the expiry and pruning logic already present in the pending map.
- It increases bookkeeping surface without adding capability beyond Option A.
- It makes the recovery harder to reason about for no acceptance benefit.

## Decision

Adopt **Option A**.

`PendingPairing` will keep a completed device snapshot in the same entry after
successful registration. The confirmation path will still validate the phrase on
every call, but it will only destroy the entry on mismatch while the pairing is
still incomplete. Once a device is registered, a matching retry returns the
cached device and a replay flag; a mismatched retry returns
`PairingPhraseMismatch` without discarding the completed snapshot.

The cached completed entry expires exactly when the original pairing invitation
expires. No new file, timer, or registry persistence is introduced.

## Detailed design

### Pairing state model

Extend `PendingPairing` with:

- `completed: Option<MobilePairedDevice>` — the redacted device record returned
  to clients after the first successful completion.

Change `PairingProgress` from a simple enum payload to:

```rust
pub enum PairingProgress {
    Pending,
    Complete {
        device: MobilePairedDevice,
        replayed: bool,
    },
}
```

`replayed: false` means the current confirmation performed the one allowed
registry write. `replayed: true` means the pairing was already complete and the
manager returned the cached result.

This keeps idempotence and audit semantics in the authority boundary instead of
forcing callers to guess.

### Confirmation algorithm

Update `PairingManager::confirm` in this order:

1. Load the pairing entry and fail with `PairingExpired` exactly as today if
   `now >= expires_at`, removing the entry on expiry.
2. Require an enrolled transcript hash exactly as today.
3. Derive the expected phrase and compare it before any state mutation.
4. On mismatch:
   - if `completed.is_none()`, keep the current behavior and remove the pairing
     before returning `PairingPhraseMismatch`;
   - if `completed.is_some()`, return `PairingPhraseMismatch` but retain the
     completed entry so a later correct retry can still succeed.
5. If `completed` is already present and the phrase matched, return
   `PairingProgress::Complete { replayed: true, .. }` immediately.
6. Otherwise apply the host or device confirmation flag.
7. If both sides are not yet confirmed, return `Pending`.
8. If this is the first completion, register the device, build the redacted
   `MobilePairedDevice`, store it in `completed`, clear the transient
   `device: Option<PendingDevice>` payload, and return
   `PairingProgress::Complete { replayed: false, .. }`.

The device registry remains unchanged. Duplicate registrations are prevented by
short-circuiting before `DeviceRegistry::register` can run a second time.

### Gateway behavior

`gateway.rs` needs to consume the replay flag in both confirmation callers.

- `/api/v1/mobile/pairings/{id}/confirm`
  - first completion: return `201` with the device payload;
  - replayed completion: return `200` with the same completed device payload.
- `/api/v1/internal/mobile/pairings/{id}/confirm`
  - return `200` for both first completion and replay;
  - append `MobileAuditEvent::PairingCompleted` only when
    `replayed == false`.

No other mobile routes change. `success_response` continues to generate a fresh
mobile envelope `request_id` on each response, so replay equivalence is defined
as identical parsed completion data rather than byte-for-byte response-body
equality.

### Retention and cleanup

Completed confirmations stay in `PairingManager.pending` only until the pairing
expires. That is the same lifetime already created by `begin_pairing` and the
same boundary currently enforced in `phrase` and both confirmation methods.

This means:

- the retry window is bounded by the existing five-minute `PAIRING_LIFETIME` in
  `gateway.rs`;
- a completed retry after expiry still returns `PairingExpired` and removes the
  cached entry;
- daemon restart behavior is unchanged because pending pairings are already
  in-memory only.

## Testing strategy

Add focused Rust tests in `pairing.rs` for:

- incomplete-pairing mismatch invalidation still removing the pairing and
  closing the retry window;
- device replay after host-first completion;
- host replay after device-first completion;
- wrong phrase after completion still failing while leaving the good retry path
  intact;
- completed retries expiring on the original deadline.

Add focused Rust tests in `gateway.rs` for:

- replayed device confirmations returning `201` on first completion, `200` on
  replay, and identical parsed success-envelope `data` even though `request_id`
  may differ;
- replayed host confirmations not appending a second `pairing_completed` audit
  record.

## Repository gates

The implementation plan must end with these repository gates:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

A targeted inner loop should run before the full gates:

```bash
cargo test -p coven-cli mobile_memory --locked -- --nocapture
```

## Risks and guardrails

- **Audit duplication:** handled by surfacing `replayed` from the manager and
  suppressing duplicate `PairingCompleted` writes in `handle_local_control`.
- **Lifetime creep:** avoided by keeping the completed snapshot inside the
  existing pairing entry instead of a new store.
- **Phrase validation drift:** avoided by reusing the existing transcript hash
  and checking it before replay short-circuiting.
- **Scope creep into persistence:** avoided by leaving `DeviceRegistry` and
  mobile auth storage unchanged.
