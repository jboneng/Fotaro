//! Downloading and updating the Lensfun database, as `lensfun-update-data` does: the
//! repository's `versions.json` (`[timestamp, [versions], [mirrors]]`) says what there is,
//! and `version_<n>.tar.bz2` holds that version's XML files.
//!
//! The files go to `<dir>/version_2/` with a `timestamp.txt`; a new download replaces them
//! only after it has been read successfully.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::Database;

/// Where the database is published, then the mirror `lensfun-update-data` knows.
const LOCATIONS: [&str; 2] = ["https://lensfun.github.io/db/", "https://wilson.bronger.org/lensfun-db/"];
/// The database version read here (Lensfun 0.3.95's; version 1 reads too).
const VERSION: u32 = 2;
/// A download larger than this is not a lens database.
const MAX_BYTES: u64 = 64 << 20;

/// The folder holding the XML files under `dir`.
pub fn folder(dir: &Path) -> PathBuf {
    dir.join(format!("version_{VERSION}"))
}

/// Is a database there?
pub fn ready(dir: &Path) -> bool {
    std::fs::read_dir(folder(dir))
        .is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "xml"))))
}

/// When the database there was made (Unix seconds).
pub fn timestamp(dir: &Path) -> Option<i64> {
    std::fs::read_to_string(folder(dir).join("timestamp.txt")).ok()?.trim().parse().ok()
}

/// What is published: (timestamp, base URL), from the first location that answers.
pub fn published() -> Result<(i64, String)> {
    let mut last = None;
    for base in LOCATIONS {
        let url = format!("{base}versions.json");
        match ureq::get(&url).call().and_then(|mut r| r.body_mut().read_to_string()) {
            Ok(body) => {
                let v: serde_json::Value = serde_json::from_str(&body).context("versions.json")?;
                let ts =
                    v.get(0).and_then(serde_json::Value::as_i64).context("versions.json: no timestamp")?;
                let has = v
                    .get(1)
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|a| a.iter().any(|x| x.as_u64() == Some(u64::from(VERSION))));
                anyhow::ensure!(has, "the lens database is no longer published in version {VERSION}");
                return Ok((ts, base.to_string()));
            }
            Err(e) => last = Some(anyhow::anyhow!("{url}: {e}")),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no lens database location answered")))
}

/// Download the database into `dir` (replacing what is there once the new one reads);
/// `progress` gets 0..1. Returns the new database.
pub fn download(dir: &Path, progress: &mut dyn FnMut(f32)) -> Result<Database> {
    let (ts, base) = published()?;
    let url = format!("{base}version_{VERSION}.tar.bz2");
    let mut resp = ureq::get(&url).call().with_context(|| format!("downloading {url}"))?;
    let total = resp.body().content_length().unwrap_or(500_000).max(1);
    let mut bytes = Vec::new();
    let mut reader = resp.body_mut().with_config().limit(MAX_BYTES).reader();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        progress((bytes.len() as f32 / total as f32).min(0.95));
    }
    let staging = dir.join(format!("version_{VERSION}.new"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    unpack(&bytes, &staging)?;
    std::fs::write(staging.join("timestamp.txt"), ts.to_string())?;
    // Only a database that reads replaces the old one.
    let db = Database::load(&staging).context("the downloaded lens database doesn't read")?;
    let target = folder(dir);
    let old = dir.join(format!("version_{VERSION}.old"));
    let _ = std::fs::remove_dir_all(&old);
    if target.exists() {
        std::fs::rename(&target, &old)?;
    }
    std::fs::rename(&staging, &target)?;
    let _ = std::fs::remove_dir_all(&old);
    progress(1.0);
    log::info!("lens database {ts}: {} cameras, {} lenses", db.cameras.len(), db.lenses.len());
    Ok(db)
}

/// Unpack the `.tar.bz2`'s XML files (by their names only, nothing outside `to`).
fn unpack(bytes: &[u8], to: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(bzip2::read::BzDecoder::new(bytes));
    let mut n = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.into_owned();
        let Some(name) = path.file_name().map(|s| s.to_string_lossy().into_owned()) else { continue };
        let safe = name.ends_with(".xml") && !name.starts_with('.') && !name.contains(['/', '\\']);
        if !safe {
            continue;
        }
        let mut data = Vec::new();
        entry.by_ref().take(MAX_BYTES).read_to_end(&mut data)?;
        std::fs::write(to.join(&name), data)?;
        n += 1;
    }
    anyhow::ensure!(n > 0, "the download held no lens data");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_unpacks_only_xml_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        // A small .tar.bz2 with an XML file, a file in a folder, and something else.
        let mut tar = tar::Builder::new(Vec::new());
        let mut add = |name: &str, data: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, name, data).unwrap();
        };
        add("slr-nikon.xml", crate::xml::FIXTURE.as_bytes());
        add("sub/compact.xml", b"<lensdatabase version=\"2\"/>");
        add("readme.txt", b"x");
        let raw = tar.into_inner().unwrap();
        let mut bz = Vec::new();
        std::io::Read::read_to_end(
            &mut bzip2::read::BzEncoder::new(&raw[..], bzip2::Compression::fast()),
            &mut bz,
        )
        .unwrap();
        unpack(&bz, tmp.path()).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into())
            .collect();
        names.sort();
        assert_eq!(names, ["compact.xml", "slr-nikon.xml"]);
        let db = Database::load(tmp.path()).unwrap();
        assert_eq!(db.lenses.len(), 4);
    }
}
