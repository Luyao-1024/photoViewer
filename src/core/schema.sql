-- media_items 主表
CREATE TABLE IF NOT EXISTS media_items (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uri             TEXT    UNIQUE NOT NULL,
    path            TEXT    NOT NULL,
    folder_path     TEXT    NOT NULL,
    mime_type       TEXT    NOT NULL,
    width           INTEGER,
    height          INTEGER,
    taken_at        INTEGER,
    file_mtime      INTEGER NOT NULL,
    file_size       INTEGER NOT NULL,
    blake3_hash     TEXT    NOT NULL,
    trashed_at      INTEGER,
    indexed_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_media_taken_at
    ON media_items(taken_at DESC) WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_folder
    ON media_items(folder_path)    WHERE trashed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_media_trashed
    ON media_items(trashed_at)     WHERE trashed_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_media_blake3
    ON media_items(blake3_hash);

-- albums 物化视图
CREATE TABLE IF NOT EXISTS albums (
    folder_path     TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    cover_uri       TEXT,
    photo_count     INTEGER NOT NULL DEFAULT 0,
    last_modified   INTEGER NOT NULL
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