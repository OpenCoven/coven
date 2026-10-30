---
title: "Task worktree lifecycle"
summary: "Bounded allocation and ownership-preserving pruning for shared Coven checkouts."
source_adjacent_reason: "Defines the repository task allocator and shared checkout cleanup contract."
---

# Task worktree lifecycle

Run `python3 scripts/task-worktree.py inventory` at session start and after delivery. Fetch origin before making a decision based on remote counts. The budget is 12 linked task worktrees, 32 local branches excluding `main`, and 48 origin branches excluding `main` and symbolic `HEAD`. Detached, locked and missing worktree registrations count because they still need an explicit disposition. These are repository coordination limits, not a runtime authority policy.

## Allocate

Inspect open PRs and `coven claim status`, fetch current main, then allocate through the shared helper:

```sh
git fetch origin main --prune
python3 scripts/task-worktree.py create --issue 42 --branch fix/42-example --path /tmp/coven-42-example
cd /tmp/coven-42-example
coven claim acquire issue-42
```

The helper holds a common-Git-directory allocation lock while it reads counts and creates the branch/worktree at `origin/main` without upstream tracking. It refuses an existing destination, an invalid branch or missing base, a held allocator lock, and any projected budget excess. It never deletes existing work to satisfy a budget. Inspect the lock's recorded PID and owning issue before recovering a lock left by a terminated process; a timestamp alone is insufficient evidence. If claim acquisition fails after allocation, stop and reconcile ownership; do not start implementation.

Existing work can continue when counts already exceed the budget. Additional allocation is refused until cleanup makes room. A limit change requires an issue-backed reviewed policy change. All repository agents must use the helper; raw Git remains capable of bypassing this cooperative repository policy, so the allocator cannot constrain an unrelated external Git client.

## Prune after delivery and before new allocation

1. Refresh origin and the issue's PR/merge state through GitHub REST. Enumerate all local and remote branches and registered worktrees. Inspect shared claims and processes using the candidate path, including working directories, build outputs and logs. Preserve externally managed or detached review work unless its owner has explicitly released it.
2. Record each candidate's owner, issue, exact HEAD, path, PR disposition and acceptance evidence on its issue. Active claims, live processes, open PR heads, uncommitted changes, untracked or ignored files, unique commits and uncertain ownership block deletion. An expired claim, old commit or missing upstream is not a release.
3. Inspect `git -C <path> status --porcelain=v1 --untracked-files=all --ignored` and `git log origin/main..<branch>`. Check ignored data explicitly. Squash merging does not make the original commits ancestors of main; confirm the delivered tree and preserve an owner-reviewed recovery bundle outside the checkout before disposing of such a branch. A bundle preserves Git objects, not untracked, ignored or runtime files.
4. Stop only your own processes, preserve required receipts outside the worktree, release your claim from that worktree, and recheck its exact HEAD and full status. If ownership or filesystem state changed, stop and reclassify. Never use `--force` to make removal succeed.
5. Remove the verified-safe worktree with `git worktree remove <path>`, then remove its local branch with `git branch -d <branch>`. If a squash merge prevents ancestry-based deletion, retain the branch until an authorized cleanup records its exact merged-PR head, merge ancestry, absence of open PR references, ownership release and verified recovery bundle on the owning issue. Recheck those conditions immediately before `git update-ref -d refs/heads/<branch> <expected-sha>`, which refuses a changed ref. This exception requires the complete evidence packet; do not silently substitute `-D` or delete a checked-out branch.
6. Delete the delivered remote branch only after rechecking the exact remote SHA and all open PR heads. Use an expected-SHA lease, such as `git push --force-with-lease=refs/heads/<branch>:<expected-sha> origin :refs/heads/<branch>`. Preserve protected, stacked, held and uncertain refs. An open PR in a fork reusing the same name is not proof that origin's branch is its head.
7. Run `git worktree prune --dry-run` before removing stale registrations. Missing paths can be temporarily unavailable mounts; absence alone is insufficient deletion evidence. Remove a registration only after its owner/process/storage disposition is established. Finish with `git fetch origin --prune` and `python3 scripts/task-worktree.py check` and record the resulting counts and retained blockers.

The scheduled branch-triage action operates on remote delivery state, not local worktree ownership. Its MERGED classification requires the exact origin head to match the merged PR and that PR's merge commit to be in the fetched base. It refreshes all open PR heads before deletion and uses an expected-SHA lease; reused names, concurrent pushes and failed checks retain the ref. It must not be used as a local cleanup shortcut. Do not use missing-upstream force deletion, age-based deletion, automatic archival of active work or a global cleanup command to satisfy the budget. Pruning is complete only when eligible artifacts are removed, uncertain work is retained with an owner and next action, and the resulting counts are recorded.

## Verification

`python3 scripts/task-worktree-test.py` exercises allocation against disposable Git repositories. `python3 scripts/branch-triage-test.py` runs remote triage against a disposable real Git remote and proves that both live and dry-run modes preserve unique local work whose upstream disappeared. Both run in the required Policy guard. Run the repository's applicable required guards before publishing allocator or lifecycle-policy changes. Passing these tests does not establish that a legacy worktree is safe to delete.
