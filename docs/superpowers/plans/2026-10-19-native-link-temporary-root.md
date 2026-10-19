---
title: "Native-link installer temporary-root correction"
summary: "Keep installer metadata within the explicitly selected temporary root."
read_when:
  - Maintaining the native-link dependency installer
source_adjacent_reason: "Records the discriminating regression and delivery requirements for issue 1408."
---

# Native-link Temporary Root Implementation Plan

> **For agentic workers:** Use executing-plans inline, with independent review before publication.

**Goal:** Correct #1408 without relaxing cleanup containment or attributing this local installer failure to the original #1227 Windows delay.

**Architecture:** Pass an explicit quoted template to the existing atomic `mktemp -d` invocation. Keep sudo cleanup and exact owned-parent assertions unchanged. A portable shim models a platform default outside TMPDIR, delegating explicit templates to the real executable.

**Tech Stack:** Bash, Python unittest, native BSD and GNU mktemp.

## Task 1: Discriminate the old behavior

- [x] Add `test_temporary_root_is_explicit_when_platform_default_ignores_tmpdir` to `scripts/install-native-link-dependencies-test.py`; all simulated default paths belong to the outer fixture.
- [x] Run `python3 scripts/install-native-link-dependencies-test.py -k temporary_root` against the unchanged installer. Retain the cleanup assertion failure.

## Task 2: Correct the owning boundary

- [x] Replace `workdir="$(mktemp -d)"` in `scripts/install-native-link-dependencies.sh` with `workdir="$(mktemp -d "${TMPDIR:-/tmp}/coven-native-link.XXXXXXXXXX")"`.
- [x] Run the complete installer suite with actual BSD and GNU executable selection, retaining selected paths, hashes, output and status. Preserve update/install error propagation and exact cleanup assertions.
- [ ] Independently review the patch and causal limits, run required Rust and full Policy checks plus staged/range guards, then sign and publish a scoped PR.
- [ ] Verify protected delivery and fetched main, retire only owned clean state, close #1408 and integrate that delivered main into #1382. Keep #1227 open.
