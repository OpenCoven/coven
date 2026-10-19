---
title: "Windows supervisor exact executable boundary"
summary: "Hold a bounded local image and non-reparse ancestry stable while hashing and creating a process."
read_when:
  - Implementing the independent Windows process supervisor
source_adjacent_reason: "Records the selected-image invariant and native proof required by the audit helper."
---

# Windows Supervisor Executable Lock

**Goal:** Execute the exact selected private `.exe` bytes, with fixed refusals for unsafe names, mutation, malformed PE metadata, architecture mismatch and digest mismatch.

**Architecture:** Common code bounds PE32+ metadata, Windows path components and streaming SHA-256. Native code opens a local fixed-drive root and each directory ancestor top-down with read sharing only, rejects reparse points, and retains those handles. Directory handles request list/read-data access as well as attributes, so the sharing exclusions are effective. Resolve the held root to a pure kernel `HarddiskVolume` root and construct the internal `GLOBALROOT` path from that OS result, preventing session-local drive alias changes from redirecting later lookups; reject SUBST/subdirectory roots. Caller-provided arbitrary device paths remain refused. Open the final regular file without reparse traversal and with write/delete sharing denied; validate metadata and hash that same held file. A spawn method borrows this owner through the existing creation-time Job Object operation. No shell or path search is involved. The file and directory locks are not a filesystem/network sandbox or authenticated producer.

The [CreateFileW share-mode contract](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew) documents exclusion of existing writable mappings when write sharing is omitted. Keep a native mapping fixture; do not infer Windows behavior from host-only parsing or cross-compilation.

[Kernel path naming](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file) and `GetFinalPathNameByHandleW` describe the native-volume anchor. Its actual use for process creation still needs native fixture proof.

## Task 1: Common RED then GREEN

- [ ] Add bounded metadata/path/hash tests before implementations and preserve actual positive-case failures.
- [ ] Accept DOS-drive and verbatim DOS-drive absolute paths only; reject UNC/device/relative paths, ambiguous separators/components, ADS, NUL/control/metacharacters, trailing-dot/space aliases and non-`.exe` names. Preserve opaque UTF-16 units. Cap path length at 8192 units, components at 255 units, reserved DOS device names (including superscript variants), and directory locks at 128.
- [ ] Read only the DOS header, COFF header and fixed PE32+ optional prefix, validating complete optional/section-table bounds. Identify x64/ARM64 metadata; cap file bytes at 256 MiB and declared image size at 512 MiB. These are implementation refusal bounds, not measured/ratified SLOs or full loader validation.
- [ ] Stream exactly the declared file length with a fixed 64 KiB buffer; reject truncation, surplus, over-budget declarations and I/O errors with fixed `launch_invalid`.

## Task 2: Native boundary and fixtures

- [ ] Hold local fixed-drive ancestry and final file open with read sharing only and no final-component reparse traversal. Check handle metadata, not a separate path stat.
- [ ] Refuse unsupported native architectures, malformed metadata and mismatched lowercase SHA-256 before process creation.
- [ ] Borrow the owner through creation-time contained spawn so the selected path cannot be replaced during that operation.
- [ ] Add real Windows fixtures for exact digest/PE, write/rename/hardlink mutation denial, preexisting writable handles/mappings, digest mismatch, malformed inputs and released locks. Native runtime evidence is required.
- [ ] Host package tests, host/Windows all-target Clippy, formatting, docs/privacy/secret guards and independent source review.

## Task 3: Remaining integration gates

- [ ] Native reparse/ancestor substitution and actual helper/controller-death fault evidence.
- [ ] Connect the helper executable, exclusive control output, controller lease, input pump and process cleanup; verify Node producer/consumer artifact/source binding.
- [ ] Actual native Windows CI at the exact source and vector set before changing the existing Windows refusal.
- [ ] Full local and protected delivery gates. Keep #1378 and full #858 open until their original acceptance is proven.

File reads, native filesystem operations and loader calls do not acquire hard wall-clock guarantees here. The controller must supervise setup as well as execution. This boundary does not authenticate DLL dependencies, caller toolchains, complete profiles or exact-release eligibility.
