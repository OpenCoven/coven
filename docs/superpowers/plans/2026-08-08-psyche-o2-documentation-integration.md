# Psyche O2 Documentation Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the proposed O2 execution-binding contract into Psyche
documentation without claiming that the deferred O3-O7 capabilities exist.

**Architecture:** `O2_CONTRACT_DESIGN.md` is the proposed detailed contract
for immutable opaque binding and exact-match correlation. The canonical
runtime, technical, plan, and review documents link to it and classify
adoption, lookup/fencing, cancellation acknowledgement, artifacts, and recovery
as distinct O3-O7 phases. No daemon route, database schema, or runtime code is
changed.

**Tech Stack:** Markdown, Git, repository privacy and secret guards.

---

## File structure

| File | Responsibility |
| --- | --- |
| `.gitignore` | Ignore only repository-root local Psyche artifacts. |
| `specs/psyche/O2_CONTRACT_DESIGN.md` | Proposed, detailed O2 contract and implementation acceptance map. |
| `specs/psyche/RUNTIME_DESIGN.md` | Canonical product/ownership boundary and phased execution-binding overview. |
| `specs/psyche/TECH.md` | Canonical technical architecture; references the O2 contract instead of duplicating an O2-O7 composite schema. |
| `specs/psyche/PLAN.md` | Program sequencing and companion-document index. |
| `specs/psyche/INTEGRATION_REVIEW.md` | Non-normative review summary that points to the proposed O2 boundary. |

### Task 1: Preserve the O2 proposal and narrow the ignore rule

**Files:**
- Modify: `.gitignore`
- Create: `specs/psyche/O2_CONTRACT_DESIGN.md`

- [ ] **Step 1: Narrow the local artifact rule**

Change the final ignore entry from:

```gitignore
.psyche*
```

to:

```gitignore
/.psyche*
```

This preserves the intended local-root artifact exclusion without hiding
`.psyche*` files in documentation, fixtures, or nested projects.

- [ ] **Step 2: Retain the proposal’s delivery status**

Keep this header in `specs/psyche/O2_CONTRACT_DESIGN.md`:

```markdown
**Status:** Design proposed; not yet approved or implemented.
```

Keep the explicit O2 non-goals: no replay/duplicate-adoption prevention, no
adoption key, no lookup/fencing, no cancellation acknowledgement, no artifact
binding, and no crash-safe O2-O6 recovery.

- [ ] **Step 3: Validate the staged scope**

Run:

```bash
git diff --check
git check-ignore -v .psyche .psyche-cache/example
```

Expected: no whitespace errors; both sample root paths report the anchored
`/.psyche*` rule.

- [ ] **Step 4: Commit the bounded proposal**

```bash
git add .gitignore specs/psyche/O2_CONTRACT_DESIGN.md
git commit -m "docs: define proposed Psyche O2 binding contract"
```

### Task 2: Reconcile the canonical runtime boundary

**Files:**
- Modify: `specs/psyche/RUNTIME_DESIGN.md`

- [ ] **Step 1: Add the O2 proposal to the canonical companion links**

Add `[O2 contract design](./O2_CONTRACT_DESIGN.md)` beside the existing O1
contract-design link near the document header. Label it as a proposed
contract in the surrounding prose.

- [ ] **Step 2: Replace the composite contract-table entry**

Replace the existing `psyche.execution_binding.v1` purpose:

```markdown
Stable attempt and request IDs, payload digest, Coven adoption resolution,
event cursor, and terminal correlation.
```

with a phased summary:

```markdown
Proposed O2 immutable opaque session binding and exact-match correlation;
O3-O7 separately add adoption, lookup/fencing, cancellation, artifacts, and
recovery.
```

- [ ] **Step 3: Make the Coven-client boundary phase-aware**

Under `### 4.9 Coven client`, retain the product requirement that Psyche
eventually adopts, follows, cancels, and recovers sessions. Add a sentence
that those operations require O3-O7 after O2 and are not capabilities of the
proposed O2 contract. Link to `O2_CONTRACT_DESIGN.md`.

- [ ] **Step 4: Correct the execution-binding capability row**

Replace the table row:

```markdown
| Coven execution binding | One node dispatches, adopts, follows, cancels, and recovers one real session. |
```

with two rows:

```markdown
| Proposed O2 execution binding | Immutable opaque request/session correlation and exact mismatch rejection only. |
| O3-O7 execution lifecycle | Planned adoption, lookup/fencing, cancellation acknowledgement, artifact binding, and recovery required before real conformance. |
```

- [ ] **Step 5: Validate links and terminology**

Run:

```bash
rg -n 'psyche\.execution_binding\.v1|O2|O3-O7' specs/psyche/RUNTIME_DESIGN.md
```

Expected: every O2 reference says proposed, and the document no longer assigns
adoption, cursors, cancellation, or terminal recovery directly to O2.

### Task 3: Reconcile the technical architecture without duplicating O2’s wire schema

**Files:**
- Modify: `specs/psyche/TECH.md`

- [ ] **Step 1: Update the contract inventory**

Replace the `psyche.execution_binding.v1` row with:

```markdown
| `psyche.execution_binding.v1` | Proposed O2 immutable opaque binding and mismatch correlation; O3-O7 separately own adoption, lookup/fencing, cancellation, artifacts, and recovery. |
```

- [ ] **Step 2: Replace the composite binding JSON**

Under `### Identity snapshot`, remove the inline
`psyche.execution_binding.v1` example containing `adoption_state`,
`event_cursor`, `cancellation_state`, and `terminal_state`. Replace it with
the following prose:

```markdown
The proposed O2 binding is specified in
[O2 contract design](./O2_CONTRACT_DESIGN.md). It carries the immutable opaque
correlation tuple that Coven persists and exact-compares. Psyche retains its
own request record and does not treat the O2 binding as an adoption, cursor,
cancellation, artifact, or recovery record; those are planned O3-O7 state.
```

- [ ] **Step 3: Reassign every composite-state claim**

At each `TECH.md` reference to `psyche.execution_binding.v1`, make the
ownership explicit:

```markdown
O2 stores immutable correlation; O3 owns adoption/uniqueness, O4 owns
lookup/fencing, O5 owns cancellation acknowledgement, O6 owns artifacts, and
O7 owns cross-phase recovery.
```

Apply this to the execution-request narrative, storage-table entry, and any
error description that currently represents O3-O7 state as a field of the O2
object. Keep the broader Psyche lifecycle and safety requirements; only stop
attributing them to O2.

- [ ] **Step 4: Verify no stale composite O2 schema remains**

Run:

```bash
rg -n 'adoption_state|event_cursor|cancellation_state|terminal_state' specs/psyche/TECH.md
```

Expected: no result is inside an O2 binding example or described as an O2
field. References to adoption/cancellation elsewhere remain only as planned
Psyche lifecycle requirements.

### Task 4: Align the program plan and review dossier

**Files:**
- Modify: `specs/psyche/PLAN.md`
- Modify: `specs/psyche/INTEGRATION_REVIEW.md`

- [ ] **Step 1: Add O2 to the program-plan companion index**

Add `[O2 contract design](./O2_CONTRACT_DESIGN.md)` after the O1 link. Add a
short O2-O7 delivery-boundary note after the O1 candidate paragraph:

```markdown
O2 is a proposed immutable binding and mismatch-correlation contract. Stable
adoption, lookup/fencing, cancellation acknowledgement, artifact binding, and
cross-phase recovery remain separate O3-O7 planned work; W5/G4 still require
the full classified conformance profile.
```

- [ ] **Step 2: Make the review dossier point to the proposed contract**

In `INTEGRATION_REVIEW.md`, add the O2 contract to the source-precedence table
as the detailed proposed contract for immutable binding. Update its
`psyche.execution_binding.v1` contract row to state:

```markdown
Proposed O2 immutable attempt/session correlation; O3-O7 own adoption, cursor,
cancellation, artifact, and recovery state.
```

Keep the dossier non-normative and preserve `RUNTIME_DESIGN.md` as the
authoritative product boundary.

- [ ] **Step 3: Validate all companion links**

Run:

```bash
rg -n 'O2 contract design|O3-O7|psyche\.execution_binding\.v1' \
  specs/psyche/PLAN.md specs/psyche/INTEGRATION_REVIEW.md
```

Expected: both documents link to the detailed O2 proposal and assign later
behavior to O3-O7.

- [ ] **Step 4: Commit the reconciliation**

```bash
git add specs/psyche/RUNTIME_DESIGN.md specs/psyche/TECH.md \
  specs/psyche/PLAN.md specs/psyche/INTEGRATION_REVIEW.md
git commit -m "docs: reconcile Psyche O2 delivery boundaries"
```

### Task 5: Run documentation safety checks and prepare review

**Files:**
- Verify: `.gitignore`
- Verify: `specs/psyche/O2_CONTRACT_DESIGN.md`
- Verify: `specs/psyche/RUNTIME_DESIGN.md`
- Verify: `specs/psyche/TECH.md`
- Verify: `specs/psyche/PLAN.md`
- Verify: `specs/psyche/INTEGRATION_REVIEW.md`

- [ ] **Step 1: Check the complete documentation diff**

Run:

```bash
git diff origin/main...HEAD --check
git diff --cached --check
```

Expected: both commands exit 0.

- [ ] **Step 2: Run repository guards**

Run:

```bash
python scripts/check-secrets.py
git add .gitignore specs/psyche/O2_CONTRACT_DESIGN.md \
  specs/psyche/RUNTIME_DESIGN.md specs/psyche/TECH.md \
  specs/psyche/PLAN.md specs/psyche/INTEGRATION_REVIEW.md
python3 scripts/check-coven-privacy.py --staged
```

Expected: the secret and privacy guards pass with no exceptions or allowlists.

- [ ] **Step 3: Review the claimed delivery state**

Run:

```bash
rg -n 'implemented|shipped|complete' specs/psyche/O2_CONTRACT_DESIGN.md \
  specs/psyche/RUNTIME_DESIGN.md specs/psyche/TECH.md \
  specs/psyche/PLAN.md specs/psyche/INTEGRATION_REVIEW.md
```

Expected: no new statement describes O2 or any O3-O7 capability as delivered.

- [ ] **Step 4: Push and open the documentation PR**

```bash
git push -u origin docs/psyche-o2-contract
gh pr create --base main --head docs/psyche-o2-contract \
  --title "docs: reconcile Psyche O2 contract boundary"
```

The PR description must state that this is documentation-only and that O2 is
proposed, while O3-O7 remain planned.
