//! Folder operations from LrC's Folders panel context menu.
//!
//! Catalog-only operations (Remove, Show Parent Folder) never touch files on disk.
//! Create Folder and Rename change the file system, then the catalog; if the
//! catalog update fails after a rename, the rename is undone.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

use crate::catalog::Catalog;

/// Reject names that would escape the parent folder or are invalid on common file systems.
pub fn validate_folder_name(name: &str) -> Result<&str> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "the folder name is empty");
    anyhow::ensure!(name != "." && name != "..", "\"{name}\" is not a valid folder name");
    const BAD: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    anyhow::ensure!(!name.contains(BAD), "folder names cannot contain any of / \\ : * ? \" < > |");
    anyhow::ensure!(!name.ends_with('.'), "folder names cannot end with a period");
    Ok(name)
}

/// Result of [`Catalog::relocate_folder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocated {
    /// The folder's new (canonical) path.
    pub path: PathBuf,
    /// Photos in the folder and its subfolders that exist at the new location...
    pub found: usize,
    /// ...out of this many.
    pub total: usize,
}

impl Catalog {
    fn folder_path(&self, id: i64) -> Result<PathBuf> {
        let p: String = self
            .conn
            .query_row("SELECT path FROM folders WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .context("folder not found in catalog")?;
        Ok(PathBuf::from(p))
    }

    /// Ids of all photos in a folder and its subfolders.
    pub fn folder_photo_ids(&self, id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE tree(id) AS (
                 SELECT ?1 UNION ALL SELECT fo.id FROM folders fo JOIN tree ON fo.parent_id = tree.id
             )
             SELECT p.id FROM photos p JOIN files f ON f.id = p.master_file_id
              WHERE f.folder_id IN tree",
        )?;
        let ids = stmt.query_map([id], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    /// Remove a folder, its subfolders and their photos from the catalog (files on
    /// disk are untouched). Returns the removed photo ids, e.g. to delete previews.
    pub fn remove_folder(&mut self, id: i64) -> Result<Vec<i64>> {
        let removed = self.folder_photo_ids(id)?;
        // Subfolders, files, photos, EXIF, keywords links... cascade from here.
        let n = self.conn.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        anyhow::ensure!(n == 1, "folder not found in catalog");
        Ok(removed)
    }

    /// LrC "Show Parent Folder": add the parent of a top-level folder to the catalog
    /// above it. Photos directly in the parent are not imported until it is synchronized.
    pub fn show_parent_folder(&mut self, id: i64) -> Result<i64> {
        let path = self.folder_path(id)?;
        let parent = path.parent().context("this folder has no parent")?;
        let tx = self.conn.transaction()?;
        let parent_id = match tx
            .query_row("SELECT id FROM folders WHERE path = ?1", [parent.to_string_lossy()], |r| r.get(0))
            .optional()?
        {
            Some(pid) => pid,
            None => {
                // The parent itself goes under its own parent if that is in the catalog.
                let grandparent: Option<i64> = match parent.parent() {
                    Some(gp) => tx
                        .query_row("SELECT id FROM folders WHERE path = ?1", [gp.to_string_lossy()], |r| {
                            r.get(0)
                        })
                        .optional()?,
                    None => None,
                };
                tx.execute(
                    "INSERT INTO folders (path, parent_id) VALUES (?1, ?2)",
                    params![parent.to_string_lossy(), grandparent],
                )?;
                tx.last_insert_rowid()
            }
        };
        tx.execute("UPDATE folders SET parent_id = ?1 WHERE id = ?2", params![parent_id, id])?;
        tx.commit()?;
        Ok(parent_id)
    }

    /// Create a new folder on disk inside a catalog folder, and add it to the catalog.
    pub fn create_subfolder(&mut self, parent_id: i64, name: &str) -> Result<i64> {
        let name = validate_folder_name(name)?;
        let path = self.folder_path(parent_id)?.join(name);
        anyhow::ensure!(!path.exists(), "\"{name}\" already exists");
        std::fs::create_dir(&path).with_context(|| format!("creating {}", path.display()))?;
        self.conn.execute(
            "INSERT INTO folders (path, parent_id) VALUES (?1, ?2)",
            params![path.to_string_lossy(), parent_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Rename a folder on disk and update it and all subfolders in the catalog.
    pub fn rename_folder(&mut self, id: i64, new_name: &str) -> Result<PathBuf> {
        let new_name = validate_folder_name(new_name)?;
        let old = self.folder_path(id)?;
        let new = old.parent().context("cannot rename a drive root")?.join(new_name);
        if new == old {
            return Ok(new);
        }
        anyhow::ensure!(!new.exists(), "\"{new_name}\" already exists");
        std::fs::rename(&old, &new).with_context(|| format!("renaming {}", old.display()))?;

        match self.rewrite_folder_paths(&old, &new) {
            Ok(()) => Ok(new),
            Err(e) => {
                // Keep disk and catalog consistent: undo the rename.
                if let Err(undo) = std::fs::rename(&new, &old) {
                    log::error!("could not undo rename of {} after catalog error: {undo}", old.display());
                }
                Err(e.context("updating the catalog after renaming"))
            }
        }
    }

    /// LrC "Find Missing Folder…" / "Update Folder Location…": point a catalog folder (and
    /// its subfolders) at a new location on disk, e.g. after the folder was moved outside
    /// the app or a drive came back under another letter. Nothing on disk changes.
    ///
    /// Refuses a location where none of the folder's photos exist (almost certainly the
    /// wrong folder), and one that would collide with folders already in the catalog.
    /// Returns how many of the folder's photos were found there; the caller should
    /// synchronize the folder afterwards to refresh missing/changed files.
    pub fn relocate_folder(&mut self, id: i64, new_location: &Path) -> Result<Relocated> {
        let old = self.folder_path(id)?;
        let new = dunce::canonicalize(new_location)
            .with_context(|| format!("{} is not available", new_location.display()))?;
        anyhow::ensure!(new.is_dir(), "{} is not a folder", new.display());
        if new == old {
            let total = self.folder_photo_ids(id)?.len();
            return Ok(Relocated { path: new, found: total, total });
        }
        anyhow::ensure!(
            !new.starts_with(&old),
            "a folder cannot be moved into itself ({} is inside {})",
            new.display(),
            old.display()
        );

        // Folders outside this subtree must not already live at (or under) the new location.
        let subtree: std::collections::HashSet<i64> = self.subtree_ids(id)?.into_iter().collect();
        for f in self.folders()? {
            if !subtree.contains(&f.id) && f.path.starts_with(&new) {
                anyhow::bail!(
                    "{} is already in the catalog. Remove it first, or relink a different folder.",
                    f.path.display()
                );
            }
        }

        // Sanity check: some of the photos should actually be at the new location.
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE tree(id) AS (
                 SELECT ?1 UNION ALL SELECT fo.id FROM folders fo JOIN tree ON fo.parent_id = tree.id
             )
             SELECT fo.path, f.name FROM files f JOIN folders fo ON fo.id = f.folder_id
              WHERE f.folder_id IN tree",
        )?;
        let files: Vec<(String, String)> =
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let total = files.len();
        let found = files
            .iter()
            .filter(|(dir, name)| {
                let rel = Path::new(dir).strip_prefix(&old).unwrap_or(Path::new(""));
                new.join(rel).join(name).is_file()
            })
            .count();
        anyhow::ensure!(
            total == 0 || found > 0,
            "none of the {total} photos in \"{}\" were found in {}",
            old.file_name().unwrap_or_default().to_string_lossy(),
            new.display()
        );

        self.rewrite_folder_paths(&old, &new)?;
        // Re-parent under the catalog folder that contains the new location, if any.
        let parent_id: Option<i64> = match new.parent() {
            Some(p) => self.folder_by_path(p)?,
            None => None,
        };
        self.conn.execute("UPDATE folders SET parent_id = ?1 WHERE id = ?2", params![parent_id, id])?;
        Ok(Relocated { path: new, found, total })
    }

    fn subtree_ids(&self, id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE tree(id) AS (
                 SELECT ?1 UNION ALL SELECT fo.id FROM folders fo JOIN tree ON fo.parent_id = tree.id
             ) SELECT id FROM tree",
        )?;
        let ids = stmt.query_map([id], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    /// Replace the `old` path prefix with `new` for a folder and all its descendants.
    fn rewrite_folder_paths(&mut self, old: &Path, new: &Path) -> Result<()> {
        let old_s = old.to_string_lossy().into_owned();
        let new_s = new.to_string_lossy().into_owned();
        let sep = std::path::MAIN_SEPARATOR;
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE folders SET path = ?2 || substr(path, length(?1) + 1)
              WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || ?3",
            params![old_s, new_s, sep.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;
    use std::fs;

    fn setup() -> (tempfile::TempDir, PathBuf, Catalog) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Photos");
        fs::create_dir_all(root.join("Trip/Day 1")).unwrap();
        fs::write(root.join("a.jpg"), b"x").unwrap();
        fs::write(root.join("Trip/b.jpg"), b"x").unwrap();
        fs::write(root.join("Trip/Day 1/c.jpg"), b"x").unwrap();
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&root).unwrap();
        let root = dunce::canonicalize(&root).unwrap();
        (tmp, root, cat)
    }

    fn id_of(cat: &Catalog, path: &Path) -> i64 {
        cat.folder_by_path(path).unwrap().unwrap()
    }

    #[test]
    fn validates_names() {
        assert!(validate_folder_name("2026 Iceland").is_ok());
        for bad in ["", "  ", "..", "a/b", "a\\b", "x:y", "trailing."] {
            assert!(validate_folder_name(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn remove_folder_removes_subtree_from_catalog_only() {
        let (_tmp, root, mut cat) = setup();
        let trip = id_of(&cat, &root.join("Trip"));
        assert_eq!(cat.folder_photo_ids(trip).unwrap().len(), 2);
        let removed = cat.remove_folder(trip).unwrap();
        assert_eq!(removed.len(), 2);
        assert_eq!(cat.photos(Source::AllPhotographs).unwrap().len(), 1);
        assert_eq!(cat.folders().unwrap().len(), 1, "subfolders removed too");
        assert!(root.join("Trip/b.jpg").exists(), "files on disk must be untouched");
    }

    #[test]
    fn rename_updates_descendants_and_keeps_photos() {
        let (_tmp, root, mut cat) = setup();
        fs::create_dir(root.join("Trip 2")).unwrap();
        fs::write(root.join("Trip 2/d.jpg"), b"x").unwrap();
        cat.sync_folder(&root).unwrap();
        let trip = id_of(&cat, &root.join("Trip"));
        let new = cat.rename_folder(trip, "Iceland").unwrap();
        assert!(new.join("Day 1/c.jpg").exists());
        assert!(cat.folder_by_path(&root.join("Iceland").join("Day 1")).unwrap().is_some());
        assert!(cat.folder_by_path(&root.join("Trip")).unwrap().is_none());
        // A sibling whose name merely starts with the old name is not affected.
        assert!(cat.folder_by_path(&root.join("Trip 2")).unwrap().is_some());
        let photos = cat.photos(Source::AllPhotographs).unwrap();
        assert!(photos.iter().all(|p| p.path.exists()), "{photos:?}");
        // Re-sync finds nothing missing.
        let r = cat.sync_folder(&root).unwrap();
        assert_eq!((r.missing, r.added), (0, 0));
    }

    #[test]
    fn rename_refuses_existing_target() {
        let (_tmp, root, mut cat) = setup();
        fs::create_dir(root.join("Taken")).unwrap();
        let trip = id_of(&cat, &root.join("Trip"));
        assert!(cat.rename_folder(trip, "Taken").is_err());
        assert!(root.join("Trip").exists());
    }

    #[test]
    fn relocate_finds_moved_folder_and_keeps_metadata() {
        let (tmp, root, mut cat) = setup();
        let trip = id_of(&cat, &root.join("Trip"));
        let b = cat.photos(Source::Folder { id: trip, include_subfolders: false }).unwrap()[0].id;
        cat.set_rating(b, 4).unwrap();

        // Move the folder behind the catalog's back (e.g. a drive letter change).
        let elsewhere = tmp.path().join("Elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::rename(root.join("Trip"), elsewhere.join("Trip")).unwrap();
        let r = cat.sync_folder(&root).unwrap();
        assert_eq!(r.missing, 2, "both photos go missing, nothing is deleted");

        let moved = cat.relocate_folder(trip, &elsewhere.join("Trip")).unwrap();
        assert_eq!((moved.found, moved.total), (2, 2));
        let r = cat.sync_folder(&moved.path).unwrap();
        assert_eq!((r.restored, r.added, r.missing), (2, 0, 0));

        let photos = cat.photos(Source::Folder { id: trip, include_subfolders: true }).unwrap();
        assert_eq!(photos.len(), 2);
        assert!(photos.iter().all(|p| p.path.exists() && !p.missing), "{photos:?}");
        assert_eq!(photos.iter().find(|p| p.id == b).unwrap().rating, 4, "ratings survive");
        let day1 = cat.folder_by_path(&moved.path.join("Day 1")).unwrap();
        assert!(day1.is_some(), "subfolders move along");
        let f = cat.folders().unwrap().into_iter().find(|f| f.id == trip).unwrap();
        assert_eq!(f.parent_id, None, "the new location is not inside a catalog folder");
    }

    #[test]
    fn relocate_refuses_wrong_or_conflicting_folders() {
        let (tmp, root, mut cat) = setup();
        let trip = id_of(&cat, &root.join("Trip"));
        let empty = tmp.path().join("Empty");
        fs::create_dir(&empty).unwrap();
        let err = cat.relocate_folder(trip, &empty).unwrap_err().to_string();
        assert!(err.contains("none of the 2 photos"), "{err}");
        // The root folder is already in the catalog.
        let err = cat.relocate_folder(trip, &root).unwrap_err().to_string();
        assert!(err.contains("already in the catalog"), "{err}");
        // Into its own subfolder.
        let root_id = id_of(&cat, &root);
        assert!(cat.relocate_folder(root_id, &root.join("Trip")).is_err());
        assert!(cat.folder_by_path(&root.join("Trip")).unwrap().is_some(), "nothing changed");
    }

    #[test]
    fn create_subfolder_and_show_parent() {
        let (_tmp, root, mut cat) = setup();
        let root_id = id_of(&cat, &root);
        let new = cat.create_subfolder(root_id, "Selects").unwrap();
        assert!(root.join("Selects").is_dir());
        assert_eq!(cat.folders().unwrap().iter().find(|f| f.id == new).unwrap().parent_id, Some(root_id));

        let parent = cat.show_parent_folder(root_id).unwrap();
        let folders = cat.folders().unwrap();
        assert_eq!(folders.iter().find(|f| f.id == root_id).unwrap().parent_id, Some(parent));
        assert_eq!(folders.iter().find(|f| f.id == parent).unwrap().parent_id, None);
    }
}
