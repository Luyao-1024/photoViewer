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
    thumbnail_generated_at INTEGER
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
-- `native_pinned_scope` records the exact capability document a successful
-- probe accepted. Later cycles compare against it instead of trusting whatever
-- the server announces at that moment.
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
    native_zones        TEXT CHECK (native_zones IS NULL OR json_valid(native_zones)),
    native_pinned_scope TEXT CHECK (native_pinned_scope IS NULL OR json_valid(native_pinned_scope)),
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

-- Complete discovered sync scope, independent from transfer/baseline state.
CREATE TABLE IF NOT EXISTS sync_inventory (
    job_id               INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    relative_path        TEXT NOT NULL,
    config_generation    INTEGER NOT NULL,
    PRIMARY KEY(job_id, relative_path)
);

-- One synchronized path per job. Native dirty truth lives on this row as
-- fields, never in a second queue: 1=NEW, 2=METADATA, 4=CONTENT, 8=DELETE,
-- 0=CLEAN. `media_id` is a presentation association only; `sync_entries.id` is
-- the stable identity through media-index rebuilds.
CREATE TABLE IF NOT EXISTS sync_entries (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id               INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    media_id             INTEGER REFERENCES media_items(id) ON DELETE SET NULL,
    relative_path        TEXT NOT NULL,
    entry_generation     INTEGER NOT NULL DEFAULT 1,
    native_retired         INTEGER NOT NULL DEFAULT 0 CHECK (native_retired IN (0,1)),
    native_dirty           INTEGER NOT NULL DEFAULT 0 CHECK (native_dirty BETWEEN 0 AND 15),
    native_local_version   INTEGER NOT NULL DEFAULT 0,
    native_metadata_version INTEGER NOT NULL DEFAULT 0,
    native_content_version INTEGER NOT NULL DEFAULT 0,
    native_deletion_version INTEGER NOT NULL DEFAULT 0,
    native_acked_metadata_version INTEGER NOT NULL DEFAULT 0,
    native_acked_content_version INTEGER NOT NULL DEFAULT 0,
    native_acked_deletion_version INTEGER NOT NULL DEFAULT 0,
    -- Independent edit events. Absent stays NULL: a missing timestamp must not
    -- become an invented edit time.
    native_metadata_event TEXT CHECK (native_metadata_event IS NULL OR json_valid(native_metadata_event)),
    native_content_event  TEXT CHECK (native_content_event IS NULL OR json_valid(native_content_event)),
    native_deletion_event TEXT CHECK (native_deletion_event IS NULL OR json_valid(native_deletion_event)),
    -- Stable remote identity. Never a path or a media_items id; an allocated id
    -- does not by itself prove a create happened.
    native_photo_id  TEXT,
    native_master_id TEXT,
    native_asset_revision TEXT,
    native_master_revision TEXT,
    native_confirmed_generation TEXT,
    native_content_generation TEXT,
    native_desired_metadata  TEXT CHECK (native_desired_metadata IS NULL OR json_valid(native_desired_metadata)),
    native_baseline_metadata TEXT CHECK (native_baseline_metadata IS NULL OR json_valid(native_baseline_metadata)),
    native_resource_baseline  TEXT CHECK (native_resource_baseline IS NULL OR json_valid(native_resource_baseline)),
    -- Digest of the local original, {size, hex}. Valid only while the file has
    -- not been marked changed since it was taken; `mark()` clears it. It exists
    -- so identity adoption hashes each file once, not once per receive cycle.
    native_local_digest TEXT CHECK (native_local_digest IS NULL OR json_valid(native_local_digest)),
    updated_at           INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(job_id, relative_path)
);

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
    native_capture     TEXT CHECK (native_capture IS NULL OR json_valid(native_capture)),
    retry_count        INTEGER NOT NULL DEFAULT 0,
    retry_at           INTEGER,
    last_error         TEXT,
    created_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at         INTEGER NOT NULL DEFAULT (unixepoch())
);

-- A conflict the user must resolve. Native candidates carry their category and
-- both competing event stamps in `native_detail`; the losing copy's ownership
-- is recorded there rather than as a second dirty truth.
CREATE TABLE IF NOT EXISTS sync_conflicts (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id             INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    entry_id           INTEGER NOT NULL REFERENCES sync_entries(id) ON DELETE CASCADE,
    kind               TEXT NOT NULL,
    state              TEXT NOT NULL DEFAULT 'open',
    resolution         TEXT,
    native_detail      TEXT CHECK (native_detail IS NULL OR json_valid(native_detail)),
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
    bootstrap_state     TEXT CHECK (bootstrap_state IS NULL OR json_valid(bootstrap_state)),
    discovery_token     TEXT,
    discovery_complete  INTEGER NOT NULL DEFAULT 0 CHECK (discovery_complete IN (0, 1)),
    ingest_token        TEXT,
    applied_token       TEXT,
    target_token        TEXT,
    round_state         TEXT CHECK (round_state IS NULL OR json_valid(round_state)),
    updated_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (job_id, zone_name, owner)
);

-- Ordered receive events. Replay is deduplicated by scoped record revision and
-- never sorts by opaque token or revision strings.
CREATE TABLE IF NOT EXISTS sync_native_inbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id          INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    generation      TEXT NOT NULL,
    zone_name       TEXT NOT NULL,
    owner           TEXT NOT NULL,
    ordinal         INTEGER NOT NULL,
    record_name     TEXT NOT NULL,
    record_type     TEXT NOT NULL,
    record_change_tag TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('created', 'updated', 'deleted')),
    payload         TEXT CHECK (payload IS NULL OR json_valid(payload)),
    created_at      INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (job_id, generation, zone_name, owner, record_name, record_change_tag)
);

-- The projected remote graph at one fixed height. Not visible media_items; a
-- photo becomes visible only through a validated, published pair.
CREATE TABLE IF NOT EXISTS sync_native_records (
    job_id             INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    generation         TEXT NOT NULL,
    zone_name          TEXT NOT NULL,
    owner              TEXT NOT NULL,
    record_name        TEXT NOT NULL,
    record_type        TEXT NOT NULL,
    record_change_tag  TEXT,
    content_generation TEXT,
    payload            TEXT NOT NULL CHECK (json_valid(payload)),
    updated_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (job_id, generation, zone_name, owner, record_name)
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
    role_state        TEXT CHECK (role_state IS NULL OR json_valid(role_state)),
    captured_versions TEXT CHECK (captured_versions IS NULL OR json_valid(captured_versions)),
    old_snapshot      TEXT CHECK (old_snapshot IS NULL OR json_valid(old_snapshot)),
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
CREATE INDEX IF NOT EXISTS idx_sync_native_inbox_scope_ordinal
    ON sync_native_inbox(job_id, generation, zone_name, owner, ordinal);
CREATE INDEX IF NOT EXISTS idx_sync_native_resources_job_state
    ON sync_native_resources(job_id, state);
CREATE INDEX IF NOT EXISTS idx_sync_publications_job_state
    ON sync_publications(job_id, state);

-- Accepted configuration edits must survive exit without mutating in-flight roots.
CREATE TABLE IF NOT EXISTS sync_job_changes (
    job_id INTEGER PRIMARY KEY REFERENCES sync_jobs(id) ON DELETE CASCADE,
    payload TEXT NOT NULL CHECK(json_valid(payload))
);
