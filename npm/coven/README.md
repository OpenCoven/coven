# @opencoven/cli

Node wrapper for the native Coven Rust CLI.

After an approved v0 release is published for macOS Apple Silicon:

```sh
npm install -g @opencoven/cli
coven doctor
```

The wrapper installs platform-specific native packages through `optionalDependencies` and runs the matching `coven` binary for your OS and CPU. No Rust toolchain is required for end users after a supported package is published.

## v0 platform scope

The first publishing proof targets `@opencoven/cli-darwin-arm64` (macOS Apple Silicon). Other platform packages are listed for the stable package contract and will be filled out by follow-up release work. Installing on any other platform will result in a missing-platform-package error until those packages are published.
