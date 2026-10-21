---
title: "Cloud contributor setup"
summary: "Repository-pinned setup and evidence for managed contributor tasks."
read_when:
  - Preparing a managed Cloud environment
  - Diagnosing contributor readiness
source_adjacent_reason: "Documents the setup entry points and capability report beside their implementation."
---

# Cloud contributor setup

These scripts provision contributor tools and report build/test capabilities.
Managed environment publication remains a separate human action. A passing local
fixture suite or a local macOS checkout is not fresh Cloud task evidence.

## Entry points

Run from a clean checkout with Python 3.11+ (including maintained tarfile security
filters), Git, Bash, and a dedicated external cache:

```sh
bash scripts/cloud/bootstrap.sh --cache /opt/coven-cloud-cache
bash scripts/cloud/readiness.sh --cache /opt/coven-cloud-cache
```

Choose a cache path writable by the setup/task user that the managed environment
persists into fresh tasks. The example path is not automatically authorized or
created with elevated privileges. Bootstrap supports Linux x86_64/aarch64 and
uses the native installer to validate Debian/Ubuntu sources. System package
installation requires root or non-interactive sudo. Readiness never installs
system packages. Missing prerequisites fail explicitly.

Bootstrap reads `rust-toolchain.toml`, `.node-version`, and each package's
`packageManager`. It installs the Rust channel/profile/components/targets with
rustup, verifies the official Node archive checksum, installs exact package
managers, and installs package dependencies with lockfiles. Channels uses npm ci;
OpenClaw uses pnpm's frozen lockfile and disables installation scripts as in CI.
The repository and external cache must not overlap. Concurrent bootstrap runs on
the same cache are rejected. Repeating bootstrap reuses installed tool artifacts
but reruns dependency installation and readiness; no marker file proves readiness.

Readiness checks actual versions, builds the workspace with `--locked`, executes
the built CLI's `doctor --json`, and runs the locked CLI smoke suite. Runtime HOME
and Coven configuration are disposable and isolated. Existing smoke tests own
their test harness fakes; the actual doctor command receives no fake harness.
Provider credentials and arbitrary inherited environment are not forwarded.
Managed proxy and certificate settings are preserved. Neither script changes
network policy or attempts an authenticated provider turn.

## Read the report

Both entry points emit JSON and exit nonzero on a required failure:

- `development_ready` means the required contributor build/diagnostic/smoke steps
  succeeded under the reported pins and source SHA.
- `doctor.exit_code`, `doctor.failed_checks`, and its capability flags retain the
  actual diagnostic result. A missing harness or missing engine is a completed
  contributor diagnostic, **not a passing doctor or usable chat session**.
  Other doctor failures, malformed output, or timeouts block development readiness.
- `operational_ready` remains false when the doctor reports missing capabilities
  or a later required stage fails. `provider_authentication` is always `unverified`.
- `stages` records exit codes and timeouts. `blocked_stage` identifies the failure.
  Bootstrap nests the full readiness report under `readiness`.

Raw child output is withheld to avoid emitting credentials from tool diagnostics.
A denied download, missing privilege, mismatched checksum, dirty tracked source,
or changed tracked source fails setup. There is no policy fallback or automatic
source repair. Preserve unexpected source changes for inspection. For incomplete
or redirected tool storage, choose a new dedicated external cache; do not delete
unrelated directories. For a denied host, record the stage and official endpoint
for the environment owner's review rather than broadening access automatically.

## Verification and managed handoff

Local fixture tests run without provisioning tools or contacting providers:

```sh
python3.12 scripts/cloud/config-test.py
python3.12 scripts/cloud/runtime-test.py
python3.12 scripts/cloud/provision-test.py
python3.12 scripts/cloud/bootstrap-test.py
python3.12 scripts/cloud/readiness-test.py
bash -n scripts/cloud/bootstrap.sh scripts/cloud/readiness.sh
```

Use an available maintained Python 3.11+ in place of `python3.12`. Bootstrap and
readiness orchestration tests inject mock command results. Artifact tests use
synthetic archives. These do not prove downloads, APT installation, real toolchain
execution, or fresh-task behavior.

Before publication, record real managed-environment evidence on #1446/#1442:
exact reviewed SHA, environment identity, distro, pins, bootstrap report, repeated
bootstrap report, unchanged tracked source, observed network destinations, and
fresh-task readiness. Resolve policy through #1447. Have the human owner review
and publish the tested setup, record that publication, then repeat readiness in
a second fresh task. Keep missing evidence explicit; do not close the epic from
fixture results alone.
