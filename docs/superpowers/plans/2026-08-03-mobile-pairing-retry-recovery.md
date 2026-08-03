# Mobile Pairing Retry Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make completed mobile pairing confirmations idempotent for the rest of the existing pairing lifetime without duplicate device registration or weaker phrase checks.

**Architecture:** Keep the retry window inside `PairingManager` by caching the first completed `MobilePairedDevice` on the existing pending pairing entry until `expires_at`. Extend `PairingProgress` with replay metadata so `gateway.rs` can return stable HTTP responses and avoid duplicate host-side `PairingCompleted` audit lines while leaving `DeviceRegistry` unchanged.

**Tech Stack:** Rust, `chrono`, `uuid`, existing mobile gateway/pairing unit tests, Cargo, repository privacy and secret checks.

---

## File structure

- Modify: `crates/coven-cli/src/mobile_memory/pairing.rs:20-30, 54-58, 104-119, 239-295, 400-567`
  - Add completed pairing cache, replay-aware completion result, and focused unit tests.
- Modify: `crates/coven-cli/src/mobile_memory/gateway.rs:626-660, 858-885, 1094-1130`
  - Preserve first-completion HTTP/audit semantics while allowing replayed confirmations to return the same device cleanly.
- No change expected: `crates/coven-cli/src/mobile_memory/registry.rs`
  - Registry uniqueness rules stay authoritative, but the new flow should never hit them on replay.

### Task 1: Lock down the pairing-manager retry contract with failing tests

**Files:**
- Modify: `crates/coven-cli/src/mobile_memory/pairing.rs:400-567`
- Test: `crates/coven-cli/src/mobile_memory/pairing.rs`

- [ ] **Step 1: Add replay-oriented helper assertions and failing tests**

```rust
fn assert_complete(progress: PairingProgress, replayed: bool) -> MobilePairedDevice {
    match progress {
        PairingProgress::Complete {
            device,
            replayed: actual,
        } => {
            assert_eq!(actual, replayed);
            device
        }
        PairingProgress::Pending => panic!("expected pairing completion"),
    }
}

#[test]
fn device_confirmation_retry_reuses_completed_device() {
    let harness = PairingHarness::new();
    let pending = harness.enroll();

    assert_eq!(harness.confirm_device(&pending.phrase), PairingProgress::Pending);
    let device = assert_complete(harness.confirm_host(&pending.phrase), false);
    let replay = assert_complete(harness.confirm_device(&pending.phrase), true);

    assert_eq!(replay, device);
    assert_eq!(harness.devices().len(), 1);
}

#[test]
fn host_confirmation_retry_reuses_completed_device() {
    let harness = PairingHarness::new();
    let pending = harness.enroll();

    assert_eq!(harness.confirm_host(&pending.phrase), PairingProgress::Pending);
    let device = assert_complete(harness.confirm_device(&pending.phrase), false);
    let replay = assert_complete(harness.confirm_host(&pending.phrase), true);

    assert_eq!(replay, device);
    assert_eq!(harness.devices().len(), 1);
}

#[test]
fn completed_pairing_rejects_wrong_phrase_but_keeps_retry_window() {
    let harness = PairingHarness::new();
    let pending = harness.enroll();

    assert_eq!(harness.confirm_host(&pending.phrase), PairingProgress::Pending);
    let device = assert_complete(harness.confirm_device(&pending.phrase), false);
    let mut wrong_phrase = pending.phrase.clone();
    wrong_phrase[0] = "wrong".to_owned();

    assert_eq!(
        harness
            .manager
            .confirm_host(harness.pairing_id, &wrong_phrase, harness.now)
            .unwrap_err(),
        PairingError::PairingPhraseMismatch
    );
    assert_eq!(
        assert_complete(harness.confirm_host(&pending.phrase), true),
        device
    );
    assert_eq!(harness.devices().len(), 1);
}

#[test]
fn completed_pairing_retry_expires_on_original_deadline() {
    let harness = PairingHarness::new();
    let pending = harness.enroll();

    assert_eq!(harness.confirm_host(&pending.phrase), PairingProgress::Pending);
    let device = assert_complete(harness.confirm_device(&pending.phrase), false);
    assert_eq!(
        assert_complete(harness.confirm_host(&pending.phrase), true),
        device
    );

    assert_eq!(
        harness
            .manager
            .confirm_device(
                harness.pairing_id,
                &pending.phrase,
                harness.now + Duration::minutes(6),
            )
            .unwrap_err(),
        PairingError::PairingExpired
    );
    assert_eq!(harness.devices().len(), 1);
}
```

Also update `pairing_requires_host_and_device_confirmation` to assert against
`PairingProgress::Complete { replayed: false, .. }` instead of the old tuple
variant.

- [ ] **Step 2: Run the pairing tests and confirm they fail first**

Run:

```bash
cargo test -p coven-cli mobile_memory::pairing::tests --locked -- --nocapture
```

Expected: FAIL because `PairingProgress::Complete { .. }` does not exist yet and
completed confirmations still remove the pairing entry.

### Task 2: Cache the completed device inside `PairingManager`

**Files:**
- Modify: `crates/coven-cli/src/mobile_memory/pairing.rs:20-30, 54-58, 104-119, 239-295`
- Test: `crates/coven-cli/src/mobile_memory/pairing.rs`

- [ ] **Step 1: Extend the pending state and progress enum**

Update the data model near the top of `pairing.rs` to this shape:

```rust
#[derive(Debug, Clone)]
pub struct PendingPairing {
    pub id: Uuid,
    pub nonce_hash: [u8; 32],
    pub expires_at: DateTime<Utc>,
    pub transcript_hash: Option<[u8; 32]>,
    pub device: Option<PendingDevice>,
    pub host_confirmed: bool,
    pub device_confirmed: bool,
    pub consumed: bool,
    pub completed: Option<MobilePairedDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingProgress {
    Pending,
    Complete {
        device: MobilePairedDevice,
        replayed: bool,
    },
}
```

Initialize `completed: None` in `begin_pairing_with_id`.

- [ ] **Step 2: Replace the confirmation tail with replay-aware logic**

In `PairingManager::confirm`, keep the existing expiry and transcript guards,
then replace the phrase / completion block with:

```rust
let expected = phrase_for_hash(transcript_hash);
if phrase != expected {
    if pairing.completed.is_none() {
        pending.remove(&pairing_id);
    }
    return Err(PairingError::PairingPhraseMismatch);
}
if let Some(device) = &pairing.completed {
    return Ok(PairingProgress::Complete {
        device: device.clone(),
        replayed: true,
    });
}
if host {
    pairing.host_confirmed = true;
} else {
    pairing.device_confirmed = true;
}
if !pairing.host_confirmed || !pairing.device_confirmed {
    return Ok(PairingProgress::Pending);
}
let device = pairing
    .device
    .clone()
    .ok_or(PairingError::PairingConfirmationRequired)?;
let record = DeviceRecord {
    id: Uuid::new_v4(),
    display_name: device.display_name,
    public_key_x963: device.public_key_x963,
    paired_at: now,
    revoked_at: None,
    scopes: vec![DeviceScope::MemoryRead],
};
self.registry
    .register(record.clone())
    .map_err(|_| PairingError::InvalidRequest)?;
let completed = MobilePairedDevice {
    id: record.id,
    display_name: record.display_name,
    paired_at: record.paired_at,
    scopes: vec![MobileDeviceScope::MemoryRead],
};
pairing.device = None;
pairing.completed = Some(completed.clone());
Ok(PairingProgress::Complete {
    device: completed,
    replayed: false,
})
```

Do **not** reintroduce `pending.remove(&pairing_id)` on first completion; the
completed entry must survive until the existing expiry removes it.

- [ ] **Step 3: Re-run the focused pairing suite**

Run:

```bash
cargo test -p coven-cli mobile_memory::pairing::tests --locked -- --nocapture
```

Expected: PASS. The suite should now prove idempotent replays, no second device
registration, preserved mismatch rejection, and expiry-bound retention.

- [ ] **Step 4: Commit the pairing-manager boundary**

```bash
git add crates/coven-cli/src/mobile_memory/pairing.rs
git commit -s -m "fix(mobile): cache completed pairing confirmations"
```

### Task 3: Preserve gateway HTTP and audit semantics on replay

**Files:**
- Modify: `crates/coven-cli/src/mobile_memory/gateway.rs:626-660, 858-885, 1094-1130`
- Test: `crates/coven-cli/src/mobile_memory/gateway.rs`

- [ ] **Step 1: Add failing gateway tests for replay responses and audit dedupe**

Add this helper and tests to the gateway test module:

```rust
fn sample_pairing_request(nonce: [u8; 32]) -> super::super::contract::MobilePairingRequest {
    let signing_key = p256::SecretKey::from_slice(&[1; 32]).unwrap();
    let public_key = signing_key.public_key().to_encoded_point(false);
    super::super::contract::MobilePairingRequest {
        protocol_version: super::super::MOBILE_PROTOCOL_VERSION,
        pairing_nonce: URL_SAFE_NO_PAD.encode(nonce),
        device_name: "Synthetic phone".to_owned(),
        device_public_key: URL_SAFE_NO_PAD.encode(public_key.as_bytes()),
        app_version: "1.0.0".to_owned(),
        supported_protocol: super::super::contract::MobileProtocolRange {
            minimum: 1,
            maximum: 1,
        },
    }
}

#[test]
fn device_confirmation_replay_returns_ok_with_the_same_body() {
    let _guard = TEST_GATEWAY_LOCK.lock().unwrap();
    let Some((temp, config, _)) = test_listener_config() else {
        return;
    };
    let _gateway = start_mobile_gateway_with_config(temp.path(), &config).unwrap();
    let state = active_gateway().unwrap();
    let now = Utc::now();
    let invitation = state.pairing.begin_pairing([7; 32], now + PAIRING_LIFETIME).unwrap();
    let enrolled = state
        .pairing
        .enroll(
            invitation.id,
            invitation.nonce,
            sample_pairing_request(invitation.nonce),
            state.host_fingerprint,
            now,
        )
        .unwrap();
    assert_eq!(
        state
            .pairing
            .confirm_host(invitation.id, &enrolled.phrase, now)
            .unwrap(),
        PairingProgress::Pending
    );

    let path = format!("/api/v1/mobile/pairings/{}/confirm", invitation.id);
    let body = serde_json::to_vec(&super::super::contract::MobilePairingConfirmation {
        phrase: enrolled.phrase.to_vec(),
    })
    .unwrap();

    let first = handle_pairing_confirmation(
        &state,
        &path,
        MobileHttpRequest {
            method: "POST".to_owned(),
            target: path.clone(),
            headers: HashMap::new(),
            body: body.clone(),
        },
    );
    let replay = handle_pairing_confirmation(
        &state,
        &path,
        MobileHttpRequest {
            method: "POST".to_owned(),
            target: path,
            headers: HashMap::new(),
            body,
        },
    );

    assert_eq!(first.status, 201);
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body, first.body);
}

#[test]
fn local_control_replay_does_not_duplicate_pairing_completed_audit() {
    let _guard = TEST_GATEWAY_LOCK.lock().unwrap();
    let Some((temp, config, _)) = test_listener_config() else {
        return;
    };
    let _gateway = start_mobile_gateway_with_config(temp.path(), &config).unwrap();
    let state = active_gateway().unwrap();
    let now = Utc::now();
    let invitation = state.pairing.begin_pairing([9; 32], now + PAIRING_LIFETIME).unwrap();
    let enrolled = state
        .pairing
        .enroll(
            invitation.id,
            invitation.nonce,
            sample_pairing_request(invitation.nonce),
            state.host_fingerprint,
            now,
        )
        .unwrap();
    assert_eq!(
        state
            .pairing
            .confirm_device(invitation.id, &enrolled.phrase, now)
            .unwrap(),
        PairingProgress::Pending
    );

    let path = format!("/api/v1/internal/mobile/pairings/{}/confirm", invitation.id);
    let body = serde_json::json!({ "phrase": enrolled.phrase }).to_string();

    let first = handle_local_control("POST", &path, Some(&body)).unwrap().unwrap();
    let replay = handle_local_control("POST", &path, Some(&body)).unwrap().unwrap();

    assert_eq!(first.status, 200);
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body, first.body);

    let audit = std::fs::read_to_string(temp.path().join("mobile/audit.jsonl")).unwrap();
    assert_eq!(audit.matches("\"event\":\"pairing_completed\"").count(), 1);
}
```

Add any missing test imports for `URL_SAFE_NO_PAD`, `Engine`, and
`ToEncodedPoint` at the top of the test module.

- [ ] **Step 2: Run the gateway tests and confirm the current behavior fails**

Run:

```bash
cargo test -p coven-cli mobile_memory::gateway::tests --locked -- --nocapture
```

Expected: FAIL because the gateway still treats replayed completions as first
completions.

- [ ] **Step 3: Update both confirmation callers to consume `replayed`**

Change the remote confirmation route to:

```rust
match state
    .pairing
    .confirm_device(id, &confirmation.phrase, Utc::now())
{
    Ok(PairingProgress::Pending) => {
        error_response(409, MobileErrorCode::PairingConfirmationRequired)
    }
    Ok(PairingProgress::Complete { device, replayed }) => {
        success_response(if replayed { 200 } else { 201 }, device)
    }
    Err(error) => {
        let _ = append_event(
            &state.coven_home,
            Utc::now(),
            MobileAuditEvent::PairingRejected,
            None,
        );
        pairing_error_response(error)
    }
}
```

Change the internal host confirmation route to:

```rust
match state
    .pairing
    .confirm_host(id, &confirmation.phrase, Utc::now())?
{
    PairingProgress::Complete { device, replayed } => {
        if !replayed {
            append_event(
                &state.coven_home,
                Utc::now(),
                MobileAuditEvent::PairingCompleted,
                Some(device.id),
            )?;
        }
        crate::api::json_response(200, &device)
    }
    PairingProgress::Pending => crate::api::api_error(
        409,
        "pairing_confirmation_required",
        "The device must also confirm the pairing phrase.",
        None,
    ),
}
```

Update any remaining `PairingProgress::Complete` matches in the file to the new
named-field variant.

- [ ] **Step 4: Re-run the focused mobile tests**

Run:

```bash
cargo test -p coven-cli mobile_memory --locked -- --nocapture
```

Expected: PASS. Pairing and gateway tests now cover the full retry contract.

- [ ] **Step 5: Run the repository gates exactly as the spec requires**

Run:

```bash
git add crates/coven-cli/src/mobile_memory/pairing.rs crates/coven-cli/src/mobile_memory/gateway.rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

Expected: PASS. No formatting drift, no lint warnings, full locked tests clean,
no secret findings, and no privacy-guard complaints on the staged patch.

- [ ] **Step 6: Commit the replay-safe implementation**

```bash
git commit -s -m "fix(mobile): make pairing confirmation retries idempotent"
```
