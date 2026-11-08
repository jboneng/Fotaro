//! Schema migrations, tracked with SQLite's `PRAGMA user_version`.
//!
//! Rules: never edit a migration that has shipped; append a new one instead.
//! Each migration runs in a transaction together with its version bump.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// `MIGRATIONS[i]` upgrades the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[
    // v1: folders, files, photos, EXIF, keywords, collections.
    r#"
    CREATE TABLE folders (
        id          INTEGER PRIMARY KEY,
        path        TEXT    NOT NULL UNIQUE,
        parent_id   INTEGER REFERENCES folders(id) ON DELETE CASCADE
    );
    CREATE INDEX folders_parent ON folders(parent_id);

    CREATE TABLE files (
        id          INTEGER PRIMARY KEY,
        folder_id   INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
        name        TEXT    NOT NULL,
        ext         TEXT    NOT NULL,
        kind        TEXT    NOT NULL CHECK (kind IN ('raw', 'image')),
        size        INTEGER NOT NULL,
        mtime_ms    INTEGER NOT NULL,
        missing     INTEGER NOT NULL DEFAULT 0,
        UNIQUE (folder_id, name)
    );

    CREATE TABLE photos (
        id              INTEGER PRIMARY KEY,
        master_file_id  INTEGER NOT NULL UNIQUE REFERENCES files(id) ON DELETE CASCADE,
        capture_time    INTEGER,
        rating          INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
        flag            INTEGER NOT NULL DEFAULT 0 CHECK (flag IN (-1, 0, 1)),
        color_label     TEXT,
        orientation     INTEGER NOT NULL DEFAULT 1,
        imported_at     INTEGER NOT NULL
    );
    CREATE INDEX photos_capture_time ON photos(capture_time);

    CREATE TABLE exif (
        photo_id    INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
        camera_make TEXT,
        camera_model TEXT,
        lens        TEXT,
        iso         REAL,
        shutter     REAL,
        aperture    REAL,
        focal_len   REAL,
        width       INTEGER,
        height      INTEGER,
        gps_lat     REAL,
        gps_lon     REAL
    );

    CREATE TABLE keywords (
        id          INTEGER PRIMARY KEY,
        name        TEXT    NOT NULL,
        parent_id   INTEGER REFERENCES keywords(id) ON DELETE CASCADE,
        UNIQUE (parent_id, name)
    );

    CREATE TABLE photo_keywords (
        photo_id    INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
        keyword_id  INTEGER NOT NULL REFERENCES keywords(id) ON DELETE CASCADE,
        PRIMARY KEY (photo_id, keyword_id)
    );

    CREATE TABLE collections (
        id          INTEGER PRIMARY KEY,
        name        TEXT    NOT NULL,
        parent_id   INTEGER REFERENCES collections(id) ON DELETE CASCADE,
        kind        TEXT    NOT NULL CHECK (kind IN ('set', 'collection', 'smart')),
        smart_rules TEXT
    );

    CREATE TABLE collection_photos (
        collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
        photo_id      INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
        position      INTEGER,
        PRIMARY KEY (collection_id, photo_id)
    );
    "#,
];

pub const LATEST_VERSION: u32 = MIGRATIONS.len() as u32;

pub fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// Bring the database up to [`LATEST_VERSION`]. Safe to call on every open.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let current = version(conn)?;
    anyhow::ensure!(
        current <= LATEST_VERSION,
        "catalog schema v{current} is newer than this app supports (v{LATEST_VERSION}); \
         please update the app"
    );
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let next = i as u32 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql).with_context(|| format!("migrating catalog to v{next}"))?;
        tx.pragma_update(None, "user_version", next)?;
        tx.commit()?;
        log::info!("catalog migrated to schema v{next}");
    }
    Ok(())
}
