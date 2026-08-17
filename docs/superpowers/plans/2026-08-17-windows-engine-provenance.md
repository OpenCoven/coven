# Windows Engine Provenance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Report Windows managed and legacy engine paths as default-derived when `dirs_next` resolves the native profile directory, even if environment variables contain the same path.

**Architecture:** Keep engine-source classification at the existing `user_home_source` boundary. Pass the platform resolver's environment dependency into a small candidate classifier so Windows always returns `PathSource::Default`, while non-Windows platforms retain environment comparison.

**Tech Stack:** Rust, Cargo, `dirs-next`, existing `coven-cli` unit and integration tests

---

### Task 1: Correct native Windows profile provenance

**Files:**
- Modify: `crates/coven-cli/src/config_paths.rs:595-617`
- Test: `crates/coven-cli/src/config_paths.rs:679-701`
- Test: `crates/coven-cli/tests/config_paths.rs:55-189`

- [x] **Step 1: Confirm the failing provenance contract**

Add a platform-neutral unit assertion around the resolver contract:

```rust
#[test]
fn native_profile_home_is_default_even_when_environment_matches() {
    let home = Path::new("/native-profile");

    assert_eq!(
        user_home_source_from_candidates(
            Some(home),
            false,
            [home.to_path_buf()].into_iter()
        ),
        PathSource::Default
    );
}
```

Run:

```bash
cargo test -p coven-cli native_profile_home_is_default_even_when_environment_matches
```

Expected before the fix: FAIL because `user_home_source_from_candidates` does not exist. The existing Windows integration assertion independently verifies the full report emits `"default"`.

- [x] **Step 2: Make provenance follow the platform resolver**

Separate environment candidate collection from provenance classification:

```rust
fn user_home_source(home: Option<&Path>) -> PathSource {
    let environment_homes = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let drive_and_path = std::env::var_os("HOMEDRIVE")
        .filter(|value| !value.is_empty())
        .zip(std::env::var_os("HOMEPATH").filter(|value| !value.is_empty()))
        .map(|(mut drive, path)| {
            drive.push(path);
            PathBuf::from(drive)
        });
    user_home_source_from_candidates(
        home,
        !cfg!(windows),
        environment_homes.chain(drive_and_path),
    )
}

fn user_home_source_from_candidates(
    home: Option<&Path>,
    resolver_uses_environment: bool,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> PathSource {
    let Some(home) = home.filter(|_| resolver_uses_environment) else {
        return PathSource::Default;
    };
    if candidates.into_iter().any(|candidate| candidate == home) {
        PathSource::Environment
    } else {
        PathSource::Default
    }
}
```

- [x] **Step 3: Run focused tests**

```bash
cargo test -p coven-cli config_paths
cargo test -p coven-cli --test config_paths
```

Expected: all selected tests pass; Windows runs specifically exercise the default-provenance assertion.

- [x] **Step 4: Run formatting and lint checks**

```bash
cargo fmt --all -- --check
cargo clippy -p coven-cli --all-targets -- -D warnings
```

Expected: both commands exit successfully with no diagnostics.

- [x] **Step 5: Commit the fix**

```bash
git add crates/coven-cli/src/config_paths.rs \
  docs/superpowers/plans/2026-08-17-windows-engine-provenance.md
git commit -m "fix(config): preserve Windows engine provenance"
```

Expected: a commit containing only the provenance correction and its implementation plan.
