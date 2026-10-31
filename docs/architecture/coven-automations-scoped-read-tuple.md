---
title: "Automations scoped read tuple"
summary: "Proposed exact tuple and threat model for certifying the project-only scoped read Runtime Authority profile, with each alternate effect route classified by what enforces it."
read_when:
  - Certifying the scoped read Runtime Authority profile
  - Changing the r0-read.2 envelope, its admitted harness version or admission
  - Deciding what a scoped read run may claim about containment
source_adjacent_reason: "Records the tuple, threat model and route evidence that issue 1608 certifies against the scoped read service and gate implementation."
---

# Automations scoped read tuple

Status: **proposal for Val's approval, not certified**. This page names one
exact tuple for the project-only scoped read profile (coven#1608) and the
narrow claim its evidence supports. It does not advertise Runtime Authority,
lift the definition opt-in, or describe a release. Activation, release
(coven#805) and Windows runner containment (coven#1378) stay their own gates.

## The proposed tuple

| Element | Value |
| --- | --- |
| Daemon | `coven` built from the head of the #1608 pull request, stacked on #1635 (#1606) and the #1603/#1604 trusted adapter |
| Harness | Claude Code **2.1.305**, the native `darwin-arm64` build. The npm package is `claude-code-darwin-arm64`, version 2.1.305, in the Anthropic scope. Tarball sha256 `c53676323f3b4e150e10507b26d914264fd6e11fdb7240b47b36e3601e887b42` (matches the registry's integrity), binary sha256 `ea3ea3c08d2a9634a4651e687fcf900d410c489bce2b00f22bd7352b91ba704f` |
| Adapter | `trusted_launch::prepare` with `StoreGate`, settlement reconciliation (#1606) and the admission refusals below |
| Envelope | `r0-read.2` (`CLAUDE_R0_SCOPED_READ`), descriptor digest `bb98895d2eceb4b926a25b1bc51cbfa03ed311c76dd7eb97f2e1c19beb47c2a2`, pinned by `runtime_envelope::tests::the_scoped_read_descriptor_digest_is_pinned` |
| OS and kernel | macOS 27.0.1 (26A434), Darwin 27.0.0, arm64 |
| Filesystem | APFS. Project folder and `COVEN_HOME` on the same local volume |
| OS enforcement backend | **None.** See below |
| Configuration | No machine policy for the harness; owner's API key saved; project folder beneath, or outside, the owner's home |

Any other harness version, build, platform or envelope is a different tuple
and is unsupported until it has its own evidence.

### Why there is no OS backend

The only native backend, `coven-restricted-runtime-macos`, enforces a sealed
offline worker: no fork, no exec of another program, no network, no writes.
Claude Code needs all four: it starts the `coven` relay as its MCP server,
calls the Messages API, and writes into its home. The backend cannot host
the harness, and no other backend exists for this platform. So the tuple
**does not claim process containment**. Every restriction it does claim is
listed below with what enforces it.

## Threat model

**Protected:** the owner's files outside the grant, the owner's harness and
credential folders, `COVEN_HOME`, and the integrity of a run's settlement and
receipt.

**Trusted:** the daemon, the store, the OS user account and kernel, and the
installed harness binary of the tuple behaving as tested.

**Adversary:** the model. It chooses every tool call and argument, and can
try any tool name, any path, and any instruction to the harness. Files in
the project can also carry instructions to the model.

**Out of scope:** a hostile harness binary or a hostile process running as
the owner. Either can read anything the owner can, because nothing below
the harness's own tool gate stops it. Defending against them needs the OS
enforcement this tuple does not have.

## Alternate effect routes

Each route is classified as **refused by the harness flags** (the tested
Claude Code 2.1.305 behavior under the envelope's arguments), **refused by
Coven**, or **needs OS enforcement** (not claimed). Installed-harness tests
are `#[ignore]` and opt-in: they need `claude` 2.1.305 on `PATH` and
`COVEN_TEST_RELAY_BIN`.

| Route | Classification | Evidence |
| --- | --- | --- |
| Built-in tools: `Bash`, `Read`, `Write`, `Edit`, `WebFetch`, `Task` | Harness flags (`--tools ""`) | `claude_runs_no_tool_it_was_not_offered`: each call answers "No such tool available"; no escape file, no fetch, secret untouched, no service operation, stream not complete |
| Another MCP server's tool | Harness flags (`--strict-mcp-config`) | Same test (`mcp__other__run`); `claude_reaches_only_the_service` |
| `.mcp.json`, `~/.claude.json` servers, `enableAllProjectMcpServers` | Harness flags (`--strict-mcp-config`, `--setting-sources ""`) and Coven (empty launch folder and home) | `claude_ignores_widening_settings_in_its_home_and_folder` |
| User, project and local settings: hooks, permissions, `bypassPermissions`, `env` | Harness flags (`--setting-sources ""`, `--bare`) and Coven (empty launch folder and home) | Same test: no hook ran, `Bash` stayed absent, the granted read still ran. Control: the same hooks run in a plain launch |
| Machine policy: `managed-settings.json`, `managed-settings.d/`, `managed-mcp.json`, managed preferences plists (`/Library/Managed Preferences/…/com.anthropic.claudecode.plist`) | **Coven**: admission and preflight refuse while any of these exist (`MachinePolicy`). The harness loads them as `policySettings` whatever `--setting-sources` says | `admission::tests::machine_policy_for_the_harness_refuses_the_launch` (red without the check), `the_machine_policy_paths_are_the_harnesses` |
| A policy folder named in the environment (`CLAUDE_CODE_MANAGED_SETTINGS_PATH`) | Coven (the environment is exactly the envelope's) and the harness (2.1.305 does not honor it) | `claude_takes_no_policy_folder_from_its_environment` |
| Remote (server-managed) settings | Not observed under API-key authentication with nonessential traffic disabled. Not otherwise enforced | Same test: only `POST /v1/messages` left the harness |
| Inherited environment | Coven: exactly `ANTHROPIC_API_KEY`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `DISABLE_AUTOUPDATER`, `HOME`, `PATH` | `a_scoped_read_launch_runs_under_the_store_gate_in_an_empty_folder`, `the_environment_is_exactly_the_envelopes` |
| Inherited descriptors | Coven: none beyond standard input, output and error | Same daemon test: descriptors 3 to 9 cannot be duplicated. Checked by hand, the probe reports a descriptor that is passed |
| Reads outside the grant through the service | Coven: the scoped reader, the gate and settlement | Existing service, gate and trusted launch tests |
| Owner's harness and credential folders, `COVEN_HOME` | Coven, through the service only | `the_owners_harness_folders_and_coven_home_are_protected` |
| A project folder that is the filesystem root, the owner's home or a folder holding it | **Coven**: admission refuses (`ProjectUnsupported`) | `a_project_folder_that_holds_the_owners_home_is_refused` (red without the check: `/` was admitted) |
| Subprocesses the harness starts itself | **Needs OS enforcement** | Not claimed |
| Network other than the Messages API | **Needs OS enforcement**. Model-requested fetches are refused by the flags | Not claimed |
| Direct file access by the harness process, credentials included | **Needs OS enforcement** | Not claimed |
| Harness version | Coven: the version the harness reports must be admitted | `a_launch_is_refused_before_it_starts`, `an_unadmitted_harness_version_burns_no_decision` |
| Harness binary identity | **Not enforced.** The version is self-reported. See the open decisions | — |

## Authority ending during a run

The gate checks the attempt's authority at every read
([gate proposal](coven-automations-scoped-read-gate-proposal.md)). Through the
live daemon, with the familiar revision revoked, the definition paused or
the run's deadline passed after the harness starts, the read is withheld,
no byte of it reaches the harness, and the service stops
(`a_run_whose_authority_ends_while_it_runs_reads_nothing_and_never_succeeds`).

Before #1608, settlement then settled those runs **succeeded, with a
receipt**: the stand-in harness claimed both reads, and a withheld operation
reconciled. Settlement now holds such a run for recovery
(`scoped_read_operation_withheld`). A crash after a commitment leaves no
close, so after restart the run neither settles nor gets a receipt
(`a_crash_after_commit_keeps_the_row_and_restart_sweeps_the_channel`).

## The claim this evidence supports

> Under the tuple above, with a harness that behaves as Claude Code 2.1.305
> was observed to, a scoped read run discloses project files to the model
> only through the scoped read service, within its signed grant, while its
> authority holds; its settlement never reports a read the gate did not
> commit and deliver; and admission refuses a machine whose harness policy,
> or a project folder, would widen that.

It does **not** claim that the harness process is contained, that a hostile
harness or process cannot read the owner's files, or that the network is
restricted. A request that needs those must be refused until an enforcing
backend exists.

## Open decisions for Val

1. **Approve or amend the tuple** above, including the platform: the
   evidence is from one macOS arm64 machine. Linux paths for machine policy
   are implemented (`/etc/claude-code`) but untested against the harness.
2. **Pin the harness binary.** Admission trusts the version the binary
   prints. Pinning the sha256 of the resolved binary would make the tuple's
   harness exact, and would refuse every other build, including npm
   installs of the same version.
3. **Withheld runs are held, not failed.** A run whose authority ended mid-run
   is held for operator recovery, the existing fail-closed path. Settling it
   as failed or cancelled instead is a settlement-semantics choice.

## Remaining assumptions

- The harness flags behave as observed for 2.1.305; another version is
  refused, not assumed.
- Remote managed settings are not fetched under API-key authentication. This
  is observed, not enforced.
- CI does not run the installed-harness tests; macOS CI jobs on the adapter
  stack are skipped. The evidence comes from local runs, with the counts
  recorded in the pull request.
