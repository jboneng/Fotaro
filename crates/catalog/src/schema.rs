//! Schema migrations, tracked with SQLite's `PRAGMA user_version`.
//!
//! Rules: never edit a migration that has shipped; append a new one instead.
//! Each migration runs in a transaction together with its version bump.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// `MIGRATIONS[i]` upgrades the schema from version `i` to `i + 1`.
pub(crate) const MIGRATIONS: &[&str] = &[
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
    // v2: IPTC text fields, XMP sidecar bookkeeping, the file's own orientation,
    // and unique top-level keywords (UNIQUE(parent_id, name) ignores NULL parents).
    r#"
    ALTER TABLE photos ADD COLUMN title     TEXT;
    ALTER TABLE photos ADD COLUMN caption   TEXT;
    ALTER TABLE photos ADD COLUMN creator   TEXT;
    ALTER TABLE photos ADD COLUMN copyright TEXT;
    -- Changed in the catalog since last written to the sidecar (LrC's "needs saving").
    ALTER TABLE photos ADD COLUMN metadata_dirty INTEGER NOT NULL DEFAULT 0;
    -- Sidecar modification time when we last read or wrote it (ms since epoch).
    ALTER TABLE photos ADD COLUMN xmp_mtime_ms INTEGER;
    -- The file's own orientation in clockwise quarter turns, to compose tiff:Orientation.
    ALTER TABLE exif ADD COLUMN orientation INTEGER;
    CREATE UNIQUE INDEX keywords_root_name ON keywords(name COLLATE NOCASE) WHERE parent_id IS NULL;
    CREATE INDEX photo_keywords_keyword ON photo_keywords(keyword_id);
    "#,
    // v3: Quick Collection, stacks, RAW+JPEG pairs (the JPEG is a sidecar file of
    // the raw photo), catalog settings.
    r#"
    CREATE TABLE quick_collection (
        photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE
    );
    CREATE TABLE stacks (
        id        INTEGER PRIMARY KEY,
        collapsed INTEGER NOT NULL DEFAULT 1
    );
    ALTER TABLE photos ADD COLUMN stack_id  INTEGER REFERENCES stacks(id) ON DELETE SET NULL;
    ALTER TABLE photos ADD COLUMN stack_pos INTEGER;
    CREATE INDEX photos_stack ON photos(stack_id);
    ALTER TABLE files ADD COLUMN sidecar_of INTEGER REFERENCES photos(id) ON DELETE CASCADE;
    CREATE INDEX files_sidecar_of ON files(sidecar_of);
    CREATE INDEX collection_photos_photo ON collection_photos(photo_id);
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
    // v4: virtual copies (LrC): several photos may share one file, each with its own
    // metadata. Dropping the UNIQUE on master_file_id needs a table rebuild (run with
    // foreign keys off, see `migrate`); ids are kept, so every reference stays valid.
    r#"
    CREATE TABLE photos_new (
        id              INTEGER PRIMARY KEY,
        master_file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        capture_time    INTEGER,
        rating          INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
        flag            INTEGER NOT NULL DEFAULT 0 CHECK (flag IN (-1, 0, 1)),
        color_label     TEXT,
        orientation     INTEGER NOT NULL DEFAULT 1,
        imported_at     INTEGER NOT NULL,
        title           TEXT,
        caption         TEXT,
        creator         TEXT,
        copyright       TEXT,
        metadata_dirty  INTEGER NOT NULL DEFAULT 0,
        xmp_mtime_ms    INTEGER,
        stack_id        INTEGER REFERENCES stacks(id) ON DELETE SET NULL,
        stack_pos       INTEGER,
        virtual_of      INTEGER REFERENCES photos(id) ON DELETE CASCADE,
        copy_name       TEXT
    );
    INSERT INTO photos_new (id, master_file_id, capture_time, rating, flag, color_label, orientation,
                            imported_at, title, caption, creator, copyright, metadata_dirty,
                            xmp_mtime_ms, stack_id, stack_pos)
        SELECT id, master_file_id, capture_time, rating, flag, color_label, orientation,
               imported_at, title, caption, creator, copyright, metadata_dirty,
               xmp_mtime_ms, stack_id, stack_pos
          FROM photos;
    DROP TABLE photos;
    ALTER TABLE photos_new RENAME TO photos;
    CREATE INDEX photos_capture_time ON photos(capture_time);
    CREATE INDEX photos_stack ON photos(stack_id);
    CREATE INDEX photos_master_file ON photos(master_file_id);
    CREATE INDEX photos_virtual_of ON photos(virtual_of);
    "#,
    // v5: GPS altitude, and whether GPS was read from the file (older versions never
    // read it; photos with `gps_read = 0` are read again in the background).
    r#"
    ALTER TABLE exif ADD COLUMN gps_alt  REAL;
    ALTER TABLE exif ADD COLUMN gps_read INTEGER NOT NULL DEFAULT 0;
    "#,
    // v6: pictures and groups replace stacks (see `pictures.rs`). Every photo belongs to
    // a picture with a role; pictures may be grouped by hand. The JPEG of a RAW+JPEG
    // pair becomes a photo of its own (a pass of the raw's picture). Table rebuilds drop
    // the stack and sidecar columns; `pictures::finish_v6` then joins passes and edits
    // by name and tidies the groups.
    r#"
    CREATE TABLE groups (
        id                INTEGER PRIMARY KEY,
        active_picture_id INTEGER
    );
    CREATE TABLE pictures (
        id              INTEGER PRIMARY KEY,
        group_id        INTEGER REFERENCES groups(id) ON DELETE SET NULL,
        group_pos       INTEGER,
        active_photo_id INTEGER
    );
    CREATE INDEX pictures_group ON pictures(group_id);

    -- A picture per photo that isn't a virtual copy (same id: the table is empty), and
    -- the old stacks as groups.
    INSERT INTO pictures (id, active_photo_id) SELECT id, id FROM photos WHERE virtual_of IS NULL;
    INSERT INTO groups (id) SELECT id FROM stacks;
    UPDATE pictures SET (group_id, group_pos) =
        (SELECT p.stack_id, p.stack_pos FROM photos p WHERE p.id = pictures.id);

    CREATE TABLE photos_new (
        id              INTEGER PRIMARY KEY,
        master_file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        capture_time    INTEGER,
        rating          INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
        flag            INTEGER NOT NULL DEFAULT 0 CHECK (flag IN (-1, 0, 1)),
        color_label     TEXT,
        orientation     INTEGER NOT NULL DEFAULT 1,
        imported_at     INTEGER NOT NULL,
        title           TEXT,
        caption         TEXT,
        creator         TEXT,
        copyright       TEXT,
        metadata_dirty  INTEGER NOT NULL DEFAULT 0,
        xmp_mtime_ms    INTEGER,
        virtual_of      INTEGER REFERENCES photos(id) ON DELETE CASCADE,
        copy_name       TEXT,
        picture_id      INTEGER REFERENCES pictures(id),
        role            TEXT    NOT NULL DEFAULT 'original'
                        CHECK (role IN ('original', 'pass', 'edit', 'copy', 'export')),
        derived_from    INTEGER REFERENCES photos(id) ON DELETE SET NULL,
        member_pos      INTEGER NOT NULL DEFAULT 0
    );
    INSERT INTO photos_new (id, master_file_id, capture_time, rating, flag, color_label, orientation,
                            imported_at, title, caption, creator, copyright, metadata_dirty,
                            xmp_mtime_ms, virtual_of, copy_name,
                            picture_id, role, derived_from, member_pos)
        SELECT id, master_file_id, capture_time, rating, flag, color_label, orientation,
               imported_at, title, caption, creator, copyright, metadata_dirty,
               xmp_mtime_ms, virtual_of, copy_name,
               coalesce(virtual_of, id),
               CASE WHEN virtual_of IS NULL THEN 'original' ELSE 'copy' END,
               virtual_of,
               CASE WHEN virtual_of IS NULL THEN 0 ELSE id END
          FROM photos;
    -- Paired JPEGs: photos of their own, with the raw photo's metadata, in its picture.
    INSERT INTO photos_new (master_file_id, capture_time, rating, color_label, orientation,
                            imported_at, title, caption, creator, copyright, picture_id, role,
                            member_pos)
        SELECT f.id, p.capture_time, p.rating, p.color_label, p.orientation, p.imported_at,
               p.title, p.caption, p.creator, p.copyright, p.id, 'pass', 0
          FROM files f JOIN photos p ON p.id = f.sidecar_of;
    INSERT INTO photo_keywords (photo_id, keyword_id)
        SELECT n.id, pk.keyword_id
          FROM photos_new n JOIN files f ON f.id = n.master_file_id
          JOIN photo_keywords pk ON pk.photo_id = f.sidecar_of
         WHERE n.role = 'pass';
    DROP TABLE photos;
    ALTER TABLE photos_new RENAME TO photos;
    CREATE INDEX photos_capture_time ON photos(capture_time);
    CREATE INDEX photos_master_file ON photos(master_file_id);
    CREATE INDEX photos_virtual_of ON photos(virtual_of);
    CREATE INDEX photos_picture ON photos(picture_id);
    DROP TABLE stacks;

    CREATE TABLE files_new (
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
    INSERT INTO files_new (id, folder_id, name, ext, kind, size, mtime_ms, missing)
        SELECT id, folder_id, name, ext, kind, size, mtime_ms, missing FROM files;
    DROP TABLE files;
    ALTER TABLE files_new RENAME TO files;

    -- Every photo has a picture: a new photo without one gets a picture of its own.
    CREATE TRIGGER photo_new_picture AFTER INSERT ON photos WHEN NEW.picture_id IS NULL
    BEGIN
        INSERT INTO pictures (active_photo_id) VALUES (NEW.id);
        UPDATE photos SET picture_id = last_insert_rowid() WHERE id = NEW.id;
    END;

    -- A picture that loses a photo (removed, or moved to another picture): gone when
    -- empty; otherwise it keeps an original and an active member.
    CREATE TRIGGER photo_gone AFTER DELETE ON photos
    BEGIN
        DELETE FROM pictures WHERE id = OLD.picture_id
           AND NOT EXISTS (SELECT 1 FROM photos WHERE picture_id = OLD.picture_id);
        UPDATE photos SET role = 'original'
         WHERE id = (SELECT id FROM photos WHERE picture_id = OLD.picture_id ORDER BY member_pos, id LIMIT 1)
           AND OLD.role = 'original'
           AND NOT EXISTS (SELECT 1 FROM photos WHERE picture_id = OLD.picture_id AND role = 'original');
        UPDATE pictures SET active_photo_id =
               (SELECT id FROM photos WHERE picture_id = pictures.id
                 ORDER BY role <> 'original', member_pos, id LIMIT 1)
         WHERE id = OLD.picture_id AND active_photo_id = OLD.id;
    END;
    CREATE TRIGGER photo_moved AFTER UPDATE OF picture_id ON photos
      WHEN OLD.picture_id IS NOT NULL AND OLD.picture_id IS NOT NEW.picture_id
    BEGIN
        DELETE FROM pictures WHERE id = OLD.picture_id
           AND NOT EXISTS (SELECT 1 FROM photos WHERE picture_id = OLD.picture_id);
        UPDATE photos SET role = 'original'
         WHERE id = (SELECT id FROM photos WHERE picture_id = OLD.picture_id ORDER BY member_pos, id LIMIT 1)
           AND OLD.role = 'original'
           AND NOT EXISTS (SELECT 1 FROM photos WHERE picture_id = OLD.picture_id AND role = 'original');
        UPDATE pictures SET active_photo_id =
               (SELECT id FROM photos WHERE picture_id = pictures.id
                 ORDER BY role <> 'original', member_pos, id LIMIT 1)
         WHERE id = OLD.picture_id AND active_photo_id = OLD.id;
    END;

    -- A group that loses a picture keeps an active picture; a group of one is no group.
    CREATE TRIGGER picture_gone AFTER DELETE ON pictures WHEN OLD.group_id IS NOT NULL
    BEGIN
        UPDATE groups SET active_picture_id =
               (SELECT id FROM pictures WHERE group_id = groups.id ORDER BY group_pos, id LIMIT 1)
         WHERE id = OLD.group_id AND active_picture_id = OLD.id;
        UPDATE pictures SET group_id = NULL, group_pos = NULL
         WHERE group_id = OLD.group_id
           AND (SELECT COUNT(*) FROM pictures WHERE group_id = OLD.group_id) < 2;
        DELETE FROM groups WHERE id = OLD.group_id
           AND NOT EXISTS (SELECT 1 FROM pictures WHERE group_id = OLD.group_id);
    END;
    CREATE TRIGGER picture_regrouped AFTER UPDATE OF group_id ON pictures
      WHEN OLD.group_id IS NOT NULL AND OLD.group_id IS NOT NEW.group_id
    BEGIN
        UPDATE groups SET active_picture_id =
               (SELECT id FROM pictures WHERE group_id = groups.id ORDER BY group_pos, id LIMIT 1)
         WHERE id = OLD.group_id AND active_picture_id = OLD.id;
        UPDATE pictures SET group_id = NULL, group_pos = NULL
         WHERE group_id = OLD.group_id
           AND (SELECT COUNT(*) FROM pictures WHERE group_id = OLD.group_id) < 2;
        DELETE FROM groups WHERE id = OLD.group_id
           AND NOT EXISTS (SELECT 1 FROM pictures WHERE group_id = OLD.group_id);
    END;
    "#,
    // v7: Develop settings (JSON from the imaging crate) and their history.
    r#"
    CREATE TABLE develop (
        photo_id   INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
        settings   TEXT    NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE develop_history (
        id         INTEGER PRIMARY KEY,
        photo_id   INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
        label      TEXT    NOT NULL,
        settings   TEXT,   -- NULL: no settings (as imported)
        created_at INTEGER NOT NULL
    );
    CREATE INDEX develop_history_photo ON develop_history(photo_id, id);
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
    if current == LATEST_VERSION {
        return Ok(());
    }
    // Table rebuilds (v4) must not cascade deletes from the old table; SQLite's documented
    // procedure: foreign keys off (outside any transaction), migrate, check, back on.
    let fk_on: bool = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let result = (|| -> Result<()> {
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let next = i as u32 + 1;
            let tx = conn.transaction()?;
            tx.execute_batch(sql).with_context(|| format!("migrating catalog to v{next}"))?;
            if next == 6 {
                crate::pictures::finish_v6(&tx).context("migrating catalog to v6")?;
            }
            let broken: i64 =
                tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
            anyhow::ensure!(
                broken == 0,
                "migrating to v{next} broke {broken} references; nothing was changed"
            );
            tx.pragma_update(None, "user_version", next)?;
            tx.commit()?;
            log::info!("catalog migrated to schema v{next}");
        }
        Ok(())
    })();
    if fk_on {
        conn.pragma_update(None, "foreign_keys", "ON")?;
    }
    result
}
