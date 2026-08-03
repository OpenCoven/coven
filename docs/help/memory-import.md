# Familiar memory import

`coven memory import` migrates allowlisted Markdown into canonical Coven memory one registered familiar at a time. Preview is always the default.

## Operator loop

```bash
# Preview native memory for one registered familiar.
coven memory import --familiar sage

# Apply the exact previewed bytes.
coven memory import --familiar sage --apply

# Preview an explicit OpenClaw workspace.
coven memory import \
  --familiar sage \
  --source openclaw \
  --openclaw-root /path/to/workspace

# Logically restore a verified import bundle.
coven memory restore --familiar sage --bundle blake3-<digest>
```

Add `--json` to any import or restore command for a stable, redacted report. Reports contain familiar IDs, logical source labels, flat target names, digests, counts, and statuses. They never contain memory content or absolute source paths.

## Safety model

- Imports are intentionally scoped to one registered familiar.
- Preview creates no migration bundle, canonical directory, or target.
- Source files are copied, never moved, edited, or deleted.
- Any divergent canonical target makes the whole apply ineligible.
- Apply publishes with atomic no-replace semantics and verifies exact BLAKE3 digests.
- Private bundles, manifests, journals, staged bytes, candidates, and locks use owner-only permissions.
- Interrupted apply and restore operations resume from durable journal state.
- Windows supports preview. Apply and restore fail before mutation until equivalent durability and marker guarantees are available.

Native discovery reads only root `MEMORY.md` and regular Markdown files below `memory/` and `notes/`. OpenClaw discovery reads only root `MEMORY.md`, root `DREAMS.md`, and regular Markdown files below `memory/`. Hidden entries, symlinks, special files, credentials, configuration, transcripts, sessions, logs, and agent instruction files are excluded.

## Restore behavior

Restore is deliberately non-destructive. POSIX does not provide an atomic operation that removes a pathname only when it still references a previously opened inode, so a pathname-based rollback could displace a concurrent user edit.

For each target created by the bundle, Coven:

1. Reopens the canonical target and private candidate without following symlinks.
2. Verifies the target type, BLAKE3 digest, and inode identity.
3. Writes a versioned restore marker through the verified file descriptor.
4. Revalidates the canonical target, candidate, marker, and digest before journaling success.

Canonical Coven readers hide a marked file only when its current bytes, familiar, bundle, target name, private manifest, terminal journal state, and candidate inode all still validate. The physical bytes remain in canonical and private storage for recovery. External filesystem tools may still see the canonical file.

Edited, replaced, missing, symlinked, or non-regular targets remain visible and untouched. Restore reports `manual_recovery` and exits nonzero when any entry is ambiguous. Re-running an interrupted or completed restore is safe and idempotent.

To reactivate a completely restored bundle, preview and apply the same unchanged sources again. Coven records a resumable `reactivating` journal phase, revalidates restore provenance after interruption, and makes the validated files visible without removing markers or changing canonical pathnames. Lost or changed provenance durably invalidates the bundle for manual recovery. A successfully reactivated bundle can be logically restored again.

## Bundles and recovery

Bundles live under:

```text
$COVEN_HOME/memory-migrations/<familiar>/<bundle-id>/
```

Same-filesystem publication evidence lives under:

```text
$COVEN_HOME/memory/<familiar>/.coven-migration-work/<bundle-id>/
```

Do not edit bundle, journal, marker, candidate, or work files. If restore reports manual recovery, leave the retained evidence in place and inspect the redacted report before making any manual filesystem change. Coven never automatically overwrites a conflict or republishes untrusted recovery state.

## Exit behavior

| Operation | Result |
| --- | --- |
| Eligible preview | Exit 0, status `preview` |
| Conflicted preview | Exit 0, status `conflict`; no mutation |
| Verified apply | Exit 0, status `verified` |
| Unsafe or conflicted apply | Nonzero; no overwrite |
| Complete logical restore | Exit 0, status `restored` |
| Ambiguous restore | Nonzero, status `manual_recovery`; target retained |
