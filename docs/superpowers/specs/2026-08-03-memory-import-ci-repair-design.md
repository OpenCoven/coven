# Memory Import CI Repair Design

## Context

PR #568 passes its focused and local workspace gates but fails two hosted CI jobs:

- Windows stable Rust rejects a test-only use of the unstable
  `std::os::windows::fs::MetadataExt` file-identity methods.
- Ubuntu runs the workspace unit suite in parallel and 61 memory migration tests
  fail together while opening or securing private fixture directories. The
  failures cascade before their interruption hooks run, which is consistent
  with resource contention among the handle-heavy migration fixtures.

The repair must not weaken production filesystem validation, change migration
semantics, or serialize unrelated workspace tests.

## Considered Approaches

### 1. Isolate only the affected test fixtures

Remove the Windows test helper because every caller is already excluded on
Windows. Wrap each trusted migration temporary directory with a process-wide
Unix test guard whose lifetime matches the fixture lifetime.

This is the recommended approach. It changes test infrastructure only, keeps
the production identity implementation intact, and serializes only the
handle-heavy migration fixture family.

### 2. Serialize the entire CI test suite

Pass `--test-threads=1` to the workspace test command.

This would likely avoid the Ubuntu contention, but it would slow every crate
and conceal future test-isolation problems outside the migration module.

### 3. Rewrite production handle management

Audit and reduce every open capability handle retained during migration.

This is unnecessarily risky for a CI-only failure with no demonstrated
production leak. It would broaden the patch and require repeating the full
security review of the migration state machine.

## Design

### Windows compilation

Keep the Unix metadata identity helper used by the two Unix-only assertions.
Delete the Windows and generic fallback variants. Since both callers already
have `#[cfg(not(windows))]`, Windows does not need to compile a corresponding
helper. Production Windows identity checks continue using
`GetFileInformationByHandleEx` through `windows-sys`.

### Unix test isolation

Introduce a test-only migration fixture mutex in the memory import module so
the canonical reader regression test can use the same gate. Add a
`TrustedTempDir` wrapper in the memory import test module. On Unix it owns:

- a guard from a static `Mutex<()>`; and
- the underlying `tempfile::TempDir`.

The wrapper exposes `path()` and drops the temporary directory before releasing
the guard. `trusted_tempdir()` acquires the guard before creating the fixture. The
cross-module canonical reader regression test acquires the same guard before
constructing its migration fixture. This serializes only tests that exercise
the migration private-directory and journal machinery. Preview tests that do
not use this helper and all unrelated workspace tests remain parallel.

The mutex acquisition must recover from poisoning because an assertion panic
must not make every later test fail without running.

### Error handling

Fixture creation continues returning `anyhow::Result`. Lock poisoning is
recovered with `into_inner()` because the protected value contains no state;
the mutex is solely a concurrency gate. Temporary-directory creation failures
remain explicit.

## Validation

1. Run the focused memory import and cockpit source unit tests with the default
   parallel test harness.
2. Run the process-level memory import integration tests.
3. Run the full workspace suite with the known unrelated mobile TLS test
   excluded.
4. Run formatting and Clippy with warnings denied.
5. Cross-check Windows compilation on stable Rust, then confirm both hosted CI
   matrix jobs pass.

## Non-Goals

- No migration protocol, journal, restore, or reader behavior changes.
- No global CI test serialization.
- No dependency additions.
- No changes to the unrelated mobile TLS test.
