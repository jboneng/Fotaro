//! Where a file is kept decides how it is best read. On a local disk a memory map is the
//! fastest way to decode; on a network share (SMB, NFS, and anything mounted through
//! FUSE) a mapped file is read by page faults, each its own round trip to the server,
//! and on Linux a fault that waits on the network can hold up the whole program. Files
//! there are read with ordinary reads instead.
//!
//! `FOTARO_IO=stream` (or `mmap`) forces one way everywhere, to compare the two.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// 0: not looked at yet; 1: decide per folder; 2: always map; 3: always read.
static MODE: AtomicU8 = AtomicU8::new(0);

/// Read every file one way, whatever it is stored on (`Some(true)`: ordinary reads,
/// `Some(false)`: memory maps), or decide per folder again (`None`). For comparing the two.
pub fn force(streamed: Option<bool>) {
    MODE.store(
        match streamed {
            None => 1,
            Some(false) => 2,
            Some(true) => 3,
        },
        Ordering::Relaxed,
    );
}

/// What is forced, by `FOTARO_IO` or [`force`].
fn forced() -> Option<bool> {
    if MODE.load(Ordering::Relaxed) == 0 {
        force(match std::env::var("FOTARO_IO").ok().map(|v| v.to_lowercase()).as_deref() {
            Some("stream" | "read") => Some(true),
            Some("mmap" | "map") => Some(false),
            Some(other) => {
                log::warn!("FOTARO_IO={other}: expected `stream` or `mmap`");
                None
            }
            None => None,
        });
    }
    match MODE.load(Ordering::Relaxed) {
        2 => Some(false),
        3 => Some(true),
        _ => None,
    }
}

/// Is `path` on storage that should be read with ordinary reads rather than mapped?
/// Asked once per folder (the answer for a folder is kept).
pub fn is_remote(path: &Path) -> bool {
    if let Some(forced) = forced() {
        return forced;
    }
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let Some(dir) = path.parent() else { return false };
    let known = KNOWN.get_or_init(Default::default);
    if let Some(remote) = known.lock().unwrap_or_else(|e| e.into_inner()).get(dir) {
        return *remote;
    }
    let remote = dir_is_remote(dir);
    if remote {
        log::info!("{} is on network storage: reading its files without a memory map", dir.display());
    }
    let mut known = known.lock().unwrap_or_else(|e| e.into_inner());
    // A long session can visit many folders; the answers are cheap to ask for again.
    if known.len() > 4096 {
        known.clear();
    }
    known.insert(dir.to_path_buf(), remote);
    remote
}

/// Open a RAW file the way its storage is best read (see the module's notes).
pub(crate) fn open_raw(path: &Path) -> libraw::Result<libraw::RawFile> {
    if is_remote(path) { libraw::RawFile::open_streamed(path) } else { libraw::RawFile::open(path) }
}

#[cfg(target_os = "linux")]
fn dir_is_remote(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    // From linux/magic.h (and the cifs and smb3 clients' own).
    const REMOTE: [i64; 8] = [
        0xFF53_4D42, // CIFS
        0xFE53_4D42, // SMB2 / SMB3
        0x517B,      // smbfs (old)
        0x6969,      // NFS
        0x6573_5546, // FUSE: sshfs, gvfs (shares opened from the file manager), rclone...
        0x0102_1997, // 9p
        0x00C3_6400, // Ceph
        0x5346_414F, // AFS
    ];
    let Ok(c_path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: statfs fills the zeroed struct for a NUL-terminated path.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return false;
    }
    // `f_type`'s integer type differs between targets; compare the low 32 bits.
    let kind = (st.f_type as i64) & 0xFFFF_FFFF;
    REMOTE.contains(&kind)
}

#[cfg(target_os = "macos")]
fn dir_is_remote(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: statfs fills the zeroed struct for a NUL-terminated path.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return false;
    }
    // SAFETY: the kernel NUL-terminates the type name.
    let name = unsafe { std::ffi::CStr::from_ptr(st.f_fstypename.as_ptr()) }.to_string_lossy().to_lowercase();
    ["smbfs", "nfs", "afpfs", "webdav", "macfuse", "osxfuse"].iter().any(|n| name.starts_with(n))
}

/// Windows serves a mapped file on a share through its cache and does it well; other
/// systems are not known to need this.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn dir_is_remote(_dir: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_folders_are_not_remote() {
        if forced().is_some() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.jpg");
        assert!(!is_remote(&file), "the temp folder is on a local disk");
        assert!(!is_remote(&file), "and the kept answer is the same");
        assert!(!is_remote(Path::new("/")), "no parent: local");
    }
}
