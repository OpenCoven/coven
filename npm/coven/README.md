# @opencoven/cli

Node wrapper for the native Coven Rust CLI.

After an approved v0 release is published for macOS Apple Silicon:

```sh
npm install -g @opencoven/cli
coven doctor
```

The wrapper installs platform-specific native packages through `optionalDependencies` and runs the matching `coven` binary for your OS and CPU. No Rust toolchain is required for end users after a supported package is published.

## v0 platform scope

The first publishing proof targets:

- `@opencoven/cli-macos` for macOS Apple Silicon
- `@opencoven/cli-linux-x64` for glibc-based Linux x64 distributions

Alpine Linux is not supported.
