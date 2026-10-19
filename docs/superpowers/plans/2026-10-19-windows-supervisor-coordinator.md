---
title: "Windows supervisor coordinator implementation plan"
summary: "Connect the held image and creation-time job to independent controller and I/O lifetimes."
read_when:
  - Implementing the native Windows independent audit helper
source_adjacent_reason: "Defines the remaining private supervisor coordination and native proof boundary for issue 1378."
---

# Windows Supervisor Coordinator Implementation Plan

**Goal:** Implement the native helper/controller, not merely another metadata
primitive. Keep standalone Windows execution refused until the producer,
consumer and native containment matrix prove the complete boundary. Preserve
all original #1378 and #858 acceptance and #857 gates.

**Architecture:** Reuse the delivered locked image, creation-time suspended Job
Object launch, bounded launch/body protocol and independent controller lease.
The helper receives exactly the subject executable and working-directory paths
as opaque OS arguments. Its four inherited pipes are input, raw subject stdout,
raw subject stderr and a duplex control/lease pipe. The independent subject's
digest remains separate from the versioned runner/helper artifact.

**Tech stack:** Private Rust supervisor package, existing serde/windows-sys,
Node 24 runner/audit, native Windows CI. No new authority provider or sandbox.

## Protocol and lifetime decisions

- Launch input retains the existing bounded JSON header followed by exactly the
  declared binary body and EOF. Do not read or write this on the coordinating
  thread. Before a header is available, use a fixed ten-second startup guard;
  after parsing, enforce its validated timeout against the original start time.
  These cooperative guards are not a ratified SLO or hard filesystem deadline.
- The control pipe emits LF-terminated JSON records. Legal sequences are a
  single `refused` after a valid header but before readiness, or `ready` followed
  by exactly one `exited` or `refused`. Reject premature `exited`, duplicate
  readiness/terminal messages and trailing records. Each record has the existing ABI and exact nonce,
  is at most 256 bytes including LF, and contains only fixed fields/codes or an
  unsigned 32-bit exit code. Before a valid header supplies a nonce, refuse by
  fixed helper exit status rather than fabricating a correlated record. Never
  put paths, arguments, payloads or raw errors in control records. Keep the pipe
  open until helper exit. Helper exit 0 is valid only for ready/ACK followed by
  exited (whose target exit code may be nonzero). Refusals use helper exit 2;
  cleanup_unproven uses exit 3. A valid refusal frame remains a failed invocation,
  never target execution evidence. Before a valid nonce, exit 2 without a frame;
  missing, partial or contradictory records never become successful evidence.
- After receiving ready, Node sends the existing exact `GO <nonce>\n` ACK and
  keeps its lease open. The helper must not resume the suspended root until the
  ready write and ACK are observed. Failure/deadline always takes priority over
  readiness, root exit or apparent I/O completion. A lease is not authentication.
  Finish the ready write before starting the blocking ACK/lease reader, and stop
  that reader after job/I/O cleanup before the final control write. This also
  avoids depending on simultaneous read/write through a synchronous pipe handle.
  Main-thread deadline checks continue while each control writer runs. The Node
  consumer must verify ACK write completion before accepting exited, buffering
  at most the one bounded terminal record if it races the ACK completion callback.
  The consumer must service control messages while body writes are backpressured;
  never await the complete input write before handling ready/ACK.
- Header input, held-image preparation/suspended spawn, input forwarding,
  stdout, stderr and control writes require independently owned workers. The
  main coordinator polls their bounded results, lease state and exact process
  handle; it never blocks on target/controller stream I/O. Preserve buffered
  binary body bytes when transferring the header reader to the input pump.
- Native pipe reads/writes must support synchronous and overlapped handles.
  Every pending operation retains its buffer, operation record, event and handle
  until terminal completion. Cancellation requests do not prove completion.
  Normalize actual pipe EOF separately from other errors; never turn an I/O
  failure into successful end-of-stream.
- Start independent target output pumps and enforce the runner's bounded output
  policy before accumulating/forwarding excess bytes: one MiB each for Windows
  stdout and stderr, with only a one-byte overflow sentinel read beyond each cap.
  POSIX stderr discard behavior is unchanged. A blocked downstream
  writer cannot prevent controller-loss/deadline observation or job termination.
- On any refusal, kill the owned job before cooperative worker teardown. On
  root exit, terminate remaining descendants and prove zero active job members;
  root success alone cannot satisfy input completion, output EOF or cleanup.
  Accept success only after exact input/EOF, both output pumps, job emptiness and
  all worker/lease cleanup are proven. Cleanup failure overrides an otherwise
  successful or ordinary refusal outcome with `cleanup_unproven`.
- Group worker cancellation under one bounded cooperative join guard, retaining
  live resources if a worker cannot finish. Combine this with the job and lease
  cleanup guards explicitly; do not multiply unbounded per-worker waits. The
  Worker termination alone does not release resource-valued results queued for
  the coordinator. Drain every completed preparation result and either assume
  its job/image ownership or explicitly terminate/drop it before declaring
  resource cleanup. A worker panic or detached timeout remains a permanent
  cleanup failure even if shutdown is called again. The
  standalone helper exits on unproven cleanup so kernel job-handle closure is
  the independent containment backstop. Suppress private panic diagnostics in
  the standalone binary, never globally in Coven's production process.

Windows distinguishes synchronous from overlapped pipe I/O; Node exposes an
explicit `overlapped` stdio option. Preserve both handle modes in native fixtures
and prove the actual chosen Node 24 startup contract. Sources:
[Microsoft pipe I/O](https://learn.microsoft.com/en-us/windows/win32/ipc/synchronous-and-overlapped-input-and-output)
and [Node child process stdio](https://nodejs.org/docs/latest-v24.x/api/child_process.html#optionsstdio).

## Task 1: Closed control framing

Files: `src/control.rs`, `src/lib.rs` in the private supervisor package.

- [x] Specify independently parsed ready/exited/refused records and reproduce
  positive behavioral RED before implementing their encoders.
- [x] Validate nonce, closed failure vocabulary, full u32 exit range, LF/length
  bound and minimized errors. No control payload may contain caller text.
- [ ] Wire these encoders into the actual coordinator; framing alone is not a
  delivered helper or native containment proof.

## Task 2: Cancellable native I/O ownership

Files: private native pipe/worker modules and the existing lease module only
where shared mechanics genuinely remove duplication.

- [ ] Add read/write, EOF/error, partial-transfer and cancellation fixtures for
  synchronous and overlapped byte pipes. Observe kernel-pending I/O before
  cancelling; avoid scheduler-sensitive promptness assertions.
- [ ] Retain storage until terminal I/O completion. Verify explicit group join
  outcomes; detach only with `cleanup_unproven` and an immediate helper exit.
- [ ] Preserve existing lease fixtures and host/Windows Clippy with denied warnings.

Write-only named-pipe handles require `FILE_READ_ATTRIBUTES` as well as write
access for byte-pipe metadata validation. Keep refusal when metadata cannot be
verified; do not silently accept an uninspected stream. See
[GetNamedPipeInfo access requirements](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-getnamedpipeinfo).

## Task 3: Native executable and coordinator

Files: private supervisor binary and native coordinator, existing image/job APIs.

- [ ] Acquire only validated noninheritable duplicates of the four-pipe contract.
- [ ] Supervise header/startup, image lock/suspended creation, ready/ACK, target
  input/output and final cleanup independently with fixed failure categories.
- [ ] Prove native ordinary success/nonzero exit, invalid spawn/image, header and
  ACK stalls, truncated/surplus input, blocked stdin/output, output overflow,
  deadline, controller death and descendants surviving root exit attempts.
- [ ] Verify no target entry before ACK and no surviving job members on cleanup.
  Child entrypoint tests may be ignored only when owning parent fixtures run them.

## Task 4: Producer/consumer integration and platform evidence

- [ ] Build the helper from the immutable common source, verify native format,
  host architecture and ABI, and bind its private bytes with the runner artifact.
- [ ] Recompute all bindings in the Node consumer before launch. Validate closed
  control frames/nonce, require complete stream closure as well as helper exit,
  cap all queues, contain helper cancellation and retain
  independent subject identity. Preserve Linux/macOS behavior.
- [ ] Exercise actual Node 24 native startup handles and controller-death/blocked
  I/O matrix in Windows CI. Remove Windows refusal only when support is proven.
- [ ] Record exact source/runner/vector/helper/subject and environment/toolchain
  evidence. Keep audit_only, full profiles, SLOs and eligibility distinct.
- [ ] Complete guards, independent source review, signed/DCO commit, exact-head
  CI, protected landing, fetched-main proof, restorable archive and owned cleanup.
  Do not close #1378 or #858 on partial implementation or a passing rerun alone.

## In-progress verification (not native execution)

The pipe adapter and bounded worker group are connected to a draft standalone
helper coordinator. Its Node24 native fixture and helper executable type-check
for Windows but have not executed there. Host tests cover control/output framing and worker result,
shutdown, panic, capacity and sticky-failure behavior. Native fixtures now include
synchronous/overlapped writes, actual EOF versus errors, empty peer writes before
payload, pending-write cancellation and group cancellation of three kernel-pending
reads with writers kept open. Windows cross-target Clippy checks types/lints only;
these native fixtures still require Windows execution. The coordinator must consume
all queued resource results as well as join workers; neither condition alone proves
job cleanup. Standalone Windows execution remains refused.

The draft coordinator now owns header/preparation/control/input/output workers,
requires ready plus ACK before resume, terminates descendants on root exit,
drains queued preparation ownership during cleanup, and preserves natural lease
failure separately from intentional stop. It rechecks the original deadline
before final success and exits immediately on unproven cleanup. The native
fixture covers success/nonzero output, invalid header/digest, total deadline
without ACK, surplus input and overflow. It reports whether a total deadline
actually reached ready; pre-ready expiry is not ACK-phase proof. Controller
death, blocked-I/O and descendant matrix cases plus production Node binding
remain unfinished. Windows CI pins the repository's Node version for the fixture.

The native fixture source now additionally exercises atomic PID readiness,
descendants retaining inherited streams beyond root exit, post-ACK controller
loss under observed stdin/stdout backpressure, an actual killed Node controller,
header stall, truncated body, invalid PE/cwd/ACK, running deadline and stderr
overflow. Native execution is still pending. Backpressure observations do not
prove a particular kernel operation is pending, and the controller-death fixture
checks the combined lease/job-close boundary rather than isolating either one.
Fixed per-case phase results and Node/host/helper/subject identities are emitted
through successful libtest capture for later CI evidence review. The binary name
matches the existing two-file runner artifact (`coven-conformance-supervisor.exe`).

The inline runner control reader and helper invocation consumer are now drafted.
They reject ambiguous/oversized/trailing or wrongly ordered control records,
require completed ACK/input writes, all readable EOFs and helper closure/status,
cap both output streams, retain the lease and minimize failure output. A missing
helper close remains failure; the fallback releases owned streams and event-loop
references after the failed termination guard. An actual-process host fixture
proves ACK service while a four-MiB input is backpressured; this is protocol/I/O
coverage, not Windows containment. All 101 existing/new runner and audit tests
pass locally. Native fixture source also calls this consumer with valid UTF-8
and invalid binary output. Producer/source/helper artifact binding, production
selection, native execution and protected delivery remain pending. The explicit
Windows CLI refusal is unchanged until the full native proof is available.

Producer and consumer binding are now connected behind the preserved Windows
refusal. The producer selects locked helper Cargo metadata from the same pinned
source, checks debug/non-test/exact manifest and target/native host/PE metadata,
publishes exclusive helper bytes and a canonical two-file manifest, and rechecks
all output bindings after execution. The consumer recomputes that manifest,
version/source/host/vector and independent subject bindings before private staging.
All 103 Node runner/audit tests pass. The native fixture now also builds an actual
helper from a private pinned Git clone using a separate Cargo target and invokes
that produced artifact through the consumer. This native path is only cross-
checked so far. Full Windows audit activation and all native runtime results are
still pending; neither mock metadata nor cross-target checks establish them.
