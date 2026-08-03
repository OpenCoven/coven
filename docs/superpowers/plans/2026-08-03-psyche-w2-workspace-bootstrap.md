# Psyche W2 Workspace Bootstrap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the standalone `OpenCoven/psyche` Rust workspace with a daemon, CLI, strict versioned configuration, structured redacting logs, a full CI gate, and a checksum-verified npm distribution — proving every `coven-psy1` acceptance criterion without implementing any Telegram, graph, identity, or Coven execution behaviour.

**Architecture:** Four crates only. `psyche-core` owns versioned schema identifiers, error types, and the `SecretRef` newtype that structurally cannot hold a literal secret. `psyche-config` parses TOML strictly — unknown fields are errors outside a versioned `extensions` table, and unknown `schema_version` is denied *before* field validation so a future config reports the real reason. `psyche-runtime` is the sole composition root and owns a three-state lifecycle with graceful shutdown. `psyche-cli` exposes `psyche` and `psyched` with `start`/`stop`/`status`/`doctor`, all of which run with no Telegram credentials present. The npm wrapper resolves a platform companion package and verifies its SHA-256 before exec.

**Tech Stack:** Rust 2021 (MSRV 1.85), serde + toml, clap 4 (derive), tokio (rt-multi-thread, signal), tracing + tracing-subscriber (JSON), thiserror, assert_cmd + predicates, cargo-deny, gitleaks, Node 22 + npm.

---

## Scope boundary

**This plan is the `coven-psy1` bootstrap slice only.** It is deliberately narrower than workstream W2 as a whole.

| In scope | Out of scope (and where it goes) |
|---|---|
| `psyche-core`, `psyche-config`, `psyche-runtime`, `psyche-cli` | The other 12 crates in `specs/psyche/TECH.md` §"Repository and crate boundaries" |
| Config `schema_version` unknown-version denial | Store migrations, state-machine/property/crash tests → **follow-on G2 plan** |
| Daemon lifecycle + graceful shutdown | Intent ledger, graph, identity snapshots → W3/W4 |
| CI gate + npm dry-run distribution | Signing, SBOM, provenance, publication → **G12** |
| — | Any Telegram, Coven socket, or add-on behaviour → W5/W6/W9 |

`PLAN.md` §3 gives W2 the exit condition "G2 passes with unknown-version denial, migration, state-machine, property, and crash tests". This plan lands **only** the unknown-version denial portion. Migrations, fakes, state-machine, property, and crash tests require `psyche-store` and canonical schemas, which are a separate bounded unit; attempting both in one plan would breach the boundedness the G3 gate exists to enforce. **W2 does not exit on this plan alone.**

## Compliance with `specs/psyche/PLAN.md` §6 child-plan standard

| # | Requirement | Where satisfied |
|---|---|---|
| 1 | Names exact files, schemas, state transitions, public boundaries | "File map" below; lifecycle transitions in Task 4; `psyche.config.v1` in Task 2 |
| 2 | Starts with failing unit/contract/property/crash tests | Every task's Step 1 writes a failing test; Step 2 runs it and records the expected failure |
| 3 | One bounded worktree, issue/Bead, and shared claim | Task 0 |
| 4 | Preserves Rust authority boundary and thin TypeScript packages | Task 8 — the npm package resolves and execs a binary; it holds no daemon, storage, identity, or policy logic |
| 5 | Defines fake and real conformance without adapter-only relaxation | Task 7 runs the identical gate locally and in CI; no CI-only skips |
| 6 | Lists security/privacy/secret failure cases | "Security and privacy failure cases" below; enforced in Tasks 3, 6, 7, 8 |
| 7 | Records verification commands and expected terminal evidence | "Verification commands" below; every task step states expected output |
| 8 | Maps the change to one or more gates | "Gate mapping" below |
| 9 | Stops at approval, publish, migration, or production cutover gates | Task 0 stops before repo creation; Task 8 is dry-run only; Task 9 stops at review |

**Fixed decisions this plan does not reopen:** the canonical package is `@opencoven/psyche` with `@opencoven/psyche-<platform>-<arch>` companions (TECH.md §"Architecture decision"); TypeScript may not own daemon, storage, identity, graph, policy, verification, or surface transport (PLAN.md §4 W2); capability flags stay false until their gate passes (PLAN.md §5), so this plan sets none.

## File map

- `Cargo.toml` — workspace root; members, shared `[workspace.package]` metadata, MSRV, release profile.
- `rust-toolchain.toml` — pins the toolchain so local and CI gates are identical.
- `deny.toml` — cargo-deny licence allowlist and advisory policy.
- `crates/psyche-core/Cargo.toml` — core crate manifest.
- `crates/psyche-core/src/lib.rs` — re-exports `schema` and `secret`.
- `crates/psyche-core/src/schema.rs` — `CONFIG_SCHEMA_VERSION`, `SchemaError`, `ensure_schema_version`.
- `crates/psyche-core/src/secret.rs` — `SecretRef` newtype with redacting `Debug`/`Display` and a single greppable accessor.
- `crates/psyche-config/Cargo.toml` — config crate manifest.
- `crates/psyche-config/src/lib.rs` — `Config`, `CovenConfig`, `ConfigError`, `load_str`, `load_path`.
- `crates/psyche-runtime/Cargo.toml` — runtime crate manifest.
- `crates/psyche-runtime/src/lib.rs` — `Runtime`, `LifecycleState`, `RuntimeError`, graceful `shutdown`.
- `crates/psyche-cli/Cargo.toml` — CLI crate manifest with two `[[bin]]` targets.
- `crates/psyche-cli/src/main.rs` — `psyche` entry point; clap command tree.
- `crates/psyche-cli/src/bin/psyched.rs` — daemon entry point.
- `crates/psyche-cli/src/doctor.rs` — credential-free environment checks returning structured results.
- `crates/psyche-cli/src/logging.rs` — JSON `tracing` subscriber install.
- `crates/psyche-cli/tests/cli.rs` — end-to-end CLI assertions via `assert_cmd`.
- `.github/workflows/ci.yml` — fmt, clippy, locked tests, cargo-deny, gitleaks, npm dry-run.
- `packages/psyche-npm/package.json` — `@opencoven/psyche` wrapper manifest.
- `packages/psyche-npm/bin/psyche.js` — resolves the platform companion, verifies SHA-256, execs.
- `packages/psyche-npm/scripts/verify-checksum.js` — checksum helper shared by the wrapper and its test.
- `packages/psyche-npm/test/verify-checksum.test.js` — Node built-in test runner coverage for the checksum path.
- `docs/CONFIGURATION.md` — the `psyche.config.v1` contract as shipped.

## Security and privacy failure cases

Each is enforced by a named test, not by review vigilance.

1. **A literal token in `secret_ref`** — `SecretRef::try_from` rejects any value without a `://` scheme (Task 3, Step 1).
2. **A secret reaching a log or panic message** — `SecretRef`'s `Debug` and `Display` emit `<redacted>`; the only accessor is `expose_reference()`, which is greppable in review (Task 3).
3. **An unknown config field silently ignored** — `#[serde(deny_unknown_fields)]` makes it an error; `extensions` is the sole escape hatch (Task 2, Step 5).
4. **A future config misreported** — version denial runs before field validation, so `psyche.config.v2` reports an unsupported version rather than a confusing unknown-field error (Task 2, Step 7).
5. **A committed credential** — gitleaks runs over full history in CI (Task 7).
6. **A tampered or substituted native binary** — the npm wrapper verifies SHA-256 against the manifest and refuses to exec on mismatch (Task 8, Step 3).
7. **A copyleft or advisory-bearing dependency** — cargo-deny fails the build (Task 7, Step 3).

## Gate mapping

- **G2 — Contract foundation:** partial. Unknown-version denial lands here (Tasks 2, 3). Schemas, migrations, fakes, and state-machine/property/crash tests remain for the follow-on plan. G2 is **not** claimed by this plan.
- **G10 — Operations:** contributes `psyche doctor` (Task 5). Retention, export/restore, incident response, and rotation are out of scope.
- **G12 — Distribution:** contributes reproducible packaging and checksum verification (Task 8). Signing, SBOM, provenance, and publication are explicitly **not** performed — Task 8 stops at `--dry-run`.

No capability flag is set by this plan. `psyche.graphs.v1` stays false until G2 passes in full.

## Verification commands

Run from the `OpenCoven/psyche` repository root. These are the same commands CI runs — there is no CI-only relaxation.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo deny check licenses advisories bans sources
gitleaks detect --no-banner --redact --log-opts="--all"
npm --prefix packages/psyche-npm test
npm pack ./packages/psyche-npm --dry-run
```

No `--all-features`: no crate in this workspace declares a feature, so the flag
is a claim of coverage the manifests cannot back. These are the exact commands
CI runs in Task 7 — if the two lists drift, the local gate stops predicting the
remote one, which is the only thing it is for.

`npm pack` takes the package as a **positional** argument. `--prefix` does not
work here: unlike `npm test`, `pack` resolves `package.json` from the working
directory and ignores the prefix, failing with `ENOENT ... /package.json` from
whatever directory you happened to be in. Verified.

**Expected terminal evidence:** `cargo fmt` prints nothing and exits 0. `clippy` prints `Finished` with no warnings. `cargo test` reports every suite `ok` with `0 failed`. `cargo deny` prints `advisories ok`, `bans ok`, `licenses ok`, `sources ok`. `gitleaks` prints `no leaks found`. `npm test` reports `pass 15  fail 0`. `npm pack --dry-run` lists `bin/psyche.js`, `scripts/verify-checksum.js`, `scripts/resolve-binary.js`, `package.json`, `README.md` — 5 files, ~2.5 kB packed — and no `test/`, `.node`, `.exe`, or `.wasm`.

---

## Task 0: Claim the unit and stop at the repository gate

**Files:**
- None (coordination only)

- [ ] **Step 1: Verify the child plan is approved before any code**

Run:

```bash
cd ~/Documents/GitHub/OpenCoven/coven
bd show coven-psy1 | grep -E "implementation_authorized|child_plan_approved"
```

Expected: `implementation_authorized: false` until this plan is reviewed. **If it still reads `false`, stop here** — PLAN.md §6.9 requires stopping at approval gates, and `coven-psy1`'s own notes say "Do not create production code until that child plan is reviewed and approved".

- [ ] **Step 2: Close the gate bead that tracks this plan**

Once the reviewer approves, run:

```bash
bd close coven-bin --reason "W2 bootstrap child plan approved at <PLAN_COMMIT_SHA>."
bd ready
```

Expected: `coven-psy1` appears in `bd ready` for the first time; `coven-bin` no longer does.

- [ ] **Step 3: Record the shared claim**

```bash
bd note coven-psy1 "Claimed for W2 bootstrap execution on branch feat/psyche-w2-bootstrap. Scope: psyche-core, psyche-config, psyche-runtime, psyche-cli, CI, npm dry-run only."
```

Expected: `✓ Note added to coven-psy1`.

- [ ] **Step 4: Create the repository (operator action)**

`OpenCoven/psyche` does not exist yet. Repository creation is an organisation-level action and is **not** performed by an agent:

```bash
gh repo create OpenCoven/psyche --private --description "Psyche: Coven-native familiar runtime" --clone
```

Expected: the operator runs this and confirms. If the repo already exists, skip. Every later task runs inside this repository, not inside `coven`.

---

## Task 1: Cargo workspace skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`

- [ ] **Step 1: Pin the toolchain**

Create `rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.85.0"
components = ["rustfmt", "clippy", "rust-src"]  # rust-src: rust-analyzer stdlib support
profile = "minimal"
```

- [ ] **Step 2: Create the workspace root**

Create `Cargo.toml`:

```toml
[workspace]
# Resolver 3 is MSRV-aware. clap, assert_cmd, and toml all declare rust-version
# 1.85 — exactly our pin — so under resolver 2 a routine `cargo update` would
# select a release requiring a newer compiler and break the build.
resolver = "3"
# Members are added by the task that creates each crate. Cargo loads every
# declared member's manifest on ANY command — `--no-deps` and `--manifest-path`
# both still walk up to the workspace root — so naming a crate before it exists
# makes the entire workspace uninvokable, including `cargo test`.
members = []

[workspace.package]
version = "0.0.0"
# Edition 2024 stabilised in Rust 1.85 — the version pinned above. Adopting it
# now costs nothing; deferring means migrating four crates of real code later,
# and the 2024 `if let` temporary-scope change alters when guards drop across
# awaits, which is a behavioural migration best done at 50 lines.
edition = "2024"
rust-version = "1.85"
publish = false   # distributed via npm, never crates.io
license = "MIT"
repository = "https://github.com/OpenCoven/psyche"

[workspace.dependencies]
# Path entries are added by the task that creates each crate, for the same
# reason `members` is: naming a path that does not exist is a latent break.
serde = { version = "1", features = ["derive"] }
toml = "1"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal", "sync", "time"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
serde_json = "1"
assert_cmd = "2"
predicates = "3"
tempfile = "3"

# Shared lint policy. Declared at bootstrap because retrofitting it later means
# editing every member manifest *and* clearing whatever backlog the new lints
# surface across real code; each crate is instead born compliant.
[workspace.lints.rust]
unsafe_code = "forbid"
missing_debug_implementations = "warn"
missing_docs = "warn"
unreachable_pub = "warn"
unused_qualifications = "warn"
rust_2018_idioms = { level = "warn", priority = -1 }

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
# The highest-value pair for a long-running daemon. clippy.toml permits them in
# tests. Deliberately NOT clippy::pedantic — noisy enough to train people to
# reach for #[allow], and module_name_repetitions fires on schema::SchemaError.
unwrap_used = "deny"
expect_used = "deny"

[profile.release]
codegen-units = 1
lto = true
opt-level = "s"
# "debuginfo", not true: `strip = true` also removes the symbol table, which
# turns field panics in an npm-distributed daemon into unresolved hex addresses.
strip = "debuginfo"
```

- [ ] **Step 3: Permit unwrap/expect in tests**

Create `clippy.toml`:

```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
```

Without this, `clippy::unwrap_used` fires on `unwrap_err()` in every test.

- [ ] **Step 4: Ignore build and local state**

Create `.gitignore`:

```gitignore
/target
**/node_modules
/*.log
.DS_Store
.env*
*.tgz
```

`.env*` matters most here: this repo will eventually hold npm publish tokens and
signing material. `*.log` is anchored to the root so it cannot silently swallow a
committed log-output test fixture, which is a realistic collision for a project
built around structured logging.

- [ ] **Step 5: Verify the empty workspace and toolchain parse**

Run:

```bash
cargo metadata --format-version 1 --no-deps > /dev/null && echo OK
```

Expected: `OK`.

`cargo build` and `cargo test` cannot succeed yet, by design: this is a virtual
manifest whose workspace has no members. Expect:

```
error: manifest path `.../psyche` contains no package: The manifest is virtual,
and the workspace has no members.
```

That build/test error is the correct state at the end of Task 1. **Do not create
crate stubs to silence it** — Task 2 owns those files, and stubbing here breaks
the task boundary. The first real build/test verification runs at the end of
Task 2.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml rust-toolchain.toml .gitignore
git commit -m "chore: create Cargo workspace skeleton and pin toolchain"
```

---

## Task 2: `psyche-core` schema versioning

**Files:**
- Create: `crates/psyche-core/Cargo.toml`
- Create: `crates/psyche-core/src/lib.rs`
- Create: `crates/psyche-core/src/schema.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/psyche-core/src/schema.rs`:

```rust
//! Versioned schema identifiers. Unknown versions are denied, never coerced.

/// The only configuration schema this build accepts.
pub const CONFIG_SCHEMA_VERSION: &str = "psyche.config.v1";

/// Reasons a declared schema version is not usable by this build.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    // {found:?} not {found}: a hand-edited `schema_version = " psyche.config.v1"`
    // would otherwise log as visually identical to the accepted value, and the
    // value is untrusted text going into a log line — newlines and ANSI escapes
    // are log injection. `expected` is not a field: it is always this const, and
    // a public field would let callers construct a state that cannot exist.
    /// The configuration declared a version this build does not accept. Denial
    /// is unconditional: there is no compatibility range and no coercion.
    #[error("unsupported schema_version {found:?}; this build accepts {CONFIG_SCHEMA_VERSION:?}")]
    UnsupportedVersion {
        /// The rejected value, byte-for-byte as it appeared in the configuration.
        found: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // The literal, not the const: this string is the on-disk contract with every
    // user's config file, so a typo in the const must fail a test rather than
    // silently redefine the format.
    #[test]
    fn the_accepted_version_string_is_stable() {
        assert_eq!(CONFIG_SCHEMA_VERSION, "psyche.config.v1");
        assert!(ensure_schema_version("psyche.config.v1").is_ok());
    }

    #[test]
    fn denies_a_future_version() {
        let err = ensure_schema_version("psyche.config.v2").unwrap_err();
        // matches!, not a full struct literal: pins what an operator can observe
        // without coupling the test to the variant's field list.
        assert!(matches!(err, SchemaError::UnsupportedVersion { ref found } if found == "psyche.config.v2"));
    }

    #[test]
    fn the_error_names_both_versions() {
        // The #[error] format string is the operator-facing contract; without
        // this it could be mangled to anything and every other test would pass.
        let rendered = ensure_schema_version("psyche.config.v2").unwrap_err().to_string();
        assert!(rendered.contains("psyche.config.v2"), "{rendered}");
        assert!(rendered.contains("psyche.config.v1"), "{rendered}");
    }

    // These pin the deliberate strictness. Without them, someone "helpfully"
    // adding .trim() or eq_ignore_ascii_case would break G2 denial silently.
    #[test]
    fn denies_near_misses() {
        for near in [
            "",
            " psyche.config.v1",
            "psyche.config.v1 ",
            "psyche.config.v1\n",
            "PSYCHE.CONFIG.V1",
            "psyche.config.v10",
            "psyche.config.v1.1",
        ] {
            assert!(
                ensure_schema_version(near).is_err(),
                "expected denial for {near:?}"
            );
        }
    }

    // psyche-runtime is tokio-based, so this error crosses task boundaries and
    // lands in Box<dyn Error + Send + Sync>. Fails at compile time if that breaks.
    const _: fn() = || {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<SchemaError>();
    };
}
```

- [ ] **Step 2: Create the manifest and lib root**

Create `crates/psyche-core/Cargo.toml`:

```toml
[package]
name = "psyche-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
thiserror = { workspace = true }

[lints]
workspace = true
```

The `[lints] workspace = true` stanza is required in **every** member manifest —
workspace lints are opt-in per package, so a crate that omits it silently escapes
the policy. Tasks 4, 5, and 6 repeat it.

Create `crates/psyche-core/src/lib.rs`:

```rust
//! Core versioned identifiers and secret-reference types for Psyche.

// One public path per item. Flat re-exports alongside public modules would give
// every type two spellings for downstream crates to drift between, and a glob
// re-export would silently promote anything later added to `secret.rs` into the
// public API. Callers write `psyche_core::schema::ensure_schema_version`.
pub mod schema;
pub mod secret;
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p psyche-core`
Expected: FAIL — `cannot find function ensure_schema_version in this scope`, plus `file not found for module secret`.

- [ ] **Step 4: Write the minimal implementation**

Append to `crates/psyche-core/src/schema.rs`, above the `#[cfg(test)]` block:

```rust
/// Returns `Ok` only for the exact supported version. No range matching, no
/// coercion — an unknown version is a denial, which is what G2 requires.
pub fn ensure_schema_version(declared: &str) -> Result<(), SchemaError> {
    if declared == CONFIG_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(SchemaError::UnsupportedVersion {
            found: declared.to_string(),
        })
    }
}
```

Create a placeholder `crates/psyche-core/src/secret.rs` so the crate compiles; Task 3 fills it:

```rust
//! Secret references. Filled in by Task 3.
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p psyche-core schema`
Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 6: Register the crate as a workspace member**

Only now that `crates/psyche-core/Cargo.toml` exists can it be declared. In the
root `Cargo.toml`, replace `members = []` and add the matching path dependency:

```toml
members = ["crates/psyche-core"]

[workspace.dependencies]
psyche-core = { path = "crates/psyche-core" }
# ...the registry dependencies declared in Task 1 remain below...
```

Then verify the workspace loads:

```bash
cargo metadata --format-version 1 --no-deps > /dev/null && echo OK
```

Expected: `OK`. Each later task appends its own crate the same way, so the list
always names exactly the crates that exist and cargo stays usable throughout.

- [ ] **Step 7: Commit**

```bash
git add crates/psyche-core
git commit -m "feat(core): deny unknown configuration schema versions"
```

---

## Task 3: `SecretRef` that cannot hold or print a secret

**Files:**
- Modify: `crates/psyche-core/src/secret.rs`

**Why this lands with no consumer yet.** No config field in this slice holds a
secret — accounts and principal bindings arrive with the surface workstreams.
`SecretRef` is built now because `coven-psy1` acceptance requires logs that
"redact secret-bearing fields", and the redaction primitive has to exist and be
proven *before* the first secret-bearing field is added, not retrofitted after.
It is deliberately the only type permitted to carry a secret reference, so
review can enforce the rule by grepping for `expose_reference`.

- [ ] **Step 1: Write the failing test**

Replace `crates/psyche-core/src/secret.rs` with:

```rust
//! A reference to a secret held by an external store. This type never contains
//! the secret itself, and never prints its reference through `Debug`/`Display`.

use std::fmt;

/// A pointer to a secret held by an external store — never the secret itself.
///
/// Deserialising goes through [`TryFrom<String>`], which accepts only an
/// allowlisted secret-store scheme — so neither a bare credential nor a URL
/// with one embedded in it can enter. Both `Debug` and `Display` redact, so a
/// reference cannot leave through a log line or a panic message either.
///
/// `Serialize` is deliberately not implemented. A type that redacts in logs but
/// prints plaintext in a config dump is a trap. If a dump ever needs it (e.g. a
/// `config show` command), it must be a hand-written impl rather than a derive,
/// so the choice is visible at the site that makes it.
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "String")]
pub struct SecretRef(String);

/// Reasons a value is not usable as a secret reference.
///
/// Every variant is deliberately payload-free. The rejection path is exactly
/// where a real secret is most likely to be present, so the rejected value is
/// dropped rather than echoed into an error message or a log line.
///
/// **That guarantee ends at this type.** A deserializer may wrap it in an error
/// that is not payload-free: `toml::de::Error` — the one the configuration
/// contract actually uses — echoes the offending source line through `Display`,
/// and its `Debug` carries the entire input file. Logging one with
/// `tracing::error!(?err)` after a failed config load would emit every secret in
/// that file, not just the rejected value. A config loader must render its own
/// message and must never log a `toml::de::Error` from a file that can contain
/// `secret_ref`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretRefError {
    /// The value did not begin with a supported secret-store scheme.
    ///
    /// This rejects ordinary URLs on purpose. A general "contains `://`" check
    /// would accept `https://host/bot<token>/send` or
    /// `https://user:pass@host/path`, both of which carry the secret *inside*
    /// the URI — the likelier paste, since it is the form API docs show.
    #[error(
        "secret_ref must name a supported secret store (case-sensitive), e.g. `op://VAULT/ITEM/field`"
    )]
    UnsupportedScheme,
    /// A supported scheme with nothing after it, such as a bare `op://`.
    ///
    /// Accepting it would defer the failure to resolution time, far from the
    /// configuration file that caused it.
    #[error("secret_ref has a scheme but no path, e.g. `op://` with no vault/item/field")]
    EmptyPath,
    /// The path is present but not a usable reference: too few segments, an empty
    /// segment, surrounding whitespace, or a control or format character.
    ///
    /// Whitespace is rejected rather than trimmed. Storing something other than
    /// what the operator wrote is worse than telling them, and control
    /// characters would otherwise reach a resolver's log line as injection —
    /// the same argument `schema` makes for `{found:?}`.
    #[error(
        "secret_ref path must be VAULT/ITEM/FIELD with no empty segments, surrounding whitespace, or control or format characters"
    )]
    MalformedPath,
    /// The reference exceeds the maximum accepted length.
    #[error("secret_ref is too long")]
    TooLong,
}

/// Schemes naming a supported external secret store.
///
/// `op://` is the only store the configuration contract defines today. Adding
/// one is a line here plus a test — deliberately an allowlist rather than a
/// general URI check, so a value can only ever *point at* a secret.
const SUPPORTED_SCHEMES: [&str; 1] = ["op://"];

/// Minimum `/`-separated segments after the scheme: vault, item, field.
///
/// Path grammar is per-store, so a second scheme means more than another entry
/// in [`SUPPORTED_SCHEMES`] — it needs per-scheme dispatch, e.g.
/// `[(&str, fn(&str) -> bool); N]`. Recorded here so that is a deliberate
/// decision rather than one made under pressure when the second store lands.
const REQUIRED_SEGMENTS: usize = 3;

/// Longest reference accepted. A vault path is short; anything near this is a
/// paste accident. Bounds the blast radius of any future redaction bug.
const MAX_REFERENCE_LEN: usize = 2048;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_reference_uri() {
        let r = SecretRef::try_from("op://VAULT/ITEM/token".to_string()).unwrap();
        assert_eq!(r.expose_reference(), "op://VAULT/ITEM/token");
    }

    #[test]
    fn rejects_a_literal_token() {
        // Built at runtime rather than written as a literal: a bot-token-shaped
        // string committed to the repo trips secret scanners and trains
        // reviewers to wave through that shape.
        let token_shaped = format!("{}:{}", "1234567890", "A".repeat(35));
        let err = SecretRef::try_from(token_shaped).unwrap_err();
        assert_eq!(err, SecretRefError::UnsupportedScheme);
    }

    // The cases a "contains ://" check would have accepted. These are the point
    // of the allowlist: a secret carried *inside* a URI is the likelier paste.
    #[test]
    fn rejects_a_url_with_the_secret_in_its_path() {
        let url = format!("https://api.example.com/bot{}/send", "A".repeat(35));
        assert_eq!(
            SecretRef::try_from(url).unwrap_err(),
            SecretRefError::UnsupportedScheme
        );
    }

    #[test]
    fn rejects_a_url_carrying_inline_credentials() {
        let url = format!("https://user:{}@example.com/path", "A".repeat(20));
        assert_eq!(
            SecretRef::try_from(url).unwrap_err(),
            SecretRefError::UnsupportedScheme
        );
    }

    #[test]
    fn rejects_an_unallowlisted_scheme() {
        for other in ["file:///etc/shadow", "http://example.com/x", "x://y"] {
            assert_eq!(
                SecretRef::try_from(other.to_string()).unwrap_err(),
                SecretRefError::UnsupportedScheme,
                "expected rejection for {other:?}"
            );
        }
    }

    #[test]
    fn rejects_a_scheme_with_no_path() {
        assert_eq!(
            SecretRef::try_from("op://".to_string()).unwrap_err(),
            SecretRefError::EmptyPath
        );
    }

    #[test]
    fn error_renderings_never_echo_any_of_the_rejected_value() {
        // Display AND Debug: Debug is what panics and `tracing` `?err` print, so
        // a payload field added to a variant would leak there first. Checks every
        // 8-byte window, not just the whole string, so a truncated echo fails too.
        let secretish = format!("{}:{}", "1234567890", "A".repeat(35));
        let long = format!("op://{}", "B".repeat(4096));
        // Deliberately NOT `op://VAULT/ITEM`: `MalformedPath`'s message states
        // the grammar as the literal text `VAULT/ITEM/FIELD`, so that input
        // collides with a static example rather than proving an echo. The
        // assertion below is unchanged in strength — only the sample differs.
        //
        // Every sample must be >= 8 bytes: windows(8) yields no iterations for
        // shorter input, so a short sample would pass vacuously. "op://" (the
        // EmptyPath case, 5 bytes) is deliberately not in this list.
        for input in [
            secretish,
            long,
            "op://alpha/beta".to_string(),
            "op:// x/y/z".to_string(),
        ] {
            let err = SecretRef::try_from(input.clone()).unwrap_err();
            for rendering in [err.to_string(), format!("{err:?}")] {
                for window in input.as_bytes().windows(8) {
                    let needle = String::from_utf8_lossy(window);
                    assert!(
                        !rendering.contains(needle.as_ref()),
                        "error echoed {needle:?} from input: {rendering}"
                    );
                }
            }
        }
    }

    // Mirrors schema.rs's denies_near_misses. Without it, someone "helpfully"
    // adding .trim() or a case-insensitive scheme match breaks these silently.
    #[test]
    fn rejects_near_misses() {
        for (input, expected) in [
            ("OP://VAULT/ITEM/field", SecretRefError::UnsupportedScheme),
            (" op://VAULT/ITEM/field", SecretRefError::UnsupportedScheme),
            ("op://VAULT/ITEM", SecretRefError::MalformedPath),
            ("op://VAULT", SecretRefError::MalformedPath),
            ("op:///", SecretRefError::MalformedPath),
            ("op://VAULT//field", SecretRefError::MalformedPath),
            ("op:// VAULT/ITEM/field", SecretRefError::MalformedPath),
            ("op://VAULT/ITEM/field ", SecretRefError::MalformedPath),
            ("op://VAULT/ITEM/field\n", SecretRefError::MalformedPath),
            (
                "op://VAULT\u{1b}[31m/ITEM/field",
                SecretRefError::MalformedPath,
            ),
            (
                "op://VAULT/ITEM/fi\u{202e}eld",
                SecretRefError::MalformedPath,
            ),
            (
                "op://VA\u{200b}ULT/ITEM/field",
                SecretRefError::MalformedPath,
            ),
        ] {
            assert_eq!(
                SecretRef::try_from(input.to_string()).unwrap_err(),
                expected,
                "wrong error for {input:?}"
            );
        }
    }

    #[test]
    fn accepts_internal_spaces_in_segment_names() {
        // 1Password vault and item names legitimately contain spaces.
        assert!(SecretRef::try_from("op://My Vault/My Item/token".to_string()).is_ok());
    }

    #[test]
    fn rejects_an_over_long_reference() {
        let long = format!("op://{}", "B".repeat(4096));
        assert_eq!(
            SecretRef::try_from(long).unwrap_err(),
            SecretRefError::TooLong
        );
    }

    // Distinct from the test above, which a mutation pass showed is weaker than
    // it reads: `op://BBBB…` is *also* malformed — one segment, not three — so
    // removing the MAX_REFERENCE_LEN check entirely still rejects it, just as
    // `MalformedPath`. That test fails on the variant mismatch, never on the
    // reference being accepted. This sample is well-formed in every respect
    // except length, so only the length check can reject it.
    #[test]
    fn rejects_an_over_long_but_otherwise_valid_reference() {
        let long = format!("op://VAULT/ITEM/{}", "B".repeat(MAX_REFERENCE_LEN));
        assert_eq!(
            SecretRef::try_from(long).unwrap_err(),
            SecretRefError::TooLong
        );
    }

    // The serde attribute is the load-bearing mechanism and was previously
    // untested: deleting `#[serde(try_from = "String")]` would make SecretRef
    // accept any string, and every other test here would still pass.
    #[test]
    fn deserialising_goes_through_validation() {
        // `unwrap_err` on `Result<Holder, _>` needs `Holder: Debug`. Deriving it
        // is safe precisely because `SecretRef`'s own `Debug` redacts, so the
        // nested rendering is `Holder { secret_ref: SecretRef(<redacted>) }`.
        #[derive(Debug, serde::Deserialize)]
        struct Holder {
            secret_ref: SecretRef,
        }

        let ok: Holder = serde_json::from_str(r#"{"secret_ref":"op://V/I/f"}"#).unwrap();
        assert_eq!(ok.secret_ref.expose_reference(), "op://V/I/f");

        let token_shaped = format!("{}:{}", "1234567890", "A".repeat(35));
        let json = format!(r#"{{"secret_ref":"{token_shaped}"}}"#);
        let err = serde_json::from_str::<Holder>(&json).unwrap_err();
        assert!(
            !err.to_string().contains(&token_shaped),
            "serde echoed input: {err}"
        );
    }

    // psyche-runtime is tokio-based; these cross task boundaries.
    const _: fn() = || {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<SecretRef>();
        assert_send_sync_static::<SecretRefError>();
    };

    #[test]
    fn debug_never_reveals_the_reference() {
        let r = SecretRef::try_from("op://VAULT/ITEM/token".to_string()).unwrap();
        let rendered = format!("{r:?}");
        assert_eq!(rendered, "SecretRef(<redacted>)");
        assert!(!rendered.contains("VAULT"));
    }

    #[test]
    fn display_never_reveals_the_reference() {
        let r = SecretRef::try_from("op://VAULT/ITEM/token".to_string()).unwrap();
        let rendered = format!("{r}");
        assert_eq!(rendered, "<redacted>");
        assert!(!rendered.contains("VAULT"));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p psyche-core secret`
Expected: FAIL — `the trait bound SecretRef: From<String> is not satisfied` and `no method named expose_reference`.

- [ ] **Step 3: Write the minimal implementation**

Insert into `crates/psyche-core/src/secret.rs`, above the `#[cfg(test)]` block:

```rust
/// Characters rejected inside a reference path.
///
/// `char::is_control` covers Unicode `Cc` only. The `Cf` format characters below
/// enable visual spoofing — a right-to-left override can make a path render as
/// something other than what it resolves to — so they are rejected explicitly
/// rather than pulling in a Unicode-category dependency for the general case.
fn is_forbidden_in_path(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200b}'..='\u{200f}'   // zero-width and directional marks
            | '\u{202a}'..='\u{202e}' // bidi embedding and overrides
            | '\u{2066}'..='\u{2069}' // bidi isolates
        )
}

impl TryFrom<String> for SecretRef {
    type Error = SecretRefError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        let Some(scheme) = SUPPORTED_SCHEMES.iter().find(|s| raw.starts_with(**s)) else {
            return Err(SecretRefError::UnsupportedScheme);
        };
        if raw.len() > MAX_REFERENCE_LEN {
            return Err(SecretRefError::TooLong);
        }
        // `starts_with` guarantees scheme.len() is a char boundary, so this
        // slice cannot panic — worth stating in a crate that denies `unwrap`.
        let path = &raw[scheme.len()..];
        if path.is_empty() {
            return Err(SecretRefError::EmptyPath);
        }
        // Surrounding whitespace only: vault and item names may legitimately
        // contain internal spaces.
        if path.trim() != path || path.chars().any(is_forbidden_in_path) {
            return Err(SecretRefError::MalformedPath);
        }
        let mut segments = 0usize;
        for segment in path.split('/') {
            if segment.is_empty() {
                return Err(SecretRefError::MalformedPath);
            }
            segments += 1;
        }
        if segments < REQUIRED_SEGMENTS {
            return Err(SecretRefError::MalformedPath);
        }
        Ok(SecretRef(raw))
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretRef(<redacted>)")
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl SecretRef {
    /// The only way to read the reference text. Deliberately verbose so that
    /// `rg expose_reference` finds every call site during review.
    pub fn expose_reference(&self) -> &str {
        &self.0
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p psyche-core`
Expected: `test result: ok. 7 passed; 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add crates/psyche-core/src/secret.rs
git commit -m "feat(core): add non-printing SecretRef that rejects literal values"
```

---

## Task 4: `psyche-config` strict versioned configuration

**Files:**
- Create: `crates/psyche-config/Cargo.toml`
- Create: `crates/psyche-config/src/lib.rs`
- Create: `docs/CONFIGURATION.md`

**Before you start — a requirement discovered during Task 3.**

`toml::de::Error` is not payload-free. Its `Display` echoes the offending source
line verbatim, and its `Debug` carries `input: Some(<the entire file>)`. A single
`tracing::error!(?err)` on a failed config load would therefore emit every secret
in the file, not just the value that failed — defeating `SecretRefError`'s
payload-free design one layer up.

So this loader must never let a `toml::de::Error` escape — not through `Debug`,
and not through `Display` either, which is the subtler half: an
`#[error("...: {0}")]` that interpolates the TOML error renders the offending
source line straight into the message.

`ConfigError` therefore does **not** hold one and has no `#[from]` for it.
Deserializer errors are reduced to message-only text at exactly one place,
`detail_from`, which drops the source line and complete input. That message is
not unconditionally value-free: serde diagnostics may embed a rejected scalar.
Secret-bearing fields must therefore use `SecretRef`; review can grep the one
reduction boundary instead of auditing every `?`.

- [ ] **Step 0: Register the crate**

Create `crates/psyche-config/` and add it to the workspace as the task that
creates it, per the rule established in Task 2 — cargo loads every declared
member on any command, so a member named before it exists breaks the workspace.

In the root `Cargo.toml`, extend `members` to
`["crates/psyche-core", "crates/psyche-config"]`, and add
`psyche-config = { path = "crates/psyche-config" }` to `[workspace.dependencies]`
beneath the `psyche-core` entry.

- [ ] **Step 1: Write the failing test**

Create `crates/psyche-config/src/lib.rs`:

```rust
//! Strict `psyche.config.v1` loading. Unknown fields are errors; unknown
//! versions are denied before field validation so the error names the real
//! cause.

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use psyche_core::schema::{SchemaError, ensure_schema_version};
use serde::Deserialize;

/// Largest configuration this build will read, in bytes.
///
/// A daemon that reloads on SIGHUP reads whatever path it was pointed at; an
/// oversized file — or an endless stream — would otherwise be an
/// out-of-memory switch. [`load_path`] enforces this against the bytes it
/// actually reads, not against a stated size.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Wire representation. Private on purpose: it is the only thing that derives
/// `Deserialize`, so the sole route to a `Config` is `load_str`/`load_path`,
/// which run `ensure_schema_version`. A public `Deserialize` on `Config` would
/// let a nested derive in a consumer crate produce an unvalidated `Config` with
/// no compile error and no failing test.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigRepr {
    // Never read: `VersionProbe` validates the value, and this parse cannot
    // disagree with it — both are `toml::from_str` over the same `&str` with the
    // same parser, a duplicate key is a hard parse error before either derive
    // runs, and `deny_unknown_fields` changes what is rejected, not which value
    // binds. The field exists only so the strict parse accepts the key.
    #[expect(
        dead_code,
        reason = "declared so deny_unknown_fields accepts the key; \
                  the value is validated once, via VersionProbe"
    )]
    schema_version: String,
    data_dir: PathBuf,
    coven: CovenConfig,
    #[serde(default)]
    extensions: toml::Table,
}

/// A validated `psyche.config.v1` document.
///
/// Obtainable only through [`load_str`] or [`load_path`], both of which deny an
/// unknown `schema_version` before any field is used.
///
/// No `Eq`: [`Extensions`] wraps a `toml::Table` whose values include
/// `Float(f64)`, so only `PartialEq` is available.
///
/// `Config` deliberately does not implement `Deserialize`. Restoring that derive
/// would let a consumer obtain one without ever running `ensure_schema_version`,
/// so this is pinned by a compile-fail test rather than left to review:
///
/// ```compile_fail
/// // Must NOT compile: the only route to a Config is load_str/load_path.
/// let _: psyche_config::Config = toml::from_str("data_dir = \"/tmp\"").unwrap();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Directory owning local Psyche state.
    pub data_dir: PathBuf,
    /// Coven daemon connection settings.
    pub coven: CovenConfig,
    /// Forward-compatible extension tables, keyed by versioned identifier.
    pub extensions: Extensions,
}

impl Config {
    /// The schema version this build accepts. Always
    /// `psyche_core::schema::CONFIG_SCHEMA_VERSION`; a validated `Config`
    /// cannot hold any other value.
    #[must_use]
    pub fn schema_version(&self) -> &'static str {
        psyche_core::schema::CONFIG_SCHEMA_VERSION
    }
}

/// Coven daemon connection settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CovenConfig {
    /// Path to the Coven daemon socket.
    pub socket: PathBuf,
    /// Named daemon contract required before any dependent action.
    pub required_api_version: String,
}

/// Extension tables from the configuration, keyed by versioned identifier.
///
/// Values are untyped, so `Debug` redacts them: a future extension may hold a
/// secret, and `tracing::debug!(?config)` after a *successful* load would
/// otherwise print it. Read values with [`Extensions::get`].
///
/// Wrapping the table also keeps `toml` out of this crate's public API, so
/// consumers are not pinned to its major version.
#[derive(Clone, PartialEq, Default)]
pub struct Extensions(toml::Table);

// Redacting Debug, hand-written for the reason in the type doc.
impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} key(s) redacted>", self.0.len())
    }
}

impl Extensions {
    /// Number of extension tables present.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether any extension table is present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether a given versioned key is present.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Deserialise one extension table into a caller-owned type.
    ///
    /// The error names the key but carries no position: the `Value`
    /// deserializer reports `span() == None`, so there is no line or column to
    /// report. Naming the key is safe for the same reason the unversioned-key
    /// check gives — a key is operator-authored structure, not a value.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if the table does not match `T`.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>, ConfigError> {
        match self.0.get(key) {
            None => Ok(None),
            // `value.clone()` is forced by the API: `toml` 1.1.4 implements
            // `Deserializer` for `Value`, not for `&Value`. Do NOT "optimise"
            // this into a string round-trip — re-serialising would write every
            // value back out, which is exactly what this crate keeps out of
            // memory it does not control.
            Some(value) => T::deserialize(value.clone())
                .map(Some)
                .map_err(|e: toml::de::Error| ConfigError::Parse {
                    detail: format!("extension {key:?}: {}", detail_from(&e)),
                    path: None,
                }),
        }
    }
}

/// Errors from loading configuration.
///
/// `Parse` deliberately does **not** hold a [`toml::de::Error`], and there is no
/// `#[from]` for one. That type's `Display` renders the offending source line
/// verbatim and its `Debug` carries `input: Some(<the entire file>)`, so holding
/// one would leave every secret in the file a single `?err` away from a log. The
/// deserializer error is reduced to a payload-free form at exactly one place —
/// [`detail_from`] — which is what review should grep for. [`reduce_toml_error`]
/// wraps it for the file-loading path only, so it is not the exhaustive one.
///
/// `Parse` has three construction sites, not two: the two deserializer paths
/// above, plus the unversioned-extension-key branch in `load_inner`. That third
/// one is built from a key this crate validated itself, never from a
/// `toml::de::Error`, so `detail_from` covering two of three is correct rather
/// than an omission.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file is not valid TOML, or violates the strict schema.
    #[error("configuration is not valid{}: {detail}", path.as_ref().map(|p| format!(" at {}", p.display())).unwrap_or_default())]
    #[non_exhaustive]
    Parse {
        /// Deserializer message, prefixed with line and column when known.
        ///
        /// File-free, but not unconditionally value-free: serde's `invalid type`
        /// diagnostic embeds the offending scalar, so a secret placed in a field
        /// of the wrong type would appear here. A field typed
        /// `psyche_core::secret::SecretRef` is unaffected, because its
        /// `try_from` routes failures through a payload-free error instead.
        detail: String,
        /// File the failure came from, when loaded from one.
        path: Option<PathBuf>,
    },
    /// The declared `schema_version` is not accepted by this build.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// The configuration file could not be read.
    #[error("cannot read configuration at {path}: {source}")]
    Read {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The configuration is larger than [`MAX_CONFIG_BYTES`].
    #[error("configuration at {path} exceeds the {MAX_CONFIG_BYTES} byte limit")]
    TooLarge {
        /// Path that was too large to read.
        path: PathBuf,
        /// Bytes read before the cap tripped — always `MAX_CONFIG_BYTES + 1`,
        /// which is a lower bound and not the true size. The read stops at the
        /// cap on purpose, and the source may be a stream with no size at all.
        bytes: u64,
    },
}

/// The single place a deserializer error is reduced to payload-free text.
///
/// `toml::de::Error::message()` is the bare diagnostic; its `Display` would
/// render the offending source line and its `Debug` carries the whole input.
/// Every crossing of that boundary goes through this function, so review greps
/// one name rather than auditing each `?`.
fn detail_from(err: &toml::de::Error) -> String {
    err.message().to_string()
}

/// Reduces a deserializer error from the file-loading path, adding position and
/// originating path to [`detail_from`]'s payload-free text.
///
/// Line and column are derived from `span()`, which is a byte offset and
/// therefore carries no file content.
fn reduce_toml_error(raw: &str, err: &toml::de::Error, path: Option<&Path>) -> ConfigError {
    // `raw.get(..start)` rather than `&raw[..start]`: `&str` slicing panics off
    // a char boundary, and nothing documents that `toml`'s spans land on one.
    // No position is a worse error, not a dead daemon, so fall back to none.
    let at = err
        .span()
        .and_then(|s| raw.get(..s.start))
        .map_or_else(String::new, |before| {
            let line = before.matches('\n').count() + 1;
            // Column is a byte offset within the line, not a character offset,
            // so it can skew on lines containing multibyte text. Content-free
            // either way.
            let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
            format!("line {line}, column {column}: ")
        });
    ConfigError::Parse {
        detail: format!("{at}{}", detail_from(err)),
        path: path.map(Path::to_path_buf),
    }
}

/// Whether a key is a versioned identifier: at least one non-empty dotted
/// segment, then a final `.v<digits>`. `psyche.experiment.v1` and `a.v0` both
/// qualify; `v1` and `not_versioned` do not.
fn is_versioned_key(key: &str) -> bool {
    let mut parts = key.rsplit('.');
    let Some(version) = parts.next() else {
        return false;
    };
    let Some(rest) = version.strip_prefix('v') else {
        return false;
    };
    !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_digit())
        && parts.next().is_some_and(|s| !s.is_empty())
}

/// Probe used only to read `schema_version`. It intentionally does *not* deny
/// unknown fields, so a future config can be version-checked before its unknown
/// fields are reported.
///
/// The document is parsed twice on purpose. Parsing once into a `toml::Value`
/// and deserialising both shapes from that would lose span information, which
/// is what [`reduce_toml_error`] reports line and column from.
#[derive(Deserialize)]
struct VersionProbe {
    schema_version: String,
}

fn load_inner(raw: &str, path: Option<&Path>) -> Result<Config, ConfigError> {
    let probe: VersionProbe = toml::from_str(raw).map_err(|e| reduce_toml_error(raw, &e, path))?;
    ensure_schema_version(&probe.schema_version)?;

    let repr: ConfigRepr = toml::from_str(raw).map_err(|e| reduce_toml_error(raw, &e, path))?;

    for key in repr.extensions.keys() {
        if !is_versioned_key(key) {
            // The key is operator-authored structure, not a value, so naming it
            // leaks nothing. The extension's *value* is deliberately absent.
            return Err(ConfigError::Parse {
                detail: format!(
                    "extension key {key:?} must be a versioned identifier such as `psyche.experiment.v1`"
                ),
                path: path.map(Path::to_path_buf),
            });
        }
    }

    Ok(Config {
        data_dir: repr.data_dir,
        coven: repr.coven,
        extensions: Extensions(repr.extensions),
    })
}

/// Parses a configuration document from memory.
///
/// The version is probed and validated *before* the strict parse, so a document
/// declaring a version this build does not accept is reported as an unsupported
/// version rather than as whatever unknown field that version happens to add.
///
/// # Errors
///
/// Returns [`ConfigError::Parse`] if the document is not valid TOML, violates
/// the strict schema, or carries an unversioned extension key, and
/// [`ConfigError::Schema`] if `schema_version` is not the version this build
/// accepts.
pub fn load_str(raw: &str) -> Result<Config, ConfigError> {
    load_inner(raw, None)
}

/// Reads and parses a configuration file from disk.
///
/// # Errors
///
/// Returns [`ConfigError::Read`] if the file cannot be opened or read,
/// [`ConfigError::TooLarge`] if more than [`MAX_CONFIG_BYTES`] can be read from
/// it, and otherwise whatever [`load_str`] returns for its contents.
pub fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let file = std::fs::File::open(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    // Bound the read, not the metadata: `metadata().len()` is 0 for FIFOs and
    // character devices, so a size check on it lets an unbounded stream through
    // — /dev/zero yields valid UTF-8 forever. Reading MAX+1 also closes the
    // TOCTOU window between stat and read.
    let mut raw = String::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if raw.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge {
            path: path.to_path_buf(),
            bytes: raw.len() as u64,
        });
    }
    load_inner(&raw, Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const HEAD: &str = "schema_version = \"psyche.config.v1\"\ndata_dir = \"/var/lib/psyche\"\n";
    const COVEN: &str =
        "\n[coven]\nsocket = \"/run/coven.sock\"\nrequired_api_version = \"coven.daemon.v1\"\n";

    fn valid() -> String {
        format!("{HEAD}{COVEN}")
    }

    fn temp_with(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn loads_a_valid_config() {
        let cfg = load_str(&valid()).unwrap();
        assert_eq!(cfg.schema_version(), "psyche.config.v1");
        assert_eq!(cfg.data_dir, PathBuf::from("/var/lib/psyche"));
        assert_eq!(cfg.coven.required_api_version, "coven.daemon.v1");
        assert!(cfg.extensions.is_empty());
    }

    // Injected between HEAD and COVEN, at document scope. The previous version
    // of this test appended after COVEN, so the key landed *inside* `[coven]`
    // and it was exercising `CovenConfig`'s `deny_unknown_fields`, never
    // `ConfigRepr`'s. Asserting on the expected-field list makes that
    // impossible to reintroduce silently.
    #[test]
    fn rejects_an_unknown_top_level_field() {
        let raw = format!("{HEAD}telegram_token = \"nope\"\n{COVEN}");
        let err = load_str(&raw).unwrap_err();
        assert!(
            matches!(err, ConfigError::Parse { .. }),
            "expected a parse error, got {err:?}"
        );
        let rendered = err.to_string();
        assert!(rendered.contains("telegram_token"), "{rendered}");
        assert!(
            rendered.contains("expected one of `schema_version`"),
            "{rendered}"
        );
    }

    #[test]
    fn rejects_an_unknown_field_in_coven() {
        let raw = format!("{}\nstray = true\n", valid());
        let err = load_str(&raw).unwrap_err();
        assert!(
            matches!(err, ConfigError::Parse { .. }),
            "expected a parse error, got {err:?}"
        );
        let rendered = err.to_string();
        assert!(rendered.contains("stray"), "{rendered}");
        assert!(rendered.contains("expected `socket`"), "{rendered}");
    }

    #[test]
    fn reports_unknown_version_as_a_version_error_not_a_field_error() {
        // A v2 config will carry fields this build has never seen. The version
        // denial must win, or the operator gets a misleading "unknown field".
        let raw = r#"
schema_version = "psyche.config.v2"
data_dir = "/var/lib/psyche"
brand_new_v2_field = true

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#;
        let err = load_str(raw).unwrap_err();
        assert!(
            matches!(err, ConfigError::Schema(_)),
            "expected a schema error, got {err:?}"
        );
        assert!(err.to_string().contains("psyche.config.v2"));
    }

    #[test]
    fn accepts_a_versioned_extensions_table() {
        let raw = format!(
            "{}\n[extensions.\"psyche.experiment.v1\"]\nenabled = true\n",
            valid()
        );
        let cfg = load_str(&raw).unwrap();
        assert!(cfg.extensions.contains_key("psyche.experiment.v1"));
        assert_eq!(cfg.extensions.len(), 1);
    }

    #[test]
    fn rejects_an_unversioned_extension_key() {
        let raw = format!("{}\n[extensions.not_versioned]\nenabled = true\n", valid());
        let err = load_str(&raw).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("not_versioned"), "{rendered}");
        assert!(rendered.contains("versioned identifier"), "{rendered}");
    }

    #[test]
    fn debug_does_not_print_extension_values() {
        let raw = format!(
            "{}\n[extensions.\"psyche.experiment.v1\"]\nlooks_like_a_secret = \"{}\"\n[extensions.\"psyche.other.v2\"]\nalso = 1\n",
            valid(),
            "A".repeat(30)
        );
        let cfg = load_str(&raw).unwrap();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("looks_like_a_secret"), "{rendered}");
        assert!(!rendered.contains(&"A".repeat(30)), "{rendered}");
        assert!(rendered.contains("2 key"), "{rendered}");
    }

    #[test]
    fn extensions_get_deserialises_into_a_caller_type() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct Experiment {
            enabled: bool,
            retries: u8,
        }

        let raw = format!(
            "{}\n[extensions.\"psyche.experiment.v1\"]\nenabled = true\nretries = 3\n",
            valid()
        );
        let cfg = load_str(&raw).unwrap();

        let got: Option<Experiment> = cfg.extensions.get("psyche.experiment.v1").unwrap();
        assert_eq!(
            got,
            Some(Experiment {
                enabled: true,
                retries: 3
            })
        );

        let missing: Option<Experiment> = cfg.extensions.get("psyche.absent.v1").unwrap();
        assert_eq!(missing, None);

        // A shape mismatch is a Parse error, not a panic, and it names the key
        // so the operator knows which extension table to look at.
        let bad = cfg
            .extensions
            .get::<u8>("psyche.experiment.v1")
            .unwrap_err();
        assert!(matches!(bad, ConfigError::Parse { .. }), "{bad:?}");
        let rendered = bad.to_string();
        assert!(rendered.contains("psyche.experiment.v1"), "{rendered}");
    }

    #[test]
    fn missing_file_reports_the_path() {
        let err = load_path(Path::new("/nonexistent/psyche.toml")).unwrap_err();
        assert!(err.to_string().contains("/nonexistent/psyche.toml"));
    }

    #[test]
    fn load_path_reads_a_real_file() {
        let f = temp_with(&valid());
        let cfg = load_path(f.path()).unwrap();
        assert_eq!(cfg.data_dir, PathBuf::from("/var/lib/psyche"));
        assert_eq!(cfg.coven.socket, PathBuf::from("/run/coven.sock"));
        assert_eq!(cfg.schema_version(), "psyche.config.v1");
    }

    #[test]
    fn refuses_a_file_over_the_size_cap() {
        let big = format!("{}\n# {}", valid(), "x".repeat(MAX_CONFIG_BYTES as usize));
        let file = temp_with(&big);
        assert!(matches!(
            load_path(file.path()).unwrap_err(),
            ConfigError::TooLarge { .. }
        ));
    }

    #[test]
    fn accepts_a_file_just_under_the_size_cap() {
        // Guards the boundary from the other side: an off-by-one that rejects
        // everything would otherwise pass the test above.
        let pad = MAX_CONFIG_BYTES as usize - valid().len() - 16;
        let ok = format!("{}\n# {}", valid(), "x".repeat(pad));
        assert!(load_path(temp_with(&ok).path()).is_ok());
    }

    // The two tests above use regular files, whose metadata is accurate, so
    // they cannot tell a bounded read from a metadata check — a mutation pass
    // confirmed both stay green if `take(MAX + 1)` is swapped back for
    // `metadata().len()`. This is the property that change actually fixed.
    #[test]
    #[cfg(unix)]
    fn refuses_a_stream_whose_metadata_understates_its_size() {
        // A FIFO reports metadata().len() == 0 no matter how much it yields.
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("cfg.fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );

        let w = fifo.clone();
        let writer = std::thread::spawn(move || {
            // Opening for write blocks until load_path opens the read end; once
            // load_path drops the file, write_all returns EPIPE and this
            // returns, so the join below cannot hang the suite.
            let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&w) else {
                return;
            };
            let chunk = format!("# {}\n", "x".repeat(4094));
            // 2 MiB: twice the cap. Bails out once the reader stops.
            for _ in 0..512 {
                if f.write_all(chunk.as_bytes()).is_err() {
                    return;
                }
            }
        });

        let err = load_path(&fifo).unwrap_err();
        let _ = writer.join();

        assert_eq!(
            std::fs::metadata(&fifo).unwrap().len(),
            0,
            "precondition: FIFO understates size"
        );
        // `bytes == MAX_CONFIG_BYTES + 1` is what makes this a bounded-read
        // test rather than another size test: it pins that the cap tripped on
        // bytes actually read, not on a size the stream claimed.
        assert!(
            matches!(err, ConfigError::TooLarge { bytes, .. } if bytes == MAX_CONFIG_BYTES + 1),
            "cap must trip on bytes read, not on stated size; got {err:?}"
        );
    }

    #[test]
    fn parse_errors_report_line_and_column() {
        // Duplicate top-level key: the bare message is "duplicate key", which
        // without a position is unactionable in a long config.
        let raw = format!("{HEAD}data_dir = \"/other\"\n{COVEN}");
        let err = load_str(&raw).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("line "), "{rendered}");
        assert!(rendered.contains("column "), "{rendered}");
    }

    // `detail_from` is the invariant this crate's docs tell review to grep for,
    // and it had no test: swapping `err.message()` for `err.to_string()` left
    // the whole suite green while the error rendered the offending source line
    // verbatim — secret and all. Every existing parse test asserts what the
    // message *contains*, never what it must omit.
    #[test]
    fn parse_errors_never_echo_the_offending_source_line() {
        // detail_from must reduce toml::de::Error to message(), never Display:
        // Display renders the source line, which is where the secret is.
        let secretish = format!("{}:{}", "1234567890", "A".repeat(35));

        // (a) unknown top-level field whose *value* is the secret
        let raw = format!("{HEAD}telegram_token = \"{secretish}\"\n{COVEN}");
        // (b) same, from a file, so reduce_toml_error's path is covered too
        let f = temp_with(&raw);

        for rendered in [
            load_str(&raw).unwrap_err().to_string(),
            load_path(f.path()).unwrap_err().to_string(),
            // (c) extension get(), the other detail_from call site
            {
                let raw = format!(
                    "{}\n[extensions.\"psyche.experiment.v1\"]\ntoken = \"{secretish}\"\n",
                    valid()
                );
                let cfg = load_str(&raw).unwrap();
                cfg.extensions
                    .get::<u8>("psyche.experiment.v1")
                    .unwrap_err()
                    .to_string()
            },
        ] {
            // windows(8), matching secret.rs: catches a truncated echo too.
            for window in secretish.as_bytes().windows(8) {
                let needle = String::from_utf8_lossy(window);
                assert!(
                    !rendered.contains(needle.as_ref()),
                    "error echoed {needle:?} from the source line: {rendered}"
                );
            }
        }
    }

    #[test]
    fn parse_errors_from_a_file_name_the_path() {
        let f = temp_with("this is not toml = = =\n");
        let err = load_path(f.path()).unwrap_err();
        let rendered = err.to_string();
        let path = f.path().display().to_string();
        assert!(rendered.contains(&path), "{rendered} / {path}");
    }

    #[test]
    fn accepts_a_byte_order_mark() {
        let raw = format!("\u{feff}{}", valid());
        assert!(load_str(&raw).is_ok(), "{:?}", load_str(&raw).unwrap_err());
    }

    #[test]
    fn accepts_crlf_line_endings() {
        let raw = valid().replace('\n', "\r\n");
        assert!(load_str(&raw).is_ok(), "{:?}", load_str(&raw).unwrap_err());
    }

    #[test]
    fn an_empty_file_names_the_missing_version_field() {
        let err = load_str("").unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("missing field `schema_version`"),
            "{rendered}"
        );
    }

    #[test]
    fn nested_extension_tables_count_only_top_level_keys() {
        let raw = format!(
            "{}\n[extensions.\"psyche.experiment.v1\".deeply.nested]\nvalue = 1\n",
            valid()
        );
        let cfg = load_str(&raw).unwrap();
        assert_eq!(cfg.extensions.len(), 1);
        assert!(cfg.extensions.contains_key("psyche.experiment.v1"));
    }

    #[test]
    fn versioned_key_rule_is_what_it_claims() {
        for good in [
            "psyche.experiment.v1",
            "psyche.experiment.v10",
            "a.v0",
            "one.two.three.v2",
        ] {
            assert!(is_versioned_key(good), "expected accept for {good:?}");
        }
        for bad in [
            "",
            "not_versioned",
            "v1",
            "psyche.experiment.v",
            "psyche.experiment.V1",
            "psyche.experiment.v1x",
            ".v1",
            "psyche.experiment.1",
        ] {
            assert!(!is_versioned_key(bad), "expected reject for {bad:?}");
        }
    }

    // psyche-runtime is tokio-based, so these cross task boundaries and land in
    // Box<dyn Error + Send + Sync>. Fails at compile time if that breaks.
    const _: fn() = || {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<ConfigError>();
        assert_send_sync_static::<Config>();
    };
}
```

- [ ] **Step 2: Create the manifest**

Create `crates/psyche-config/Cargo.toml`:

```toml
[package]
name = "psyche-config"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
psyche-core = { workspace = true }
serde = { workspace = true }
toml = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p psyche-config`
Expected: FAIL — `cannot find function load_str in this scope` and `cannot find function load_path in this scope`.

- [ ] **Step 4: Write the minimal implementation**

Insert into `crates/psyche-config/src/lib.rs`, above the `#[cfg(test)]` block:

```rust
impl Config {
    /// The schema version this build accepts. Always
    /// `psyche_core::schema::CONFIG_SCHEMA_VERSION`; a validated `Config`
    /// cannot hold any other value.
    #[must_use]
    pub fn schema_version(&self) -> &'static str {
        psyche_core::schema::CONFIG_SCHEMA_VERSION
    }
}

/// Coven daemon connection settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CovenConfig {
    /// Path to the Coven daemon socket.
    pub socket: PathBuf,
    /// Named daemon contract required before any dependent action.
    pub required_api_version: String,
}

/// Extension tables from the configuration, keyed by versioned identifier.
///
/// Values are untyped, so `Debug` redacts them: a future extension may hold a
/// secret, and `tracing::debug!(?config)` after a *successful* load would
/// otherwise print it. Read values with [`Extensions::get`].
///
/// Wrapping the table also keeps `toml` out of this crate's public API, so
/// consumers are not pinned to its major version.
#[derive(Clone, PartialEq, Default)]
pub struct Extensions(toml::Table);

// Redacting Debug, hand-written for the reason in the type doc.
impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} key(s) redacted>", self.0.len())
    }
}

impl Extensions {
    /// Number of extension tables present.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether any extension table is present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether a given versioned key is present.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Deserialise one extension table into a caller-owned type.
    ///
    /// The error names the key but carries no position: the `Value`
    /// deserializer reports `span() == None`, so there is no line or column to
    /// report. Naming the key is safe for the same reason the unversioned-key
    /// check gives — a key is operator-authored structure, not a value.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if the table does not match `T`.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>, ConfigError> {
        match self.0.get(key) {
            None => Ok(None),
            // `value.clone()` is forced by the API: `toml` 1.1.4 implements
            // `Deserializer` for `Value`, not for `&Value`. Do NOT "optimise"
            // this into a string round-trip — re-serialising would write every
            // value back out, which is exactly what this crate keeps out of
            // memory it does not control.
            Some(value) => T::deserialize(value.clone())
                .map(Some)
                .map_err(|e: toml::de::Error| ConfigError::Parse {
                    detail: format!("extension {key:?}: {}", detail_from(&e)),
                    path: None,
                }),
        }
    }
}

/// Errors from loading configuration.
///
/// `Parse` deliberately does **not** hold a [`toml::de::Error`], and there is no
/// `#[from]` for one. That type's `Display` renders the offending source line
/// verbatim and its `Debug` carries `input: Some(<the entire file>)`, so holding
/// one would leave every secret in the file a single `?err` away from a log. The
/// deserializer error is reduced to a payload-free form at exactly one place —
/// [`detail_from`] — which is what review should grep for. [`reduce_toml_error`]
/// wraps it for the file-loading path only, so it is not the exhaustive one.
///
/// `Parse` has three construction sites, not two: the two deserializer paths
/// above, plus the unversioned-extension-key branch in `load_inner`. That third
/// one is built from a key this crate validated itself, never from a
/// `toml::de::Error`, so `detail_from` covering two of three is correct rather
/// than an omission.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file is not valid TOML, or violates the strict schema.
    #[error("configuration is not valid{}: {detail}", path.as_ref().map(|p| format!(" at {}", p.display())).unwrap_or_default())]
    #[non_exhaustive]
    Parse {
        /// Deserializer message, prefixed with line and column when known.
        ///
        /// File-free, but not unconditionally value-free: serde's `invalid type`
        /// diagnostic embeds the offending scalar, so a secret placed in a field
        /// of the wrong type would appear here. A field typed
        /// `psyche_core::secret::SecretRef` is unaffected, because its
        /// `try_from` routes failures through a payload-free error instead.
        detail: String,
        /// File the failure came from, when loaded from one.
        path: Option<PathBuf>,
    },
    /// The declared `schema_version` is not accepted by this build.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// The configuration file could not be read.
    #[error("cannot read configuration at {path}: {source}")]
    Read {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The configuration is larger than [`MAX_CONFIG_BYTES`].
    #[error("configuration at {path} exceeds the {MAX_CONFIG_BYTES} byte limit")]
    TooLarge {
        /// Path that was too large to read.
        path: PathBuf,
        /// Bytes read before the cap tripped — always `MAX_CONFIG_BYTES + 1`,
        /// which is a lower bound and not the true size. The read stops at the
        /// cap on purpose, and the source may be a stream with no size at all.
        bytes: u64,
    },
}

/// The single place a deserializer error is reduced to payload-free text.
///
/// `toml::de::Error::message()` is the bare diagnostic; its `Display` would
/// render the offending source line and its `Debug` carries the whole input.
/// Every crossing of that boundary goes through this function, so review greps
/// one name rather than auditing each `?`.
fn detail_from(err: &toml::de::Error) -> String {
    err.message().to_string()
}

/// Reduces a deserializer error from the file-loading path, adding position and
/// originating path to [`detail_from`]'s payload-free text.
///
/// Line and column are derived from `span()`, which is a byte offset and
/// therefore carries no file content.
fn reduce_toml_error(raw: &str, err: &toml::de::Error, path: Option<&Path>) -> ConfigError {
    // `raw.get(..start)` rather than `&raw[..start]`: `&str` slicing panics off
    // a char boundary, and nothing documents that `toml`'s spans land on one.
    // No position is a worse error, not a dead daemon, so fall back to none.
    let at = err
        .span()
        .and_then(|s| raw.get(..s.start))
        .map_or_else(String::new, |before| {
            let line = before.matches('\n').count() + 1;
            // Column is a byte offset within the line, not a character offset,
            // so it can skew on lines containing multibyte text. Content-free
            // either way.
            let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
            format!("line {line}, column {column}: ")
        });
    ConfigError::Parse {
        detail: format!("{at}{}", detail_from(err)),
        path: path.map(Path::to_path_buf),
    }
}

/// Whether a key is a versioned identifier: at least one non-empty dotted
/// segment, then a final `.v<digits>`. `psyche.experiment.v1` and `a.v0` both
/// qualify; `v1` and `not_versioned` do not.
fn is_versioned_key(key: &str) -> bool {
    let mut parts = key.rsplit('.');
    let Some(version) = parts.next() else {
        return false;
    };
    let Some(rest) = version.strip_prefix('v') else {
        return false;
    };
    !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_digit())
        && parts.next().is_some_and(|s| !s.is_empty())
}

/// Probe used only to read `schema_version`. It intentionally does *not* deny
/// unknown fields, so a future config can be version-checked before its unknown
/// fields are reported.
///
/// The document is parsed twice on purpose. Parsing once into a `toml::Value`
/// and deserialising both shapes from that would lose span information, which
/// is what [`reduce_toml_error`] reports line and column from.
#[derive(Deserialize)]
struct VersionProbe {
    schema_version: String,
}

fn load_inner(raw: &str, path: Option<&Path>) -> Result<Config, ConfigError> {
    let probe: VersionProbe = toml::from_str(raw).map_err(|e| reduce_toml_error(raw, &e, path))?;
    ensure_schema_version(&probe.schema_version)?;

    let repr: ConfigRepr = toml::from_str(raw).map_err(|e| reduce_toml_error(raw, &e, path))?;

    for key in repr.extensions.keys() {
        if !is_versioned_key(key) {
            // The key is operator-authored structure, not a value, so naming it
            // leaks nothing. The extension's *value* is deliberately absent.
            return Err(ConfigError::Parse {
                detail: format!(
                    "extension key {key:?} must be a versioned identifier such as `psyche.experiment.v1`"
                ),
                path: path.map(Path::to_path_buf),
            });
        }
    }

    Ok(Config {
        data_dir: repr.data_dir,
        coven: repr.coven,
        extensions: Extensions(repr.extensions),
    })
}

/// Parses a configuration document from memory.
///
/// The version is probed and validated *before* the strict parse, so a document
/// declaring a version this build does not accept is reported as an unsupported
/// version rather than as whatever unknown field that version happens to add.
///
/// # Errors
///
/// Returns [`ConfigError::Parse`] if the document is not valid TOML, violates
/// the strict schema, or carries an unversioned extension key, and
/// [`ConfigError::Schema`] if `schema_version` is not the version this build
/// accepts.
pub fn load_str(raw: &str) -> Result<Config, ConfigError> {
    load_inner(raw, None)
}

/// Reads and parses a configuration file from disk.
///
/// # Errors
///
/// Returns [`ConfigError::Read`] if the file cannot be opened or read,
/// [`ConfigError::TooLarge`] if more than [`MAX_CONFIG_BYTES`] can be read from
/// it, and otherwise whatever [`load_str`] returns for its contents.
pub fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let file = std::fs::File::open(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    // Bound the read, not the metadata: `metadata().len()` is 0 for FIFOs and
    // character devices, so a size check on it lets an unbounded stream through
    // — /dev/zero yields valid UTF-8 forever. Reading MAX+1 also closes the
    // TOCTOU window between stat and read.
    let mut raw = String::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if raw.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge {
            path: path.to_path_buf(),
            bytes: raw.len() as u64,
        });
    }
    load_inner(&raw, Some(path))
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p psyche-config`
Expected: `test result: ok. 6 passed; 0 failed`.

- [ ] **Step 6: Document the shipped contract**

Create `docs/CONFIGURATION.md`:

```markdown
# Psyche configuration contract

The root config declares `schema_version = "psyche.config.v1"`. This build
accepts that exact value and denies every other, including future versions —
an unknown version is reported as an unsupported version, not as an unknown
field.

Unknown fields are errors. The only exception is the `extensions` table, whose
keys must themselves be versioned identifiers.

First-party secret-bearing fields accept references only (for example
`op://VAULT/ITEM/token`) and reject literal values at parse time. Extension
tables are untyped; extension owners must apply the same `SecretRef` contract to
their secret-bearing fields. Config debug output redacts all extension values.

## Minimal example

    schema_version = "psyche.config.v1"
    data_dir = "/var/lib/psyche"

    [coven]
    socket = "/run/coven.sock"
    required_api_version = "coven.daemon.v1"

## Fields

| Field | Type | Required | Meaning |
|---|---|---|---|
| `schema_version` | string | yes | Must be `psyche.config.v1`. |
| `data_dir` | path | yes | Directory owning local Psyche state. |
| `coven.socket` | path | yes | Coven daemon socket path. |
| `coven.required_api_version` | string | yes | Named daemon contract required before dependent actions. |
| `extensions` | table | no | Versioned escape hatch for forward-compatible additions. |

Account, principal-binding, and streaming tables are **not** part of this
release; they arrive with the surface workstreams.
```

- [ ] **Step 7: Commit**

```bash
git add crates/psyche-config docs/CONFIGURATION.md
git commit -m "feat(config): strict psyche.config.v1 loading with version-first denial"
```

---

## Task 5: `psyche-runtime` lifecycle and graceful shutdown

**Files:**
- Create: `crates/psyche-runtime/Cargo.toml`
- Create: `crates/psyche-runtime/src/lib.rs`

**API decisions this crate must get right before `psyche-cli` compiles against it.**

Each is free now and a break later, so they are settled here rather than
discovered:

- **`start` returns `Result<Self, RuntimeError>`**, not `Self`. The same
  argument that made it `async` applies with more force to fallibility: the G2
  work this seam is reserved for — opening `data_dir`, binding the Coven socket,
  acquiring a lease — is exactly the work that fails. Widening the return type
  after the CLI exists is the break the async decision was made to avoid.
- **`RuntimeError` is `#[non_exhaustive]`**, so adding a variant later is not a
  compile break in the CLI. `LifecycleState` deliberately is **not** — a new
  state should break the status renderer, so the omission is caught rather than
  silently falling through a `_` arm.
- **`LifecycleState` implements `Display`** with the wire spellings `running`,
  `draining`, `stopped`. Without one the CLI reaches for `{:?}`, and `Debug`
  output becomes a de facto compatibility surface nobody can rename.
- **Structured log fields use one spelling.** Emit `state = %s` everywhere via
  that `Display`; the drafted code mixed a lowercase string at start with a
  `Debug`-rendered capitalised value on transitions, which is a log-schema bug
  for anyone filtering on the key.
- **`Runtime::config()` returns `&Config`.** Task 6 needs `data_dir` for
  `doctor`, and cloning the config into the CLI would create two copies that can
  diverge. This also retires the `#[expect(dead_code)]`, which would otherwise
  start warning the moment the field is read.

**The shutdown loser must not return while the drain is still running.**

`psyched` awaits a signal and calls `shutdown`. Operators send a second SIGTERM
when the first appears not to work. If the loser returns immediately, the
natural CLI shape — `rt.shutdown().await?` then return from `main` — exits the
process mid-drain, and the graceful-shutdown guarantee is defeated by the thing
operators routinely do. This is a semantic contract, so it is expensive to
change even though no signature moves.

Keep the mutex as the election — it is proven — and add a `tokio::sync::watch`
channel beside it, fed from inside the transition while the guard is held. The
loser waits for `Stopped` and then returns. A `watch` cannot replace the mutex:
`send` has no compare-and-swap, so the single-acquisition election would be
lost.

That channel also supplies `Runtime::subscribe() -> watch::Receiver<LifecycleState>`,
which is the only way anything outside the winner can observe the transition to
`Stopped` without a poll loop, and makes the mid-flight ordering testable.

**The `!Send` hazard needs a real check, not a comment.** The drafted comment
claims holding a `std` guard across the drain await would break the
`assert_send_sync_static::<Runtime>()` assertion. It would not: that constrains
the *type*, and a guard held across an await affects the *future*. The failure
would surface one crate away at the CLI's `tokio::spawn`, pointing at the spawn
rather than the guard. Assert the futures directly:

```rust
const _: fn() = || {
    fn assert_send<T: Send>(_: &T) {}
    fn futures_are_send(rt: &Runtime, cfg: Config) {
        assert_send(&Runtime::start(cfg));
        assert_send(&rt.shutdown());
    }
};
```

- [ ] **Step 0: Register the crate**

Per the rule from Task 2 — cargo loads every declared member on any command, so
a member named before it exists breaks the workspace. Extend the root `members`
to include `crates/psyche-runtime`, and add
`psyche-runtime = { path = "crates/psyche-runtime" }` to
`[workspace.dependencies]` beneath `psyche-config`.

- [ ] **Step 1: Write the failing test**

Create `crates/psyche-runtime/src/lib.rs`:

```rust
//! Composition root. Owns the daemon lifecycle and the only shutdown path.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use psyche_config::Config;
use tokio::sync::watch;

/// Graceful shutdown stops intake, then drains, then exits. `Draining` is
/// observable so `psyche status` can distinguish it from `Running`.
///
/// Deliberately **not** `#[non_exhaustive]`, unlike [`RuntimeError`]. A new
/// state is a new thing an operator can be told, and it *should* break every
/// renderer until each one decides how to say it. `#[non_exhaustive]` would push
/// consumers into a `_` arm that silently misreports the new state as whatever
/// the fallback says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    /// Accepting work.
    Running,
    /// Intake stopped; in-flight work finishing.
    Draining,
    /// Fully stopped. Terminal.
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        psyche_config::load_str(
            r#"
schema_version = "psyche.config.v1"
data_dir = "/tmp/psyche-test"

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn starts_running() {
        let rt = Runtime::start(test_config()).await.unwrap();
        assert_eq!(rt.state(), LifecycleState::Running);
    }

    #[tokio::test]
    async fn the_configuration_is_readable_after_start() {
        let rt = Runtime::start(test_config()).await.unwrap();
        assert_eq!(
            rt.config().data_dir,
            std::path::Path::new("/tmp/psyche-test")
        );
    }

    // The wire spellings are what `psyche status --json` emits and what log
    // filters match on. Pinned against the literals, not against each other, so
    // renaming a variant cannot silently rename a field an operator scripts.
    #[test]
    fn states_render_as_their_wire_spellings() {
        assert_eq!(LifecycleState::Running.to_string(), "running");
        assert_eq!(LifecycleState::Draining.to_string(), "draining");
        assert_eq!(LifecycleState::Stopped.to_string(), "stopped");
    }

    #[tokio::test]
    async fn shutdown_drains_then_stops_in_order() {
        let rt = Runtime::start(test_config()).await.unwrap();
        rt.shutdown().await.unwrap();
        assert_eq!(rt.state(), LifecycleState::Stopped);
        assert_eq!(
            rt.transitions(),
            vec![
                LifecycleState::Running,
                LifecycleState::Draining,
                LifecycleState::Stopped
            ]
        );
    }

    // Previously asserted that a second shutdown is an error. It is not any
    // more: it waits for the first to finish and reports the truth, which for a
    // runtime that has already stopped is available immediately.
    //
    // Both calls are bounded by a timeout because the failure they guard
    // against is a *hang*, not a wrong answer: a `transition_to` that publishes
    // with `send` rather than `send_replace` stores nothing when there are no
    // receivers, so the second call waits on a `Stopped` that was announced to
    // nobody and never recorded. Verified by mutation — without the timeout
    // that swap wedges the suite indefinitely instead of failing it, which in
    // CI is a stuck job rather than a red one.
    #[tokio::test]
    async fn a_later_shutdown_succeeds_without_redriving_the_drain() {
        use std::time::Duration;

        let rt = Runtime::start(test_config()).await.unwrap();
        for attempt in 0..2 {
            tokio::time::timeout(Duration::from_secs(5), rt.shutdown())
                .await
                .unwrap_or_else(|_| panic!("shutdown {attempt} never returned"))
                .unwrap();
        }
        assert_eq!(rt.state(), LifecycleState::Stopped);
        assert_eq!(rt.transitions().len(), 3, "{:?}", rt.transitions());
    }

    // The role, not just the outcome: with both callers returning `Ok`, this is
    // the only place the election is visible.
    #[tokio::test]
    async fn only_the_first_caller_drives_the_drain() {
        let rt = Runtime::start(test_config()).await.unwrap();
        assert_eq!(rt.shutdown_inner().await.unwrap(), ShutdownRole::Driver);
        assert_eq!(rt.shutdown_inner().await.unwrap(), ShutdownRole::Observer);
    }

    // The property A4 exists for: a caller that lost the election must not
    // return while the drain is still running. Driving the transitions directly
    // rather than racing two `shutdown` calls is deliberate — the drain seam
    // contains no await, so a real winner passes from `Draining` to `Stopped`
    // within a single poll and there is no window a second task could be
    // scheduled in. A test that tried to hit that window would be measuring the
    // scheduler, and would pass against a loser that returns immediately
    // whenever the winner happened to finish first.
    #[tokio::test]
    async fn a_losing_caller_does_not_return_before_stopped_is_published() {
        use std::time::Duration;

        let rt = Runtime::start(test_config()).await.unwrap();
        // Enter `Draining` with no winner running, so nothing will publish
        // `Stopped` unless this test does.
        assert!(rt.transition_to(LifecycleState::Draining));

        let waited = tokio::time::timeout(Duration::from_millis(250), rt.shutdown()).await;
        assert!(
            waited.is_err(),
            "a losing caller returned while the runtime was still {}",
            rt.state()
        );

        assert!(rt.transition_to(LifecycleState::Stopped));
        // Now that `Stopped` is published the same call returns, and promptly:
        // a caller arriving after the drain has finished must not block either.
        tokio::time::timeout(Duration::from_millis(250), rt.shutdown())
            .await
            .unwrap()
            .unwrap();
    }

    // The transition log is what `psyche status` and the ordering assertion
    // above both read. A plain `push` would grow it on every rejected shutdown,
    // which in a daemon that is signalled repeatedly is an unbounded allocation.
    #[tokio::test]
    async fn the_transition_log_is_bounded_by_the_state_count() {
        let rt = Runtime::start(test_config()).await.unwrap();
        for _ in 0..1_000 {
            let _ = rt.shutdown().await;
        }
        assert_eq!(rt.transitions().len(), 3, "{:?}", rt.transitions());
    }

    // A subscriber that keeps up sees every state, in order, with none skipped.
    // The transitions are driven by the test rather than by `shutdown` because
    // `watch` coalesces: against a real shutdown, whose two transitions happen
    // in one poll, a receiver is *expected* to observe only `Stopped`, and an
    // assertion of the full sequence would be green or red depending on the
    // scheduler. This form asserts the channel's ordering guarantee, which is
    // the part that is actually a guarantee.
    #[tokio::test]
    async fn a_subscriber_that_keeps_up_observes_every_state_in_order() {
        let rt = Runtime::start(test_config()).await.unwrap();
        let mut receiver = rt.subscribe();

        let mut seen = vec![*receiver.borrow_and_update()];
        for next in [LifecycleState::Draining, LifecycleState::Stopped] {
            assert!(rt.transition_to(next));
            receiver.changed().await.unwrap();
            seen.push(*receiver.borrow_and_update());
        }

        assert_eq!(
            seen,
            vec![
                LifecycleState::Running,
                LifecycleState::Draining,
                LifecycleState::Stopped
            ]
        );
    }

    // A subscriber created after the fact still learns the current state, which
    // is what makes `await_stopped` safe for a caller that arrives late.
    #[tokio::test]
    async fn a_late_subscriber_sees_the_state_the_runtime_is_actually_in() {
        let rt = Runtime::start(test_config()).await.unwrap();
        rt.shutdown().await.unwrap();
        assert_eq!(*rt.subscribe().borrow(), LifecycleState::Stopped);
    }

    // Two callers must not both drive the machine: with a separate test and
    // transition, both could observe `Running` and both proceed, which once the
    // drain seam does real work means draining twice.
    //
    // OS threads released by a `Barrier`, deliberately not tokio tasks.
    // `shutdown` contains no await inside its critical region, so tokio tasks
    // run to completion before the next is polled and never overlap no matter
    // how many workers the runtime has — a task-based version of this test
    // passes against a check-then-act implementation, which is the exact
    // regression it exists to catch.
    //
    // Mutation-tested rather than assumed. Against a `shutdown` that tests the
    // state and then transitions in a second lock acquisition, the previous
    // tokio-task form of this test detected the race 0 times in 60 runs; this
    // form detected it 60 times in 60, and 60/60 again against the wider
    // check-`== Stopped`-then-transition variant. If these constants are
    // lowered, re-run that mutation — at 8 threads and 200 rounds detection
    // drops to 42/60, which is not a guard.
    //
    // Many short rounds rather than one long one: the window a racy
    // implementation opens is the gap between dropping the lock after the test
    // and retaking it for the write, so what matters is how often threads
    // arrive at a *fresh* `Running` runtime together, not how long any one of
    // them runs.
    //
    // The reader thread added alongside them takes part in the same barrier, so
    // the contenders are still released together and the constants above still
    // mean what they meant when they were measured.
    #[test]
    fn concurrent_shutdowns_elect_exactly_one_winner() {
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const THREADS: usize = 16;
        const ROUNDS: usize = 1_000;

        // One executor for the whole test. `Handle::block_on` drives the future
        // on the *calling* thread rather than handing it to a worker, so the
        // threads below contend directly instead of being serialised through
        // the scheduler.
        let executor = tokio::runtime::Builder::new_multi_thread().build().unwrap();

        for round in 0..ROUNDS {
            let rt = Arc::new(executor.block_on(Runtime::start(test_config())).unwrap());
            // +1 for the reader.
            let barrier = Arc::new(Barrier::new(THREADS + 1));
            let drivers = Arc::new(AtomicUsize::new(0));

            std::thread::scope(|scope| {
                for _ in 0..THREADS {
                    let rt = Arc::clone(&rt);
                    let barrier = Arc::clone(&barrier);
                    let drivers = Arc::clone(&drivers);
                    let handle = executor.handle().clone();
                    scope.spawn(move || {
                        // Everything that can be done ahead of the contended
                        // section is done before the barrier, so the threads
                        // are released as close to simultaneously as the OS
                        // allows.
                        let shutdown = async { rt.shutdown_inner().await };
                        barrier.wait();
                        // `matches!`, not `==`: `RuntimeError` is deliberately
                        // not `PartialEq`, and comparing errors is not what
                        // this asserts anyway.
                        if matches!(handle.block_on(shutdown), Ok(ShutdownRole::Driver)) {
                            drivers.fetch_add(1, Ordering::SeqCst);
                        }
                        // Whatever this caller's role was, `shutdown` returning
                        // means the runtime is stopped. A loser that returned
                        // early would be caught here observing `Running` or
                        // `Draining`.
                        assert_eq!(
                            rt.state(),
                            LifecycleState::Stopped,
                            "round {round}: shutdown returned before the runtime stopped"
                        );
                    });
                }

                // Read-only, and asserts the one thing a concurrent observer of
                // a state machine must be able to rely on.
                let rt = Arc::clone(&rt);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    let mut highest = LifecycleState::Running.rank();
                    for _ in 0..10_000 {
                        let observed = rt.state().rank();
                        assert!(
                            observed >= highest,
                            "round {round}: state went backwards, {observed} after {highest}"
                        );
                        highest = observed;
                        if highest == LifecycleState::Stopped.rank() {
                            break;
                        }
                    }
                });
            });

            assert_eq!(
                drivers.load(Ordering::SeqCst),
                1,
                "round {round}: expected exactly one caller to own the shutdown"
            );
            assert_eq!(
                rt.transitions(),
                vec![
                    LifecycleState::Running,
                    LifecycleState::Draining,
                    LifecycleState::Stopped
                ],
                "round {round}"
            );
        }
    }

    // `Runtime` derives `Debug` purely on the strength of `Config` redacting its
    // untyped extensions table. Asserting it here means replacing the field with
    // something that renders differently fails a test rather than quietly
    // turning `tracing::debug!(?runtime)` into a secret disclosure.
    #[tokio::test]
    async fn debug_does_not_print_an_extension_secret() {
        let secretish = "A".repeat(30);
        let raw = format!(
            r#"
schema_version = "psyche.config.v1"
data_dir = "/tmp/psyche-test"

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"

[extensions."psyche.experiment.v1"]
looks_like_a_secret = "{secretish}"
"#
        );
        let rt = Runtime::start(psyche_config::load_str(&raw).unwrap())
            .await
            .unwrap();
        let rendered = format!("{rt:?}");
        assert!(!rendered.contains("looks_like_a_secret"), "{rendered}");
        assert!(!rendered.contains(&secretish), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
    }

    #[test]
    fn a_lifecycle_only_ever_moves_forward() {
        let mut lifecycle = Lifecycle {
            current: LifecycleState::Running,
            history: vec![LifecycleState::Running],
        };
        assert!(!lifecycle.advance(LifecycleState::Running));
        assert!(lifecycle.advance(LifecycleState::Stopped));
        // Backwards from the terminal state, which is what a resurrected
        // runtime would look like to `psyche status`.
        assert!(!lifecycle.advance(LifecycleState::Draining));
        assert_eq!(lifecycle.current, LifecycleState::Stopped);
        assert_eq!(
            lifecycle.history,
            vec![LifecycleState::Running, LifecycleState::Stopped]
        );
    }

    // A poisoned lock must not take the daemon's shutdown path down with it.
    #[tokio::test]
    async fn a_poisoned_lock_does_not_panic_the_shutdown_path() {
        let rt = Runtime::start(test_config()).await.unwrap();
        let lock = Arc::clone(&rt.lifecycle);
        std::thread::spawn(move || {
            let _guard = lock.lock().unwrap();
            panic!("poison the lifecycle mutex");
        })
        .join()
        .expect_err("the spawned thread is expected to panic");
        assert!(rt.lifecycle.is_poisoned());

        assert_eq!(rt.state(), LifecycleState::Running);
        rt.shutdown().await.unwrap();
        assert_eq!(rt.state(), LifecycleState::Stopped);
    }
}
```

- [ ] **Step 2: Create the manifest**

Create `crates/psyche-runtime/Cargo.toml`:

```toml
[package]
name = "psyche-runtime"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
psyche-config = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p psyche-runtime`
Expected: FAIL — `no function or associated item named start found for struct Runtime`.

- [ ] **Step 4: Write the minimal implementation**

Insert into `crates/psyche-runtime/src/lib.rs`, above the `#[cfg(test)]` block:

```rust
impl LifecycleState {
    /// Position in the lifecycle. The lifecycle only ever moves forward, and
    /// [`Lifecycle::advance`] enforces it against this.
    ///
    /// Deliberately not a public `Ord` derive: the ordering is an internal
    /// invariant of the state machine, and publishing it would invite
    /// `state < Stopped` comparisons that a future non-linear state (a failed
    /// or restarting runtime) could not honour.
    fn rank(self) -> u8 {
        match self {
            LifecycleState::Running => 0,
            LifecycleState::Draining => 1,
            LifecycleState::Stopped => 2,
        }
    }
}

/// The wire spelling of a state: `running`, `draining`, `stopped`.
///
/// This exists so no consumer has to reach for `{:?}`. A `Debug` rendering that
/// something else formats into JSON or a log field becomes a compatibility
/// surface that can never be renamed — and `Debug` here would emit the
/// capitalised variant name, which is not what any of them want.
impl fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LifecycleState::Running => "running",
            LifecycleState::Draining => "draining",
            LifecycleState::Stopped => "stopped",
        })
    }
}

/// Failures from driving the runtime lifecycle.
///
/// Deliberately empty. Nothing in this slice can fail: [`Runtime::start`] does
/// no I/O yet, and a losing [`Runtime::shutdown`] caller waits for the winner
/// and returns `Ok` rather than erroring. The type and the `Result` signatures
/// exist so that the first real failure — opening `data_dir`, binding the Coven
/// socket, acquiring a lease — is an added variant rather than a breaking
/// signature change.
///
/// Do not add a variant speculatively. Add it with the code that returns it. An
/// earlier draft carried a `ShutdownInProgress` variant that nothing
/// constructed; it implied to every reader that `shutdown` refuses a second
/// caller, which is the behaviour the waiting loser deliberately replaced.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeError {}

/// Which side of the shutdown election a caller ended up on.
///
/// Private, and the reason [`Runtime::shutdown`] is a thin wrapper over
/// [`Runtime::shutdown_inner`]: the public contract is that *the runtime is
/// stopped*, which is true for both roles, but "exactly one caller advances the
/// state machine" is an internal invariant that the concurrency test has to be
/// able to see. Exposing it publicly would be API no caller has asked for; if
/// one ever does, the extension point is `Ok(ShutdownOutcome)`, never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownRole {
    /// Won the election and drove the drain.
    Driver,
    /// Lost the election and waited for the driver to reach
    /// [`LifecycleState::Stopped`].
    Observer,
}

/// Current state plus the ordered log of states this runtime has occupied.
///
/// One mutex, not two: [`Runtime::shutdown`] must decide *and* publish its
/// transition without another caller interleaving, which is impossible if the
/// state and its log are separately locked.
#[derive(Debug)]
struct Lifecycle {
    current: LifecycleState,
    /// Bounded by construction: [`Lifecycle::advance`] refuses any move that is
    /// not strictly forward, and [`LifecycleState`] has three variants, so this
    /// holds at most three entries for the life of the process. An unguarded
    /// `push` would be a slow leak in a daemon that runs for months.
    history: Vec<LifecycleState>,
}

impl Lifecycle {
    /// Moves to `next` and records it, if `next` is strictly forward of the
    /// current state. Returns whether the move happened.
    ///
    /// Rejecting a non-forward move is what bounds `history`; it is not merely
    /// defensive. It also makes a double transition a no-op rather than a
    /// duplicate log entry that would break the ordering assertion.
    fn advance(&mut self, next: LifecycleState) -> bool {
        if next.rank() <= self.current.rank() {
            return false;
        }
        self.current = next;
        self.history.push(next);
        true
    }
}

/// The daemon composition root.
///
/// Deriving `Debug` is safe here only because [`psyche_config::Config`] redacts
/// its untyped `extensions` table, so `tracing::debug!(?runtime)` cannot print a
/// secret placed there. That property belongs to `Config` — if this field is
/// ever replaced with something that renders differently, this derive must be
/// revisited.
#[derive(Debug)]
pub struct Runtime {
    lifecycle: Arc<Mutex<Lifecycle>>,
    /// Publishes each transition to [`Runtime::subscribe`]rs, and is how a
    /// losing shutdown caller waits for the drain it did not drive.
    ///
    /// The mutex above remains the election; this is only the announcement.
    /// `watch::Sender::send` has no compare-and-swap, so deciding the transition
    /// here instead would lose the single-acquisition property that the
    /// 24,000-attempt concurrency test exists to protect.
    state_tx: watch::Sender<LifecycleState>,
    config: Config,
}

impl Runtime {
    /// Builds the composition root and brings it to [`LifecycleState::Running`].
    ///
    /// `async` although nothing is awaited yet, and fallible although nothing
    /// fails yet, for the same reason: the store and lease wiring in the
    /// follow-on G2 plan starts here, and that work — opening `data_dir`,
    /// binding the Coven socket, acquiring a lease — is exactly what fails.
    /// Widening either signature later would break every caller, which is the
    /// break these two decisions exist to avoid.
    ///
    /// # Errors
    ///
    /// None are possible in this build — [`RuntimeError`] has no variants, so
    /// this always returns `Ok`. The signature is the point: the first thing
    /// startup acquires becomes a variant, not a breaking change.
    pub async fn start(config: Config) -> Result<Self, RuntimeError> {
        // `Sender::new`, not `watch::channel(..)`: `channel` also hands back a
        // `Receiver` that this type has no use for, and dropping it would leave
        // a sender with no receivers. See `transition_to` for why that matters.
        let state_tx = watch::Sender::new(LifecycleState::Running);
        tracing::info!(state = %LifecycleState::Running, "psyche runtime started");
        Ok(Self {
            lifecycle: Arc::new(Mutex::new(Lifecycle {
                current: LifecycleState::Running,
                history: vec![LifecycleState::Running],
            })),
            state_tx,
            config,
        })
    }

    /// Takes the lifecycle lock, recovering from poisoning instead of panicking.
    ///
    /// `expect` is denied outside tests, and the shutdown path is the last place
    /// that should panic — a daemon that panics on the way down leaves its
    /// socket and any lease behind. Poisoning means an earlier holder panicked,
    /// but every critical section here is a field assignment plus a `push` onto
    /// a three-element `Vec`, neither of which can leave `Lifecycle` in a
    /// half-written state. Recovering the guard is therefore sound, and the
    /// runtime still reaches `Stopped`.
    fn lifecycle(&self) -> MutexGuard<'_, Lifecycle> {
        self.lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The state this runtime currently occupies.
    #[must_use]
    pub fn state(&self) -> LifecycleState {
        self.lifecycle().current
    }

    /// The configuration this runtime was started with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// A receiver that observes each state this runtime enters.
    ///
    /// The only way to learn that a runtime has stopped without polling
    /// [`Runtime::state`]. Two properties of `watch` a caller must know:
    ///
    /// - The current state counts as already seen, so `changed()` reports the
    ///   *next* transition. Read [`watch::Receiver::borrow`] first if the state
    ///   at subscription time matters.
    /// - Values coalesce. A receiver that does not keep up sees the latest
    ///   state, not every state — a subscriber polled once after a fast
    ///   shutdown observes `Stopped` and never `Draining`. [`Runtime::transitions`]
    ///   is the lossless record; this is the live one.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<LifecycleState> {
        self.state_tx.subscribe()
    }

    /// Every state this runtime has occupied, oldest first.
    ///
    /// Ordered, not a set: the contract graceful shutdown owes an operator is
    /// that `Draining` happened *between* `Running` and `Stopped`, which a
    /// final-state check cannot distinguish from skipping the drain entirely.
    /// At most three entries — see [`Lifecycle::history`].
    #[must_use]
    pub fn transitions(&self) -> Vec<LifecycleState> {
        self.lifecycle().history.clone()
    }

    /// Records a forward transition, publishing and logging it if it happened,
    /// and reports whether it happened.
    ///
    /// The bool is the concurrency primitive: whichever caller gets `true` for
    /// [`LifecycleState::Draining`] owns the shutdown, because the test and the
    /// write both happen inside one lock acquisition.
    fn transition_to(&self, next: LifecycleState) -> bool {
        let mut lifecycle = self.lifecycle();
        if !lifecycle.advance(next) {
            return false;
        }
        // Published while the guard is still held, so a subscriber can never
        // observe a state the machine has not entered, and two transitions
        // cannot be published in the opposite order to the one they were made.
        //
        // `send_replace`, not `send`: `send` fails when no receiver exists and
        // — this is the part that bites — does not store the value it failed to
        // deliver. A `let _ = send(..)` here would leave the published state at
        // `Running` for a runtime nobody had subscribed to yet, and the first
        // losing caller to subscribe would then wait on a `Stopped` that had
        // already been sent and dropped. `send_replace` always stores.
        self.state_tx.send_replace(next);
        // Dropped before logging: a subscriber's `Layer` runs arbitrary code on
        // this thread, and panicking with the lifecycle lock held would poison
        // it in the middle of shutdown.
        drop(lifecycle);
        tracing::info!(state = %next, "psyche lifecycle transition");
        true
    }

    /// Waits until [`LifecycleState::Stopped`] has been published.
    ///
    /// Returns immediately if it already has, which is the common case for a
    /// caller arriving after the runtime has fully stopped.
    async fn await_stopped(&self) {
        let mut receiver = self.subscribe();
        // `borrow_and_update` before `changed`, in that order and in a loop:
        // `subscribe` marks the current value as seen, so awaiting `changed`
        // first would miss a `Stopped` that was published before this caller
        // subscribed, and wait for a transition that can never come.
        while *receiver.borrow_and_update() != LifecycleState::Stopped {
            if receiver.changed().await.is_err() {
                // Unreachable: the sender lives in `self`, which this call
                // borrows. Breaking rather than panicking anyway — this is the
                // shutdown path.
                break;
            }
        }
    }

    /// Stops intake, drains in-flight work, then exits. There is no forced
    /// path — a caller wanting immediate exit terminates the process.
    ///
    /// Safe to call from any number of callers. Exactly one drives the drain;
    /// the rest wait for it and return once the runtime is
    /// [`LifecycleState::Stopped`]. That waiting is not politeness, it is the
    /// contract: an operator whose first SIGTERM appears to do nothing sends a
    /// second, and a caller that returned immediately would let `psyched` fall
    /// out of `main` and exit the process mid-drain — graceful shutdown
    /// defeated by ordinary operator behaviour.
    ///
    /// Returns `Ok(())` for both roles. A caller that merely waited still has
    /// the answer it asked for — the runtime is stopped, and it stopped
    /// gracefully. An error would be false by the time it was returned, and
    /// every caller would have to translate it back into success.
    ///
    /// # Errors
    ///
    /// None are possible in this build — [`RuntimeError`] has no variants, so
    /// this always returns `Ok`. Losing the election is explicitly *not* an
    /// error; that is what the paragraph above is about.
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        match self.shutdown_inner().await? {
            // Both roles, one answer — see the note above.
            ShutdownRole::Driver | ShutdownRole::Observer => Ok(()),
        }
    }

    /// [`Runtime::shutdown`], plus which side of the election the caller was on.
    async fn shutdown_inner(&self) -> Result<ShutdownRole, RuntimeError> {
        // Claim the shutdown and publish `Draining` under a single lock
        // acquisition. Testing the state and then transitioning in a second
        // acquisition would let two concurrent callers both observe `Running`
        // and both drive the machine, which once there is real drain work means
        // running it twice.
        if !self.transition_to(LifecycleState::Draining) {
            self.await_stopped().await;
            return Ok(ShutdownRole::Observer);
        }

        // The drain seam. Nothing durable is in flight in this slice; the store
        // and lease work in the follow-on G2 plan awaits here. No lifecycle
        // guard is live across it — `transition_to` takes and drops its own —
        // which is what keeps this future `Send`; the assertion below fails to
        // compile if that stops being true.

        self.transition_to(LifecycleState::Stopped);
        Ok(ShutdownRole::Driver)
    }
}

// psyche-cli holds a `Runtime` across tokio task boundaries and awaits these two
// futures inside `tokio::spawn`.
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<Runtime>();
    assert_send_sync_static::<RuntimeError>();

    // The types being `Send` is not the property the drain seam needs. Holding a
    // `std` `MutexGuard` across the await in `shutdown` would leave `Runtime`
    // perfectly `Send` and make the *future* `!Send`, and the error would
    // surface in psyche-cli at the `tokio::spawn`, naming the spawn rather than
    // the seam. These two assertions put it here instead.
    fn assert_send<T: Send>(_: &T) {}
    fn futures_are_send(runtime: &Runtime, config: Config) {
        assert_send(&Runtime::start(config));
        assert_send(&runtime.shutdown());
    }
};
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p psyche-runtime`
Expected: `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 6: Commit**

```bash
git add crates/psyche-runtime
git commit -m "feat(runtime): add lifecycle with ordered graceful shutdown"
```

---

## Task 6: `psyche-cli` with credential-free `doctor`

**Files:**
- Create: `crates/psyche-cli/Cargo.toml`
- Create: `crates/psyche-cli/src/lib.rs`
- Create: `crates/psyche-cli/src/main.rs`
- Create: `crates/psyche-cli/src/daemon.rs`
- Create: `crates/psyche-cli/src/doctor.rs`
- Create: `crates/psyche-cli/src/status.rs`
- Create: `crates/psyche-cli/src/logging.rs`
- Create: `crates/psyche-cli/src/bin/psyched.rs`
- Create: `crates/psyche-cli/tests/cli.rs`

- [ ] **Step 0: Register the crate**

Per the rule from Task 2 — cargo loads every declared member on any command, so
naming one before it exists breaks the workspace. Extend the root `members` with
`crates/psyche-cli`. No `[workspace.dependencies]` entry is needed: nothing
depends on the CLI.

- [ ] **Step 1: Write the failing test**

Create `crates/psyche-cli/tests/cli.rs`:

```rust
//! End-to-end checks over the two shipped binaries.
//!
//! These run the real executables, so they cover the whole output surface an
//! operator sees — including the two security properties this crate owns:
//! `doctor` never renders a `Config` with `{:?}`, and never prints an extension
//! value.

use assert_cmd::Command;
// `.and(..)` on a predicate comes from this trait, not from `contains` itself.
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;
// The exit codes are asserted against the constants the binaries return, not
// against literals: a test carrying its own copy of `3` would keep passing
// through a renumbering that broke every unit file in the field.
use psyche_cli::{EXIT_CONFIG, EXIT_UNAVAILABLE};
// Split out rather than folded into the line above: its only use is inside a
// `#[cfg(unix)]` test, so on Windows the import is dead and `-D warnings` — which
// CI sets workflow-wide — promotes that to a hard error in both the clippy and
// the test job. Latent on Unix, which is why it survived until a cross-check.
#[cfg(unix)]
use psyche_cli::EXIT_CHECK_FAILED;

/// A 30-byte stand-in for a credential parked in an extension table. Long and
/// distinctive so a partial echo is still detectable by the window scan below.
const SECRETISH: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// Renders a path as a TOML basic string, escaping what TOML gives meaning to.
///
/// A Windows temp directory is `C:\Users\RUNNER~1\AppData\Local\Temp\...`, and
/// interpolating that raw makes `\U` a unicode escape: the loader fails with
/// "too few unicode value digits" and the test reads as though the configuration
/// loader were broken rather than the fixture. Nothing platform-specific here —
/// the escaping is simply what writing a path into TOML has always required.
fn toml_str(path: &std::path::Path) -> String {
    format!(
        "\"{}\"",
        path.display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

fn config_body(data_dir: &std::path::Path) -> String {
    format!(
        r#"
schema_version = "psyche.config.v1"
data_dir = {}

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#,
        toml_str(data_dir)
    )
}

// These return `io::Result` and are unwrapped by their callers rather than
// unwrapping internally: `clippy.toml` sets `allow-unwrap-in-tests`, but clippy
// recognises only frames reachable from a `#[test]`-annotated function, so an
// `unwrap` in a free helper here is a hard `-D clippy::unwrap-used` error. That
// is the right place for the panic anyway — it names the failing test.
fn write_config(dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    write_config_with(dir, "")
}

fn write_config_with(dir: &std::path::Path, extra: &str) -> std::io::Result<std::path::PathBuf> {
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir)?;
    let path = dir.join("psyche.toml");
    std::fs::write(&path, format!("{}{extra}", config_body(&data_dir)))?;
    Ok(path)
}

/// Asserts `haystack` carries no 8-byte window of `needle`.
///
/// Substring equality alone would miss a truncated or line-wrapped echo, which
/// is still a disclosure. Mirrors the window scan psyche-config uses.
fn assert_no_trace_of(haystack: &str, needle: &str, label: &str) {
    for window in needle.as_bytes().windows(8) {
        let fragment = String::from_utf8_lossy(window);
        assert!(
            !haystack.contains(fragment.as_ref()),
            "{label} echoed {fragment:?}: {haystack}"
        );
    }
}

#[test]
fn doctor_succeeds_without_any_telegram_credentials() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("TELEGRAM_BOT_TOKEN")
        .env_remove("PSYCHE_TELEGRAM_TOKEN")
        .args(["doctor", "--config", config.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("config: ok").and(contains("data_dir: ok")));
}

/// `status` emits no `state` at all, because it observed none.
///
/// The document used to say `{"state":"stopped","observed":false}`, which is a
/// false statement on any host where a daemon *is* running — and `jq -r .state`
/// is what people actually write. The caveat has to be structural: a field that
/// is absent cannot be read past.
///
/// Parsed rather than substring-matched, which also pins that stdout is a whole
/// valid JSON document: a log line leaking onto stdout would fail here.
#[test]
fn status_json_does_not_report_a_state_it_could_not_observe() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    let assert = Command::cargo_bin("psyche")
        .unwrap()
        .args(["status", "--config", config.to_str().unwrap(), "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let document: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    // Versioned, like `psyche.config.v1` and `coven.daemon.v1`. This repository
    // treats schema versioning as first-class everywhere except, until now, its
    // own machine-readable output.
    assert_eq!(
        document["schema"],
        serde_json::json!("psyche.status.v1"),
        "{stdout}"
    );
    assert_eq!(
        document.get("observed"),
        Some(&serde_json::json!(false)),
        "the answer must be marked as not observed: {stdout}"
    );
    // Present and null, not absent: a consumer distinguishing "no state" from
    // "no such field" should not have to.
    assert_eq!(
        document.get("state"),
        Some(&serde_json::Value::Null),
        "state must be null when nothing was observed: {stdout}"
    );
    assert_eq!(document["reason"], serde_json::json!("no-ipc"), "{stdout}");
}

/// The human rendering carries the same caveat, and likewise names no state.
/// Without this the two output modes disagree about how much the command knows.
#[test]
fn status_text_says_the_state_was_not_observed() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    Command::cargo_bin("psyche")
        .unwrap()
        .args(["status", "--config", config.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("not observed").and(contains("no-ipc")))
        // The old rendering led with `state: stopped`, which is the claim being
        // retracted. An operator skimming for a state word must not find one.
        .stdout(contains("stopped").not());
}

/// The reason lands in the report, on stdout, inside the `config` check — not as
/// one raw error on stderr with no checks run. `doctor` is dispatched before the
/// configuration is loaded precisely so it has something to say here.
#[test]
fn doctor_reports_a_bad_schema_version_as_a_failed_check() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("psyche.toml");
    std::fs::write(
        &path,
        r#"
schema_version = "psyche.config.v99"
data_dir = "/tmp"

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#,
    )
    .unwrap();
    Command::cargo_bin("psyche")
        .unwrap()
        .args(["doctor", "--config", path.to_str().unwrap()])
        .assert()
        .code(i32::from(EXIT_CONFIG))
        .stdout(
            contains("config: fail")
                .and(contains("unsupported schema_version"))
                .and(contains("psyche.config.v99"))
                // Every dependent check is reported as not run. A shorter list
                // would read as though they had passed.
                .and(contains("data_dir: skipped"))
                .and(contains("coven_socket_path: skipped"))
                .and(contains("extensions: skipped")),
        );
}

/// `psyche doctor --config /nope.toml` used to print one raw `ConfigError` and
/// exit with **zero checks run** — the single case the command most exists for
/// was the one case it refused to run in.
#[test]
fn doctor_still_runs_when_the_config_file_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("nope.toml");
    Command::cargo_bin("psyche")
        .unwrap()
        .args(["doctor", "--config", missing.to_str().unwrap()])
        .assert()
        .code(i32::from(EXIT_CONFIG))
        .stdout(contains("config: fail").and(contains("nope.toml")))
        // Four lines, always. The check list is the contract; a load failure
        // shortens no part of it.
        .stdout(predicates::function::function(|out: &str| {
            out.lines().count() == 4
        }));
}

/// An unwritable `data_dir` is a failed *check*, not a bad configuration, and
/// the two get different codes on purpose — an operator scripting `doctor`
/// cannot otherwise tell "your file is malformed" from "your disk is not
/// writable".
#[cfg(unix)]
#[test]
fn doctor_exits_with_the_check_failed_code_on_an_unwritable_data_dir() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempfile::tempdir().unwrap();
    let blocked = tmp.path().join("blocked");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o500)).unwrap();

    // Root ignores mode bits. Determined by trying rather than by reading a uid,
    // which would need libc — forbidden here.
    if std::fs::write(blocked.join(".root-check"), b"").is_ok() {
        eprintln!(
            "skipping: this process writes through mode 0o500 (root, or a permissionless fs)"
        );
        return;
    }

    let path = tmp.path().join("psyche.toml");
    std::fs::write(
        &path,
        format!(
            r#"
schema_version = "psyche.config.v1"
data_dir = {}

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#,
            toml_str(&blocked)
        ),
    )
    .unwrap();

    Command::cargo_bin("psyche")
        .unwrap()
        .args(["doctor", "--config", path.to_str().unwrap()])
        .assert()
        .code(i32::from(EXIT_CHECK_FAILED))
        .stdout(contains("data_dir: fail"));

    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// `doctor --json` is versioned for the same reason the configuration and the
/// Coven API are: the alternative is the `name: status (detail)` line format
/// getting `grep`ped into a contract nobody chose to make.
///
/// Parsed rather than substring-matched, which also pins that stdout is one
/// whole JSON document — a log line leaking onto stdout would fail here.
#[test]
fn doctor_json_emits_a_versioned_document() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    let assert = Command::cargo_bin("psyche")
        .unwrap()
        .args(["doctor", "--config", config.to_str().unwrap(), "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let document: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    assert_eq!(
        document["schema"],
        serde_json::json!("psyche.doctor.v1"),
        "{stdout}"
    );
    assert_eq!(document["failed"], serde_json::json!(0), "{stdout}");
    let checks = document["checks"].as_array().unwrap();
    let names: Vec<&str> = checks.iter().filter_map(|c| c["name"].as_str()).collect();
    assert_eq!(
        names,
        ["config", "data_dir", "coven_socket_path", "extensions"],
        "{stdout}"
    );
    // `coven_socket_path` contacts nothing and cannot fail. Reporting it as `ok`
    // in a list where any non-`ok` fails the command was a claim of
    // verification it never performed.
    assert_eq!(checks[2]["status"], serde_json::json!("info"), "{stdout}");
}

#[test]
fn start_and_stop_run_without_any_telegram_credentials() {
    // coven-psy1 acceptance requires all four subcommands to run with no
    // credentials present, not just doctor and status.
    //
    // `start` is driven with `--shutdown-after-start` because it now actually
    // starts the daemon. The previous form of this test passed against a stub
    // that printed a line and exited 0, which is precisely the thing that made
    // the subcommand's own help text a lie.
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    let path = config.to_str().unwrap();

    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("TELEGRAM_BOT_TOKEN")
        .env_remove("PSYCHE_TELEGRAM_TOKEN")
        .args(["start", "--config", path, "--shutdown-after-start"])
        .assert()
        .success()
        // Logs go to stderr, never stdout. The invariant was pinned only for
        // `status --json`, and the daemon is the path that logs volumes — a
        // subscriber misconfigured to stdout would corrupt every `--json`
        // pipeline running alongside it.
        .stdout(predicates::str::is_empty());

    // `stop` is asserted against its documented code, not `.success()`. It has
    // no daemon IPC to use, so "did nothing" is the truthful answer and 0 would
    // be the false one — see `stop_reports_that_it_cannot_reach_a_daemon`.
    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("TELEGRAM_BOT_TOKEN")
        .env_remove("PSYCHE_TELEGRAM_TOKEN")
        .args(["stop", "--config", path])
        .assert()
        .code(i32::from(EXIT_UNAVAILABLE));
}

/// `psyche stop` used to print `no running daemon to stop` and exit 0, while its
/// own help text promised it would "ask a running daemon to shut down
/// gracefully". Structurally the same defect as the `psyche start` stub: the
/// scripted failure is `psyche stop && deploy`, or a rolling restart that
/// believes the old daemon is gone.
#[test]
fn stop_reports_that_it_cannot_reach_a_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    Command::cargo_bin("psyche")
        .unwrap()
        .args(["stop", "--config", config.to_str().unwrap()])
        .assert()
        .code(i32::from(EXIT_UNAVAILABLE))
        .stderr(contains("not implemented in this build"))
        // The caveat belongs on stderr so a `--json`-style stdout pipeline stays
        // clean, and so `stop > /dev/null` cannot hide it.
        .stdout(predicates::str::is_empty());
}

/// `--config` beats `$PSYCHE_CONFIG` beats `./psyche.toml`, on both binaries.
///
/// The default is relative to the working directory, and a systemd system unit
/// defaults to `WorkingDirectory=/` — so a `psyched.service` without an explicit
/// `--config` was resolving `/psyche.toml`. An environment variable is also how a
/// container image parameterises this, and there was none.
#[test]
fn config_resolves_from_the_flag_then_the_environment_then_the_default() {
    let tmp = tempfile::tempdir().unwrap();
    let chosen = write_config(tmp.path()).unwrap();
    // A path that would fail to load if it were ever preferred over `--config`.
    let decoy = tmp.path().join("decoy.toml");
    std::fs::write(&decoy, "schema_version = \"psyche.config.v99\"\n").unwrap();

    for binary in ["psyche", "psyched"] {
        // `$PSYCHE_CONFIG` alone.
        let mut command = Command::cargo_bin(binary).unwrap();
        if binary == "psyche" {
            command.arg("start");
        }
        command
            .env("PSYCHE_CONFIG", &chosen)
            .arg("--shutdown-after-start")
            .assert()
            .success();

        // `--config` wins over `$PSYCHE_CONFIG`; the decoy would exit 3.
        let mut command = Command::cargo_bin(binary).unwrap();
        if binary == "psyche" {
            command.arg("start");
        }
        command
            .env("PSYCHE_CONFIG", &decoy)
            .args([
                "--config",
                chosen.to_str().unwrap(),
                "--shutdown-after-start",
            ])
            .assert()
            .success();
    }
}

/// With neither the flag nor the variable set, the default is `./psyche.toml` —
/// and the error has to name the path that was actually tried, or an operator
/// whose service resolved `/psyche.toml` has nothing to go on.
#[test]
fn the_default_config_is_relative_to_the_working_directory() {
    let present = tempfile::tempdir().unwrap();
    write_config(present.path()).unwrap();
    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("PSYCHE_CONFIG")
        .current_dir(present.path())
        .arg("doctor")
        .assert()
        .success();

    // `doctor` puts the reason in its report on stdout; `status` short-circuits
    // on stderr. Both have to name the file they actually tried.
    let absent = tempfile::tempdir().unwrap();
    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("PSYCHE_CONFIG")
        .current_dir(absent.path())
        .arg("doctor")
        .assert()
        .code(i32::from(EXIT_CONFIG))
        .stdout(contains("psyche.toml"));

    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("PSYCHE_CONFIG")
        .current_dir(absent.path())
        .arg("status")
        .assert()
        .code(i32::from(EXIT_CONFIG))
        .stderr(contains("psyche.toml"));
}

/// `psyche --config X status` reads as the natural order and used to be a usage
/// error. A global argument accepts both placements, and the old form still
/// works — this asserts the pair, because "global" is only worth having if it is
/// backwards compatible.
#[test]
fn config_is_accepted_before_or_after_the_subcommand() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    let path = config.to_str().unwrap();

    for args in [["--config", path, "status"], ["status", "--config", path]] {
        Command::cargo_bin("psyche")
            .unwrap()
            .args(args)
            .assert()
            .success();
    }
}

/// Every long option `psyche start` accepts, `psyched` accepts too, and the
/// reverse.
///
/// The two are documented as equivalent, and `daemon.rs` is shared so they
/// cannot drift in behaviour — but nothing stopped one of them growing a flag
/// the other lacked, which is the same promise broken at the argument layer.
#[test]
fn psyche_start_and_psyched_accept_the_same_flags() {
    /// Long option names appearing anywhere in a help text.
    ///
    /// A scan rather than a regex: this crate has no regex dependency and does
    /// not need one to find `--word` tokens.
    fn long_options(help: &str) -> std::collections::BTreeSet<String> {
        let mut found = std::collections::BTreeSet::new();
        for word in help.split_whitespace() {
            let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            // Nested rather than a `let` chain: those stabilised after this
            // workspace's 1.85 MSRV.
            if let Some(name) = word.strip_prefix("--") {
                let plausible = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                if plausible {
                    found.insert(name.to_owned());
                }
            }
        }
        // clap attaches these itself, and at different levels: `--version` sits
        // on `psyche` rather than on `psyche start`. Neither is a flag either
        // program declares, so neither is a drift this test is about.
        found.remove("help");
        found.remove("version");
        found
    }

    let of = |args: &[&str], binary: &str| {
        let assert = Command::cargo_bin(binary)
            .unwrap()
            .args(args)
            .assert()
            .success();
        long_options(&String::from_utf8_lossy(&assert.get_output().stdout))
    };

    let start = of(&["start", "--help"], "psyche");
    let psyched = of(&["--help"], "psyched");
    assert_eq!(start, psyched, "psyche start and psyched disagree on flags");
    // A guard on the scan itself: an extraction that found nothing would make
    // the equality above vacuously true.
    assert!(start.contains("config"), "{start:?}");
    assert!(start.contains("shutdown-after-start"), "{start:?}");
}

/// A malformed `PSYCHE_LOG` is not the same thing as an absent one.
///
/// `try_from_env(..).unwrap_or_else(|_| info)` treated them identically, so a
/// mistyped filter silently ran at info and the operator concluded that the
/// level they asked for was broken. The warning goes to stderr with `eprintln!`
/// because the subscriber is not up yet — `tracing::warn!` here goes nowhere.
///
/// `info=bogus` rather than `trce`: `trce` *parses*, as a bare target directive,
/// so no amount of error handling can catch it. See `logging.rs`.
#[test]
fn a_malformed_log_filter_is_reported_and_an_absent_one_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    let path = config.to_str().unwrap();

    Command::cargo_bin("psyche")
        .unwrap()
        .env("PSYCHE_LOG", "info=bogus")
        .args(["status", "--config", path])
        .assert()
        // A bad filter is not a reason to refuse to run.
        .success()
        .stderr(contains("PSYCHE_LOG").and(contains("not a valid filter")))
        // And it must not corrupt the output stream a consumer parses.
        .stdout(contains("not observed"));

    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("PSYCHE_LOG")
        .args(["status", "--config", path])
        .assert()
        .success()
        .stderr(predicates::str::is_empty());
}

/// The resolution order is stated in `--help`. An operator writing a unit file
/// reads that, not this repository.
#[test]
fn help_states_the_config_resolution_order() {
    for binary in ["psyche", "psyched"] {
        Command::cargo_bin(binary)
            .unwrap()
            .arg("--help")
            .assert()
            .success()
            .stdout(contains("PSYCHE_CONFIG").and(contains("psyche.toml")));
    }
}

/// Every entry point owes the same answer for the same broken configuration.
///
/// Asserted per code rather than as "non-zero": the whole point of the space is
/// that an operator scripting these can tell "your configuration is malformed"
/// from "your environment is not in the state it needs to be", and `failure()`
/// cannot see the difference.
#[test]
fn a_broken_config_exits_with_the_configuration_code() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist.toml");
    let unsupported = tmp.path().join("unsupported.toml");
    std::fs::write(
        &unsupported,
        r#"
schema_version = "psyche.config.v99"
data_dir = "/tmp"

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#,
    )
    .unwrap();
    let malformed = tmp.path().join("malformed.toml");
    std::fs::write(
        &malformed,
        "schema_version = \"psyche.config.v1\"\nthis is not toml",
    )
    .unwrap();

    let entry_points: [(&str, &[&str]); 5] = [
        ("psyche", &["doctor"]),
        ("psyche", &["status"]),
        ("psyche", &["start", "--shutdown-after-start"]),
        ("psyche", &["stop"]),
        ("psyched", &["--shutdown-after-start"]),
    ];

    for path in [&missing, &unsupported, &malformed] {
        for (binary, args) in entry_points {
            let mut command = Command::cargo_bin(binary).unwrap();
            command.args(args);
            command
                .args(["--config", path.to_str().unwrap()])
                .assert()
                .code(i32::from(EXIT_CONFIG));
        }
    }
}

/// `psyche start` says it starts the daemon in the foreground, so it must run
/// the same lifecycle `psyched` runs — not exit 0 having started nothing, which
/// is what `psyche start && systemctl ...` would have read as success.
///
/// Asserted against the transitions themselves rather than the exit code: a stub
/// that exits 0 is exactly what this is here to catch.
#[test]
fn psyche_start_runs_the_same_lifecycle_as_psyched() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    // Rebuilt per iteration: `AndPredicate` is not `Clone` because its `Item`
    // type parameter is `str`, which is unsized.
    let expected = || {
        contains("psyche runtime started")
            .and(contains("\"state\":\"draining\""))
            .and(contains("\"state\":\"stopped\""))
    };

    for binary in ["psyche", "psyched"] {
        let mut command = Command::cargo_bin(binary).unwrap();
        if binary == "psyche" {
            command.arg("start");
        }
        command
            .args([
                "--config",
                config.to_str().unwrap(),
                "--shutdown-after-start",
            ])
            .assert()
            .success()
            .stderr(expected());
    }
}

#[test]
fn psyched_start_then_stop_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path()).unwrap();
    Command::cargo_bin("psyched")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--shutdown-after-start",
        ])
        .assert()
        .success()
        .stderr(contains("psyche lifecycle transition"));
}

/// The signal path, driven with a real signal.
///
/// Every other lifecycle test here uses `--shutdown-after-start`, which by
/// design never installs a handler — so the whole signal branch was unexercised,
/// and `psyched` shipped handling SIGINT only. `systemctl stop`, `docker stop`,
/// and a bare `kill` all send SIGTERM, so the drain was unreachable in exactly
/// the deployments that matter.
///
/// Both binaries and both signals, because `psyche start` and `psyched` are
/// supposed to be the same daemon, and an operator's `kill` is as much a
/// shutdown request as their Ctrl-C.
#[cfg(unix)]
mod signals {
    use std::io::{BufRead as _, BufReader};
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    /// Emitted by `daemon::run` once the runtime is up. Waiting for it is what
    /// keeps the signal from arriving before a handler could possibly exist —
    /// without it this test would be racing startup rather than testing drain.
    const READY: &str = "psyche daemon ready";

    /// Bounds the whole exchange. A daemon that ignores the signal keeps running
    /// forever, and an unbounded read of its stderr would wedge CI rather than
    /// fail it — which has already happened once in this project, for 600s.
    const LIMIT: Duration = Duration::from_secs(30);

    /// Runs `binary args...`, waits for the daemon to report ready, sends
    /// `signal`, and returns the exit status with everything written to stderr.
    ///
    /// Returns `io::Result` and is unwrapped by its caller: `clippy.toml` allows
    /// `unwrap` only in frames reachable from a `#[test]` fn, and a free helper
    /// here is not one.
    fn drain_under(
        binary: &str,
        args: &[&str],
        signal: &str,
    ) -> std::io::Result<(ExitStatus, String)> {
        let mut child = Command::new(binary)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("child stderr was not piped"))?;

        // Read on another thread and deliver through a channel, so every wait
        // below is a `recv_timeout` rather than a blocking read. The thread ends
        // when the child closes stderr, which disconnects the channel and is how
        // the post-signal collection below learns it is done.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });

        let mut log = String::new();
        let timed_out = |e: RecvTimeoutError| {
            std::io::Error::other(format!("gave up waiting on {binary} stderr: {e:?}"))
        };
        loop {
            let line = rx.recv_timeout(LIMIT).map_err(timed_out)?;
            let ready = line.contains(READY);
            log.push_str(&line);
            log.push('\n');
            if ready {
                break;
            }
        }

        // `/bin/kill` through `Command`, not `libc::kill`: `unsafe_code` is
        // forbidden at the workspace level and cannot be re-allowed, so there is
        // no in-process way to raise a signal at another pid.
        let status = Command::new("kill")
            .args([&format!("-{signal}"), &child.id().to_string()])
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "kill -{signal} failed: {status}"
            )));
        }

        // Drain to EOF. Disconnected means the child closed stderr, i.e. exited.
        loop {
            match rx.recv_timeout(LIMIT) {
                Ok(line) => {
                    log.push_str(&line);
                    log.push('\n');
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(e) => return Err(timed_out(e)),
            }
        }

        Ok((child.wait()?, log))
    }

    /// Asserts `log` shows a full graceful drain: `draining`, then `stopped`.
    ///
    /// Ordered, not merely present. A daemon that jumped straight to `stopped`
    /// skipped the drain, which is the whole property being bought here.
    fn assert_drained(label: &str, status: ExitStatus, log: &str) {
        assert!(
            status.success(),
            "{label}: expected a graceful exit, got {status}\n{log}"
        );
        let draining = log
            .find("\"state\":\"draining\"")
            .unwrap_or_else(|| panic!("{label}: never entered draining\n{log}"));
        let stopped = log
            .find("\"state\":\"stopped\"")
            .unwrap_or_else(|| panic!("{label}: never reached stopped\n{log}"));
        assert!(
            draining < stopped,
            "{label}: reached stopped without draining first\n{log}"
        );
    }

    #[test]
    fn both_binaries_drain_on_sigterm_and_sigint() {
        let tmp = tempfile::tempdir().unwrap();
        let config = super::write_config(tmp.path()).unwrap();
        let config = config.to_str().unwrap();

        let cases: [(&str, Vec<&str>, &str); 3] = [
            (
                env!("CARGO_BIN_EXE_psyched"),
                vec!["--config", config],
                "TERM",
            ),
            (
                env!("CARGO_BIN_EXE_psyched"),
                vec!["--config", config],
                "INT",
            ),
            (
                env!("CARGO_BIN_EXE_psyche"),
                vec!["start", "--config", config],
                "TERM",
            ),
        ];

        for (binary, args, signal) in cases {
            let label = format!("{binary} {} on SIG{signal}", args.join(" "));
            let (status, log) = drain_under(binary, &args, signal).unwrap();
            assert_drained(&label, status, &log);
        }
    }
}

/// Extension tables are untyped, so a future one may hold a credential. `doctor`
/// may report how many there are; it may never report what is in them.
///
/// Shown to fail rather than assumed: with `doctor` temporarily printing
/// `config.extensions.get::<serde_json::Value>("psyche.experiment.v1")`, this
/// test goes red on the `looks_like_a_secret` assertion. Restoring the count-only
/// detail returns it to green.
#[test]
fn doctor_output_never_contains_an_extension_value() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config_with(
        tmp.path(),
        &format!(
            "\n[extensions.\"psyche.experiment.v1\"]\nlooks_like_a_secret = \"{SECRETISH}\"\n"
        ),
    )
    .unwrap();
    let assert = Command::cargo_bin("psyche")
        .unwrap()
        .args(["doctor", "--config", config.to_str().unwrap()])
        .assert()
        .success();
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The count is reportable and is reported: without this, a `doctor` that
    // simply never mentions extensions would pass the absence checks below
    // while proving nothing about the redaction.
    //
    // `info`, not `ok`: a tally verifies nothing, and it sat in a list where any
    // non-`ok` entry failed the whole command.
    assert!(stdout.contains("extensions: info"), "{stdout}");
    assert!(stdout.contains("1 table(s) present"), "{stdout}");

    for (stream, label) in [(&stdout, "stdout"), (&stderr, "stderr")] {
        // The inner key is part of the value, unlike the versioned table name.
        assert!(
            !stream.contains("looks_like_a_secret"),
            "{label} printed an extension key: {stream}"
        );
        assert_no_trace_of(stream, SECRETISH, label);
    }
}

/// Rule one of this crate's output discipline: `doctor` prints fields it chose,
/// never a struct dump. `Config`'s `Debug` redacts today, but the guarantee must
/// not rest on a type this crate does not own.
///
/// Also shown to fail: adding `println!("{config:?}")` to `doctor` trips both
/// assertions below.
#[test]
fn doctor_output_is_not_a_config_debug_dump() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config_with(
        tmp.path(),
        "\n[extensions.\"psyche.experiment.v1\"]\nenabled = true\n",
    )
    .unwrap();
    let assert = Command::cargo_bin("psyche")
        .unwrap()
        .args(["doctor", "--config", config.to_str().unwrap()])
        .assert()
        .success();
    let output = assert.get_output();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains("Config {"), "{combined}");
    // `Extensions`' redacting Debug renders "<N key(s) redacted>"; seeing it
    // means something rendered the struct rather than named fields.
    assert!(!combined.contains("redacted"), "{combined}");
}
```

- [ ] **Step 2: Create the manifest**

Create `crates/psyche-cli/Cargo.toml`:

```toml
[package]
name = "psyche-cli"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
# `publish` is NOT inherited implicitly: without this line the member falls back
# to `publish = true` and the workspace's `publish = false` is inert.
publish.workspace = true

# The shared implementation. Declared explicitly alongside the binaries rather
# than left to autodiscovery, so all three targets read from one list. The two
# binaries were previously stitched together with `#[path]` includes, which
# compiled each shared file once per binary and left none of them reachable from
# `tests/` or from a doc-test.
[lib]
name = "psyche_cli"
path = "src/lib.rs"

# Both binary targets are declared explicitly because neither inferred name is
# the one operators type: `src/main.rs` would infer `psyche-cli`, not `psyche`.
[[bin]]
name = "psyche"
path = "src/main.rs"

[[bin]]
name = "psyched"
path = "src/bin/psyched.rs"

[dependencies]
psyche-config = { workspace = true }
psyche-runtime = { workspace = true }
clap = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
# Deliberately NOT depending on psyche-core: everything this crate needs from it
# arrives through `psyche_config::Config` (`schema_version()` returns
# `psyche_core::schema::CONFIG_SCHEMA_VERSION`, and a `SchemaError` reaches the
# operator through `ConfigError`). An unused manifest entry is a false edge in
# the dependency graph, and nothing here would fail to compile without it.

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }

[lints]
workspace = true
```

Create `crates/psyche-cli/src/lib.rs` so both binaries and integration tests use the same implementation:

```rust
//! Everything `psyche` and `psyched` both do, in one place.
//!
//! The two binaries are two front doors onto one daemon. `psyche start`'s help
//! text says it starts the daemon in the foreground, and the only way for that
//! to keep being true is for it to run the code `psyched` runs — a second
//! implementation would drift, and the way it would drift is by quietly becoming
//! a stub that exits 0 having started nothing. The same argument covers
//! [`logging`]: two subscribers configured independently would eventually
//! disagree about the writer, and a daemon logging to stdout instead of stderr
//! is a corrupted `--json` pipeline.
//!
//! That rationale is why these modules are shared. It is not why they were once
//! shared with `#[path]` includes from two crate roots — that mechanism compiled
//! each file twice per build, kept doc-tests from ever running against them (a
//! binary target has none), and left them unreachable from `tests/`. A library
//! target costs nothing here: `publish = false` is set workspace-wide, so the
//! nominal public surface never reaches a registry.

pub mod daemon;
pub mod doctor;
pub mod logging;
pub mod status;

// The exit-code space, defined in one place because both binaries owe an
// operator the same one.
//
// An exit code is the most expensive contract a CLI has: it gets baked into a
// systemd `SuccessExitStatus=`, a Kubernetes probe, and a shell `||`, none of
// which are visible from this repository once shipped. Defining the space before
// the first release is the only cheap moment.
//
// The codes are `u8` rather than `ExitCode` values because `ExitCode::from` is
// not a `const fn`; callers convert at the return site. Tests compare against
// these constants, and `assert_cmd`'s `code()` wants an integer anyway.
//
// | code | meaning                                                    |
// |------|------------------------------------------------------------|
// | 0    | the command did what it was asked                          |
// | 1    | unexpected — deliberately unassigned, so it stays a signal |
// | 2    | usage; owned by clap, never returned from this crate       |
// | 3    | configuration is missing, unreadable, or invalid           |
// | 4    | a daemon was needed and could not be reached               |
// | 5    | a check ran and failed                                     |

/// Success. The command did what it was asked to do.
pub const EXIT_OK: u8 = 0;

/// Usage error: unknown flag, missing argument, bad subcommand.
///
/// Owned by clap, which exits `2` itself before any code here runs. Declared so
/// the space is documented in one place and so nothing else claims `2`.
pub const EXIT_USAGE: u8 = 2;

/// Configuration is missing, unreadable, too large, or invalid.
///
/// The distinction `doctor` exists to draw: this code means the file is wrong,
/// as opposed to [`EXIT_CHECK_FAILED`], which means the file was fine and the
/// environment it describes was not.
pub const EXIT_CONFIG: u8 = 3;

/// A running daemon was required and could not be reached.
///
/// Also what a subcommand returns when the build has no way to reach one at all
/// — an operator scripting `psyche stop && deploy` needs that to be a failure,
/// not a `0` that reads as "the daemon is gone".
pub const EXIT_UNAVAILABLE: u8 = 4;

/// A check ran to completion and reported a failure.
///
/// The configuration was readable and valid; something it describes is not in
/// the state it needs to be.
pub const EXIT_CHECK_FAILED: u8 = 5;
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p psyche-cli`
Expected: FAIL — `couldn't find bin psyche` / compilation error, since no source files exist yet.

- [ ] **Step 4: Write the logging installer**

Create `crates/psyche-cli/src/logging.rs`:

```rust
//! Structured JSON logs. Secret-bearing values never reach here as strings —
//! `psyche_core::secret::SecretRef` renders `<redacted>` through both `Debug`
//! and `Display`, so a field holding one cannot leak by accident.
//!
//! Logs go to stderr, never stdout. `psyche status --json` writes a document to
//! stdout that an operator is expected to pipe into a parser; interleaving log
//! lines there would corrupt it.
//!
//! Shared by both binaries — they link this module from the `psyche_cli`
//! library rather than carrying a copy each, so the writer and the format cannot
//! drift apart.

use tracing_subscriber::EnvFilter;

/// Environment variable holding the log filter directives.
pub const LOG_ENV: &str = "PSYCHE_LOG";

/// Filter used when [`LOG_ENV`] is absent or unusable.
pub const DEFAULT_FILTER: &str = "info";

/// Installs the process-wide subscriber, if one is not already installed.
///
/// `try_init`'s error is discarded on purpose: a second install attempt is not
/// a reason to refuse to run, and the failure mode — no logs — is visible.
///
/// A *malformed* filter is not the same thing as an absent one, and this used to
/// treat them identically: `try_from_env(..).unwrap_or_else(|_| info)` meant
/// `PSYCHE_LOG=trce` ran at info in silence, and the operator concluded that
/// trace logging was broken rather than that they had mistyped it. An absent
/// variable still falls back without comment — that is the ordinary case, not a
/// mistake.
///
/// The variable is read here rather than through `EnvFilter::try_from_env`
/// because that collapses "not present" and "does not parse" into one opaque
/// error type whose cause it does not expose.
///
/// This catches syntax, and only syntax. `PSYCHE_LOG=trce` — the obvious typo
/// for `trace` — *is* valid: a bare word is a target directive, so it enables a
/// target named `trce` and nothing else, and the process runs in total silence.
/// Measured, not assumed. Nothing here can distinguish that from a deliberate
/// target filter, and guessing which crate names an operator meant would be a
/// worse error than the one it prevented.
///
/// `eprintln!`, not `tracing::warn!`: the subscriber this function installs is
/// not up yet, so a `tracing` event at this point goes nowhere. stderr also
/// keeps it off the stdout stream that `--json` consumers parse.
pub fn install() {
    let filter = match std::env::var(LOG_ENV) {
        Ok(directives) => EnvFilter::try_new(&directives).unwrap_or_else(|e| {
            // The directives are operator-authored filter syntax, never
            // configuration content, so echoing them back is safe and is the
            // only way the operator learns which part they got wrong.
            eprintln!("{LOG_ENV} is not a valid filter ({e}); using {DEFAULT_FILTER:?}");
            EnvFilter::new(DEFAULT_FILTER)
        }),
        Err(std::env::VarError::NotPresent) => EnvFilter::new(DEFAULT_FILTER),
        // Set, but not readable as UTF-8. Distinct from absent for the same
        // reason a parse failure is: the operator meant something by it.
        Err(e) => {
            eprintln!("{LOG_ENV} could not be read ({e}); using {DEFAULT_FILTER:?}");
            EnvFilter::new(DEFAULT_FILTER)
        }
    };
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .with_writer(std::io::stderr)
        .try_init();
}
```

- [ ] **Step 5: Write the doctor checks**

Create `crates/psyche-cli/src/doctor.rs`:

```rust
//! Environment checks that must pass with no Telegram credentials present.
//!
//! Every string built here is printed verbatim to an operator's terminal and
//! routinely pasted into a bug report, so this module is the crate's output
//! surface. Two rules hold for everything it produces:
//!
//! 1. A [`Config`] is never rendered with `{:?}`. `Config`'s `Debug` redacts its
//!    extension table, so a dump would be safe today — but `doctor` names the
//!    fields it chose to print, so the guarantee does not depend on the `Debug`
//!    impl of a type this crate does not own.
//! 2. An extension value is never printed. Extension tables are untyped and a
//!    future one may hold a credential, so only the count is reported.
//!
//! Both are pinned by tests in `tests/cli.rs`, each shown to fail against a
//! `doctor` that violates it.
//!
//! A third rule joins them, and it is the reason this module takes a
//! `Result<&Config, &ConfigError>` rather than a `&Config`: **a check reports
//! what it observed, never what it assumes.** `doctor` used to call
//! `create_dir_all` and print "writable" on `Ok(())` — which that function
//! returns for a directory that already exists at mode 500 — so the one word an
//! operator ran the command to see was never verified. It also could not run at
//! all against a configuration that failed to load, which is the single case it
//! most exists for.

use std::path::Path;

use psyche_config::{Config, ConfigError};

/// What a check found.
///
/// Replaces a `bool`. Two of the entries below cannot fail — they report a path
/// and a table count — and living in a list where any `false` failed the command
/// made them look like assertions they were not. Splitting the outcomes also
/// gives the loader failure somewhere honest to put the checks it prevented from
/// running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Checked, and the thing checked is in the state it needs to be.
    Ok,
    /// Checked, usable, and worth an operator's attention anyway.
    Warn,
    /// Checked, and wrong. Any single one of these fails the command.
    Fail,
    /// Not a check. Reported because an operator reading a bug report wants it,
    /// and it can never fail.
    Info,
    /// Not run, because something it needed was unavailable.
    Skipped,
}

/// The wire spelling of a status: `ok`, `warn`, `fail`, `info`, `skipped`.
///
/// One spelling, used by both the text and the JSON rendering. The text form
/// previously shouted `FAIL` while the machine-readable form did not exist yet;
/// giving each its own vocabulary is how the two drift, and an operator who
/// greps for what they saw on their terminal then finds nothing in the JSON.
/// Visual salience is not worth a second definition of the same word.
impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Info => "info",
            Status::Skipped => "skipped",
        })
    }
}

/// One named check and the single line it contributes to `doctor` output.
#[derive(Debug)]
pub struct Check {
    /// Stable identifier an operator or a script can grep for.
    pub name: &'static str,
    /// What the check found. Any [`Status::Fail`] fails the command.
    pub status: Status,
    /// Operator-facing explanation. Built from named config fields only.
    pub detail: String,
}

/// Schema identifier on `doctor --json` output.
///
/// This repository versions its configuration (`psyche.config.v1`) and the Coven
/// API (`coven.daemon.v1`); its own machine-readable output is owed the same. The
/// alternative is the ad-hoc `name: status (detail)` line format, which someone
/// would `grep`, and which would then be frozen without anyone deciding to
/// freeze it.
pub const DOCTOR_SCHEMA: &str = "psyche.doctor.v1";

/// Writes and removes a file inside `dir`, creating `dir` if it is absent.
///
/// Returns whether `dir` already existed. `create_dir_all` alone proves nothing:
/// it returns `Ok(())` for a directory that exists at any mode, so a mode-500
/// `data_dir` reported "writable" and exited 0. The only way to learn that a
/// directory is writable is to write to it.
///
/// The distinction between created and pre-existing matters just as much: on a
/// typo'd path the old code silently *created* the directory and blessed it,
/// hiding the exact misconfiguration `doctor` exists to surface.
fn probe(dir: &Path) -> std::io::Result<bool> {
    let existed = dir.try_exists()?;
    std::fs::create_dir_all(dir)?;
    // A fixed name, not a random one: a probe file left behind by a killed
    // `doctor` should be overwritten by the next run rather than accumulating.
    let probe = dir.join(".psyche-doctor-probe");
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(probe)?;
    Ok(existed)
}

/// Every check here is local and credential-free. Reaching the Coven socket or
/// a Telegram API is explicitly *not* done — those belong to later gates.
///
/// Takes the load *result*, not a `Config`. `doctor` is dispatched before the
/// configuration is loaded precisely so it can report on a configuration that
/// does not load: the caller used to short-circuit on a bad file and print one
/// raw error with zero checks run, which made the `config` check below vacuous —
/// it could only ever report `ok`, because an invalid configuration never
/// reached it.
///
/// `path` is passed alongside because a `ConfigError` may or may not carry one,
/// and the operator's question is always "which file did you read".
pub fn run(path: &Path, config: Result<&Config, &ConfigError>) -> Vec<Check> {
    let config = match config {
        Ok(config) => config,
        Err(e) => return skipped_because_of(path, e),
    };

    let mut checks = vec![Check {
        name: "config",
        status: Status::Ok,
        // `schema_version()` is a method returning a `&'static str` const, not a
        // value read back from the file: a validated `Config` cannot hold any
        // other version, so this reports the build's contract, not user input.
        detail: format!("{} is schema {}", path.display(), config.schema_version()),
    }];

    let data_dir: &Path = config.data_dir.as_path();
    let (status, detail) = match probe(data_dir) {
        Ok(true) => (
            Status::Ok,
            format!("{} exists and is writable", data_dir.display()),
        ),
        // Created, and said so. `doctor` performing a side effect is defensible
        // — a first run on a fresh host should not fail for a directory it can
        // make — but doing it silently turns a typo'd path into a green line.
        Ok(false) => (
            Status::Warn,
            format!("{} created (did not exist)", data_dir.display()),
        ),
        Err(e) => (Status::Fail, format!("{}: {e}", data_dir.display())),
    };
    checks.push(Check {
        name: "data_dir",
        status,
        detail,
    });

    checks.push(Check {
        name: "coven_socket_path",
        // Info, not Ok: nothing was contacted, so there is nothing this could
        // have found. It was hardcoded `ok: true` in a list where any `!ok`
        // failed the command, which is a claim of verification it never did.
        status: Status::Info,
        detail: format!(
            "{} (not contacted at this gate)",
            config.coven.socket.display()
        ),
    });

    // Count only, and deliberately not `Extensions`' `Debug` — that renders
    // "<N key(s) redacted>", which reads to an operator as though something was
    // withheld from them rather than as a plain tally. The table names would
    // also be safe to print (they are operator-authored structure, which is why
    // psyche-config names them in its errors), but `Extensions` exposes no
    // iterator over its keys and this crate does not reach into another to add
    // one. Values are never in scope here under any API.
    checks.push(Check {
        name: "extensions",
        // Info for the same reason as the socket path: a tally is not a verdict.
        status: Status::Info,
        detail: format!(
            "{} table(s) present; contents not read",
            config.extensions.len()
        ),
    });

    checks
}

/// The report for a configuration that would not load: one real failure, and
/// every dependent check marked as not run.
///
/// Skipped rather than omitted. A shorter list would read as though those checks
/// had passed, and the operator would not learn that `doctor` still has nothing
/// to say about their `data_dir`.
fn skipped_because_of(path: &Path, error: &ConfigError) -> Vec<Check> {
    // `Display`, never `{:?}`: `ConfigError` reduces every deserializer error to
    // a payload-free message at one place inside psyche-config and holds no
    // `toml::de::Error`, whose own `Debug` would carry the entire configuration
    // file, secrets included. This rule is load-bearing across the project.
    let mut checks = vec![Check {
        name: "config",
        status: Status::Fail,
        detail: format!("{}: {error}", path.display()),
    }];
    for name in ["data_dir", "coven_socket_path", "extensions"] {
        checks.push(Check {
            name,
            status: Status::Skipped,
            detail: "configuration did not load".to_owned(),
        });
    }
    checks
}

/// How many checks failed. Anything above zero fails the command.
///
/// [`Status::Warn`] deliberately does not count: a `data_dir` this run created
/// is worth saying out loud and is not a reason for a health probe to go red.
#[must_use]
pub fn failures(checks: &[Check]) -> usize {
    checks.iter().filter(|c| c.status == Status::Fail).count()
}

/// The human rendering: one `name: status (detail)` line per check.
///
/// Returns a `String` rather than printing, so it can be asserted without
/// spawning a process.
#[must_use]
pub fn render_text(checks: &[Check]) -> String {
    let mut out = String::new();
    for check in checks {
        out.push_str(&format!(
            "{}: {} ({})\n",
            check.name, check.status, check.detail
        ));
    }
    out
}

/// The machine rendering: a versioned `psyche.doctor.v1` document.
///
/// `failed` is carried in the document as well as in the exit code, so a
/// consumer that already has the JSON does not have to also thread the status
/// through its shell.
#[must_use]
pub fn render_json(checks: &[Check]) -> String {
    let checks: Vec<serde_json::Value> = checks
        .iter()
        .map(|check| {
            serde_json::json!({
                "name": check.name,
                "status": check.status.to_string(),
                "detail": check.detail,
            })
        })
        .collect();
    serde_json::json!({
        "schema": DOCTOR_SCHEMA,
        "checks": checks,
        "failed": failures_in(&checks),
    })
    .to_string()
}

/// `failed` over already-rendered values, so [`render_json`] does not have to
/// hold both forms at once.
fn failures_in(checks: &[serde_json::Value]) -> usize {
    checks
        .iter()
        .filter(|c| c["status"] == serde_json::json!(Status::Fail.to_string()))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the `Result` rather than unwrapping: `clippy.toml` allows
    /// `unwrap` only in frames reachable from a `#[test]` fn, and a free helper
    /// is not one. The caller unwrapping also names the failing test.
    /// Renders a path as a TOML basic string, escaping what TOML gives meaning to.
    ///
    /// A Windows temp directory is `C:\Users\RUNNER~1\AppData\Local\Temp\...`,
    /// and interpolating that raw makes `\U` a unicode escape: the loader fails
    /// with "too few unicode value digits" and the test reads as though the
    /// configuration loader were broken. Nothing platform-specific here — the
    /// escaping is simply what writing a path into TOML has always required.
    fn toml_str(path: &Path) -> String {
        format!(
            "\"{}\"",
            path.display()
                .to_string()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        )
    }

    fn config_for(data_dir: &Path) -> Result<Config, ConfigError> {
        psyche_config::load_str(&format!(
            r#"
schema_version = "psyche.config.v1"
data_dir = {}

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#,
            toml_str(data_dir)
        ))
    }

    fn check<'a>(checks: &'a [Check], name: &str) -> Option<&'a Check> {
        checks.iter().find(|c| c.name == name)
    }

    /// A Windows path survives being written into a TOML fixture.
    ///
    /// Runs on every platform on purpose: the bug this pins is not conditional
    /// code, it is a string that means something different to the TOML parser,
    /// so a literal Windows path exercises it from macOS just as well. Without
    /// this, five `doctor` tests passed everywhere developers work and failed on
    /// `windows-latest` with "too few unicode value digits" — an error naming the
    /// config loader, which is not where the defect was.
    #[test]
    fn a_windows_style_path_round_trips_through_a_toml_fixture() {
        let windows = Path::new(r"C:\Users\RUNNER~1\AppData\Local\Temp\psyche");
        let rendered = toml_str(windows);
        assert_eq!(
            rendered, r#""C:\\Users\\RUNNER~1\\AppData\\Local\\Temp\\psyche""#,
            "every backslash must be escaped or TOML reads \\U as a unicode escape"
        );

        let config = psyche_config::load_str(&format!(
            r#"
schema_version = "psyche.config.v1"
data_dir = {rendered}

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#
        ))
        .expect("a path with backslashes must survive the fixture");
        assert_eq!(config.data_dir, windows);
    }

    /// The whole of `doctor`'s coverage used to be process spawns, because these
    /// modules were reachable only through two binary crate roots. Calling `run`
    /// directly is what the library target bought.
    #[test]
    fn every_check_is_named_and_explained() {
        let tmp = tempfile::tempdir().unwrap();
        let config = config_for(tmp.path()).unwrap();
        let checks = run(Path::new("psyche.toml"), Ok(&config));

        let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            ["config", "data_dir", "coven_socket_path", "extensions"],
            "the check list is the command's contract with a scripting operator"
        );
        for check in &checks {
            assert!(!check.detail.is_empty(), "{} has no detail", check.name);
        }
        assert_eq!(failures(&checks), 0);
    }

    /// An existing directory is reported as existing, and the word "writable" is
    /// earned by a write.
    #[test]
    fn an_existing_writable_data_dir_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let config = config_for(tmp.path()).unwrap();
        let checks = run(Path::new("psyche.toml"), Ok(&config));
        let data_dir = check(&checks, "data_dir").unwrap();

        assert_eq!(data_dir.status, Status::Ok, "{}", data_dir.detail);
        assert!(
            data_dir.detail.contains("exists and is writable"),
            "{}",
            data_dir.detail
        );
        // The probe cleans up after itself; a leftover file in an operator's
        // data directory is litter, and one that persisted would also make the
        // "did not exist" branch below unreachable on a second run.
        assert!(!tmp.path().join(".psyche-doctor-probe").exists());
    }

    /// A typo'd path used to be silently created and blessed with a green line.
    /// It is still created — failing a fresh host for a directory `doctor` can
    /// make would be worse — but the report says so.
    #[test]
    fn a_data_dir_that_did_not_exist_is_reported_as_created() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("typo").join("psyche");
        let config = config_for(&missing).unwrap();
        let checks = run(Path::new("psyche.toml"), Ok(&config));
        let data_dir = check(&checks, "data_dir").unwrap();

        assert_eq!(data_dir.status, Status::Warn, "{}", data_dir.detail);
        assert!(
            data_dir.detail.contains("did not exist"),
            "{}",
            data_dir.detail
        );
        // A warning is not a failure: a first run on a clean host must not exit
        // non-zero for a directory it successfully prepared.
        assert_eq!(failures(&checks), 0);
    }

    /// Mode 500: readable and traversable, not writable. `create_dir_all`
    /// returns `Ok(())` for it, which is exactly how "writable" came to be
    /// printed about a directory that is not.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_data_dir_fails() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o500)).unwrap();

        // Determined by trying, not by asking for a uid: reading the process uid
        // needs libc, and `unsafe_code` is forbidden workspace-wide. Root
        // ignores mode bits, so without this the test would pass by reporting
        // the opposite of what it claims to check.
        if std::fs::write(blocked.join(".root-check"), b"").is_ok() {
            eprintln!(
                "skipping an_unwritable_data_dir_fails: this process writes through mode 0o500, \
                 so it is root or on a filesystem that ignores mode bits"
            );
            return;
        }

        let config = config_for(&blocked).unwrap();
        let checks = run(Path::new("psyche.toml"), Ok(&config));
        let data_dir = check(&checks, "data_dir").unwrap();

        assert_eq!(data_dir.status, Status::Fail, "{}", data_dir.detail);
        assert!(data_dir.detail.contains(&blocked.display().to_string()));
        assert_eq!(failures(&checks), 1);

        // Restore, or the tempdir cannot be removed on drop.
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// The case `doctor` most exists for. It used to be the case `doctor`
    /// refused to run in.
    #[test]
    fn a_config_that_will_not_load_fails_that_check_and_skips_the_rest() {
        let error =
            psyche_config::load_str("schema_version = \"psyche.config.v99\"\n").unwrap_err();
        let checks = run(Path::new("/etc/psyche/psyche.toml"), Err(&error));

        let config = check(&checks, "config").unwrap();
        assert_eq!(config.status, Status::Fail);
        assert!(
            config.detail.contains("/etc/psyche/psyche.toml"),
            "{}",
            config.detail
        );
        assert!(
            config.detail.contains("psyche.config.v99"),
            "{}",
            config.detail
        );
        assert_eq!(failures(&checks), 1);

        // Skipped, not omitted: a shorter list reads as though these passed.
        for name in ["data_dir", "coven_socket_path", "extensions"] {
            assert_eq!(
                check(&checks, name).unwrap().status,
                Status::Skipped,
                "{name}"
            );
        }
    }

    /// The checks that cannot fail must not sit in the list looking like
    /// assertions. `coven_socket_path` was hardcoded `ok: true`.
    #[test]
    fn checks_that_verify_nothing_are_marked_info() {
        let tmp = tempfile::tempdir().unwrap();
        let config = config_for(tmp.path()).unwrap();
        let checks = run(Path::new("psyche.toml"), Ok(&config));

        for name in ["coven_socket_path", "extensions"] {
            assert_eq!(check(&checks, name).unwrap().status, Status::Info, "{name}");
        }
    }

    #[test]
    fn the_json_document_is_versioned_and_counts_failures() {
        let error =
            psyche_config::load_str("schema_version = \"psyche.config.v99\"\n").unwrap_err();
        let checks = run(Path::new("psyche.toml"), Err(&error));
        let document: serde_json::Value = serde_json::from_str(&render_json(&checks)).unwrap();

        assert_eq!(document["schema"], serde_json::json!(DOCTOR_SCHEMA));
        assert_eq!(document["failed"], serde_json::json!(1));
        assert_eq!(document["checks"].as_array().map(Vec::len), Some(4));
        assert_eq!(document["checks"][0]["name"], serde_json::json!("config"));
        assert_eq!(document["checks"][0]["status"], serde_json::json!("fail"));
        assert_eq!(
            document["checks"][1]["status"],
            serde_json::json!("skipped")
        );
    }

    /// One spelling per status, shared by both renderings. Pinned against the
    /// literals so renaming a variant cannot silently rename a word an operator
    /// greps for.
    #[test]
    fn statuses_render_as_their_wire_spellings() {
        assert_eq!(Status::Ok.to_string(), "ok");
        assert_eq!(Status::Warn.to_string(), "warn");
        assert_eq!(Status::Fail.to_string(), "fail");
        assert_eq!(Status::Info.to_string(), "info");
        assert_eq!(Status::Skipped.to_string(), "skipped");
    }

    #[test]
    fn the_text_rendering_names_every_check_and_its_status() {
        let tmp = tempfile::tempdir().unwrap();
        let config = config_for(tmp.path()).unwrap();
        let rendered = render_text(&run(Path::new("psyche.toml"), Ok(&config)));

        assert!(rendered.contains("config: ok ("), "{rendered}");
        assert!(rendered.contains("data_dir: ok ("), "{rendered}");
        assert!(rendered.contains("coven_socket_path: info ("), "{rendered}");
        assert_eq!(rendered.lines().count(), 4, "{rendered}");
    }
}
```

Create `crates/psyche-cli/src/status.rs` for the versioned observed/unobserved status contract:

```rust
//! What `psyche status` may say, and what it may not.
//!
//! `status` runs in a different process from the daemon and this build has no
//! IPC, so it cannot see a running `psyched`. The previous document said
//! `{"state":"stopped","observed":false}` — a false statement on any host where
//! a daemon *is* running, and `jq -r .state` is what a consumer actually writes.
//! An `observed` flag next to a populated `state` is an invitation to read the
//! state and ignore the flag.
//!
//! So the caveat is structural rather than advisory: [`Observation`] can hold a
//! state or a reason, never both, and the rendering below cannot emit a `state`
//! it was not given. A consumer that learns to trust a bare `state` field, and is
//! told about the caveat in a later release, has already shipped the code that
//! ignores it.

use psyche_runtime::LifecycleState;

/// Schema identifier on `status --json` output.
///
/// Versioned like `psyche.config.v1` and `coven.daemon.v1`. This repository
/// treats schema versioning as first-class for its configuration and for the
/// Coven API; its own machine-readable output is owed the same, and the moment
/// to add the envelope is before anyone parses a document without one.
pub const STATUS_SCHEMA: &str = "psyche.status.v1";

/// Why no state was observed.
///
/// A closed vocabulary, and deliberately a Rust enum rather than a free string:
/// a consumer branching on `reason` needs the set to be enumerable, and a
/// `format!` at the call site is how it stops being.
///
/// `NoIpc` is the only variant this build can produce, because there is no IPC
/// at all yet. The IPC work extends this with the distinctions that only exist
/// once there is a socket to fail against:
///
/// - `socket-absent` — the configured path does not exist; nothing is running.
/// - `connect-refused` — the path exists but nothing is listening, which is a
///   stale socket rather than an absent daemon.
/// - `permission-denied` — a daemon may well be running; this caller cannot ask.
///
/// They are named here rather than added now because a variant nothing
/// constructs implies a distinction the build cannot actually draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unobserved {
    /// This build has no daemon IPC, so no state can be read from anywhere.
    NoIpc,
}

/// The wire spelling of a reason. One definition, used by both renderings.
impl std::fmt::Display for Unobserved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Unobserved::NoIpc => "no-ipc",
        })
    }
}

/// What `status` managed to find out.
///
/// The invariant "`state` is populated only when `observed` is true" lives in
/// this type rather than in the code that builds the document, so there is no
/// way to write a renderer that violates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// A daemon was reached and reported this state.
    ///
    /// Unreachable in this build. It exists so that the IPC work adds a call
    /// site rather than a field, and so the renderings below are already written
    /// for the answer they will eventually have to give.
    Observed(LifecycleState),
    /// No state was read, for this reason.
    Unobserved(Unobserved),
}

/// The machine rendering: a versioned `psyche.status.v1` document.
///
/// `state` is `null` rather than absent when nothing was observed — a consumer
/// distinguishing "no state" from "no such field" should not have to — and
/// `reason` is present only in that case, because a reason alongside an answer
/// would be a reason for nothing.
#[must_use]
pub fn render_json(observation: &Observation) -> String {
    // The state's spelling comes from `LifecycleState`'s `Display`, never from a
    // literal here, so the wire word has exactly one definition.
    let (state, reason) = match observation {
        Observation::Observed(state) => (
            serde_json::Value::String(state.to_string()),
            serde_json::Value::Null,
        ),
        Observation::Unobserved(reason) => (
            serde_json::Value::Null,
            serde_json::Value::String(reason.to_string()),
        ),
    };
    serde_json::json!({
        "schema": STATUS_SCHEMA,
        "observed": matches!(observation, Observation::Observed(_)),
        "state": state,
        "reason": reason,
    })
    .to_string()
}

/// The human rendering, carrying the same caveat.
///
/// Names no state when none was observed. The previous line led with
/// `state: stopped (not observed: ...)`, and an operator skimming for a state
/// word found one.
#[must_use]
pub fn render_text(observation: &Observation) -> String {
    match observation {
        Observation::Observed(state) => format!("state: {state}\n"),
        Observation::Unobserved(reason) => {
            format!("state: not observed ({reason}: no daemon IPC in this build)\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unobserved_status_carries_a_null_state_and_a_reason() {
        let document: serde_json::Value =
            serde_json::from_str(&render_json(&Observation::Unobserved(Unobserved::NoIpc)))
                .unwrap();

        assert_eq!(document["schema"], serde_json::json!(STATUS_SCHEMA));
        assert_eq!(document["observed"], serde_json::json!(false));
        assert_eq!(document["state"], serde_json::Value::Null);
        assert_eq!(document["reason"], serde_json::json!("no-ipc"));
    }

    /// The shape the IPC work will produce. Asserted now so that adding the call
    /// site later is not also a change to the document.
    #[test]
    fn an_observed_status_carries_the_state_and_no_reason() {
        let document: serde_json::Value = serde_json::from_str(&render_json(
            &Observation::Observed(LifecycleState::Draining),
        ))
        .unwrap();

        assert_eq!(document["observed"], serde_json::json!(true));
        assert_eq!(document["state"], serde_json::json!("draining"));
        assert_eq!(document["reason"], serde_json::Value::Null);
    }

    /// The two renderings must agree about how much the command knows. The text
    /// form previously said `stopped` while the JSON form said `observed: false`.
    #[test]
    fn the_text_rendering_names_no_state_it_did_not_observe() {
        let rendered = render_text(&Observation::Unobserved(Unobserved::NoIpc));
        assert!(rendered.contains("not observed"), "{rendered}");
        assert!(rendered.contains("no-ipc"), "{rendered}");
        for state in ["running", "draining", "stopped"] {
            assert!(!rendered.contains(state), "{rendered}");
        }
    }

    #[test]
    fn reasons_render_as_their_wire_spellings() {
        assert_eq!(Unobserved::NoIpc.to_string(), "no-ipc");
    }
}
```

**The output surface is this task's security responsibility.**

`doctor` and `status` render text an operator reads and pastes into issues. Two
rules, both testable:

1. **Never render a `Config` with `{:?}`.** `Config`'s `Debug` redacts
   `extensions`, so it is safe today — but `doctor` should print named fields it
   has chosen, not a struct dump, so the guarantee does not depend on a type it
   does not own.
2. **Never print an extension value.** Extension tables are untyped and a future
   one may hold a secret. `doctor` reports counts and names, never values.

Both are pinned by `doctor_output_never_contains_an_extension_value` in Step 8.
Follow the mutation rule this project now uses: a test asserting an absence must
be shown to fail when the absence is violated, or it proves nothing.

- [ ] **Step 6: Write the `psyche` entry point**

Create `crates/psyche-cli/src/main.rs`:

```rust
//! `psyche` — the operator-facing command line.
//!
//! Argument parsing and dispatch only; the work lives in the `psyche_cli`
//! library, which `psyched` links too.
//!
//! Nothing here reaches the network or reads a credential: every subcommand in
//! this slice is local, so `psyche doctor` is usable on a machine that has never
//! been given a Telegram token. See [`psyche_cli::doctor`] for the rules
//! governing what may be printed.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
// From the `psyche_cli` library target, which `psyched` also links. See that
// crate root for why the daemon path and the log subscriber are shared rather
// than reimplemented per binary.
use psyche_cli::{
    EXIT_CHECK_FAILED, EXIT_CONFIG, EXIT_OK, EXIT_UNAVAILABLE, daemon, doctor, logging, status,
};

#[derive(Debug, Parser)]
#[command(name = "psyche", version, about = "Psyche familiar runtime")]
struct Cli {
    /// Configuration file. Resolution order: --config, $PSYCHE_CONFIG,
    /// ./psyche.toml.
    ///
    /// Accepted on either side of the subcommand.
    ///
    /// The default is relative to the working directory, which a systemd system
    /// unit leaves at `/`. Set $PSYCHE_CONFIG or pass --config in a unit file or
    /// a container, or the path resolves to /psyche.toml.
    //
    // Doc comment above is operator-facing and reaches `--help` verbatim, so the
    // rationale lives down here instead:
    //
    // `global` collapses what were four identical per-subcommand declarations
    // and the four-arm match that existed only to pull the value back out of
    // them, and makes `psyche --config X status` parse — it reads as the natural
    // order and used to be a usage error. Backwards compatible either way.
    //
    // Deliberately no XDG or `/etc` lookup: deferred to the packaging work, not
    // rejected.
    #[arg(
        long,
        global = true,
        env = "PSYCHE_CONFIG",
        default_value = "psyche.toml"
    )]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the daemon in the foreground. Equivalent to running `psyched`.
    Start {
        /// Start, then immediately shut down. Used by tests and smoke checks so
        /// the full lifecycle runs without needing a signal.
        #[arg(long)]
        shutdown_after_start: bool,
    },
    /// Not implemented in this build: there is no daemon IPC yet.
    ///
    /// The help text says so because the command cannot do it. It used to
    /// promise a graceful shutdown and exit 0 having done nothing, which is what
    /// `psyche stop && deploy` reads as success.
    Stop,
    /// Report daemon state. This build cannot observe one — there is no daemon
    /// IPC — so it reports why instead.
    Status {
        /// Emit a `psyche.status.v1` document on stdout instead of a line of
        /// prose.
        #[arg(long)]
        json: bool,
    },
    /// Run local, credential-free environment checks.
    Doctor {
        /// Emit a `psyche.doctor.v1` document on stdout instead of one line per
        /// check.
        ///
        /// The line format is what an operator would otherwise `grep`, and a
        /// format that gets grepped is frozen whether or not anyone decided to
        /// freeze it.
        #[arg(long)]
        json: bool,
    },
}

/// Loads the configuration, runs every check against whatever came back, prints
/// the report, and returns the code that describes it.
///
/// The load result is passed into [`doctor::run`] rather than unwrapped here:
/// a configuration that will not load is the case `doctor` most exists for, and
/// the command has to have something to say about it beyond one raw error.
///
/// Three outcomes, three codes. `EXIT_CONFIG` means the file is wrong;
/// `EXIT_CHECK_FAILED` means the file was fine and something it describes is
/// not. An operator scripting this could not previously tell them apart.
fn doctor_command(path: &std::path::Path, json: bool) -> ExitCode {
    let loaded = psyche_config::load_path(path);
    let checks = doctor::run(path, loaded.as_ref());

    // stdout: the report *is* this command's output, and `doctor > report.txt`
    // has to capture all of it. The failure reason travels inside the `config`
    // check rather than being duplicated onto stderr.
    print!(
        "{}",
        if json {
            doctor::render_json(&checks) + "\n"
        } else {
            doctor::render_text(&checks)
        }
    );

    if loaded.is_err() {
        ExitCode::from(EXIT_CONFIG)
    } else if doctor::failures(&checks) > 0 {
        ExitCode::from(EXIT_CHECK_FAILED)
    } else {
        ExitCode::from(EXIT_OK)
    }
}

/// `unwrap`/`expect` are denied outside tests, so every failure path here
/// returns a named exit code after rendering the error with `Display`. The codes
/// are defined and documented in [`psyche_cli`]; nothing here invents one.
///
/// `Display`, never `{:?}`: `psyche_config::ConfigError` reduces every
/// deserializer error to a payload-free message at one place inside that crate,
/// and holds no `toml::de::Error` — whose own `Debug` would carry the entire
/// configuration file, secrets included.
///
/// `#[tokio::main]` on the whole binary rather than an executor built inside the
/// `Start` arm, so `psyche start` and `psyched` run the daemon on an identically
/// configured one. Two independently built executors are the same drift risk
/// that makes `daemon.rs` and `logging.rs` shared files. The cost is that
/// `doctor` and `status` construct an executor they never use.
#[tokio::main]
async fn main() -> ExitCode {
    logging::install();
    let cli = Cli::parse();

    let path = cli.config;

    // `doctor` is dispatched *before* the load, and is the only subcommand that
    // is. Loading first short-circuited it on exactly the input it exists to
    // explain: `psyche doctor --config /nope.toml` printed one raw `ConfigError`
    // and exited with zero checks run, which also left the `config` check
    // vacuous — it could only ever report `ok`.
    //
    // Taken out of `cli.command` here rather than matched again below, so the
    // match that follows has no `Doctor` arm to write. A dead arm returning a
    // code nothing can observe is the same kind of defect as a command that
    // reports a state it did not measure.
    let command = match cli.command {
        Command::Doctor { json } => return doctor_command(&path, json),
        other => other,
    };

    let config = match psyche_config::load_path(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_CONFIG);
        }
    };

    // The path is argv, not configuration content. `?config` is deliberately not
    // recorded: `Config`'s `Debug` redacts today, but this crate does not print
    // structs it does not own.
    tracing::debug!(path = %path.display(), "configuration loaded");

    match command {
        // Returned by the match above, which is the only way `command` is
        // bound. Exhaustiveness still demands the arm; `unreachable!` is the
        // honest thing to put in it, and it names the reason so the next reader
        // does not try to write a test that reaches here.
        Command::Doctor { .. } => {
            unreachable!("doctor is dispatched before the configuration load")
        }
        Command::Status { json } => {
            // Nothing was observed, and this build has no way to observe
            // anything: `status` runs in a different process from the daemon and
            // there is no IPC. Which is why it reports a *reason* rather than a
            // state — see `psyche_cli::status` for why the caveat is structural.
            let observation = status::Observation::Unobserved(status::Unobserved::NoIpc);
            print!(
                "{}",
                if json {
                    status::render_json(&observation) + "\n"
                } else {
                    status::render_text(&observation)
                }
            );
            ExitCode::from(EXIT_OK)
        }
        Command::Start {
            shutdown_after_start,
            ..
        } => daemon::run(config, shutdown_after_start).await,
        Command::Stop => {
            // Non-zero, because nothing was stopped. The previous form printed
            // "no running daemon to stop" and exited 0, which a rolling restart
            // scripted as `psyche stop && deploy` reads as "the old daemon is
            // gone". It is not gone; this build has no way to ask it.
            eprintln!("stop is not implemented in this build (no daemon IPC)");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}
```

Create `crates/psyche-cli/src/daemon.rs` as the one run path shared by `psyche start` and `psyched`:

```rust
//! The daemon run path, shared by `psyched` and `psyche start`.
//!
//! One function, called from two binaries, for the reason `logging.rs` is also
//! shared: `psyche start`'s help text says it starts the daemon in the
//! foreground, and the only way for that to keep being true is for it to run the
//! same code `psyched` runs. A second implementation would drift, and the way it
//! would drift is by quietly becoming a stub that exits 0 having started
//! nothing.

use std::io;
use std::process::ExitCode;

use psyche_config::Config;
use psyche_runtime::Runtime;

/// The signals that mean "shut down", with their handlers already installed.
///
/// Holding them in a value is the point: on Unix a handler exists from the
/// moment [`Signals::install`] returns, not from the first await. That is what
/// lets [`run`] install *before* [`Runtime::start`], so a signal arriving during
/// startup is queued rather than killing the process at its default
/// disposition — which for SIGTERM means no drain and, once `start` opens the
/// data directory and binds the Coven socket, a leaked socket and lease.
#[cfg(unix)]
struct Signals {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    /// Installs handlers for SIGTERM and SIGINT.
    ///
    /// Both, not just SIGINT: `tokio::signal::ctrl_c` covers SIGINT alone, so a
    /// daemon relying on it leaves SIGTERM at its default disposition — and
    /// SIGTERM is what `systemctl stop`, `docker stop`, a Kubernetes eviction
    /// and a bare `kill` all send. The drain was unreachable in production
    /// before this existed.
    fn install() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};

        Ok(Self {
            term: signal(SignalKind::terminate())?,
            int: signal(SignalKind::interrupt())?,
        })
    }

    /// Resolves when either signal is delivered.
    ///
    /// Which one it was is deliberately not reported: there is exactly one
    /// shutdown path, and a caller cannot act on the distinction.
    async fn wait(&mut self) {
        tokio::select! {
            _ = self.term.recv() => {}
            _ = self.int.recv() => {}
        }
    }
}

/// Windows shim. Task 8 ships Windows binaries through npm, and this file is on
/// that build's path even though a Windows service is not a supported
/// deployment yet.
///
/// Unlike the Unix form this cannot install anything eagerly — `ctrl_c`
/// registers its handler at first await — so `install` succeeding here means
/// only "nothing to do", not "a handler is armed". The startup window that the
/// Unix ordering closes therefore stays open on Windows; it is called out here
/// rather than papered over.
#[cfg(not(unix))]
struct Signals {
    _private: (),
}

#[cfg(not(unix))]
impl Signals {
    /// Infallible: there is nothing to install until the first await.
    fn install() -> io::Result<Self> {
        Ok(Self { _private: () })
    }

    /// Resolves on Ctrl-C, or immediately if the handler cannot be installed.
    ///
    /// An install failure here is treated as a shutdown request rather than as
    /// a hang: the alternative is a daemon that can never be stopped except by
    /// killing it, which is strictly worse than one that exits early and says
    /// nothing was wrong with the drain.
    async fn wait(&mut self) {
        if let Err(e) = tokio::signal::ctrl_c().await {
            eprintln!("failed to install signal handler: {e}");
        }
    }
}

/// Brings a runtime up, waits for a shutdown signal, then takes the graceful
/// path down.
///
/// Takes an already-loaded [`Config`] rather than a path: the caller has one in
/// hand, and re-reading the file here would both duplicate the error rendering
/// and leave a window in which the daemon runs a different configuration from
/// the one its caller validated.
///
/// Ordering is load-bearing. Handlers are installed first, the runtime is
/// started second, and only then is the signal awaited. Installing lazily at the
/// await — which is what `ctrl_c().await` does — leaves every signal delivered
/// during startup at its default disposition.
///
/// When installation fails there is deliberately no runtime yet, so the honest
/// answer is to report the failure and exit without starting anything. The
/// previous form installed after `start` and returned failure while a `Running`
/// runtime went out of scope undrained.
pub async fn run(config: Config, shutdown_after_start: bool) -> ExitCode {
    // `--shutdown-after-start` skips installation entirely: the flag exists so
    // the full lifecycle can run without a signal, and arming handlers that
    // nothing will ever wait on would only change what the process does to
    // signals it was not asked to handle.
    let signals = if shutdown_after_start {
        None
    } else {
        match Signals::install() {
            Ok(signals) => Some(signals),
            Err(e) => {
                eprintln!("failed to install signal handlers: {e}");
                return ExitCode::FAILURE;
            }
        }
    };

    let runtime = match Runtime::start(config).await {
        Ok(runtime) => runtime,
        // Unreachable, and permanently so: `RuntimeError` has no variants, so
        // this arm cannot be constructed and cannot be covered by a test. It is
        // written out anyway because the signature is what absorbs the first
        // real startup failure — see the type's docs in psyche-runtime.
        Err(e) => {
            // Display, not `{:?}` — see the note on `psyche`'s `main`.
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    // Read back through `config()` because `start` consumed the `Config`. The
    // data directory is the field operators most often have pointed somewhere
    // they did not mean, and it is a path, never a value from an extension
    // table.
    tracing::info!(
        data_dir = %runtime.config().data_dir.display(),
        "psyche daemon ready"
    );

    if let Some(mut signals) = signals {
        signals.wait().await;
    }

    match runtime.shutdown().await {
        Ok(()) => ExitCode::SUCCESS,
        // Unreachable for the same reason as the arm above.
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 7: Write the `psyched` entry point**

Create `crates/psyche-cli/src/bin/psyched.rs`:

```rust
//! `psyched` — the Psyche daemon, in the foreground.
//!
//! Runs until interrupted, then takes the one graceful shutdown path
//! `psyche_runtime::Runtime` offers. There is no forced exit: a caller wanting
//! one terminates the process.
//!
//! A thin wrapper: argument parsing, configuration loading, then the shared run
//! path in [`psyche_cli::daemon`]. `psyche start` calls the same function, so the
//! two cannot come to mean different things.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
// Linked from the `psyche_cli` library rather than copied or `#[path]`-included:
// two subscribers configured independently would eventually disagree about the
// writer, and a daemon logging to stdout instead of stderr is a corrupted
// `--json` pipeline.
use psyche_cli::{EXIT_CONFIG, daemon, logging};

#[derive(Debug, Parser)]
#[command(
    name = "psyched",
    version,
    about = "Psyche daemon, in the foreground. Equivalent to `psyche start`."
)]
struct Cli {
    /// Configuration file. Resolution order: --config, $PSYCHE_CONFIG,
    /// ./psyche.toml.
    ///
    /// The default is relative to the working directory, which a systemd system
    /// unit leaves at `/`. Set $PSYCHE_CONFIG or pass --config in a unit file or
    /// a container, or the path resolves to /psyche.toml.
    //
    // Must match `psyche`'s, flag for flag — an operator who learns one and
    // writes the other into a unit file is entitled to have it work. Asserted by
    // `psyche_start_and_psyched_accept_the_same_flags`.
    #[arg(long, env = "PSYCHE_CONFIG", default_value = "psyche.toml")]
    config: PathBuf,
    /// Start, then immediately shut down. Used by tests and smoke checks so the
    /// full lifecycle runs without needing a signal.
    #[arg(long)]
    shutdown_after_start: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    logging::install();

    let cli = Cli::parse();
    let config = match psyche_config::load_path(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            // Display, not `{:?}` — see the note on `psyche`'s `main`. The code
            // is the same one `psyche` returns for the same file: an operator's
            // unit file must not have to know which binary it invoked.
            eprintln!("{e}");
            return ExitCode::from(EXIT_CONFIG);
        }
    };

    daemon::run(config, cli.shutdown_after_start).await
}
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p psyche-cli`
Expected: library `14 passed; 0 failed`, integration tests `21 passed; 0 failed`.

- [ ] **Step 9: Run the whole workspace**

Run: `cargo test --workspace --locked --all-features`
Expected: every suite `ok`, `0 failed` overall.

- [ ] **Step 10: Commit**

```bash
git add crates/psyche-cli
git commit -m "feat(cli): add psyche/psyched with credential-free doctor and status"
```

---

## Task 7: CI gate

**Files:**
- Create: `deny.toml`
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Configure dependency policy**

Create `deny.toml`:

```toml
[advisories]
yanked = "deny"

[licenses]
allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Unicode-3.0", "Zlib"]
confidence-threshold = 0.9

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

- [ ] **Step 2: Verify the policy passes locally before CI depends on it**

Run:

```bash
cargo +stable install cargo-deny --locked --version 0.19.8
cargo deny check licenses advisories bans sources
```

Expected: `licenses ok`, `advisories ok`, `bans ok`, `sources ok`. If a transitive crate uses a licence outside the allowlist, add it to `allow` **only** if it is permissive — do not add a copyleft licence to make the build pass.

- [ ] **Step 3: Write the workflow**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  rust:
    name: Rust checks (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4
      # Pinned to the MSRV rather than `@stable`, and stated explicitly rather
      # than left to the action's default: `rust-toolchain.toml` already pins
      # 1.85.0 and takes precedence over whatever cargo is invoked through, so
      # `@stable` would download a toolchain the build then never uses — a
      # slower job that silently tests nothing about the version we ship.
      # The two pins must agree; Step 4 is what catches it if they drift.
      #
      # Verified: this action runs `rustup default <toolchain>` and does NOT
      # export RUSTUP_TOOLCHAIN, so `rust-toolchain.toml` really does win. That
      # is why the pin here is documentation rather than mechanism — and why the
      # `supply-chain` job below has to escape it explicitly.
      - uses: dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8  # stable @ 2026-05-20
        with:
          toolchain: "1.85.0"
          components: rustfmt, clippy
      # `RUSTFLAGS` is part of this action's cache key (it hashes every env var
      # whose name starts with CARGO/CC/CFLAGS/CXX/CMAKE/RUST). Setting it at
      # workflow scope, above, is what keeps that key constant across runs; a
      # per-step `RUSTFLAGS` would be invisible here and the restored cache
      # would have been built with different flags.
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: Tests
        run: cargo test --workspace --locked

  supply-chain:
    name: Dependency audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # NOT the MSRV pin, unlike the `rust` job. cargo-deny is a tool we run
      # against the tree, not something we ship, so its build toolchain says
      # nothing about what we support. It also cannot use the pin: cargo-deny
      # 0.19.8 declares rust-version 1.88.0, and `cargo install` under 1.85.0
      # refuses outright —
      #   error: cannot install package `cargo-deny`, it requires rustc 1.88.0
      #   or newer, while the currently active rustc version is 1.85.0
      # No `components`: this job never runs fmt or clippy.
      - uses: dtolnay/rust-toolchain@stable
      # `+stable` is load-bearing, not decoration. `rust-toolchain.toml` sits at
      # the repo root and outranks `rustup default`, so a bare `cargo install`
      # here would run under 1.85.0 and hit the error above no matter which
      # toolchain the step above installed. An explicit `+toolchain` is the one
      # thing that outranks the toolchain file.
      #
      # Installed directly rather than via a third-party action: it is the same
      # binary and the same command engineers run locally in Task 7 Step 2, so
      # there is no CI-only path, and it adds no extra action to trust. Pinned
      # to the version that policy was actually validated against, so a new
      # cargo-deny release cannot turn CI red with no change to this repo.
      - name: Install cargo-deny
        run: cargo +stable install cargo-deny --locked --version 0.19.8
      - name: Audit
        run: cargo deny check licenses advisories bans sources

  secrets:
    name: Secret guard
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      # The gitleaks CLI, not `gitleaks/gitleaks-action`. The action is
      # separately licensed: its entrypoint looks up the repository owner and,
      # when the owner is an Organization, exits 1 unless a `GITLEAKS_LICENSE`
      # secret is present. `OpenCoven` is an Organization, so the action form
      # would fail every run until someone buys a key. The CLI it wraps is MIT
      # and has no such gate.
      #
      # This is the same reasoning the `supply-chain` job applies to cargo-deny:
      # run the real binary, with the same arguments an engineer runs locally,
      # rather than a wrapper. Version and digest are pinned because a job whose
      # whole purpose is supply-chain hygiene should not curl an unverified
      # tarball.
      - name: Install gitleaks
        env:
          GITLEAKS_VERSION: 8.30.1
          GITLEAKS_SHA256: 551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
        run: |
          set -euo pipefail
          archive="gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz"
          curl --proto '=https' --tlsv1.2 --retry 3 --location --silent --show-error --fail \
            --output "$archive" \
            "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/${archive}"
          echo "${GITLEAKS_SHA256}  ${archive}" | sha256sum --check --strict -
          tar -xzf "$archive" gitleaks
          install -m 0755 gitleaks /usr/local/bin/gitleaks
          rm -f "$archive" gitleaks
          gitleaks version
      # `--log-opts=--all` scans every commit on every ref, which is what the
      # `fetch-depth: 0` above is for: a secret that was committed and then
      # reverted is still a leaked secret. `--redact` keeps the finding out of
      # the public log, which would otherwise re-leak whatever it found.
      - name: Scan history
        run: gitleaks detect --no-banner --redact --log-opts="--all"

# The npm distribution job is added by Task 8, which creates
# `packages/psyche-npm`. Adding it here would leave CI red for a whole task,
# for the same reason a workspace member cannot be declared before it exists.
```

- [ ] **Step 4: Prove the gate locally**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo test --workspace --locked && \
cargo deny check licenses advisories bans sources
```

Expected: all four succeed with the terminal evidence listed in "Verification commands".

- [ ] **Step 5: Commit**

```bash
git add deny.toml .github/workflows/ci.yml
git commit -S -m "ci: add fmt, clippy, locked tests, dependency audit, and secret guard"
```

---

## Task 8: npm distribution with checksum verification

**Files:**
- Create: `packages/psyche-npm/package.json`
- Create: `packages/psyche-npm/scripts/verify-checksum.js`
- Create: `packages/psyche-npm/scripts/resolve-binary.js`
- Create: `packages/psyche-npm/bin/psyche.js`
- Create: `packages/psyche-npm/test/verify-checksum.test.js`
- Create: `packages/psyche-npm/test/signal-forwarding.test.js`
- Create: `packages/psyche-npm/README.md`

- [ ] **Step 1: Write the failing test**

Create `packages/psyche-npm/test/verify-checksum.test.js`:

```javascript
const { test } = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');

const { verifyChecksum, resolvePackageName, SUPPORTED } = require('../scripts/verify-checksum.js');
const manifest = require('../package.json');

function tempFile(contents) {
  const p = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'psyche-')), 'bin');
  fs.writeFileSync(p, contents);
  return p;
}

test('accepts a binary whose digest matches', () => {
  const file = tempFile('pretend-binary');
  const digest = crypto.createHash('sha256').update('pretend-binary').digest('hex');
  assert.doesNotThrow(() => verifyChecksum(file, digest));
});

test('rejects a substituted binary', () => {
  const file = tempFile('tampered');
  const wrong = crypto.createHash('sha256').update('original').digest('hex');
  assert.throws(() => verifyChecksum(file, wrong), /checksum mismatch/);
});

test('rejects a missing binary rather than exec-ing nothing', () => {
  assert.throws(() => verifyChecksum('/nonexistent/psyche', 'deadbeef'), /not found/);
});

test('maps platform and arch to the companion package name', () => {
  assert.strictEqual(resolvePackageName('darwin', 'arm64'), '@opencoven/psyche-darwin-arm64');
  assert.strictEqual(resolvePackageName('linux', 'x64'), '@opencoven/psyche-linux-x64');
  assert.throws(() => resolvePackageName('sunos', 'sparc'), /unsupported platform/);
});

// --- the shipped manifest ----------------------------------------------------

const ZERO_DIGEST = '0'.repeat(64);

test('the placeholder digests fail closed rather than disabling the check', () => {
  // No release artifact exists at this gate, so no real digest can be recorded
  // and every entry is all-zero. That is a *closed* door, not an open one: a
  // SHA-256 of all zero bytes is not the digest of any file, so `verifyChecksum`
  // rejects every real binary against it. The release job that builds the
  // companion packages is what replaces these. If this test ever has to change,
  // the change is "the digests are real now" — never "the check is off".
  const shipped = Object.entries(manifest.psyche.checksums);
  assert.ok(shipped.length > 0);
  for (const [key, digest] of shipped) {
    assert.strictEqual(digest, ZERO_DIGEST, `${key} should still be a placeholder`);
  }

  // The property the placeholder relies on, asserted rather than assumed.
  const file = tempFile('any real binary at all');
  assert.throws(() => verifyChecksum(file, ZERO_DIGEST), /checksum mismatch/);
  // Empty files included: sha256("") is e3b0c442..., not zeros.
  const empty = tempFile('');
  assert.throws(() => verifyChecksum(empty, ZERO_DIGEST), /checksum mismatch/);
});

test('every supported platform has a manifest entry and a companion dependency', () => {
  // A platform in SUPPORTED but absent from the manifest resolves far enough to
  // pass `resolvePackageName` and then dies on "no recorded checksum" — at the
  // user's terminal, not here. Keep the three lists in lockstep.
  const platforms = [...SUPPORTED].sort();
  assert.deepStrictEqual(Object.keys(manifest.psyche.checksums).sort(), platforms);
  assert.deepStrictEqual(
    Object.keys(manifest.optionalDependencies).sort(),
    platforms.map((p) => `@opencoven/psyche-${p}`)
  );
});

// --- the wrapper's own logic -------------------------------------------------
//
// `bin/psyche.js` holds the entire user-facing path: platform resolution,
// checksum lookup, package resolution, spawn, and exit-code mapping. Left in the
// bin entry point it is reachable only by spawning the whole wrapper, which
// needs the companion packages installed — so in practice it would ship with no
// coverage at all. `resolveBinary` and `exitCodeFor` are therefore exported from
// a module and the bin file is a shim over them. The companion-package lookup is
// injected so these run on a machine where nothing has been published.

const { resolveBinary, exitCodeFor } = require('../scripts/resolve-binary.js');

const MANIFEST = {
  psyche: { checksums: { 'linux-x64': 'abc', 'darwin-arm64': 'def' } },
};

test('resolves the binary inside the companion package', () => {
  const found = resolveBinary('linux', 'x64', MANIFEST, () => '/pkgs/psyche-linux-x64/package.json');
  assert.strictEqual(found.binary, path.join('/pkgs/psyche-linux-x64', 'bin', 'psyche'));
  assert.strictEqual(found.expected, 'abc');
});

test('appends .exe only on Windows', () => {
  const manifest = { psyche: { checksums: { 'win32-x64': 'ghi' } } };
  const found = resolveBinary('win32', 'x64', manifest, () => '/pkgs/psyche-win32-x64/package.json');
  assert.ok(found.binary.endsWith('psyche.exe'));
});

/** A resolver that fails the way Node's really does, carrying `code`. */
function failsWith(code) {
  return () => {
    const err = new Error(`stand-in for ${code}`);
    err.code = code;
    throw err;
  };
}

test('names the missing companion package rather than surfacing MODULE_NOT_FOUND', () => {
  // The bare `require.resolve` failure reads
  // "Cannot find module '@opencoven/psyche-linux-x64/package.json'", which tells
  // an operator nothing about what to do. npm skips optional dependencies whose
  // platform does not match, so this is the *expected* state on an unsupported
  // host, not an exotic one.
  //
  // The stand-in sets `code`, not just a message: that is the field Node
  // populates and the field `resolveBinary` branches on, so a fake that only
  // matched the text would pass while proving nothing.
  assert.throws(
    () => resolveBinary('linux', 'x64', MANIFEST, failsWith('MODULE_NOT_FOUND')),
    /@opencoven\/psyche-linux-x64.*not installed/s
  );
});

test('does not tell an operator to reinstall a package that is already installed', () => {
  // A companion package declaring `exports` without a `"./package.json"` entry
  // is present and resolvable, yet `require.resolve('<pkg>/package.json')` fails
  // with ERR_PACKAGE_PATH_NOT_EXPORTED. Reporting "not installed" there sends
  // the operator round a reinstall loop that can never succeed, so absence and
  // misconfiguration must not collapse into one message.
  assert.throws(
    () => resolveBinary('linux', 'x64', MANIFEST, failsWith('ERR_PACKAGE_PATH_NOT_EXPORTED')),
    (err) =>
      /is installed but its package.json could not be resolved/.test(err.message) &&
      /exports/.test(err.message) &&
      !/not installed\./.test(err.message)
  );
});

test('refuses a platform with no recorded checksum', () => {
  assert.throws(
    () => resolveBinary('darwin', 'x64', MANIFEST, () => '/pkgs/x/package.json'),
    /no recorded checksum/
  );
});

test('reports a signal death as 128 + signal, not as success or a bare 1', () => {
  // `spawnSync` sets status to null when the child is killed by a signal. A
  // wrapper that maps that to 1 tells `psyche start; echo $?` the daemon exited
  // with a generic error when it was actually SIGKILLed — and a wrapper that
  // maps it to 0 is worse. 128+n is what a shell reports for the same death.
  assert.strictEqual(exitCodeFor({ status: 0, signal: null }), 0);
  assert.strictEqual(exitCodeFor({ status: 3, signal: null }), 3);
  assert.strictEqual(exitCodeFor({ status: null, signal: 'SIGKILL' }), 137);
  assert.strictEqual(exitCodeFor({ status: null, signal: 'SIGTERM' }), 143);
  // Neither status nor signal: spawn itself failed.
  assert.strictEqual(exitCodeFor({ status: null, signal: null }), 1);
});
```

Create `packages/psyche-npm/test/signal-forwarding.test.js`:

```javascript
// The wrapper must not swallow a shutdown signal.
//
// Each case stages its own wrapper, manifest, helpers, and companion package.
// Nothing mutates the source package: Node runs test files concurrently by
// default, so rewriting ../package.json races the checksum tests and makes the
// suite nondeterministic.

const { test } = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');

const PACKAGE_ROOT = path.join(__dirname, '..');

/** Poll until `predicate` holds, or fail the test rather than hang. */
async function waitFor(predicate, description, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.fail(`timed out after ${timeoutMs}ms waiting for ${description}`);
}

/**
 * Builds a fully isolated wrapper tree and companion package. The copied
 * package.json carries the fake binary's real digest, so verification passes
 * without changing the package the concurrent checksum tests imported.
 */
function stageWrapper(script) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'psyche-sig-'));
  const wrapperRoot = path.join(root, 'wrapper');
  const wrapperBin = path.join(wrapperRoot, 'bin');
  const wrapperScripts = path.join(wrapperRoot, 'scripts');
  fs.mkdirSync(wrapperBin, { recursive: true });
  fs.mkdirSync(wrapperScripts, { recursive: true });

  fs.copyFileSync(path.join(PACKAGE_ROOT, 'bin', 'psyche.js'), path.join(wrapperBin, 'psyche.js'));
  for (const helper of ['resolve-binary.js', 'verify-checksum.js']) {
    fs.copyFileSync(path.join(PACKAGE_ROOT, 'scripts', helper), path.join(wrapperScripts, helper));
  }

  const key = `${process.platform}-${process.arch}`;
  const companionBin = path.join(
    wrapperRoot,
    'node_modules',
    '@opencoven',
    `psyche-${key}`,
    'bin',
  );
  fs.mkdirSync(companionBin, { recursive: true });
  const binary = path.join(companionBin, process.platform === 'win32' ? 'psyche.exe' : 'psyche');
  fs.writeFileSync(binary, script, { mode: 0o755 });
  fs.writeFileSync(
    path.join(companionBin, '..', 'package.json'),
    JSON.stringify({ name: `@opencoven/psyche-${key}`, version: '0.0.0' }),
  );

  const digest = crypto.createHash('sha256').update(script).digest('hex');
  const manifest = JSON.parse(fs.readFileSync(path.join(PACKAGE_ROOT, 'package.json'), 'utf8'));
  manifest.psyche.checksums[key] = digest;
  fs.writeFileSync(path.join(wrapperRoot, 'package.json'), JSON.stringify(manifest, null, 2) + '\n');

  return {
    wrapper: path.join(wrapperBin, 'psyche.js'),
    log: path.join(root, 'child.log'),
  };
}

// `SIGQUIT` rather than a second SIGTERM: using two signals proves the handler
// forwards the signal it received instead of hardcoding one.
for (const signal of ['SIGTERM', 'SIGQUIT']) {
  test(`forwards ${signal} to the daemon instead of orphaning it`, { skip: process.platform === 'win32' && 'POSIX signals' }, async () => {
    const script = `#!/bin/sh
trap 'echo GOT-${signal} >> "$PSYCHE_TEST_LOG"; exit 0' ${signal.slice(3)}
echo UP >> "$PSYCHE_TEST_LOG"
while true; do sleep 0.1; done
`;
    const staged = stageWrapper(script);
    const child = spawn(process.execPath, [staged.wrapper], {
      env: { ...process.env, PSYCHE_TEST_LOG: staged.log },
      stdio: 'ignore',
    });

    const logged = () => (fs.existsSync(staged.log) ? fs.readFileSync(staged.log, 'utf8') : '');
    await waitFor(() => logged().includes('UP'), 'the stand-in binary to start');
    child.kill(signal);
    await waitFor(() => logged().includes(`GOT-${signal}`), `the child to receive ${signal}`);
    await waitFor(() => child.exitCode !== null || child.signalCode !== null, 'the wrapper to exit');
  });
}

test('reports a spawn failure instead of exiting 1 in silence', { skip: process.platform === 'win32' && 'shebang-less exec' }, () => {
  const staged = stageWrapper('\x7fELF not really an executable');
  const result = spawnSync(process.execPath, [staged.wrapper], { encoding: 'utf8' });
  assert.notStrictEqual(result.status, 0, 'a binary that cannot exec must not report success');
  assert.notStrictEqual(result.stderr.trim(), '', `exit ${result.status} with no explanation`);
});
```

- [ ] **Step 2: Run the test to verify it fails**

The package manifest does not exist yet, so invoke the checksum test directly;
the complete npm command becomes available after Step 5 creates `package.json`.

Run: `node --test packages/psyche-npm/test/verify-checksum.test.js`
Expected: FAIL — `Cannot find module '../scripts/verify-checksum.js'`.

- [ ] **Step 3: Write the checksum helper**

Create `packages/psyche-npm/scripts/verify-checksum.js`:

```javascript
'use strict';

const fs = require('node:fs');
const crypto = require('node:crypto');

const SUPPORTED = new Set([
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-x64',
]);

/** Maps process.platform/process.arch to the companion package that ships the binary. */
function resolvePackageName(platform, arch) {
  const key = `${platform}-${arch}`;
  if (!SUPPORTED.has(key)) {
    throw new Error(`unsupported platform: ${key}`);
  }
  return `@opencoven/psyche-${key}`;
}

/**
 * Refuses to hand back a binary that is missing or whose digest does not match
 * the manifest. This is the only integrity check between npm and exec.
 */
function verifyChecksum(binaryPath, expectedSha256) {
  if (!fs.existsSync(binaryPath)) {
    throw new Error(`psyche binary not found at ${binaryPath}`);
  }
  const actual = crypto
    .createHash('sha256')
    .update(fs.readFileSync(binaryPath))
    .digest('hex');
  if (actual !== expectedSha256) {
    throw new Error(
      `psyche binary checksum mismatch at ${binaryPath}: expected ${expectedSha256}, found ${actual}`
    );
  }
  return binaryPath;
}

module.exports = { resolvePackageName, verifyChecksum, SUPPORTED };
```

- [ ] **Step 4: Write the resolution helper**

Everything the wrapper does apart from spawning the child lives here, so it can
be unit-tested without the companion packages being installed anywhere.

Create `packages/psyche-npm/scripts/resolve-binary.js`:

```javascript
'use strict';

const path = require('node:path');
const { resolvePackageName } = require('./verify-checksum.js');

// `os.constants.signals` rather than a hand-written table: the numbers differ
// across platforms and a wrong constant here produces a plausible but incorrect
// exit code, which is the hardest kind of bug to notice.
const { signals } = require('node:os').constants;

/**
 * Locates the platform binary and the digest it must match.
 *
 * `resolvePkgJson` is injected — defaulting to `require.resolve` — so tests can
 * exercise this on a machine where no companion package has been published.
 */
function resolveBinary(platform, arch, manifest, resolvePkgJson = (id) => require.resolve(id)) {
  const pkg = resolvePackageName(platform, arch);
  const key = `${platform}-${arch}`;

  const expected = manifest.psyche && manifest.psyche.checksums[key];
  if (!expected) {
    throw new Error(`no recorded checksum for ${key}`);
  }

  let pkgJson;
  try {
    pkgJson = resolvePkgJson(`${pkg}/package.json`);
  } catch (cause) {
    // Only absence means "not installed". A package that declares `exports`
    // without a `"./package.json"` entry is present and resolvable yet fails
    // here with ERR_PACKAGE_PATH_NOT_EXPORTED, and telling that operator to
    // reinstall sends them round a loop that cannot terminate. Companion
    // packages must therefore omit `exports` or export `./package.json`; this
    // branch is what makes a violation legible instead of misleading.
    if (cause && cause.code !== 'MODULE_NOT_FOUND') {
      throw new Error(
        `${pkg} is installed but its package.json could not be resolved ` +
          `(${cause.code ?? 'unknown error'}). A companion package must not ` +
          `hide package.json behind an "exports" map.`,
        { cause }
      );
    }
    // npm silently skips an optional dependency whose platform does not match,
    // so this is the ordinary state on an unsupported host. Say which package is
    // missing and that reinstalling is the fix; `MODULE_NOT_FOUND` says neither.
    throw new Error(
      `${pkg} is not installed. It ships the ${key} binary and is an optional ` +
        `dependency of @opencoven/psyche; reinstall without --no-optional.`,
      { cause }
    );
  }

  const binaryName = platform === 'win32' ? 'psyche.exe' : 'psyche';
  return { binary: path.join(path.dirname(pkgJson), 'bin', binaryName), expected };
}

/**
 * Signals this wrapper forwards to the daemon.
 *
 * Ctrl-C already reaches the child without any of this — a tty signals the whole
 * foreground process group — which is exactly why the gap was easy to miss. A
 * directed `kill` from a supervisor hits the wrapper alone, so forwarding is the
 * only thing that lets `psyched`'s SIGTERM handling run at all.
 *
 * Windows has no real signals; Node emulates SIGINT, SIGBREAK, SIGHUP and
 * SIGTERM on top of console events, and listening for a signal it does not
 * emulate throws. SIGUSR1 and SIGUSR2 are deliberately absent everywhere: Node
 * reserves SIGUSR1 to start its inspector, and taking it over would silently
 * disable that for anyone debugging the wrapper.
 */
const FORWARDED_SIGNALS =
  process.platform === 'win32'
    ? ['SIGINT', 'SIGTERM', 'SIGBREAK']
    : ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'];

/**
 * Maps a child's `(status, signal)` outcome to the exit code this process
 * should report. Shaped for `child.on('close')`, and identical to what
 * `spawnSync` returns, so the contract did not change when the wrapper moved off
 * the synchronous spawn.
 *
 * A child killed by a signal has `status === null`; reporting 1 for that claims
 * the daemon chose to fail when it was actually killed, and reporting 0 would
 * claim it succeeded. 128+n is what a shell reports for the same death.
 */
function exitCodeFor(result) {
  if (result.status !== null && result.status !== undefined) {
    return result.status;
  }
  if (result.signal) {
    return 128 + (signals[result.signal] ?? 0);
  }
  return 1; // spawn itself failed
}

module.exports = { resolveBinary, exitCodeFor, FORWARDED_SIGNALS };
```

- [ ] **Step 5: Write the wrapper and manifest**

`bin/psyche.js` is deliberately a shim: every branch worth testing was moved into
the two modules above, so what remains is the one thing a unit test cannot cover
anyway — actually spawning the binary.

Create `packages/psyche-npm/bin/psyche.js`:

```javascript
#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const { verifyChecksum } = require('../scripts/verify-checksum.js');
const { resolveBinary, exitCodeFor, FORWARDED_SIGNALS } = require('../scripts/resolve-binary.js');

// The wrapper resolves and execs a Rust binary. It holds no daemon, storage,
// identity, policy, or transport logic — that boundary is fixed by PLAN.md W2.
function main() {
  const { binary, expected } = resolveBinary(
    process.platform,
    process.arch,
    require('../package.json')
  );

  // Verified immediately before exec. This is a check, not a guarantee: nothing
  // stops the file being replaced between the digest and the spawn. On Linux
  // `open()` plus exec of `/proc/self/fd/N` would close the window; there is no
  // portable equivalent, so the window is documented rather than claimed shut.
  //
  // Note also what this check is *for*: the expected digest ships inside this
  // package, so it authenticates the companion binary against a claim this
  // wrapper makes — not against a compromised wrapper. It is a substitution
  // check, not a signature.
  verifyChecksum(binary, expected);

  // `spawn`, never `spawnSync`. A synchronous spawn blocks the event loop inside
  // waitpid, so a `process.on('SIGTERM')` handler is JavaScript that cannot run
  // until the child has already exited — the wrapper dies, the daemon is
  // reparented to init, and it never drains. Verified: under `spawnSync` a
  // `kill -TERM` of the wrapper leaves the child alive with PPID 1 and its TERM
  // trap unfired.
  //
  // That failure is invisible interactively, which is what makes it dangerous:
  // Ctrl-C appears to work because the tty signals the whole foreground process
  // group and the child hears it directly, with the wrapper playing no part.
  // Every non-interactive supervisor — `docker stop`, `systemctl stop`,
  // supervisord, a Kubernetes preStop hook — signals the wrapper PID alone, and
  // psyched's SIGTERM handling is unreachable through a synchronous wrapper.
  //
  // `stdio: 'inherit'` so the daemon's stderr reaches the operator's terminal
  // unbuffered, and so `psyche status --json | jq` still works through here.
  const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });

  // Forwarded rather than relied upon: the process-group delivery that makes
  // Ctrl-C work does not happen for a directed kill.
  for (const signal of FORWARDED_SIGNALS) {
    process.on(signal, () => {
      child.kill(signal);
    });
  }

  // `spawn` reports exec failure through this event rather than by throwing, so
  // without it the user gets a bare exit 1 and no output. Reachable past a
  // passing checksum: a companion package shipping a wrong-architecture binary
  // matches its recorded digest and then fails at exec.
  child.on('error', (err) => {
    console.error(err.message);
    process.exit(1);
  });

  // `close`, not `exit`: it fires after the inherited stdio streams are done, so
  // the daemon's final lines cannot be truncated by this process exiting first.
  child.on('close', (status, signal) => {
    process.exit(exitCodeFor({ status, signal }));
  });
}

try {
  main();
} catch (err) {
  console.error(err.message);
  process.exit(1);
}
```

Create `packages/psyche-npm/package.json`:

```json
{
  "name": "@opencoven/psyche",
  "version": "0.0.0",
  "description": "Psyche familiar runtime",
  "license": "MIT",
  "repository": { "type": "git", "url": "git+https://github.com/OpenCoven/psyche.git" },
  "bin": { "psyche": "bin/psyche.js" },
  "files": ["bin/", "scripts/", "README.md"],
  "engines": { "node": ">=20" },
  "scripts": {
    "test": "node --test"
  },
  "optionalDependencies": {
    "@opencoven/psyche-darwin-arm64": "0.0.0",
    "@opencoven/psyche-darwin-x64": "0.0.0",
    "@opencoven/psyche-linux-arm64": "0.0.0",
    "@opencoven/psyche-linux-x64": "0.0.0",
    "@opencoven/psyche-win32-x64": "0.0.0"
  },
  "psyche": {
    "checksums": {
      "darwin-arm64": "0000000000000000000000000000000000000000000000000000000000000000",
      "darwin-x64": "0000000000000000000000000000000000000000000000000000000000000000",
      "linux-arm64": "0000000000000000000000000000000000000000000000000000000000000000",
      "linux-x64": "0000000000000000000000000000000000000000000000000000000000000000",
      "win32-x64": "0000000000000000000000000000000000000000000000000000000000000000"
    }
  }
}
```

The test script is bare `node --test`, **not** `node --test test/`. Passing a
directory fails on Node 24 with `Cannot find module '<abs>/test'` — a
`MODULE_NOT_FOUND` that reads as a broken `require` inside the suite rather than
as a wrong invocation, so it costs real time to diagnose. Bare `node --test`
auto-discovers `**/*.test.js`, excludes `node_modules`, and works from Node 18.13
through 24. Verified on v24.18.1: directory form fails, bare form reports
`pass 15`.

The all-zero digests are correct at this gate: no release artifact exists yet, so no real digest can be recorded. Publication is gated at **G12**, and the release job that builds the companion packages is what replaces them. A zero digest fails closed — `verifyChecksum` rejects any real binary against it.

Create `packages/psyche-npm/README.md`:

```markdown
# @opencoven/psyche

Thin wrapper that resolves the platform-specific Psyche binary, verifies its
SHA-256 against the recorded manifest, and execs it.

This package contains no daemon, storage, identity, graph, policy,
verification, or surface transport logic. That boundary is fixed by the Psyche
program plan and is not negotiable per-package.

Not yet published. Publication is gated at G12.
```

- [ ] **Step 6: Run the complete test suite to verify it passes**

The checksum tests import `package.json`, and the signal-forwarding tests stage
copies of both `package.json` and `bin/psyche.js`, so run the complete suite only
after Step 5 has created those files.

Run: `npm --prefix packages/psyche-npm test`
Expected: `tests 15`, `pass 15`, `fail 0` under Node's default test concurrency.

- [ ] **Step 7: Verify packing includes no binary**

Run: `npm pack ./packages/psyche-npm --dry-run`

Positional, not `--prefix`: `pack` ignores the prefix and reads `package.json`
from the working directory, so `npm --prefix packages/psyche-npm pack --dry-run`
fails with `ENOENT ... /package.json` naming the *wrong* directory. `--dry-run`
writes no tarball.

Expected: exactly five files — `bin/psyche.js`,
`scripts/verify-checksum.js`, `scripts/resolve-binary.js`, `package.json`,
`README.md` — and **no** `.node`, `.exe`, or platform binary. Roughly 4.3 kB
packed, well under 10 kB.

Note the `test/` directory must *not* appear: `files` lists `bin/`, `scripts/`,
and `README.md` only, so tests ship in the repository and not to consumers.

- [ ] **Step 8: Add the npm job to CI**

Task 7 deliberately left this out, because the package did not exist yet. Append
to `.github/workflows/ci.yml`:

```yaml
  npm:
    name: npm distribution
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '22'
      - name: Wrapper tests
        run: npm --prefix packages/psyche-npm test
      # Positional, not `--prefix`: `pack` ignores the prefix and reads
      # package.json from the working directory.
      - name: Pack dry run
        run: npm pack ./packages/psyche-npm --dry-run
```

- [ ] **Step 9: Commit**

```bash
git add packages/psyche-npm .github/workflows/ci.yml
git commit -m "feat(dist): add @opencoven/psyche wrapper with checksum verification"
```

---

## Task 9: Release gate and merge evidence

**Files:**
- None (verification and reporting only)

- [ ] **Step 1: Run the complete local gate**

Run every command in "Verification commands" above, in order.
Expected: the terminal evidence recorded there, with no failures and no skipped step.

- [ ] **Step 2: Open the pull request**

```bash
gh pr create --repo OpenCoven/psyche \
  --title "feat: bootstrap Psyche Rust workspace, daemon, CLI, and distribution" \
  --body "Implements the approved W2 bootstrap child plan for coven-psy1. Scope: psyche-core, psyche-config, psyche-runtime, psyche-cli, CI gate, and npm dry-run distribution. No Telegram, Coven execution, store, graph, or identity behaviour. No capability flag is set. Publication remains gated at G12; production child dispatch remains unauthorized pending G6."
```

Expected: a PR URL. Do **not** enable auto-merge.

**Squash-merge this branch.** Review fixes are applied as follow-up commits rather
than amends (so each reviewed state stays inspectable), which means intermediate
commits can be individually non-building — for example, the commit that adds the
first crate still declares four workspace members and does not compile until the
next commit narrows the list. Squashing collapses that into one buildable commit
and keeps `main` bisectable. Rebase-merging would preserve the broken states.

- [ ] **Step 3: Stop at the review gate**

PLAN.md §6.9 requires stopping at approval gates and §7 requires review threads to reach terminal state before merge. **Do not merge.** Report the PR URL and CI status, then stop.

- [ ] **Step 4: Record merge evidence after a human merges**

Once merged, and only then:

```bash
cd ~/Documents/GitHub/OpenCoven/coven
SHA=$(gh pr view <PR_NUMBER> --repo OpenCoven/psyche --json mergeCommit --jq '.mergeCommit.oid')
bd note coven-psy1 "W2 bootstrap merged at $SHA. Scope: workspace, config, runtime, CLI, CI, npm dry-run. G2 not claimed: schemas, migrations, fakes, and state-machine/property/crash tests remain in the follow-on plan. Production child dispatch remains unauthorized."
```

Expected: `✓ Note added to coven-psy1`. Leave `coven-psy1` **open** — its acceptance is met, but W2's G2 exit is not, and the follow-on plan attaches here.

---

## Follow-on plan (not this unit)

A second child plan covers the remainder of W2 and is what actually reaches **G2**:

- `psyche-store` — SQLite migrations, transactions, leases, retention.
- Canonical schemas for intent, graph, node, and attempt records.
- Fake Coven and fake surface boundaries with no adapter-only relaxation.
- State-machine and property tests over lifecycle transitions.
- Crash and restart tests proving no lost or double-adopted work.

That plan must not begin until this one merges — it builds on the workspace, config loader, and lifecycle seam created here.
