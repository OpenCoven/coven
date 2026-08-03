# Memory Import CI Repair Design

## Context

PR #568 passes its focused and local workspace gates but fails two hosted CI jobs:

- Windows stable Rust rejects a test-only use of the unstable
  `std::os::windows::fs::MetadataExt` file-identity methods.
- Ubuntu fails every migration test that creates a private import directory.
  The first apply test fails immediately, while discovery-only tests pass.
  `cap_std::fs::Dir` may hold an `O_PATH` descriptor on Linux. Converting that
  descriptor to `std::fs::File` before calling `set_permissions` makes
  `fchmod` fail, and calling `sync_all` on the cloned descriptor makes `fsync`
  fail.

The repair must not weaken production filesystem validation, change migration
semantics, or serialize unrelated workspace tests.

## Considered Approaches

### 1. Use capability-relative permission changes

Remove the Windows test helper because every caller is already excluded on
Windows. Change private directory hardening to call
`Dir::set_permissions(".", ...)`, allowing `cap-std` to handle Linux `O_PATH`
descriptors without abandoning the pinned capability root.

This is the recommended approach. It preserves the pinned-directory authority,
uses the dependency's Linux-specific safe implementation, and fixes the
production incompatibility exposed by CI.

### 2. Serialize migration tests

Add a process-wide mutex around migration fixtures.

This does not address the failure: the first apply test already fails before
parallel pressure develops.

### 3. Reopen directories by ambient pathname

Use `std::fs::set_permissions` on the canonical pathname.

This would work around `O_PATH`, but would reintroduce a pathname race between
validation and permission changes. It is incompatible with the migration
safety model.

## Design

### Windows compilation

Keep the Unix metadata identity helper used by the two Unix-only assertions.
Delete the Windows and generic fallback variants. Since both callers already
have `#[cfg(not(windows))]`, Windows does not need to compile a corresponding
helper. Production Windows identity checks continue using
`GetFileInformationByHandleEx` through `windows-sys`.

### Linux directory hardening

Keep operating through the already-opened `cap_std::fs::Dir`. Set mode `0700`
on `"."` with `Dir::set_permissions` and `cap_std::fs::PermissionsExt`.
`cap-std` handles Linux `O_PATH` descriptors by using its capability-safe
permission implementation, including its `/proc/self/fd` and normal-handle
fallbacks.

For durability, open `"."` relative to the same pinned directory with read,
no-follow, and directory options, then call `sync_all` on that normal file
descriptor. The subsequent ownership and mode validation remains unchanged.

### Error handling

Permission errors remain redacted as `unable to secure private import
directory`, preserving the existing output contract. The mode and ownership
validation immediately following the permission change remains the authority
for accepting the directory.

## Validation

1. Run the focused memory import and cockpit source unit tests.
2. Run the process-level memory import integration tests.
3. Run the full workspace suite with the known unrelated mobile TLS test
   excluded.
4. Run formatting and Clippy with warnings denied.
5. Cross-check Windows compilation on stable Rust, then confirm both hosted CI
   matrix jobs pass.

## Non-Goals

- No migration protocol, journal, restore, or reader behavior changes.
- No CI or test serialization.
- No dependency additions.
- No changes to the unrelated mobile TLS test.
