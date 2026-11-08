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
