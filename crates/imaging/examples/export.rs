//! Export one file with each format, timing render and encode:
//! `cargo run --release -p imaging --example export -- <photo> <out-dir> [long-edge|-] [16]`
use std::path::Path;
use std::time::Instant;

use imaging::export::{EmbeddedMetadata, FileFormat, ImageOptions, encode, render, write_atomically};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (src, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let edge: Option<u32> = args.get(3).and_then(|s| s.parse().ok());
    let ext = src.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    let raw = matches!(ext.as_str(), "nef" | "dng" | "cr2" | "cr3" | "arw" | "raf");
    let kind = if raw { imaging::SourceKind::Raw } else { imaging::SourceKind::Image };
    let mut opts = ImageOptions { resize: edge.is_some(), edge: edge.unwrap_or(0), ..Default::default() };
    // A 4th argument "16": 16-bit ProPhoto TIFF / PNG (RAWs rendered from the raw data).
    // "16srgb": the same in sRGB.
    if let Some(a) = args.get(4).filter(|a| a.starts_with("16")) {
        opts.sixteen_bit = true;
        if a == "16" {
            opts.color_space = imaging::export::ColorSpace::ProPhotoRgb;
        }
        opts.format = FileFormat::Tiff;
    }
    let t = Instant::now();
    let (img, meta) = render(src, kind, 0, &opts, None)?;
    println!("render {}x{} in {:.0?}", img.width(), img.height(), t.elapsed());
    let embedded =
        EmbeddedMetadata { camera: Some(meta), copyright: Some("test".into()), ..Default::default() };
    std::fs::create_dir_all(out)?;
    let stem = src.file_stem().unwrap().to_string_lossy();
    for f in [FileFormat::Jpeg, FileFormat::Tiff, FileFormat::Png] {
        opts.format = f;
        let t = Instant::now();
        let bytes = encode(&img, &opts, &embedded)?;
        let path = out.join(format!("{stem}.{}", f.extension().unwrap()));
        write_atomically(&path, &bytes)?;
        println!("{:5} {:>9} bytes in {:.0?}", f.label(), bytes.len(), t.elapsed());
    }
    Ok(())
}
