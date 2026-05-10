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

## v0 platform scope

Current early-adopter packages target:

- `@opencoven/cli-macos` for macOS Apple Silicon
- `@opencoven/cli-linux-x64` for glibc-based Linux x64 distributions
- `@opencoven/cli-windows` for Windows x64, starting with the next release that includes Windows artifacts

Alpine Linux is not supported.
