# opencoven-coven-client

`opencoven-coven-client` is the owner-adjacent Rust client for the OpenCoven
**Coven** daemon. It speaks the named `coven.daemon.v1` local API contract over
same-user IPC — a Unix domain socket under the daemon home on Unix-like
systems, an owner-only named pipe on Windows — and gives a Rust program the
same discovery, transport, health, and error surface that the `coven` CLI
itself is built on.

The crate is **pre-1.0** and stays in the
[`OpenCoven/coven`](https://github.com/OpenCoven/coven) repository as a
workspace member next to the daemon it talks to. It is published under the
package name `opencoven-coven-client`; the library it exposes is still named
`coven_client`, so code written against the workspace crate does not change.

```toml
[dependencies]
coven-client = { package = "opencoven-coven-client", version = "0.1" }
```

```rust
use coven_client::{DaemonClient, DaemonEndpoint, ReadEndpoint, PROTOCOL_VERSION};

fn main() -> Result<(), coven_client::ClientError> {
    // Resolve the daemon for this user's Coven home and refuse anything that is
    // not the owner-only local endpoint.
    let coven_home = std::env::var_os("COVEN_HOME").expect("COVEN_HOME is set");
    let endpoint = DaemonEndpoint::discover(&coven_home)?;
    let mut client = DaemonClient::new(endpoint);

    // Negotiate health first: the client refuses a daemon whose apiVersion is
    // not `coven.daemon.v1` or that lacks structured errors.
    let health = client.health()?;
    assert_eq!(health.api_version, PROTOCOL_VERSION);
    println!("coven {} ok={}", health.coven_version, health.ok);

    // Typed reads and writes bind every request to the negotiated peer.
    let sessions: serde_json::Value = client.get_json(ReadEndpoint::Sessions {
        limit: Some(20),
        cursor: None,
        include_archived: false,
    })?;
    println!("{sessions}");
    Ok(())
}
```

Item-level documentation (`cargo doc -p opencoven-coven-client --open`) is
the authority for signatures; the example shows the shape of a session.

## What the crate does

- **Discovery.** `DaemonEndpoint` locates the running daemon for the current
  user, validates the socket or pipe path, and reads the bounded daemon status
  file (`MAX_DAEMON_STATUS_BYTES`).
- **Transport.** `DaemonClient` sends bounded HTTP/1.1 requests over the local
  transport and enforces response-size and response-deadline limits
  (`MAX_RESPONSE_BODY_BYTES`, `is_response_deadline_timeout`).
- **Contract.** `Health`, `HealthCapabilities`, `ReadEndpoint`, and
  `WriteEndpoint` model the `coven.daemon.v1` envelope. `PROTOCOL_VERSION`
  names the contract this crate was built against.
- **Errors.** `ClientError` covers transport and validation failures;
  `DaemonError` carries the daemon's structured error body.

Items marked `#[doc(hidden)]` are lifecycle hooks for the daemon's own CLI
(probe, shutdown, Windows status writing). They are not part of the supported
public surface and may change in any release.

## What the crate does not do

- It has **no CLI, TUI, or agent-runtime dependency**. `coven-cli` composes
  over this crate; this crate never depends on `coven-cli` or `coven-agents`.
- It does not open network sockets. The daemon's API is same-user local IPC by
  design; see the repository's API contract documentation.
- It does not embed credentials, tokens, or a home-directory scanner beyond
  the documented daemon-home resolution.

## Compatibility policy

- The crate follows SemVer precedence while pre-1.0: a **minor** bump may
  break the public surface; a **patch** bump must not.
- The API contract is additive within `coven.daemon.v1`. Unknown fields in
  the health envelope are ignored, missing optional capabilities default to
  `false`, and a daemon that reports a different `apiVersion` is refused
  rather than guessed at.
- Each published version records the daemon versions it was tested against in
  the release notes. Pin `version = "0.1"` and upgrade deliberately.

## Package contents

`cargo package` ships exactly: this README, `Cargo.toml`, `src/**`, `tests/**`,
and `fixtures/**`. The fixtures are part of the contract — the package-contract
test reads them — so a tarball without them cannot pass its own tests.

`tests/package_contract.rs` asserts the package name, pre-1.0 version,
license/repository/readme metadata, the `coven_client` library name, the
absence of forbidden dependencies, crate-level docs, and fixture presence.
`scripts/verify-coven-client-package.mjs` at the repository root checks the
same contract from outside Cargo and is what the crate release workflow runs
before `cargo publish --dry-run`.

## Releasing

Publication is owned by `.github/workflows/release-crates.yml` in the
repository: a signed, annotated `coven-client-v<version>` tag on `main`, the
full workspace gates, `cargo package --locked`, the package verifier, a
`cargo publish --dry-run`, and then — only when crates.io authority is
configured for the repository — the real publish. See
`docs/reference/coven-client-crate.md`.

## License

MIT, as the rest of the OpenCoven Coven workspace.
