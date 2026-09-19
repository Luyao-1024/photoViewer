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

-- Provider-neutral synchronization state. Credentials are referenced from the
-- platform secret store and never persisted in SQLite.
CREATE TABLE IF NOT EXISTS sync_connections (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_kind   TEXT NOT NULL,
    endpoint         TEXT NOT NULL,
    username         TEXT NOT NULL,
    credential_ref  TEXT NOT NULL UNIQUE,
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_at       INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at       INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS sync_jobs (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    connection_id     INTEGER NOT NULL REFERENCES sync_connections(id) ON DELETE CASCADE,
    local_root        TEXT NOT NULL,
    remote_root       TEXT NOT NULL,
    direction         TEXT NOT NULL DEFAULT 'bidirectional',
    upload_scope      TEXT NOT NULL DEFAULT 'selected_albums',
    propagate_deletes INTEGER NOT NULL DEFAULT 0,
    paused            INTEGER NOT NULL DEFAULT 0,
    config_generation INTEGER NOT NULL DEFAULT 1,
    last_started_at   INTEGER,
    last_completed_at INTEGER,
    last_error        TEXT,
    UNIQUE(connection_id, local_root, remote_root)
);

CREATE TABLE IF NOT EXISTS sync_job_upload_albums (
    job_id          INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    relative_album  TEXT NOT NULL,
    PRIMARY KEY(job_id, relative_album)
);

CREATE TABLE IF NOT EXISTS sync_entries (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id               INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    media_id             INTEGER REFERENCES media_items(id) ON DELETE SET NULL,
    relative_path        TEXT NOT NULL,
    local_fingerprint    TEXT,
    local_size           INTEGER,
    local_mtime_ns       INTEGER,
    remote_fingerprint   TEXT,
    remote_size          INTEGER,
    remote_revision      TEXT,
    remote_revision_weak INTEGER NOT NULL DEFAULT 0,
    baseline_fingerprint TEXT,
    baseline_size        INTEGER,
    baseline_remote_revision TEXT,
    state                TEXT NOT NULL DEFAULT 'pending',
    entry_generation     INTEGER NOT NULL DEFAULT 1,
    updated_at           INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(job_id, relative_path)
);

CREATE TABLE IF NOT EXISTS sync_tasks (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id       TEXT NOT NULL UNIQUE,
    job_id             INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    entry_id           INTEGER REFERENCES sync_entries(id) ON DELETE SET NULL,
    action             TEXT NOT NULL,
    state              TEXT NOT NULL,
    expected_revision  TEXT,
    expected_weak      INTEGER NOT NULL DEFAULT 0,
    artifact_path      TEXT,
    artifact_hash      TEXT,
    config_generation  INTEGER NOT NULL,
    entry_generation   INTEGER NOT NULL,
    retry_count        INTEGER NOT NULL DEFAULT 0,
    retry_at           INTEGER,
    last_error         TEXT,
    created_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at         INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS sync_conflicts (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id             INTEGER NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    entry_id           INTEGER NOT NULL REFERENCES sync_entries(id) ON DELETE CASCADE,
    kind               TEXT NOT NULL,
    local_fingerprint  TEXT,
    remote_fingerprint TEXT,
    remote_revision    TEXT,
    state              TEXT NOT NULL DEFAULT 'open',
    resolution         TEXT,
    created_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    resolved_at        INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sync_entries_job_state
    ON sync_entries(job_id, state, relative_path);
CREATE INDEX IF NOT EXISTS idx_sync_tasks_ready
    ON sync_tasks(state, retry_at, job_id);
CREATE INDEX IF NOT EXISTS idx_sync_conflicts_open
    ON sync_conflicts(job_id, state) WHERE state = 'open';
CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_conflicts_one_open_per_entry
    ON sync_conflicts(entry_id) WHERE state = 'open';
