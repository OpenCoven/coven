---
name: "Bug: Sessions List Display Cache"
about: "coven sessions command shows deleted sessions"
title: "bug: coven sessions list shows deleted sessions (display cache issue)"
labels: bug,display,cli
---

## Issue
After deleting sessions with `coven sacrifice <id> --yes`, the `coven sessions` command still displays those deleted sessions in the list.

## Steps to Reproduce
1. Run `coven sessions` to see all sessions
2. Note any orphaned sessions
3. Run `coven sacrifice <orphaned-id> --yes` for one or more orphaned sessions
4. Run `coven sessions` again
5. The deleted sessions still appear in the list, even though:
   - Attempting to sacrifice the same ID again returns 'session not found'
   - The session data is actually deleted from the SQLite store

## Expected Behavior
After deleting a session, `coven sessions` should not display it in the output.

## Actual Behavior
The session list appears to be cached and shows stale/deleted entries even after successful deletion.

## Root Cause
The sessions query result set appears to be cached at the command level and not invalidated after delete operations.

## Impact
Minor UX issue—the data layer is correct (sessions are actually deleted), but the display layer shows stale results.

## Test Results
- Deleted 14 orphaned sessions successfully (verified via re-attempt: "session not found")
- SQLite store confirmed clean (DB modified timestamp updated)
- `coven sessions` still displays all 14 deleted sessions
- Full daemon restart didn't clear the display

## Suggested Fix
Invalidate the session list cache after successful sacrifice/delete operations, or switch to live queries instead of cached results.
