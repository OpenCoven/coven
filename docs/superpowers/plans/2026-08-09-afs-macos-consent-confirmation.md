# AgentFS macOS Consent Confirmation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reconcile source and public AgentFS documentation with the verified macOS Terminal NFS mounted-I/O confirmation while retaining all remaining security and deployment gates.

**Architecture:** Treat the successful Terminal run as evidence about one consent-enabled NFS client, not a new product capability. The source spike record remains the evidence authority; source design and research documents summarize its changed implication; public documentation summarizes the same limit without publishing a supported mount workflow.

**Tech Stack:** Markdown/MDX, GitHub pull requests, Coven documentation guards, Mintlify documentation build.

---

## File structure

| File | Responsibility |
| --- | --- |
| `specs/coven-agent-fs/MOUNT-SPIKE.md` | Canonical macOS NFS evidence and mount go/no-go decision. |
| `specs/coven-agent-fs/DESIGN.md` | Delivery sequencing for the AgentFS product design. |
| `specs/coven-agent-fs/RESEARCH.md` | Research conclusions and recommended work. |
| `content/docs/guide/agent-filesystem.mdx` in `OpenCoven/coven-docs` | Public capability, safety boundary, and roadmap status. |
| `content/docs/guide/architecture.mdx` in `OpenCoven/coven-docs` | Public architecture-level summary. |
| `content/docs/daemon/security.mdx` in `OpenCoven/coven-docs` | Public security-boundary summary. |

### Task 1: Record the source evidence and revised gate

**Files:**
- Modify: `specs/coven-agent-fs/MOUNT-SPIKE.md:12-14,142-157,171-175`
- Modify: `specs/coven-agent-fs/DESIGN.md:431-438`
- Modify: `specs/coven-agent-fs/RESEARCH.md:63-78`

- [ ] **Step 1: Replace the stale source conclusion with the verified result**

In `MOUNT-SPIKE.md`, state that the NFS mount works for a consent-enabled
client process and make the evidence auditable:

```markdown
On 2026-08-09, a human-operated Terminal observed `afs_serve` listening on
`127.0.0.1:12049`, mounted `localhost:/` at `/private/tmp/afsmnt`, and
successfully created, wrote, and read back `/private/tmp/afsmnt/hello/file.txt`.
```

State that the earlier connection-refused run happened before the server was
listening and the resulting local-directory write is not mount evidence.
Replace the `Conditional on the mount` paragraph with wording that permits
mount-dependent engineering validation for consent-enabled clients, but keeps
`afsMount: false` as the safe default until loopback NFS access control,
concurrency, recovery, Linux/FUSE, sandboxing, and default-on policy have
independent decisions.

- [ ] **Step 2: Update the design and research summaries**

In `DESIGN.md`, replace the delivery-table phrase `conditional on the Full Disk
Access confirmation` with:

```markdown
merged experimental spike — PR #680; Terminal mounted I/O confirmation passed,
with per-process network-volume consent required
```

In `RESEARCH.md`, replace the outstanding-confirmation blocker with language
that says the manual Terminal confirmation passed, and that automated
agent/client processes may still require explicit macOS privacy or
network-volume consent. Keep Linux/FUSE and loopback access-control work open.

- [ ] **Step 3: Inspect the changed source documents**

Run:

```sh
git diff --check
rg -n -C 2 'outstanding|needs an agent-process|blocked on the Full Disk|no agent process has yet' \
  specs/coven-agent-fs/MOUNT-SPIKE.md \
  specs/coven-agent-fs/DESIGN.md \
  specs/coven-agent-fs/RESEARCH.md
```

Expected: `git diff --check` has no output. The search has no stale claim that
the successful Terminal confirmation is outstanding; it may show historical
context explaining the original agent-process denial.

- [ ] **Step 4: Commit the source documentation**

```sh
git add specs/coven-agent-fs/MOUNT-SPIKE.md \
  specs/coven-agent-fs/DESIGN.md \
  specs/coven-agent-fs/RESEARCH.md
git commit -m "docs: record AFS macOS consent validation" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: Reconcile the public AgentFS documentation

**Files:**
- Modify: `content/docs/guide/agent-filesystem.mdx` in `OpenCoven/coven-docs`
- Modify: `content/docs/guide/architecture.mdx` in `OpenCoven/coven-docs`
- Modify: `content/docs/daemon/security.mdx` in `OpenCoven/coven-docs`

- [ ] **Step 1: Create a clean public-docs worktree from `origin/main`**

```sh
docs_repo="$(dirname "$(dirname "$(git rev-parse --git-common-dir)")")/coven-docs"
git -C "$docs_repo" fetch origin main --quiet
git -C "$docs_repo" worktree add -b docs/afs-macos-consent-confirmation \
  /tmp/coven-docs-afs-macos-consent-confirmation origin/main
cd /tmp/coven-docs-afs-macos-consent-confirmation
```

Expected: the worktree is clean and tracks the current `origin/main`. Do not
reuse or change any pre-existing public-docs checkout.

- [ ] **Step 2: Update the public capability and roadmap language**

In `agent-filesystem.mdx`, replace the outstanding Full Disk Access
confirmation with this bounded result:

```markdown
A human-operated macOS Terminal successfully mounted the loopback export and
performed a mounted directory create, file write, and read-back. This validates
the NFS path for a consent-enabled client; an automated agent or other client
process may still need explicit macOS network-volume/privacy consent.
```

Change the roadmap so the macOS confirmation is delivered, while
concurrent-client behavior, Linux/FUSE, loopback access control, recovery,
sandboxing, and default-enable decisions remain planned. Retain all statements
that there is no `coven afs` command, daemon-managed lifecycle, supported mount
workflow, or sandboxed execution mode.

- [ ] **Step 3: Update architecture and security summaries**

In `architecture.mdx`, replace `the outstanding macOS privacy confirmation`
with a concise reference to the passed consent-enabled Terminal validation and
the continuing per-process consent requirement.

In `security.mdx`, replace `the macOS agent-process read/write confirmation is
still outstanding` with wording that says a human-operated Terminal passed
mounted I/O, but that a particular automated client can still be blocked by
macOS privacy policy. Preserve the adjacent warnings that loopback NFS lacks
application-layer authentication and is not a sandbox or access-control
boundary.

- [ ] **Step 4: Run public documentation validation**

Run the existing repository commands advertised by `package.json`:

```sh
pnpm run build
pnpm run check:links
git diff --check
```

Expected: each command exits successfully and `git diff --check` has no output.

- [ ] **Step 5: Commit the public documentation**

```sh
git add content/docs/guide/agent-filesystem.mdx \
  content/docs/guide/architecture.mdx \
  content/docs/daemon/security.mdx
git commit -m "docs: clarify AFS macOS consent result" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Validate, integrate, and close the bead

**Files:**
- No additional repository files.

- [ ] **Step 1: Run the source documentation guards**

From the Coven source worktree, run:

```sh
git diff --check
python scripts/check-secrets.py
git add specs/coven-agent-fs/MOUNT-SPIKE.md \
  specs/coven-agent-fs/DESIGN.md \
  specs/coven-agent-fs/RESEARCH.md
python3 scripts/check-coven-privacy.py --staged
```

Expected: no whitespace errors, no secret findings, and a passing staged
privacy guard. Do not add unrelated source files.

- [ ] **Step 2: Open source and public documentation pull requests**

Push each scoped branch, open a PR against `main`, and describe the same
bounded conclusion in both PR bodies: Terminal mounted I/O passed; per-process
consent and all remaining mount-safety gates are unchanged.

- [ ] **Step 3: Merge only after required checks pass**

Merge both PRs only when their required checks are green. Do not bypass branch
protection or weaken any documentation, security, or privacy gate to merge.

- [ ] **Step 4: Close `coven-x77` with exact evidence**

After both merges, add this evidence to the bead and close it:

```text
Verified on macOS 2026-08-09: afs_serve listened on 127.0.0.1:12049;
mount_nfs mounted localhost:/ at /private/tmp/afsmnt; mounted mkdir, file write,
and read-back returned "written". The earlier connection-refused attempt
preceded server readiness and is not mount evidence. Documentation now records
the per-process network-volume consent requirement and remaining safety gates.
```

- [ ] **Step 5: Clean the manual test resources if still present**

Inspect the exact mount and server PID before cleanup. If still mounted, run
`umount -f /tmp/afsmnt`; if PID `26067` is still the test server, run
`kill 26067`. Do not use process-name-based termination.
