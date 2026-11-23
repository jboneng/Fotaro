//! Try the depth model on real photos:
//! `cargo run -p segment --example depthbench --release -- <models dir> <out dir> [photo ...]`
//!
//! The models dir holds Depth Anything V2 Small's `model.onnx` and `model_fp16.onnx`
//! (from `onnx-community/depth-anything-v2-small`). Without photos, those in
//! `testdata/real` that are there. Each model runs on the CPU and through DirectML at
//! 518 px and at 1036 px; it prints the times (the second run of each, without loading)
//! and writes `<photo>_<size>.png`: the photo beside its map (white: near).

use std::path::PathBuf;
use std::time::Instant;

use image::{GrayImage, Luma, RgbImage, imageops};
use imaging::SourceKind;
use segment::{Device, Model};

const PHOTOS: &[&str] = &[
    "testdata/real/other/_JMB5147.jpg",
    "testdata/real/nef/_JMB4583.NEF",
    "testdata/real/nef/_JMB5064.NEF",
    "testdata/real/dng-camera/_JMB4933.dng",
    "testdata/real/dng-camera/_JMB4951.dng",
    "testdata/real/film/20260926_001.tiff",
    "testdata/real/film/20260926_003.tiff",
    "testdata/real/tiff/20261002_001.tif",
];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [models, out, photos @ ..] = args.as_slice() else {
        anyhow::bail!("usage: depthbench <models dir> <out dir> [photo ...]")
    };
    let (models, out) = (PathBuf::from(models), PathBuf::from(out));
    std::fs::create_dir_all(&out)?;
    let photos: Vec<PathBuf> = if photos.is_empty() {
        PHOTOS.iter().map(PathBuf::from).filter(|p| p.exists()).collect()
    } else {
        photos.iter().map(PathBuf::from).collect()
    };
    let mut images = Vec::new();
    for path in &photos {
        let raw = path.extension().is_some_and(|e| ["nef", "dng"].iter().any(|r| e.eq_ignore_ascii_case(r)));
        let (img, _) = imaging::decode(path, if raw { SourceKind::Raw } else { SourceKind::Image }, 2048)?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_owned();
        images.push((stem, img));
    }
    println!("DirectML runs on: {}", segment::gpu::chosen().name);
    for file in ["model.onnx", "model_fp16.onnx"] {
        for device in [Device::DirectMl, Device::Cpu] {
            let started = Instant::now();
            let mut model = match Model::load(&models.join(file), device) {
                Ok(m) => m,
                Err(e) => {
                    println!("{file} {}: does not load: {e:#}", device.label());
                    continue;
                }
            };
            let (inputs, outputs) = model.io();
            println!(
                "{file} {}: loaded in {:.1?} ({inputs:?} -> {outputs:?})",
                device.label(),
                started.elapsed()
            );
            for size in [518u32, 1036] {
                let mut times = Vec::new();
                for (stem, img) in &images {
                    let mut run = || -> anyhow::Result<(Vec<f32>, u32, u32, std::time::Duration)> {
                        let t = Instant::now();
                        let (near, w, h) = segment::depth(&mut model, img, size)?;
                        Ok((near, w, h, t.elapsed()))
                    };
                    // Twice: the first run of a size also sets the model up for it.
                    let result = run().and_then(|_| run());
                    match result {
                        Ok((near, w, h, took)) => {
                            times.push(took.as_secs_f32());
                            if file == "model.onnx" && device == Device::DirectMl || images.len() == 1 {
                                save(&out.join(format!("{stem}_{size}.png")), img, &near, w, h)?;
                            }
                        }
                        Err(e) => {
                            println!("  {size} px, {stem}: fails: {e:#}");
                            break;
                        }
                    }
                }
                if !times.is_empty() {
                    let mean = times.iter().sum::<f32>() / times.len() as f32;
                    let most = times.iter().copied().fold(0.0, f32::max);
                    println!(
                        "  {size} px: {mean:.2} s a photo (slowest {most:.2} s, {} photos)",
                        times.len()
                    );
                }
            }
        }
    }
    Ok(())
}

/// The photo beside its map (white: near), both 900 px wide.
fn save(path: &std::path::Path, img: &RgbImage, near: &[f32], w: u32, h: u32) -> anyhow::Result<()> {
    let map = GrayImage::from_fn(w, h, |x, y| Luma([(near[(y * w + x) as usize] * 255.0).round() as u8]));
    let (pw, ph) = (900, 900 * img.height() / img.width());
    let photo = imageops::resize(img, pw, ph, imageops::FilterType::Triangle);
    let map = imageops::resize(&map, pw, ph, imageops::FilterType::Triangle);
    let mut both = RgbImage::new(2 * pw + 8, ph);
    imageops::replace(&mut both, &photo, 0, 0);
    imageops::replace(&mut both, &image::DynamicImage::ImageLuma8(map).to_rgb8(), i64::from(pw + 8), 0);
    both.save(path)?;
    Ok(())
}
