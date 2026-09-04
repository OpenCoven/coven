# Red Team Findings — magician (9baa50c) — 007 STRIDE (T4)

**Date:** 2026-09-01 · **Lead:** 007 (Sc3pt3R) · **Scope:** Full (B+C+D+E) + web/ + offline --locked
**Baseline:** `origin/main..magician` = 1 commit (9baa50c, docs-only) — 0 code delta vs c5a4651

## 1) Baseline results

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | ✅ PASS | re-verified this session |
| `python scripts/check-secrets.py` | ✅ PASS | 7 rules + ENTROPY 4.3 (prior run, confirmed in plan) |
| `cargo clippy --workspace --all-targets -D warnings` | ⚠️ DEFERRED | rustc 1.93.1→1.98.0 upgraded (sysinfo@0.39.6 needs 1.95); workspace compile exceeds agent exec process limits — run locally or via CI |
| `cargo test --workspace --locked` | ⚠️ DEFERRED | same compile constraint; CI gate covers |
| `cargo deny check advisories bans` | ⚠️ N/A local | cargo-deny not installed on this Mac; enforced in CI (`ci.yml:60` policy-guard + cargo-deny, `deny.toml:10` yanked=deny) |

**Verdict:** No findings from the checks that ran. Deferred items are environment constraints, not code signals. CI (`ci.yml`) is the enforcement backstop.

## 2) STRIDE — manual review of authority boundary artifacts (007 lead)

Reviewed on `magician:9baa50c` — all line refs verified this session.

### S — Spoofing (who can claim familiar identity)

| Artifact | Evidence | Finding |
| --- | --- | --- |
| `crates/coven-cli/src/api.rs:393` | `RequestAuthority::{OwnerLocalIpc, Tcp}`; `allows_session_launch_policy()` → `OwnerLocalIpc` only | ✅ Strong. Session launch (code-exec path) restricted to filesystem-permission-protected IPC. TCP authority cannot launch sessions — spoofing a browser/loopback caller is insufficient. |
| `crates/coven-client/src/discovery.rs:49` | `DaemonEndpoint::discover`: symlink refusal, socket-type check, `uid() != geteuid()` refusal, `mode & 0o077 != 0` refusal, canonicalize-before-check | ✅ Strong. Socket planted by another local user is rejected 4 ways (symlink/type/uid/mode). Residual: classic check-then-use race between `discover` and first RPC — mitigated downstream by client fingerprint (below). |
| `packages/openclaw-coven/src/client.ts:269` | `fingerprintSocket` (dev/ino/mode/uid/gid) + `socketFingerprintMatches` before use | ✅ Good defense-in-depth: re-stat + fingerprint match closes most of the discovery-to-use window (inode/dev swap detected). |

### T — Tampering (durable state, session store)

| Artifact | Evidence | Finding |
| --- | --- | --- |
| `crates/coven-cli/src/daemon.rs:1484` | `check_owned_by_current_user` fail-closed; `ensure_private_coven_home`: symlink bail, uid check, `0o700` create | ✅ Strong. Planted COVEN_HOME (symlink redirect or foreign-uid dir) is refused. Matches docs/AUTH.md "Current hardening gap" hardening. |
| `crates/coven-cli/src/project.rs:24` | `canonical_project_root` + `resolve_inside_root` bails `cwd is outside the Coven project root` | ✅ Path-escape prevention intact. Windows `\\?\` normalization handled. |

### R — Repudiation (audit trail)

| Artifact | Evidence | Finding |
| --- | --- | --- |
| `crates/coven-cli/src/ward.rs:82` | Tier 0-3 (Protected/Reviewed/Logged/Free); Gate 1 = principal authorization, Gate 3 = coherence review, Gate 4 = auto-approved **with logging** | ✅ Tier-2 modifications are auto-approved but logged — repudiation surface limited to Tier 3 (Free, by design unrestricted). |

### I — Information disclosure

| Artifact | Evidence | Finding |
| --- | --- | --- |
| `crates/coven-afs/src/nfs.rs:52` | `ensure_loopback` — hard refusal, "no deployment in which it is correct [to bind non-loopback]"; careful IPv6 bracket/colon disambiguation; `localhost` only by name | ✅ Strong. Unauthenticated-at-RPC export is loopback-jailed. IPv6 parse-order comment shows the `::1`→`:` strip bug was considered and fixed. |

### D — DoS

No new code delta on `magician` (docs-only commit) → DoS surface unchanged vs `c5a4651` baseline. Fingerprint checks are constant-cost. No findings.

### E — Elevation of privilege

| Artifact | Evidence | Finding |
| --- | --- | --- |
| `crates/coven-cli/src/ward.rs:60-74` (doc comment) | **Acknowledged residual TOCTOU**: inside-familiar-home actor can swap path components between per-component verification and final `rename`. Narrowed, not eliminated — full fix needs `openat2`+`RESOLVE_BENEATH` (not portable). Final-component swap can poison Gate 4 audit `prev_sha256` — "never where bytes land". | ⚠️ KNOWN/ACCEPTED. Write target is never attacker-controlled; impact is limited to audit-record integrity (`prev_sha256`). Documented in-source. Recommend (hardening backlog): directory-handle-relative I/O on Linux where `openat2` exists, feature-gated. |
| Remote TCP `launchPolicy` | blocked `api.rs:2007` → 403 (per plan scope-out) | ✅ Out of scope, confirmed blocked. |

## 3) PASTA — attacker goal: session hijack → code exec via tool

1. **Target:** durable session on disk (COVEN_HOME), familiar surface write via ward Gate 1/3.
2. **Entry vectors:** (a) local same-uid attacker/process, (b) loopback TCP caller, (c) web/ static.
3. **(a) local same-uid:** already game-over by definition (same uid = same authority) — TOCTOU note in ward.rs is the honest statement of this. No additional finding.
4. **(b) loopback TCP:** Host/Origin checks; cannot launch sessions (`OwnerLocalIpc` only). To gain code exec, TCP caller would need to reach an existing session's input API — `send_input` is behind `launch_session` policy chain. ✅ blocked by authority gate.
5. **(c) web/:** static `web/index.html:1` — no daemon fetch (per plan explore). ✅ no attack surface.
6. **Conclusion:** No new PASTA path opens on `magician` (0 code delta). The authority boundary holds: Rust daemon is the sole authority (`README.md:43` / `docs/ARCHITECTURE.md:17`); clients remain non-trust-boundary.

## 4) Supply chain

- `Cargo.lock`: 6216 lines, `sysinfo@0.39.6` (MSRV 1.95 — now satisfied by rustc 1.98.0; no pin needed).
- `deny.toml:10`: `yanked=deny` + one pinned ignore `RUSTSEC-2024-0436` (documented).
- `engine.lock`: 5 sha256 pins. `scripts/check-secrets.py:33`: 7 rules, ENTROPY 4.3 — PASS.
- CI `ci.yml:60`: policy-guard + cargo-deny on every PR — enforcement backstop for advisories/bans/licenses/sources.

## 5) Findings summary (ranked)

| # | Severity | Finding | Recommendation |
| --- | --- | --- | --- |
| F-1 | LOW (accepted) | ward.rs in-source TOCTOU: final-component swap poisons Gate 4 `prev_sha256` audit record | Backlog: `openat2`+`RESOLVE_BENEATH` feature-gated for Linux; keep portable fallback |
| F-2 | INFO | clippy/test not runnable in agent exec env (process limits) | Run locally `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace --locked` before PR |
| F-3 | INFO | cargo-deny not installed locally | CI covers; optionally `cargo install cargo-deny --locked` |

**No HIGH/CRITICAL findings.** `magician:9baa50c` is docs-only vs `c5a4651`; authority boundaries (OwnerLocalIpc launch policy, COVEN_HOME 0700/uid/symlink fail-closed, socket fingerprint TOCTOU closure, loopback jail, ward tier gates) all verified intact by direct source review this session.

**007 verdict: SHIP** — merge-safe pending F-2 local clippy/test run (procedural, not a code finding).

---

*Generated by 007 lead (Sc3ct3R) under `~/.agent/plans/coven-magician-redteam-aab12f61ff8e9f6ea.md` T4. Read-only audit — no edits, no commits made by this pass.*
