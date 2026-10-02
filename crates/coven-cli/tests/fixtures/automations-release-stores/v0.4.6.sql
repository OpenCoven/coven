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
CREATE TABLE automation_runtime_terminal_evidence (
        evidence_id TEXT PRIMARY KEY NOT NULL,
        attempt_id TEXT UNIQUE NOT NULL,
        session_id TEXT UNIQUE NOT NULL,
        run_id TEXT NOT NULL,
        binding_id TEXT NOT NULL,
        binding_digest TEXT UNIQUE NOT NULL CHECK (
            length(binding_digest) = 64
            AND binding_digest NOT GLOB '*[^0-9a-f]*'
        ),
        runtime_id TEXT NOT NULL,
        descriptor_digest TEXT NOT NULL CHECK (
            length(descriptor_digest) = 64
            AND descriptor_digest NOT GLOB '*[^0-9a-f]*'
        ),
        producer_key_id TEXT NOT NULL,
        evidence_digest TEXT NOT NULL CHECK (
            length(evidence_digest) = 64
            AND evidence_digest NOT GLOB '*[^0-9a-f]*'
        ),
        classification TEXT NOT NULL CHECK (
            classification IN (
                'receipt_eligible_complete',
                'authenticated_partial_or_ambiguous',
                'authenticated_unknown',
                'policy_violating'
            )
        ),
        produced_at TEXT NOT NULL,
        received_at TEXT NOT NULL,
        canonical_json TEXT NOT NULL CHECK (json_valid(canonical_json)),
        FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE RESTRICT,
        FOREIGN KEY (run_id) REFERENCES automation_runs(id) ON DELETE RESTRICT,
        FOREIGN KEY (attempt_id) REFERENCES automation_attempts(id) ON DELETE RESTRICT
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
INSERT INTO sessions VALUES('session-52d3c6fa3ae54c04b8018d73812f0c68','/work/project','codex','Nightly review','failed',NULL,NULL,'2026-10-02T01:51:23.377Z','2026-10-02T01:51:23.547Z',NULL,NULL,'private',NULL,NULL,0,NULL,NULL);
INSERT INTO sessions VALUES('session-35b82012a5cf445a87ac49a207d4385d','/work/project','claude','Weekly digest','failed',NULL,NULL,'2026-10-02T01:51:23.636Z','2026-10-02T01:51:23.638Z',NULL,NULL,'private','charm',NULL,0,NULL,NULL);
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
INSERT INTO store_meta VALUES('travel_source_hub_id','hub_b2b42b39-b3f4-42d9-b933-dbae69665542');
CREATE TABLE coven_ward_audit_capacity (
            singleton          INTEGER PRIMARY KEY CHECK (singleton = 1),
            limit_bytes        INTEGER NOT NULL CHECK (limit_bytes > 0),
            used_bytes         INTEGER NOT NULL CHECK (used_bytes >= 0),
            row_overhead_bytes INTEGER NOT NULL CHECK (row_overhead_bytes > 0),
            wal_limit_bytes    INTEGER NOT NULL CHECK (wal_limit_bytes > 0)
        );
INSERT INTO coven_ward_audit_capacity VALUES(1,268435456,0,16384,134217728);
CREATE TABLE coven_ward_audit_reservations (
            token          TEXT PRIMARY KEY NOT NULL,
            purpose        TEXT NOT NULL,
            reserved_bytes INTEGER NOT NULL CHECK (reserved_bytes >= 0),
            created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        );
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
        revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
        definition_digest TEXT,
        lifecycle_state TEXT NOT NULL DEFAULT 'draft'
            CHECK (lifecycle_state IN ('draft', 'paused', 'active', 'disabled', 'invalid')),
        tombstoned_at TEXT,
        authority_version INTEGER NOT NULL DEFAULT 0 CHECK (authority_version IN (0, 1)),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
INSERT INTO automation_definitions VALUES('nightly-review','Nightly review','ACTIVE','{"schemaVersion":1,"id":"nightly-review","name":"Nightly review","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=3","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"codex","cwd":"/work/project","prompt":"Summarize the changes merged yesterday and list follow-ups."}',2,'984bc39689b3348b78c228b93a2f85871dc00813e7b04e7e1196e61f8e46bb7f','active',NULL,0,'2026-10-02T01:51:23.203Z','2026-10-02T01:51:23.603Z');
INSERT INTO automation_definitions VALUES('weekly-digest','Weekly digest','PAUSED','{"schemaVersion":1,"id":"weekly-digest","name":"Weekly digest","status":"PAUSED","rrule":"FREQ=WEEKLY;BYDAY=MO,TH;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"claude","familiarId":"charm","cwd":"/work/project","prompt":"Write the weekly digest and flag open risks."}',2,'e3c9b82bcd70ba2f1391383715a1a5d1ae4a51f576e68347cc7dd9dfafc9c2a6','paused',NULL,0,'2026-10-02T01:51:23.263Z','2026-10-02T01:51:23.577Z');
INSERT INTO automation_definitions VALUES('legacy-standup','Legacy standup','PAUSED','{"schemaVersion":1,"id":"legacy-standup","name":"Legacy standup","status":"PAUSED","rrule":"FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=8","timezone":"UTC","misfire":"latest","overlap":"forbid","timeoutMinutes":60,"runtime":"coven-code","prompt":"Draft the standup notes."}',1,'f8ba39e750792229998dd5950a6e48330419aec9e5c446ba9461ed5e810506fb','paused',NULL,0,'2026-10-02T01:51:23.298Z','2026-10-02T01:51:23.298Z');
CREATE TABLE automation_command_reservations (
        adoption_key TEXT PRIMARY KEY NOT NULL,
        request_digest TEXT NOT NULL,
        command TEXT NOT NULL,
        reserved_at TEXT NOT NULL
    );
CREATE TABLE automation_command_adoptions (
        adoption_key TEXT PRIMARY KEY NOT NULL,
        request_digest TEXT NOT NULL,
        command TEXT NOT NULL,
        automation_id TEXT,
        outcome TEXT NOT NULL CHECK (outcome IN ('committed', 'rejected')),
        revision INTEGER,
        response_json TEXT NOT NULL,
        adopted_at TEXT NOT NULL
    );
INSERT INTO automation_command_adoptions VALUES('legacy:8bceb2b4d02945b1834372ceaf13366c','86f02a963e054b805c3f6f8fa0ca1cb25a193d576daf58cb77f8f2c403f8109d','legacy.definition.create.v1','nightly-review','committed',1,'{"outcome":"committed","result":{"createdAt":"2026-10-02T01:51:23.203Z","routine":{"cwd":"/work/project","id":"nightly-review","misfire":"latest","name":"Nightly review","overlap":"forbid","prompt":"Summarize the changes merged yesterday.","rrule":"FREQ=DAILY;BYHOUR=3","runtime":"codex","schemaVersion":1,"status":"ACTIVE","timeoutMinutes":30,"timezone":"utc"}},"event_ref":{"stream":"automation/nightly-review","sequence":0}}','2026-10-02T01:51:23.203Z');
INSERT INTO automation_command_adoptions VALUES('legacy:59483f7757a04241bdedadb17ccdd6ca','fb030a98f9f68b37e5753b4e50dd1b8302f4cf39519f97bb9fcc105d2149cab0','legacy.definition.create.v1','weekly-digest','committed',1,'{"outcome":"committed","result":{"createdAt":"2026-10-02T01:51:23.263Z","routine":{"cwd":"/work/project","familiarId":"charm","id":"weekly-digest","misfire":"latest","name":"Weekly digest","overlap":"forbid","prompt":"Write the weekly digest.","rrule":"FREQ=WEEKLY;BYDAY=MO;BYHOUR=9","runtime":"claude","schemaVersion":1,"status":"PAUSED","timeoutMinutes":30,"timezone":"utc"}},"event_ref":{"stream":"automation/weekly-digest","sequence":0}}','2026-10-02T01:51:23.263Z');
INSERT INTO automation_command_adoptions VALUES('legacy:c8d365f64a064eac8e7bcc00db4c935b','2c8ffa20a954b193c6e04c8b70cd67ac65b5969e23f933a90c739059e2f91957','legacy.definition.revise.v1','weekly-digest','committed',2,'{"outcome":"committed","result":{"routine":{"cwd":"/work/project","familiarId":"charm","id":"weekly-digest","misfire":"latest","name":"Weekly digest","overlap":"forbid","prompt":"Write the weekly digest and flag open risks.","rrule":"FREQ=WEEKLY;BYDAY=MO,TH;BYHOUR=9","runtime":"claude","schemaVersion":1,"status":"PAUSED","timeoutMinutes":30,"timezone":"utc"},"updatedAt":"2026-10-02T01:51:23.577Z"},"event_ref":{"stream":"automation/weekly-digest","sequence":1}}','2026-10-02T01:51:23.577Z');
INSERT INTO automation_command_adoptions VALUES('legacy:0498104d3bf942ffb86cdfca2435ca69','1eaceb739f0eedf50ac63fcdb99919754e6f3c43f95d2840ba0d483f083e9866','legacy.definition.revise.v1','nightly-review','committed',2,'{"outcome":"committed","result":{"routine":{"cwd":"/work/project","id":"nightly-review","misfire":"latest","name":"Nightly review","overlap":"forbid","prompt":"Summarize the changes merged yesterday and list follow-ups.","rrule":"FREQ=DAILY;BYHOUR=3","runtime":"codex","schemaVersion":1,"status":"ACTIVE","timeoutMinutes":30,"timezone":"utc"},"updatedAt":"2026-10-02T01:51:23.603Z"},"event_ref":{"stream":"automation/nightly-review","sequence":1}}','2026-10-02T01:51:23.603Z');
CREATE TABLE automation_cancellations (
        adoption_key TEXT PRIMARY KEY NOT NULL,
        request_digest TEXT NOT NULL,
        automation_id TEXT NOT NULL,
        run_id TEXT NOT NULL UNIQUE,
        attempt_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        scope TEXT NOT NULL CHECK (scope = 'run'),
        requested_by_json TEXT NOT NULL,
        reason TEXT,
        state TEXT NOT NULL CHECK (
            state IN ('requested', 'stopping', 'cancelled', 'recovery_required', 'rejected')
        ),
        requested_at TEXT NOT NULL,
        execution_expires_at TEXT NOT NULL,
        acknowledged_at TEXT,
        reconciled_at TEXT,
        result_json TEXT
    );
CREATE TABLE automation_stop_fences (
        run_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL,
        owner TEXT NOT NULL CHECK (owner IN ('cancellation', 'timeout', 'recovery')),
        operation_key TEXT,
        acquired_at TEXT NOT NULL,
        execution_expires_at TEXT NOT NULL
    );
CREATE TABLE automation_scheduler_authority (
        id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
        owner_id TEXT,
        generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
        acquired_at TEXT
    );
INSERT INTO automation_scheduler_authority VALUES(1,NULL,1,'2026-10-02T01:51:23.050Z');
CREATE TABLE automation_scheduler_last_pass (
        id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
        scheduler_generation INTEGER NOT NULL CHECK (scheduler_generation >= 1),
        trigger TEXT NOT NULL CHECK (trigger IN ('startup', 'deadline', 'wake')),
        scheduled_at TEXT NOT NULL,
        started_at TEXT NOT NULL,
        finished_at TEXT NOT NULL,
        duration_ms INTEGER NOT NULL CHECK (duration_ms >= 0),
        status TEXT NOT NULL CHECK (status IN ('succeeded', 'failed')),
        error_class TEXT,
        planned INTEGER CHECK (planned IS NULL OR planned >= 0),
        recovered INTEGER CHECK (recovered IS NULL OR recovered >= 0),
        claimed INTEGER CHECK (claimed IS NULL OR claimed >= 0),
        dispatched INTEGER CHECK (dispatched IS NULL OR dispatched >= 0),
        failures INTEGER CHECK (failures IS NULL OR failures >= 0),
        CHECK (
            (
                status = 'succeeded'
                AND error_class IS NULL
                AND planned IS NOT NULL
                AND recovered IS NOT NULL
                AND claimed IS NOT NULL
                AND dispatched IS NOT NULL
                AND failures IS NOT NULL
            )
            OR (status = 'failed' AND error_class IS NOT NULL)
        )
    );
INSERT INTO automation_scheduler_last_pass VALUES(1,1,'wake','2026-10-02T01:51:23.606Z','2026-10-02T01:51:23.606Z','2026-10-02T01:51:23.617Z',10,'succeeded',NULL,0,0,0,0,0);
CREATE TABLE automation_occurrences (
        id TEXT PRIMARY KEY NOT NULL,
        automation_id TEXT NOT NULL,
        automation_revision INTEGER NOT NULL DEFAULT 1 CHECK (automation_revision >= 1),
        definition_digest TEXT,
        scheduled_for TEXT NOT NULL,
        kind TEXT NOT NULL DEFAULT 'scheduled',
        state TEXT NOT NULL DEFAULT 'planned',
        lease_owner TEXT,
        lease_expires_at TEXT,
        scheduler_generation INTEGER CHECK (scheduler_generation IS NULL OR scheduler_generation >= 1),
        attempt INTEGER NOT NULL DEFAULT 0,
        failure_reason TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        UNIQUE(automation_id, scheduled_for)
    );
INSERT INTO automation_occurrences VALUES('occ-be2126fafa27428594c06c182deac119','nightly-review',1,'3d520bfda3ee4b66a5d60bdec9190bd7ccd17300660cc89eae501a237d931630','2026-10-02T01:51:23.375796000Z','manual','failed',NULL,NULL,NULL,1,'failed to spawn harness `codex` in piped mode: No such file or directory (os error 2)','2026-10-02T01:51:23.375Z','2026-10-02T01:51:23.547Z');
INSERT INTO automation_occurrences VALUES('occ-26b8d977f09a4182afc8a3b06b7d0c58','weekly-digest',2,'e3c9b82bcd70ba2f1391383715a1a5d1ae4a51f576e68347cc7dd9dfafc9c2a6','2026-10-02T01:51:23.634946000Z','manual','failed',NULL,NULL,NULL,1,'unknown familiar `charm`; no familiars are configured in /coven-home/familiars.toml','2026-10-02T01:51:23.634Z','2026-10-02T01:51:23.638Z');
CREATE TABLE automation_scheduler_planning_cursor (
        id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
        after_name TEXT,
        after_definition_id TEXT,
        revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
        CHECK (
            (after_name IS NULL AND after_definition_id IS NULL)
            OR (after_name IS NOT NULL AND after_definition_id IS NOT NULL)
        )
    );
INSERT INTO automation_scheduler_planning_cursor VALUES(1,NULL,NULL,7);
CREATE TABLE automation_runs (
        id TEXT PRIMARY KEY NOT NULL,
        automation_id TEXT NOT NULL,
        automation_revision INTEGER NOT NULL DEFAULT 1 CHECK (automation_revision >= 1),
        definition_digest TEXT,
        definition_json TEXT,
        occurrence_id TEXT,
        authority_profile TEXT CHECK (
            authority_profile IS NULL
            OR authority_profile = 'coven.automations.authority.v1'
        ),
        receipt_id TEXT,
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
INSERT INTO automation_runs VALUES('run-db6e6bd8cdea4dec8bf18bc022619c60','nightly-review',1,'3d520bfda3ee4b66a5d60bdec9190bd7ccd17300660cc89eae501a237d931630','{"schemaVersion":1,"id":"nightly-review","name":"Nightly review","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=3","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"codex","cwd":"/work/project","prompt":"Summarize the changes merged yesterday."}','occ-be2126fafa27428594c06c182deac119',NULL,NULL,'session-52d3c6fa3ae54c04b8018d73812f0c68',NULL,'codex','failed',NULL,NULL,NULL,'2026-10-02T01:51:23.377Z','2026-10-02T02:21:23.377Z','2026-10-02T01:51:23.547Z');
INSERT INTO automation_runs VALUES('run-03eb84c547ac46e48d1a52b5ec51361f','weekly-digest',2,'e3c9b82bcd70ba2f1391383715a1a5d1ae4a51f576e68347cc7dd9dfafc9c2a6','{"schemaVersion":1,"id":"weekly-digest","name":"Weekly digest","status":"PAUSED","rrule":"FREQ=WEEKLY;BYDAY=MO,TH;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"claude","familiarId":"charm","cwd":"/work/project","prompt":"Write the weekly digest and flag open risks."}','occ-26b8d977f09a4182afc8a3b06b7d0c58',NULL,NULL,'session-35b82012a5cf445a87ac49a207d4385d','charm','claude','failed',NULL,NULL,NULL,'2026-10-02T01:51:23.636Z','2026-10-02T02:21:23.636Z','2026-10-02T01:51:23.638Z');
CREATE TABLE automation_attempts (
        id TEXT PRIMARY KEY NOT NULL,
        run_id TEXT NOT NULL,
        occurrence_id TEXT NOT NULL,
        attempt_number INTEGER NOT NULL CHECK (attempt_number BETWEEN 1 AND 10),
        adoption_key TEXT NOT NULL UNIQUE,
        occurrence_fence_generation INTEGER NOT NULL
            CHECK (occurrence_fence_generation >= 1),
        dispatch_generation INTEGER NOT NULL DEFAULT 0
            CHECK (dispatch_generation >= 0),
        state TEXT NOT NULL CHECK (
            state IN (
                'adopted', 'dispatching', 'started', 'observing',
                'succeeded', 'failed', 'cancelled', 'timed_out', 'ambiguous'
            )
        ),
        failure_class TEXT CHECK (
            failure_class IS NULL OR failure_class IN (
                'transient_dispatch', 'lease_expired', 'runtime_unavailable',
                'launch_refused', 'runtime_error', 'timeout', 'cancelled',
                'ambiguous_evidence', 'runtime_authority_unsupported'
            )
        ),
        prior_attempt_number INTEGER CHECK (
            prior_attempt_number IS NULL OR prior_attempt_number >= 1
        ),
        prior_disposition TEXT CHECK (
            prior_disposition IS NULL OR prior_disposition IN (
                'failed', 'timed_out', 'cancelled', 'ambiguous'
            )
        ),
        retry_classification TEXT NOT NULL CHECK (
            retry_classification IN (
                'initial', 'automatic_retry', 'operator_retry', 'operator_recovery'
            )
        ),
        authority_extension_json TEXT,
        not_before TEXT NOT NULL,
        session_id TEXT UNIQUE,
        state_reason TEXT,
        opened_at TEXT NOT NULL,
        settled_at TEXT,
        FOREIGN KEY (run_id) REFERENCES automation_runs(id) ON DELETE CASCADE,
        FOREIGN KEY (occurrence_id) REFERENCES automation_occurrences(id) ON DELETE RESTRICT,
        FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE RESTRICT,
        UNIQUE (run_id, attempt_number),
        CHECK (
            (attempt_number = 1
             AND prior_attempt_number IS NULL
             AND prior_disposition IS NULL)
            OR
            (attempt_number > 1
             AND prior_attempt_number = attempt_number - 1
             AND prior_disposition IS NOT NULL)
        )
    );
INSERT INTO automation_attempts VALUES('attempt-run-db6e6bd8cdea4dec8bf18bc022619c60-1','run-db6e6bd8cdea4dec8bf18bc022619c60','occ-be2126fafa27428594c06c182deac119',1,'automation:run-db6e6bd8cdea4dec8bf18bc022619c60:1',1,1,'failed','runtime_unavailable',NULL,NULL,'initial',NULL,'2026-10-02T01:51:23.377Z',NULL,'failed to spawn harness `codex` in piped mode: No such file or directory (os error 2)','2026-10-02T01:51:23.377Z','2026-10-02T01:51:23.547Z');
INSERT INTO automation_attempts VALUES('attempt-run-03eb84c547ac46e48d1a52b5ec51361f-1','run-03eb84c547ac46e48d1a52b5ec51361f','occ-26b8d977f09a4182afc8a3b06b7d0c58',1,'automation:run-03eb84c547ac46e48d1a52b5ec51361f:1',1,1,'failed','launch_refused',NULL,NULL,'initial',NULL,'2026-10-02T01:51:23.636Z',NULL,'unknown familiar `charm`; no familiars are configured in /coven-home/familiars.toml','2026-10-02T01:51:23.636Z','2026-10-02T01:51:23.638Z');
CREATE TABLE automation_retry_state (
        automation_id TEXT PRIMARY KEY NOT NULL,
        consecutive_exhaustions INTEGER NOT NULL DEFAULT 0
            CHECK (consecutive_exhaustions >= 0),
        quarantined_at TEXT,
        failure_class TEXT,
        reason TEXT,
        updated_at TEXT NOT NULL,
        FOREIGN KEY (automation_id)
            REFERENCES automation_definitions(id) ON DELETE CASCADE,
        CHECK (
            (quarantined_at IS NULL AND failure_class IS NULL AND reason IS NULL)
            OR
            (quarantined_at IS NOT NULL AND failure_class IS NOT NULL AND reason IS NOT NULL)
        )
    );
CREATE TABLE automation_contract_migrations (
        profile TEXT PRIMARY KEY NOT NULL,
        definitions_migrated INTEGER NOT NULL,
        occurrences_migrated INTEGER NOT NULL,
        runs_migrated INTEGER NOT NULL,
        unverifiable_definitions INTEGER NOT NULL,
        unverifiable_occurrences INTEGER NOT NULL,
        unverifiable_runs INTEGER NOT NULL,
        migrated_at TEXT NOT NULL
    );
INSERT INTO automation_contract_migrations VALUES('coven.automations.v1',0,0,0,0,0,0,'2026-10-02T01:51:23.013Z');
CREATE TABLE automation_event_stream_heads (
        stream_kind TEXT NOT NULL
            CHECK (stream_kind IN ('automation', 'occurrence', 'run', 'feed')),
        stream_id TEXT NOT NULL,
        next_sequence INTEGER NOT NULL CHECK (next_sequence >= 0),
        earliest_sequence INTEGER NOT NULL DEFAULT 0 CHECK (earliest_sequence >= 0),
        updated_at TEXT NOT NULL,
        PRIMARY KEY(stream_kind, stream_id)
    );
INSERT INTO automation_event_stream_heads VALUES('automation','nightly-review',2,0,'2026-10-02T01:51:23.603Z');
INSERT INTO automation_event_stream_heads VALUES('automation','weekly-digest',2,0,'2026-10-02T01:51:23.577Z');
INSERT INTO automation_event_stream_heads VALUES('automation','legacy-standup',1,0,'2026-10-02T01:51:23.298Z');
CREATE TABLE automation_events (
        feed_position INTEGER PRIMARY KEY AUTOINCREMENT,
        event_id TEXT UNIQUE NOT NULL,
        stream_kind TEXT NOT NULL
            CHECK (stream_kind IN ('automation', 'occurrence', 'run', 'feed')),
        stream_id TEXT NOT NULL,
        sequence INTEGER NOT NULL CHECK (sequence >= 0),
        recorded_at TEXT NOT NULL,
        recorded_at_millis INTEGER NOT NULL,
        observed_at TEXT NOT NULL,
        event_json TEXT NOT NULL,
        UNIQUE(stream_kind, stream_id, sequence)
    );
INSERT INTO automation_events VALUES(1,'evtc8fe07cf93c341c28e4bdf5eb0a70c27','automation','nightly-review',0,'2026-10-02T01:51:23.203Z',1790905883203,'2026-10-02T01:51:23.203Z','{"schemaVersion":"coven.automations.v1","eventId":"evtc8fe07cf93c341c28e4bdf5eb0a70c27","stream":{"kind":"automation","id":"nightly-review"},"sequence":0,"recordedAt":"2026-10-02T01:51:23.203Z","observedAt":"2026-10-02T01:51:23.203Z","producer":{"component":"coven-daemon","instanceId":"local-authority"},"causation":{"adoptionKey":"legacy:8bceb2b4d02945b1834372ceaf13366c"},"automationId":"nightly-review","kind":"definition.created","summary":"automation nightly-review active at revision 1","payload":{"revision":1,"definitionDigest":{"algorithm":"sha256","canonicalization":"jcs-rfc8785","value":"3d520bfda3ee4b66a5d60bdec9190bd7ccd17300660cc89eae501a237d931630"},"lifecycleState":"active"},"privacy":{"classification":"operational","retention":{"classification":"standard"}}}');
INSERT INTO automation_events VALUES(2,'evtcb7384288d234660825319c676ad48cc','automation','weekly-digest',0,'2026-10-02T01:51:23.263Z',1790905883263,'2026-10-02T01:51:23.263Z','{"schemaVersion":"coven.automations.v1","eventId":"evtcb7384288d234660825319c676ad48cc","stream":{"kind":"automation","id":"weekly-digest"},"sequence":0,"recordedAt":"2026-10-02T01:51:23.263Z","observedAt":"2026-10-02T01:51:23.263Z","producer":{"component":"coven-daemon","instanceId":"local-authority"},"causation":{"adoptionKey":"legacy:59483f7757a04241bdedadb17ccdd6ca"},"automationId":"weekly-digest","kind":"definition.created","summary":"automation weekly-digest paused at revision 1","payload":{"revision":1,"definitionDigest":{"algorithm":"sha256","canonicalization":"jcs-rfc8785","value":"4837a60fdd5cfc3eedc7dab6ff36ab133262e5736b923581f530a0097e8ad819"},"lifecycleState":"paused"},"privacy":{"classification":"operational","retention":{"classification":"standard"}}}');
INSERT INTO automation_events VALUES(3,'evtc106e47dc4b46bea7cfbd8ba654f94ce','automation','legacy-standup',0,'2026-10-02T01:51:23.298Z',1790905883298,'2026-10-02T01:51:23.298Z','{"schemaVersion":"coven.automations.v1","eventId":"evtc106e47dc4b46bea7cfbd8ba654f94ce","stream":{"kind":"automation","id":"legacy-standup"},"sequence":0,"recordedAt":"2026-10-02T01:51:23.298Z","observedAt":"2026-10-02T01:51:23.298Z","producer":{"component":"coven-daemon","instanceId":"local-authority"},"automationId":"legacy-standup","kind":"definition.imported","summary":"automation legacy-standup paused at revision 1","payload":{"revision":1,"definitionDigest":{"algorithm":"sha256","canonicalization":"jcs-rfc8785","value":"f8ba39e750792229998dd5950a6e48330419aec9e5c446ba9461ed5e810506fb"},"lifecycleState":"paused","importedFrom":"codex-automation-toml"},"privacy":{"classification":"operational","retention":{"classification":"standard"}}}');
INSERT INTO automation_events VALUES(4,'evt97bf25536b0f418aa93911f9784111a4','automation','weekly-digest',1,'2026-10-02T01:51:23.577Z',1790905883577,'2026-10-02T01:51:23.577Z','{"schemaVersion":"coven.automations.v1","eventId":"evt97bf25536b0f418aa93911f9784111a4","stream":{"kind":"automation","id":"weekly-digest"},"sequence":1,"recordedAt":"2026-10-02T01:51:23.577Z","observedAt":"2026-10-02T01:51:23.577Z","producer":{"component":"coven-daemon","instanceId":"local-authority"},"causation":{"adoptionKey":"legacy:c8d365f64a064eac8e7bcc00db4c935b"},"automationId":"weekly-digest","kind":"definition.revised","summary":"automation weekly-digest paused at revision 2","payload":{"revision":2,"definitionDigest":{"algorithm":"sha256","canonicalization":"jcs-rfc8785","value":"e3c9b82bcd70ba2f1391383715a1a5d1ae4a51f576e68347cc7dd9dfafc9c2a6"},"lifecycleState":"paused"},"privacy":{"classification":"operational","retention":{"classification":"standard"}}}');
INSERT INTO automation_events VALUES(5,'evt5c53e492cb66497c9f2411878ee6bb0d','automation','nightly-review',1,'2026-10-02T01:51:23.603Z',1790905883603,'2026-10-02T01:51:23.603Z','{"schemaVersion":"coven.automations.v1","eventId":"evt5c53e492cb66497c9f2411878ee6bb0d","stream":{"kind":"automation","id":"nightly-review"},"sequence":1,"recordedAt":"2026-10-02T01:51:23.603Z","observedAt":"2026-10-02T01:51:23.603Z","producer":{"component":"coven-daemon","instanceId":"local-authority"},"causation":{"adoptionKey":"legacy:0498104d3bf942ffb86cdfca2435ca69"},"automationId":"nightly-review","kind":"definition.revised","summary":"automation nightly-review active at revision 2","payload":{"revision":2,"definitionDigest":{"algorithm":"sha256","canonicalization":"jcs-rfc8785","value":"984bc39689b3348b78c228b93a2f85871dc00813e7b04e7e1196e61f8e46bb7f"},"lifecycleState":"active"},"privacy":{"classification":"operational","retention":{"classification":"standard"}}}');
CREATE TABLE automation_event_checkpoints (
        checkpoint TEXT PRIMARY KEY NOT NULL,
        stream_kind TEXT NOT NULL
            CHECK (stream_kind IN ('automation', 'occurrence', 'run', 'feed')),
        stream_id TEXT NOT NULL,
        after_sequence INTEGER NOT NULL CHECK (after_sequence >= -1),
        issued_at TEXT NOT NULL,
        expires_at TEXT NOT NULL
    );
CREATE TABLE automation_event_migrations (
        name TEXT PRIMARY KEY NOT NULL,
        completed_at TEXT NOT NULL
    );
INSERT INTO automation_event_migrations VALUES('definition-import-baseline-v1','2026-10-02T01:51:23.015Z');
CREATE TABLE automation_receipts (
        id TEXT PRIMARY KEY NOT NULL,
        run_id TEXT UNIQUE NOT NULL,
        attempt_id TEXT UNIQUE NOT NULL,
        automation_id TEXT NOT NULL,
        automation_revision INTEGER NOT NULL CHECK (automation_revision >= 1),
        definition_digest TEXT,
        occurrence_id TEXT NOT NULL,
        occurrence_fence_generation INTEGER NOT NULL
            CHECK (occurrence_fence_generation >= 1),
        attempt_number INTEGER NOT NULL CHECK (attempt_number BETWEEN 1 AND 10),
        outcome TEXT NOT NULL CHECK (
            outcome IN ('succeeded', 'failed', 'cancelled', 'timed_out', 'ambiguous')
        ),
        receipt_digest TEXT NOT NULL CHECK (
            length(receipt_digest) = 64
            AND receipt_digest NOT GLOB '*[^0-9a-f]*'
        ),
        receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
        event_id TEXT UNIQUE NOT NULL,
        produced_at TEXT NOT NULL,
        FOREIGN KEY (run_id) REFERENCES automation_runs(id) ON DELETE RESTRICT,
        FOREIGN KEY (attempt_id) REFERENCES automation_attempts(id) ON DELETE RESTRICT,
        FOREIGN KEY (occurrence_id) REFERENCES automation_occurrences(id) ON DELETE RESTRICT
    );
CREATE TABLE automation_receipt_authority_extensions (
        receipt_id TEXT PRIMARY KEY NOT NULL,
        run_id TEXT UNIQUE NOT NULL,
        attempt_id TEXT UNIQUE NOT NULL,
        binding_id TEXT NOT NULL,
        binding_digest TEXT NOT NULL CHECK (
            length(binding_digest) = 64
            AND binding_digest NOT GLOB '*[^0-9a-f]*'
        ),
        base_receipt_digest TEXT NOT NULL CHECK (
            length(base_receipt_digest) = 64
            AND base_receipt_digest NOT GLOB '*[^0-9a-f]*'
        ),
        terminal_evidence_digest TEXT NOT NULL CHECK (
            length(terminal_evidence_digest) = 64
            AND terminal_evidence_digest NOT GLOB '*[^0-9a-f]*'
        ),
        authority_json TEXT NOT NULL CHECK (json_valid(authority_json)),
        produced_at TEXT NOT NULL,
        FOREIGN KEY (receipt_id) REFERENCES automation_receipts(id) ON DELETE RESTRICT,
        FOREIGN KEY (run_id) REFERENCES automation_runs(id) ON DELETE RESTRICT,
        FOREIGN KEY (attempt_id) REFERENCES automation_attempts(id) ON DELETE RESTRICT
    );
CREATE TABLE automation_timezone_migrations (
        automation_id TEXT NOT NULL,
        from_revision INTEGER NOT NULL,
        to_revision INTEGER NOT NULL,
        from_timezone TEXT NOT NULL,
        to_timezone TEXT NOT NULL,
        previous_definition_json TEXT NOT NULL,
        previous_definition_digest TEXT,
        definition_digest TEXT NOT NULL,
        migrated_at TEXT NOT NULL,
        PRIMARY KEY (automation_id, from_revision)
    );
INSERT INTO sqlite_sequence VALUES('executor_result_envelopes',0);
INSERT INTO sqlite_sequence VALUES('automation_events',5);
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
CREATE TRIGGER automation_runtime_terminal_evidence_no_update
    BEFORE UPDATE ON automation_runtime_terminal_evidence
    BEGIN
        SELECT RAISE(ABORT, 'automation runtime terminal evidence is immutable');
    END;
CREATE TRIGGER automation_runtime_terminal_evidence_no_delete
    BEFORE DELETE ON automation_runtime_terminal_evidence
    BEGIN
        SELECT RAISE(ABORT, 'automation runtime terminal evidence is immutable');
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
CREATE TRIGGER automation_command_adoptions_immutable_columns
    BEFORE UPDATE OF
        adoption_key,
        request_digest,
        command,
        automation_id,
        outcome,
        revision,
        adopted_at
    ON automation_command_adoptions
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;
CREATE TRIGGER automation_command_adoptions_immutable_rowid
    BEFORE UPDATE ON automation_command_adoptions
    WHEN NEW.rowid IS NOT OLD.rowid
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;
CREATE TRIGGER automation_command_adoptions_response_update_guard
    BEFORE UPDATE OF response_json ON automation_command_adoptions
    WHEN NOT (
        OLD.command IN ('definition.create.v1', 'definition.revise.v1')
        AND OLD.outcome = 'rejected'
        AND json_valid(OLD.response_json)
        AND json_extract(OLD.response_json, '$.outcome') IS 'rejected'
        AND json_extract(OLD.response_json, '$.error.code') IS 'VALIDATION_FAILED'
        AND json_valid(NEW.response_json)
        AND json_type(NEW.response_json, '$') IS 'object'
        AND json_extract(NEW.response_json, '$.outcome') IS 'rejected'
        AND json_type(NEW.response_json, '$.error') IS 'object'
        AND json_extract(NEW.response_json, '$.error.code') IS 'VALIDATION_FAILED'
        AND json_extract(NEW.response_json, '$.error.httpStatus') IS 400
        AND json_extract(NEW.response_json, '$.error.message')
            IS 'automation definition failed validation'
        AND json_extract(NEW.response_json, '$.error.retryable') IS 0
        AND json_extract(NEW.response_json, '$.error.currentRevision')
            IS json_extract(OLD.response_json, '$.error.currentRevision')
        AND NOT EXISTS (
            SELECT 1
            FROM json_each(NEW.response_json)
            WHERE key NOT IN ('outcome', 'error')
        )
        AND NOT EXISTS (
            SELECT 1
            FROM json_each(NEW.response_json, '$.error')
            WHERE key NOT IN (
                'code',
                'httpStatus',
                'message',
                'retryable',
                'currentRevision'
            )
        )
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;
CREATE TRIGGER automation_command_adoptions_no_delete
    BEFORE DELETE ON automation_command_adoptions
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;
CREATE TRIGGER automation_attempts_terminal_immutable
    BEFORE UPDATE ON automation_attempts
    WHEN OLD.state IN ('succeeded', 'failed', 'cancelled', 'timed_out', 'ambiguous')
    BEGIN
        SELECT RAISE(ABORT, 'terminal automation attempt is immutable');
    END;
CREATE TRIGGER automation_attempts_delete_terminal_refused
    BEFORE DELETE ON automation_attempts
    WHEN OLD.state IN ('succeeded', 'failed', 'cancelled', 'timed_out', 'ambiguous')
    BEGIN
        SELECT RAISE(ABORT, 'terminal automation attempt cannot be deleted');
    END;
CREATE TRIGGER automation_run_authority_profile_immutable
         BEFORE UPDATE OF authority_profile ON automation_runs
         WHEN OLD.authority_profile IS NOT NEW.authority_profile
         BEGIN
             SELECT RAISE(ABORT, 'automation run authority profile is immutable');
         END;
CREATE TRIGGER automation_attempt_authority_immutable
         BEFORE UPDATE OF authority_extension_json ON automation_attempts
         WHEN OLD.authority_extension_json IS NOT NULL
              AND OLD.authority_extension_json IS NOT NEW.authority_extension_json
         BEGIN
             SELECT RAISE(ABORT, 'automation attempt authority is immutable');
         END;
CREATE TRIGGER automation_attempt_authority_delete_refused
         BEFORE DELETE ON automation_attempts
         WHEN OLD.authority_extension_json IS NOT NULL
         BEGIN
             SELECT RAISE(ABORT, 'authority-bound automation attempt cannot be deleted');
         END;
CREATE TRIGGER automation_attempt_adoption_key_global_insert
    BEFORE INSERT ON automation_attempts
    WHEN EXISTS (
        SELECT 1 FROM automation_command_reservations
        WHERE adoption_key = NEW.adoption_key
    ) OR EXISTS (
        SELECT 1 FROM automation_command_adoptions
        WHERE adoption_key = NEW.adoption_key
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation adoption key is already used by a command');
    END;
CREATE TRIGGER automation_command_reservation_key_global_insert
    BEFORE INSERT ON automation_command_reservations
    WHEN EXISTS (
        SELECT 1 FROM automation_attempts
        WHERE adoption_key = NEW.adoption_key
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation adoption key is already used by an attempt');
    END;
CREATE TRIGGER automation_command_adoption_key_global_insert
    BEFORE INSERT ON automation_command_adoptions
    WHEN EXISTS (
        SELECT 1 FROM automation_attempts
        WHERE adoption_key = NEW.adoption_key
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation adoption key is already used by an attempt');
    END;
CREATE TRIGGER automation_events_no_update
    BEFORE UPDATE ON automation_events
    BEGIN
        SELECT RAISE(ABORT, 'automation events are append-only');
    END;
CREATE TRIGGER automation_events_no_delete
    BEFORE DELETE ON automation_events
    BEGIN
        SELECT RAISE(ABORT, 'automation events are append-only');
    END;
CREATE TRIGGER automation_receipts_no_update
    BEFORE UPDATE ON automation_receipts
    BEGIN
        SELECT RAISE(ABORT, 'automation receipts are immutable');
    END;
CREATE TRIGGER automation_receipts_no_delete
    BEFORE DELETE ON automation_receipts
    BEGIN
        SELECT RAISE(ABORT, 'automation receipts are immutable');
    END;
CREATE TRIGGER automation_run_receipt_once
    BEFORE UPDATE OF receipt_id ON automation_runs
    WHEN OLD.receipt_id IS NOT NEW.receipt_id
         AND (
             OLD.receipt_id IS NOT NULL
             OR NEW.receipt_id IS NULL
             OR NOT EXISTS (
                 SELECT 1
                 FROM automation_receipts
                 WHERE id = NEW.receipt_id AND run_id = OLD.id
             )
         )
    BEGIN
        SELECT RAISE(ABORT, 'automation run receipt is immutable');
    END;
CREATE TRIGGER automation_receipt_authority_extensions_no_update
    BEFORE UPDATE ON automation_receipt_authority_extensions
    BEGIN
        SELECT RAISE(ABORT, 'automation receipt authority extensions are immutable');
    END;
CREATE TRIGGER automation_receipt_authority_extensions_no_delete
    BEFORE DELETE ON automation_receipt_authority_extensions
    BEGIN
        SELECT RAISE(ABORT, 'automation receipt authority extensions are immutable');
    END;
CREATE INDEX ward_audit_familiar_idx ON ward_audit (familiar_id, recorded_at);
CREATE INDEX ward_audit_event_idx    ON ward_audit (event_type, recorded_at);
CREATE INDEX idx_automation_runtime_terminal_evidence_run
        ON automation_runtime_terminal_evidence(run_id);
CREATE UNIQUE INDEX idx_automation_runtime_terminal_evidence_binding
        ON automation_runtime_terminal_evidence(binding_id);
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
CREATE INDEX idx_automation_command_adoptions_automation
        ON automation_command_adoptions(automation_id, adopted_at);
CREATE INDEX idx_automation_occurrences_scheduled
        ON automation_occurrences(automation_id, scheduled_for);
CREATE INDEX idx_automation_occurrences_state
        ON automation_occurrences(state, lease_expires_at);
CREATE INDEX idx_automation_definitions_planning
        ON automation_definitions(name, id)
        WHERE tombstoned_at IS NULL;
CREATE INDEX idx_automation_runs_automation_started
        ON automation_runs(automation_id, started_at DESC);
CREATE INDEX idx_automation_attempts_dispatch
        ON automation_attempts(state, not_before);
CREATE INDEX idx_automation_events_recorded
        ON automation_events(recorded_at_millis, feed_position);
CREATE INDEX idx_automation_event_checkpoints_expiry
        ON automation_event_checkpoints(expires_at);
PRAGMA writable_schema=OFF;
COMMIT;
