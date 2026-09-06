# Threads real-daemon E2E

The `threads_e2e` integration target exercises the Ward/Threads boundary
through a real `coven daemon` process and its local IPC transport. Each test
uses a unique temporary `COVEN_HOME`, familiar workspace, SQLite store, pending
directory, and daemon socket.

Run the advisory smoke journeys from a Coven checkout:

```sh
cargo test --locked -p coven-cli --test threads_e2e -- --nocapture
```

The initial target covers the bounded permit/apply, unsigned protected
rejection, and out-of-band drift/staging paths. These are process-boundary
smoke tests, not closure evidence for all eight journeys in
[OpenCoven/coven#884](https://github.com/OpenCoven/coven/issues/884).

## Testing a local Threads checkout

A downstream Threads job overlays its checkout of `coven-threads-core` into the
pinned Coven revision. Set the following variable to make the test fail before
daemon startup unless `cargo metadata` proves that a local path package is
active:

```sh
COVEN_THREADS_E2E_REQUIRE_LOCAL_OVERRIDE=1 \
  cargo test --locked -p coven-cli --test threads_e2e -- --nocapture
```

The downstream job remains responsible for applying its ephemeral Cargo patch
and updating the ephemeral lock state before invoking the locked test command.
Normal Coven runs exercise the repository's reviewed Git-pinned Threads
revision.

## Evidence

Every journey writes JUnit to:

```text
target/e2e-artifacts/<run-id>/junit.xml
```

On failure, the same directory also contains the synthetic request and
response, exact Coven and Threads revisions, daemon recovery log, Ward audit
rows, hashed pending/workspace inventories, and SQLite schema. The fixtures use
only synthetic identities and content; evidence never reads a developer's real
Coven home.
