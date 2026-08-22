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
- bounded channels apply backpressure instead of allowing unbounded memory growth;
- a peer disconnect closes the remaining peer;
- idle connections expire;
- the room is destroyed after both peers disconnect.

Knowing a relay room and credential only permits rendezvous. The tunneled OpenCoven protocol must still authenticate both endpoints and enforce the current device grant.

## Limits

The current server bounds:

- active rooms: 1,024;
- peers per room: one host and one client;
- WebSocket message size: 4 MiB;
- WebSocket frame size: 64 KiB;
- queued live frames per peer: 32;
- idle lifetime: 120 seconds.

These defaults are deliberately conservative and may become explicit deployment configuration after production measurements. The relay never logs room credentials or application frames.

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
