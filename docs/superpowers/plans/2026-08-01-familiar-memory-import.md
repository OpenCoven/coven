# Familiar-Scoped Memory Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a preview-first, copy-only `coven memory import` workflow that migrates one registered familiar at a time into canonical Coven memory and can safely verify, resume, and restore only its own unchanged writes.

**Architecture:** Rust remains the sole filesystem authority. A focused `memory_import` module discovers allowlisted Markdown sources, builds a deterministic redacted plan for one familiar, and applies that plan through a private bundle and append-only journal outside the canonical memory scanner root. Publication is no-replace and restore is conditional on the recorded digest, so conflicts, retries, interruption, and user edits fail closed.

**Tech Stack:** Rust 2021, Clap, Serde/JSON, BLAKE3, cap-std/cap-fs-ext, tempfile integration fixtures, existing Coven privacy and secret guards.

---

## File responsibility map

- `crates/coven-cli/src/memory_import.rs` — source adapters, deterministic plan, bundle/journal state machine, apply/restore, redacted rendering, and focused unit tests.
- `crates/coven-cli/src/main.rs` — CLI types and dispatch only; preserve bare `coven memory`, `--json`, `open`, and `mobile`.
- `crates/coven-cli/tests/memory_import.rs` — process-level synthetic tests for native/OpenClaw preview, apply, conflict, retry, interruption, verification, and restore.
- `docs/help/memory-import.md` — operator workflow, source allowlist, safety model, recovery, and examples.
- `README.md` — one discoverability link to the detailed guide.

### Task 1: Lock the CLI and report contract

**Files:**
- Modify: `crates/coven-cli/src/main.rs`
- Create: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Write failing parse tests**

Add assertions for:

```rust
coven memory import --familiar sage
coven memory import --familiar sage --apply
coven memory import --familiar sage --source openclaw --openclaw-root <path>
coven memory restore --familiar sage --bundle <id>
```

Also assert that `coven memory import --json` uses the nested command's JSON flag while bare `coven memory --json` remains unchanged.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p coven-cli memory_import_cli --locked
```

Expected: compile or parse failure because the new commands do not exist.

- [ ] **Step 3: Add command types**

Define:

```rust
enum MemoryImportSourceArg {
    Native,
    Openclaw,
}

enum MemoryCommand {
    Open,
    Mobile { command: MobileMemoryCommand },
    Import {
        familiar: String,
        source: MemoryImportSourceArg,
        openclaw_root: Option<PathBuf>,
        apply: bool,
        json: bool,
    },
    Restore {
        familiar: String,
        bundle: String,
        json: bool,
    },
}
```

Validation must require `--openclaw-root` only for `--source openclaw`, reject it for native imports, and keep parent `memory --json` list-only.

- [ ] **Step 4: Define stable redacted reports**

In `memory_import.rs`, define serializable plan/apply/restore reports with only:

```rust
familiar_id
source_kind
bundle_id
status
file_count
created_count
unchanged_count
restored_count
conflict_count
entries[].source_label
entries[].target_name
entries[].digest
entries[].status
```

Reports must never include memory content or absolute source paths.

- [ ] **Step 5: Verify GREEN and commit**

Run:

```bash
cargo test -p coven-cli memory_import_cli --locked
```

Commit:

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/memory_import.rs
git commit -s -m "feat(memory): define familiar import workflow"
```

### Task 2: Discover one familiar's allowlisted sources

**Files:**
- Modify: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/tests/memory_import.rs`

- [ ] **Step 1: Write failing source-boundary tests**

Create synthetic registered familiar workspaces and assert:

- native imports read only root `MEMORY.md` and regular `.md` files below `memory/` and `notes/`;
- OpenClaw imports read only root `MEMORY.md`, root `DREAMS.md`, and `.md` files below `memory/`;
- hidden files/directories, symlinks, non-UTF-8 names, special files, transcripts, sessions, logs, config, auth, credentials, `AGENTS.md`, `USER.md`, `SOUL.md`, and `TOOLS.md` are excluded;
- an unknown familiar fails before source enumeration;
- OpenClaw always requires the explicit target familiar.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p coven-cli memory_import::tests::discovers --locked
```

- [ ] **Step 3: Implement adapters**

Use separate `NativeSourceAdapter` and `OpenClawSourceAdapter` implementations behind a small internal trait. Resolve the native workspace through `cockpit_sources::read_familiars` and `familiar_workspace`. Open every path without following symlinks, require a regular file, bound each file to the existing memory content maximum, validate UTF-8, and return logical labels rather than absolute paths.

- [ ] **Step 4: Verify and commit**

Run:

```bash
cargo test -p coven-cli memory_import::tests::discovers --locked
cargo test -p coven-cli --test memory_import source_boundaries --locked
```

Commit:

```bash
git add crates/coven-cli/src/memory_import.rs crates/coven-cli/tests/memory_import.rs
git commit -s -m "feat(memory): discover familiar import sources"
```

### Task 3: Build deterministic plans and fail closed on conflicts

**Files:**
- Modify: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/tests/memory_import.rs`

- [ ] **Step 1: Write failing planning tests**

Assert:

- the target is exactly `$COVEN_HOME/memory/<familiar>/<flat>.md`;
- nested names flatten deterministically with collision-resistant digest suffixes;
- input order does not change plan order, target names, digests, or bundle ID;
- exact-byte existing targets are `unchanged`;
- any divergent target marks the whole plan `conflict` and prevents all writes;
- duplicate logical sources and Unicode/case-fold target collisions fail closed;
- preview creates no target, bundle, or journal.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p coven-cli memory_import::tests::plans --locked
```

- [ ] **Step 3: Implement the planner**

Hash exact bytes with BLAKE3. Derive a stable target name from the logical source label, retaining `.md`, and append a short digest when flattening would collide. Derive the bundle ID from the familiar, source kind, sorted logical labels, target names, and digests. Inspect all targets before mutation and return a complete conflict report without creating directories.

- [ ] **Step 4: Verify and commit**

Run:

```bash
cargo test -p coven-cli memory_import::tests::plans --locked
cargo test -p coven-cli --test memory_import preview --locked
```

Commit:

```bash
git add crates/coven-cli/src/memory_import.rs crates/coven-cli/tests/memory_import.rs
git commit -s -m "feat(memory): plan deterministic familiar imports"
```

### Task 4: Apply through a private recoverable bundle

**Files:**
- Modify: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/tests/memory_import.rs`

- [ ] **Step 1: Write failing transaction tests**

Assert:

- apply creates `$COVEN_HOME/memory-migrations/<familiar>/<bundle>/` with private permissions;
- `manifest.json`, staged files, and `journal.jsonl` are outside `$COVEN_HOME/memory`;
- staged bytes and digests are verified before publication;
- targets publish with no-replace semantics;
- a conflict discovered immediately before publication rolls back this run's creations;
- each journal transition is durable before the next mutation;
- an interrupted `prepared` or partially `published` bundle resumes safely;
- exact rerun is idempotent and creates no duplicate files.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p coven-cli memory_import::tests::apply --locked
```

- [ ] **Step 3: Implement the state machine**

Use explicit states:

```text
prepared -> publishing -> verified
prepared|publishing -> rolling_back -> rolled_back
verified -> restoring -> restored
```

Create directories and files with private permissions, `create_new`, no-follow checks, sibling staging, `sync_all`, and parent directory syncs. Journal every target as `prepared`, `published`, or `verified`. On failure, remove only targets created by the current bundle whose digest still matches.

Apply is supported only where the implementation can guarantee private files,
atomic no-replace publication, and durable parent-directory synchronization.
On Windows, preview remains available but apply must fail before creating the
migration or canonical memory directories until equivalent durability
primitives are implemented.

- [ ] **Step 4: Verify and commit**

Run:

```bash
cargo test -p coven-cli memory_import::tests::apply --locked
cargo test -p coven-cli --test memory_import apply --locked
```

Commit:

```bash
git add crates/coven-cli/src/memory_import.rs crates/coven-cli/tests/memory_import.rs
git commit -s -m "feat(memory): apply recoverable familiar imports"
```

### Task 5: Verify and conditionally restore

**Files:**
- Modify: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/src/memory_import.rs`
- Test: `crates/coven-cli/tests/memory_import.rs`

- [ ] **Step 1: Write failing restore tests**

Assert:

- successful apply reopens every canonical target and verifies its digest;
- restore logically suppresses only a target whose current bytes and inode still match the journal and private candidate;
- user-edited, replaced, symlinked, missing, or non-regular targets are retained and reported;
- restore is idempotent;
- restoring one familiar never inspects or mutates another familiar;
- a restored or partially restored bundle resumes safely after interruption.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p coven-cli memory_import::tests::restore --locked
```

- [ ] **Step 3: Implement restore**

Load the bundle by familiar and validated bundle ID. Revalidate the manifest and journal before mutation. For each recorded creation, open without following symlinks, compare BLAKE3 and inode identity with the private candidate, and set a versioned digest-bound restore marker through the verified file descriptor. Canonical readers suppress only markers whose familiar, bundle, target, manifest, terminal journal state, candidate identity, and current digest all validate. Physical bytes remain recoverable; edited, replaced, or unsafe targets stay visible and untouched with a non-success status. This logical suppression is required because POSIX has no atomic "unlink this name only if it still references this opened inode" operation.

Reapplying the same unchanged source plan to a completely restored bundle uses a resumable `restoring -> restored -> reactivating -> verified` journal cycle. Reactivation changes only journal authority, leaves canonical pathnames and markers untouched, and allows the bundle to be restored repeatedly. Every initial or resumed reactivation revalidates canonical, marker, and candidate provenance; a mismatch transitions durably to `invalidated` manual recovery.

- [ ] **Step 4: Verify and commit**

Run:

```bash
cargo test -p coven-cli memory_import::tests::restore --locked
cargo test -p coven-cli --test memory_import restore --locked
```

Commit:

```bash
git add crates/coven-cli/src/memory_import.rs crates/coven-cli/tests/memory_import.rs
git commit -s -m "feat(memory): restore unchanged imported files"
```

### Task 6: Wire output, documentation, and comprehensive gates

**Files:**
- Modify: `crates/coven-cli/src/main.rs`
- Modify: `README.md`
- Create: `docs/help/memory-import.md`
- Modify: `crates/coven-cli/tests/memory_import.rs`

- [ ] **Step 1: Add end-to-end CLI tests**

Cover human and JSON preview/apply/restore output, exact exit codes for conflicts and unsafe bundles, native and OpenClaw variants, one-familiar isolation, idempotent rerun, interrupted resume, conditional restore, and canonical `coven memory --json` readback after apply.

- [ ] **Step 2: Document the operator loop**

Document:

```bash
coven memory import --familiar sage
coven memory import --familiar sage --apply
coven memory import --familiar sage --source openclaw --openclaw-root <workspace>
coven memory restore --familiar sage --bundle <bundle-id>
```

State explicitly that preview is the default, sources remain untouched, divergent targets abort, restore is digest-conditional, bundles are local/private, and imports are intentionally one familiar at a time.

- [ ] **Step 3: Run targeted validation**

```bash
cargo fmt --check
cargo test -p coven-cli --test memory_import --locked
cargo test -p coven-cli memory_import --locked
cargo clippy -p coven-cli --all-targets -- -D warnings
```

- [ ] **Step 4: Run repository gates**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
git diff --check
git add crates/coven-cli/src/main.rs crates/coven-cli/src/memory_import.rs crates/coven-cli/tests/memory_import.rs docs/help/memory-import.md README.md
python3 scripts/check-coven-privacy.py --staged
```

- [ ] **Step 5: Verify the reversible loop**

In an isolated temporary `COVEN_HOME`, prove:

```text
preview -> apply -> canonical readback -> identical rerun -> restore
```

Compare source bytes before/after, canonical target hashes after apply, canonical reader absence after restore, and retained physical recovery bytes. Repeat with a user edit after apply and verify restore leaves it visible and untouched.

- [ ] **Step 6: Commit**

```bash
git commit -s -m "docs(memory): document familiar import recovery"
```
