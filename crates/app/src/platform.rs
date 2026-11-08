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

/// Free space per volume, refreshed on a background thread. Querying the OS can
/// block for hundreds of milliseconds while the disk is busy, so it must never
/// happen while drawing.
#[derive(Default)]
pub struct FreeSpaceCache {
    values: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u64>>>,
    refreshing: std::sync::Arc<std::sync::atomic::AtomicBool>,
    last_refresh: Option<std::time::Instant>,
}

impl FreeSpaceCache {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30);

    /// Cached free bytes for a volume key from [`volume_of`]; schedules a refresh when stale.
    pub fn get(&mut self, volume: &str) -> Option<u64> {
        let value = self.values.lock().unwrap().get(volume).copied();
        let stale = self.last_refresh.is_none_or(|t| t.elapsed() > Self::MAX_AGE) || value.is_none();
        if stale && !self.refreshing.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.last_refresh = Some(std::time::Instant::now());
            let (values, refreshing, key) = (self.values.clone(), self.refreshing.clone(), volume.to_owned());
            std::thread::spawn(move || {
                if let Ok(free) = fs4::available_space(Path::new(&key)) {
                    values.lock().unwrap().insert(key, free);
                }
                refreshing.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
        value
    }
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
        _ => (
            "/".to_owned(),
            if cfg!(target_os = "macos") { "Macintosh HD" } else { "File System" }.to_owned(),
        ),
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

/// Is Caps Lock on? LrC uses it as a temporary "Auto Advance". Windows and macOS ask the
/// OS directly; elsewhere this is always false (use Photo ▸ Auto Advance instead).
pub fn caps_lock_on() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetKeyState(virtual_key: i32) -> i16;
        }
        const VK_CAPITAL: i32 = 0x14;
        // SAFETY: GetKeyState takes a plain key code and has no other preconditions.
        // The low bit is the toggle state.
        unsafe { GetKeyState(VK_CAPITAL) & 1 != 0 }
    }
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventSourceFlagsState(state_id: i32) -> u64;
        }
        const COMBINED_SESSION_STATE: i32 = 0;
        const ALPHA_SHIFT_MASK: u64 = 0x0001_0000;
        // SAFETY: plain function of an enum value; returns the current modifier flags.
        unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) & ALPHA_SHIFT_MASK != 0 }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
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
