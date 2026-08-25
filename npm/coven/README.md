# @opencoven/cli

Node wrapper for the native Coven Rust CLI.

Install or run the published wrapper:

```sh
npm install -g @opencoven/cli
coven doctor

# or without global install:
npx @opencoven/cli doctor
```

The wrapper installs platform-specific native packages through `optionalDependencies` and runs the matching `coven` binary for your OS and CPU. No Rust toolchain is required for end users after a supported package is published.

`@opencoven/coven-memory-dashboard` is a separate opt-in install, not a
dependency of this wrapper. When it is present, `coven memory open` passes only
that package's installed entrypoint and the current Node executable to the
native CLI, which launches the private loopback-only dashboard; when it is
absent, the command prints the install instruction. Existing `coven memory` and
`coven memory --json` continue to print the memory list either way.

The wrapper and native CLI support Node.js 18 or newer. The dashboard requires
Node.js 24 or newer; on Node.js 18–23, `coven memory open` prints an upgrade
instruction while other Coven commands continue to work.

## v0 platform scope

Current early-adopter packages target:

- `@opencoven/cli-macos` for macOS Apple Silicon
- `@opencoven/cli-macos-x64` for Intel macOS x64
- `@opencoven/cli-linux-x64` for glibc-based Linux x64 distributions
- `@opencoven/cli-windows` for Windows x64

Alpine Linux is not supported.

## Desktop no-window launch on Windows

An embedding desktop client that intentionally owns no terminal may set
`COVEN_WINDOWS_HIDE_NATIVE_WINDOW=1` when it starts this npm wrapper. On
Windows x64 the wrapper then launches `coven.exe` with Node's `windowsHide`
option and pipe-backed, backpressure-aware forwarding for stdin, stdout, and
stderr. Avoiding inherited stdio handles is required for `CREATE_NO_WINDOW` to
take effect. Other values and ordinary CLI launches retain the default visible
or inherited console, direct stdio, and Ctrl-C behavior. This signal affects
only the wrapper-to-native boundary; Coven independently suppresses console
windows for noninteractive harness children.

Desktop integrations that need an exact native process handle may run the npm
wrapper with the sole argument `--print-native-binary-path`. On success it
prints exactly one absolute native Coven binary path plus `\n` and exits 0
without launching Coven. The flag cannot be combined with another argument;
resolution or validation failures write a diagnostic to stderr and exit 1.
Callers can then spawn that path directly with `coven process-supervisor
--protocol coven.process-supervisor.v1`, avoiding a JavaScript-wrapper process
between their owned child handle and Coven's OS process-tree containment.

`process-supervisor` is a narrow desktop-integration contract, not an ordinary
interactive CLI. Its stdin must begin with exactly one LF-terminated JSON frame:

```json
{"version":1,"program":"/absolute/path/to/program","args":["arg"],"cwd":"/absolute/existing/directory"}
```

The complete frame, including its LF, is limited to 256 KiB. Unknown fields,
NUL bytes, non-absolute `program` or `cwd` values, and a nonexistent `cwd` are
rejected before launch. The first stderr line is always prefixed with
`COVEN_PROCESS_SUPERVISOR_V1 ` and contains either a JSON `ready` event or an
`error` event (`unsupported_protocol`, `invalid_request`, or `spawn_failed`).
After `ready`, target stdout and stderr are forwarded unchanged and the
supervisor mirrors the target's exit result. Keep supervisor stdin open as the
ownership lease; EOF or an owner termination signal cancels and reaps the
complete target process tree. The target does not receive this control stdin.
On Windows, clients must use the native-path discovery contract above and own
the resulting native supervisor process directly; placing the npm wrapper
between the client and supervisor would weaken exact-handle ownership.
