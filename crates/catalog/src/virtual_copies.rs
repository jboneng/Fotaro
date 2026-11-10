//! Virtual copies (LrC Photo ▸ Create Virtual Copy, Ctrl+'): more photos of the same
//! file, each with its own keywords, IPTC fields and collections. A copy starts with its
//! master's keywords, IPTC fields and rotation, and is a member of its picture (role
//! Copy), sharing the stack's rating, flag and label. Only the master writes an XMP
//! sidecar.

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

use crate::catalog::Catalog;

impl Catalog {
    /// Create a virtual copy of a photo (of its master, if it is itself a copy).
    /// Returns the new photo's id.
    pub fn create_virtual_copy(&mut self, photo_id: i64) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let master: i64 = tx
            .query_row("SELECT coalesce(virtual_of, id) FROM photos WHERE id = ?1", [photo_id], |r| r.get(0))
            .optional()?
            .context("photo not found")?;
        let copies: i64 =
            tx.query_row("SELECT COUNT(*) FROM photos WHERE virtual_of = ?1", [master], |r| r.get(0))?;
        let name = format!("Copy {}", copies + 1);
        tx.execute(
            "INSERT INTO photos (master_file_id, capture_time, color_label, orientation, imported_at,
                                 title, caption, creator, copyright, virtual_of, copy_name)
             SELECT master_file_id, capture_time, color_label, orientation, imported_at,
                    title, caption, creator, copyright, id, ?2
               FROM photos WHERE id = ?1",
            params![master, name],
        )?;
        let copy = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO exif (photo_id, camera_make, camera_model, lens, iso, shutter, aperture, focal_len,
                               width, height, gps_lat, gps_lon, gps_alt, gps_read, orientation)
             SELECT ?2, camera_make, camera_model, lens, iso, shutter, aperture, focal_len,
                    width, height, gps_lat, gps_lon, gps_alt, gps_read, orientation
               FROM exif WHERE photo_id = ?1",
            params![master, copy],
        )?;
        tx.execute(
            "INSERT INTO photo_keywords (photo_id, keyword_id) SELECT ?2, keyword_id FROM photo_keywords WHERE photo_id = ?1",
            params![master, copy],
        )?;

        // A member of the master's picture, and what the unit shows (you make a copy to
        // work on it).
        crate::pictures::join(&tx, copy, master, crate::pictures::Role::Copy, Some(photo_id), true)?;
        tx.commit()?;
        Ok(copy)
    }

    /// Give `to` the catalog metadata of `from`: rating, label, title, caption, creator,
    /// copyright and keywords (LrC's Edit In copies them to the edited file). Flags and
    /// collections stay the photo's own.
    pub fn copy_metadata(&mut self, from: i64, to: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE photos SET (rating, color_label, title, caption, creator, copyright, metadata_dirty) =
                    (SELECT rating, color_label, title, caption, creator, copyright, 1
                       FROM photos WHERE id = ?1)
              WHERE id = ?2 AND EXISTS (SELECT 1 FROM photos WHERE id = ?1)",
            params![from, to],
        )?;
        anyhow::ensure!(changed == 1, "photo not found");
        tx.execute(
            "INSERT OR IGNORE INTO photo_keywords (photo_id, keyword_id)
             SELECT ?2, keyword_id FROM photo_keywords WHERE photo_id = ?1",
            params![from, to],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Delete virtual copies (masters in `photo_ids` are ignored). Returns how many
    /// were deleted. Their file stays: it belongs to the master.
    pub fn delete_virtual_copies(&mut self, photo_ids: &[i64]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for id in photo_ids {
            n += tx.execute("DELETE FROM photos WHERE id = ?1 AND virtual_of IS NOT NULL", [id])?;
        }
        tx.commit()?;
        Ok(n)
    }
}

impl Catalog {
    /// Is this photo a virtual copy?
    pub fn is_virtual_copy(&self, photo_id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT virtual_of IS NOT NULL FROM photos WHERE id = ?1", [photo_id], |r| r.get(0))
            .optional()?
            .unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Catalog, Flag, IptcField, Source};

    #[test]
    fn copies_have_their_own_metadata_and_join_the_masters_picture() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("a.NEF"), b"x").unwrap();
        std::fs::write(dir.join("b.NEF"), b"x").unwrap();
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&dir).unwrap();
        let a = cat.photos(Source::AllPhotographs).unwrap()[0].id;
        cat.set_rating(a, 4).unwrap();
        cat.set_flag(a, Flag::Picked).unwrap();
        cat.set_color_label(a, Some("red")).unwrap();
        cat.set_iptc(&[a], IptcField::Title, "Hegra").unwrap();
        let kw = cat.ensure_keyword(&["Film".into()]).unwrap();
        cat.add_keyword(&[a], kw).unwrap();

        let c1 = cat.create_virtual_copy(a).unwrap();
        let c2 = cat.create_virtual_copy(c1).unwrap(); // a copy of a copy is a copy of the master
        let members = cat.unit_members(a).unwrap();
        let names: Vec<(i64, Option<&str>, bool)> =
            members.iter().map(|m| (m.photo.id, m.photo.virtual_copy.as_deref(), m.active)).collect();
        assert_eq!(
            names,
            [(a, None, false), (c1, Some("Copy 1"), false), (c2, Some("Copy 2"), true)],
            "the newest shown"
        );
        assert_eq!(members[2].derived_from, Some(c1));
        let photos: Vec<_> = members.into_iter().map(|m| m.photo).collect();
        let copy = photos.iter().find(|p| p.id == c1).unwrap();
        assert_eq!(
            (copy.rating, copy.flag, copy.color_label.as_deref()),
            (4, Flag::Picked, Some("red")),
            "the stack's marks"
        );
        assert_eq!(copy.path, photos[0].path, "same file");
        assert_eq!(cat.photo_keywords(c1).unwrap(), vec![vec!["Film".to_string()]]);
        assert_eq!(cat.iptc(c1).unwrap().title.as_deref(), Some("Hegra"));
        assert_eq!((copy.unit.members, copy.unit.role), (3, crate::Role::Copy));

        // Each photo still stores its own (the app sets a stack's marks on all of them).
        cat.set_rating(c1, 1).unwrap();
        assert_eq!(cat.photo(a).unwrap().unwrap().rating, 4);

        // Copies never write sidecars; the master does.
        let none: crate::OrientationReader = &|_, _| None;
        assert!(cat.write_xmp(c1, none).unwrap().is_none());
        assert!(cat.write_xmp(a, none).unwrap().is_some());

        // Deleting copies leaves the master (and the file), shown again.
        assert_eq!(cat.delete_virtual_copies(&[a, c1, c2]).unwrap(), 2);
        let left = cat.photos(Source::AllPhotographs).unwrap();
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|p| p.unit.members == 1 && p.virtual_copy.is_none()));
        assert!(dir.join("a.NEF").exists());

        // Removing the folder removes copies with their masters.
        let c3 = cat.create_virtual_copy(a).unwrap();
        let folder = cat.folders().unwrap()[0].id;
        let removed = cat.remove_folder(folder).unwrap();
        assert!(removed.contains(&c3), "{removed:?}");
        assert!(cat.photos(Source::AllPhotographs).unwrap().is_empty());
    }

    #[test]
    fn edited_files_get_the_originals_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("a.NEF"), b"x").unwrap();
        std::fs::write(dir.join("a_NegPy.tif"), b"x").unwrap();
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&dir).unwrap();
        // Look photos up among the unit's members: the grid shows the edit only.
        let id = |cat: &Catalog, n: &str| {
            let shown = cat.photos(Source::AllPhotographs).unwrap()[0].id;
            cat.unit_members(shown).unwrap().into_iter().map(|m| m.photo).find(|p| p.file_name == n).unwrap()
        };
        let (a, e) = (id(&cat, "a.NEF").id, id(&cat, "a_NegPy.tif").id);
        cat.set_rating(a, 3).unwrap();
        cat.set_flag(a, Flag::Picked).unwrap();
        cat.set_color_label(a, Some("green")).unwrap();
        cat.set_iptc(&[a], crate::IptcField::Title, "Pier").unwrap();
        let k = cat.create_keyword(None, "Film").unwrap();
        cat.add_keyword(&[a], k).unwrap();
        cat.copy_metadata(a, e).unwrap();
        let edited = id(&cat, "a_NegPy.tif");
        assert_eq!((edited.rating, edited.color_label.as_deref()), (3, Some("green")));
        assert_eq!(edited.flag, Flag::Unflagged, "flags stay the photo's own");
        assert_eq!(cat.xmp_data(e).unwrap().title.as_deref(), Some("Pier"));
        assert_eq!(cat.photo_keywords(e).unwrap(), vec![vec!["Film".to_owned()]]);
        assert!(cat.copy_metadata(a, 9999).is_err());
    }

    #[test]
    fn migration_to_v4_keeps_everything_attached() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        std::fs::write(dir.join("b.jpg"), b"x").unwrap();
        let db = tmp.path().join("c.plcat");
        // Build a v3 catalog with keywords, collections, a stack and the Quick Collection.
        {
            let conn = rusqlite::Connection::open({
                std::fs::create_dir_all(db.join("previews")).unwrap();
                db.join("catalog.db")
            })
            .unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for (i, sql) in crate::schema::MIGRATIONS.iter().take(3).enumerate() {
                conn.execute_batch(sql).unwrap();
                conn.pragma_update(None, "user_version", i as u32 + 1).unwrap();
            }
            conn.execute("INSERT INTO folders (path) VALUES (?1)", [dir.to_string_lossy()]).unwrap();
            for (i, name) in ["a.jpg", "b.jpg"].iter().enumerate() {
                conn.execute(
                    "INSERT INTO files (folder_id, name, ext, kind, size, mtime_ms) VALUES (1, ?1, 'jpg', 'image', 1, 0)",
                    [name],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO photos (master_file_id, imported_at, rating) VALUES (?1, 0, 3)",
                    [i as i64 + 1],
                )
                .unwrap();
            }
            conn.execute_batch(
                "INSERT INTO exif (photo_id, camera_model) VALUES (1, 'D750');
                 INSERT INTO keywords (name) VALUES ('cat');
                 INSERT INTO photo_keywords VALUES (1, 1);
                 INSERT INTO collections (name, kind) VALUES ('Best', 'collection');
                 INSERT INTO collection_photos (collection_id, photo_id) VALUES (1, 2);
                 INSERT INTO quick_collection VALUES (1);
                 INSERT INTO stacks (collapsed) VALUES (0);
                 UPDATE photos SET stack_id = 1, stack_pos = id - 1;",
            )
            .unwrap();
        }
        let cat = Catalog::open(&db).unwrap();
        assert_eq!(cat.schema_version().unwrap(), crate::schema::LATEST_VERSION);
        let photos = cat.photos(Source::AllPhotographs).unwrap();
        assert_eq!(photos.len(), 1, "the old stack is a group");
        assert_eq!((photos[0].unit.members, photos[0].unit.pictures), (2, 2));
        assert!(cat.unit_members(1).unwrap().iter().all(|m| m.photo.rating == 3));
        assert_eq!(cat.photo_keywords(1).unwrap(), vec![vec!["cat".to_string()]]);
        // The collection holds photo 2: it shows that photo's unit, the group (photo 1 shown).
        assert_eq!(cat.photos(Source::Collection { id: 1 }).unwrap()[0].id, 1);
        assert!(cat.photo(1).unwrap().unwrap().in_quick_collection);
        assert_eq!(cat.metadata(1).unwrap().unwrap().model.as_deref(), Some("D750"));
    }
}
