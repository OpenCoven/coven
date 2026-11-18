// Real-daemon Ward acknowledgment durability under a synthetic power loss
// (#1640). The daemon registers its xSync-observing VFS from an owner-private
// fixture, every request travels the ordinary edits endpoint over owner IPC,
// and a "power cut" freezes the last synced database/WAL images, terminates
// the daemon and restores only those images before restarting it. Process
// termination alone would keep every written WAL frame, so it is never used
// as the loss model by itself.

use super::*;

const BEFORE: &str = "before";
const AFTER: &str = "after";

fn seed_ward_power_loss(coven_home: &Path, capability: &str) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let root = coven_home.join("test-fixtures");
    let directory = root.join("ward-power-loss");
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        for path in [&root, &directory] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    write_private_fixture_file(&directory.join("enabled"), "ward_power_loss_v1\n")?;
    write_private_fixture_file(&directory.join("capability"), capability)?;
    Ok(())
}

fn run_power_loss_journey(
    name: &str,
    prepare: impl FnOnce(&Path, &Path) -> Result<()>,
    journey: impl FnOnce(&mut ThreadsFixture, &str) -> Result<()>,
) -> Result<()> {
    let clock = Uuid::new_v4().to_string();
    let capability = Uuid::new_v4().to_string();
    run_fixture_journey(
        name,
        |evidence| {
            ThreadsFixture::start_with_setup(evidence, |home, workspace| {
                seed_clock(home, &clock)?;
                seed_ward_power_loss(home, &capability)?;
                prepare(home, workspace)
            })
        },
        |fixture| journey(fixture, &capability),
    )
}

fn power_loss_control(fixture: &mut ThreadsFixture, capability: &str, action: &str) -> Result<Value> {
    let response = fixture.request(
        "POST",
        "/api/v1/internal/ward/power-loss",
        Some(&json!({"capability": capability, "action": action})),
    )?;
    anyhow::ensure!(
        response.status == 200 && response.body["ok"] == true,
        "power-loss control {action} failed: {response:?}"
    );
    Ok(response.body["report"].clone())
}

/// Start observation from a quiescent store. The fixture refuses a baseline
/// with a typed 503 while the daemon's startup store is still open or another
/// connection holds the checkpoint lock, so wait for it to accept one.
fn power_loss_baseline(fixture: &mut ThreadsFixture, capability: &str) -> Result<Value> {
    // Hang guard only, far above what load can produce: the startup store
    // closes within tens of milliseconds of readiness.
    const HANG_GUARD: Duration = Duration::from_secs(60);
    let started = Instant::now();
    let mut refusals = 0_u32;
    loop {
        let response = fixture.request(
            "POST",
            "/api/v1/internal/ward/power-loss",
            Some(&json!({"capability": capability, "action": "baseline"})),
        )?;
        if response.status == 200 && response.body["ok"] == true {
            return Ok(response.body["report"].clone());
        }
        anyhow::ensure!(
            response.status == 503
                && response.body["error"]["code"] == "ward_power_loss_fixture_not_quiescent",
            "power-loss control baseline failed: {response:?}"
        );
        refusals += 1;
        anyhow::ensure!(
            started.elapsed() < HANG_GUARD,
            "the store was still not quiescent after {:?} and {refusals} refusals: {response:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Freeze the synced images, terminate the daemon, restore only those images
/// and start a fresh daemon on them.
fn power_cut(fixture: &mut ThreadsFixture, capability: &str) -> Result<()> {
    let report = power_loss_control(fixture, capability, "freeze")?;
    anyhow::ensure!(report["databaseBytes"].as_u64().is_some_and(|bytes| bytes > 0));
    fixture.crash_daemon()?;
    let fixture_dir = fixture.coven_home.join("test-fixtures/ward-power-loss");
    let database = fixture.coven_home.join("coven.sqlite3");
    let wal = fixture.coven_home.join("coven.sqlite3-wal");
    fs::copy(fixture_dir.join("frozen-coven.sqlite3"), &database)?;
    match fs::read(fixture_dir.join("frozen-coven.sqlite3-wal")) {
        Ok(bytes) => fs::write(&wal, bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = fs::remove_file(&wal);
        }
        Err(error) => return Err(error.into()),
    }
    let _ = fs::remove_file(fixture.coven_home.join("coven.sqlite3-shm"));
    fixture.start_daemon()
}

/// A deliberately unsynced NORMAL write from a separate connection. It reaches
/// the live WAL but no synced image, so a power cut must discard it.
fn write_unsynced_tail(fixture: &ThreadsFixture, key: &str) -> Result<()> {
    let conn = fixture.store()?;
    conn.execute_batch("PRAGMA synchronous = NORMAL")?;
    conn.execute(
        "INSERT INTO store_meta(key, value) VALUES (?1, 'lost')",
        [key],
    )?;
    let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    anyhow::ensure!(synchronous == 1, "tail write was not NORMAL");
    Ok(())
}

fn tail_rows(fixture: &ThreadsFixture, key: &str) -> Result<i64> {
    Ok(fixture.store()?.query_row(
        "SELECT COUNT(*) FROM store_meta WHERE key = ?1",
        [key],
        |row| row.get(0),
    )?)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}

/// Count `apply_audit` rows that exactly describe one direct write.
fn exact_direct_audits(fixture: &ThreadsFixture, target: &str, before: &str, after: &str) -> Result<i64> {
    Ok(fixture.store()?.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'
         AND familiar_id = ?1 AND proposal_id IS NULL
         AND files_touched = ?2 AND decision = 'applied' AND tier = 'tier_2'
         AND lower(hex(diff_hash)) = ?3
         AND json_extract(detail, '$.bytes_written') = ?4
         AND json_extract(detail, '$.prev_sha256') = ?5",
        rusqlite::params![
            FAMILIAR_ID,
            serde_json::to_string(&[target])?,
            sha256_hex(after.as_bytes()),
            i64::try_from(after.len())?,
            sha256_hex(before.as_bytes()),
        ],
        |row| row.get(0),
    )?)
}

fn direct_reservations(fixture: &ThreadsFixture) -> Result<i64> {
    Ok(fixture.store()?.query_row(
        "SELECT COUNT(*) FROM coven_ward_audit_reservations WHERE purpose = 'direct-ward-apply'",
        [],
        |row| row.get(0),
    )?)
}

fn post_direct(fixture: &mut ThreadsFixture, target: &str, contents: &str) -> Result<HttpResponse> {
    fixture.request(
        "POST",
        "/api/v1/familiars/sage/edits",
        Some(&json!({
            "edits": [{"target": target, "contents": contents}],
            "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
        })),
    )
}

fn seed_notes(workspace: &Path, names: &[&str]) -> Result<()> {
    fs::create_dir_all(workspace.join("notes"))?;
    for name in names {
        fs::write(workspace.join("notes").join(name), BEFORE)?;
    }
    Ok(())
}

fn seed_reviewed(workspace: &Path, names: &[&str]) -> Result<()> {
    fs::create_dir_all(workspace.join("reviewed"))?;
    for name in names {
        fs::write(workspace.join("reviewed").join(name), BEFORE)?;
    }
    Ok(())
}

fn pending_proposals(fixture: &ThreadsFixture) -> Result<Vec<PathBuf>> {
    let directory = fixture.coven_home.join("pending");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "json"));
    paths.sort();
    Ok(paths)
}

fn submission_receipts(fixture: &ThreadsFixture, id: &str) -> Result<i64> {
    Ok(fixture.store()?.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_submitted'",
        [id],
        |row| row.get(0),
    )?)
}

#[test]
fn direct_acknowledged_write_survives_power_loss_over_real_daemon() -> Result<()> {
    run_power_loss_journey(
        "ward-power-loss-direct-acknowledged",
        |_, workspace| seed_notes(workspace, &["today.md", "after-recovery.md"]),
        |fixture, capability| {
            let refused = fixture.request(
                "POST",
                "/api/v1/internal/ward/power-loss",
                Some(&json!({"capability": "not-the-capability", "action": "freeze"})),
            )?;
            anyhow::ensure!(refused.status == 403, "wrong capability was accepted: {refused:?}");
            power_loss_baseline(fixture, capability)?;

            let response = post_direct(fixture, "notes/today.md", AFTER)?;
            anyhow::ensure!(
                response.status == 200 && response.body["disposition"] == "applied",
                "direct write was not acknowledged: {response:?}"
            );
            write_unsynced_tail(fixture, "power-loss-tail")?;
            anyhow::ensure!(tail_rows(fixture, "power-loss-tail")? == 1);

            power_cut(fixture, capability)?;

            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("notes/today.md"))? == AFTER,
                "acknowledged target bytes changed across the power cut"
            );
            anyhow::ensure!(
                tail_rows(fixture, "power-loss-tail")? == 0,
                "the unsynced NORMAL tail survived, so the loss model did not cut"
            );
            // Reservation acquisition and release both commit with NORMAL.
            // The evidence's FULL commit syncs the acquisition frames, but the
            // later release stays unsynced, so the cut resurrects the
            // reservation as held capacity. That is conservative, not
            // authority, and is recorded as a residual on #1640.
            let resurrected = direct_reservations(fixture)?;
            anyhow::ensure!(
                resurrected == 1,
                "expected the unsynced reservation release to be lost: {resurrected}"
            );
            let audits = exact_direct_audits(fixture, "notes/today.md", BEFORE, AFTER)?;
            anyhow::ensure!(
                audits == 1,
                "acknowledged direct write lost its exact audit evidence: {audits}"
            );
            let recovered = post_direct(fixture, "notes/after-recovery.md", AFTER)?;
            anyhow::ensure!(
                recovered.status == 200,
                "daemon did not accept a direct write after recovery: {recovered:?}"
            );
            anyhow::ensure!(
                exact_direct_audits(fixture, "notes/after-recovery.md", BEFORE, AFTER)? == 1
            );
            Ok(())
        },
    )
}

#[test]
fn legacy_acknowledged_submission_survives_power_loss_over_real_daemon() -> Result<()> {
    run_power_loss_journey(
        "ward-power-loss-legacy-acknowledged",
        |_, workspace| seed_reviewed(workspace, &["skill.md"]),
        |fixture, capability| {
            power_loss_baseline(fixture, capability)?;
            let staged = post_direct(fixture, "reviewed/skill.md", AFTER)?;
            anyhow::ensure!(
                staged.status == 202 && staged.body["disposition"] == "staged",
                "legacy submission was not acknowledged: {staged:?}"
            );
            anyhow::ensure!(staged.body.get("scheduledProposal").is_none());
            let id = staged.body["proposalId"]
                .as_str()
                .context("legacy proposal id")?
                .to_owned();
            let pending = pending_proposals(fixture)?;
            anyhow::ensure!(pending.len() == 1, "expected one pending file: {pending:?}");
            let bytes = fs::read(&pending[0])?;
            write_unsynced_tail(fixture, "power-loss-tail")?;

            power_cut(fixture, capability)?;

            anyhow::ensure!(tail_rows(fixture, "power-loss-tail")? == 0);
            anyhow::ensure!(
                fs::read(&pending[0])? == bytes,
                "acknowledged pending bytes changed across the power cut"
            );
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("reviewed/skill.md"))? == BEFORE,
                "a staged legacy submission changed its target"
            );
            let receipts = submission_receipts(fixture, &id)?;
            anyhow::ensure!(
                receipts == 1,
                "acknowledged legacy submission lost its receipt: {receipts}"
            );
            Ok(())
        },
    )
}

#[test]
fn direct_sync_failures_never_acknowledge_and_recover_over_real_daemon() -> Result<()> {
    run_power_loss_journey(
        "ward-power-loss-direct-failures",
        |_, workspace| seed_notes(workspace, &["sync.md", "barrier.md", "after-recovery.md"]),
        |fixture, capability| {
            let mut retained = 0;
            for (target, action, committed) in [
                ("notes/sync.md", "fail-next-store-sync", false),
                ("notes/barrier.md", "fail-next-store-directory-barrier", true),
            ] {
                power_loss_baseline(fixture, capability)?;
                // Restart the WAL from this process so the armed fault reaches
                // the evidence commit rather than a fresh WAL header write.
                write_unsynced_tail(fixture, &format!("prime-{action}"))?;
                power_loss_control(fixture, capability, action)?;
                let response = post_direct(fixture, target, AFTER)?;
                anyhow::ensure!(
                    response.status == 500
                        && response.body["error"]["code"] == "audit_persist_failed"
                        && response.body["error"]["details"]["writeApplied"] == true
                        && response.body["error"]["details"]["retrySafe"] == false,
                    "{action}: failed evidence was not a typed 500: {response:?}"
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(target))? == AFTER,
                    "{action}: the applied target bytes were not reported truthfully"
                );
                retained += 1;
                anyhow::ensure!(
                    direct_reservations(fixture)? == retained,
                    "{action}: the failed write's audit reservation was not retained"
                );
                let live = exact_direct_audits(fixture, target, BEFORE, AFTER)?;
                if committed {
                    // The barrier failed after COMMIT: never read the error as rollback.
                    anyhow::ensure!(live == 1, "{action}: committed evidence missing live: {live}");
                }

                power_cut(fixture, capability)?;

                let restored = exact_direct_audits(fixture, target, BEFORE, AFTER)?;
                anyhow::ensure!(
                    restored == i64::from(committed),
                    "{action}: restored audit evidence {restored} does not match its synced state"
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(target))? == AFTER,
                    "{action}: installed target bytes changed across the power cut"
                );
                // The reservation was acquired with NORMAL. A failed SQLite sync
                // leaves it unsynced, so the cut loses it, inside the documented
                // pre-audit window. A failed directory barrier follows a
                // successful WAL sync, which made it durable.
                let expected = retained - 1 + i64::from(committed);
                retained = direct_reservations(fixture)?;
                anyhow::ensure!(
                    retained == expected,
                    "{action}: restored reservations {retained}, expected {expected}"
                );
            }
            let recovered = post_direct(fixture, "notes/after-recovery.md", AFTER)?;
            anyhow::ensure!(
                recovered.status == 200,
                "daemon did not recover after evidence failures: {recovered:?}"
            );
            anyhow::ensure!(
                exact_direct_audits(fixture, "notes/after-recovery.md", BEFORE, AFTER)? == 1
            );
            Ok(())
        },
    )
}

#[test]
fn legacy_sync_failures_never_acknowledge_and_recover_over_real_daemon() -> Result<()> {
    run_power_loss_journey(
        "ward-power-loss-legacy-failures",
        |_, workspace| seed_reviewed(workspace, &["sync.md", "barrier.md"]),
        |fixture, capability| {
            for (target, action, committed) in [
                ("reviewed/sync.md", "fail-next-store-sync", false),
                ("reviewed/barrier.md", "fail-next-store-directory-barrier", true),
            ] {
                power_loss_baseline(fixture, capability)?;
                write_unsynced_tail(fixture, &format!("prime-{action}"))?;
                let before = pending_proposals(fixture)?;
                power_loss_control(fixture, capability, action)?;
                let response = post_direct(fixture, target, AFTER)?;
                anyhow::ensure!(
                    response.status == 500,
                    "{action}: legacy submission was acknowledged without durable receipt: {response:?}"
                );
                let published: Vec<_> = pending_proposals(fixture)?
                    .into_iter()
                    .filter(|path| !before.contains(path))
                    .collect();
                anyhow::ensure!(
                    published.len() == 1,
                    "{action}: the published pending file was not retained: {published:?}"
                );
                let bytes = fs::read(&published[0])?;
                let document: Value = serde_json::from_slice(&bytes)?;
                let id = document["id"].as_str().context("legacy id")?.to_owned();
                let live = submission_receipts(fixture, &id)?;
                if committed {
                    anyhow::ensure!(live == 1, "{action}: committed receipt missing live: {live}");
                }

                power_cut(fixture, capability)?;

                anyhow::ensure!(
                    fs::read(&published[0])? == bytes,
                    "{action}: pending bytes changed across the power cut"
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(target))? == BEFORE,
                    "{action}: an unacknowledged legacy submission changed its target"
                );
                let restored = submission_receipts(fixture, &id)?;
                anyhow::ensure!(
                    restored == i64::from(committed),
                    "{action}: restored receipt {restored} does not match its synced state"
                );
                if !committed {
                    // Existing fail-closed handling: a receiptless pending file
                    // is quarantined, never executed.
                    let response = fixture.request(
                        "POST",
                        &format!("/api/v1/threads/proposals/{id}/approve"),
                        Some(&json!({})),
                    )?;
                    anyhow::ensure!(
                        response.status == 409
                            && response.body["why"] == "proposal-submission-receipt-invalid",
                        "{action}: receiptless proposal was not refused: {response:?}"
                    );
                    anyhow::ensure!(
                        fs::read_to_string(fixture.workspace.join(target))? == BEFORE
                    );
                }
            }
            Ok(())
        },
    )
}
