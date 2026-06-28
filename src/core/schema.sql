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
    width           INTEGER,
    height          INTEGER,
    video_duration_secs REAL,
    taken_at        INTEGER,
    file_mtime      INTEGER NOT NULL,
    file_size       INTEGER NOT NULL,
    blake3_hash     TEXT    NOT NULL,
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    trashed_at      INTEGER,
    indexed_at      INTEGER NOT NULL,
    thumbnail_generated_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_media_taken_at
    ON media_items(taken_at DESC) WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_folder
    ON media_items(folder_path)    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_trashed
    ON media_items(trashed_at)     WHERE trashed_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_media_blake3
    ON media_items(blake3_hash);

CREATE INDEX IF NOT EXISTS idx_media_favorite
    ON media_items(is_favorite)
    WHERE trashed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_media_kind
    ON media_items(media_kind)
    WHERE trashed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_media_subkind
    ON media_items(media_subkind)
    WHERE trashed_at IS NULL;

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

-- schema 版本表
CREATE TABLE IF NOT EXISTS schema_version (
    version         INTEGER PRIMARY KEY,
    applied_at      INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_version (version, applied_at)
VALUES (1, unixepoch());
