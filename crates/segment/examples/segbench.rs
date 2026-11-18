//! Benchmark the AI mask models on photos, on the CPU and through DirectML:
//! `cargo run -p segment --example segbench --release -- <models dir> <out dir> <photos...>`
//!
//! For each model and device: load time, the first run (which includes warm-up) and the
//! best of the next runs. Masks are saved next to the photo as `<photo>.<model>.png`
//! overlays (from the CPU run), and DirectML's masks are compared with the CPU's.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use image::{GrayImage, Rgb, RgbImage};
use imaging::SourceKind;
use segment::{
    Device, Model, SamKind, sam_decode, sam_decode_prompt, sam_encode, sky, subject_at, subject_rmbg,
};

/// The last box prompt's mask (saved next to the click's).
static BOXED: std::sync::Mutex<Option<GrayImage>> = std::sync::Mutex::new(None);

/// The photo at the size the app would hand the models.
const WORKING_PX: u32 = 1600;
const RUNS: usize = 3;

struct Bench {
    name: &'static str,
    files: &'static [&'static str],
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    Subject(u32),
    Rmbg,
    Sky,
    Sam(SamKind),
}

const BENCHES: &[Bench] = &[
    Bench { name: "birefnet_lite", files: &["birefnet_lite/model.onnx"], kind: Kind::Subject(1024) },
    Bench { name: "birefnet_lite_768", files: &["birefnet_lite/model.onnx"], kind: Kind::Subject(768) },
    Bench { name: "birefnet_lite_512", files: &["birefnet_lite/model.onnx"], kind: Kind::Subject(512) },
    Bench {
        name: "birefnet_lite_fp16",
        files: &["birefnet_lite/model_fp16.onnx"],
        kind: Kind::Subject(1024),
    },
    Bench { name: "rmbg14", files: &["rmbg14/model.onnx"], kind: Kind::Rmbg },
    Bench { name: "rmbg14_quantized", files: &["rmbg14/model_quantized.onnx"], kind: Kind::Rmbg },
    Bench { name: "segformer_b0", files: &["segformer_b0/model.onnx"], kind: Kind::Sky },
    Bench { name: "segformer_b2", files: &["segformer_b2/model.onnx"], kind: Kind::Sky },
    Bench {
        name: "slimsam",
        files: &["slimsam/vision_encoder.onnx", "slimsam/prompt_encoder_mask_decoder.onnx"],
        kind: Kind::Sam(SamKind::Sam1),
    },
    Bench {
        name: "sam21_tiny",
        files: &["sam21_tiny/vision_encoder.onnx", "sam21_tiny/prompt_encoder_mask_decoder.onnx"],
        kind: Kind::Sam(SamKind::Sam2),
    },
];

fn ms(d: Duration) -> String {
    format!("{:>7.0} ms", d.as_secs_f64() * 1000.0)
}

/// The photo with the mask as a red tint.
fn overlay(img: &RgbImage, mask: &GrayImage) -> RgbImage {
    RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        let a = f32::from(mask.get_pixel(x, y)[0]) / 255.0 * 0.6;
        Rgb([0, 1, 2].map(|c| {
            let target = if c == 0 { 255.0 } else { 0.0 };
            (f32::from(p[c]) + a * (target - f32::from(p[c]))) as u8
        }))
    })
}

fn mean_diff(a: &GrayImage, b: &GrayImage) -> f32 {
    let sum: u64 = a.pixels().zip(b.pixels()).map(|(p, q)| u64::from(p[0].abs_diff(q[0]))).sum();
    sum as f32 / a.pixels().len().max(1) as f32
}

/// Run one model on one photo: (first run, best of the rest, the mask).
fn run(
    models: &mut [Model],
    kind: Kind,
    img: &RgbImage,
) -> anyhow::Result<(Duration, Duration, GrayImage, String)> {
    let mut times = Vec::new();
    let mut mask = GrayImage::new(1, 1);
    let mut detail = String::new();
    for _ in 0..=RUNS {
        let t = Instant::now();
        mask = match kind {
            Kind::Subject(size) => subject_at(&mut models[0], img, size)?,
            Kind::Rmbg => subject_rmbg(&mut models[0], img)?,
            Kind::Sky => sky(&mut models[0], img)?,
            Kind::Sam(k) => {
                let encoded = sam_encode(&mut models[0], k, img)?;
                let enc = t.elapsed();
                // One click in the middle (where a subject usually is).
                let (w, h) = img.dimensions();
                let d = Instant::now();
                let m = sam_decode(&mut models[1], &encoded, &[(w as f32 / 2.0, h as f32 / 2.0, true)])?;
                let click = d.elapsed();
                // And a box around the middle half, alone (as Select Object's rectangle).
                let d = Instant::now();
                let (fw, fh) = (w as f32, h as f32);
                let rect = [fw * 0.25, fh * 0.2, fw * 0.75, fh * 0.8];
                let boxed = sam_decode_prompt(&mut models[1], &encoded, &[], Some(rect))?;
                let covered = boxed.pixels().filter(|p| p[0] > 127).count() as f32 / (w * h) as f32;
                detail = format!(
                    "(encoder {}, a click {}, a box {} covering {:.0}%)",
                    ms(enc).trim(),
                    ms(click).trim(),
                    ms(d.elapsed()).trim(),
                    covered * 100.0
                );
                *BOXED.lock().unwrap() = Some(boxed);
                m
            }
        };
        times.push(t.elapsed());
    }
    let best = times[1..].iter().min().copied().unwrap_or_default();
    Ok((times[0], best, mask, detail))
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [models_dir, out_dir, photos @ ..] = args.as_slice() else {
        anyhow::bail!("usage: segbench <models dir> <out dir> <photos...>");
    };
    let (models_dir, out_dir) = (PathBuf::from(models_dir), PathBuf::from(out_dir));
    std::fs::create_dir_all(&out_dir)?;
    let images: Vec<(String, RgbImage)> = photos
        .iter()
        .map(|p| {
            let path = Path::new(p);
            let raw = ["nef", "dng", "cr2", "cr3", "arw"]
                .iter()
                .any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e)));
            let kind = if raw { SourceKind::Raw } else { SourceKind::Image };
            let (img, _) = imaging::decode(path, kind, WORKING_PX)?;
            let stem = path.file_stem().map_or("photo".into(), |s| s.to_string_lossy().into_owned());
            Ok((stem, img))
        })
        .collect::<anyhow::Result<_>>()?;
    println!("{} photos at up to {WORKING_PX} px; best of {RUNS} after the first run\n", images.len());

    // PL_BENCH=name,name: only those (names containing them).
    let only: Vec<String> =
        std::env::var("PL_BENCH").map(|v| v.split(',').map(str::to_owned).collect()).unwrap_or_default();
    for bench in
        BENCHES.iter().filter(|b| only.is_empty() || only.iter().any(|o| b.name.contains(o.as_str())))
    {
        let mut cpu_masks: Vec<GrayImage> = Vec::new();
        for device in [Device::Cpu, Device::DirectMl] {
            let t = Instant::now();
            let models: anyhow::Result<Vec<Model>> =
                // SAM's decoder runs on the CPU (as the app runs it): its inputs change shape.
                bench.files.iter().enumerate().map(|(i, f)| Model::load(&models_dir.join(f), if i > 0 { Device::Cpu } else { device })).collect();
            let mut models = match models {
                Ok(m) => m,
                Err(e) => {
                    println!("{:<20} {:<9} failed to load: {e:#}", bench.name, device.label());
                    continue;
                }
            };
            let load = t.elapsed();
            if device == Device::Cpu {
                for m in &models {
                    let described = m.describe();
                    println!("{:<20} {described}", bench.name);
                }
            }
            for (k, (stem, img)) in images.iter().enumerate() {
                match run(&mut models, bench.kind, img) {
                    Ok((first, best, mask, detail)) => {
                        let check = match device {
                            Device::Cpu => {
                                overlay(img, &mask)
                                    .save(out_dir.join(format!("{stem}.{}.png", bench.name)))?;
                                if let Some(b) = BOXED.lock().unwrap().take() {
                                    overlay(img, &b)
                                        .save(out_dir.join(format!("{stem}.{}_box.png", bench.name)))?;
                                }
                                cpu_masks.push(mask);
                                String::new()
                            }
                            Device::DirectMl => cpu_masks
                                .get(k)
                                .map(|c| format!("differs from CPU by {:.1}/255", mean_diff(c, &mask)))
                                .unwrap_or_default(),
                        };
                        println!(
                            "{:<20} {:<9} {:<14} load {} first {} best {} {detail} {check}",
                            bench.name,
                            device.label(),
                            stem,
                            ms(load),
                            ms(first),
                            ms(best)
                        );
                    }
                    Err(e) => println!("{:<20} {:<9} {stem}: {e:#}", bench.name, device.label()),
                }
            }
        }
        println!();
    }
    Ok(())
}
