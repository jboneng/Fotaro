//! How long reading photos from a folder takes with and without a memory map:
//!
//!   cargo run -p imaging --example sharebench --release -- <folder> [max files] [skip]
//!
//! For a folder on a network share. Files are read two through a memory map, then two with
//! ordinary reads, and so on (in twos, so a folder of pairs such as scan + infrared gives
//! both ways both kinds), and each file is read cold exactly once; per file the steps
//! are the ones an import makes: read the metadata, make a thumbnail, build the preview.
//! Run it on files that have not just been read (the system keeps what it read): `skip`
//! passes over that many files first, for a second run in the same folder.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use imaging::{PreviewSize, SourceKind};

fn kind(path: &Path) -> Option<SourceKind> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "nef" | "dng" | "cr2" | "cr3" | "arw" | "raf" | "orf" | "rw2" | "pef" | "srw" | "nrw" => {
            Some(SourceKind::Raw)
        }
        "jpg" | "jpeg" | "tif" | "tiff" | "png" | "webp" => Some(SourceKind::Image),
        _ => None,
    }
}

fn collect(dir: &Path, out: &mut Vec<(PathBuf, SourceKind)>) {
    let mut entries: Vec<PathBuf> =
        std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if let Some(k) = kind(&p) {
            out.push((p, k));
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Times {
    files: u32,
    bytes: u64,
    metadata: Duration,
    thumb: Duration,
    preview: Duration,
    failed: u32,
}

fn timed<T>(total: &mut Duration, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    *total += t.elapsed();
    r
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(folder) = args.next().map(PathBuf::from) else {
        eprintln!("usage: sharebench <folder> [max files] [skip]");
        std::process::exit(2);
    };
    let max: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(40);
    let skip: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let t = Instant::now();
    let mut files = Vec::new();
    collect(&folder, &mut files);
    println!("{} photos under {} (listed in {:.2?})", files.len(), folder.display(), t.elapsed());
    let files: Vec<_> = files.into_iter().skip(skip).take(max).collect();
    imaging::storage::force(None);
    let auto = files.first().is_some_and(|(p, _)| imaging::storage::is_remote(p));
    println!(
        "Fotaro reads this folder {} by itself",
        if auto { "with ordinary reads (network storage)" } else { "through memory maps (local storage)" }
    );

    // [raw, image] x [mapped, streamed]
    let mut times = [[Times::default(); 2]; 2];
    for (i, (path, kind)) in files.iter().enumerate() {
        let streamed = (i / 2) % 2 == 1;
        imaging::storage::force(Some(streamed));
        let t = &mut times[usize::from(*kind == SourceKind::Image)][usize::from(streamed)];
        t.files += 1;
        t.bytes += std::fs::metadata(path).map_or(0, |m| m.len());
        let before = (t.metadata, t.thumb, t.preview);
        let a = timed(&mut t.metadata, || imaging::read_metadata(path, *kind).is_ok());
        let b = timed(&mut t.thumb, || imaging::decode(path, *kind, PreviewSize::Thumb.max_px()).is_ok());
        let c =
            timed(&mut t.preview, || imaging::decode(path, *kind, PreviewSize::Standard.max_px()).is_ok());
        if !(a && b && c) {
            t.failed += 1;
        }
        println!(
            "  {:8} {:>7.0?} {:>7.0?} {:>7.0?}  {}",
            if streamed { "read" } else { "mapped" },
            t.metadata - before.0,
            t.thumb - before.1,
            t.preview - before.2,
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    println!("\nMean per file          files   size   metadata  thumbnail    preview      total");
    for (k, name) in ["RAW", "image"].iter().enumerate() {
        for (m, mode) in ["memory map", "ordinary reads"].iter().enumerate() {
            let t = times[k][m];
            if t.files == 0 {
                continue;
            }
            let per = |d: Duration| d / t.files;
            println!(
                "{name:5} {mode:15} {:>6} {:>5.0}M {:>10.1?} {:>10.1?} {:>10.1?} {:>10.1?}{}",
                t.files,
                t.bytes as f64 / f64::from(t.files) / 1e6,
                per(t.metadata),
                per(t.thumb),
                per(t.preview),
                per(t.metadata + t.thumb + t.preview),
                if t.failed > 0 { format!("  ({} failed)", t.failed) } else { String::new() }
            );
        }
    }
}
