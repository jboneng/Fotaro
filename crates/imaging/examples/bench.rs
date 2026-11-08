//! Time each stage of preview building for one file:
//! `cargo run -p imaging --example bench --release -- <image or raw file>`

use std::time::Instant;

use image::DynamicImage;
use imaging::{PreviewSize, SourceKind};

fn main() {
    let path = std::env::args().nth(1).expect("usage: bench <file>");
    let path = std::path::Path::new(&path);
    let kind = if catalog_like_is_raw(path) { SourceKind::Raw } else { SourceKind::Image };
    const RUNS: u32 = 10;

    let t = Instant::now();
    let mut standard = None;
    for _ in 0..RUNS {
        standard = Some(imaging::decode(path, kind, PreviewSize::Standard.max_px()).unwrap().0);
    }
    let standard = standard.unwrap();
    println!(
        "decode+fit to {}px: {:.1?}/run ({}x{})",
        PreviewSize::Standard.max_px(),
        t.elapsed() / RUNS,
        standard.width(),
        standard.height()
    );

    let t = Instant::now();
    let mut thumb = None;
    for _ in 0..RUNS {
        thumb = Some(DynamicImage::ImageRgb8(standard.clone()).thumbnail(256, 256).into_rgb8());
    }
    println!("thumbnail 256px:     {:.1?}/run", t.elapsed() / RUNS);
    let thumb = thumb.unwrap();

    for (name, img, q) in [("standard", &standard, 88u8), ("thumb", &thumb, 80)] {
        let t = Instant::now();
        let mut len = 0;
        for _ in 0..RUNS {
            let mut buf = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q).encode_image(img).unwrap();
            len = buf.len();
        }
        println!("image  encode {name:8}: {:.1?}/run ({} KB)", t.elapsed() / RUNS, len / 1024);

        let t = Instant::now();
        for _ in 0..RUNS {
            let mut buf = Vec::new();
            let enc = jpeg_encoder::Encoder::new(&mut buf, q);
            enc.encode(img.as_raw(), img.width() as u16, img.height() as u16, jpeg_encoder::ColorType::Rgb)
                .unwrap();
            len = buf.len();
        }
        println!("jpeg-encoder {name:8}: {:.1?}/run ({} KB)", t.elapsed() / RUNS, len / 1024);
    }

    let bytes = std::fs::read(path).unwrap();
    let t = Instant::now();
    for _ in 0..RUNS {
        let _ = imaging::read_metadata(path, kind);
    }
    println!("read_metadata:       {:.1?}/run ({} KB file)", t.elapsed() / RUNS, bytes.len() / 1024);
}

fn catalog_like_is_raw(p: &std::path::Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    ["nef", "cr2", "cr3", "arw", "raf", "dng", "orf", "rw2"].contains(&ext.as_str())
}
