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
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-features
cargo deny check licenses advisories bans sources
gitleaks detect --no-banner --redact --log-opts="--all"
npm --prefix packages/psyche-npm test
npm --prefix packages/psyche-npm pack --dry-run
```

**Expected terminal evidence:** `cargo fmt` prints nothing and exits 0. `clippy` prints `Finished` with no warnings. `cargo test` reports every suite `ok` with `0 failed`. `cargo deny` prints `advisories ok`, `bans ok`, `licenses ok`, `sources ok`. `gitleaks` prints `no leaks found`. `npm test` reports `pass 4  fail 0`. `npm pack --dry-run` lists `bin/psyche.js`, `scripts/verify-checksum.js`, `package.json`, `README.md` and no `.node`/`.wasm` binary.

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

`ConfigError` therefore does **not** hold one and has no `#[from]` for it. The
deserializer error is reduced to a fixed, payload-free variant at exactly one
place, `reduce_toml_error`. Do not retain even `toml::de::Error::message()`:
serde diagnostics may embed the rejected scalar. Task 3 made its rule greppable
through a single `expose_reference` accessor; this is the same move, so review
can grep one name instead of auditing every `?`.

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
use std::path::{Path, PathBuf};

use psyche_core::schema::{ensure_schema_version, SchemaError};
use serde::Deserialize;

/// A parsed `psyche.config.v1` document.
///
/// No `Eq`: `extensions` is a `toml::Table` whose values include `Float(f64)`,
/// so `toml::Value` derives only `PartialEq`. No derived `Debug` either — see
/// the manual impl below.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Must be `psyche.config.v1`; any other value is denied.
    pub schema_version: String,
    /// Directory owning local Psyche state.
    pub data_dir: PathBuf,
    /// Coven daemon connection settings.
    pub coven: CovenConfig,
    /// The only place unknown keys are tolerated, and only under an explicitly
    /// versioned table.
    #[serde(default)]
    pub extensions: toml::Table,
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

/// Errors from loading configuration.
///
/// `Parse` deliberately does **not** hold a [`toml::de::Error`], and there is no
/// `#[from]` for one. That type's `Display` renders the offending source line
/// verbatim and its `Debug` carries `input: Some(<the entire file>)`, so holding
/// one would leave every secret in the file a single `?err` away from a log. The
/// deserializer error is reduced to a payload-free form at exactly one place —
/// [`reduce_toml_error`] — which is what review should grep for.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file is not valid TOML, or violates the strict schema.
    #[error("configuration is not valid TOML or violates the strict schema")]
    Parse,
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
}

// Manual `Debug`, not derived. `extensions` holds arbitrary untyped
// `toml::Value`, so a derived impl would print whatever is in it — including a
// secret placed there by a future extension — on `tracing::debug!(?config)`
// after a *successful* load. `reduce_toml_error` guards the failure path; this
// guards the success path, which is the one more often logged.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("schema_version", &self.schema_version)
            .field("data_dir", &self.data_dir)
            .field("coven", &self.coven)
            .field(
                "extensions",
                &format_args!("<{} key(s) redacted>", self.extensions.len()),
            )
            .finish()
    }
}

/// The single conversion from a deserializer error into a payload-free one.
///
/// Taking ownership and discarding the error is deliberate. Both the complete
/// error and its message can carry input values. Keeping this in one function
/// means a review can grep `reduce_toml_error` to find every place a TOML error
/// crosses the boundary, rather than auditing every `?`.
fn reduce_toml_error(_: toml::de::Error) -> ConfigError {
    ConfigError::Parse
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
schema_version = "psyche.config.v1"
data_dir = "/var/lib/psyche"

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#;

    #[test]
    fn loads_a_valid_config() {
        let cfg = load_str(VALID).unwrap();
        assert_eq!(cfg.schema_version, "psyche.config.v1");
        assert_eq!(cfg.data_dir, PathBuf::from("/var/lib/psyche"));
        assert_eq!(cfg.coven.required_api_version, "coven.daemon.v1");
        assert!(cfg.extensions.is_empty());
    }

    #[test]
    fn rejects_an_unknown_top_level_field() {
        let sentinel = "must-never-appear-in-an-error";
        let raw = VALID.replacen(
            "\n[coven]",
            &format!("\ntelegram_token = \"{sentinel}\"\n\n[coven]"),
            1,
        );
        let err = load_str(&raw).unwrap_err();
        assert!(
            matches!(err, ConfigError::Parse),
            "expected a parse error, got {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "configuration is not valid TOML or violates the strict schema"
        );
        assert!(!format!("{err:?}").contains(sentinel));
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
        let raw = format!("{VALID}\n[extensions.\"psyche.experiment.v1\"]\nenabled = true\n");
        let cfg = load_str(&raw).unwrap();
        assert!(cfg.extensions.contains_key("psyche.experiment.v1"));
    }

    #[test]
    fn debug_does_not_print_extension_values() {
        let sentinel = "must-never-appear-in-debug";
        let raw = format!(
            "{VALID}\n[extensions.\"psyche.experiment.v1\"]\nvalue = \"{sentinel}\"\n"
        );
        let cfg = load_str(&raw).unwrap();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("value"), "{rendered}");
        assert!(!rendered.contains(sentinel), "{rendered}");
        assert!(rendered.contains("1 key(s) redacted"), "{rendered}");
    }

    #[test]
    fn missing_file_reports_the_path() {
        let err = load_path(Path::new("/nonexistent/psyche.toml")).unwrap_err();
        assert!(err.to_string().contains("/nonexistent/psyche.toml"));
    }
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
/// Probe used only to read `schema_version`. It intentionally does *not* deny
/// unknown fields, so a future config can be version-checked before its unknown
/// fields are reported.
#[derive(Deserialize)]
struct VersionProbe {
    schema_version: String,
}

pub fn load_str(raw: &str) -> Result<Config, ConfigError> {
    let probe: VersionProbe = toml::from_str(raw).map_err(reduce_toml_error)?;
    ensure_schema_version(&probe.schema_version)?;
    let config: Config = toml::from_str(raw).map_err(reduce_toml_error)?;
    Ok(config)
}

pub fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    load_str(&raw)
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

Raw secret values are never valid configuration. Secrets are named by reference
(for example `op://VAULT/ITEM/token`); a literal value is rejected at parse
time.

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

- [ ] **Step 1: Write the failing test**

Create `crates/psyche-runtime/src/lib.rs`:

```rust
//! Composition root. Owns the daemon lifecycle and the only shutdown path.

use std::sync::{Arc, Mutex};

use psyche_config::Config;

/// Graceful shutdown stops intake, then drains, then exits. `Draining` is
/// observable so `psyche status` can distinguish it from `Running`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Running,
    Draining,
    Stopped,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime already stopped")]
    AlreadyStopped,
}

pub struct Runtime {
    state: Arc<Mutex<LifecycleState>>,
    transitions: Arc<Mutex<Vec<LifecycleState>>>,
    #[allow(dead_code)]
    config: Config,
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
        let rt = Runtime::start(test_config()).await;
        assert_eq!(rt.state(), LifecycleState::Running);
    }

    #[tokio::test]
    async fn shutdown_drains_then_stops_in_order() {
        let rt = Runtime::start(test_config()).await;
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

    #[tokio::test]
    async fn second_shutdown_is_an_error_not_a_panic() {
        let rt = Runtime::start(test_config()).await;
        rt.shutdown().await.unwrap();
        let err = rt.shutdown().await.unwrap_err();
        assert!(matches!(err, RuntimeError::AlreadyStopped));
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
psyche-core = { workspace = true }
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
impl Runtime {
    pub async fn start(config: Config) -> Self {
        tracing::info!(state = "running", "psyche runtime started");
        Self {
            state: Arc::new(Mutex::new(LifecycleState::Running)),
            transitions: Arc::new(Mutex::new(vec![LifecycleState::Running])),
            config,
        }
    }

    pub fn state(&self) -> LifecycleState {
        *self.state.lock().expect("lifecycle mutex poisoned")
    }

    pub fn transitions(&self) -> Vec<LifecycleState> {
        self.transitions
            .lock()
            .expect("transition mutex poisoned")
            .clone()
    }

    fn set(&self, next: LifecycleState) {
        *self.state.lock().expect("lifecycle mutex poisoned") = next;
        self.transitions
            .lock()
            .expect("transition mutex poisoned")
            .push(next);
        tracing::info!(state = ?next, "psyche lifecycle transition");
    }

    /// Stops intake, drains in-flight work, then exits. There is no forced
    /// path — a caller wanting immediate exit terminates the process.
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        if self.state() == LifecycleState::Stopped {
            return Err(RuntimeError::AlreadyStopped);
        }
        self.set(LifecycleState::Draining);
        // Nothing durable is in flight in this slice; the drain point exists so
        // the store and lease work in the follow-on G2 plan has a seam to use.
        self.set(LifecycleState::Stopped);
        Ok(())
    }
}
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
- Create: `crates/psyche-cli/src/main.rs`
- Create: `crates/psyche-cli/src/doctor.rs`
- Create: `crates/psyche-cli/src/logging.rs`
- Create: `crates/psyche-cli/src/bin/psyched.rs`
- Create: `crates/psyche-cli/tests/cli.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/psyche-cli/tests/cli.rs`:

```rust
use assert_cmd::Command;
use predicates::str::contains;

fn write_config(dir: &std::path::Path) -> std::path::PathBuf {
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let path = dir.join("psyche.toml");
    std::fs::write(
        &path,
        format!(
            r#"
schema_version = "psyche.config.v1"
data_dir = "{}"

[coven]
socket = "/run/coven.sock"
required_api_version = "coven.daemon.v1"
"#,
            data_dir.display()
        ),
    )
    .unwrap();
    path
}

#[test]
fn doctor_succeeds_without_any_telegram_credentials() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path());
    Command::cargo_bin("psyche")
        .unwrap()
        .env_remove("TELEGRAM_BOT_TOKEN")
        .env_remove("PSYCHE_TELEGRAM_TOKEN")
        .args(["doctor", "--config", config.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("config: ok").and(contains("data_dir: ok")));
}

#[test]
fn status_reports_stopped_when_no_daemon_is_running() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path());
    Command::cargo_bin("psyche")
        .unwrap()
        .args(["status", "--config", config.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(contains("\"state\":\"stopped\""));
}

#[test]
fn doctor_fails_clearly_on_an_unsupported_schema_version() {
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
        .failure()
        .stderr(contains("unsupported schema_version").and(contains("psyche.config.v99")));
}

#[test]
fn start_and_stop_run_without_any_telegram_credentials() {
    // coven-psy1 acceptance requires all four subcommands to run with no
    // credentials present, not just doctor and status.
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path());
    for subcommand in ["start", "stop"] {
        Command::cargo_bin("psyche")
            .unwrap()
            .env_remove("TELEGRAM_BOT_TOKEN")
            .env_remove("PSYCHE_TELEGRAM_TOKEN")
            .args([subcommand, "--config", config.to_str().unwrap()])
            .assert()
            .success();
    }
}

#[test]
fn psyched_start_then_stop_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_config(tmp.path());
    Command::cargo_bin("psyched")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "--shutdown-after-start"])
        .assert()
        .success()
        .stderr(contains("psyche lifecycle transition"));
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

[[bin]]
name = "psyche"
path = "src/main.rs"

[[bin]]
name = "psyched"
path = "src/bin/psyched.rs"

[dependencies]
psyche-config = { workspace = true }
psyche-core = { workspace = true }
psyche-runtime = { workspace = true }
clap = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p psyche-cli`
Expected: FAIL — `couldn't find bin psyche` / compilation error, since no source files exist yet.

- [ ] **Step 4: Write the logging installer**

Create `crates/psyche-cli/src/logging.rs`:

```rust
//! Structured JSON logs. Secret-bearing values never reach here as strings —
//! `psyche_core::SecretRef` renders `<redacted>` through both `Debug` and
//! `Display`, so a field holding one cannot leak by accident.

use tracing_subscriber::EnvFilter;

pub fn install() {
    let filter = EnvFilter::try_from_env("PSYCHE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
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

use std::path::Path;

use psyche_config::Config;

pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// Every check here is local and credential-free. Reaching the Coven socket or
/// a Telegram API is explicitly *not* done — those belong to later gates.
pub fn run(config: &Config) -> Vec<Check> {
    let mut checks = vec![Check {
        name: "config",
        ok: true,
        detail: format!("schema {}", config.schema_version),
    }];

    let data_dir: &Path = config.data_dir.as_path();
    let (ok, detail) = match std::fs::create_dir_all(data_dir) {
        Ok(()) => (true, format!("{} writable", data_dir.display())),
        Err(e) => (false, format!("{}: {e}", data_dir.display())),
    };
    checks.push(Check {
        name: "data_dir",
        ok,
        detail,
    });

    checks.push(Check {
        name: "coven_socket_path",
        ok: true,
        detail: format!("{} (not contacted at this gate)", config.coven.socket.display()),
    });

    checks
}
```

- [ ] **Step 6: Write the `psyche` entry point**

Create `crates/psyche-cli/src/main.rs`:

```rust

mod doctor;
mod logging;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "psyche", version, about = "Psyche familiar runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the daemon in the foreground.
    Start {
        #[arg(long, default_value = "psyche.toml")]
        config: PathBuf,
    },
    /// Ask a running daemon to shut down gracefully.
    Stop {
        #[arg(long, default_value = "psyche.toml")]
        config: PathBuf,
    },
    /// Report daemon state.
    Status {
        #[arg(long, default_value = "psyche.toml")]
        config: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Run local, credential-free environment checks.
    Doctor {
        #[arg(long, default_value = "psyche.toml")]
        config: PathBuf,
    },
}

fn main() -> ExitCode {
    logging::install();
    let cli = Cli::parse();

    let path = match &cli.command {
        Command::Start { config }
        | Command::Stop { config }
        | Command::Status { config, .. }
        | Command::Doctor { config } => config.clone(),
    };

    let config = match psyche_config::load_path(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    match cli.command {
        Command::Doctor { .. } => {
            let checks = doctor::run(&config);
            let failed = checks.iter().filter(|c| !c.ok).count();
            for c in &checks {
                println!("{}: {} ({})", c.name, if c.ok { "ok" } else { "FAIL" }, c.detail);
            }
            if failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::Status { json, .. } => {
            // No supervisor exists at this gate, so state is always "stopped".
            // The follow-on plan replaces this with a store-backed probe.
            if json {
                println!("{}", serde_json::json!({ "state": "stopped" }));
            } else {
                println!("state: stopped");
            }
            ExitCode::SUCCESS
        }
        Command::Start { .. } => {
            eprintln!("run `psyched` to start the daemon in the foreground");
            ExitCode::SUCCESS
        }
        Command::Stop { .. } => {
            eprintln!("no running daemon to stop");
            ExitCode::SUCCESS
        }
    }
}
```

- [ ] **Step 7: Write the `psyched` entry point**

Create `crates/psyche-cli/src/bin/psyched.rs`:

```rust

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use psyche_runtime::Runtime;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "psyched", version, about = "Psyche daemon")]
struct Cli {
    #[arg(long, default_value = "psyche.toml")]
    config: PathBuf,
    /// Start, then immediately shut down. Used by tests and smoke checks so the
    /// full lifecycle runs without needing a signal.
    #[arg(long)]
    shutdown_after_start: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let filter = EnvFilter::try_from_env("PSYCHE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    let config = match psyche_config::load_path(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = Runtime::start(config).await;

    if !cli.shutdown_after_start {
        if tokio::signal::ctrl_c().await.is_err() {
            eprintln!("failed to install signal handler");
            return ExitCode::FAILURE;
        }
    }

    match runtime.shutdown().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p psyche-cli`
Expected: `test result: ok. 4 passed; 0 failed`.

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
cargo install cargo-deny --locked
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
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Clippy
        run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - name: Tests
        run: cargo test --workspace --locked --all-features

  supply-chain:
    name: Dependency audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      # Installed directly rather than via a third-party action: it is the same
      # binary and the same command engineers run locally in Task 7 Step 2, so
      # there is no CI-only path, and it adds no extra action to trust.
      - name: Install cargo-deny
        run: cargo install cargo-deny --locked
      - name: Audit
        run: cargo deny check licenses advisories bans sources

  secrets:
    name: Secret guard
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: gitleaks/gitleaks-action@v2
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

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
      - name: Pack dry run
        run: npm --prefix packages/psyche-npm pack --dry-run
```

- [ ] **Step 4: Prove the gate locally**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy --workspace --all-targets --all-features -- -D warnings && \
cargo test --workspace --locked --all-features && \
cargo deny check licenses advisories bans sources
```

Expected: all four succeed with the terminal evidence listed in "Verification commands".

- [ ] **Step 5: Commit**

```bash
git add deny.toml .github/workflows/ci.yml
git commit -m "ci: add fmt, clippy, locked tests, dependency audit, and secret guard"
```

---

## Task 8: npm distribution with checksum verification

**Files:**
- Create: `packages/psyche-npm/package.json`
- Create: `packages/psyche-npm/scripts/verify-checksum.js`
- Create: `packages/psyche-npm/bin/psyche.js`
- Create: `packages/psyche-npm/test/verify-checksum.test.js`
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

const { verifyChecksum, resolvePackageName } = require('../scripts/verify-checksum.js');

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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm --prefix packages/psyche-npm test`
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

- [ ] **Step 4: Run the test to verify it passes**

Run: `npm --prefix packages/psyche-npm test`
Expected: `pass 4`, `fail 0`.

- [ ] **Step 5: Write the wrapper and manifest**

Create `packages/psyche-npm/bin/psyche.js`:

```javascript
#!/usr/bin/env node
'use strict';

const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { resolvePackageName, verifyChecksum } = require('../scripts/verify-checksum.js');

// The wrapper resolves and execs a Rust binary. It holds no daemon, storage,
// identity, policy, or transport logic — that boundary is fixed by PLAN.md W2.
function main() {
  const pkg = resolvePackageName(process.platform, process.arch);
  const manifest = require('../package.json');
  const expected = manifest.psyche.checksums[`${process.platform}-${process.arch}`];
  if (!expected) {
    throw new Error(`no recorded checksum for ${process.platform}-${process.arch}`);
  }

  const binaryName = process.platform === 'win32' ? 'psyche.exe' : 'psyche';
  const binary = path.join(path.dirname(require.resolve(`${pkg}/package.json`)), 'bin', binaryName);

  verifyChecksum(binary, expected);

  const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
  process.exit(result.status === null ? 1 : result.status);
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
    "test": "node --test test/"
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

- [ ] **Step 6: Verify packing includes no binary**

Run: `npm --prefix packages/psyche-npm pack --dry-run`
Expected: the file list contains `bin/psyche.js`, `scripts/verify-checksum.js`, `package.json`, `README.md`, and **no** `.node`, `.exe`, or platform binary. Total size under 10 kB.

- [ ] **Step 7: Commit**

```bash
git add packages/psyche-npm
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
