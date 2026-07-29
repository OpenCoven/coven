# Capability Discovery Cache Design

## Goal

Make fresh capability reads entirely memory-backed and ensure filesystem
discovery never holds the shared cache lock.

## Scope

The affected API routes are `GET /api/v1/capabilities/harnesses` and
`GET /api/v1/capabilities/:harnessId`. Their existing `refresh=1` and five-minute
TTL behavior remain unchanged. Coven continues to read harness and skill
directories only; this change introduces no writes to those directories.

## Design

Replace the mutable manifest-only cache with an immutable, complete response
snapshot containing harness manifests, Coven skills, and the discovery time.
Store that snapshot behind an `RwLock`.

1. A non-refresh request first takes a shared lock. If the snapshot is within
   the TTL, it clones the snapshot and returns without a harness or skill
   directory scan.
2. A refresh request, or an expired/missing snapshot, collects every harness
   manifest and the Coven skill list outside the lock.
3. After collection succeeds, it takes the write lock only long enough to
   atomically replace the complete snapshot, then returns the same immutable
   result.
4. `get_one` reads from that same snapshot, so it cannot race a second cache
   lookup after `get_all` has warmed it.

Concurrent cold or forced refreshes may perform duplicate scans. This is
intentional: readers never wait for filesystem I/O and no partial state is
published. A successful later refresh simply replaces the prior complete
snapshot.

## Failure behavior

Harness scanners preserve their existing warning-bearing manifests. A skill
scan failure remains represented as an empty skill list, matching the current
route behavior. Publishing occurs only after the complete replacement snapshot
has been built, so an already-valid cached response stays intact until a new
one is ready.

## Verification

Add focused tests proving that:

- a warm cache returns the original complete snapshot without rescanning
  harnesses or Coven skills;
- a refresh builds outside the shared read lock, allowing unrelated readers to
  return the last complete snapshot while the refresh is blocked in discovery;
- an explicit refresh atomically replaces the entire response and `get_one`
  sees the matching manifest;
- the capability benchmark covers concurrent reads and reports the hot-path
  result.

Run the existing Rust, JavaScript, secret, privacy, and CI gates before merge.
