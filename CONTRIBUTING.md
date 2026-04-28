# Contributing to OpenCoven / Coven

Coven is built as a small, boring Rust authority layer with TypeScript integration packages around it. The development loop should keep that boundary clear.

## Prerequisites

- Rust stable toolchain
- Git
- Node.js 18+ and `pnpm` for package/plugin work
- A supported harness CLI for manual smoke tests, usually Codex or Claude Code

## Local Development

1. Build the workspace:

```bash
cargo build --workspace
```

2. Run the Rust checks:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

3. Check local harness availability:

```bash
cargo run -p coven-cli -- doctor
```

4. Exercise the CLI from a disposable project when changing runtime behavior:

```bash
cargo run -p coven-cli -- daemon start
cargo run -p coven-cli -- run codex "say hello from coven"
cargo run -p coven-cli -- sessions
cargo run -p coven-cli -- daemon stop
```

Use a throwaway repository for smoke runs. Do not run untrusted prompts or harnesses in sensitive projects.

## Recommended Daily Workflow

1. Keep one clean checkout for running tests and release checks.
2. Use one feature branch/worktree per change.
3. Keep Rust runtime changes separate from package/plugin documentation where possible.
4. Re-run `cargo fmt`, `cargo clippy`, and `cargo test` before opening a PR.
5. If you touch `packages/openclaw-coven`, also run that package's TypeScript tests/checks once a package manager workflow is present.

## Architecture Rules

- Rust owns process launch, cwd/project-root validation, PTY lifecycle, session persistence, and socket request enforcement.
- Socket clients are not trusted, including comux and the OpenClaw plugin.
- TypeScript clients may improve UX, but must not become the authority boundary.
- Keep v0 harness support focused on Codex and Claude Code until policy and adapter contracts are stable.
- Do not place Coven code in OpenClaw core. The integration belongs in `packages/openclaw-coven` and publishes as `@opencoven/coven`.

## Documentation Rules

OpenCoven docs should be public, direct, and concrete:

- Say **OpenCoven** for the ecosystem/org and **Coven** for the CLI/daemon product.
- Keep the terminal command as `coven`; do not tell users to run `opencoven` or `@opencoven` as a command.
- Use the canonical community references: `discord.gg/opencoven` and `@OpenCvn`.
- Do not imply stable package availability until the package/release path is ready.
- Keep VMUX/comux comparisons high-level: Coven is the runtime substrate, comux is the cockpit, VMUX is not required to understand Coven.

## Pull Request Workflow

1. Keep changes scoped and reviewable.
2. Run the relevant checks:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

3. Include smoke-test notes for runtime or API changes.
4. Update docs when command behavior, API behavior, or trust boundaries change.

## Maintainer Checklist Before Release

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

For package releases, also verify package contents with a dry run and attach checksums for native binaries.
