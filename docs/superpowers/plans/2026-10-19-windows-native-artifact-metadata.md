---
title: "Windows native artifact metadata implementation plan"
summary: "First artifact-identity prerequisite for the full Windows independent-runner outcome."
read_when:
  - Implementing issue 1378 platform metadata and native containment
source_adjacent_reason: "Records the audit parser boundary and verification next to its source-owned runner."
---

# Windows Native Artifact Metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bounded PE32+ architecture/header observation as the first executable-identity prerequisite for #1378, without enabling Windows execution.

**Architecture:** Extend the existing audit header inspector in place. Keep the closed job/result binding and explicit Windows execution refusals unchanged. This component does not satisfy the full native containment, helper artifact/source binding, native fixtures or CI acceptance; those remain required under #1378 and #858.

**Tech Stack:** Node.js built-in Buffer/assert/test and existing audit error conventions.

## Full outcome and sequencing

The full task requires a Rust kill-on-close Job Object boundary attached atomically at process creation, explicit versioned helper/runner artifact binding, exact private subject identity, parent/controller and descendant cleanup proof, bounded minimized I/O and cleanup, and native Windows CI. Existing suspend-then-assign code cannot establish the required launcher-crash boundary. Use creation-time job assignment and a noninherited job handle; do not modify the unrelated production launcher in this component.

This component observes only image metadata. Recognizing an ARM64 or x64 header does not prove native execution or platform support. Do not infer image signature, complete loader validity, executable containment, profile completion or release eligibility.

[PE field definitions](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format) define the DOS pointer, COFF/magic fields, bounded section count and PE32+ fixed optional-header prefix used here. [Creation-time job assignment](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute) is the next separate native boundary.

## Task 1: Reproduce missing PE inspection

**Files:** Modify `conformance/automations/runner/audit.test.mjs` and the export on `nativePlatform` in `conformance/automations/runner/audit.mjs`.

- [x] Add `nativePlatform` to the existing named import from `./audit.mjs` and append these tests:

```js

function nativePeFixture(machine = 0x8664, sections = 1) {
  const bytes = Buffer.alloc(Math.max(512, 0x98 + 0xf0 + sections * 40));
  bytes.writeUInt16LE(0x5a4d, 0);
  bytes.writeUInt32LE(0x80, 0x3c);
  bytes.writeUInt32LE(0x00004550, 0x80);
  bytes.writeUInt16LE(machine, 0x84);
  bytes.writeUInt16LE(sections, 0x86);
  bytes.writeUInt16LE(0xf0, 0x94);
  bytes.writeUInt16LE(0x0002, 0x96);
  bytes.writeUInt16LE(0x20b, 0x98);
  return bytes;
}

test("native binary PE32+ headers identify architecture without claiming execution", () => {
  assert.deepEqual(nativePlatform(nativePeFixture()), { os: "win32", arch: "x64" });
  assert.deepEqual(nativePlatform(nativePeFixture(0xaa64)), { os: "win32", arch: "arm64" });
  assert.deepEqual(nativePlatform(nativePeFixture(0x8664, 96)), { os: "win32", arch: "x64" });
});

test("native binary PE metadata refuses truncated prefixes with fixed errors", () => {
  const bytes = nativePeFixture();
  for (let length = 0; length < 0x98 + 0xf0 + 40; length += 1) {
    assert.throws(() => nativePlatform(bytes.subarray(0, length)), (error) =>
      error.name !== "RangeError" && /^native binary /.test(error.message),
    `accepted truncated PE prefix: ${length}`);
  }
});

test("native binary PE metadata refuses incompatible and malformed images", () => {
  const mutations = [
    (bytes) => bytes.writeUInt32LE(63, 0x3c),
    (bytes) => bytes.writeUInt32LE(0xffffffff, 0x3c),
    (bytes) => bytes.writeUInt32LE(0, 0x80),
    (bytes) => bytes.writeUInt16LE(0x14c, 0x84),
    (bytes) => bytes.writeUInt16LE(0xffff, 0x84),
    (bytes) => bytes.writeUInt16LE(0, 0x86),
    (bytes) => bytes.writeUInt16LE(97, 0x86),
    (bytes) => bytes.writeUInt16LE(111, 0x94),
    (bytes) => bytes.writeUInt16LE(0xffff, 0x94),
    (bytes) => bytes.writeUInt16LE(0, 0x96),
    (bytes) => bytes.writeUInt16LE(0x2002, 0x96),
    (bytes) => bytes.writeUInt16LE(0x10b, 0x98),
  ];
  for (const mutate of mutations) {
    const bytes = nativePeFixture();
    bytes.write("PRIVATE-PE-MARKER", 8, "ascii");
    mutate(bytes);
    assert.throws(() => nativePlatform(bytes), (error) =>
      error.name !== "RangeError" && /^native binary /.test(error.message) &&
        !error.message.includes("PRIVATE-PE-MARKER"));
  }
});

test("native binary metadata keeps Mach-O and ELF recognition", () => {
  const macho = Buffer.alloc(64);
  macho.writeUInt32LE(0xfeedfacf, 0);
  macho.writeUInt32LE(0x01000007, 4);
  macho.writeUInt32LE(2, 12);
  assert.deepEqual(nativePlatform(macho), { os: "darwin", arch: "x64" });
  const elf = Buffer.alloc(64);
  elf.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]);
  elf.writeUInt16LE(2, 16);
  elf.writeUInt16LE(62, 18);
  elf.writeUInt32LE(1, 20);
  assert.deepEqual(nativePlatform(elf), { os: "linux", arch: "x64" });
});
```

- [x] Run `node --test --test-name-pattern='native binary' conformance/automations/runner/audit.test.mjs`. The first run fails because the inspector is not exported. Change its declaration to `export function nativePlatform(bytes)` without changing its body, then rerun to record PE refusal while legacy formats still pass.

## Task 2: Implement bounded header observation

**Files:** Modify `conformance/automations/runner/audit.mjs`.

- [x] Insert this complete helper immediately before `nativePlatform`:

```js
function nativePePlatform(bytes) {
  const offset = bytes.readUInt32LE(0x3c);
  requireAudit(offset >= 64 && offset <= bytes.length - 24,
    "native binary PE header offset is invalid");
  requireAudit(bytes.readUInt32LE(offset) === 0x00004550,
    "native binary PE signature is invalid");
  const sections = bytes.readUInt16LE(offset + 6);
  const optionalSize = bytes.readUInt16LE(offset + 20);
  const optional = offset + 24;
  const sectionTable = optional + optionalSize;
  requireAudit(
    optionalSize >= 112 && sections >= 1 && sections <= 96 &&
      sectionTable <= bytes.length && sectionTable + sections * 40 <= bytes.length,
    "native binary PE optional header or section table is invalid",
  );
  const arch = { 0x8664: "x64", 0xaa64: "arm64" }[bytes.readUInt16LE(offset + 4)];
  const characteristics = bytes.readUInt16LE(offset + 22);
  requireAudit(
    arch !== undefined && bytes.readUInt16LE(optional) === 0x20b &&
      (characteristics & 0x0002) !== 0 && (characteristics & 0x2000) === 0,
    "native binary architecture or PE32+ executable header is unsupported",
  );
  return { os: "win32", arch };
}

```

- [x] After the existing 64-byte minimum check, insert `if (bytes.readUInt16LE(0) === 0x5a4d) return nativePePlatform(bytes);`. Update the unsupported-format message to name PE32+ alongside Mach-O/ELF. Do not change `runAudit` or runner platform refusals.
- [x] Run the focused command above. All four focused tests must pass, including every incomplete prefix, malformed field, DLL/PE32/unknown-machine refusal and the legacy-format regression.
- [x] Run `node --test conformance/automations/runner/conformance.test.mjs conformance/automations/runner/audit.test.mjs`. The existing CI Policy guard already runs both files, so no workflow change is needed for this component.

## Task 3: Record scope and continue the full native boundary

**Files:** Modify `conformance/automations/runner/README.md`.

- [x] Add beside the existing Windows refusal: “PE32+ header inspection reports x64/ARM64 metadata only. It does not establish native execution, full image/signature validity, process containment or release eligibility. Windows execution remains refused pending native containment evidence.”
- [ ] Record focused RED/GREEN results and unchanged platform guards on #1378 and the task execution ledger. Continue the full native helper/binding/CI work; do not close #1378 or #858 after this component.
- [ ] Before any commit or PR, run the required repository guards and applicable complete local checks, source review, signed/DCO source binding and exact-head CI. Keep the full native-platform acceptance and explicit unsupported state until actual native evidence passes. Execute inline under the existing goal authorization; do not start implementation subagents.
