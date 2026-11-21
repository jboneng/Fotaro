//! The app was called "Photo Library" before it became Fotaro: its settings, presets,
//! LUTs, lens database and AI models are moved from the old name's folders to the new
//! ones the first time Fotaro starts, so nothing is lost or downloaded again.

use std::path::Path;

/// The names the app's folders were made under before.
const OLD_DATA_NAME: &str = "PhotoLibrary";
const OLD_SETTINGS_NAME: &str = "Photo Library";

/// Move what the old name's folders hold into the new ones (once: what is moved is gone
/// from the old folders, and anything already in the new ones is kept).
pub fn move_old_folders() {
    let mut moves = Vec::new();
    // The window's settings (last catalog, panels, preferences), kept by eframe.
    if let (Some(old), Some(new)) =
        (eframe::storage_dir(OLD_SETTINGS_NAME), eframe::storage_dir(crate::APP_NAME))
    {
        moves.push((old, new));
    }
    // Presets, LUTs and the lens database; and the AI models (the local data folder).
    if let (Some(old), Some(new)) = (
        directories::ProjectDirs::from("", "", OLD_DATA_NAME),
        directories::ProjectDirs::from("", "", crate::APP_NAME),
    ) {
        moves.push((old.data_dir().to_path_buf(), new.data_dir().to_path_buf()));
        moves.push((old.data_local_dir().to_path_buf(), new.data_local_dir().to_path_buf()));
    }
    for (old, new) in moves {
        if old != new && old.is_dir() {
            log::info!("moving {} to {}", old.display(), new.display());
            move_contents(&old, &new);
        }
    }
}

/// Move everything in `old` into `new` (made if missing), keeping what `new` already has
/// under the same name; then remove `old`, and its parent, if they are left empty.
fn move_contents(old: &Path, new: &Path) {
    if let Err(e) = std::fs::create_dir_all(new) {
        log::warn!("can't make {}: {e}", new.display());
        return;
    }
    let Ok(entries) = std::fs::read_dir(old) else { return };
    for entry in entries.flatten() {
        let (from, to) = (entry.path(), new.join(entry.file_name()));
        if to.exists() {
            log::info!("keeping {} (it is already there)", to.display());
            continue;
        }
        // A rename is instant; across drives (or if it fails) a copy, then the original goes.
        if std::fs::rename(&from, &to).is_err() {
            match copy_all(&from, &to) {
                Ok(()) => {
                    let _ = if from.is_dir() {
                        std::fs::remove_dir_all(&from)
                    } else {
                        std::fs::remove_file(&from)
                    };
                }
                Err(e) => log::warn!("can't move {} to {}: {e}", from.display(), to.display()),
            }
        }
    }
    // Only empty folders are removed (anything that couldn't be moved stays).
    if std::fs::remove_dir(old).is_ok()
        && let Some(parent) = old.parent()
    {
        let _ = std::fs::remove_dir(parent);
    }
}

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_contents_move_and_what_is_there_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("Photo Library").join("data");
        let new = tmp.path().join("Fotaro").join("data");
        std::fs::create_dir_all(old.join("LUTs")).unwrap();
        std::fs::write(old.join("LUTs").join("a.cube"), "lut").unwrap();
        std::fs::write(old.join("app.ron"), "old settings").unwrap();
        std::fs::write(old.join("presets.json"), "old presets").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("presets.json"), "new presets").unwrap();

        move_contents(&old, &new);
        assert_eq!(std::fs::read_to_string(new.join("LUTs").join("a.cube")).unwrap(), "lut");
        assert_eq!(std::fs::read_to_string(new.join("app.ron")).unwrap(), "old settings");
        // Already there: kept, and the old one stays where it was (so the old folder does too).
        assert_eq!(std::fs::read_to_string(new.join("presets.json")).unwrap(), "new presets");
        assert!(old.join("presets.json").is_file());
        assert!(!old.join("LUTs").exists());

        // Once the rest is gone, the emptied old folders go too.
        std::fs::remove_file(old.join("presets.json")).unwrap();
        move_contents(&old, &new);
        assert!(!old.exists() && !old.parent().unwrap().exists());
    }

    #[test]
    fn copies_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a");
        std::fs::create_dir_all(from.join("b").join("c")).unwrap();
        std::fs::write(from.join("b").join("c").join("x"), "1").unwrap();
        copy_all(&from, &tmp.path().join("z")).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.path().join("z").join("b").join("c").join("x")).unwrap(), "1");
    }
}
