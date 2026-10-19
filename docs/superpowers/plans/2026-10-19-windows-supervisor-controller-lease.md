---
title: "Windows supervisor controller lease implementation plan"
summary: "An independent inherited pipe detects controller loss while target I/O is blocked."
read_when:
  - Implementing the independent Windows process supervisor
source_adjacent_reason: "Documents the native audit helper's private launch and cleanup boundary."
---

# Windows Supervisor Controller Lease

**Goal:** Obtain a distinct controller pipe without probing arbitrary CRT descriptors, watch the exact nonce acknowledgement and subsequent controller loss on an independent thread, and provide bounded cooperative watcher cleanup. Native fixtures must observe a kernel-pending read before cancellation. The helper's future native coordinator must react to loss by terminating its owned Job Object.

**Architecture:** Node spawns exactly four pipe descriptors. The fourth is the dedicated lease and is excluded from target handle inheritance. Validate the libuv startup descriptor table and duplicate only its lease handle as noninheritable; do not take ownership of the CRT's original handle. Verify a byte pipe at the native boundary. Use native `ReadFile` with a unique operation/event for both synchronous and overlapped pipe handles; never call Rust `File::read` on an unproven handle mode. The watcher handles only this pipe, so target input/output cannot consume its thread. State is monotonic awaiting-ACK, acknowledged, failed; after ACK any byte, EOF or read error is failure. Native process coordination must prioritize failed state, enforce the independent launch/run deadline and prove final job cleanup.

Primary sources: [libuv stdio table and flags](https://github.com/libuv/libuv/blob/v1.x/src/win/process-stdio.c), [libuv pipe construction](https://github.com/libuv/libuv/blob/v1.x/src/win/pipe.c), [GetStartupInfoW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getstartupinfow), and [CRT invalid-descriptor behavior](https://learn.microsoft.com/en-us/cpp/c-runtime-library/reference/get-osfhandle).

## Task 1: Test bounded decoding before implementation

- [x] Decode only the expected count of four descriptors and exact table length for 32/64-bit little-endian HANDLE representations. Require open byte-pipe CRT flags for all four, distinct real nonzero handle values and no null/pseudo handles.
- [x] Refuse all truncated prefixes, count/width/size/flag/alias/invalid-handle errors without a panic or private data in errors.
- [x] Preserve the missing-decoder RED and common-test GREEN.

## Task 2: Implement and typecheck native acquisition and watcher

- [x] Get the current process's OS-owned startup table, cap it to the expected native size, decode before any selected handle use, and duplicate the lease with inheritance disabled. Reject nonpipe/message-pipe handles with fixed `launch_invalid`.
- [x] Read ACK on a dedicated named thread, publish acknowledged state, then treat any subsequent read completion as loss. A caller-provided cancellation flag is checked before every read so `read_exact` retries cannot defeat native cancellation.
- [x] Stop the watcher by setting cancellation, repeatedly cancelling its synchronous I/O while the worker cancels an exact pending overlapped request and checking the exact thread handle. The ten-second wait is a cooperative cleanup hang guard, not an SLO; explicit stop returns `cleanup_unproven` on failure. Keep worker-owned resources live if a thread must be detached after failure.
- [x] Add real native pipe fixtures for ACK plus EOF/data, wrong ACK and stopping a blocked pre/post-ACK read; assert private duplicate inheritance and nonpipe refusal. Exercise synchronous and overlapped pipes, observe `GetThreadIOPendingFlag` before cancelling pre/post-ACK reads, and prove data failure without closing the writer.
- [ ] Run host common tests and host/Windows all-target Clippy with warnings denied; inspect lockfile metadata for existing dependency changes.
- [x] Independent source review and correction of required findings.

These checked implementation steps have host/common or cross-target source evidence only. The Windows fixtures have not executed locally. Review corrected an unverified synchronous-handle assumption by replacing Rust `File::read` with mode-aware native I/O. Pending operation/buffer/event storage stays live until completion, even when cancellation is requested or waiting fails. Explicit stop may detach a worker after its cleanup guard; the future coordinator must report that failure and exit after job cleanup.

## Task 3: Required native integration proof

- [ ] Prove actual Node 24 startup-table/pipe compatibility on Windows; no architecture/platform support is inferred from metadata decoding or cross-compilation.
- [ ] Connect watcher failure/deadline to pre-resume refusal and final Job Object termination, with image locks, target input pump and bounded output ownership.
- [ ] Prove actual controller death, blocked target I/O, timeout/cancellation, descendants and cleanup on native Windows before removing the existing Windows refusal.
- [ ] Complete producer/consumer exact artifact/source bindings, local guards, signed/DCO commit, exact-head CI and protected delivery before closing #1378.

A watcher alone does not kill a process tree. Startup-table validation does not authenticate a controller or artifact, and table-format support remains a private libuv compatibility contract. This work does not establish Runtime Authority, complete #858 profiles, measured SLOs or authenticated release eligibility.
