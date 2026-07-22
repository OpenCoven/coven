# Contributing to OpenCoven / Coven

> **Contribution Status — Updated July 2026**
>
> External Pull Requests are open. Please start from an issue for larger changes
> and include the readiness packet requested by the PR template.

Coven is built as a small, boring Rust authority layer with TypeScript integration packages around it. The development loop should keep that boundary clear.

OpenCoven is MIT licensed and community-driven. We want contributing to be easy, open, and safe for everyone.

## How to Contribute

1. Fork the repository (or branch directly if you have push access).
2. Create one branch per change, named for the concern it addresses in the same
   conventional-commit style used for subjects (`feat:`, `fix:`, `docs:`,
   `chore:`, `refactor:`) — for example `fix/448-cli-help-text`.
3. Make your changes with signed-off commits: `git commit -s`. Sign-off is
   required for every commit — see
   [OpenCoven DCO and Patent Terms](#opencoven-dco-and-patent-terms) below.
4. Open a pull request with a clear description and fill in the PR template.

For larger changes, start from an issue first and include the readiness packet
requested by the PR template. AI agents should also read [AGENTS.md](AGENTS.md)
for the claim registry and worktree workflow layered on top of this guide.

## Prerequisites

- Rust stable toolchain
- Git
- Node.js 18+ and `pnpm` for package/plugin work
- A supported harness CLI for manual smoke tests, usually Codex or Claude Code

## Contributor First 10 Minutes

Use this path for a fresh source checkout before opening a docs or code PR:

```bash
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
cargo test -p coven-cli --test smoke -- --nocapture
```

A healthy first pass means the workspace builds, `doctor` prints the local store/project/harness status, and the smoke test passes. It is okay if `doctor` reports Codex or Claude Code as missing as long as it gives install/setup guidance; install and authenticate a real harness only before running manual `coven run ...` sessions.

The smoke test is safe for first-time contributors because it uses an isolated temporary `COVEN_HOME` and injects a fake `codex` executable into `PATH`. It does not require private Codex or Claude credentials.

After this first pass, use the fuller local loop below. For product context, start with the [README](README.md) and [Getting started](docs/GETTING-STARTED.md) guide.

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

4. Run the contributor-safe smoke loop when changing daemon, session, attach, or session ritual behavior:

```bash
cargo test -p coven-cli --test smoke -- --nocapture
```

The smoke test uses an isolated temporary `COVEN_HOME` and injects a fake `codex` executable into `PATH`, so it does not require private Codex or Claude credentials.

5. Exercise the CLI manually from a disposable project when changing runtime behavior:

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
- Keep harness support focused on Codex, Claude Code, and GitHub Copilot CLI until policy and adapter contracts are stable.
- Do not place Coven code in OpenClaw core. The integration belongs in `packages/openclaw-coven` and publishes as `@opencoven/coven`.

## Documentation Rules

OpenCoven docs should be public, direct, and concrete:

- Say **OpenCoven** for the ecosystem/org and **Coven** for the CLI/daemon product.
- Keep the terminal command as `coven`; do not tell users to run `opencoven` or `@opencoven` as a command.
- Use the canonical community references: `discord.gg/opencoven` and `@OpenCvn`.
- Be precise about package status: npm packages are published for early adopters, but Coven remains an early local-first MVP.
- Keep VMUX/comux comparisons high-level: Coven is the runtime substrate, comux is the cockpit, VMUX is not required to understand Coven.

## Pull Request Workflow

1. For larger changes, start from an issue and include the readiness packet
   requested by the PR template.
2. Keep changes scoped and reviewable.
3. Sign off every commit (`git commit -s`); see
   [OpenCoven DCO and Patent Terms](#opencoven-dco-and-patent-terms).
4. Run the relevant checks:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

5. Include smoke-test notes for runtime or API changes.
6. Update docs when command behavior, API behavior, or trust boundaries change.

## OpenCoven DCO and Patent Terms

### Developer Certificate of Origin (DCO)

OpenCoven uses the **Developer Certificate of Origin (DCO) v1.1** for all contributions. This is a lightweight mechanism — not a CLA — that asks you to certify that you have the right to submit what you're submitting.

By making a contribution to this project, you certify that:

> (a) The contribution was created in whole or in part by you and you have the right to submit it under the open source license indicated in the file; or
>
> (b) The contribution is based upon previous work that, to the best of your knowledge, is covered under an appropriate open source license and you have the right under that license to submit that work with modifications, whether created in whole or in part by you, under the same open source license (unless you are permitted to submit under a different license), as indicated in the file; or
>
> (c) The contribution was provided directly to you by some other person who certified (a), (b) or (c) and you have not modified it.
>
> (d) You understand and agree that this project and the contribution are public and that a record of the contribution (including all personal information you submit with it, including your sign-off) is maintained indefinitely and may be redistributed consistent with this project or the open source license(s) involved.

### How to Sign Off

Add a `Signed-off-by` line to your commit message:

```
git commit -s -m "Your commit message"
```

This produces:

```
Your commit message

Signed-off-by: Your Name <your.email@example.com>
```

### Patent Non-Assertion

By contributing, you additionally agree not to assert any patent claims — now held or later acquired — against this project or its users that arise from your contribution. See [PATENTS](./PATENTS) for the full non-assertion pledge.

## What We're Looking For

- Bug fixes and reliability improvements
- Documentation and example improvements
- New skills, tools, and integrations
- Performance improvements
- Community-requested features

## What We're Not

OpenCoven is not a contribution vehicle for proprietary forks. If you are building a closed-source derivative of OpenCoven's architecture, please do not use contribution as a means to learn implementation details that are not yet public. We welcome genuine collaborators.

## Maintainer Checklist Before Release

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

For package releases, also verify package contents with a dry run and attach checksums for native binaries.

## Questions?

Join the Discord: https://discord.gg/opencoven
