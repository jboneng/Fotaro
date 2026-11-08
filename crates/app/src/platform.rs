//! OS integration helpers.

use std::path::Path;
use std::process::Command;

/// Menu label for revealing a file, matching each OS's wording (as LrC does).
pub const REVEAL_LABEL: &str = if cfg!(target_os = "windows") {
    "Show in Explorer"
} else if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in File Manager"
};

/// Reveal a file (selected) or folder in the system file manager.
pub fn reveal(path: &Path) {
    let result = if cfg!(target_os = "windows") {
        if path.is_dir() {
            Command::new("explorer").arg(path).spawn()
        } else {
            Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
        }
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        Command::new("xdg-open").arg(dir).spawn()
    };
    if let Err(e) = result {
        log::warn!("could not reveal {}: {e}", path.display());
    }
}

/// Free space on the volume holding `path`, if it can be determined.
pub fn free_space(path: &Path) -> Option<u64> {
    fs4::available_space(path).ok()
}

/// Group a folder path under its volume, the way LrC's Folders panel does.
/// Returns (volume key, display name).
pub fn volume_of(path: &Path) -> (String, String) {
    let s = path.to_string_lossy();
    if cfg!(target_os = "windows") {
        let drive: String = s.chars().take_while(|c| *c != '\\').collect();
        return (format!("{drive}\\"), drive);
    }
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["Volumes", name, ..] => (format!("/Volumes/{name}"), (*name).to_owned()),
        ["media", user, name, ..] => (format!("/media/{user}/{name}"), (*name).to_owned()),
        ["mnt", name, ..] => (format!("/mnt/{name}"), (*name).to_owned()),
        _ => ("/".to_owned(), if cfg!(target_os = "macos") { "Macintosh HD" } else { "File System" }.to_owned()),
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn groups_by_volume() {
        if cfg!(target_os = "windows") {
            assert_eq!(volume_of(Path::new(r"D:\Photos\2026")).1, "D:");
        } else {
            assert_eq!(volume_of(Path::new("/Volumes/Archive/2026")).1, "Archive");
        }
    }
}
