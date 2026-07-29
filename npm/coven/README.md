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

It also installs `@opencoven/coven-memory-dashboard` as an optional companion.
`coven memory open` passes only that package's installed entrypoint and the
current Node executable to the native CLI, which launches the private
loopback-only dashboard. Existing `coven memory` and `coven memory --json`
continue to print the memory list.

The wrapper and native CLI support Node.js 18 or newer. The optional dashboard
requires Node.js 24 or newer; on Node.js 18–23, `coven memory open` prints an
upgrade instruction while other Coven commands continue to work.

## v0 platform scope

Current early-adopter packages target:

- `@opencoven/cli-macos` for macOS Apple Silicon
- `@opencoven/cli-linux-x64` for glibc-based Linux x64 distributions
- `@opencoven/cli-windows` for Windows x64

Alpine Linux is not supported.
