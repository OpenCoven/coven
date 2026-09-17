# macOS Seatbelt backend for the restricted worker controller

This crate is the first reviewed-in-progress `Driver` backend for
[`coven-restricted-runtime`](../coven-restricted-runtime/README.md)
(OpenCoven/coven#1004). It turns the controller's sequencing into real OS
enforcement for **one offline worker on macOS** and proves it with a
whole-process conformance test that the kernel, not the library, denies the
worker everything outside its sealed closure.

It is **not** `coven.session-policy.v1` support. Nothing here touches
discovery, capability negotiation, `SessionRuntime`, PTY runners, harness
launches, or `POST /api/v1/sessions/restricted`. v1 stays refusal-only until a
separately reviewed integration consumes this backend and the remaining
obligations below are closed.

## What the backend does

`SeatbeltDriver::seal(config, stdio)` takes custody of:

- a **private workspace** (`0700`, owned by the caller) opened as a directory
  with `O_NOFOLLOW`; every child below is then opened with `openat` relative
  to a held directory descriptor and `O_NOFOLLOW`, so no path component can be
  swapped for a symlink;
- the private **`bin/` directory** (`0700`, caller-owned, same filesystem);
- the pinned **executable** `bin/coven-worker-target` (regular file,
  caller-owned, executable, not group/world-writable, same filesystem);
- the staged **guardian binary** `bin/coven-worker-guardian` (same checks; the
  caller copies its trusted host binary there before sealing);
- the **closure directory** `closure/` (same filesystem);
- the three **controller-owned stdio pipes** from `WorkerStdio::pipes()`.

Every privacy check above also refuses an **extended ACL**. Mode bits are not
the whole access story on Darwin: an ACL entry can grant another principal
write or traverse while `stat` still reports caller ownership and `0700`, so a
mode-only check would call a shared path private and let a non-owner replace
the staged target or guardian. `acl_get_fd_np(fd, ACL_TYPE_EXTENDED)` runs on
the descriptor already held, and any ACL at all is refused — an owner-only ACL
would be redundant with the mode bits already required. `closure/` needs no
check of its own: it is only reachable by traversing the workspace, which is
now proven owner-only.

It returns the driver plus an `InstantClock` sharing the backend's monotonic
origin, so the controller lease and the guardian deadline live in one domain.
`Binding` identities are the physical `(dev, ino)` pairs of the held
descriptors, a hash of the fixed profile, and fresh random attempt/worker IDs.

| Controller operation | Backend behavior |
| --- | --- |
| `prepare` | Re-`fstat`s every held descriptor and `lstat`s its path; both must still name the sealed inode. |
| `install_restrictions` | Pins the single-line Seatbelt profile (below). The kernel install happens inside the forked worker **before its `execve`**, so no target code can run unrestricted; a profile that fails to compile aborts the exec. |
| `retain_lifetime` | Spawns the **guardian** (`Role::Guardian`, own process group, empty environment, stderr discarded) with the owner PID and the executable path. The worker pipes travel as fds 3-5; every other inheritable descriptor is closed. The remaining lease is measured immediately before it is written as the first stdin line (`lease <ms>`), and the guardian starts its deadline on receipt, so handoff latency can only shorten the lease. Refuses if the lease already expired. |
| `revalidate` | Same physical checks as `prepare`, plus the guardian must still be alive. |
| `execute` | Re-checks the stop signal immediately before the irreversible `spawn` send and again after `spawned`; a cancel landing in between kills the fresh worker and returns `Rejected`. Tells the guardian to `spawn`; the guardian forks the worker in a **new process group**, closes inheritable descriptors, calls `sandbox_init`, then execs with an empty environment. `spawned <pid>` completes the handoff. The guardian re-reads the owner/lease boundary **after** the fork and may answer `terminated` instead: a worker that crossed the boundary mid-spawn is killed before it is ever reported as running, and that line is absorbed here so cleanup still sees a proven-empty group. |
| `request_cleanup` | Sends `kill`; the guardian `SIGKILL`s the whole group until `killpg(pgid, 0)` reports no member and the leader is reaped (2 s budget). `Released` only when that completes; otherwise `Pending`, in which case the guardian stays alive and keeps killing until it can report `terminated`. With no guardian, nothing was launched and cleanup is `Released`. |
| `observe_termination` | `Confirmed` only after the guardian reported `terminated` (leader reaped **and** group empty). A guardian that exited without that report is `Unavailable`, which fences and remains retryable as an observation. |

### Seatbelt profile

```scheme
(version 1) (deny default) (import "dyld-support.sb")
(allow process-exec (literal "<workspace>/bin/coven-worker-target"))
(allow file-read* (subpath "<workspace>"))
(allow file-read* (subpath "/usr/lib") (literal "/dev/null"))
(allow sysctl-read)
```

`dyld-support.sb` is Apple's own read-only shared-cache/loader rule set; a
Mach-O cannot start without it. `/usr/lib` covers `libSystem`; `sysctl-read`
is required by the Rust runtime's stack-guard setup. `process-fork` is denied,
so the worker cannot spawn anything, not even its own image. `process-exec` is
pinned to the one literal path, and the `exec-other` probe proves that
separately from fork (below). Paths with `"`, `\`, NUL
or line breaks are refused at seal time. Everything else — home directories,
`/etc`, writes anywhere (including the workspace), exec of any image, fork,
network, Mach lookups — is denied by default.

### Guardian

The guardian is a separate process, so it survives controller drop, driver
drop, a blocked caller, and owner death. It fences autonomously on:

- **lease expiry** (its own monotonic deadline, started when the `lease` line
  arrives);
- **owner death** (`kill(owner, 0)` → `ESRCH`, or EOF on its command pipe);
- explicit `kill`.

The guardian is single-threaded, so it cannot process a fence while it is
inside `fork`/`sandbox_init`/`exec`. `boundary_open` is therefore read twice
around `Worker::spawn`: once to authorise the fork, once after it. A worker
whose owner exited or whose lease expired during the spawn is killed before
`spawned` is ever said, and the guardian answers `terminated` instead. It
answers through `finish`, not `spawn-failed`, because only `terminated`
proves the process group is empty — exiting with `spawn-failed` after a real
fork would strand the controller on `Pending`/`Unavailable` for a group the
guardian had already cleared. This narrows what is reported, not what is
enforced: the profile is installed in the child before its `execve`, so
anything running in that window is already sealed, and the loop tail fences
immediately after spawn either way.

Every fence is a `SIGKILL` of the whole process group until empty. The
guardian exits after reporting `terminated`, or on `release`. If the first
bounded attempt (2 s) fails it reports `pending` once, then stays alive and
keeps killing until the group is gone rather than orphaning a live worker. It
is single-threaded (nonblocking stdin) so the fork/`sandbox_init` path stays
safe. `sandbox_init` is linked explicitly from `libSystem`.

## Conformance evidence

`tests/seatbelt.rs` (`harness = false`) plays three roles chosen only by
`argv[0]`: the test, the guardian, and the sealed worker. The worker is a copy
of the test binary placed under the workspace; it probes the kernel and prints
one line per probe.

`exec-other` is a bare `execv` of `/bin/ls`, not a `Command`. A `Command` must
fork before it can exec, and `process-fork` is denied, so it would report
`denied` even if `process-exec` wrongly allowed `/bin/ls` — no evidence for the
exec restriction at all. The bare `execv` returns only when the kernel refuses
it; were it allowed, `/bin/ls` would replace the worker and neither that line
nor `spawn-child` would reach the test. `spawn-child` remains the fork
evidence. Adding `(literal "/bin/ls")` to the profile's `process-exec` rule
fails three cases, and failed none before this change.

The test asserts, through the real controller:

| Case | Evidence |
| --- | --- |
| `kernel_denies_everything_outside_the_sealed_closure` | `env:0`, `inside-read:allowed`, `outside-read:denied`, `home-read:denied`, `inside-write:denied`, `outside-write:denied`, `exec-other:denied`, `spawn-child:denied`; no escape file appears; natural exit → `Terminated`; cleanup `Released`. |
| `guardian_kills_the_group_when_the_lease_expires` | A worker sleeping 120 s dies between 600 ms and 10 s; status reason `Expired`. |
| `explicit_cancel_cleanup_terminates_the_group` | `cancel` → `poll` fences `Stopping`; cleanup `Pending` while possibly live; observation confirms `Terminated` with `Released`; execution stays `PossiblyStarted`. |
| `guardian_kills_the_group_when_the_owner_dies` | A child owner launches a lingering worker and exits with no fence or cleanup; the worker dies anyway. |
| `seal_refuses_unsafe_workspaces` | `0755` workspace, group/world-writable target, missing closure, group/world-writable or missing staged guardian, symlinked `bin/` → typed `SealError`s. |
| `seal_refuses_an_extended_acl` | An `everyone allow write` ACL on the workspace, then on the target, with mode bits asserted still `0700`: `SealError::Workspace` / `SealError::Executable`. Removing the ACL — the only change — seals again. |
| `substituted_executable_is_refused_before_execution` | Swapping the inode at the pinned path fails `Revalidate`; state `Refused`, execution `NotStarted`. |

```sh
CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo test -p coven-restricted-runtime-macos --locked
CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo clippy -p coven-restricted-runtime-macos --all-targets --locked -- -D warnings
cargo fmt --check
```

On non-macOS targets the crate exposes only `TARGET_NAME`, `GUARDIAN_NAME`
and `role()`; the test prints a skip line. CI on Linux therefore does not
produce this evidence — it must be run on macOS.

## Obligations still open before v1 can leave refusal-only

- **No `fexecve` on macOS.** The worker and the guardian are exec'd by
  pathname immediately after `lstat`/`fstat` agree with the held inodes, and
  the profile pins the worker's literal path. Both images live inside the
  `0700` workspace, so the residual rename race is limited to same-UID
  processes; it is not eliminated.
- **Guardian handoff is not a kernel-atomic gate.** The worker's process group
  exists before its exec (std sets `setpgid` in the child), and the guardian is
  the parent, so it knows the pid at fork. A guardian crash in that
  microsecond window would orphan a restricted worker until lease expiry has
  nobody to enforce it.
- **Descendant tracking is process-group based.** `process-fork` is denied so
  no descendants can exist, but the worker itself could `setsid`/`setpgid` out
  of its group and escape `killpg`; Seatbelt does not deny those calls here.
- **Sealed closure is read-only, not a verified manifest.** The backend pins
  identities of the workspace, executable and closure directory, not a hash of
  every file inside `closure/`.
- **Only the direct owner PID is watched.** Owner loss is detected per process;
  there is no session/login boundary.
- **No integration into `SessionRuntime`, discovery, or the wire contract.** A
  positive negotiated acceptance contract (v2) still has to be designed and
  reviewed with the OpenCoven/coven#858 owner before any production path can
  launch through this backend, and `OpenCoven/sdk#199` must then follow it.
