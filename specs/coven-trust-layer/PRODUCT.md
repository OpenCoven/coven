# Coven Trust Layer — PRODUCT

**Status:** Draft v1 · 2026-05-26
**Owner:** Coven runtime
**Acceptance target:** "Sensitive data boundaries are clear."

## Problem

CastCodes is being positioned to render real agent work — transcripts, commands, file diffs, and tool calls produced by Claude, Codex, and future harnesses running under Coven. Before CastCodes can be trusted to show that history, Coven needs a stated contract for what it stores, what it transforms before storing, what it never stores, and what it eventually deletes. Without that contract, every consumer (CastCodes, dashboards, future SDKs) has to guess.

Today the runtime has the right primitives (`privacy.rs` redaction filters, `encrypted_artifacts.rs` for at-rest encryption, retention config in `~/.coven/privacy.toml`) but the *policy* is implicit and partially enforced. This spec makes the policy explicit and removes the "implicit" half.

## The contract, by data class

Each row below is the **only** answer for that data class. There are no per-call overrides for "sensitive" classes — opting out requires changing config or moving the data to a different class.

| Data class | Stored? | Form on disk | Redacted before storing? | Encrypted at rest? | Retention | Visible over `/v1/*` (CastCodes)? |
|---|---|---|---|---|---|---|
| **Transcript text** (user prompts, assistant replies, plain prose) | Yes | SQLite `events.payload_json` | Yes — redact filter applied (regex + field-name patterns) before insert | No (redacted; not "sensitive") | `log_retention_days` (default 30) | Yes |
| **Tool-call input/output** (command, args, exit code, stdout/stderr) | Yes | SQLite `events.payload_json` | Yes — same redact filter | No | `log_retention_days` (default 30) | Yes |
| **Raw artifact** (un-redacted command output, full tool-call body when redaction can't be guaranteed faithful) | Yes (only when `persist_raw_artifacts = true`) | SQLite `sensitive_artifacts.ciphertext` (XChaCha20Poly1305) | No (raw is the point) | **Yes, always** | `raw_artifact_retention_days` (default 7) | **No** — TCP path returns a placeholder ref; decrypt is Unix-socket-only |
| **Changed-file snapshot** (pre/post content for files Coven touched) | Yes | SQLite `sensitive_artifacts.ciphertext` | No (file contents are not text-pattern redactable safely) | **Yes, always** | `raw_artifact_retention_days` (default 7) | **No** content over TCP; metadata (path, byte count, hash) yes |
| **Verification output** (test runs, type checks, lint results) | Yes | `events.payload_json` if size ≤ 64 KiB, else encrypted artifact + ref | Yes (redact filter) | If artifact, yes | `log_retention_days` (default 30) | Yes (metadata always; full body if it was inline) |
| **Handoff packet** | Yes | `events.payload_json` of kind `handoff` | Yes (redact filter) | No | `log_retention_days` (default 30) | Yes |
| **Final session summary** | Yes | `events.payload_json` of kind `summary` | Yes (redact filter) | No | `log_retention_days` (default 30) | Yes |
| **Provenance record** (which harness/run/cwd produced the event) | Yes | Columns on `events` (`producer_harness`, `producer_run_id`, `producer_cwd`) | Never redacted (it's metadata about the run, not content from the run) | No | Same as parent event | Yes |
| **Secret material** (private keys, API tokens, bearer tokens, password fields) | **Never stored** — even when discovered mid-stream, the bytes are replaced with `[REDACTED]` before persistence. Coven does not keep the original. | — | — | — | — | — |
| **Session-encryption key** (`~/.coven/keys/session-artifacts.key`) | Yes (on disk, mode 0600) | Hex key file outside the database | n/a | n/a (it IS the key) | Persistent until user rotates | **Never** over any wire |
| **`~/.coven/token`** (the bearer token CastCodes uses) | Yes (file, mode 0600) | Hex on disk | n/a | n/a | Persistent until rotated | Never (consumer reads its local file; daemon checks the value but never returns it) |
| **OS keychain entries** | Not used today | — | — | — | — | — |

## Defaults that must hold

- `persist_raw_artifacts` defaults to **`false`**. Plain operation of Coven produces only redacted text in `events.payload_json`. The encrypted-artifact table is empty unless the user explicitly opts in.
- When `persist_raw_artifacts = true`, raw artifacts are **always** encrypted. There is no "raw + unencrypted" state.
- Redaction is **applied at write time**, not at read time. The database never holds an unredacted copy of a "Transcript text" / "Tool-call" / "Verification" / "Summary" / "Handoff" event.
- Retention is **enforced** by a daemon prune pass on startup and every 6 hours. Expired rows are deleted (not soft-deleted) and the underlying SQLite page space is reclaimed via incremental vacuum.

## "Never stored" — the absolute list

These bytes never land in the SQLite database, never land in an encrypted artifact, never appear in any log Coven writes:

1. The original bytes of a value that matched a secret pattern (private keys, `Authorization` header values, API keys for known vendors, `secret|password|*_token|*_key|auth*|api*` field values).
2. The plaintext of `~/.coven/keys/session-artifacts.key` or `~/.coven/token` (Coven reads them and uses them; it does not echo them into events, logs, or responses).
3. The contents of files the user has explicitly added to `~/.coven/privacy.toml` `excluded_paths` (planned; see Gaps).
4. Anything outside the project root that wasn't part of the agent's input or output (Coven does not snoop the home directory).

## What CastCodes sees over `/v1/*`

A CastCodes client authenticated with a bearer token sees:

- All `events` columns including `payload_json` (already redacted).
- `provenance` columns (harness, run id, cwd).
- Metadata for any encrypted artifact: id, kind, byte count, hash, created_at, expires_at. **Never the ciphertext or the decrypted plaintext.**
- Session-level fields: id, project_root, harness, title, status, exit_code, created_at, updated_at.

A CastCodes client does **not** see, ever, over TCP:

- Encrypted artifact bodies (ciphertext or decrypted).
- The session-artifacts key.
- The bearer token itself (the client supplied it; the server doesn't echo it).
- Any field added to a future `daemon.json` `[gateway].never_expose` block.

## Pruning behavior

- On daemon startup: scan `events` and `sensitive_artifacts` for rows past their retention deadline; delete; `PRAGMA incremental_vacuum`.
- Every 6 hours while daemon is running: same pass.
- On `coven prune --aggressive`: collapse retention to "now" for any data class the user names. Operator-only, Unix-socket only.
- On `coven session archive <id>`: stops further retention pruning of that session's rows until unarchived. Archive does **not** copy data — it sets `archived_at` on the session row.

## Gaps this spec consciously does not yet fix

These are flagged so the implementation order is clear; they're not part of v1 acceptance:

- **`excluded_paths` config** — user-controlled list of paths Coven refuses to snapshot. Today there is no such list; project-root scoping is the only boundary.
- **Per-session key rotation** — today, one `session-artifacts.key` per machine. Rotation requires re-encrypting historical artifacts.
- **OS keychain integration** for the session-artifacts key — today it lives in `~/.coven/keys/` as a plain file with mode 0600.
- **Audit log** of decrypts — when a user decrypts an artifact via Unix-socket, that fact is not currently recorded as its own event.

## Acceptance for v1

The trust layer is considered "v1 done" when:

1. The contract table above is enforced by code, not just documented (see TECH.md for the gap list).
2. `coven doctor` reports a green/red status for each row of the table on the local machine.
3. CastCodes can fetch a session's events over TCP and observe that no row in the response contains a secret-pattern match (regression-tested).
4. Pruning runs on startup and on schedule, and `coven session list --include-pruned-counts` shows how many rows were dropped in the last pass.
