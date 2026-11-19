//! Benchmark Remove's models on real removals, on the CPU and through DirectML:
//! `cargo run -p segment --example inpaintbench --release -- <models dir> <out dir>`
//!
//! The models dir holds `lama/lama_fp32.onnx`, `lama/lama_int4_int8.onnx`,
//! `migan/migan-512-generator.onnx` and `sd/{unet,vae_encoder,vae_decoder,text_encoder}/
//! model.onnx` with `sd/tokenizer/vocab.json`. Each removal is cut as the app would (a
//! square around the area, grown, scaled to 512) and saved with every model's result side
//! by side: `<task>.png`. `PL_BENCH=lama,migan,sd` picks models; `PL_SD_STEPS` and
//! `PL_SD_CFG` set the sampler.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use image::{GrayImage, Luma, Rgb, RgbImage, imageops};
use imaging::SourceKind;
use segment::inpaint::{SIZE, lama, migan};
use segment::sd::{Prompts, Sampling, Sd, embed, serde_json_lite::Vocab, tokens};
use segment::{Device, Model};

/// A removal: the photo, and an ellipse to remove (centre and radii as 0..1 of the photo's
/// width and height).
struct Task {
    name: &'static str,
    photo: &'static str,
    centre: [f32; 2],
    radii: [f32; 2],
}

const TASKS: &[Task] = &[
    Task {
        name: "truck_person",
        photo: "testdata/real/other/_JMB5147.jpg",
        centre: [0.893, 0.745],
        radii: [0.06, 0.21],
    },
    Task {
        name: "truck_sticker",
        photo: "testdata/real/other/_JMB5147.jpg",
        centre: [0.752, 0.762],
        radii: [0.055, 0.035],
    },
    Task {
        name: "truck_flags",
        photo: "testdata/real/other/_JMB5147.jpg",
        centre: [0.305, 0.285],
        radii: [0.06, 0.06],
    },
    Task {
        name: "cat_eye",
        photo: "testdata/real/nef/_JMB4583.NEF",
        centre: [0.41, 0.33],
        radii: [0.065, 0.085],
    },
    Task {
        name: "ball",
        photo: "testdata/web/d750/lossless_compressed_14_bit.NEF",
        centre: [0.5, 0.5],
        radii: [0.25, 0.37],
    },
];

const POSITIVE: &str = "background, seamless, empty, natural, photo";
const NEGATIVE: &str = "object, person, text, logo, blurry";

fn ms(d: Duration) -> String {
    format!("{:.0} ms", d.as_secs_f64() * 1000.0)
}

/// The task's 512 crop and mask (255: remove), cut as the app will: a square around the
/// area grown by 1.5 radii (at least 64 px).
fn cut(img: &RgbImage, t: &Task) -> (RgbImage, GrayImage) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy, rx, ry) = (t.centre[0] * w, t.centre[1] * h, t.radii[0] * w, t.radii[1] * h);
    let half = (rx.max(ry) * 2.5).max(rx.max(ry) + 64.0);
    let side = (2.0 * half).min(w.min(h));
    let x0 = (cx - side / 2.0).clamp(0.0, w - side);
    let y0 = (cy - side / 2.0).clamp(0.0, h - side);
    let crop = imageops::crop_imm(img, x0 as u32, y0 as u32, side as u32, side as u32).to_image();
    let crop = imageops::resize(&crop, SIZE, SIZE, imageops::FilterType::Lanczos3);
    let k = SIZE as f32 / side;
    let mask = GrayImage::from_fn(SIZE, SIZE, |x, y| {
        let (px, py) = ((x as f32 + 0.5) / k + x0, (y as f32 + 0.5) / k + y0);
        let d = ((px - cx) / rx).powi(2) + ((py - cy) / ry).powi(2);
        Luma([if d <= 1.0 { 255 } else { 0 }])
    });
    (crop, mask)
}

/// The crop with the area to remove tinted red.
fn shown(img: &RgbImage, mask: &GrayImage) -> RgbImage {
    RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let p = img.get_pixel(x, y);
        if mask.get_pixel(x, y)[0] > 127 { Rgb([255, p[1] / 3, p[2] / 3]) } else { *p }
    })
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [models, out] = args.as_slice() else { anyhow::bail!("usage: inpaintbench <models dir> <out dir>") };
    let (models, out) = (PathBuf::from(models), PathBuf::from(out));
    std::fs::create_dir_all(&out)?;
    let only: Vec<String> =
        std::env::var("PL_BENCH").map(|v| v.split(',').map(str::to_owned).collect()).unwrap_or_default();
    let want = |name: &str| only.is_empty() || only.iter().any(|o| name.contains(o.as_str()));

    // The GPUs DirectML could use, and the one it will.
    for g in segment::gpu::gpus() {
        println!("GPU {}: {} ({} MB)", g.index, g.name, g.memory >> 20);
    }
    println!("DirectML runs on: {}", segment::gpu::chosen().name);

    // The removals, cut once.
    let mut photos: Vec<(&str, RgbImage)> = Vec::new();
    let mut cuts: Vec<(&Task, RgbImage, GrayImage)> = Vec::new();
    for t in TASKS {
        if !photos.iter().any(|(p, _)| *p == t.photo) {
            let path = Path::new(t.photo);
            let raw = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("nef"));
            let kind = if raw { SourceKind::Raw } else { SourceKind::Image };
            let (img, _) = imaging::decode(path, kind, 8000)?;
            photos.push((t.photo, img));
        }
        let img = &photos.iter().find(|(p, _)| *p == t.photo).expect("decoded").1;
        let (crop, mask) = cut(img, t);
        cuts.push((t, crop, mask));
    }
    // Per task: the panels to save side by side (label, image).
    let mut panels: Vec<Vec<(String, RgbImage)>> =
        cuts.iter().map(|(_, c, m)| vec![("original".to_owned(), shown(c, m))]).collect();

    // Content-aware models.
    type Run = fn(&mut Model, &RgbImage, &GrayImage) -> anyhow::Result<RgbImage>;
    let fills: [(&str, &str, Run); 5] = [
        ("lama_fp32", "lama/lama_fp32.onnx", lama),
        ("lama_int8", "lama/lama_int4_int8.onnx", lama),
        ("lama_r4", "lama/lama_int4_int8_r4.onnx", lama),
        ("lama_dilated", "lama_dilated/lama_dilated.onnx", lama),
        ("migan", "migan/migan-512-generator.onnx", migan),
    ];
    for (name, file, run) in fills {
        if !want(name) {
            continue;
        }
        for device in [Device::Cpu, Device::DirectMl] {
            let t = Instant::now();
            let mut model = match Model::load(&models.join(file), device) {
                Ok(m) => m,
                Err(e) => {
                    println!("{name:<10} {:<9} failed to load: {e:#}", device.label());
                    continue;
                }
            };
            if device == Device::Cpu {
                println!("{name:<10} {}", model.describe());
            }
            let load = t.elapsed();
            let mut times = Vec::new();
            for (k, (task, crop, mask)) in cuts.iter().enumerate() {
                let t = Instant::now();
                match run(&mut model, crop, mask) {
                    Ok(filled) => {
                        times.push(t.elapsed());
                        {
                            let nan = filled.pixels().filter(|p| p.0 == [0, 0, 0]).count();
                            if nan > 0 {
                                println!("{name} {} {}: {nan} black pixels", device.label(), task.name);
                            }
                            panels[k].push((format!("{name} {}", device.label()), filled));
                        }
                    }
                    Err(e) => println!("{name:<10} {:<9} {}: {e:#}", device.label(), task.name),
                }
            }
            if !times.is_empty() {
                let first = times[0];
                let best = times[1..].iter().min().copied().unwrap_or(first);
                println!(
                    "{name:<10} {:<9} load {} first {} then {}",
                    device.label(),
                    ms(load),
                    ms(first),
                    ms(best)
                );
            }
        }
    }

    // Generative: SD 1.5 inpainting.
    if want("sd") {
        let sd_dir = models.join("sd");
        let vocab = Vocab::parse(&std::fs::read_to_string(sd_dir.join("tokenizer/vocab.json"))?);
        let mut text = Model::load(&sd_dir.join("text_encoder/model.onnx"), Device::Cpu)?;
        println!("text_encoder {}", text.describe());
        let positive = embed(&mut text, &tokens(POSITIVE, &vocab)?)?;
        let negative = embed(&mut text, &tokens(NEGATIVE, &vocab)?)?;
        drop(text);
        // Saved for the app: positive then negative, f32 little-endian.
        let bytes: Vec<u8> = positive.iter().chain(&negative).flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(out.join("sd_prompts.bin"), bytes)?;
        let prompts = Prompts { positive, negative };
        let steps: usize = std::env::var("PL_SD_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
        let guidance: f32 = std::env::var("PL_SD_CFG").ok().and_then(|v| v.parse().ok()).unwrap_or(5.0);
        for device in [Device::DirectMl, Device::Cpu] {
            let t = Instant::now();
            let loaded = (|| {
                Ok::<_, anyhow::Error>(Sd {
                    unet: Model::load(&sd_dir.join("unet/model.onnx"), device)?,
                    vae_encoder: Model::load(&sd_dir.join("vae_encoder/model.onnx"), device)?,
                    vae_decoder: Model::load(&sd_dir.join("vae_decoder/model.onnx"), device)?,
                })
            })();
            let mut sd = match loaded {
                Ok(sd) => sd,
                Err(e) => {
                    println!("sd         {:<9} failed to load: {e:#}", device.label());
                    continue;
                }
            };
            if device == Device::DirectMl {
                println!("unet {}", sd.unet.describe());
            }
            let load = t.elapsed();
            // The CPU: a short run per task to time a step; DirectML: full fills, and 3
            // seeds for the first task.
            let runs: Vec<(usize, u64, usize)> = match device {
                Device::Cpu => vec![(0, 0, 2)],
                Device::DirectMl => {
                    let mut r: Vec<(usize, u64, usize)> = (0..cuts.len()).map(|k| (k, 0, steps)).collect();
                    r.extend([(0, 1, steps), (0, 2, steps)]);
                    r
                }
            };
            for (k, seed, n) in runs {
                let (task, crop, mask) = &cuts[k];
                let t = Instant::now();
                let mut step_times = Vec::new();
                let mut last = Instant::now();
                let sampling = Sampling { steps: n, guidance, seed };
                match sd.inpaint(crop, mask, &prompts, sampling, &mut |_, _| {
                    step_times.push(last.elapsed());
                    last = Instant::now();
                }) {
                    Ok(filled) => {
                        let step = step_times.iter().skip(1).min().copied().unwrap_or_default();
                        println!(
                            "sd         {:<9} {:<14} seed {seed} load {} total {} ({n} steps, {} a step, cfg {guidance})",
                            device.label(),
                            task.name,
                            ms(load),
                            ms(t.elapsed()),
                            ms(step)
                        );
                        if device == Device::DirectMl {
                            panels[k].push((format!("sd seed {seed}"), filled));
                        }
                    }
                    Err(e) => println!("sd         {:<9} {}: {e:#}", device.label(), task.name),
                }
            }
        }
    }

    // Side by side, per task.
    for ((task, ..), row) in cuts.iter().zip(&panels) {
        let mut sheet = RgbImage::new(SIZE * row.len() as u32, SIZE);
        for (i, (_, img)) in row.iter().enumerate() {
            imageops::replace(&mut sheet, img, i as i64 * SIZE as i64, 0);
        }
        let labels: Vec<&str> = row.iter().map(|(l, _)| l.as_str()).collect();
        sheet.save(out.join(format!("{}.png", task.name)))?;
        println!("{}: {}", task.name, labels.join(" | "));
    }
    Ok(())
}
