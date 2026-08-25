# Maintenance Participant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a maintenance command launched inside a Coven-managed session exclude only its own generation-bound writer intent while every other writer remains a blocker.

**Architecture:** Extend the Rust maintenance protocol with an optional `WriterParticipant` stored on the owner record. `WriterLease` serializes an exact id/generation capability into the harness environment, owner acquisition validates it under the gate lock, and status calculation removes only that exact generation from the blocking `writers` set. Direct, patch, daemon, and adopted launch paths inject the capability immediately before spawning the harness.

**Tech Stack:** Rust, `serde`/`serde_json`, `clap`, `portable-pty`, Cargo unit and integration tests.

---

## File map

- Modify `crates/coven-cli/src/maintenance_gate.rs`
  - Own the participant type, capability encoding/decoding, acquisition
    validation, owner persistence, and effective writer-set calculation.
- Modify `crates/coven-cli/src/main.rs`
  - Read the inherited capability for maintenance acquisition and propagate a
    writer capability into direct and patch harness commands.
- Modify `crates/coven-cli/src/pty_runner.rs`
  - Add one narrow `HarnessCommand` environment override method.
- Modify `crates/coven-cli/src/daemon.rs`
  - Add the capability to daemon/API-launched harness commands before spawn.
- Modify `crates/coven-cli/tests/smoke.rs`
  - Exercise the complete CLI contract with a fake harness process.
- Modify `docs/reference/cli-maintenance.md`
  - Document the corrected in-session maintenance behavior.

### Task 1: Define and enforce the participant protocol

**Files:**
- Modify: `crates/coven-cli/src/maintenance_gate.rs`

- [ ] **Step 1: Add failing tests for exact-generation participation**

Add these tests beside `owner_drains_existing_writer_then_becomes_held`:

```rust
#[test]
fn owner_excludes_only_its_exact_participant_writer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    let participant = gate.acquire_writer("session-self", "session")?;
    let blocker = gate.acquire_writer("session-other", "session")?;

    let mut owner =
        gate.acquire_owner("cave", Some(participant.participant().clone()))?;
    assert_eq!(owner.owner().phase, OwnerPhase::Draining);
    let status = owner.refresh_phase()?;
    assert_eq!(
        status.writers.iter().map(|writer| writer.id.as_str()).collect::<Vec<_>>(),
        vec!["session-other"],
    );

    drop(blocker);
    let status = owner.refresh_phase()?;
    assert!(status.writers.is_empty());
    owner.assert_held()?;
    owner.release()?;
    drop(participant);
    Ok(())
}

#[test]
fn owner_rejects_a_stale_or_forged_participant_generation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    let writer = gate.acquire_writer("session-self", "session")?;
    let mut participant = writer.participant().clone();
    participant.generation = "wrong-generation".to_string();

    let error = gate.acquire_owner("cave", Some(participant)).unwrap_err();
    assert!(error
        .downcast_ref::<GateError>()
        .is_some_and(|error| matches!(error, GateError::ParticipantInvalid)));
    assert!(gate.status()?.owner.is_none());
    drop(writer);
    Ok(())
}

#[test]
fn owner_does_not_exclude_a_replacement_writer_with_the_same_id() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    let first = gate.acquire_writer("session-self", "session")?;
    let participant = first.participant().clone();
    drop(first);
    let replacement = gate.acquire_writer("session-self", "session")?;

    let error = gate.acquire_owner("cave", Some(participant)).unwrap_err();
    assert!(error
        .downcast_ref::<GateError>()
        .is_some_and(|error| matches!(error, GateError::ParticipantInvalid)));
    drop(replacement);
    Ok(())
}

#[test]
fn owner_without_participant_remains_backward_compatible() -> Result<()> {
    let owner: Owner = serde_json::from_value(serde_json::json!({
        "owner_id": "legacy",
        "generation": "generation",
        "expires_at": 9999999999_u64,
        "phase": "held"
    }))?;
    assert_eq!(owner.participant, None);
    Ok(())
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p coven-cli maintenance_gate::tests::owner_ --locked
```

Expected: compilation fails because `WriterParticipant`,
`WriterLease::participant`, `Owner::participant`, and the new
`acquire_owner` argument do not exist.

- [ ] **Step 3: Add the participant type and owner compatibility field**

Add near `WriterIntent`:

```rust
pub const MAINTENANCE_PARTICIPANT_ENV: &str = "COVEN_MAINTENANCE_PARTICIPANT";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriterParticipant {
    pub id: String,
    pub generation: String,
}

impl WriterParticipant {
    pub fn encode(&self) -> Result<String> {
        serde_json::to_string(self).context("failed to encode maintenance participant")
    }

    pub fn decode(value: &str) -> Result<Self> {
        let participant: Self =
            serde_json::from_str(value).context("invalid maintenance participant capability")?;
        if participant.id.trim().is_empty() || participant.generation.trim().is_empty() {
            anyhow::bail!("invalid maintenance participant capability");
        }
        Ok(participant)
    }
}
```

Extend `Owner`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub participant: Option<WriterParticipant>,
```

Add `GateError::ParticipantInvalid` and render it as:

```rust
Self::ParticipantInvalid => {
    write!(f, "maintenance participant is stale, missing, or mismatched")
}
```

- [ ] **Step 4: Expose the exact writer capability**

Store the full participant on `WriterLease`:

```rust
pub struct WriterLease {
    gate: MaintenanceGate,
    path: PathBuf,
    participant: WriterParticipant,
    stopper: Arc<(Mutex<bool>, Condvar)>,
    renewer: Option<thread::JoinHandle<()>>,
}
```

Construct it from the acquired intent and add:

```rust
pub fn participant(&self) -> &WriterParticipant {
    &self.participant
}

pub fn participant_capability(&self) -> Result<String> {
    self.participant.encode()
}
```

Update `Drop` and the renewer thread to use
`self.participant.generation` instead of the removed standalone field.

- [ ] **Step 5: Validate participation atomically during owner acquisition**

Change the signature:

```rust
pub fn acquire_owner(
    &self,
    owner_id: impl Into<String>,
    participant: Option<WriterParticipant>,
) -> Result<OwnerLease>
```

Inside the existing metadata lock, before writing the owner record, validate:

```rust
if let Some(participant) = participant.as_ref() {
    let path = self.writers_dir().join(writer_file_name(&participant.id));
    let data = fs::read(&path).map_err(|_| GateError::ParticipantInvalid)?;
    let writer: WriterIntent =
        serde_json::from_slice(&data).map_err(|_| GateError::ParticipantInvalid)?;
    if writer.expires_at <= unix_now()
        || writer.id != participant.id
        || writer.generation != participant.generation
    {
        return Err(GateError::ParticipantInvalid.into());
    }
}
```

Persist `participant` on the new `Owner`. Update every non-participant caller
to pass `None`.

- [ ] **Step 6: Return only blocking writers while preserving participant state**

Add:

```rust
fn blocking_writers(owner: Option<&Owner>, writers: Vec<WriterIntent>) -> Vec<WriterIntent> {
    let Some(participant) = owner.and_then(|owner| owner.participant.as_ref()) else {
        return writers;
    };
    writers
        .into_iter()
        .filter(|writer| {
            writer.id != participant.id || writer.generation != participant.generation
        })
        .collect()
}
```

Use it in `status` and `OwnerLease::refresh_phase`. Change `heartbeat_owner` to read the persisted owner first and reject a
different owner id or generation before constructing `OwnerLease`. This
preserves the stored participant:

```rust
pub fn heartbeat_owner(&self, owner_id: &str, generation: &str) -> Result<GateStatus> {
    let owner = self.read_owner()?.ok_or(GateError::OwnerChanged)?;
    if owner.owner_id != owner_id || owner.generation != generation {
        return Err(GateError::OwnerChanged.into());
    }
    OwnerLease {
        gate: self.clone(),
        owner,
    }
    .refresh_phase()
}
```

- [ ] **Step 7: Run the maintenance-gate tests**

Run:

```bash
cargo test -p coven-cli maintenance_gate::tests --locked
```

Expected: all maintenance-gate tests pass.

- [ ] **Step 8: Commit the protocol core**

```bash
git add crates/coven-cli/src/maintenance_gate.rs
git commit -s -m "feat: add maintenance participant capability"
```

### Task 2: Consume the capability in maintenance CLI commands

**Files:**
- Modify: `crates/coven-cli/src/main.rs`
- Test: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Add a pure capability parser test**

Add a helper:

```rust
fn maintenance_participant_from_value(
    value: Option<&str>,
) -> Result<Option<maintenance_gate::WriterParticipant>> {
    value
        .map(maintenance_gate::WriterParticipant::decode)
        .transpose()
}
```

Add tests:

```rust
#[test]
fn maintenance_participant_env_is_optional_and_strict() {
    assert_eq!(maintenance_participant_from_value(None).unwrap(), None);
    assert!(maintenance_participant_from_value(Some("not-json")).is_err());
    assert!(maintenance_participant_from_value(Some(r#"{"id":"","generation":"g"}"#)).is_err());
    assert_eq!(
        maintenance_participant_from_value(Some(r#"{"id":"session-1","generation":"g"}"#))
            .unwrap()
            .unwrap(),
        maintenance_gate::WriterParticipant {
            id: "session-1".to_string(),
            generation: "g".to_string(),
        },
    );
}
```

- [ ] **Step 2: Run the parser test and verify it passes only after the helper exists**

Run:

```bash
cargo test -p coven-cli maintenance_participant_env_is_optional_and_strict --locked
```

Expected: PASS.

- [ ] **Step 3: Wire acquisition to the inherited environment**

In `run_maintenance_command`, change the acquire branch to:

```rust
let participant = maintenance_participant_from_value(
    std::env::var(maintenance_gate::MAINTENANCE_PARTICIPANT_ENV)
        .ok()
        .as_deref(),
)?;
let mut lease = gate.acquire_owner(owner, participant)?;
```

Update every other `acquire_owner` call in tests and production to pass `None`.

- [ ] **Step 4: Run CLI and protocol tests**

Run:

```bash
cargo test -p coven-cli maintenance_ --locked
```

Expected: all matching tests pass.

- [ ] **Step 5: Commit CLI acquisition**

```bash
git add crates/coven-cli/src/main.rs
git commit -s -m "feat: accept inherited maintenance participation"
```

### Task 3: Add a narrow harness environment API

**Files:**
- Modify: `crates/coven-cli/src/pty_runner.rs`

- [ ] **Step 1: Add a failing environment override test**

Inside `pty_runner.rs` tests:

```rust
#[test]
fn harness_command_can_set_a_private_runtime_environment_value() {
    let mut command = HarnessCommand::fixture("echo", Vec::new(), PathBuf::from("/tmp"));
    command.set_environment_override("COVEN_TEST_CAPABILITY", Some("opaque-value"));
    assert_eq!(
        command.env_overrides,
        vec![(
            "COVEN_TEST_CAPABILITY".to_string(),
            Some("opaque-value".to_string())
        )]
    );
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo test -p coven-cli harness_command_can_set_a_private_runtime_environment_value --locked
```

Expected: compilation fails because the method is missing.

- [ ] **Step 3: Implement the helper**

Add to `impl HarnessCommand`:

```rust
pub(crate) fn set_environment_override(
    &mut self,
    name: impl Into<String>,
    value: Option<impl Into<String>>,
) {
    self.env_overrides
        .push((name.into(), value.map(Into::into)));
}
```

- [ ] **Step 4: Run the focused test**

Run:

```bash
cargo test -p coven-cli harness_command_can_set_a_private_runtime_environment_value --locked
```

Expected: PASS.

- [ ] **Step 5: Commit the command API**

```bash
git add crates/coven-cli/src/pty_runner.rs
git commit -s -m "refactor: support private harness environment overrides"
```

### Task 4: Propagate participation through direct and patch sessions

**Files:**
- Modify: `crates/coven-cli/src/main.rs`
- Test: `crates/coven-cli/src/main.rs`

- [ ] **Step 1: Return the lease capability from the session writer helper**

Keep `acquire_session_writer` returning `Option<WriterLease>`, but add:

```rust
fn apply_maintenance_participant(
    command: &mut pty_runner::HarnessCommand,
    writer: Option<&maintenance_gate::WriterLease>,
) -> Result<()> {
    if let Some(writer) = writer {
        command.set_environment_override(
            maintenance_gate::MAINTENANCE_PARTICIPANT_ENV,
            Some(writer.participant_capability()?),
        );
    }
    Ok(())
}
```

- [ ] **Step 2: Add a unit test for propagation**

```rust
#[test]
fn maintenance_participant_is_added_only_when_a_writer_exists() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = maintenance_gate::MaintenanceGate::at_for_test(temp.path().to_path_buf());
    let writer = gate.acquire_writer("session-1", "session")?;
    let mut command =
        pty_runner::HarnessCommand::fixture("echo", Vec::new(), temp.path().to_path_buf());

    apply_maintenance_participant(&mut command, Some(&writer))?;
    assert!(command.environment_override_for_test(
        maintenance_gate::MAINTENANCE_PARTICIPANT_ENV
    ).is_some());
    Ok(())
}
```

Expose `MaintenanceGate::at_for_test` and
`HarnessCommand::environment_override_for_test` under `#[cfg(test)]`.

- [ ] **Step 3: Apply the capability to direct `coven run` commands**

Rename:

```rust
let _maintenance_writer = acquire_session_writer(&project_root, "session")?;
```

to:

```rust
let maintenance_writer = acquire_session_writer(&project_root, "session")?;
```

After the harness command is successfully built and before any spawn path:

```rust
let mut command = command?;
apply_maintenance_participant(&mut command, maintenance_writer.as_ref())?;
```

Keep `maintenance_writer` alive through the complete harness run.

- [ ] **Step 4: Apply the capability to patch sessions**

Rename the patch writer binding to `maintenance_writer`. After building the
patch harness command and before spawning it:

```rust
apply_maintenance_participant(&mut command, maintenance_writer.as_ref())?;
```

Keep the lease in scope until the patch harness exits.

- [ ] **Step 5: Run direct and patch focused tests**

Run:

```bash
cargo test -p coven-cli maintenance_participant --locked
cargo test -p coven-cli patch --locked
```

Expected: all matching tests pass.

- [ ] **Step 6: Commit direct launch propagation**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/maintenance_gate.rs crates/coven-cli/src/pty_runner.rs
git commit -s -m "feat: propagate maintenance participation to harnesses"
```

### Task 5: Propagate participation through daemon and adopted sessions

**Files:**
- Modify: `crates/coven-cli/src/daemon.rs`
- Test: `crates/coven-cli/src/daemon.rs`

- [ ] **Step 1: Add a command-preparation helper**

In `LiveSessionRuntime`:

```rust
fn apply_writer_participant(
    command: &mut pty_runner::HarnessCommand,
    writer: Option<&crate::maintenance_gate::WriterLease>,
) -> Result<()> {
    if let Some(writer) = writer {
        command.set_environment_override(
            crate::maintenance_gate::MAINTENANCE_PARTICIPANT_ENV,
            Some(writer.participant_capability()?),
        );
    }
    Ok(())
}
```

- [ ] **Step 2: Add a daemon unit test**

Construct a temporary gate, writer, and fixture `HarnessCommand`, call the
helper, and assert the exact environment name and encoded value are present:

```rust
#[test]
fn daemon_harness_receives_maintenance_participant_without_debug_disclosure() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = crate::maintenance_gate::MaintenanceGate::at_for_test(
        temp.path().to_path_buf(),
    );
    let writer = gate.acquire_writer("daemon-session-1", "session")?;
    let capability = writer.participant_capability()?;
    let mut command =
        pty_runner::HarnessCommand::fixture("echo", Vec::new(), temp.path().to_path_buf());

    LiveSessionRuntime::apply_writer_participant(&mut command, Some(&writer))?;
    assert_eq!(
        command.environment_override_for_test(
            crate::maintenance_gate::MAINTENANCE_PARTICIPANT_ENV
        ),
        Some(capability.as_str()),
    );
    Ok(())
}
```

- [ ] **Step 3: Apply the capability before daemon spawn**

Make the command mutable in `launch_session_inner`:

```rust
let mut command = if ... { ... } else { ... };
Self::apply_writer_participant(&mut command, writer.as_ref())?;
self.launch_prepared_session(launch, writer, command, ownership_established)
```

Because both ordinary and adopted sessions flow through
`launch_session_inner`, this covers both paths without duplicating logic.

- [ ] **Step 4: Run daemon and API focused tests**

Run:

```bash
cargo test -p coven-cli daemon::tests --locked
cargo test -p coven-cli api::tests --locked
```

Expected: all daemon/API tests pass.

- [ ] **Step 5: Commit daemon propagation**

```bash
git add crates/coven-cli/src/daemon.rs crates/coven-cli/src/pty_runner.rs
git commit -s -m "feat: propagate maintenance participation in daemon sessions"
```

### Task 6: Prove the end-to-end CLI behavior

**Files:**
- Modify: `crates/coven-cli/tests/smoke.rs`

- [ ] **Step 1: Add a fake harness integration test**

Add a Unix smoke test that creates a fake `codex` executable. The fake harness
invokes the same built Coven binary through an explicit test-only environment
path:

```rust
#[test]
fn session_harness_can_acquire_maintenance_against_its_own_writer() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let coven_home = temp.path().join("coven-home");
    let repo = temp.path().join("repo");
    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&repo)?;
    fs::create_dir_all(&fake_bin)?;
    init_git_repo(&repo)?;

    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
set -eu
test -n "${COVEN_MAINTENANCE_PARTICIPANT:-}"
"${COVEN_TEST_COVEN_BIN}" maintenance acquire in-session-owner --wait-ms 1000 --json
"#,
    )?;
    let mut permissions = fs::metadata(&fake_codex)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions)?;

    let path = std::env::join_paths(
        std::iter::once(fake_bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )?;
    let coven = coven_bin();
    let output = run_coven_in(
        &coven,
        &coven_home,
        &path,
        &repo,
        &[("COVEN_TEST_COVEN_BIN", coven.to_str().context("non-UTF-8 coven path")?)],
        &["run", "codex", "verify maintenance participation"],
    )?;
    assert_success("in-session maintenance acquire", &output);
    let status_line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.contains(r#""owner_id":"in-session-owner""#))
        .context("missing maintenance status JSON")?;
    let status: serde_json::Value = serde_json::from_str(status_line)?;
    assert_eq!(status["owner"]["phase"], "held");
    assert_eq!(status["writers"], serde_json::json!([]));
    Ok(())
}
```

- [ ] **Step 2: Add the non-participant blocker assertion**

Extend the fixture so a separately acquired writer remains live. Verify the
in-session acquire reports `draining` and returns that writer in `writers`,
then release the blocker and verify heartbeat reaches `held`.

- [ ] **Step 3: Run the integration test**

Run:

```bash
cargo test -p coven-cli --test smoke session_harness_can_acquire_maintenance_against_its_own_writer --locked -- --nocapture
```

Expected: PASS with the owner in `held` for the self-only case and `draining`
while an unrelated writer remains.

- [ ] **Step 4: Commit the integration proof**

```bash
git add crates/coven-cli/tests/smoke.rs
git commit -s -m "test: prove in-session maintenance participation"
```

### Task 7: Document and validate the release-ready change

**Files:**
- Modify: `docs/reference/cli-maintenance.md`
- Verify: repository-wide

- [ ] **Step 1: Document participant behavior**

Add after the acquisition example:

```markdown
When a maintenance client runs inside a Coven-managed harness, Coven supplies a
generation-bound participant capability automatically. The owner excludes only
that exact supervisor-owned writer from its blocker set; every other existing
writer must still drain. The capability is internal, is not a user-facing
credential, and must not be copied into logs or persisted session metadata.
```

- [ ] **Step 2: Run formatting**

Run:

```bash
cargo fmt --check
```

Expected: exit 0. If it reports diffs, run `cargo fmt`, inspect the changes, and
rerun the check.

- [ ] **Step 3: Run clippy**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: exit 0 with no warnings.

- [ ] **Step 4: Run the complete Rust suite**

Run:

```bash
cargo test --workspace --locked
```

Expected: all workspace tests pass.

- [ ] **Step 5: Run repository safety checks**

Run:

```bash
python scripts/check-secrets.py
git add crates/coven-cli/src/maintenance_gate.rs \
  crates/coven-cli/src/main.rs \
  crates/coven-cli/src/pty_runner.rs \
  crates/coven-cli/src/daemon.rs \
  crates/coven-cli/tests/smoke.rs \
  docs/reference/cli-maintenance.md \
  docs/superpowers/specs/2026-08-23-maintenance-participant-design.md \
  docs/superpowers/plans/2026-08-23-maintenance-participant.md
python3 scripts/check-coven-privacy.py --staged
```

Expected: both checks pass with no secret or privacy findings.

- [ ] **Step 6: Commit documentation and validation state**

```bash
git commit -s -m "docs: document in-session maintenance participation"
```

- [ ] **Step 7: Push and open the PR**

```bash
git push -u origin fix/795-maintenance-participant
gh pr create \
  --title "feat: allow in-session maintenance participation" \
  --body-file .github/PULL_REQUEST_TEMPLATE.md
```

Fill the PR readiness packet with:

- issue #795;
- downstream `cave-cgk9v`;
- exact local validation commands and results;
- compatibility statement for old owner JSON and existing Cave clients;
- explicit statement that unrelated live writers still block.

- [ ] **Step 8: Resolve review and merge exact head**

Require every required check to pass on the current `headRefOid`, resolve every
actionable review thread, and squash-merge without administrative bypass.

- [ ] **Step 9: Reconcile both trackers**

After a released Coven version contains the merge:

1. update Cave's `COVEN_MAINTENANCE_MINIMUM_VERSION`;
2. add a Cave integration contract proving managed worktree creation succeeds
   from a Coven-launched harness;
3. remove the last-resort guidance that describes the catch-22;
4. close `cave-cgk9v` with the Coven issue, PR, release, Cave adoption PR, and
   exact-head CI evidence.
