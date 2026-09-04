# Coven Automations v1 Critical-Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Coven-native automation from the landed durable-routine foundation to one released, identity-bound, authority-checked, independently certified v1 consumed by Cave and the supported SDK/orchestration surfaces.

**Architecture:** Coven's Rust daemon remains the only automation lifecycle authority. Familiar Contract supplies immutable embodiment identity, Coven Threads supplies operation-specific authority and approval decisions, runtimes execute replaceable work, and Cave/SDK/Psyche consume versioned artifacts without creating parallel truth. Work advances through dependency-gated repository lanes, each with its own issue, worktree, claim, tests, pull request, and immutable evidence receipt.

**Tech Stack:** Rust, SQLite, JSON Schema 2020-12, TypeScript, Node.js 24, pnpm, Next.js/React, GitHub Actions, Beads 1.3.0-rc.1 with embedded Dolt schema v66, npm OIDC trusted publishing.

---

## Scope and status snapshot

This is a **program execution plan**, not a single cross-repository patch. Each numbered task is independently reviewable and must be implemented in its owning repository. Do not combine P0 protocol, scheduler, identity, authority, or certification work into one branch.

Snapshot refreshed on 2026-09-03:

| Outcome | State | Evidence |
| --- | --- | --- |
| Native durable-routine foundation | Closed and released | OpenCoven/coven#816 closed by merged PR #896; correctness hardening landed in #906 and shipped in `v0.4.3` |
| Tracker roadmap and drift-check source | Closed and operational | OpenCoven/coven#859, OpenCoven/coven-cave#5220, and Cave beads `cave-hlv.10` / `cave-tmegk` are closed; #900, #901, OpenCoven/coven-cave#5277, and OpenCoven/coven-cave#5278 carry the evidence |
| `coven.automations.v1` specification | Artifacts landed, runtime implementation open | OpenCoven/coven#855 remains open; `spec/coven-automations/v1/` exists |
| Scheduler hardening | Open | OpenCoven/coven#856 |
| Familiar embodiment profile | Open | OpenCoven/familiar-contract#17 |
| Automation authority profile | Open | OpenCoven/coven-threads#29 |
| Dispatch binding and receipts | Open | OpenCoven/coven#857 |
| Certification plane | Open | OpenCoven/coven#858; experimental commit `8f75aed` is not on `main` |
| SDK, Cave, Psyche, docs, organization workflows | Open | OpenCoven/sdk#80, OpenCoven/coven-cave#5217, OpenCoven/psyche#18, OpenCoven/coven-docs#76, OpenCoven/.github#2 |
| Public release containing the hardened foundation | Published | Signed `v0.4.3` at `8baa9c9b722a3a9553c6ed39b7e1ba2296ced95a` is certified across Codex, Claude Code, Copilot CLI, npm packages, and four native platforms |

Current Coven planning baseline: `origin/main` at `8baa9c9b722a3a9553c6ed39b7e1ba2296ced95a`.

## Critical-path graph

```text
Task 1: tracker graph (#859 / coven-cave#5220) [complete]
    |
    v
Task 2: protocol runtime implementation (#855)
    |\
    | +----------------------+
    v                        v
Task 3A: scheduler (#856)   Task 3B: familiar embodiment (familiar-contract#17)
                                      |
                                      v
                              Task 3C: authority profile (coven-threads#29)
    \                        /
     \                      /
      +---- Task 4: Coven dispatch binding and receipts (#857)
                         |
                         v
              Task 5: certification (#858)
                         |
                         v
      Task 6: SDK + Cave + Psyche + docs + organization canaries
                         |
                         v
              Task 7: exact-artifact release
                         |
                         v
              Task 8: program reconciliation (#854)
```

Task 0 is an optional source-build dogfood lane. It may run immediately, but it is never release evidence and must remain local-only with external mutations disabled.

## Program-wide execution rules

1. Run each outcome from its canonical repository and a fresh issue-scoped worktree.
2. Check active claims and open PRs before creating a branch.
3. Acquire the exact issue-keyed claim named by that task from inside its worktree; Task 2, for example, uses `coven claim acquire issue-855`.
4. A Beads assignment does not replace the repository write claim.
5. Keep the primary checkout read-only.
6. Do not start a dependent lane until the prerequisite's merge commit and focused behavioral receipt are both recorded.
7. Do not treat a merged specification, test vector, commit, or PR as production behavior unless the production path executes it.
8. Do not release or enable unattended external mutations before Tasks 4 and 5 pass.
9. Event/webhook triggers, generalized action adapters, multi-host routing, hosted execution, and team federation are P2 and must not enter this plan.
10. Every PR runs its repository's required formatting, lint, test, secret, and privacy gates in addition to the focused commands below.

Before running cross-repository commands, set the local checkout root once:

```bash
export OPENCOVEN_ROOT="/path/to/OpenCoven"
export COVEN_REPO="$OPENCOVEN_ROOT/coven"
export COVEN_CAVE_REPO="$OPENCOVEN_ROOT/coven-cave"
```

## Evidence packet shape

Each lane appends this exact information to its GitHub issue and Bead:

```json
{
  "repository": "owner/repository",
  "issue": 855,
  "baseCommit": "40-character SHA",
  "mergeCommit": "40-character SHA",
  "artifactDigests": {
    "relative/path": "sha256:lowercase-hex"
  },
  "verification": [
    {
      "command": "exact command",
      "exitCode": 0,
      "result": "concise pass count or observable behavior"
    }
  ],
  "knownLimitations": [],
  "downstreamUnblocked": ["owner/repository#number"]
}
```

Use the correct repository and issue values for each lane. Do not include prompts, credentials, terminal dumps, private paths, or raw identity/approval payloads.

## Lane closeout contract

After a lane's focused and repository-wide checks pass:

1. Inspect `git diff --cached --check`, `git status --short`, and the complete staged diff.
2. Obtain the integration authority required by the active familiar/session policy.
3. Create signed-off conventional commits using the subject below.
4. Push the issue branch and open exactly one PR against current upstream `main`.
5. Wait for required CI and review; do not merge without explicit authority.
6. After merge, record the PR URL, merge SHA, exact verification commands/results, artifact digests, and downstream outcomes unblocked.
7. Release the repository claim and retire or preserve the worktree through that repository's lifecycle workflow.

| Lane | Commit subject |
| --- | --- |
| Task 1 Cave graph | `chore: seed the Automations v1 execution graph` |
| Task 1 Coven mapping | `docs: reconcile the Automations v1 tracker mapping` |
| Task 2 | `feat(automations): implement the v1 protocol` |
| Task 3A | `fix(automations): harden scheduler recovery and fencing` |
| Task 3B | `feat: publish familiar embodiment binding v1` |
| Task 3C | `feat: publish automation authority semantics` |
| Task 4 | `feat(automations): bind dispatch authority and receipts` |
| Task 5 | `test(automations): add exact-artifact certification` |
| Task 6A | `feat: add the constrained Automations SDK` |
| Task 6B | `feat: add v1 automation oversight and recovery` |
| Task 6C | `feat: add the Coven automation adapter` |
| Task 6D | `docs: publish the Automations v1 operator guide` |
| Task 6E | `ci: add reusable Automations v1 conformance` |
| Task 8 | `docs: close the Automations v1 evidence rollup` |

---

### Task 0: Start a safe source-build dogfood lane

**Purpose:** Make the existing local-only foundation usable now without misrepresenting it as v1-certified.

**Files:**
- Read: `crates/coven-cli/src/automations/`
- Read: `crates/coven-cli/src/control_plane.rs`
- Read: `crates/coven-cli/src/daemon.rs`

- [ ] **Step 1: Build and test current Coven `main` in an isolated checkout**

```bash
git fetch origin main
git worktree add -b dogfood/automations-v1-source \
  "$HOME/.coven/worktrees/coven-automations-v1-source" origin/main
cd "$HOME/.coven/worktrees/coven-automations-v1-source"
coven claim acquire issue-854
cargo build -p coven-cli --locked
cargo test -p coven-cli automations:: --locked
```

Expected: build succeeds and every filtered automation test passes.

- [ ] **Step 2: Start a disposable daemon using the source binary**

```bash
export COVEN_DOGFOOD_ROOT="$HOME/.coven/dogfood/automations-v1"
export COVEN_HOME="$COVEN_DOGFOOD_ROOT/home"
export COVEN_BIN="$HOME/.coven/worktrees/coven-automations-v1-source/target/debug/coven"
export COVEN_DOGFOOD_PROJECT="$COVEN_DOGFOOD_ROOT/project"
mkdir -p "$COVEN_HOME" "$COVEN_DOGFOOD_PROJECT"
git -C "$COVEN_DOGFOOD_PROJECT" init
"$COVEN_BIN" daemon start
"$COVEN_BIN" daemon status --json | jq -e '.ok and .status == "running"'
```

Expected: `jq` exits 0.

- [ ] **Step 3: Create one paused definition through the authoritative daemon API**

```bash
curl --fail --silent --show-error \
  --unix-socket "$COVEN_HOME/coven.sock" \
  -H 'content-type: application/json' \
  --data-binary @- \
  http://localhost/api/v1/actions <<JSON |
jq -e '.ok and .accepted and .event.payload.routine.id == "cody-dogfood"'
{
  "action": "coven.automations.create",
  "definition": {
    "schemaVersion": 1,
    "id": "cody-dogfood",
    "name": "Cody automation dogfood",
    "status": "PAUSED",
    "rrule": "FREQ=DAILY;BYHOUR=9",
    "timezone": "local",
    "misfire": "latest",
    "overlap": "forbid",
    "timeoutMinutes": 10,
    "runtime": "coven-code",
    "cwd": "$COVEN_DOGFOOD_PROJECT",
    "familiarId": "cody",
    "prompt": "Create a local-only dogfood completion note without external side effects."
  }
}
JSON
```

Expected: `jq` exits 0 and the definition remains paused.

- [ ] **Step 4: Exercise manual run and history through the same daemon**

```bash
curl --fail --silent --show-error \
  --unix-socket "$COVEN_HOME/coven.sock" \
  -H 'content-type: application/json' \
  --data '{"action":"coven.automations.run","id":"cody-dogfood"}' \
  http://localhost/api/v1/actions |
jq -e '.ok and .accepted and .event.payload.status == "running"'

curl --fail --silent --show-error \
  --unix-socket "$COVEN_HOME/coven.sock" \
  -H 'content-type: application/json' \
  --data '{"action":"coven.automations.runs","id":"cody-dogfood"}' \
  http://localhost/api/v1/actions |
jq -e '.ok and .event.payload.runs[0].automationId == "cody-dogfood"'

for attempt in $(seq 1 60); do
  run_json="$(
    curl --fail --silent --show-error \
      --unix-socket "$COVEN_HOME/coven.sock" \
      -H 'content-type: application/json' \
      --data '{"action":"coven.automations.runs","id":"cody-dogfood"}' \
      http://localhost/api/v1/actions
  )"
  status="$(jq -r '.event.payload.runs[0].status' <<<"$run_json")"
  case "$status" in
    succeeded|failed|cancelled|timed_out|ambiguous)
      jq '.event.payload.runs[0]' <<<"$run_json"
      break
      ;;
  esac
  if [ "$attempt" -eq 60 ]; then
    echo "dogfood run did not settle within 120 seconds" >&2
    exit 1
  fi
  sleep 2
done
```

Keep the routine paused so no scheduled occurrence can execute later.

Expected observable result:

```text
POST /api/v1/actions coven.automations.create
  -> Coven SQLite definition

POST /api/v1/actions coven.automations.run
  -> claimed occurrence
  -> automation_runs row with session_id
  -> terminal state derived from session/process evidence
```

- [ ] **Step 5: Stop and preserve only redacted evidence**

```bash
"$COVEN_BIN" daemon stop
```

Record the automation id, run id, terminal state, and exit result. Do not publish prompts, paths, or logs.

**Exit gate:** One local familiar invocation reaches a terminal state through daemon -> runtime -> ledger. This unblocks source dogfooding only; it does not prove the Cave consumer path and does not unblock Task 7.

---

### Task 1: Provision and verify the canonical Beads execution graph

**Owner:** OpenCoven/coven-cave#5220 and OpenCoven/coven#859

**Files:**
- Modify: `OpenCoven/coven-cave/docs/roadmaps/coven-automations-v1.mapping.json`
- Modify: `OpenCoven/coven-cave/docs/roadmaps/coven-automations-v1.md`
- Modify: `OpenCoven/coven/docs/roadmaps/coven-automations-v1.mapping.json`
- Regenerate: `OpenCoven/coven/docs/roadmaps/coven-automations-v1.md`
- Verify: `OpenCoven/coven/docs/roadmaps/drift-check.mjs`
- Never edit directly: `OpenCoven/coven-cave/.beads/issues.jsonl`

- [x] **Step 1: Load Cave's local Beads and repository-coordination skills**

Read both repositories' `AGENTS.md` files and the Beads workflow skill referenced
by the Cave instructions.

- [x] **Step 2: Create or reuse the bootstrap Bead from an exclusively claimed Cave root**

```bash
cd "$COVEN_CAVE_REPO"
bd list --json | jq -r '.[] | select((.external_ref // .externalRef // "") == "https://github.com/OpenCoven/coven-cave/issues/5220") | .id'
```

If exactly one id is returned, reuse it. If none is returned, acquire the repository write claim `issue-5220-bootstrap` for the Beads/Dolt surface only, then run:

```bash
pnpm beads:create --surface shared \
  --title "P0: Seed Coven Automations v1 into Cave's canonical Beads/Dolt execution graph" \
  --description "Execute OpenCoven/coven-cave#5220; GitHub owns public acceptance, Cave Beads owns execution state, and Coven owns production automation state." \
  --type task \
  --priority 1 \
  --labels "program:automations-v1,release-blocker,verification-required,familiar:cody" \
  --external-ref "https://github.com/OpenCoven/coven-cave/issues/5220" \
  --json
```

After creation, resolve and verify the single id:

```bash
export SEED_BEAD_ID="$(
  bd list --json |
    jq -er '[.[] | select((.external_ref // .externalRef // "") == "https://github.com/OpenCoven/coven-cave/issues/5220")] | if length == 1 then .[0].id else error("expected exactly one #5220 Bead") end'
)"
bd show "$SEED_BEAD_ID" --json
```

If the query reports zero or multiple matching Beads, stop and reconcile before any dependency mutation. If the root cannot be exclusively claimed, Task 1 is blocked; do not bypass the managed lifecycle.

- [x] **Step 3: Create the managed writer worktree**

Using the verified `SEED_BEAD_ID`, run:

```bash
pnpm beads:worktrees:create \
  --bead "$SEED_BEAD_ID" \
  --branch "feat/5220-automations-v1-beads" \
  --owner "Cody" \
  --purpose "Seed and verify the Coven Automations v1 Beads/Dolt graph"
```

Use the exact path printed by the command. From that path:

```bash
coven claim acquire issue-5220
pnpm beads:prime
pnpm beads:doctor
pnpm beads:surfaces
bd --version
bd show cave-hlv --json
git rev-parse refs/dolt/data
```

Expected: doctor and surface audit pass, `cave-hlv` exists, and the pre-mutation Dolt OID is recorded.

- [x] **Step 4: Create or reuse one program epic and one Bead per outcome**

Use `pnpm beads:create` after searching by each exact GitHub URL. Create nothing when exactly one matching Bead already exists.

Required mappings:

```text
OpenCoven/coven#854
OpenCoven/coven#859
OpenCoven/coven#855
OpenCoven/coven#856
OpenCoven/familiar-contract#17
OpenCoven/coven-threads#29
OpenCoven/coven#857
OpenCoven/coven#858
OpenCoven/sdk#80
OpenCoven/coven-cave#5217
OpenCoven/psyche#18
OpenCoven/coven-docs#76
OpenCoven/.github#2
```

Do not recreate the closed foundation as pending work. Map OpenCoven/coven#816 as `verified-foundation` with PR #896 and merge commit `0d8c2004c3557019e39e5e4db70ae34c9d49a65a`.

- [x] **Step 5: Add dependencies one edge at a time**

Before each edge:

```bash
bd dep --help
```

Add each edge with blocked-first semantics:

```bash
bd dep add "$BLOCKED_BEAD_ID" --blocked-by "$BLOCKER_BEAD_ID" --json
bd dep list "$BLOCKED_BEAD_ID" --direction down --json
bd dep list "$BLOCKER_BEAD_ID" --direction up --json
bd ready --json
```

For each edge, set `BLOCKED_BEAD_ID` and `BLOCKER_BEAD_ID` from the single ids just written to the reviewed mapping. The mutation receipt must show `issue_id` equal to the blocked id and `depends_on_id` equal to the blocker id.

The verified graph must match the graph at the top of this plan. Reverse any edge that makes a prerequisite wait on its consumer.

- [x] **Step 6: Synchronize and prove durability**

```bash
pnpm beads:doctor
pnpm beads:surfaces
pnpm beads:sync
git rev-parse refs/dolt/data
bd ready --json
```

Expected: post-sync OID differs when mutations occurred; a fresh `bd` read returns every mapped id and dependency.

- [x] **Step 7: Update both reviewed roadmap mappings**

Replace every pending `bead.id` with its real id, update dispositions and sync metadata in the Cave mapping, and open its reviewed PR. Then create the Coven issue worktree:

```bash
cd "$COVEN_REPO"
git fetch origin main
git worktree add -b docs/859-automations-v1-mapping \
  "$HOME/.coven/worktrees/coven-issue-859" origin/main
cd "$HOME/.coven/worktrees/coven-issue-859"
coven claim acquire issue-859
node docs/roadmaps/drift-check.mjs --render
node docs/roadmaps/drift-check.mjs --strict
node docs/roadmaps/drift-check.mjs --selftest
```

Expected: strict drift check exits 0 with no `W010` pending-provisioning warnings.

- [x] **Step 8: Open reviewed PRs and attach the evidence packet**

Required receipts: pre/post Dolt OIDs, Beads version/schema, exact mapping, `bd dep list` output summary, `bd ready --json` summary, drift-check exit codes, PR URLs, and merge SHAs.

**Exit gate:** #854/#859 and every P0/P1 child map one-to-one to durable Beads; dependency direction is proven; strict drift check is green.

**Completion receipt (2026-09-03):** OpenCoven/coven-cave#5277 and
OpenCoven/coven-cave#5278 seeded and terminally reconciled the graph;
OpenCoven/coven#900 and OpenCoven/coven#901 landed the Coven mapping and drift
controls; OpenCoven/coven-cave#5220, OpenCoven/coven#859, `cave-tmegk`, and
`cave-hlv.10` are closed. A checksum-verified Beads 1.3.0-rc.1 client read the
schema-v66 store, `pnpm beads:sync` completed pull and push, and #855 plus
OpenCoven/familiar-contract#17 are the ready implementation outcomes.

---

### Task 2: Implement `coven.automations.v1` in the Rust authority layer

**Owner:** OpenCoven/coven#855

**Files:**
- Create: `crates/coven-cli/src/automations/contract/mod.rs`
- Create: `crates/coven-cli/src/automations/contract/types.rs`
- Create: `crates/coven-cli/src/automations/contract/error.rs`
- Create: `crates/coven-cli/src/automations/contract/canonical_json.rs`
- Create: `crates/coven-cli/src/automations/contract/commands.rs`
- Create: `crates/coven-cli/src/automations/contract/events.rs`
- Create: `crates/coven-cli/src/automations/contract/migration.rs`
- Create: `scripts/package-automations-protocol.mjs`
- Create: `scripts/package-automations-protocol.test.mjs`
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release-github.yml`
- Modify: `docs/reference/releasing.md`
- Modify: `crates/coven-cli/src/automations/mod.rs`
- Modify: `crates/coven-cli/src/automations/store.rs`
- Modify: `crates/coven-cli/src/automations/occurrences.rs`
- Modify: `crates/coven-cli/src/automations/runs.rs`
- Modify: `crates/coven-cli/src/automations/runner.rs`
- Modify: `crates/coven-cli/src/control_plane.rs`
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `crates/coven-cli/src/store.rs`
- Test: colocated `#[cfg(test)]` modules plus `crates/coven-cli/tests/automation_protocol.rs`
- Consume unchanged: `spec/coven-automations/v1/*`

- [ ] **Step 1: Create the issue worktree and record the schema baseline**

```bash
git fetch origin main
git worktree add -b feat/855-automations-v1-protocol \
  "$HOME/.coven/worktrees/coven-issue-855" origin/main
cd "$HOME/.coven/worktrees/coven-issue-855"
coven claim acquire issue-855
mkdir -p "$HOME/.coven/artifacts/coven-issue-855"
shasum -a 256 spec/coven-automations/v1/* \
  > "$HOME/.coven/artifacts/coven-issue-855/spec-sha256.txt"
```

Expected: the checkout is clean and every current protocol artifact has a recorded digest.

- [ ] **Step 2: Write failing schema-to-Rust round-trip tests**

Tests must load `test-vectors.json`, deserialize valid objects into typed `#[serde(deny_unknown_fields)]` structures, reject every invalid/unknown-field vector, and serialize valid objects without field loss.

Run:

```bash
cargo test -p coven-cli automation_protocol --locked
```

Expected before implementation: failure because the contract module and migration are absent.

- [ ] **Step 3: Implement the typed projection and canonical digests**

Map every schema object exactly: definition, occurrence, run, attempt, receipt, command/response, error, and event. Implement RFC 8785-compatible canonical serialization for the artifact's supported JSON domain and SHA-256 digest generation. Reject unsupported profile/variant values with the typed codes in `error-envelope.schema.json`.

- [ ] **Step 4: Write failing adoption and revision tests**

Cover:

```text
same adoptionKey + same command/payload -> replay first result, no second event
same adoptionKey + different command/payload -> ADOPTION_REPLAY_MISMATCH
matching expectedRevision -> one revision increment
stale expectedRevision -> REVISION_CONFLICT, no mutation
domain failure -> ok=false, accepted=false, status=rejected
```

- [ ] **Step 5: Add transactional command adoption**

Persist the adoption key and committed outcome in the same SQLite transaction as the state change. Route legacy `coven.automations.*` control actions and new v1 envelopes through the same handlers.

- [ ] **Step 6: Write failing migration tests**

Create a pre-contract database containing definitions, occurrences, and runs. Assert:

```text
row counts unchanged
legacy definition bytes unchanged
revision 1 and digest sidecars populated
historical occurrences/runs pin revision 1
no historical receipt fabricated
second migration run changes zero rows
```

- [ ] **Step 7: Implement the idempotent migration**

Use additive columns/tables and preserve all existing data. Hard deletion becomes tombstoning for contract definitions; retained history remains queryable.

- [ ] **Step 8: Write failing changefeed reducer tests**

Cover duplicate delivery, reconnect from cursor, out-of-order append refusal, compacted snapshot plus tail replay, and deterministic projection equality.

- [ ] **Step 9: Implement transactional event append and read APIs**

Append state mutation and event sequence in one transaction. Expose read/subscribe semantics through the existing daemon transport without trusting clients to author lifecycle state.

- [ ] **Step 10: Produce the deterministic protocol bundle**

Implement `scripts/package-automations-protocol.mjs` so it copies only
`spec/coven-automations/v1/` contract files into a lexically ordered archive,
writes a manifest containing the source commit, each file's SHA-256 digest, and
a `contractContentSha256` computed only from the ordered relative-path/digest
pairs, normalizes archive timestamps and ownership, and refuses a dirty or
mismatched input tree. The content digest deliberately excludes source commit,
archive metadata, and manifest metadata so later release candidates can prove
the contract bytes are unchanged even though their source-bound bundle digests
differ. With `SOURCE_COMMIT="$(git rev-parse HEAD)"`, the output is
`coven-automations-v1-contract-${SOURCE_COMMIT}.tar.gz`.

Run:

```bash
export AUTOMATIONS_ARTIFACT_DIR="$HOME/.coven/artifacts/coven-issue-855/contract"
node --test scripts/package-automations-protocol.test.mjs
node scripts/package-automations-protocol.mjs \
  --output "$AUTOMATIONS_ARTIFACT_DIR"
export COVEN_PROTOCOL_BUNDLE="$(
  find "$AUTOMATIONS_ARTIFACT_DIR" -maxdepth 1 -type f \
    -name 'coven-automations-v1-contract-*.tar.gz' -print -quit
)"
test -n "$COVEN_PROTOCOL_BUNDLE"
shasum -a 256 "$COVEN_PROTOCOL_BUNDLE" |
  tee "$AUTOMATIONS_ARTIFACT_DIR/protocol-bundle.sha256"
jq -er '.contractContentSha256' \
  "$AUTOMATIONS_ARTIFACT_DIR/manifest.json" |
  tee "$AUTOMATIONS_ARTIFACT_DIR/protocol-content.sha256"
```

Update `.github/workflows/ci.yml` to upload the bundle and manifest as the
artifact `coven-automations-v1-contract-${{ github.sha }}` on the exact commit.
Update `.github/workflows/release-github.yml` to rebuild the same deterministic
bundle from the tagged source and publish it as an additional canonical release
asset. Keep `SHA256SUMS` scoped to the four native archives and verify the
protocol bundle against its manifest digest separately. Document the added
asset and verification command in `docs/reference/releasing.md`.

- [ ] **Step 11: Run protocol verification**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p coven-cli automation_protocol --locked
cargo test --workspace --locked
node --test scripts/package-automations-protocol.test.mjs
python3 scripts/check-secrets.py
git add crates/coven-cli/src/automations \
  crates/coven-cli/src/control_plane.rs \
  crates/coven-cli/src/api.rs \
  crates/coven-cli/src/store.rs \
  crates/coven-cli/tests/automation_protocol.rs \
  scripts/package-automations-protocol.mjs \
  scripts/package-automations-protocol.test.mjs \
  .github/workflows/ci.yml \
  .github/workflows/release-github.yml \
  docs/reference/releasing.md
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

Expected: all commands exit 0.

- [ ] **Step 12: Prove two external packed-artifact canaries**

Download the exact-commit CI artifact rather than copying from the Coven source
tree:

```bash
export COVEN_PROTOCOL_RUN_ID="$(
  gh run list \
    --repo OpenCoven/coven \
    --workflow ci.yml \
    --commit "$(git rev-parse HEAD)" \
    --status success \
    --limit 1 \
    --json databaseId,headSha \
    --jq '.[0].databaseId'
)"
test -n "$COVEN_PROTOCOL_RUN_ID"
export COVEN_PROTOCOL_SHA256="$(
  awk 'NR == 1 { print $1 }' \
    "$AUTOMATIONS_ARTIFACT_DIR/protocol-bundle.sha256"
)"
[[ "$COVEN_PROTOCOL_SHA256" =~ ^[0-9a-f]{64}$ ]]
gh run download "$COVEN_PROTOCOL_RUN_ID" \
  --repo OpenCoven/coven \
  --name "coven-automations-v1-contract-$(git rev-parse HEAD)" \
  --dir "$AUTOMATIONS_ARTIFACT_DIR/downloaded"
printf '%s  %s\n' \
  "$COVEN_PROTOCOL_SHA256" \
  "$AUTOMATIONS_ARTIFACT_DIR/downloaded/coven-automations-v1-contract-$(git rev-parse HEAD).tar.gz" |
  shasum -a 256 -c -
```

SDK and Cave must each run their contract verifier against that downloaded
bundle path and expected digest. Record the CI run URL, bundle digest, manifest
digest, contract-content digest, source commit, and both canary results. No canary may import
`OpenCoven/coven/spec/` or another source-relative path.

**Exit gate:** all #855 acceptance criteria pass; v1 commands are adopted transactionally; domain failures reject truthfully; changefeed replay deterministically rehydrates state; legacy data migrates without deletion.

---

### Task 3A: Harden scheduler time, fencing, retry, cancel, and recovery

**Owner:** OpenCoven/coven#856

**Depends on:** Task 2

**Files:**
- Create: `crates/coven-cli/src/automations/clock.rs`
- Create: `crates/coven-cli/src/automations/leader.rs`
- Create: `crates/coven-cli/src/automations/retry.rs`
- Create: `crates/coven-cli/src/automations/cancellation.rs`
- Create: `crates/coven-cli/src/automations/diagnostics.rs`
- Modify: `crates/coven-cli/src/automations/schedule.rs`
- Modify: `crates/coven-cli/src/automations/daemon_tick.rs`
- Modify: `crates/coven-cli/src/automations/occurrences.rs`
- Modify: `crates/coven-cli/src/automations/runner.rs`
- Modify: `crates/coven-cli/src/automations/health.rs`
- Modify: `crates/coven-cli/src/control_plane.rs`
- Test: `crates/coven-cli/tests/automation_scheduler.rs`

- [ ] **Step 1: Create and claim `feat/856-automation-scheduler`**

Use worktree `$HOME/.coven/worktrees/coven-issue-856` and claim `issue-856`.

- [ ] **Step 2: Write virtual-time failures first**

Test daily/weekly RRULEs across IANA zones, DST spring gaps, fall folds, host-zone change, backward/forward wall-clock jumps, suspend/resume, startup, shutdown, and definition revision changes.

- [ ] **Step 3: Inject clock and wake abstractions**

Production uses system clock/wake implementations; tests use a manually advanced clock. Remove scheduler correctness assertions that depend on sleeping real wall time.

- [ ] **Step 4: Write retry-classification failures**

Prove only configured retryable deterministic failures retry, attempt numbers never repeat, backoff survives restart, authority/capability failures do not retry, and ambiguous side effects enter `recovery_required`.

- [ ] **Step 5: Implement persisted retry and quarantine**

Store next eligible time, prior attempt disposition, backoff state, retry budget, and quarantine reason in the authoritative ledger.

- [ ] **Step 6: Write cancel/timeout race failures**

Cover cancel-before-dispatch, cancel during launch, cancel while running, timeout concurrent with completion, runtime kill failure, daemon death during cancellation, and late terminal evidence.

- [ ] **Step 7: Implement convergent cancellation**

Cancellation remains requested until runtime acknowledgment or reconciliation. One compare-and-set settlement wins; late evidence cannot regress terminal state.

- [ ] **Step 8: Write competing-scheduler failures**

Start two local scheduler processes against the same database. Assert one leader generation and one dispatch per occurrence fence.

- [ ] **Step 9: Implement local leader fencing**

Persist monotonic leader generation and reject stale leaders before dispatch. Never rely only on an in-memory mutex.

- [ ] **Step 10: Run the crash matrix**

Inject process termination:

```text
before occurrence insert
after occurrence insert
after claim
after run acceptance
before runtime ownership
after runtime ownership
during event append
during output delivery
during cancellation
during receipt preparation
```

Expected: no silent loss, duplicate dispatch, false success, or automatic retry of ambiguous mutation.

- [ ] **Step 11: Add operator diagnosis and reconciliation commands**

Every nonterminal/stuck state must produce a typed diagnosis and one safe action: wait, cancel, quarantine, mark deterministic failure, or explicitly approved recovery. No runbook may require raw SQLite edits.

- [ ] **Step 12: Verify**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p coven-cli automation_scheduler --locked
cargo test --workspace --locked
python3 scripts/check-secrets.py
git add crates/coven-cli/src/automations \
  crates/coven-cli/src/control_plane.rs \
  crates/coven-cli/tests/automation_scheduler.rs
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

**Exit gate:** virtual-time, DST, retry, cancel, dual-scheduler, and crash suites pass with one authoritative terminal outcome per occurrence.

---

### Task 3B: Publish the universal familiar embodiment profile

**Owner:** OpenCoven/familiar-contract#17

**Depends on:** closed foundation #816; may run in parallel with Tasks 2 and 3A

**Files:**
- Create: `schemas/familiar-embodiment-binding.schema.json`
- Create: `tests/conformance/embodiment-bindings/positive/*.json`
- Create: `tests/conformance/embodiment-bindings/negative/*.json`
- Modify: `SPEC.md`
- Modify: `validators/validate.js`
- Modify: `tests/conformance/run-conformance.sh`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Create and claim `feat/17-embodiment-binding`**

Use worktree `$HOME/.coven/worktrees/familiar-contract-issue-17` and claim `issue-17`.

- [ ] **Step 2: Write failing positive and negative vectors**

Cover exact root/revision/digest binding, alias resolution, stale revision, revoked revision, retired/restored identity, same-familiar lineage, fork/succession, historical rehydration, unknown fields, and a concurrent revocation between resolve and commit.

- [ ] **Step 3: Publish `familiar.embodiment_binding.v1`**

The schema must bind `bindingId`, `familiarRootId`, `identityRevisionId`, lineage position, digest, validity window, resolution evidence, and verification state. It must not contain scheduler or protected-action decisions.

- [ ] **Step 4: Extend the validator and conformance runner**

Reject stale, revoked, ambiguous, or malformed bindings; preserve explicit degraded historical verification without treating it as current dispatch authority.

- [ ] **Step 5: Verify**

```bash
npm ci
npm test
npm run validate:examples
```

Expected: all existing and new conformance cases pass.

**Exit gate:** a digest-pinned profile and golden vectors are merged at an immutable commit consumed by Task 4.

---

### Task 3C: Publish automation authority and approval semantics

**Owner:** OpenCoven/coven-threads#29

**Depends on:** Task 3B

**Files:**
- Create: `crates/coven-threads-core/src/automation_authority.rs`
- Create: `crates/coven-threads-core/tests/automation_authority.rs`
- Create: `specs/AUTOMATION-AUTHORITY-V1.md`
- Create: `e2e/automation-authority-v1.json`
- Modify: `crates/coven-threads-core/src/lib.rs`
- Modify: `docs/authority-model.md`

- [ ] **Step 1: Create and claim `feat/29-automation-authority`**

Use worktree `$HOME/.coven/worktrees/coven-threads-issue-29` and claim `issue-29`.

- [ ] **Step 2: Write failing decision vectors**

Cover permit, require approval, degrade to proposal, reject, expired approval, replayed approval, revoked capability, stale familiar binding, runtime capability downgrade, confused deputy, approval invalidated by definition revision, and TOCTOU revocation before dispatch.

- [ ] **Step 3: Implement versioned request and decision types**

Bind exact principal, familiar embodiment, definition/action digest, occurrence/run, requested scope, runtime capabilities, policy revision, side-effect class, and decision nonce.

- [ ] **Step 4: Implement replay-safe approval consumption**

Approval must be operation-specific, expiring, revocable, single-consumption, and invalidated by any bound-input change.

- [ ] **Step 5: Implement degrade-to-proposal**

An unavailable authority service or insufficient grant may produce a non-executing proposal only when policy allows. A proposal can never be interpreted as permit.

- [ ] **Step 6: Verify**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

**Exit gate:** immutable vectors prove permit/approval/proposal/reject behavior and are consumable by Task 4.

---

### Task 4: Bind every Coven dispatch to identity, authority, approval, and receipts

**Owner:** OpenCoven/coven#857

**Depends on:** Tasks 2, 3B, and 3C

**Companion-profile checkpoint:** the first independently publishable slice is
the separately advertised `coven.automations.authority.v1` profile under
`spec/coven-automations/authority/v1/`. It travels only through the frozen base
run `extensions` bag, carries an immutable execution binding plus
receipt-correlated authority evidence, and pins the reviewed Familiar Contract
and Threads commits and file digests in `upstream-artifacts.json`. Portable
Node validation and the Rust projection/adapter seam are in scope; scheduler
dispatch, persistence, and production adapters remain in Steps 3–9 below.
Generic base consumers preserve the value opaquely. Runtime Authority
consumers advertise all required profiles/capabilities and fail closed on
missing adapters or trusted state.

**Files:**
- Create: `crates/coven-cli/src/automations/binding.rs`
- Create: `crates/coven-cli/src/automations/authority.rs`
- Create: `crates/coven-cli/src/automations/approval.rs`
- Create: `crates/coven-cli/src/automations/receipts.rs`
- Modify: `crates/coven-cli/src/automations/runner.rs`
- Modify: `crates/coven-cli/src/automations/runs.rs`
- Modify: `crates/coven-cli/src/automations/occurrences.rs`
- Modify: `crates/coven-cli/src/automations/contract/types.rs`
- Modify: `crates/coven-cli/src/control_plane.rs`
- Modify: `crates/coven-cli/src/api.rs`
- Test: `crates/coven-cli/tests/automation_authority.rs`

- [ ] **Step 1: Create and claim `feat/857-automation-authority-binding`**

Use worktree `$HOME/.coven/worktrees/coven-issue-857` and claim `issue-857`.

- [ ] **Step 2: Pin exact upstream artifacts**

Record immutable Familiar Contract and Threads commits plus SHA-256 digests of every consumed schema/vector. Do not copy hand-maintained parallel types.

- [ ] **Step 3: Write fail-closed dispatch tests**

Prove no runtime call occurs for missing principal, stale/revoked familiar revision, insufficient capability, unavailable authority, expired/replayed approval, changed definition revision, changed runtime capability, or mismatched adoption key.

- [ ] **Step 4: Implement pre-dispatch binding transaction**

Before consequential runtime work, atomically persist adopted request, definition revision/digest, occurrence fence, principal, embodiment binding, authority decision, approval reference/consumption, and runtime descriptor.

- [ ] **Step 5: Write TOCTOU tests**

Pause between decision and dispatch, revoke each bound input, then resume. Expected: revalidation refuses dispatch and records a typed rejection.

- [ ] **Step 6: Implement side-effect gating**

Default to local-read/local-write limits. External mutation requires exact policy and approval; irreversible external mutation remains refused in v1 unless explicitly ratified by both authority profile and release certification.

- [ ] **Step 7: Write receipt verification failures**

Cover canonical digest verification, tamper detection, wrong definition digest, wrong fence generation, wrong principal/familiar binding, replayed decision nonce, redaction, retention, delivery failure, and unauthenticated producer distinction.

- [ ] **Step 8: Implement immutable receipts**

Write one terminal receipt that pins all exercised authority and outcome evidence. Receipt persistence and terminal settlement must be transactional or leave an explicit recoverable state; clients and runtimes cannot forge it.

- [ ] **Step 9: Prove direct and Psyche-ready path equivalence**

The direct familiar invocation and future Psyche adapter must call the same binding/authority/receipt service. No string-only `familiar_id` fallback is allowed for v1 dispatch.

- [ ] **Step 10: Verify**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p coven-cli automation_authority --locked
cargo test --workspace --locked
python3 scripts/check-secrets.py
git add crates/coven-cli/src/automations \
  crates/coven-cli/src/control_plane.rs \
  crates/coven-cli/src/api.rs \
  crates/coven-cli/tests/automation_authority.rs
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

**Exit gate:** every dispatch is fully bound, every negative vector fails before runtime effects, and every terminal run has an independently verifiable privacy-classified receipt.

---

### Task 5: Land independent conformance, chaos, SLO, and diagnostics

**Owner:** OpenCoven/coven#858

**Depends on:** Tasks 2, 3A, and 4

**Files:**
- Create: `conformance/automations/manifest.json`
- Create: `conformance/automations/runner/conformance.mjs`
- Create: `conformance/automations/runner/conformance.test.mjs`
- Create: `conformance/automations/runner/lib/clock.mjs`
- Create: `conformance/automations/runner/lib/dispatch.mjs`
- Create: `conformance/automations/runner/lib/doctor.mjs`
- Create: `conformance/automations/runner/lib/evaluate.mjs`
- Create: `conformance/automations/runner/lib/model.mjs`
- Create: `conformance/automations/runner/lib/ops.mjs`
- Create: `conformance/automations/runner/lib/redact.mjs`
- Create: `conformance/automations/runner/lib/schema.mjs`
- Create: `conformance/automations/scenarios/**/*.json`
- Create: `conformance/automations/schemas/*.json`
- Create: `conformance/automations/slo/slo.v1.json`
- Create: `conformance/automations/vectors/**/*.json`
- Create: `scripts/agent-bootstrap`
- Create: `scripts/agent-check`
- Modify: `.github/workflows/ci.yml`
- Modify: `spec/coven-automations/v1/conformance-manifest.json`

- [ ] **Step 1: Create and claim `feat/858-automations-conformance`**

Use worktree `$HOME/.coven/worktrees/coven-issue-858` and claim `issue-858`.

- [ ] **Step 2: Treat closed PR #882 as reference, not landed behavior**

Inspect commit `8f75aed` and its review history. Reuse only code that still matches the merged Tasks 2-4 contract; rewrite stale schemas/vectors rather than resurrecting them unchanged.

- [ ] **Step 3: Write runner self-tests before production scenarios**

Prove schema validation, deterministic virtual clock, duplicate/reorder injection, process-kill injection, redaction, profile separation, report schema, and nonzero exit on gate failure.

- [ ] **Step 4: Implement separate conformance profiles**

Required profiles:

```text
structural
scheduler
authority
continuity
privacy
interoperability
full
```

Never emit one generic `compliant: true` in place of profile results.

- [ ] **Step 5: Encode all golden scenarios**

At minimum: paused activation, twice-daily schedule, spring gap, fall fold, restart before due, restart after claim, sleep/misfire collapse, competing schedulers, adopted run replay, unavailable runtime, ambiguous side effect, cancel/timeout race, delivery failure, revoked familiar, approval consume/replay/expiry, capability downgrade, quarantine, changefeed reconnect, retention/redaction, legacy import, and direct/Psyche equivalence.

- [ ] **Step 6: Add operator doctor projections**

Diagnostics must explain no leader, stale lease, backlog, repeated failure, quarantine, ambiguous outcome, delivery/receipt failure, and changefeed lag without exposing prompts or requiring raw database mutation.

- [ ] **Step 7: Ratify measurable SLO gates**

Bind each threshold to the exact release artifact and include observed values in failures. Do not use scheduler-jitter-sensitive wall-clock assertions where deterministic virtual time can prove the behavior.

- [ ] **Step 8: Wire CI**

Runner mode is separate from reported profiles: `fast` mode runs deterministic schema/state profiles on PRs; `full` mode runs every profile, including integration, restart, packed-artifact, and supported-platform scenarios, before release. Reports upload as machine-readable artifacts.

- [ ] **Step 9: Verify**

```bash
node --test conformance/automations/runner/conformance.test.mjs
mkdir -p "$HOME/.coven/artifacts/coven-issue-858"
node conformance/automations/runner/conformance.mjs \
  --mode fast \
  --report "$HOME/.coven/artifacts/coven-issue-858/automations-fast.json"
node conformance/automations/runner/conformance.mjs \
  --mode full \
  --report "$HOME/.coven/artifacts/coven-issue-858/automations-full.json"
jq -e '.profiles | length >= 6' \
  "$HOME/.coven/artifacts/coven-issue-858/automations-full.json"
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python3 scripts/check-secrets.py
git add conformance/automations \
  scripts/agent-bootstrap \
  scripts/agent-check \
  .github/workflows/ci.yml \
  spec/coven-automations/v1/conformance-manifest.json
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

Expected: all commands exit 0 and reports contain separate profile results.

**Exit gate:** the exact candidate artifact passes deterministic, crash, duplicate, authority, privacy, interoperability, and SLO gates with machine-readable reports.

---

### Task 6: Graduate supported consumers in parallel

**Depends on:** Task 5 for mutation/recovery graduation. Read-only canary preparation may start after Task 2.

#### Task 6A: SDK read, verify, subscribe, then mutations

**Owner:** OpenCoven/sdk#80

**Files:**
- Create: `packages/coven/src/automations/types.ts`
- Create: `packages/coven/src/automations/client.ts`
- Create: `packages/coven/src/automations/subscription.ts`
- Create: `packages/coven/src/automations/verify.ts`
- Create: `packages/coven/src/automations/errors.ts`
- Create: `packages/coven/src/automations/index.ts`
- Modify: `packages/coven/src/index.ts`
- Modify: `packages/coven/package.json`
- Test: colocated `*.test.ts` plus packed-package canaries under `tests/`

- [ ] Pin the exact protocol/conformance artifact digest.
- [ ] Generate or mechanically verify types; do not hand-diverge from schemas.
- [ ] Ship read/health/history/receipt verification and duplicate-safe subscriptions first.
- [ ] Add lifecycle mutations with adoption keys and expected revisions.
- [ ] Keep run/cancel/retry/approval APIs disabled until Task 5's immutable report is supplied.
- [ ] Run:

```bash
corepack pnpm@10.11.1 typecheck
corepack pnpm@10.11.1 test
corepack pnpm@10.11.1 verify:contracts
corepack pnpm@10.11.1 verify:package
corepack pnpm@10.11.1 pack:public
```

**Exit gate:** a packed consumer verifies receipts and resumes a changefeed without direct SQLite, Codex, Cave, or runtime access.

#### Task 6B: Cave oversight, approvals, recovery, and facade retirement

**Owner:** OpenCoven/coven-cave#5217

**Files:**
- Modify: `src/lib/server/coven-automations-client.ts`
- Replace generated types in: `src/lib/coven-automations-types.ts`
- Modify: `src/components/automations-view.tsx`
- Modify: `src/components/automations/live-run-card.tsx`
- Modify: `src/components/automations/cron-detail-panel.tsx`
- Create: `src/components/automations/approval-panel.tsx`
- Create: `src/components/automations/receipt-panel.tsx`
- Create: `src/components/automations/recovery-panel.tsx`
- Create: `tests/automations-v1-release.spec.ts`
- Modify: `src/lib/automations/daemon-projection.ts`
- Retire after migration: `src/lib/coven-automations-facade.ts`
- Retire after migration: `src/app/api/codex-automations/**`

- [ ] Resolve the single `cave-oversight` Bead from `docs/roadmaps/coven-automations-v1.mapping.json`, create its managed worktree, and claim the issue:

```bash
cd "$COVEN_CAVE_REPO"
export CAVE_AUTOMATIONS_BEAD_ID="$(
  jq -er '
    [.outcomes[] |
      select(.github == "OpenCoven/coven-cave#5217") |
      .beadId] |
    if length == 1 and .[0] != null
    then .[0]
    else error("expected one provisioned #5217 Bead")
    end
  ' docs/roadmaps/coven-automations-v1.mapping.json
)"
pnpm beads:worktrees:create \
  --bead "$CAVE_AUTOMATIONS_BEAD_ID" \
  --branch "feat/5217-coven-automations-v1" \
  --owner "Cody" \
  --purpose "Ship Cave oversight and recovery for Coven Automations v1"
export CAVE_AUTOMATIONS_WORKTREE="$COVEN_CAVE_REPO/.worktrees/5217-coven-automations-v1"
test -d "$CAVE_AUTOMATIONS_WORKTREE"
cd "$CAVE_AUTOMATIONS_WORKTREE"
coven claim acquire issue-5217
```

- [ ] Consume packed v1 artifacts and use adopted commands.
- [ ] Render definition -> occurrence -> run -> attempt -> receipt from changefeed state.
- [ ] Show exact familiar, authority, approval, runtime, stale/degraded, and receipt evidence.
- [ ] Refuse blind retry of ambiguous effects; expose only state-valid cancel/reconcile/quarantine actions.
- [ ] Preserve daemon-unavailable failure; never restore direct Codex execution.
- [ ] Keep legacy import, then remove Codex naming/routes only after migration and canaries pass.
- [ ] Run:

```bash
pnpm typecheck
node --experimental-strip-types --test src/lib/server/coven-automations-client.test.ts
pnpm test:app
pnpm lint
pnpm build
```

**Exit gate:** Cave renders one live Coven truth and can approve/recover safely
without authoring lifecycle state. Close OpenCoven/coven-cave#5217 and its Bead
after the implementation PR and pre-release exact-bundle canary pass. Task 7
owns a separate post-release acceptance receipt and does not reopen this
implementation outcome.

#### Task 6C: Psyche adapter

**Owner:** OpenCoven/psyche#18

**Files:**
- Create: `crates/psyche-runtime/src/coven_automation.rs`
- Create: `crates/psyche-runtime/tests/coven_automation.rs`
- Modify: `crates/psyche-runtime/src/lib.rs`
- Modify: `docs/CONFIGURATION.md`

- [ ] Define versioned invocation, event, result, and error types.
- [ ] Adopt invocations idempotently and preserve Coven run/occurrence ownership.
- [ ] Narrow authority per lane; never broaden principal/familiar grants.
- [ ] Compose Psyche evidence into the Coven receipt.
- [ ] Prove cancel/restart/replay/ambiguous recovery without duplicate orchestration.
- [ ] Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

**Exit gate:** direct and Psyche runs share identity, authority, adoption, terminal, and receipt semantics; no Psyche API edits schedules.

#### Task 6D: Public documentation

**Owner:** OpenCoven/coven-docs#76

**Files:**
- Modify: `content/docs/reference/automations.mdx`
- Create: `content/docs/automations/architecture.mdx`
- Create: `content/docs/automations/quickstart.mdx`
- Create: `content/docs/automations/protocol.mdx`
- Create: `content/docs/automations/scheduling.mdx`
- Create: `content/docs/automations/authority.mdx`
- Create: `content/docs/automations/lifecycle.mdx`
- Create: `content/docs/automations/operations.mdx`
- Create: `content/docs/automations/migration.mdx`
- Create: `content/docs/automations/integrations.mdx`
- Create: `content/docs/automations/security.mdx`
- Create: `content/docs/automations/compatibility.mdx`
- Create: `content/docs/automations/meta.json`
- Modify: `content/docs/meta.json`
- Modify: `docs/site-manifest.json`
- Modify: `docs/source-lock.json`

- [ ] Generate examples from exact released schemas/vectors.
- [ ] Document safe defaults, ambiguity, cancellation, recovery, authority, privacy, and source-preserving migration.
- [ ] Never recommend raw SQLite repair or blind retry.
- [ ] Publish the exact artifact/content hash.
- [ ] Run:

```bash
pnpm check
pnpm build
pnpm test:smoke
pnpm check:production
```

**Exit gate:** deployed docs resolve every v1 operator and integration path at a content hash matching the candidate.

#### Task 6E: Organization workflows and compatibility ledger

**Owner:** OpenCoven/.github#2

**Files:**
- Create: `workflow-templates/opencoven-agent-readiness.yml`
- Create: `.github/workflows/automations-contract-producer.yml`
- Create: `.github/workflows/automations-consumer-canary.yml`
- Create: `.github/workflows/automations-conformance.yml`
- Create: `.github/workflows/automations-roadmap-drift.yml`
- Create: `.github/workflows/automation-evidence-packet.yml`
- Create: `.github/workflows/automations-compatibility-ledger.yml`
- Create: `schemas/automations-repository-manifest.v1.schema.json`

- [ ] Pin every action and cross-repository artifact by immutable SHA/digest.
- [ ] Use least-privilege permissions and no secrets for fork PRs.
- [ ] Keep conformance profiles separate in outputs.
- [ ] Generate a public compatibility ledger artifact consumed by docs.
- [ ] Schedule stale-reference checks with issue deduplication.
- [ ] Prove Coven, Familiar Contract, Threads, SDK, Cave, Psyche, and docs consume the workflows.

**Exit gate:** every supported repository reports producer/consumer revision, profile results, and drift without leaking Beads or runtime payloads.

**Exit gate:** the packed SDK, Cave, Psyche, docs, and organization workflows all consume the same immutable Task 5 candidate and report compatible profile evidence.

---

### Task 7: Cut the exact-artifact Coven release

**Owner:** OpenCoven/coven#854 release gate

**Depends on:** merged and verified implementation outcomes from Tasks 1-6.
The post-release Cave acceptance is part of this task and is not a prerequisite
for starting it.

**Files:**
- Modify: `CHANGELOG.md`
- Verify: `.github/workflows/release-npm.yml`
- Verify: `.github/workflows/release-github.yml`
- Produce outside git: redacted certification reports, conformance reports, and the deterministic protocol bundle

- [ ] **Step 1: Generate the go/no-go manifest**

Create the candidate packet outside the repository:

```bash
cd "$COVEN_REPO"
coven claim status
gh pr list --state open --repo OpenCoven/coven
git fetch origin main
git worktree add -b release/854-automations-v1 \
  "$HOME/.coven/worktrees/coven-issue-854-release" origin/main
cd "$HOME/.coven/worktrees/coven-issue-854-release"
coven claim acquire issue-854
export RELEASE_CANDIDATE_SHA="$(git rev-parse HEAD)"
export RELEASE_PACKET_DIR="$HOME/.coven/artifacts/coven-release-candidate-${RELEASE_CANDIDATE_SHA:0:12}"
test ! -e "$RELEASE_PACKET_DIR"
mkdir -p "$RELEASE_PACKET_DIR"
```

Write `$RELEASE_PACKET_DIR/go-no-go.json` with schema `coven.automations-v1.release-candidate/v1`, the candidate commit, exact merge SHAs and SHA-256 evidence digests for lanes `1`, `2`, `3A`, `3B`, `3C`, `4`, `5`, `6A`, `6B`, `6C`, `6D`, and `6E`, separate conformance profile results, known limitations, P2 exclusions, and an approval object initially set to `pending`. Reject an incomplete packet:

```bash
jq -e \
  --arg commit "$RELEASE_CANDIDATE_SHA" \
  '
  .schema == "coven.automations-v1.release-candidate/v1" and
  .candidate_commit == $commit and
  ([.lanes[].task] | sort) ==
    (["1","2","3A","3B","3C","4","5","6A","6B","6C","6D","6E"] | sort) and
  all(.lanes[];
    (.merge_commit | test("^[0-9a-f]{40}$")) and
    (.evidence_sha256 | test("^[0-9a-f]{64}$"))) and
  ([.profiles[].name] | sort) ==
    (["structural","scheduler","authority","continuity","privacy","interoperability","full"] | sort) and
  all(.profiles[];
    .passed == true and
    (.report_sha256 | test("^[0-9a-f]{64}$"))) and
  (.known_limitations | type == "array") and
  (.p2_exclusions | type == "array") and
  .approval.status == "pending"
  ' "$RELEASE_PACKET_DIR/go-no-go.json"
```

Any missing, duplicate, or stale digest is a no-go.

Load the protocol digest from the accepted Task 2 lane instead of relying on
shell state from an earlier session:

```bash
export COVEN_PROTOCOL_CONTENT_SHA256="$(
  jq -er '
    .lanes[] |
    select(.task == "2") |
    .artifact_digests.contract_content
  ' "$RELEASE_PACKET_DIR/go-no-go.json"
)"
[[ "$COVEN_PROTOCOL_CONTENT_SHA256" =~ ^[0-9a-f]{64}$ ]]
```

- [ ] **Step 2: Run local release gates from a clean checkout**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python3 scripts/check-secrets.py
node scripts/test-cli-prepublish.mjs
cargo build -p coven-cli
node scripts/package-automations-protocol.mjs \
  --output "$RELEASE_PACKET_DIR/contract"
```

The release candidate has a new source-bound bundle and manifest digest. Compare
only its content digest to Task 2, then record the candidate-specific archive
and manifest digests:

```bash
export COVEN_RELEASE_PROTOCOL_CONTENT_SHA256="$(
  jq -er '.contractContentSha256' \
    "$RELEASE_PACKET_DIR/contract/manifest.json"
)"
test "$COVEN_RELEASE_PROTOCOL_CONTENT_SHA256" = \
  "$COVEN_PROTOCOL_CONTENT_SHA256"
export COVEN_RELEASE_PROTOCOL_BUNDLE="$(
  find "$RELEASE_PACKET_DIR/contract" -maxdepth 1 -type f \
    -name "coven-automations-v1-contract-${RELEASE_CANDIDATE_SHA}.tar.gz" \
    -print -quit
)"
test -n "$COVEN_RELEASE_PROTOCOL_BUNDLE"
export COVEN_RELEASE_PROTOCOL_SHA256="$(
  shasum -a 256 "$COVEN_RELEASE_PROTOCOL_BUNDLE" | awk '{ print $1 }'
)"
export COVEN_RELEASE_PROTOCOL_MANIFEST_SHA256="$(
  shasum -a 256 "$RELEASE_PACKET_DIR/contract/manifest.json" |
    awk '{ print $1 }'
)"
[[ "$COVEN_RELEASE_PROTOCOL_SHA256" =~ ^[0-9a-f]{64}$ ]]
[[ "$COVEN_RELEASE_PROTOCOL_MANIFEST_SHA256" =~ ^[0-9a-f]{64}$ ]]
```

Re-run the SDK and Cave packed-artifact canaries against this exact candidate
bundle and record both results in the release packet. Any content-digest change
or candidate canary failure is a no-go.

- [ ] **Step 3: Produce provider certification reports**

```bash
export COVEN_CANDIDATE_BIN="$PWD/target/debug/coven"
test -x "$COVEN_CANDIDATE_BIN"
mkdir -p "$RELEASE_PACKET_DIR/cert"
"$COVEN_CANDIDATE_BIN" setup codex \
  --verify-only \
  --report-json "$RELEASE_PACKET_DIR/cert/codex.json"
"$COVEN_CANDIDATE_BIN" setup claude \
  --verify-only \
  --report-json "$RELEASE_PACKET_DIR/cert/claude.json"
"$COVEN_CANDIDATE_BIN" setup copilot \
  --verify-only \
  --report-json "$RELEASE_PACKET_DIR/cert/copilot.json"
jq -s \
  --arg commit "$RELEASE_CANDIDATE_SHA" \
  'all(.[]; .completed == true and .candidate_commit == $commit)' \
  "$RELEASE_PACKET_DIR"/cert/*.json |
jq -e '. == true'
```

Expected: every report is complete and its `candidate_commit` equals the intended tag commit.

- [ ] **Step 4: Obtain explicit maintainer approval for the irreversible release**

Do not tag or push before approval. Record the approved version and candidate SHA. After approval, export the exact tag and derive the npm version:

```bash
read -r -p "Maintainer-approved release tag (vMAJOR.MINOR.PATCH): " RELEASE_TAG
export RELEASE_TAG
[[ "$RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
export RELEASE_VERSION="${RELEASE_TAG#v}"
export RELEASE_ARTIFACT_DIR="$HOME/.coven/artifacts/release-$RELEASE_TAG"
mkdir -p "$RELEASE_ARTIFACT_DIR"
```

The quoted `RELEASE_TAG` value is supplied by the maintainer's approval and is not inferred by the worker. Stop if the regular-expression check fails.

After the maintainer records approval in `$RELEASE_PACKET_DIR/go-no-go.json`, bind it to the frozen candidate and requested tag:

```bash
jq -e \
  --arg commit "$RELEASE_CANDIDATE_SHA" \
  --arg tag "$RELEASE_TAG" \
  '.approval.status == "approved" and
   .approval.candidate_commit == $commit and
   .approval.release_tag == $tag' \
  "$RELEASE_PACKET_DIR/go-no-go.json"
```

- [ ] **Step 5: Preflight SSH signing, create, verify, and push one immutable tag**

```bash
set -euo pipefail

git fetch origin main
test "$(git rev-parse origin/main)" = "$RELEASE_CANDIDATE_SHA"
test "$(git rev-parse HEAD)" = "$RELEASE_CANDIDATE_SHA"

test "$(git config --get gpg.format)" = "ssh"
export RELEASE_SIGNING_KEY="$(git config --get user.signingkey)"
test -n "$RELEASE_SIGNING_KEY"
test -f "$RELEASE_SIGNING_KEY"

export RELEASE_ALLOWED_SIGNERS="$RELEASE_PACKET_DIR/allowed-signers"
gh variable get NPM_RELEASE_ALLOWED_SIGNERS \
  --repo OpenCoven/coven > "$RELEASE_ALLOWED_SIGNERS"
test -s "$RELEASE_ALLOWED_SIGNERS"

if [[ "$RELEASE_SIGNING_KEY" == *.pub ]]; then
  cp "$RELEASE_SIGNING_KEY" "$RELEASE_PACKET_DIR/signing-key.pub"
else
  ssh-keygen -y -f "$RELEASE_SIGNING_KEY" \
    > "$RELEASE_PACKET_DIR/signing-key.pub"
fi
gh api user/ssh_signing_keys --paginate --jq '.[].key' |
  grep -F -x "$(cat "$RELEASE_PACKET_DIR/signing-key.pub")"

if git rev-parse --verify --quiet "refs/tags/$RELEASE_TAG"; then
  echo "local tag already exists: $RELEASE_TAG" >&2
  exit 1
fi
if git ls-remote --exit-code --tags origin "refs/tags/$RELEASE_TAG" \
  >/dev/null 2>&1; then
  echo "remote tag already exists: $RELEASE_TAG" >&2
  exit 1
fi

git tag -s "$RELEASE_TAG" -m "Coven $RELEASE_TAG"
git -c gpg.ssh.allowedSignersFile="$RELEASE_ALLOWED_SIGNERS" \
  verify-tag "$RELEASE_TAG"
test "$(git rev-list -n 1 "$RELEASE_TAG")" = "$RELEASE_CANDIDATE_SHA"
git push origin "$RELEASE_TAG"
```

The local verification must name a signer from the repository's
`NPM_RELEASE_ALLOWED_SIGNERS`, and the signing public key must already be
registered as an SSH signing key on the authenticated GitHub maintainer
account. Never move or reuse the tag. If any preflight fails, delete only the
unpublished local tag if it was created and stop. If `origin/main` advanced
after certification, the equality check must fail; rebuild the candidate packet
and repeat certification rather than tagging a different commit.

- [ ] **Step 6: Verify npm and GitHub release receipts**

```bash
for package in \
  @opencoven/cli \
  @opencoven/cli-macos \
  @opencoven/cli-macos-x64 \
  @opencoven/cli-linux-x64 \
  @opencoven/cli-windows
do
  test "$(npm view "$package" version)" = "$RELEASE_VERSION"
  npm view "$package" dist-tags --json |
    jq -e --arg version "$RELEASE_VERSION" '.latest == $version'
done
gh release view "$RELEASE_TAG" --repo OpenCoven/coven
gh release download "$RELEASE_TAG" \
  --repo OpenCoven/coven \
  --dir "$RELEASE_ARTIFACT_DIR/release-check"
cd "$RELEASE_ARTIFACT_DIR/release-check" && shasum -a 256 -c SHA256SUMS
test -f "coven-automations-v1-contract-${RELEASE_CANDIDATE_SHA}.tar.gz"
test "$(shasum -a 256 "coven-automations-v1-contract-${RELEASE_CANDIDATE_SHA}.tar.gz" | awk '{print $1}')" = "$COVEN_RELEASE_PROTOCOL_SHA256"
```

Expected: all five npm packages resolve to the approved version, GitHub release
targets the signed tag, all native archives report `OK`, and the published
protocol bundle exactly matches the Task 2 digest.

- [ ] **Step 7: Verify a fresh registry install**

```bash
export FRESH_INSTALL_ROOT="$RELEASE_ARTIFACT_DIR/fresh-consumer"
mkdir -p "$FRESH_INSTALL_ROOT/home"
cd "$FRESH_INSTALL_ROOT"
npm init --yes
npm install --save-exact "@opencoven/cli@$RELEASE_VERSION"
npm audit signatures
npm ls --depth 1 @opencoven/cli
export RELEASED_COVEN_BIN="$FRESH_INSTALL_ROOT/node_modules/.bin/coven"
test -x "$RELEASED_COVEN_BIN"
"$RELEASED_COVEN_BIN" --version |
  grep -F "$RELEASE_VERSION"
doctor_status=0
HOME="$FRESH_INSTALL_ROOT/home" "$RELEASED_COVEN_BIN" doctor ||
  doctor_status=$?
test "$doctor_status" -eq 1
```

Expected: the wrapper and exactly one platform-native package match `RELEASE_VERSION`, provenance verifies, `coven --version` prints `RELEASE_VERSION`, and Doctor gives setup guidance with its documented bare-home exit code.

- [ ] **Step 8: Release Cave against the exact Coven artifact**

After Task 6B's PR merge is present on `origin/main`, create a clean detached
verification worktree at that exact revision and run the real-daemon Playwright
journey. This worktree is read-only release evidence, not a second #5217
implementation claim:

```bash
cd "$COVEN_CAVE_REPO"
git fetch origin main
export CAVE_AUTOMATIONS_WORKTREE="$HOME/.coven/worktrees/coven-cave-automations-v1-release-acceptance"
test ! -e "$CAVE_AUTOMATIONS_WORKTREE"
git worktree add --detach "$CAVE_AUTOMATIONS_WORKTREE" origin/main
cd "$CAVE_AUTOMATIONS_WORKTREE"
test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
test -z "$(git status --porcelain)"
COVEN_BIN="$RELEASED_COVEN_BIN" \
COVEN_EXPECTED_VERSION="$RELEASE_VERSION" \
pnpm exec playwright test tests/automations-v1-release.spec.ts
```

Expected: the test creates a paused definition, activates it, observes one
scheduled and one manual run through adopted commands, verifies receipts, then
removes the disposable definition without external mutation. Record the Cave
`origin/main` SHA and Playwright result in #854's release packet, then remove
the detached verification worktree.

**Exit gate:** immutable signed tag, npm provenance, GitHub release/checksums, fresh install, certification packet, and Cave end-to-end receipt all identify the same Coven commit.

---

### Task 8: Reconcile and close the program

**Owner:** OpenCoven/coven#854

**Depends on:** Task 7

**Files:**
- Modify: `docs/roadmaps/coven-automations-v1.mapping.json`
- Regenerate: `docs/roadmaps/coven-automations-v1.md`
- Create: `docs/roadmaps/coven-automations-v1.release.json`

- [ ] Update every Bead and GitHub outcome from exact merge/release evidence.
- [ ] Run strict roadmap drift and self-tests.
- [ ] Verify no P2 work became a v1 blocker or undocumented shipped behavior.
- [ ] Confirm every downstream canary pins the released artifact, not a branch or source-relative file.
- [ ] Close child outcomes only when their own acceptance criteria and receipts are present.
- [x] Close #859 after tracker graph and strict drift evidence are durable.
- [ ] Close #854 only after the final rollup resolves all required P0/P1 outcomes and release receipts.

Run:

```bash
node docs/roadmaps/drift-check.mjs --strict
node docs/roadmaps/drift-check.mjs --selftest
```

**Exit gate:** #854 links one machine-readable rollup that resolves tracker state, source revisions, conformance profiles, package provenance, release checksums, Cave acceptance, and all remaining known limitations.

---

## Parallelism and stop conditions

| Lane | Earliest safe start | Must stop when |
| --- | --- | --- |
| Task 0 dogfood | Now | Any external mutation is requested or daemon/runtime state diverges |
| Task 1 tracker | Now | Managed worktree or Beads writer is unavailable; do not bypass |
| Task 2 protocol | After Task 1 creates canonical ownership | Schema and implementation disagree; correct the specification or code before consumers |
| Task 3A scheduler | Task 2 merge | A state/error needed by scheduler is not ratified |
| Task 3B identity | Now | Identity work starts encoding authority or scheduler semantics |
| Task 3C authority profile | Task 3B immutable artifact | Approval cannot bind exact embodiment/revision |
| Task 4 dispatch binding | Tasks 2, 3B, 3C merge | Any string-only or allow-on-error fallback remains |
| Task 5 certification | Tasks 2, 3A, 4 merge | Runner tests its own model instead of the production artifact |
| Task 6 consumers | Read-only preparation after Task 2; mutations after Task 5 | Consumer authors lifecycle truth or uses mutable/source-relative artifacts |
| Task 7 release | Tasks 1-6 implementation outcomes merged and pre-release canaries verified | Any digest/profile/platform/provider receipt is missing or stale |
| Task 8 closure | Task 7 verified | Release and tracker evidence do not resolve to the same revisions |

## Definition of done

Coven-native automation is up and running only when:

1. an installed released Coven daemon—not a source checkout—owns definitions, occurrences, runs, attempts, leases, events, approvals, delivery, and receipts;
2. one scheduled and one manual familiar invocation traverse the same adopted, fenced, authority-checked production path;
3. restart, duplicate scheduler, retry, cancel, timeout, delivery failure, and ambiguous-effect scenarios converge without duplicate execution or false success;
4. every run pins exact principal, familiar revision, authority decision, runtime capabilities, definition revision, occurrence fence, and immutable receipt;
5. Cave and the SDK consume packed immutable artifacts, replay the changefeed safely, and never fall back to Codex files or direct runtime launch;
6. Psyche composes orchestration evidence without owning schedules;
7. conformance, privacy, security, SLO, provider, platform, package-provenance, checksum, and fresh-install receipts all resolve to the same release commit;
8. the Beads/GitHub roadmap is drift-free and the final #854 rollup is machine-readable.
