//! Luminar Neo, opened the way its own Lightroom Classic plug-in opens it:
//! `Luminar Neo.exe lightroom <base64 photo path> -MPResPath <base64 result path>`.
//! Luminar opens the original (RAW included) in its plug-in mode; Apply writes the edit
//! to the result path, a TIFF next to the original, and Luminar exits. One photo per
//! launch: several would be taken as an HDR bracket.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{Launch, ResultName};

/// Luminar's result name for `photo.NEF`: `photo_LuminarNeo-edit.tif`.
pub const RESULT_SUFFIX: &str = "_LuminarNeo-edit";

/// An installed Luminar Neo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Luminar {
    pub exe: PathBuf,
    pub version: Option<String>,
}

impl Luminar {
    /// "Luminar Neo 1.29.0 (C:\…\Luminar Neo.exe)".
    pub fn describe(&self) -> String {
        let version = self.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        format!("Luminar Neo{version} ({})", self.exe.display())
    }

    /// One launch per photo, each waiting for the one before to close. Results are named
    /// like the plug-in names them, `_0`, `_1`… when the name is taken (on disk, or by an
    /// earlier photo of the same request).
    pub fn launches(&self, originals: &[(i64, PathBuf)]) -> Vec<Launch> {
        let mut taken: Vec<PathBuf> = Vec::new();
        let mut launches = Vec::new();
        for (id, path) in originals {
            if launches.iter().any(|l: &Launch| l.args.get(1) == Some(&encode_path(path))) {
                continue; // a virtual copy of a photo already in the list
            }
            let result = result_path(path, |p| p.exists() || taken.contains(&p.to_path_buf()));
            taken.push(result.clone());
            let dir = result.parent().map(Path::to_path_buf).unwrap_or_default();
            let name = result.file_name().unwrap_or_default().to_os_string();
            launches.push(Launch {
                editor: super::Editor::Luminar,
                program: self.exe.clone(),
                args: launch_args(path, &result),
                cwd: self.exe.parent().map(Path::to_path_buf),
                results: vec![(*id, dir, ResultName::Exact(PathBuf::from(name)))],
                wait_for_exit: true,
                then: Vec::new(),
                clear_first: Vec::new(),
                finish: Vec::new(),
            });
        }
        launches
    }
}

/// Luminar's arguments for editing `original` into `result`.
pub fn launch_args(original: &Path, result: &Path) -> Vec<OsString> {
    vec!["lightroom".into(), encode_path(original), "-MPResPath".into(), encode_path(result)]
}

fn encode_path(path: &Path) -> OsString {
    base64(path.to_string_lossy().as_bytes()).into()
}

/// `<dir>/<stem>_LuminarNeo-edit.tif`, or `…-edit_0.tif`, `…-edit_1.tif`… while `taken`.
pub fn result_path(original: &Path, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let dir = original.parent().unwrap_or(Path::new(""));
    let stem = original.file_stem().unwrap_or_default().to_string_lossy();
    let first = dir.join(format!("{stem}{RESULT_SUFFIX}.tif"));
    if !taken(&first) {
        return first;
    }
    (0..).map(|i| dir.join(format!("{stem}{RESULT_SUFFIX}_{i}.tif"))).find(|p| !taken(p)).unwrap_or(first)
}

/// Standard base64 with padding (what Lightroom's `LrStringUtils.encodeBase64` writes).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The program at `path` (or in the folder `path`).
pub fn at_location(path: &Path) -> Result<Luminar, String> {
    let exe = if path.is_dir() {
        ["Luminar Neo.exe", "Contents/MacOS/Luminar Neo"]
            .iter()
            .map(|n| path.join(n))
            .find(|p| p.is_file())
            .ok_or_else(|| format!("no Luminar Neo program in {}", path.display()))?
    } else {
        path.to_path_buf()
    };
    if !exe.is_file() {
        return Err(format!("{} not found", exe.display()));
    }
    Ok(Luminar { exe, version: installed_version() })
}

#[cfg(windows)]
fn installed_version() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let key = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Luminar Neo";
    let v: String =
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(key).ok()?.get_value("DisplayVersion").ok()?;
    // "1.29.0.18200": the last part is the build.
    Some(v.splitn(4, '.').take(3).collect::<Vec<_>>().join("."))
}

#[cfg(not(windows))]
fn installed_version() -> Option<String> {
    None
}

/// Where Luminar Neo installs.
fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            if let Ok(exe) = RegKey::predef(root)
                .open_subkey(r"Software\Skylum\Luminar Neo")
                .and_then(|k| k.get_value::<String, _>("ApplicationPath"))
            {
                v.push(PathBuf::from(exe));
            }
        }
        if let Ok(dir) = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Luminar Neo")
            .and_then(|k| k.get_value::<String, _>("InstallLocation"))
        {
            v.push(PathBuf::from(dir));
        }
        if let Some(base) = std::env::var_os("ProgramFiles") {
            v.push(PathBuf::from(base).join(r"Skylum\Luminar Neo"));
        }
    }
    #[cfg(target_os = "macos")]
    v.push(PathBuf::from("/Applications/Luminar Neo.app"));
    v
}

/// The Luminar Neo to use: the chosen location, else an installed one.
pub fn detect(configured: Option<&Path>) -> Result<Luminar, String> {
    if let Some(path) = configured {
        return at_location(path);
    }
    candidates()
        .iter()
        .filter(|p| p.exists())
        .find_map(|p| at_location(p).ok())
        .ok_or_else(|| "Luminar Neo was not found".to_owned())
}
