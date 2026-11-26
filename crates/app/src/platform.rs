//! OS integration helpers.

use std::path::{Path, PathBuf};
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
    /// Per volume: (free, total) bytes.
    values: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (u64, u64)>>>,
    refreshing: std::sync::Arc<std::sync::atomic::AtomicBool>,
    last_refresh: Option<std::time::Instant>,
}

impl FreeSpaceCache {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30);

    /// Cached (free, total) bytes for a volume key from [`volume_of`]; schedules a refresh
    /// when stale.
    pub fn get(&mut self, volume: &str) -> Option<(u64, u64)> {
        let value = self.values.lock().unwrap().get(volume).copied();
        let stale = self.last_refresh.is_none_or(|t| t.elapsed() > Self::MAX_AGE) || value.is_none();
        if stale && !self.refreshing.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.last_refresh = Some(std::time::Instant::now());
            let (values, refreshing, key) = (self.values.clone(), self.refreshing.clone(), volume.to_owned());
            std::thread::spawn(move || {
                let path = Path::new(&key);
                if let Ok(free) = fs4::available_space(path) {
                    let total = fs4::total_space(path).unwrap_or(0);
                    values.lock().unwrap().insert(key, (free, total));
                }
                refreshing.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
        value
    }
}

/// Group a folder path under its volume, the way LrC's Folders panel does.
/// Returns (volume key, display name). On Windows a network share
/// (`\\server\share\…`, also written `\\?\UNC\server\share\…`) is a volume of its own,
/// named "share (server)"; its key is the share's root, for free space.
pub fn volume_of(path: &Path) -> (String, String) {
    volume_and_parts(path).0
}

/// The folders of `path` below its volume ([`volume_of`]), outermost first.
pub fn parts_below_volume(path: &Path) -> Vec<String> {
    volume_and_parts(path).1
}

fn volume_and_parts(path: &Path) -> ((String, String), Vec<String>) {
    let s = path.to_string_lossy();
    if cfg!(target_os = "windows") {
        let split = |rest: &str| -> Vec<String> {
            rest.split(['\\', '/']).filter(|p| !p.is_empty()).map(str::to_owned).collect()
        };
        // `\\?\` (long paths) is not part of the name.
        let plain = s.strip_prefix(r"\\?\").unwrap_or(&s);
        let unc = match plain.strip_prefix(r"UNC\") {
            Some(rest) => Some(rest),
            None if !s.starts_with(r"\\?\") => s.strip_prefix(r"\\"),
            None => None,
        };
        if let Some(unc) = unc {
            let mut parts = split(unc).into_iter();
            let server = parts.next().unwrap_or_default();
            let share = parts.next().unwrap_or_default();
            let name = if share.is_empty() { server.clone() } else { format!("{share} ({server})") };
            return ((format!(r"\\{server}\{share}\"), name), parts.collect());
        }
        let drive: String = plain.chars().take_while(|c| *c != '\\').collect();
        let rest = plain.get(drive.len()..).unwrap_or_default();
        return ((format!("{drive}\\"), drive), split(rest));
    }
    let all: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    let (volume, skip) = match all.as_slice() {
        ["Volumes", name, ..] => ((format!("/Volumes/{name}"), (*name).to_owned()), 2),
        ["media", user, name, ..] => ((format!("/media/{user}/{name}"), (*name).to_owned()), 3),
        ["mnt", name, ..] => ((format!("/mnt/{name}"), (*name).to_owned()), 2),
        _ => (
            (
                "/".to_owned(),
                if cfg!(target_os = "macos") { "Macintosh HD" } else { "File System" }.to_owned(),
            ),
            0,
        ),
    };
    (volume, all[skip.min(all.len())..].iter().map(|p| (*p).to_owned()).collect())
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

/// A folder picker that is open (or has just closed); see [`pick_folder`].
pub struct FolderPick(std::sync::mpsc::Receiver<Option<PathBuf>>);

impl FolderPick {
    /// `None` while the picker is still open; then the chosen folder, if any.
    pub fn poll(&self) -> Option<Option<PathBuf>> {
        match self.0.try_recv() {
            Ok(choice) => Some(choice),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
        }
    }
}

/// Open the system folder picker without blocking the window. On Linux the picker is a
/// separate program (the desktop portal), and the window must keep answering the
/// compositor while it is open: browsing a slow network share can take a while, and a
/// window that stops answering gets the "close or wait" prompt. Elsewhere the picker
/// must run on the main thread, so it is modal there as before.
pub fn pick_folder(ctx: &eframe::egui::Context, title: &str) -> FolderPick {
    let (tx, rx) = std::sync::mpsc::channel();
    let dialog = rfd::FileDialog::new().set_title(title);
    if cfg!(target_os = "linux") {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_folder());
            ctx.request_repaint();
        });
    } else {
        let _ = tx.send(dialog.pick_folder());
    }
    FolderPick(rx)
}

/// A drive or mounted volume offered as an import source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub path: PathBuf,
    pub name: String,
    /// Has a `DCIM` folder: a camera card (LrC lists these as Devices).
    pub card: bool,
}

/// Mounted volumes, cards first. Can be slow (network drives): call off the UI thread.
pub fn volumes() -> Vec<Volume> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        // A: and B: are legacy floppy letters; probing them can stall.
        roots.extend(('C'..='Z').map(|l| PathBuf::from(format!("{l}:\\"))));
    } else {
        let mut dirs = vec![PathBuf::from("/Volumes"), PathBuf::from("/mnt")];
        if let Ok(user) = std::env::var("USER") {
            dirs.push(PathBuf::from("/media").join(&user));
            dirs.push(PathBuf::from("/run/media").join(&user));
        }
        for d in dirs {
            if let Ok(entries) = std::fs::read_dir(d) {
                roots.extend(entries.flatten().map(|e| e.path()));
            }
        }
    }
    let mut out: Vec<Volume> = roots
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|path| {
            let name =
                path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into());
            let name = name.trim_end_matches('\\').to_owned();
            Volume { card: path.join("DCIM").is_dir(), name, path }
        })
        .collect();
    out.sort_by_key(|v| (!v.card, v.name.to_lowercase()));
    out
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
            assert_eq!(parts_below_volume(Path::new(r"D:\Photos\2026")), ["Photos", "2026"]);
            assert_eq!(parts_below_volume(Path::new(r"\\?\D:\Photos")), ["Photos"]);
            // A network share, as written either way: one volume, with its own free space.
            for p in [r"\\?\UNC\nas\Media\2026 bilder\Trip", r"\\nas\Media\2026 bilder\Trip"] {
                assert_eq!(
                    volume_of(Path::new(p)),
                    (r"\\nas\Media\".to_owned(), "Media (nas)".to_owned()),
                    "{p}"
                );
                assert_eq!(parts_below_volume(Path::new(p)), ["2026 bilder", "Trip"]);
            }
        } else {
            assert_eq!(volume_of(Path::new("/Volumes/Archive/2026")).1, "Archive");
            assert_eq!(parts_below_volume(Path::new("/Volumes/Archive/2026/x")), ["2026", "x"]);
            assert_eq!(parts_below_volume(Path::new("/home/me/Pictures")), ["home", "me", "Pictures"]);
        }
    }
}
