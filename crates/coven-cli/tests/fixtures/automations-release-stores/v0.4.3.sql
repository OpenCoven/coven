/* WARNING: Script requires that SQLITE_DBCONFIG_DEFENSIVE be disabled */
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE ward_audit (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type    TEXT    NOT NULL CHECK (event_type IN (
                      'proposal_submitted','proposal_window_opened',
                      'proposal_approved','proposal_rejected',
                      'proposal_vetoed','ward_updated','memory_entry_admitted',
                      'principal_authorized_write','validation_verdict',
                      'compaction_ledger','apply_audit')),
    proposal_id   TEXT,
    familiar_id   TEXT    NOT NULL,
    ward_version  TEXT,
    ward_hash     BLOB    NOT NULL,
    tier          TEXT,
    decision      TEXT    NOT NULL,
    approver      TEXT,
    diff_hash     BLOB,
    detail        TEXT,             -- event-type-specific JSON; see module docs
    files_touched TEXT    NOT NULL, -- JSON array of surface ids
    channel       TEXT,
    thread_id     TEXT,
    submitted_at  TEXT    NOT NULL, -- RFC 3339
    decided_at    TEXT    NOT NULL, -- RFC 3339
    recorded_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    CHECK (
        event_type != 'proposal_window_opened' OR (
            proposal_id IS NOT NULL
            AND detail IS NOT NULL AND json_valid(detail)
            AND json_type(detail, '$.approval_path_label') IS 'text'
            AND length(trim(json_extract(detail, '$.approval_path_label'))) > 0
            AND json_type(detail, '$.deadline') IS 'text'
            AND julianday(json_extract(detail, '$.deadline')) IS NOT NULL
            AND json_type(detail, '$.earliest_close') IS 'text'
            AND julianday(json_extract(detail, '$.earliest_close')) IS NOT NULL
            AND julianday(json_extract(detail, '$.earliest_close'))
                <= julianday(json_extract(detail, '$.deadline'))
            AND json_type(detail, '$.evidence_replay_hash_hex') IS 'text'
            AND length(json_extract(detail, '$.evidence_replay_hash_hex')) = 64
            AND json_extract(detail, '$.evidence_replay_hash_hex')
                NOT GLOB '*[^0-9A-Fa-f]*'
            AND json_type(detail, '$.affected_regions') IS 'array'
        )
    ),
    CHECK (
        event_type != 'memory_entry_admitted' OR (
            detail IS NOT NULL AND json_valid(detail)
            AND json_type(detail, '$.entry_hash') IS 'array'
            AND json_array_length(detail, '$.entry_hash') = 32
            AND json_type(detail, '$.source_attestation') IS 'text'
            AND length(trim(json_extract(detail, '$.source_attestation'))) > 0
        )
    )
);
CREATE TABLE sessions (
            id TEXT PRIMARY KEY NOT NULL,
            project_root TEXT NOT NULL,
            harness TEXT NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            exit_code INTEGER,
            archived_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            conversation_id TEXT,
            labels TEXT,
            visibility TEXT NOT NULL DEFAULT 'private',
            familiar_id TEXT,
            execution_binding_json TEXT,
            external INTEGER NOT NULL DEFAULT 0,
            transcript_path TEXT
        , transcript_indexed_at TEXT);
INSERT INTO sessions VALUES('session-3bec78bf81fa4606ba82fe9f7a78ced2','/work/project','codex','Nightly review','failed',NULL,NULL,'2026-10-02T01:51:22.601Z','2026-10-02T01:51:22.601Z',NULL,NULL,'private',NULL,NULL,0,NULL,NULL);
INSERT INTO sessions VALUES('session-179a7eeb4ef746dc9eae908eb3da3b25','/work/project','claude','Weekly digest','failed',NULL,NULL,'2026-10-02T01:51:22.834Z','2026-10-02T01:51:22.834Z',NULL,NULL,'private','charm',NULL,0,NULL,NULL);
CREATE TABLE request_adoptions (
            id TEXT PRIMARY KEY NOT NULL,
            adoption_key TEXT,
            contract TEXT,
            operation TEXT NOT NULL CHECK (operation IN ('launch', 'input')),
            request_digest TEXT NOT NULL,
            session_id TEXT NOT NULL,
            execution_binding_json TEXT NOT NULL,
            principal_ref TEXT,
            project_digest TEXT,
            graph_id TEXT,
            node_id TEXT,
            attempt_id TEXT,
            adopted_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE RESTRICT,
            CHECK (
                (adoption_key IS NULL AND contract IS NULL AND operation = 'launch')
                OR
                (adoption_key IS NOT NULL AND contract IS NOT NULL)
            ),
            CHECK (
                (operation = 'launch'
                    AND principal_ref IS NOT NULL
                    AND project_digest IS NOT NULL
                    AND graph_id IS NOT NULL
                    AND node_id IS NOT NULL
                    AND attempt_id IS NOT NULL)
                OR
                (operation = 'input'
                    AND adoption_key IS NOT NULL
                    AND principal_ref IS NULL
                    AND project_digest IS NULL
                    AND graph_id IS NULL
                    AND node_id IS NULL
                    AND attempt_id IS NULL)
            )
        );
CREATE TABLE events (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            redaction_status TEXT NOT NULL DEFAULT 'redacted',
            sensitive INTEGER NOT NULL DEFAULT 0,
            request_adoption_id TEXT REFERENCES request_adoptions(id) ON DELETE RESTRICT,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );
CREATE TABLE session_handoffs (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            packet_json TEXT NOT NULL,
            event_cursor INTEGER NOT NULL,
            workspace_json TEXT NOT NULL,
            state TEXT NOT NULL,
            claimant TEXT,
            idempotency_key TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(session_id, generation),
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );
CREATE TABLE session_input_leases (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );
CREATE TABLE handoff_continuations (
            id TEXT PRIMARY KEY NOT NULL,
            handoff_id TEXT NOT NULL,
            source_session_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            destination TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(handoff_id, destination),
            FOREIGN KEY (handoff_id) REFERENCES session_handoffs(id) ON DELETE CASCADE
        );
CREATE TABLE sensitive_artifacts (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            nonce BLOB NOT NULL,
            ciphertext BLOB NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
        );
CREATE TABLE repositories (
            id TEXT PRIMARY KEY NOT NULL,
            path TEXT NOT NULL,
            package_name TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE store_meta (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
INSERT INTO store_meta VALUES('events_fts_backfill_complete','1');
INSERT INTO store_meta VALUES('travel_source_hub_id','hub_40914d51-27eb-478d-862d-db2254d1a6c0');
CREATE TABLE travel_profiles (
            id TEXT PRIMARY KEY NOT NULL,
            familiar_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            version TEXT NOT NULL,
            generated_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            stale_after TEXT NOT NULL,
            source_hub_id TEXT NOT NULL,
            source_revision_json TEXT NOT NULL,
            permissions_json TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            encoding TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            profile_blob TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
CREATE TABLE travel_deltas (
            id TEXT PRIMARY KEY NOT NULL,
            profile_id TEXT NOT NULL,
            source_hub_id TEXT NOT NULL,
            client_id TEXT NOT NULL,
            state TEXT NOT NULL,
            raw_delta_json TEXT NOT NULL,
            accepted_events INTEGER NOT NULL,
            accepted_artifacts INTEGER NOT NULL,
            memory_review_state TEXT NOT NULL,
            canonical_memory_overwrite_applied INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (profile_id) REFERENCES travel_profiles(id) ON DELETE CASCADE
        );
CREATE TABLE scheduler_decisions (
            id TEXT PRIMARY KEY NOT NULL,
            job_id TEXT NOT NULL,
            target_role TEXT NOT NULL,
            target_node_id TEXT,
            target_json TEXT NOT NULL,
            reason TEXT NOT NULL,
            inputs_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
CREATE TABLE executor_queue (
            node_id TEXT PRIMARY KEY NOT NULL,
            job_ids_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE node_registry (
            node_id TEXT PRIMARY KEY NOT NULL,
            role TEXT NOT NULL,
            transport TEXT NOT NULL,
            transport_config_json TEXT,
            capabilities_json TEXT NOT NULL,
            available INTEGER NOT NULL DEFAULT 0,
            queue_pressure INTEGER NOT NULL DEFAULT 0,
            last_health_at TEXT NOT NULL,
            last_error TEXT,
            registered_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE executor_dispatches (
            job_id TEXT PRIMARY KEY NOT NULL,
            node_id TEXT NOT NULL,
            status TEXT NOT NULL,
            job_json TEXT NOT NULL,
            envelope_json TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE executor_result_envelopes (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            envelope_id TEXT UNIQUE NOT NULL,
            job_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            envelope_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL
        );
CREATE TABLE hub_jobs (
            job_id TEXT PRIMARY KEY NOT NULL,
            state TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            required_capabilities_json TEXT NOT NULL,
            assigned_node_id TEXT,
            loop_id TEXT,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE routing_table (
            job_id TEXT PRIMARY KEY NOT NULL,
            node_id TEXT NOT NULL,
            decision_id TEXT,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE loop_state (
            loop_id TEXT PRIMARY KEY NOT NULL,
            job_id TEXT NOT NULL,
            state TEXT NOT NULL,
            decision_id TEXT NOT NULL,
            target_json TEXT NOT NULL,
            preserved_subqueue_node_id TEXT NOT NULL,
            node_availability_json TEXT NOT NULL,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (decision_id) REFERENCES scheduler_decisions(id) ON DELETE CASCADE
        );
PRAGMA writable_schema=ON;
INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)VALUES('table','events_fts','events_fts',0,'CREATE VIRTUAL TABLE events_fts USING fts5(
            payload_json,
            content=''events'',
            content_rowid=''rowid''
        )');
CREATE TABLE IF NOT EXISTS 'events_fts_data'(id INTEGER PRIMARY KEY, block BLOB);
INSERT INTO events_fts_data VALUES(1,X'');
INSERT INTO events_fts_data VALUES(10,X'00000000000000');
CREATE TABLE IF NOT EXISTS 'events_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS 'events_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE IF NOT EXISTS 'events_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;
INSERT INTO events_fts_config VALUES('version',4);
CREATE TABLE ward_manifest (
    familiar_id  TEXT NOT NULL,
    surface      TEXT NOT NULL,
    manifest_id  TEXT NOT NULL,
    entry_hash   BLOB NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (familiar_id, surface)
);
CREATE TABLE automation_definitions (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        status TEXT NOT NULL,
        definition_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
INSERT INTO automation_definitions VALUES('nightly-review','Nightly review','ACTIVE','{"schemaVersion":1,"id":"nightly-review","name":"Nightly review","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=3","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"codex","cwd":"/work/project","prompt":"Summarize the changes merged yesterday and list follow-ups."}','2026-10-02T01:51:22.487Z','2026-10-02T01:51:22.802Z');
INSERT INTO automation_definitions VALUES('weekly-digest','Weekly digest','PAUSED','{"schemaVersion":1,"id":"weekly-digest","name":"Weekly digest","status":"PAUSED","rrule":"FREQ=WEEKLY;BYDAY=MO,TH;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"claude","familiarId":"charm","cwd":"/work/project","prompt":"Write the weekly digest and flag open risks."}','2026-10-02T01:51:22.514Z','2026-10-02T01:51:22.774Z');
INSERT INTO automation_definitions VALUES('legacy-standup','Legacy standup','PAUSED','{"schemaVersion":1,"id":"legacy-standup","name":"Legacy standup","status":"PAUSED","rrule":"FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=8","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":60,"runtime":"coven-code","prompt":"Draft the standup notes."}','2026-10-02T01:51:22.544Z','2026-10-02T01:51:22.544Z');
CREATE TABLE automation_occurrences (
        id TEXT PRIMARY KEY NOT NULL,
        automation_id TEXT NOT NULL,
        scheduled_for TEXT NOT NULL,
        kind TEXT NOT NULL DEFAULT 'scheduled',
        state TEXT NOT NULL DEFAULT 'planned',
        lease_owner TEXT,
        lease_expires_at TEXT,
        attempt INTEGER NOT NULL DEFAULT 0,
        failure_reason TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        UNIQUE(automation_id, scheduled_for)
    );
INSERT INTO automation_occurrences VALUES('occ-d1bb940e722b46ed85df40e0a4c635df','nightly-review','2026-10-02T01:51:22.599144000Z','manual','failed',NULL,NULL,1,'failed to spawn harness `codex` in piped mode: No such file or directory (os error 2)','2026-10-02T01:51:22.599Z','2026-10-02T01:51:22.601Z');
INSERT INTO automation_occurrences VALUES('occ-c87fcc5616734d1f8033ebc6655e7a96','weekly-digest','2026-10-02T01:51:22.832672000Z','manual','failed',NULL,NULL,1,'unknown familiar `charm`; no familiars are configured in /coven-home/familiars.toml','2026-10-02T01:51:22.832Z','2026-10-02T01:51:22.834Z');
CREATE TABLE automation_runs (
        id TEXT PRIMARY KEY NOT NULL,
        automation_id TEXT NOT NULL,
        occurrence_id TEXT,
        session_id TEXT,
        familiar_id TEXT,
        runtime TEXT,
        status TEXT NOT NULL,
        exit_code INTEGER,
        log_json TEXT,
        output_commit TEXT,
        started_at TEXT NOT NULL,
        timeout_at TEXT,
        finished_at TEXT,
        FOREIGN KEY (occurrence_id) REFERENCES automation_occurrences(id) ON DELETE SET NULL
    );
INSERT INTO automation_runs VALUES('run-e9999d226f7c489d86937bc456c12176','nightly-review','occ-d1bb940e722b46ed85df40e0a4c635df','session-3bec78bf81fa4606ba82fe9f7a78ced2',NULL,'codex','failed',NULL,NULL,NULL,'2026-10-02T01:51:22.601Z','2026-10-02T02:21:22.601Z','2026-10-02T01:51:22.601Z');
INSERT INTO automation_runs VALUES('run-d118d9690d1549bdb86e61acb31898c4','weekly-digest','occ-c87fcc5616734d1f8033ebc6655e7a96','session-179a7eeb4ef746dc9eae908eb3da3b25','charm','claude','failed',NULL,NULL,NULL,'2026-10-02T01:51:22.834Z','2026-10-02T02:21:22.834Z','2026-10-02T01:51:22.834Z');
INSERT INTO sqlite_sequence VALUES('executor_result_envelopes',0);
CREATE TRIGGER ward_audit_append_only_update
BEFORE UPDATE ON ward_audit
BEGIN
    SELECT RAISE(ABORT, 'ward_audit is append-only (RFC-0001 §5.6)');
END;
CREATE TRIGGER ward_audit_append_only_delete
BEFORE DELETE ON ward_audit
BEGIN
    SELECT RAISE(ABORT, 'ward_audit is append-only (RFC-0001 §5.6)');
END;
CREATE TRIGGER ward_audit_require_single_terminal_insert
BEFORE INSERT ON ward_audit
WHEN NEW.event_type IN ('proposal_approved','proposal_rejected','proposal_vetoed')
    AND (
        NEW.proposal_id IS NULL
        OR EXISTS (
            SELECT 1 FROM ward_audit
            WHERE proposal_id = NEW.proposal_id
              AND event_type IN (
                  'proposal_approved','proposal_rejected','proposal_vetoed'
              )
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'proposal requires exactly one terminal event');
END;
CREATE TRIGGER ward_audit_require_authorization_insert
BEFORE INSERT ON ward_audit
WHEN NEW.event_type IN ('ward_updated', 'principal_authorized_write')
    AND (
        NEW.detail IS NULL OR NOT json_valid(NEW.detail)
        OR json_type(NEW.detail, '$.principal_authorization') IS NOT 'text'
        OR COALESCE(length(trim(json_extract(NEW.detail, '$.principal_authorization'))), 0) = 0
    )
BEGIN
    SELECT RAISE(ABORT, 'authorized Ward writes require principal_authorization');
END;
CREATE TRIGGER ward_audit_require_proposal_approval_detail_insert
BEFORE INSERT ON ward_audit
WHEN NEW.event_type = 'proposal_approved'
    AND (
        NEW.detail IS NULL OR NOT json_valid(NEW.detail)
        OR json_type(NEW.detail, '$.approval_path_label') IS NOT 'text'
        OR json_extract(NEW.detail, '$.approval_path_label')
            NOT IN ('auto','familiar_review','human_review','human_required')
        OR COALESCE(json_type(NEW.detail, '$.rationale'), 'missing')
            NOT IN ('null','text')
        OR COALESCE(json_type(NEW.detail, '$.window_close'), 'missing')
            NOT IN ('null','object')
        OR (
            json_extract(NEW.detail, '$.approval_path_label')
                IN ('human_review','human_required')
            AND (
                NEW.approver IS NULL
                OR length(trim(NEW.approver)) = 0
            )
        )
        OR (
            json_extract(NEW.detail, '$.approval_path_label') = 'human_required'
            AND (
                json_type(NEW.detail, '$.rationale') IS NOT 'text'
                OR length(trim(json_extract(NEW.detail, '$.rationale'))) = 0
            )
        )
        OR (
            json_extract(NEW.detail, '$.approval_path_label')
                IN ('human_review','human_required')
            AND json_type(NEW.detail, '$.window_close') IS NOT 'null'
        )
        OR (
            json_extract(NEW.detail, '$.approval_path_label') = 'familiar_review'
            AND json_type(NEW.detail, '$.window_close') IS NOT 'object'
        )
        OR (
            EXISTS (
                SELECT 1 FROM ward_audit
                WHERE proposal_id = NEW.proposal_id
                  AND event_type = 'proposal_window_opened'
            )
            AND json_type(NEW.detail, '$.window_close') IS NOT 'object'
        )
        OR (
            json_type(NEW.detail, '$.window_close') IS 'object'
            AND (
                json_extract(NEW.detail, '$.window_close.reason') != 'applied'
                OR json_type(NEW.detail, '$.window_close.replay_hash_matched')
                    IS NOT 'true'
                OR COALESCE(
                    json_type(NEW.detail, '$.window_close.rationale'),
                    'missing'
                ) NOT IN ('null','text')
            )
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'proposal approval requires valid path-specific detail');
END;
CREATE TRIGGER ward_audit_require_window_close_detail_insert
BEFORE INSERT ON ward_audit
WHEN NEW.event_type IN ('proposal_rejected', 'proposal_vetoed')
    AND EXISTS (
        SELECT 1 FROM ward_audit
        WHERE proposal_id = NEW.proposal_id
          AND event_type = 'proposal_window_opened'
    )
    AND (
        NEW.detail IS NULL OR NOT json_valid(NEW.detail)
        OR json_type(NEW.detail, '$.reason') IS NOT 'text'
        OR json_extract(NEW.detail, '$.reason') NOT IN (
            'applied','vetoed','evidence_diverged','revalidation_failed','superseded'
        )
        OR COALESCE(json_type(NEW.detail, '$.replay_hash_matched'), 'missing')
            NOT IN ('null','true','false')
        OR COALESCE(json_type(NEW.detail, '$.rationale'), 'missing')
            NOT IN ('null','text')
        OR (
            NEW.event_type = 'proposal_vetoed'
            AND json_extract(NEW.detail, '$.reason') != 'vetoed'
        )
        OR (
            NEW.event_type = 'proposal_rejected'
            AND json_extract(NEW.detail, '$.reason')
                NOT IN ('evidence_diverged','revalidation_failed','superseded')
        )
        OR (
            json_extract(NEW.detail, '$.reason') = 'applied'
            AND json_type(NEW.detail, '$.replay_hash_matched') IS NOT 'true'
        )
        OR (
            json_extract(NEW.detail, '$.reason')
                IN ('evidence_diverged','revalidation_failed')
            AND json_type(NEW.detail, '$.replay_hash_matched') IS NOT 'false'
        )
        OR (
            json_extract(NEW.detail, '$.reason') IN ('vetoed','superseded')
            AND json_type(NEW.detail, '$.replay_hash_matched') IS NOT 'null'
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'window terminal events require a valid close reason');
END;
CREATE TRIGGER events_fts_insert AFTER INSERT ON events BEGIN
            INSERT INTO events_fts(rowid, payload_json) VALUES (new.rowid, new.payload_json);
        END;
CREATE TRIGGER events_fts_delete AFTER DELETE ON events BEGIN
            INSERT INTO events_fts(events_fts, rowid, payload_json) VALUES('delete', old.rowid, old.payload_json);
        END;
CREATE TRIGGER events_fts_update AFTER UPDATE ON events BEGIN
            INSERT INTO events_fts(events_fts, rowid, payload_json) VALUES('delete', old.rowid, old.payload_json);
            INSERT INTO events_fts(rowid, payload_json) VALUES (new.rowid, new.payload_json);
        END;
CREATE TRIGGER events_request_adoption_integrity
         BEFORE INSERT ON events
         WHEN NEW.request_adoption_id IS NOT NULL
         BEGIN
           SELECT CASE WHEN NEW.kind != 'input' OR NOT EXISTS (
             SELECT 1 FROM request_adoptions
             WHERE id = NEW.request_adoption_id AND operation = 'input' AND session_id = NEW.session_id
           ) THEN RAISE(ABORT, 'invalid request adoption event correlation') END;
         END;
CREATE TRIGGER events_request_adoption_update_integrity
         BEFORE UPDATE OF session_id, kind, request_adoption_id ON events
         WHEN NEW.request_adoption_id IS NOT NULL
         BEGIN
           SELECT CASE WHEN NEW.kind != 'input' OR NOT EXISTS (
             SELECT 1 FROM request_adoptions
             WHERE id = NEW.request_adoption_id AND operation = 'input' AND session_id = NEW.session_id
           ) THEN RAISE(ABORT, 'invalid request adoption event correlation') END;
         END;
CREATE TRIGGER events_request_adoption_no_rebind
         BEFORE UPDATE OF session_id, kind, request_adoption_id ON events
         WHEN (OLD.request_adoption_id IS NOT NULL OR NEW.request_adoption_id IS NOT NULL)
           AND (NEW.request_adoption_id IS NOT OLD.request_adoption_id OR NEW.session_id IS NOT OLD.session_id OR NEW.kind IS NOT OLD.kind)
         BEGIN
           SELECT RAISE(ABORT, 'request adoption event correlation is immutable');
         END;
CREATE TRIGGER request_adoptions_no_update
         BEFORE UPDATE ON request_adoptions
         BEGIN
           SELECT RAISE(ABORT, 'request adoptions are immutable');
         END;
CREATE TRIGGER request_adoptions_no_delete
         BEFORE DELETE ON request_adoptions
         BEGIN
           SELECT RAISE(ABORT, 'request adoptions are retained');
         END;
CREATE TRIGGER request_adoptions_no_replace
         BEFORE INSERT ON request_adoptions
         BEGIN
           SELECT CASE
             WHEN EXISTS (
               SELECT 1 FROM request_adoptions WHERE id = NEW.id
             ) THEN RAISE(ABORT, 'request adoptions are retained')
             WHEN NEW.adoption_key IS NOT NULL AND EXISTS (
               SELECT 1 FROM request_adoptions WHERE adoption_key = NEW.adoption_key
             ) THEN RAISE(ABORT, 'request adoptions are retained')
             WHEN NEW.operation = 'launch' AND EXISTS (
               SELECT 1 FROM request_adoptions
               WHERE operation = 'launch'
                 AND principal_ref = NEW.principal_ref
                 AND project_digest = NEW.project_digest
                 AND graph_id = NEW.graph_id
                 AND node_id = NEW.node_id
                 AND attempt_id = NEW.attempt_id
             ) THEN RAISE(ABORT, 'request adoptions are retained')
             WHEN NEW.operation = 'launch' AND EXISTS (
               SELECT 1 FROM request_adoptions
               WHERE operation = 'launch' AND session_id = NEW.session_id
             ) THEN RAISE(ABORT, 'request adoptions are retained')
           END;
         END;
CREATE INDEX ward_audit_familiar_idx ON ward_audit (familiar_id, recorded_at);
CREATE INDEX ward_audit_event_idx    ON ward_audit (event_type, recorded_at);
CREATE INDEX idx_sessions_created_at
            ON sessions(created_at DESC);
CREATE INDEX idx_events_session_created_at
            ON events(session_id, created_at);
CREATE INDEX idx_session_handoffs_session
            ON session_handoffs(session_id, generation DESC);
CREATE INDEX idx_session_input_leases_session
            ON session_input_leases(session_id);
CREATE INDEX idx_events_created_at
            ON events(created_at);
CREATE INDEX idx_sensitive_artifacts_session
            ON sensitive_artifacts(session_id, created_at);
CREATE INDEX idx_sensitive_artifacts_expires_at
            ON sensitive_artifacts(expires_at);
CREATE INDEX idx_sensitive_artifacts_created_at
            ON sensitive_artifacts(created_at);
CREATE INDEX idx_travel_profiles_scope
            ON travel_profiles(familiar_id, workspace_id, generated_at DESC);
CREATE INDEX idx_travel_deltas_client
            ON travel_deltas(client_id, updated_at DESC);
CREATE INDEX idx_scheduler_decisions_job
            ON scheduler_decisions(job_id, created_at DESC);
CREATE INDEX idx_node_registry_available
            ON node_registry(available, queue_pressure);
CREATE INDEX idx_executor_dispatches_node
            ON executor_dispatches(node_id, updated_at DESC);
CREATE INDEX idx_executor_result_envelopes_job
            ON executor_result_envelopes(job_id, sequence);
CREATE INDEX idx_hub_jobs_state
            ON hub_jobs(state, priority DESC, created_at);
CREATE INDEX idx_hub_jobs_assigned_node
            ON hub_jobs(assigned_node_id, state);
CREATE INDEX idx_routing_table_node
            ON routing_table(node_id, updated_at DESC);
CREATE INDEX idx_loop_state_job
            ON loop_state(job_id, updated_at DESC);
CREATE INDEX idx_sessions_conversation_id
            ON sessions(conversation_id);
CREATE INDEX idx_sessions_familiar_id
            ON sessions(familiar_id) WHERE familiar_id IS NOT NULL;
CREATE UNIQUE INDEX request_adoptions_key
            ON request_adoptions(adoption_key) WHERE adoption_key IS NOT NULL;
CREATE UNIQUE INDEX request_adoptions_launch_attempt
            ON request_adoptions(principal_ref, project_digest, graph_id, node_id, attempt_id)
            WHERE operation = 'launch';
CREATE UNIQUE INDEX request_adoptions_launch_session
            ON request_adoptions(session_id) WHERE operation = 'launch';
CREATE INDEX request_adoptions_session
            ON request_adoptions(session_id);
CREATE UNIQUE INDEX events_request_adoption
            ON events(request_adoption_id) WHERE request_adoption_id IS NOT NULL;
CREATE INDEX idx_automation_definitions_updated_at
        ON automation_definitions(updated_at DESC);
CREATE INDEX idx_automation_occurrences_scheduled
        ON automation_occurrences(automation_id, scheduled_for);
CREATE INDEX idx_automation_occurrences_state
        ON automation_occurrences(state, lease_expires_at);
CREATE INDEX idx_automation_runs_automation_started
        ON automation_runs(automation_id, started_at DESC);
PRAGMA writable_schema=OFF;
COMMIT;
