//! Updates: ask GitHub for Fotaro's latest release, download the installer made for this
//! system, and run it once Fotaro has closed.
//!
//! A release's installers are named `Fotaro-<version>-<os>-<arch>…`:
//! `Fotaro-0.6.0-windows-x86_64-setup.exe`, `Fotaro-0.6.0-macos-aarch64.dmg` (or
//! `-macos-universal.dmg`), `Fotaro-0.6.0-linux-x86_64.AppImage`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;
use sha2::Digest;

/// Where releases are published.
const LATEST: &str = "https://api.github.com/repos/jboneng/Fotaro/releases/latest";
/// An installer larger than this is not one.
const MAX_BYTES: u64 = 1 << 30;
/// How often a running Fotaro looks again.
const RECHECK: Duration = Duration::from_secs(24 * 60 * 60);

/// A published release newer than this Fotaro.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// Its version, as `0.6.0`.
    pub version: String,
    /// The release's notes (Markdown, as written on GitHub).
    pub notes: String,
    /// The release's page.
    pub page: String,
    /// The installer for this system, if the release has one.
    pub asset: Option<Asset>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// `sha256:<hex>`, when GitHub gives it.
    pub digest: Option<String>,
}

/// `v0.6.0`, `0.6` → (0, 6, 0). Pre-releases (`0.6.0-beta`) and other text: `None`.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut parts = s.split('.');
    let mut next = || -> Option<u64> { parts.next().map_or(Some(0), |p| p.parse().ok()) };
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

/// Is `version` newer than `current`?
pub fn is_newer(version: &str, current: &str) -> bool {
    matches!((parse_version(version), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// The installer for `os` / `arch` (as `std::env::consts` names them) among a release's
/// files, by the names described at the top.
pub fn pick_asset<'a>(names: impl IntoIterator<Item = &'a str>, os: &str, arch: &str) -> Option<&'a str> {
    let ending = match os {
        "windows" => "-setup.exe",
        "macos" => ".dmg",
        "linux" => ".appimage",
        _ => return None,
    };
    let archs: &[&str] = match arch {
        "x86_64" => &["x86_64", "x64", "amd64"],
        "aarch64" => &["aarch64", "arm64"],
        _ => &[],
    };
    let mut fallback = None;
    for name in names {
        let lower = name.to_lowercase();
        if !lower.starts_with("fotaro-") || !lower.ends_with(ending) || !lower.contains(&format!("-{os}-")) {
            continue;
        }
        if archs.iter().any(|a| lower.contains(&format!("-{a}"))) {
            return Some(name);
        }
        if os == "macos" && lower.contains("-universal") {
            fallback = Some(name);
        }
    }
    fallback
}

/// Read the latest release from GitHub's answer; `None` if it is not newer than `current`.
fn read_release(json: &serde_json::Value, current: &str) -> Option<Release> {
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let tag = json["tag_name"].as_str()?;
    if !is_newer(tag, current) {
        return None;
    }
    let assets = json["assets"].as_array().map(Vec::as_slice).unwrap_or_default();
    let names = assets.iter().filter_map(|a| a["name"].as_str());
    let asset = pick_asset(names, std::env::consts::OS, std::env::consts::ARCH).and_then(|name| {
        let a = assets.iter().find(|a| a["name"].as_str() == Some(name))?;
        Some(Asset {
            name: name.to_owned(),
            url: a["browser_download_url"].as_str()?.to_owned(),
            size: a["size"].as_u64().unwrap_or(0),
            digest: a["digest"].as_str().map(str::to_owned),
        })
    });
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_owned(),
        notes: json["body"].as_str().unwrap_or_default().trim().to_owned(),
        page: json["html_url"].as_str().unwrap_or("https://github.com/jboneng/Fotaro/releases").to_owned(),
        asset,
    })
}

fn user_agent() -> String {
    format!("{}/{}", crate::APP_NAME, crate::VERSION)
}

/// Ask GitHub for the latest release; `None`: this Fotaro is up to date.
fn latest() -> Result<Option<Release>> {
    let response = ureq::get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", &user_agent())
        .call();
    let body = match response {
        Ok(mut r) => r.body_mut().read_to_string().context("reading the release")?,
        // No release published yet.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).context("asking GitHub for the latest release"),
    };
    let json: serde_json::Value = serde_json::from_str(&body).context("GitHub's answer")?;
    Ok(read_release(&json, crate::VERSION))
}

/// Where installers are downloaded to.
fn download_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", crate::APP_NAME).map(|d| d.data_local_dir().join("Updates"))
}

/// Does the file at `path` have `digest` (`sha256:<hex>`)? Without one, any file does.
fn matches_digest(path: &Path, digest: Option<&str>) -> Result<bool> {
    let Some(want) = digest.and_then(|d| d.strip_prefix("sha256:")) else { return Ok(true) };
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(got.eq_ignore_ascii_case(want))
}

/// Download `asset` into the updates folder (kept if it is already there and whole);
/// `progress` gets 0..1.
fn download(asset: &Asset, progress: &mut dyn FnMut(f32)) -> Result<PathBuf> {
    let dir = download_dir().context("no folder for downloads")?;
    std::fs::create_dir_all(&dir)?;
    // Old installers go; only this one's name is kept.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy() != asset.name {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let name = Path::new(&asset.name).file_name().context("the installer has no name")?;
    let path = dir.join(name);
    let whole = |p: &Path| {
        p.metadata().is_ok_and(|m| asset.size == 0 || m.len() == asset.size)
            && matches_digest(p, asset.digest.as_deref()).unwrap_or(false)
    };
    if whole(&path) {
        progress(1.0);
        return Ok(path);
    }

    let mut response = ureq::get(&asset.url)
        .header("User-Agent", &user_agent())
        .call()
        .with_context(|| format!("downloading {}", asset.name))?;
    let total = response.body().content_length().unwrap_or(asset.size).max(1);
    let part = path.with_extension("part");
    let mut out = std::fs::File::create(&part)?;
    let mut reader = response.body_mut().with_config().limit(MAX_BYTES).reader();
    let (mut done, mut buf) = (0u64, vec![0u8; 256 * 1024]);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress((done as f32 / total as f32).min(0.99));
    }
    out.sync_all()?;
    drop(out);
    if asset.size > 0 && done != asset.size {
        bail!("the download stopped early ({done} of {} bytes)", asset.size);
    }
    if !matches_digest(&part, asset.digest.as_deref())? {
        let _ = std::fs::remove_file(&part);
        bail!("the downloaded installer is damaged (its checksum doesn't match)");
    }
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&part, &path)?;
    progress(1.0);
    Ok(path)
}

/// Was this Fotaro put here by its installer (rather than built and run from source)?
pub fn installed() -> bool {
    cfg!(windows)
        && std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("unins000.exe").is_file()))
            == Some(true)
}

/// How an update was started.
pub enum Started {
    /// The installer runs once Fotaro has closed, and starts the new Fotaro: close now.
    AfterClose,
    /// The installer (or the downloaded file) was opened; Fotaro keeps running.
    Opened,
}

/// Start installing the downloaded `installer`.
fn start_install(installer: &Path) -> Result<Started> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if installed() {
            // Wait for this Fotaro to close (it still saves the catalog and settings), then
            // update quietly; the installer starts the new version when it is done.
            let quoted = installer.display().to_string().replace('\'', "''");
            let script = format!(
                "Wait-Process -Id {} -ErrorAction SilentlyContinue; \
                 Start-Process -FilePath '{quoted}' -ArgumentList '/SILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-'",
                std::process::id()
            );
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .context("starting the installer")?;
            return Ok(Started::AfterClose);
        }
        // Built from source: offer the installer as is, and leave this copy alone.
        std::process::Command::new(installer).spawn().context("starting the installer")?;
        Ok(Started::Opened)
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(installer).spawn().context("opening the disk image")?;
        Ok(Started::Opened)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        crate::platform::reveal(installer);
        Ok(Started::Opened)
    }
}

/// Where the updater is.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Checking,
    /// A newer release is out.
    Available(Release),
    Downloading(Release),
    /// Downloaded: installs on the next click.
    Ready(Release, PathBuf),
}

enum Msg {
    Checked(Result<Option<Release>, String>),
    Downloaded(Result<PathBuf, String>),
}

pub struct Updater {
    pub state: State,
    /// The last download's error, shown on the indicator.
    pub error: Option<String>,
    rx: Option<Receiver<Msg>>,
    /// Downloading: how far, in thousandths.
    progress: Arc<AtomicU32>,
    /// The check was asked for (Help ▸ Check for Updates): say what it found.
    manual: bool,
    last_check: Option<Instant>,
}

impl Default for Updater {
    fn default() -> Self {
        Updater {
            state: State::Idle,
            error: None,
            rx: None,
            progress: Arc::default(),
            manual: false,
            last_check: None,
        }
    }
}

impl Updater {
    /// Ask GitHub for the latest release (`manual`: the user asked, so the answer is shown
    /// whatever it is).
    pub fn check(&mut self, ctx: &egui::Context, manual: bool) {
        if !matches!(self.state, State::Idle | State::Available(_)) || self.rx.is_some() {
            return;
        }
        self.manual = manual;
        self.last_check = Some(Instant::now());
        let (tx, rx) = unbounded();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = latest().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Checked(r));
            ctx.request_repaint();
        });
        if manual {
            self.state = State::Checking;
        }
        self.rx = Some(rx);
    }

    /// Check again if a day has gone by since the last check.
    pub fn check_daily(&mut self, ctx: &egui::Context) {
        if self.last_check.is_none_or(|t| t.elapsed() > RECHECK) {
            self.check(ctx, false);
        }
        ctx.request_repaint_after(RECHECK);
    }

    /// Download the available release's installer.
    pub fn download(&mut self, ctx: &egui::Context) {
        let State::Available(release) = &self.state else { return };
        let Some(asset) = release.asset.clone() else { return };
        self.progress.store(0, Ordering::Relaxed);
        self.error = None;
        let (tx, rx) = unbounded();
        let (ctx, progress) = (ctx.clone(), self.progress.clone());
        std::thread::spawn(move || {
            let r = download(&asset, &mut |f| {
                let f = (f.clamp(0.0, 1.0) * 1000.0) as u32;
                // A repaint per tenth of a percent at most.
                if progress.swap(f, Ordering::Relaxed) != f {
                    ctx.request_repaint();
                }
            });
            let _ = tx.send(Msg::Downloaded(r.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
        self.state = State::Downloading(release.clone());
        self.rx = Some(rx);
    }

    /// How far the download is (0..1).
    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// Install the downloaded update. `AfterClose`: Fotaro should close now.
    pub fn install(&mut self) -> Result<Started, String> {
        let State::Ready(_, path) = &self.state else { return Err("No update has been downloaded".into()) };
        start_install(path).map_err(|e| format!("{e:#}"))
    }

    /// Take a finished check or download; returns a message to show, if any. A release
    /// the user chose to skip (`skipped`) is not offered unless they asked.
    pub fn poll(&mut self, skipped: Option<&str>) -> Option<String> {
        let msg = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        let manual = std::mem::take(&mut self.manual);
        match msg {
            Msg::Checked(Ok(Some(release))) => {
                let offered = manual || skipped != Some(release.version.as_str());
                log::info!("{} {} is available", crate::APP_NAME, release.version);
                if offered {
                    self.state = State::Available(release);
                } else if self.state == State::Checking {
                    self.state = State::Idle;
                }
                None
            }
            Msg::Checked(Ok(None)) => {
                self.state = State::Idle;
                manual.then(|| format!("{} {} is up to date", crate::APP_NAME, crate::VERSION))
            }
            Msg::Checked(Err(e)) => {
                log::warn!("checking for updates: {e}");
                if self.state == State::Checking {
                    self.state = State::Idle;
                }
                manual.then(|| format!("Could not check for updates: {e}"))
            }
            Msg::Downloaded(r) => {
                let State::Downloading(release) = std::mem::replace(&mut self.state, State::Idle) else {
                    return None;
                };
                match r {
                    Ok(path) => {
                        log::info!("downloaded {}", path.display());
                        self.state = State::Ready(release, path);
                        None
                    }
                    Err(e) => {
                        log::warn!("downloading the update: {e}");
                        self.state = State::Available(release);
                        self.error = Some(e.clone());
                        Some(format!("Could not download the update: {e}"))
                    }
                }
            }
        }
    }

    /// Forget the offered release (Skip This Version).
    pub fn dismiss(&mut self) {
        if matches!(self.state, State::Available(_)) {
            self.state = State::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.6.0"), Some((0, 6, 0)));
        assert_eq!(parse_version("0.5"), Some((0, 5, 0)));
        assert_eq!(parse_version("1"), Some((1, 0, 0)));
        assert_eq!(parse_version("0.6.0-beta.1"), None);
        assert_eq!(parse_version("0.6.0.1"), None);
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer("v0.6.0", "0.5.0"));
        assert!(is_newer("0.10.0", "0.9.3"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("v0.5.0", "0.5.0"));
        assert!(!is_newer("0.4.9", "0.5.0"));
        assert!(!is_newer("0.6.0-rc1", "0.5.0"));
    }

    #[test]
    fn picks_the_installer_for_the_system() {
        let names = [
            "Fotaro-0.6.0-windows-x86_64-setup.exe",
            "Fotaro-0.6.0-windows-aarch64-setup.exe",
            "Fotaro-0.6.0-macos-universal.dmg",
            "Fotaro-0.6.0-linux-x86_64.AppImage",
            "Fotaro-0.6.0-source.tar.gz",
            "checksums.txt",
        ];
        let pick = |os, arch| pick_asset(names, os, arch);
        assert_eq!(pick("windows", "x86_64"), Some("Fotaro-0.6.0-windows-x86_64-setup.exe"));
        assert_eq!(pick("windows", "aarch64"), Some("Fotaro-0.6.0-windows-aarch64-setup.exe"));
        assert_eq!(pick("macos", "aarch64"), Some("Fotaro-0.6.0-macos-universal.dmg"));
        assert_eq!(pick("linux", "x86_64"), Some("Fotaro-0.6.0-linux-x86_64.AppImage"));
        assert_eq!(pick("linux", "aarch64"), None);
        assert_eq!(pick("freebsd", "x86_64"), None);
        assert_eq!(pick_asset(["Fotaro-0.6.0-windows-x86_64.zip"], "windows", "x86_64"), None);
    }

    #[test]
    fn reads_github_releases() {
        let json = serde_json::json!({
            "tag_name": "v0.6.0",
            "html_url": "https://github.com/jboneng/Fotaro/releases/tag/v0.6.0",
            "body": "  What's new\n",
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": format!("Fotaro-0.6.0-{}-{}{}", std::env::consts::OS, std::env::consts::ARCH,
                    if cfg!(windows) { "-setup.exe" } else if cfg!(target_os = "macos") { ".dmg" } else { ".AppImage" }),
                "browser_download_url": "https://example.invalid/installer",
                "size": 1234,
                "digest": "sha256:00"
            }]
        });
        let r = read_release(&json, "0.5.0").unwrap();
        assert_eq!(r.version, "0.6.0");
        assert_eq!(r.notes, "What's new");
        let asset = r.asset.unwrap();
        assert_eq!((asset.size, asset.digest.as_deref()), (1234, Some("sha256:00")));
        assert_eq!(read_release(&json, "0.6.0"), None);
        let mut pre = json.clone();
        pre["prerelease"] = true.into();
        assert_eq!(read_release(&pre, "0.5.0"), None);
    }

    #[test]
    fn checks_digests() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f");
        std::fs::write(&p, "abc").unwrap();
        let abc = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(matches_digest(&p, Some(abc)).unwrap());
        assert!(!matches_digest(&p, Some("sha256:00")).unwrap());
        assert!(matches_digest(&p, None).unwrap());
    }
}
