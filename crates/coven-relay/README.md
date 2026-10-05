---
source_adjacent_reason: "Documents the relay wire protocol, limits, and crate-local conformance evidence."
---

# coven-relay

`coven-relay` is a bounded, ephemeral WebSocket rendezvous service for OpenCoven devices. It allows a Coven/Psyche host and a mobile client to meet through outbound connections when direct networking is unavailable.

The relay is **not** an OpenCoven identity or authorization authority. It forwards opaque binary frames only. Endpoint authentication, device grants, transcript verification, and application encryption remain end to end between the host and client.

## Protocol v1

Connect to:

```text
/ws?v=1&room=<ROOM_ID>&role=<host|client>
Authorization: Bearer <ROOM_CREDENTIAL>
```

`ROOM_ID` and `ROOM_CREDENTIAL` are separate canonical base64url encodings of 32 random bytes. They are short-lived rendezvous material, not durable device credentials.

The first peer creates an in-memory room. Exactly one `host` and one `client` may occupy that room at a time, and both must present the same credential. Once connected:

- only binary application messages are forwarded;
- text frames are rejected;
- ping/pong remains transport-local;
- no offline buffering or message persistence occurs;
- bounded channels and a relay-wide byte budget apply backpressure instead of allowing unbounded memory growth;
- a peer disconnect closes the remaining peer;
- idle connections expire;
- the room is destroyed after both peers disconnect.

Knowing a relay room and credential only permits rendezvous. The tunneled OpenCoven protocol must still authenticate both endpoints and enforce the current device grant.

## Limits

The current server bounds:

- active rooms: 1,024;
- peers per room: one host and one client;
- inbound WebSocket message size: 4 MiB;
- inbound WebSocket frame size: 64 KiB;
- queued application messages per peer: 32;
- queued and in-flight application data across the relay: 16 MiB;
- outbound send lifetime: 10 seconds;
- idle lifetime: 120 seconds.

Fragmented inbound messages are reassembled and forwarded as one binary message with unchanged payload bytes. Outbound frame boundaries can differ from inbound frame boundaries.

These defaults are deliberately conservative and may become explicit deployment configuration after production measurements. The diagnostic subscriber accepts only relay-owned event targets, even with `RUST_LOG=trace`: dependency logs can contain HTTP credentials or frame data and are suppressed. Relay-owned diagnostics must never include room identifiers, credentials, or application frames.

## Conformance checkpoint (#787)

Run the transport suite independently:

```sh
CARGO_TARGET_DIR=/tmp/coven-relay-target cargo test -p coven-relay --locked ws::wire
CARGO_TARGET_DIR=/tmp/coven-relay-target cargo clippy -p coven-relay --all-targets --locked -- -D warnings
```

The suite starts the actual Axum handler on ephemeral loopback TCP ports and
crosses HTTP/1.1 upgrade and RFC 6455 framing. The small masked-frame codec and
controllable server writes are test-only fixtures using existing dependencies.
Readiness comes from accepted sockets and registered role slots; queue/permit
predicates and write notifications establish ordering. Tests keep the production
send and idle deadlines, including a real 120-second idle-expiry case.

Proven cases include:

- host-first and client-first rendezvous, unchanged binary messages in both
  directions, empty messages, 64 KiB frames, and fragmented 4 MiB messages;
- malformed/versioned queries, missing/malformed/duplicate bearer headers,
  wrong room credentials, duplicate roles, and room capacity rejection;
- text, unmasked frames, reserved opcodes, oversized control frames, oversized
  data frames and fragmented messages; ping/pong stays local;
- queue and global byte-budget exhaustion with a blocked writer; permits remain
  charged during sends and are released on delivery, failure, or teardown;
- graceful and abrupt disconnect, disconnect with a full data queue, failed
  HTTP upgrade, broken socket writes, send deadline and idle expiry;
- captured verbose diagnostics exclude rendezvous secrets and payload markers.

Disconnect signaling has its own bounded slot, so a full data queue cannot
silently discard it. An in-flight send may finish or reach its deadline before
closure; queued application frames are discarded when the connection exits.

This is **relay-server conformance**, not completion of trusted-device
reconnection. This crate neither implements nor proves endpoint authentication,
application encryption, device authorization, grant enforcement, client
reconnection/resumption, LAN discovery, or direct/relay fallback. Those remain
endpoint/integration work under #787. Possessing a room credential authorizes
rendezvous only, never application actions. The relay can observe, alter, or
forge unprotected application data; this checkpoint makes no claim of resistance
to a malicious relay. Clients must establish the authenticated encrypted channel
end to end before sensitive traffic or production acceptance.

## Running locally

```sh
cargo run -p coven-relay
# or with a custom address:
LISTEN_ADDR=127.0.0.1:9000 cargo run -p coven-relay
```

Health check:

```sh
curl http://localhost:8080/healthz
```

## Deployment

The existing Fly.io deployment configuration lives in `deploy/`.

```sh
cd crates/coven-relay/deploy
fly deploy
```

A production rollout should remain gated until host/mobile clients tunnel the authenticated OpenCoven connection over this broker and integration tests prove that relay compromise cannot read or forge application traffic.
