# coven-relay

Stateless WebSocket relay for Hexes — bridges the Coven gateway running on a Mac to iOS Hexes clients.

See `docs/specs/2026-05-31-hexes-implementation-plan.md` Track 2 for the full
implementation roadmap.

## Running locally

```sh
cargo run -p coven-relay
# or with a custom address:
LISTEN_ADDR=127.0.0.1:9000 cargo run -p coven-relay
```

Health check: `curl http://localhost:8080/healthz`

## Deploy

Deployed on Fly.io (`personal` org, region `ord`, hostname `relay.opencoven.dev`).

```sh
cd crates/coven-relay/deploy
fly deploy
```

Requires `FLY_API_TOKEN` in the environment (or `fly auth login`).
