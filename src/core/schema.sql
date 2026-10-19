-- media_items 主表
CREATE TABLE IF NOT EXISTS media_items (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uri             TEXT    UNIQUE NOT NULL,
    path            TEXT    NOT NULL,
    folder_path     TEXT    NOT NULL,
    mime_type       TEXT    NOT NULL,
    media_kind      TEXT    NOT NULL DEFAULT 'image',
    media_subkind   TEXT    NOT NULL DEFAULT 'standard',
    media_attributes TEXT   NOT NULL DEFAULT '{}',
    media_type_flags INTEGER NOT NULL DEFAULT 0,
    width           INTEGER,
    height          INTEGER,
    video_duration_secs REAL,
    taken_at        INTEGER,
    file_mtime      INTEGER NOT NULL,
    file_mtime_ns   INTEGER NOT NULL DEFAULT 0,
    file_size       INTEGER NOT NULL,
    blake3_hash     TEXT    NOT NULL,
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    trashed_at      INTEGER,
    indexed_at      INTEGER NOT NULL,
    thumbnail_generated_at INTEGER,
    storage_location INTEGER NOT NULL DEFAULT 1 CHECK (storage_location IN (1,2,3)),
    thumbnail_ready INTEGER NOT NULL DEFAULT 0 CHECK (thumbnail_ready IN (0,1))
);

CREATE INDEX IF NOT EXISTS idx_media_taken_at
    ON media_items(taken_at DESC) WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_live_sort
    ON media_items(COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_folder
    ON media_items(folder_path)    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_folder_sort
    ON media_items(folder_path, COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL;
-- `albums::refresh` selects the newest file-mtime cover for every folder.
-- Keep that correlated lookup ordered by the index, rather than building a
-- temporary sort for each folder during a full materialized-view refresh.
CREATE INDEX IF NOT EXISTS idx_media_folder_mtime
    ON media_items(folder_path, file_mtime DESC)
    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_trashed
    ON media_items(trashed_at)     WHERE trashed_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_media_blake3
    ON media_items(blake3_hash);
-- Native sync joins its per-job relative paths back to the physical media row.
-- Without this index each receive reconciliation scans the complete library.
CREATE INDEX IF NOT EXISTS idx_media_path
    ON media_items(path);

CREATE INDEX IF NOT EXISTS idx_media_favorite
    ON media_items(is_favorite)
    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_favorite_sort
    ON media_items(is_favorite, COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_media_kind
    ON media_items(media_kind)
    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_kind_sort
    ON media_items(media_kind, COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_media_subkind
    ON media_items(media_subkind)
    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_subkind_sort
    ON media_items(media_subkind, COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL;

-- Logical media-type albums use a bitset because categories can overlap.
-- Keep one partial sort index per currently exposed flag so sidebar counts,
-- covers and album pages never scan the whole live library.
CREATE INDEX IF NOT EXISTS idx_media_type_motion_photo_sort
    ON media_items(COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL AND (media_type_flags & 1) != 0;
CREATE INDEX IF NOT EXISTS idx_media_type_animated_sort
    ON media_items(COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL AND (media_type_flags & 2) != 0;
CREATE INDEX IF NOT EXISTS idx_media_type_hdr_sort
    ON media_items(COALESCE(taken_at, file_mtime) DESC, id DESC)
    WHERE trashed_at IS NULL AND (media_type_flags & 4) != 0;

-- albums 物化视图
CREATE TABLE IF NOT EXISTS albums (
    folder_path     TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    cover_uri       TEXT,
    photo_count     INTEGER NOT NULL DEFAULT 0,
    last_modified   INTEGER NOT NULL
);

-- album_order 持久化用户在侧栏中拖动重排相册得到的顺序。
-- 单独成表而非加列到 `albums`：`albums` 在每次扫描 / `albums::refresh` 时被
-- DELETE + 重新 INSERT，独立表才不会被这轮重建清掉。键是 `folder_path`
-- （对虚拟相册则是其魔法路径），因此虚拟相册同样可被拖动排序。
CREATE TABLE IF NOT EXISTS album_order (
    folder_path     TEXT PRIMARY KEY,
    sort_order      INTEGER NOT NULL
);

-- album_covers 持久化用户手动指定的相册封面。
-- 单独成表而非写入 albums.cover_uri：albums 是物化视图，每次刷新会重建。
CREATE TABLE IF NOT EXISTS album_covers (
    folder_path     TEXT PRIMARY KEY,
    cover_uri       TEXT NOT NULL
);

-- edits 非破坏性编辑记录
CREATE TABLE IF NOT EXISTS edits (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id        INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    edit_type       TEXT    NOT NULL,
    params          TEXT    NOT NULL,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_edits_media ON edits(media_id);

-- settings
CREATE TABLE IF NOT EXISTS settings (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);

-- Native SyncServer connection state. Credentials are referenced from the
-- platform secret store and never persisted in SQLite: a Bearer API key is the
-- only client credential, and its reference lives in `credential_ref`.
--
-- Accepted identity, capabilities and limits are scalar columns. A stable
-- account FK groups connections without a serialized identity document.
CREATE TABLE IF NOT EXISTS sync_connections (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_kind   TEXT NOT NULL DEFAULT 'native_syncserver'
                    CHECK (provider_kind = 'native_syncserver'),
    endpoint        TEXT NOT NULL,
    credential_ref  TEXT NOT NULL UNIQUE,
    enabled         INTEGER NOT NULL DEFAULT 1,
    native_public_base_url    TEXT,
    native_protocol_version   INTEGER,
    native_service_version    TEXT,
    native_stage              TEXT,
    native_checksum_algorithm TEXT,
    native_tenant       TEXT,
    native_container    TEXT,
    native_environment  TEXT,
    native_database     TEXT,
    native_database_generation TEXT,
    native_account_id INTEGER REFERENCES sync_accounts(id) ON DELETE SET NULL,
    native_pinned INTEGER NOT NULL DEFAULT 0,
    native_auth_mode TEXT,
    native_user_id TEXT,
    native_username TEXT,
    native_resource_bearer_required INTEGER,
    native_cap_zone_listing INTEGER,
    native_cap_record_crud INTEGER,
    native_cap_snapshots INTEGER,
    native_cap_incremental_changes INTEGER,
    native_cap_upload_leases INTEGER,
    native_cap_fencing INTEGER,
    native_cap_token_current_gate INTEGER,
    native_cap_upload_phases INTEGER,
    native_cap_operation_status_read INTEGER,
    native_cap_asset_upload INTEGER,
    native_cap_asset_download INTEGER,
    native_cap_photo_publication INTEGER,
    native_cap_query_batch INTEGER,
    native_cap_change_aliases INTEGER,
    native_cap_apple_authentication INTEGER,
    native_cap_sharing INTEGER,
    native_cap_upload_resume INTEGER,
    native_limit_max_asset_bytes INTEGER,
    native_limit_thumbnail_max_bytes INTEGER,
    native_limit_upload_concurrency INTEGER,
    native_limit_download_concurrency INTEGER,
    native_limit_upload_idle_timeout_secs INTEGER,
    native_limit_upload_total_timeout_secs INTEGER,
    native_limit_db_queue_capacity INTEGER,
    native_limit_json_body_limit INTEGER,
    native_limit_lease_ttl_secs INTEGER,
    native_limit_lease_heartbeat_secs INTEGER,
    native_limit_control_queue_capacity INTEGER,
    native_sync_limit_max_page INTEGER,
    native_sync_limit_max_zones INTEGER,
    native_record_limit_max_operations INTEGER,
    native_record_limit_max_record_bytes INTEGER,
    native_record_limit_max_result_bytes INTEGER,
    native_probed_at    INTEGER,
    native_probe_error  TEXT,
    created_at       INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at       INTEGER NOT NULL DEFAULT (unixepoch())
);

-- One native job maps one local root to one zone. The zone is persisted
-- explicitly; it is never smuggled through a path column. A zone has one
-- effective uplink device per server, which the UNIQUE constraint enforces
-- alongside the application-level check that names the offending task.
CREATE TABLE IF NOT EXISTS sync_jobs (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    connection_id     INTEGER NOT NULL REFERENCES sync_connections(id) ON DELETE CASCADE,
    local_root        TEXT NOT NULL,
    direction         TEXT NOT NULL DEFAULT 'bidirectional',
    upload_scope      TEXT NOT NULL DEFAULT 'selected_albums',
    propagate_deletes INTEGER NOT NULL DEFAULT 0,
    native_cycle_id TEXT,
    paused            INTEGER NOT NULL DEFAULT 0,
    config_generation INTEGER NOT NULL DEFAULT 1,
    native_zone_name  TEXT NOT NULL CHECK (native_zone_name <> ''),
    native_owner      TEXT NOT NULL DEFAULT '_defaultOwner' CHECK (native_owner <> ''),
    last_started_at   INTEGER,
    last_completed_at INTEGER,
    last_error        TEXT,
    UNIQUE(connection_id, local_root),
    UNIQUE(connection_id, native_zone_name)
);

CREATE TABLE IF NOT EXISTS sync_job_upload_albums (
    job_id          INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    relative_album  TEXT NOT NULL,
    PRIMARY KEY(job_id, relative_album)
);

-- One synchronized path per job. Native dirty truth lives on this row as
-- fields, never in a second queue: 0=CLEAN, 1=NEW, 2=MODIFIED, 4=DELETE. `media_id` is a presentation association only; `sync_entries.id` is
-- the stable identity through media-index rebuilds.
CREATE TABLE IF NOT EXISTS sync_entries (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id               INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    media_id             INTEGER REFERENCES media_items(id) ON DELETE SET NULL,
    relative_path        TEXT NOT NULL,
    entry_generation     INTEGER NOT NULL DEFAULT 1,
    native_retired         INTEGER NOT NULL DEFAULT 0 CHECK (native_retired IN (0,1)),
    native_dirty           INTEGER NOT NULL DEFAULT 0 CHECK (native_dirty IN (0, 1, 2, 4)),
    -- Diagnostic/dispatch guard for one upload cycle; Begin clears it per job.
    native_upload_error    TEXT,
    native_local_version   INTEGER NOT NULL DEFAULT 0,
    native_metadata_version INTEGER NOT NULL DEFAULT 0,
    native_content_version INTEGER NOT NULL DEFAULT 0,
    native_deletion_version INTEGER NOT NULL DEFAULT 0,
    native_acked_metadata_version INTEGER NOT NULL DEFAULT 0,
    native_acked_content_version INTEGER NOT NULL DEFAULT 0,
    native_acked_deletion_version INTEGER NOT NULL DEFAULT 0,
    -- Independent edit events. Absent stays NULL: a missing timestamp must not
    -- become an invented edit time.
    native_metadata_edited_ms INTEGER,
    native_metadata_source TEXT,
    native_metadata_event_id TEXT,
    native_content_edited_ms INTEGER,
    native_content_source TEXT,
    native_content_event_id TEXT,
    native_deletion_edited_ms INTEGER,
    native_deletion_source TEXT,
    native_deletion_event_id TEXT,
    native_digest_entry_id INTEGER,
    native_digest_version INTEGER,
    native_digest_sha256 TEXT,
    native_digest_size INTEGER,
    -- Stable remote identity. Never a path or a media_items id; an allocated id
    -- does not by itself prove a create happened.
    native_photo_id  TEXT,
    native_master_id TEXT,
    native_asset_revision TEXT,
    native_master_revision TEXT,
    native_confirmed_generation TEXT,
    native_content_generation TEXT,
    native_has_baseline INTEGER NOT NULL DEFAULT 0,
    native_has_pending_master INTEGER NOT NULL DEFAULT 0,
    native_has_pending_asset INTEGER NOT NULL DEFAULT 0,
    -- Versioned local digest, {entryId, localVersion, digest: {size, hex, base64}}.
    -- Valid only for that entry/version; local edits clear it. It exists
    -- so identity adoption hashes each file once, not once per receive cycle.
    updated_at           INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(job_id, relative_path)
);

CREATE INDEX IF NOT EXISTS idx_sync_entries_media ON sync_entries(media_id,job_id);

-- Issued native operation evidence: the intent is written immediately before
-- an uncertain network mutation, including its captured versions and the exact
-- logical body, and recovered before a new operation is derived for the same
-- photo. This is recovery evidence, not a dirty source of truth.
CREATE TABLE IF NOT EXISTS sync_tasks (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id       TEXT NOT NULL UNIQUE,
    job_id             INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    entry_id           INTEGER REFERENCES sync_entries(id) ON DELETE SET NULL,
    action             TEXT NOT NULL,
    state              TEXT NOT NULL,
    config_generation  INTEGER NOT NULL,
    entry_generation   INTEGER NOT NULL,
    capture_present INTEGER NOT NULL DEFAULT 0,
    capture_job INTEGER,
    capture_entry INTEGER,
    capture_config INTEGER,
    capture_local_version INTEGER,
    capture_metadata_version INTEGER,
    capture_content_version INTEGER,
    capture_deletion_version INTEGER,
    capture_dirty INTEGER,
    body_present INTEGER,
    body_zone_kind INTEGER,
    body_atomic_present INTEGER,
    body_atomic INTEGER,
    body_operations_kind INTEGER,
    prep_present INTEGER,
    prep_metadata_kind INTEGER,
    prep_events_kind INTEGER,
    prep_resources_kind INTEGER,
    prep_send_present INTEGER,
    prep_send_metadata INTEGER,
    prep_asset_revision_present INTEGER,
    prep_master_revision_present INTEGER,
    capture_generation TEXT,
    capture_phase TEXT,
    capture_photo TEXT,
    capture_master TEXT,
    capture_content_generation TEXT,
    body_zone_name TEXT,
    body_zone_owner TEXT,
    prep_asset_revision TEXT,
    prep_master_revision TEXT,
    retry_count        INTEGER NOT NULL DEFAULT 0,
    retry_at           INTEGER,
    last_error         TEXT,
    created_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at         INTEGER NOT NULL DEFAULT (unixepoch())
);

-- A conflict the user must resolve. Native candidates carry their category and
-- competing structured record versions; the recovery path is an ordinary
-- column rather than a second dirty source.
CREATE TABLE IF NOT EXISTS sync_conflicts (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id             INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    entry_id           INTEGER NOT NULL REFERENCES sync_entries(id) ON DELETE CASCADE,
    kind               TEXT NOT NULL,
    state              TEXT NOT NULL DEFAULT 'open',
    resolution         TEXT,
    native_detail_present INTEGER NOT NULL DEFAULT 0,
    native_reason TEXT,
    native_recovery_path TEXT,
    native_remote_present INTEGER NOT NULL DEFAULT 0,
    created_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    resolved_at        INTEGER
);

-- Native receive state. These tables are not local-change truth: they record
-- what the cloud has said and what this client still owes it. Every one is
-- scoped to a job plus a namespace generation, so a rebuilt server database
-- starts from clean progress instead of reusing stale tokens.

-- Snapshot/bootstrap, delta ingest, applied and target tokens, and round state
-- for one job's configured zone. Configured download scope stays distinct from
-- database discovery here.
CREATE TABLE IF NOT EXISTS sync_native_progress (
    job_id              INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    zone_name           TEXT NOT NULL,
    owner               TEXT NOT NULL,
    tenant              TEXT NOT NULL,
    container           TEXT NOT NULL,
    environment         TEXT NOT NULL,
    database            TEXT NOT NULL,
    database_generation TEXT NOT NULL,
    -- Digest of the configured upload/download scope, so changing the scope
    -- invalidates the previous completeness claim instead of inheriting it.
    scope_digest        TEXT NOT NULL,
    snapshot_token      TEXT,
    -- Independent per-record-type bootstrap markers. One type or one page is
    -- never a complete inventory.
    bootstrap_started INTEGER NOT NULL DEFAULT 0,
    discovery_token     TEXT,
    discovery_complete  INTEGER NOT NULL DEFAULT 0 CHECK (discovery_complete IN (0, 1)),
    ingest_token        TEXT,
    applied_token       TEXT,
    target_token        TEXT,
    updated_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (job_id, zone_name, owner)
);

-- Role-specific resource expectations and owned staging progress. Never holds
-- a lease token or an expiring URL as identity.
CREATE TABLE IF NOT EXISTS sync_native_resources (
    job_id           INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    generation       TEXT NOT NULL,
    asset_id         TEXT NOT NULL,
    role             TEXT NOT NULL CHECK (role IN ('original', 'thumbnail')),
    record_name      TEXT,
    expected_sha256  TEXT,
    expected_size    INTEGER,
    staging_path     TEXT,
    bytes_done       INTEGER NOT NULL DEFAULT 0,
    state            TEXT NOT NULL DEFAULT 'pending',
    verified_cache   TEXT,
    updated_at       INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (job_id, generation, asset_id, role)
);

-- Owned pair/file/cache publication intent, captured local versions and the
-- last complete snapshot. Distinct from the server's upload lease and from the
-- dirty flags: this is how an interrupted publish settles or is restored.
CREATE TABLE IF NOT EXISTS sync_publications (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    publication_id    TEXT NOT NULL UNIQUE,
    job_id            INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    entry_id          INTEGER REFERENCES sync_entries(id) ON DELETE SET NULL,
    photo_id          TEXT,
    master_id         TEXT,
    original_asset_id TEXT,
    thumbnail_asset_id TEXT,
    state             TEXT NOT NULL DEFAULT 'pending',
    capture_present INTEGER NOT NULL DEFAULT 0,
    capture_projection_present INTEGER NOT NULL DEFAULT 0,
    recovery_present INTEGER NOT NULL DEFAULT 0,
    recovery_old_snapshot_path TEXT,
    capture_job INTEGER,
    capture_entry INTEGER,
    capture_local_version INTEGER,
    capture_config INTEGER,
    capture_dirty INTEGER,
    capture_photo TEXT,
    capture_content_generation TEXT,
    capture_preserve_metadata INTEGER,
    capture_local_path TEXT,
    capture_favorite INTEGER,
    capture_taken_at INTEGER,
    recovery_collision INTEGER,
    recovery_source TEXT,
    recovery_preserved TEXT,
    recovery_root TEXT,
    recovery_target TEXT,
    recovery_new TEXT,
    recovery_backup TEXT,
    recovery_old_path TEXT,
    recovery_sha256 TEXT,
    recovery_size INTEGER,
    recovery_had_old INTEGER,
    recovery_stamp_0 INTEGER,
    recovery_stamp_1 INTEGER,
    recovery_stamp_2 INTEGER,
    recovery_stamp_3 INTEGER,
    recovery_stamp_4 INTEGER,
    recovery_old_stamp_0 INTEGER,
    recovery_old_stamp_1 INTEGER,
    recovery_old_local_stamp_0 INTEGER,
    recovery_old_local_stamp_1 INTEGER,
    original_staging  TEXT,
    thumbnail_staging TEXT,
    created_at        INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at        INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_sync_entries_native_dirty
    ON sync_entries(job_id, native_dirty, id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_entries_native_photo
    ON sync_entries(job_id, native_photo_id) WHERE native_photo_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sync_tasks_ready
    ON sync_tasks(state, retry_at, job_id);
CREATE INDEX IF NOT EXISTS idx_sync_conflicts_open
    ON sync_conflicts(job_id, state) WHERE state = 'open';
CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_conflicts_one_open_per_entry
    ON sync_conflicts(entry_id) WHERE state = 'open';
CREATE INDEX IF NOT EXISTS idx_sync_native_resources_job_state
    ON sync_native_resources(job_id, state);
CREATE INDEX IF NOT EXISTS idx_sync_publications_job_state
    ON sync_publications(job_id, state);
-- The overview probes readiness per entry. Keep this lookup bounded instead
-- of scanning all publications once for every photo on each Settings refresh.
CREATE INDEX IF NOT EXISTS idx_sync_publications_entry_state
    ON sync_publications(entry_id, state);

-- Accepted configuration edits must survive exit without mutating in-flight roots.
CREATE TABLE IF NOT EXISTS sync_job_changes (
    job_id INTEGER PRIMARY KEY REFERENCES sync_jobs(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL DEFAULT 0,
    desired_direction TEXT,
    albums_changed INTEGER NOT NULL DEFAULT 0,
    delete_requested INTEGER NOT NULL DEFAULT 0
);

-- Presentation uses declared file/thumbnail availability, never path probes.
CREATE VIEW IF NOT EXISTS visible_media AS
SELECT m.* FROM media_items m
WHERE m.storage_location<>2 OR (m.thumbnail_ready=1 AND EXISTS(
    SELECT 1 FROM sync_entries e JOIN sync_jobs j ON j.id=e.job_id
    JOIN sync_connections c ON c.id=j.connection_id
    WHERE e.media_id=m.id AND c.enabled=1 AND e.native_dirty<>4
));


CREATE INDEX IF NOT EXISTS idx_sync_entries_native_master ON sync_entries(job_id,native_master_id) WHERE native_master_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sync_publications_photo ON sync_publications(job_id,photo_id,state);
CREATE TRIGGER IF NOT EXISTS sync_entry_records_delete AFTER DELETE ON sync_entries
BEGIN DELETE FROM sync_records WHERE owner_kind IN ('pending','baseline','baseline_metadata','desired_metadata') AND owner_key=CAST(OLD.id AS TEXT); END;
CREATE INDEX IF NOT EXISTS idx_sync_connections_identity ON sync_connections(endpoint,native_tenant,native_user_id);
CREATE TRIGGER IF NOT EXISTS sync_publication_records_delete AFTER DELETE ON sync_publications
BEGIN DELETE FROM sync_records WHERE owner_kind='publication' AND owner_key=OLD.publication_id; END;
CREATE TRIGGER IF NOT EXISTS sync_conflict_records_delete AFTER DELETE ON sync_conflicts
BEGIN DELETE FROM sync_records WHERE owner_kind='conflict' AND owner_key=CAST(OLD.id AS TEXT); END;
CREATE INDEX IF NOT EXISTS idx_sync_tasks_photo ON sync_tasks(job_id,capture_photo);
CREATE TRIGGER IF NOT EXISTS sync_task_records_delete AFTER DELETE ON sync_tasks
BEGIN DELETE FROM sync_records WHERE (owner_kind IN ('issued','preparation') AND owner_key=OLD.operation_id) OR (owner_kind='prepared_asset' AND owner_key IN (OLD.operation_id||':original',OLD.operation_id||':thumbnail')); END;
CREATE INDEX IF NOT EXISTS idx_sync_connections_account ON sync_connections(native_account_id);
CREATE TRIGGER IF NOT EXISTS sync_connection_account_delete AFTER DELETE ON sync_connections
BEGIN DELETE FROM sync_accounts WHERE id=OLD.native_account_id AND NOT EXISTS(SELECT 1 FROM sync_connections WHERE native_account_id=OLD.native_account_id); END;
-- Typed native record fragments, baseline pairs and issued request records.
-- Records are scoped by their owning state, never a second cloud inventory.
CREATE TABLE IF NOT EXISTS sync_records (
    id INTEGER PRIMARY KEY,
    owner_kind TEXT NOT NULL,
    owner_key TEXT NOT NULL,
    job_id INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    entry_id INTEGER REFERENCES sync_entries(id) ON DELETE SET NULL,
    position INTEGER NOT NULL,
    operation_type TEXT,
    record_name TEXT NOT NULL,
    record_type TEXT,
    record_revision TEXT,
    photo_id TEXT,
    master_id TEXT,
    relative_path TEXT,
    content_generation TEXT,
    zone_name TEXT,
    zone_owner TEXT,
    deleted INTEGER NOT NULL DEFAULT 0,
    record_shape INTEGER NOT NULL DEFAULT 127,
    event_shape INTEGER NOT NULL DEFAULT 7,
    fields_present INTEGER NOT NULL DEFAULT 0,
    events_present INTEGER NOT NULL DEFAULT 0,
    metadata_edited_ms INTEGER,
    metadata_source TEXT,
    metadata_event_id TEXT,
    content_edited_ms INTEGER,
    content_source TEXT,
    content_event_id TEXT,
    deletion_edited_ms INTEGER,
    deletion_source TEXT,
    deletion_event_id TEXT,
    UNIQUE(owner_kind,owner_key,position)
);
CREATE INDEX IF NOT EXISTS idx_sync_records_identity ON sync_records(job_id,owner_kind,record_name);
CREATE INDEX IF NOT EXISTS idx_sync_records_photo ON sync_records(job_id,owner_kind,photo_id);
CREATE INDEX IF NOT EXISTS idx_sync_records_master ON sync_records(job_id,owner_kind,master_id);
CREATE INDEX IF NOT EXISTS idx_sync_records_entry ON sync_records(entry_id,owner_kind);


CREATE TABLE IF NOT EXISTS sync_record_fields (
    record_id INTEGER NOT NULL REFERENCES sync_records(id) ON DELETE CASCADE,
    field_name TEXT NOT NULL,
    field_type TEXT,
    field_present INTEGER NOT NULL,
    value_present INTEGER NOT NULL,
    value_kind TEXT NOT NULL,
    text_value TEXT,
    integer_value INTEGER,
    real_value REAL,
    reference_name TEXT,
    reference_action TEXT,
    asset_id TEXT,
    asset_checksum TEXT,
    asset_size INTEGER,
    asset_mime TEXT,
    asset_width INTEGER,
    asset_height INTEGER,
    checksum_algorithm TEXT,
    PRIMARY KEY(record_id,field_name)
);
CREATE INDEX IF NOT EXISTS idx_sync_record_fields_asset ON sync_record_fields(asset_id) WHERE asset_id IS NOT NULL;



CREATE TABLE IF NOT EXISTS sync_connection_zones (
    connection_id INTEGER NOT NULL REFERENCES sync_connections(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    zone_name TEXT NOT NULL,
    PRIMARY KEY(connection_id,position)
);
CREATE INDEX IF NOT EXISTS idx_sync_connection_zones_name ON sync_connection_zones(connection_id,zone_name);
CREATE TABLE IF NOT EXISTS sync_bootstrap_types (
    job_id INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    record_type TEXT NOT NULL,
    complete INTEGER NOT NULL DEFAULT 0,
    continuation_marker TEXT,
    record_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(job_id,record_type)
);
CREATE TABLE IF NOT EXISTS sync_job_change_albums (
    job_id INTEGER NOT NULL REFERENCES sync_job_changes(job_id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    relative_album TEXT NOT NULL,
    PRIMARY KEY(job_id,position)
);




CREATE TABLE IF NOT EXISTS sync_task_resources (
    task_id INTEGER NOT NULL REFERENCES sync_tasks(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    document_present INTEGER NOT NULL,
    member_mask INTEGER NOT NULL,
    source_path TEXT,
    digest_sha256 TEXT,
    digest_base64 TEXT,
    digest_size INTEGER,
    session_id TEXT,
    asset_kind INTEGER NOT NULL,
    PRIMARY KEY(task_id,role)
);

CREATE TABLE IF NOT EXISTS sync_accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    public_base_url TEXT NOT NULL,
    tenant TEXT NOT NULL,
    user_id TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_accounts_user ON sync_accounts(public_base_url,tenant,user_id) WHERE user_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_accounts_legacy ON sync_accounts(public_base_url,tenant) WHERE user_id IS NULL;

CREATE TABLE IF NOT EXISTS sync_reset_state (
    id INTEGER PRIMARY KEY CHECK(id=1),
    token TEXT NOT NULL,
    last_error TEXT
);
