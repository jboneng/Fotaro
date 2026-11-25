//! AI masks for Develop's masking: the photo's subject (BiRefNet), its sky (SegFormer
//! trained on ADE20K) and objects picked with points (Segment Anything), from ONNX models
//! run with ONNX Runtime on the CPU or, through DirectML, on any DirectX 12 GPU.
//!
//! Each function takes an 8-bit RGB image (the photo at a working size) and returns a
//! mask of the same size (0..255).

use std::path::Path;

pub mod gpu;
pub mod inpaint;
pub mod sd;
mod store;
use std::sync::OnceLock;
pub use store::{AiModel, data_dir, download_size, ensure, ready, runtime_lib};

use anyhow::{Context, Result, anyhow, bail};
use image::{GrayImage, RgbImage, imageops};
use ort::session::Session;
use ort::value::Tensor;

/// Where a model runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Cpu,
    /// Any DirectX 12 GPU on Windows (integrated or not).
    DirectMl,
}

impl Device {
    pub fn label(self) -> &'static str {
        match self {
            Device::Cpu => "CPU",
            Device::DirectMl => "DirectML",
        }
    }
}

/// A loaded model.
pub struct Model {
    session: Session,
}

impl Model {
    pub fn load(path: &Path, device: Device) -> Result<Model> {
        let err = |e: ort::Error<ort::session::builder::SessionBuilder>| anyhow!("{e}");
        let builder = Session::builder(environment()?)?;
        let mut builder = match device {
            Device::Cpu => builder
                .with_intra_threads(cpu_threads())
                .map_err(err)?
                .with_execution_providers([ort::ep::CPU::default().build()])
                .map_err(err)?,
            // DirectML wants one thread driving the GPU and no memory pattern reuse.
            Device::DirectMl => builder
                .with_optimization_level(match std::env::var("PL_ORT_OPT").as_deref() {
                    Ok("0") => ort::session::builder::GraphOptimizationLevel::Disable,
                    Ok("1") => ort::session::builder::GraphOptimizationLevel::Level1,
                    _ => ort::session::builder::GraphOptimizationLevel::All,
                })
                .map_err(err)?
                .with_parallel_execution(false)
                .map_err(err)?
                .with_memory_pattern(false)
                .map_err(err)?
                .with_execution_providers([ort::ep::DirectML::default()
                    .with_device_id(gpu::chosen().index)
                    .build()
                    .error_on_failure()])
                .map_err(err)?,
        };
        let session =
            builder.commit_from_file(path).with_context(|| format!("loading {}", path.display()))?;
        Ok(Model { session })
    }

    /// Input and output names (for checking a model's export).
    pub fn io(&self) -> (Vec<String>, Vec<String>) {
        let names = |o: &[ort::value::Outlet]| o.iter().map(|o| o.name().to_string()).collect();
        (names(self.session.inputs()), names(self.session.outputs()))
    }
}

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// `img` squashed to `w × h`, ImageNet-normalised, as NCHW floats.
fn normalised(img: &RgbImage, w: u32, h: u32) -> Vec<f32> {
    let small = imageops::resize(img, w, h, imageops::FilterType::Triangle);
    let n = (w * h) as usize;
    let mut out = vec![0.0f32; 3 * n];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            out[c * n + i] = (f32::from(p[c]) / 255.0 - MEAN[c]) / STD[c];
        }
    }
    out
}

/// A probability map (0..1) of `w × h` resized to `size` as an 8-bit mask.
fn to_mask(prob: &[f32], w: u32, h: u32, size: (u32, u32)) -> GrayImage {
    let small = GrayImage::from_fn(w, h, |x, y| {
        image::Luma([(prob[(y * w + x) as usize] * 255.0).round().clamp(0.0, 255.0) as u8])
    });
    imageops::resize(&small, size.0, size.1, imageops::FilterType::Triangle)
}

fn sigmoid(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}

/// The photo's main subject (BiRefNet): 1024 × 1024 in, a matte out.
pub fn subject(model: &mut Model, img: &RgbImage) -> Result<GrayImage> {
    subject_at(model, img, 1024)
}

/// [`subject`] at another input size (the model was trained at 1024; smaller is faster).
pub fn subject_at(model: &mut Model, img: &RgbImage, size: u32) -> Result<GrayImage> {
    let s = size.clamp(256, 2048);
    let input = Tensor::from_array(([1usize, 3, s as usize, s as usize], normalised(img, s, s)))?;
    let outputs = model.session.run(ort::inputs![input])?;
    let (_, logits) = outputs[0].try_extract_tensor::<f32>()?;
    let prob: Vec<f32> = logits.iter().map(|v| sigmoid(*v)).collect();
    Ok(to_mask(&prob, s, s, img.dimensions()))
}

/// ADE20K's sky class.
const ADE_SKY: usize = 2;

/// The photo's sky (SegFormer, ADE20K): 512 × 512 in, the sky class's probability out.
pub fn sky(model: &mut Model, img: &RgbImage) -> Result<GrayImage> {
    const S: u32 = 512;
    let input = Tensor::from_array(([1usize, 3, S as usize, S as usize], normalised(img, S, S)))?;
    let outputs = model.session.run(ort::inputs![input])?;
    let (shape, logits) = outputs[0].try_extract_tensor::<f32>()?;
    let [_, classes, h, w] = shape[..] else { bail!("unexpected SegFormer output {shape:?}") };
    let (classes, h, w) = (classes as usize, h as usize, w as usize);
    let n = h * w;
    let prob: Vec<f32> = (0..n)
        .map(|i| {
            let max = (0..classes).map(|c| logits[c * n + i]).fold(f32::MIN, f32::max);
            let sum: f32 = (0..classes).map(|c| (logits[c * n + i] - max).exp()).sum();
            (logits[ADE_SKY * n + i] - max).exp() / sum
        })
        .collect();
    Ok(to_mask(&prob, w as u32, h as u32, img.dimensions()))
}

/// How near everything in the photo is (Depth Anything V2): the photo at `long_side`
/// pixels (the model was trained at 518; larger gives a finer map, slower) in, with its
/// shape kept and its sides multiples of 14; nearness out, 0 (the farthest in this
/// photo) to 1 (the nearest), as `(values, width, height)` at the model's size. Relative
/// only: there are no distances in it.
pub fn depth(model: &mut Model, img: &RgbImage, long_side: u32) -> Result<(Vec<f32>, u32, u32)> {
    let (iw, ih) = img.dimensions();
    let k = long_side.clamp(140, 2800) as f32 / iw.max(ih).max(1) as f32;
    let side = |v: u32| ((v as f32 * k / 14.0).round().max(1.0) as u32) * 14;
    let (w, h) = (side(iw), side(ih));
    let input = Tensor::from_array(([1usize, 3, h as usize, w as usize], normalised(img, w, h)))?;
    let outputs = model.session.run(ort::inputs![input])?;
    let (shape, near) = outputs[0].try_extract_tensor::<f32>()?;
    let [.., oh, ow] = shape[..] else { bail!("unexpected depth output {shape:?}") };
    let (lo, hi) = near.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    // A GPU in trouble can answer with nothing at all instead of an error: better to fail
    // (and have the CPU do it) than to keep a map that says everything is as far.
    let span = hi - lo;
    if span.is_nan() || span <= 1e-6 {
        bail!("the depth model gave an empty map");
    }
    Ok((near.iter().map(|v| (v - lo) / span).collect(), ow as u32, oh as u32))
}

/// The model's long side for a depth map: twice what it was trained at (518), which gives
/// sharper edges between near and far things and takes three to five times as long (8 s
/// on a small GPU, half a minute on a CPU), once per photo.
const DEPTH_SIDE: u32 = 1036;

/// [`depth`] as a picture of the photo's size: white near, black far, as the square root
/// of the nearness, which leaves more of the 256 steps to what is far away (where the
/// nearness itself hardly changes).
fn depth_picture(model: &mut Model, img: &RgbImage) -> Result<GrayImage> {
    let (near, w, h) = depth(model, img, DEPTH_SIDE)?;
    let coded: Vec<f32> = near.iter().map(|v| v.max(0.0).sqrt()).collect();
    Ok(to_mask(&coded, w, h, img.dimensions()))
}

/// Which way every surface in the photo faces (MoGe-2): the photo at `long_side` pixels
/// in, with its shape kept, and the number of `tokens` the model looks at it in (1200 to
/// 2500; more is finer and slower); a unit direction out per pixel, as (x to the right, y
/// down, z towards the camera), as `(values, width, height)` at the model's size. What
/// the model says is nothing (the sky) faces the camera.
pub fn surface(
    model: &mut Model,
    img: &RgbImage,
    long_side: u32,
    tokens: u32,
) -> Result<(Vec<[f32; 3]>, u32, u32)> {
    let (iw, ih) = img.dimensions();
    let k = long_side.clamp(140, 2800) as f32 / iw.max(ih).max(1) as f32;
    let side = |v: u32| ((v as f32 * k).round() as u32).max(28);
    let (w, h) = (side(iw), side(ih));
    let small = imageops::resize(img, w, h, imageops::FilterType::Triangle);
    let n = (w * h) as usize;
    let mut pixels = vec![0.0f32; 3 * n];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            pixels[c * n + i] = f32::from(p[c]) / 255.0;
        }
    }
    let image = Tensor::from_array(([1usize, 3, h as usize, w as usize], pixels))?;
    let count = Tensor::from_array((Vec::<usize>::new(), vec![i64::from(tokens.clamp(256, 4000))]))?;
    let outputs = model.session.run(ort::inputs!["image" => image, "num_tokens" => count])?;
    let (shape, normal) = outputs["normal"].try_extract_tensor::<f32>()?;
    let [_, oh, ow, 3] = shape[..] else { bail!("unexpected surface output {shape:?}") };
    let (_, there) = outputs["mask"].try_extract_tensor::<f32>()?;
    let count = (oh * ow) as usize;
    if normal.len() != 3 * count || there.len() != count {
        bail!("unexpected surface output {shape:?}");
    }
    let mut tilted = 0.0f32;
    let out: Vec<[f32; 3]> = (0..count)
        .map(|i| {
            // The model's camera looks along +z: towards it is −z.
            let (x, y, z) = (normal[3 * i], normal[3 * i + 1], -normal[3 * i + 2]);
            let len = (x * x + y * y + z * z).sqrt();
            if there[i] < 0.5 || !len.is_finite() || len < 1e-6 {
                return [0.0, 0.0, 1.0];
            }
            tilted = tilted.max(x.abs().max(y.abs()) / len);
            [x / len, y / len, z / len]
        })
        .collect();
    // (As for depth: a GPU in trouble can answer with nothing at all.)
    if tilted < 1e-3 {
        bail!("the surface model gave an empty map");
    }
    Ok((out, ow as u32, oh as u32))
}

/// The model's long side and tokens for a surface map.
pub const SURFACE_SIDE: u32 = 518;
pub const SURFACE_TOKENS: u32 = 1200;

/// [`surface`] as two pictures of the photo's size: how far each surface leans to the
/// right (black: fully left, white: fully right, mid grey: not at all) and down. Lightly
/// smoothed first: the model's blocks show as a faint grid.
pub fn surface_pictures(
    model: &mut Model,
    img: &RgbImage,
    long_side: u32,
    tokens: u32,
) -> Result<[GrayImage; 2]> {
    let (faces, w, h) = surface(model, img, long_side, tokens)?;
    let (w, h) = (w as usize, h as usize);
    let plane = |c: usize| -> GrayImage {
        let mut v: Vec<f32> = faces.iter().map(|f| f[c]).collect();
        for _ in 0..2 {
            let at = |x: isize, y: isize| {
                v[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize]
            };
            let across: Vec<f32> = (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as isize, (i / w) as isize);
                    0.25 * at(x - 1, y) + 0.5 * at(x, y) + 0.25 * at(x + 1, y)
                })
                .collect();
            let at = |x: isize, y: isize| across[y.clamp(0, h as isize - 1) as usize * w + x as usize];
            v = (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as isize, (i / w) as isize);
                    0.25 * at(x, y - 1) + 0.5 * at(x, y) + 0.25 * at(x, y + 1)
                })
                .collect();
        }
        let coded: Vec<f32> = v.iter().map(|v| 0.5 + 0.5 * v).collect();
        to_mask(&coded, w as u32, h as u32, img.dimensions())
    };
    Ok([plane(0), plane(1)])
}

/// A photo encoded for Segment Anything: the encoder's outputs (fed to the decoder by
/// name) and how its 1024-pixel frame maps to the photo.
pub struct SamImage {
    outputs: Vec<(String, Vec<i64>, Vec<f32>)>,
    /// Photo pixels → the model's frame, per axis.
    scale: (f32, f32),
    /// The photo's part of the model's frame (SAM 1 pads the short side).
    used: (f32, f32),
    size: (u32, u32),
}

/// Which Segment Anything export: SAM 1 (SlimSAM: the long side to 1024, padded) or SAM 2
/// (squashed to 1024 × 1024).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamKind {
    Sam1,
    Sam2,
}

/// Run the image encoder once for a photo.
pub fn sam_encode(model: &mut Model, kind: SamKind, img: &RgbImage) -> Result<SamImage> {
    const S: u32 = 1024;
    let (w, h) = img.dimensions();
    let (pixels, scale, used) = match kind {
        SamKind::Sam2 => {
            (normalised(img, S, S), (S as f32 / w as f32, S as f32 / h as f32), (S as f32, S as f32))
        }
        SamKind::Sam1 => {
            let k = S as f32 / w.max(h) as f32;
            let (rw, rh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
            let inner = normalised(img, rw, rh);
            let (n, m) = ((rw * rh) as usize, (S * S) as usize);
            let mut out = vec![0.0f32; 3 * m];
            for c in 0..3 {
                for y in 0..rh as usize {
                    let row = &inner[c * n + y * rw as usize..c * n + (y + 1) * rw as usize];
                    out[c * m + y * S as usize..c * m + y * S as usize + rw as usize].copy_from_slice(row);
                }
            }
            (out, (k, k), (rw as f32, rh as f32))
        }
    };
    let input = Tensor::from_array(([1usize, 3, S as usize, S as usize], pixels))?;
    let names: Vec<String> = model.io().1;
    let outputs = model.session.run(ort::inputs![input])?;
    let mut kept = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let (shape, data) = outputs[i].try_extract_tensor::<f32>()?;
        kept.push((name.clone(), shape.iter().copied().collect(), data.to_vec()));
    }
    Ok(SamImage { outputs: kept, scale, used, size: img.dimensions() })
}

/// The object at `points` (photo pixels; true: on it, false: not on it).
pub fn sam_decode(model: &mut Model, image: &SamImage, points: &[(f32, f32, bool)]) -> Result<GrayImage> {
    sam_decode_prompt(model, image, points, None)
}

/// The object at `points` and/or inside `rect` (photo pixels: left, top, right, bottom).
pub fn sam_decode_prompt(
    model: &mut Model,
    image: &SamImage,
    points: &[(f32, f32, bool)],
    rect: Option<[f32; 4]>,
) -> Result<GrayImage> {
    let (inputs, _) = model.io();
    // No points: one padding point (label -1), which the prompt encoder ignores.
    let (coords, labels): (Vec<f32>, Vec<i64>) = if points.is_empty() {
        (vec![0.0, 0.0], vec![-1])
    } else {
        (
            points.iter().flat_map(|&(x, y, _)| [x * image.scale.0, y * image.scale.1]).collect(),
            points.iter().map(|&(.., on)| i64::from(on)).collect(),
        )
    };
    let n = labels.len();
    let boxes: Vec<f32> = rect
        .map(|r| vec![r[0] * image.scale.0, r[1] * image.scale.1, r[2] * image.scale.0, r[3] * image.scale.1])
        .unwrap_or_default();
    let mut values: Vec<(String, ort::value::DynValue)> = Vec::new();
    for name in &inputs {
        let value: ort::value::DynValue = match name.as_str() {
            "input_points" => Tensor::from_array(([1usize, 1, n, 2], coords.clone()))?.into_dyn(),
            "input_labels" => Tensor::from_array(([1usize, 1, n], labels.clone()))?.into_dyn(),
            // SAM 2 also takes boxes (none, or one).
            "input_boxes" => Tensor::from_array(([1usize, boxes.len() / 4, 4], boxes.clone()))?.into_dyn(),
            other => {
                let Some((_, shape, data)) = image.outputs.iter().find(|(o, ..)| o == other) else {
                    bail!("the decoder wants {other}, which the encoder doesn't give");
                };
                let shape: Vec<usize> = shape.iter().map(|d| *d as usize).collect();
                Tensor::from_array((shape, data.clone()))?.into_dyn()
            }
        };
        values.push((name.clone(), value));
    }
    let outputs = model.session.run(values)?;
    let (_, scores) = outputs["iou_scores"].try_extract_tensor::<f32>()?;
    let (shape, masks) = outputs["pred_masks"].try_extract_tensor::<f32>()?;
    let (mh, mw) = (shape[shape.len() - 2] as usize, shape[shape.len() - 1] as usize);
    let best = (0..scores.len()).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap_or(0);
    let plane = &masks[best * mh * mw..(best + 1) * mh * mw];
    // The low-resolution logits cover the model's frame; crop to the photo's part.
    let (uw, uh) = (
        (image.used.0 / 1024.0 * mw as f32).round() as u32,
        (image.used.1 / 1024.0 * mh as f32).round() as u32,
    );
    let prob: Vec<f32> =
        (0..uh as usize).flat_map(|y| (0..uw as usize).map(move |x| sigmoid(plane[y * mw + x]))).collect();
    Ok(to_mask(&prob, uw.max(1), uh.max(1), image.size))
}

/// The ONNX Runtime environment, made once for the process.
fn environment() -> Result<&'static ort::environment::Environment> {
    static ENV: OnceLock<ort::environment::Environment> = OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let dylib = store::runtime_lib();
    let env = ort::init_from(&dylib)
        .map_err(|e| anyhow!("loading ONNX Runtime from {}: {e}", dylib.display()))?
        .with_name("photo-library")
        .build()?;
    Ok(ENV.get_or_init(|| env))
}

/// Threads for the CPU: PL_ORT_THREADS, else half the logical cores, at most 8. Laptop
/// chips mix fast and slow cores (and hyper-threads); one thread per logical core runs
/// several times slower than this (measured on an i7-1260P: 1.28 s against 0.25 s).
fn cpu_threads() -> usize {
    std::env::var("PL_ORT_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).clamp(1, 8)))
}

impl Model {
    /// Inputs and outputs with their types and shapes (for checking a model's export).
    pub fn describe(&self) -> String {
        let list = |o: &[ort::value::Outlet]| -> Vec<String> {
            o.iter().map(|o| format!("{}: {:?}", o.name(), o.dtype())).collect()
        };
        format!("in {:?} out {:?}", list(self.session.inputs()), list(self.session.outputs()))
    }
}

/// The photo's main subject with BRIA's RMBG-1.4 (IS-Net): 1024 × 1024 in, normalised to
/// −0.5..0.5; its matte, stretched to the full range.
pub fn subject_rmbg(model: &mut Model, img: &RgbImage) -> Result<GrayImage> {
    const S: u32 = 1024;
    let small = imageops::resize(img, S, S, imageops::FilterType::Triangle);
    let n = (S * S) as usize;
    let mut pixels = vec![0.0f32; 3 * n];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            pixels[c * n + i] = f32::from(p[c]) / 255.0 - 0.5;
        }
    }
    let input = Tensor::from_array(([1usize, 3, S as usize, S as usize], pixels))?;
    let outputs = model.session.run(ort::inputs![input])?;
    let (_, matte) = outputs[0].try_extract_tensor::<f32>()?;
    let (lo, hi) = matte.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let prob: Vec<f32> = matte.iter().map(|v| (v - lo) / (hi - lo).max(1e-6)).collect();
    Ok(to_mask(&prob, S, S, img.dimensions()))
}

/// The last model used, kept loaded (a session holds GPU memory, so only one).
static LOADED: std::sync::Mutex<Option<(AiModel, Device, Model)>> = std::sync::Mutex::new(None);

/// Run `model` on `img` (the developed photo, upright, at a working size): its mask at
/// the image's size. On the GPU (DirectML) when `gpu`, falling back to the CPU if the GPU
/// can't load or run it (too little memory, an unsupported operation). Returns the device
/// it ran on.
pub fn select(model: AiModel, img: &RgbImage, gpu: bool) -> Result<(GrayImage, Device)> {
    on_device(model, gpu, |session| match model {
        AiModel::Subject => subject_rmbg(session, img),
        AiModel::Sky => sky(session, img),
        AiModel::Depth => depth_picture(session, img),
        AiModel::Object
        | AiModel::Remove
        | AiModel::RemoveBest
        | AiModel::Scratches
        | AiModel::People
        | AiModel::Surface => {
            bail!("{} isn't a selection", model.label())
        }
    })
}

/// Which way every surface in `img` faces (see [`surface_pictures`]), on the GPU when
/// `gpu` and else, or if that fails, on the CPU. Returns the device it ran on.
pub fn select_surface(img: &RgbImage, gpu: bool) -> Result<([GrayImage; 2], Device)> {
    on_device(AiModel::Surface, gpu, |session| surface_pictures(session, img, SURFACE_SIDE, SURFACE_TOKENS))
}

/// `run` with `model` loaded (kept loaded for the next time): on the GPU (DirectML) when
/// `gpu`, falling back to the CPU if the GPU can't load or run it.
fn on_device<T>(model: AiModel, gpu: bool, run: impl Fn(&mut Model) -> Result<T>) -> Result<(T, Device)> {
    let devices: &[Device] =
        if gpu && cfg!(windows) { &[Device::DirectMl, Device::Cpu] } else { &[Device::Cpu] };
    let mut last = None;
    for &device in devices {
        let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(&*loaded, Some((m, d, _)) if *m == model && *d == device) {
            *loaded = None;
            match Model::load(&model.path(), device) {
                Ok(m) => *loaded = Some((model, device, m)),
                Err(e) => {
                    log::warn!("AI masks: {} on {}: {e:#}", model.label(), device.label());
                    last = Some(e);
                    continue;
                }
            }
        }
        let Some((_, _, session)) = loaded.as_mut() else { continue };
        match run(session) {
            Ok(mask) => return Ok((mask, device)),
            Err(e) => {
                log::warn!("AI masks: {} failed on {}: {e:#}", model.label(), device.label());
                *loaded = None;
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no device to run {} on", model.label())))
}

/// Segment Anything's two sessions, and the last photo encoded (by the caller's key).
struct Sam {
    device: Device,
    encoder: Model,
    decoder: Model,
}
static SAM: std::sync::Mutex<Option<Sam>> = std::sync::Mutex::new(None);
static ENCODED: std::sync::Mutex<Option<(u64, SamImage)>> = std::sync::Mutex::new(None);

/// Is the photo `key` (the caller's: the photo and how it was rendered) encoded already?
/// Then [`select_object`] needs no image.
pub fn object_encoded(key: u64) -> bool {
    ENCODED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(k, _)| *k == key)
}

/// Select Object: the thing at `points` and/or inside `rect` (pixels of `img`), with
/// Segment Anything 2.1 on the GPU (DirectML) when `gpu`, else (or if that fails) the CPU.
/// The encoding of photo `key` is kept, so more objects on it take only the decoder.
pub fn select_object(
    key: u64,
    img: &RgbImage,
    points: &[(f32, f32, bool)],
    rect: Option<[f32; 4]>,
    gpu: bool,
) -> Result<(GrayImage, Device)> {
    let devices: &[Device] =
        if gpu && cfg!(windows) { &[Device::DirectMl, Device::Cpu] } else { &[Device::Cpu] };
    let model = AiModel::Object;
    let mut last = None;
    for &device in devices {
        let mut sam = SAM.lock().unwrap_or_else(|e| e.into_inner());
        if sam.as_ref().is_none_or(|s| s.device != device) {
            *sam = None;
            // The decoder always on the CPU: it is small (a fifth of a second), and its inputs
            // change shape with the prompt, which makes DirectML rebuild it every time.
            let loaded = Model::load(&model.file(0), device)
                .and_then(|encoder| Ok((encoder, Model::load(&model.file(1), Device::Cpu)?)));
            match loaded {
                Ok((encoder, decoder)) => *sam = Some(Sam { device, encoder, decoder }),
                Err(e) => {
                    log::warn!("AI masks: {} on {}: {e:#}", model.label(), device.label());
                    last = Some(e);
                    continue;
                }
            }
        }
        let Some(s) = sam.as_mut() else { continue };
        let mut encoded = ENCODED.lock().unwrap_or_else(|e| e.into_inner());
        let result = (|| {
            if !encoded.as_ref().is_some_and(|(k, _)| *k == key) {
                if img.width() == 0 {
                    bail!("the photo isn't encoded");
                }
                *encoded = None;
                *encoded = Some((key, sam_encode(&mut s.encoder, SamKind::Sam2, img)?));
            }
            let Some((_, image)) = encoded.as_ref() else { bail!("no encoding") };
            sam_decode_prompt(&mut s.decoder, image, points, rect)
        })();
        match result {
            Ok(mask) => return Ok((mask, device)),
            Err(e) => {
                log::warn!("AI masks: {} failed on {}: {e:#}", model.label(), device.label());
                *sam = None;
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no device to run {} on", model.label())))
}

/// The Remove models, each kept loaded on its own (big-LaMa takes many seconds to load, and
/// the masks' models come and go).
static REMOVE: std::sync::Mutex<Option<(Device, Model)>> = std::sync::Mutex::new(None);
static REMOVE_BEST: std::sync::Mutex<Option<(Device, Model)>> = std::sync::Mutex::new(None);

fn with_remove<T>(
    best: bool,
    gpu: bool,
    run: &mut dyn FnMut(&mut Model) -> Result<T>,
) -> Result<(T, Device)> {
    let devices: &[Device] =
        if gpu && cfg!(windows) { &[Device::DirectMl, Device::Cpu] } else { &[Device::Cpu] };
    let (model, slot) = if best { (AiModel::RemoveBest, &REMOVE_BEST) } else { (AiModel::Remove, &REMOVE) };
    let mut last = None;
    for &device in devices {
        let mut loaded = slot.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(&*loaded, Some((d, _)) if *d == device) {
            *loaded = None;
            let started = std::time::Instant::now();
            // Big-LaMa's int4/int8 weights decode wrongly on DirectML: it runs a copy with
            // them decoded beforehand (made once, on the CPU).
            let path = match (best, device) {
                (true, Device::DirectMl) => match folded(&model.path()) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("AI: preparing {} for the GPU: {e:#}", model.label());
                        last = Some(e);
                        continue;
                    }
                },
                _ => model.path(),
            };
            match Model::load(&path, device) {
                Ok(m) => {
                    log::info!(
                        "AI: {} loaded on {} in {:.1?}",
                        model.label(),
                        device.label(),
                        started.elapsed()
                    );
                    *loaded = Some((device, m));
                }
                Err(e) => {
                    log::warn!("AI: {} on {}: {e:#}", model.label(), device.label());
                    last = Some(e);
                    continue;
                }
            }
        }
        let Some((_, session)) = loaded.as_mut() else { continue };
        match run(session) {
            Ok(v) => return Ok((v, device)),
            Err(e) => {
                log::warn!("AI: {} failed on {}: {e:#}", model.label(), device.label());
                *loaded = None;
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no device to run {} on", model.label())))
}

/// Load a Remove model ahead of use (when the Remove tool opens), if it is downloaded.
pub fn warm_remove(best: bool, gpu: bool) {
    let model = if best { AiModel::RemoveBest } else { AiModel::Remove };
    if ready(&[model]) {
        let _ = with_remove(best, gpu, &mut |_| Ok(()));
    }
}

/// Remove: fill `mask` (255: fill) of `img`, both [`inpaint::SIZE`] square, with LaMa-Dilated
/// (or big-LaMa when `best`).
pub fn remove(img: &RgbImage, mask: &GrayImage, best: bool, gpu: bool) -> Result<(RgbImage, Device)> {
    with_remove(best, gpu, &mut |m| inpaint::lama(m, img, mask))
}

/// A copy of a model with its quantized (int4/int8) weights decoded to floats, next to
/// it as `<name>.folded.onnx`, made once on the CPU. DirectML decodes such weights wrongly
/// (big-LaMa's come out as nonsense and its fills black); with them already floats it
/// gives the CPU's results.
fn folded(src: &Path) -> Result<std::path::PathBuf> {
    let dst = src.with_extension("folded.onnx");
    if dst.exists() {
        return Ok(dst);
    }
    let part = src.with_extension("folded.part");
    let started = std::time::Instant::now();
    let err = |e: ort::Error<ort::session::builder::SessionBuilder>| anyhow!("{e}");
    // Basic optimisation is constant folding (no device-specific fusions), which decodes
    // the weights once quantized-graph handling is off.
    Session::builder(environment()?)?
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level1)
        .map_err(err)?
        .with_config_entry("session.disable_quant_qdq", "1")
        .map_err(err)?
        .with_optimized_model_path(&part)
        .map_err(err)?
        .with_execution_providers([ort::ep::CPU::default().build()])
        .map_err(err)?
        .commit_from_file(src)
        .with_context(|| format!("decoding {}", src.display()))?;
    std::fs::rename(&part, &dst)?;
    log::info!("AI: decoded {} for the GPU in {:.1?}", src.display(), started.elapsed());
    Ok(dst)
}

/// COCO classes (of the detector's 80) that go with the person carrying or riding them.
const BELONGINGS: [usize; 8] = [
    24, // backpack
    25, // umbrella
    26, // handbag
    28, // suitcase
    32, // sports ball
    33, // kite
    36, // skateboard
    37, // surfboard
];

/// A person found in a photo: where (photo pixels: left, top, right, bottom), how sure the
/// detector is (0..1), and their outline over `rect` (with what they carry), 255 inside.
pub struct Person {
    pub rect: [u32; 4],
    pub score: f32,
    pub mask: GrayImage,
}

/// A person with the shadow they cast on the ground, if one is found: ground beside
/// their feet that is clearly darker than the ground around it but of its colour, joined
/// to the feet, reaching no farther than the person is tall, and with lit ground around
/// most of it. `None` when there is none to be sure of (shade all around, a dark floor,
/// something dark that goes on out of sight): a guess that only takes what it can tell.
pub fn with_shadow(img: &RgbImage, person: &Person) -> Option<Person> {
    let (w, h) = img.dimensions();
    let [l, t, r, b] = person.rect;
    let (pw, ph) = (r.saturating_sub(l), b.saturating_sub(t));
    if pw < 8 || ph < 16 || person.mask.dimensions() != (pw, ph) {
        return None;
    }
    // Around the feet: the person's height to either side, from the waist down to a little
    // more than half their height below.
    let x0 = l.saturating_sub(ph);
    let y0 = t + ph / 2;
    let x1 = (r + ph).min(w);
    let y1 = (b + ph * 6 / 10).min(h);
    if x1 <= x0 + 8 || y1 <= y0 + 8 {
        return None;
    }
    // Small: a shadow is a large soft thing, and grain is not to count.
    let k = ((x1 - x0).max(y1 - y0) as f32 / 256.0).max(1.0);
    let (sw, sh) = ((((x1 - x0) as f32 / k) as u32).max(8), (((y1 - y0) as f32 / k) as u32).max(8));
    let crop = imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
    let small = imageops::resize(&crop, sw, sh, imageops::FilterType::Triangle);
    let (sw, sh) = (sw as usize, sh as usize);
    let n = sw * sh;
    let body: Vec<bool> = (0..n)
        .map(|i| {
            let px = x0 as f32 + ((i % sw) as f32 + 0.5) * k;
            let py = y0 as f32 + ((i / sw) as f32 + 0.5) * k;
            px >= l as f32
                && py >= t as f32
                && (px as u32) < r
                && (py as u32) < b
                && person.mask.get_pixel(px as u32 - l, py as u32 - t)[0] > 0
        })
        .collect();
    // Light (linear, so that "half as bright" means it) and colour apart from lightness.
    let lin = |v: u8| (f32::from(v) / 255.0).powf(2.2);
    let (mut light, mut tone) = (vec![0.0f32; n], vec![[0.0f32; 2]; n]);
    for (i, p) in small.pixels().enumerate() {
        let (rr, g, bb) = (lin(p[0]), lin(p[1]), lin(p[2]));
        let sum = (rr + g + bb).max(1e-4);
        light[i] = 0.2126 * rr + 0.7152 * g + 0.0722 * bb;
        tone[i] = [rr / sum, bb / sum];
    }
    // The ground by the feet: what is not the person, from a little above their feet down.
    let feet = ((b as f32 - 0.12 * ph as f32 - y0 as f32) / k).max(0.0) as usize;
    let ground: Vec<usize> = (0..n).filter(|i| !body[*i] && i / sw >= feet).collect();
    if ground.len() < 50 {
        return None;
    }
    let mut lights: Vec<f32> = ground.iter().map(|i| light[*i]).collect();
    lights.sort_by(f32::total_cmp);
    let lit = lights[lights.len() * 3 / 4];
    if lit < 0.02 {
        return None;
    }
    let bright: Vec<usize> = ground.iter().copied().filter(|i| light[*i] >= 0.8 * lit).collect();
    let middle = |c: usize| {
        let mut v: Vec<f32> = bright.iter().map(|i| tone[*i][c]).collect();
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let colour = [middle(0), middle(1)];
    // Shadow, perhaps: clearly darker than the lit ground, of about its colour (a shadow is
    // lit by the sky: a little bluer).
    let dark: Vec<bool> = (0..n)
        .map(|i| {
            !body[i]
                && light[i] < 0.6 * lit
                && (tone[i][0] - colour[0]).abs() < 0.07
                && (tone[i][1] - colour[1]).abs() < 0.09
        })
        .collect();
    // From the feet: the person's lowest part, a little grown.
    let low = ((b as f32 - 0.15 * ph as f32 - y0 as f32) / k).max(0.0) as usize;
    let reach = ph as f32 / k;
    let (fx, fy) = (((l + r) as f32 / 2.0 - x0 as f32) / k, (b as f32 - y0 as f32) / k);
    let mut shadow = vec![false; n];
    let mut queue: Vec<usize> = Vec::new();
    for y in low..sh {
        for x in 0..sw {
            if !body[y * sw + x] {
                continue;
            }
            for dy in -2i32..=3 {
                for dx in -2i32..=2 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= sw as i32 || ny >= sh as i32 {
                        continue;
                    }
                    let j = ny as usize * sw + nx as usize;
                    if dark[j] && !shadow[j] {
                        shadow[j] = true;
                        queue.push(j);
                    }
                }
            }
        }
    }
    while let Some(i) = queue.pop() {
        let (x, y) = ((i % sw) as i32, (i / sw) as i32);
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= sw as i32 || ny >= sh as i32 {
                continue;
            }
            let j = ny as usize * sw + nx as usize;
            if dark[j] && !shadow[j] && (nx as f32 - fx).hypot(ny as f32 - fy) <= reach {
                shadow[j] = true;
                queue.push(j);
            }
        }
    }
    // Is it a shadow? Not too little or too much of it, not going on out of sight, and
    // lit ground around most of it.
    let area = shadow.iter().filter(|s| **s).count();
    let person_area = body.iter().filter(|s| **s).count().max(1);
    let whole = person.mask.pixels().filter(|p| p[0] > 0).count() as f32 / (k * k);
    if (area as f32) < 0.03 * whole || (area as f32) > 1.2 * whole.max(person_area as f32) {
        return None;
    }
    let (mut around, mut lit_around, mut cut) = (0u32, 0u32, 0u32);
    for i in (0..n).filter(|i| shadow[*i]) {
        let (x, y) = ((i % sw) as i32, (i / sw) as i32);
        // (The picture's own edges do not count: only where the photo goes on.)
        let at_edge = (x == 0 && x0 > 0)
            || (x == sw as i32 - 1 && x1 < w)
            || (y == sh as i32 - 1 && y1 < h)
            || (f32::from(x as i16) - fx).hypot(f32::from(y as i16) - fy) > reach - 1.5;
        cut += u32::from(at_edge);
        for (dx, dy) in [(-2, 0), (2, 0), (0, -2), (0, 2)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= sw as i32 || ny >= sh as i32 {
                continue;
            }
            let j = ny as usize * sw + nx as usize;
            if !shadow[j] && !body[j] {
                around += 1;
                lit_around += u32::from(light[j] >= 0.75 * lit);
            }
        }
    }
    if cut as f32 > 0.04 * area as f32 || around == 0 || (lit_around as f32) < 0.6 * around as f32 {
        return None;
    }
    // The person and the shadow (a little grown: its edge is soft), over both.
    let mut grown = shadow.clone();
    for i in (0..n).filter(|i| shadow[*i]) {
        let (x, y) = ((i % sw) as i32, (i / sw) as i32);
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (nx, ny) = (x + dx, y + dy);
                if nx >= 0 && ny >= 0 && nx < sw as i32 && ny < sh as i32 {
                    grown[ny as usize * sw + nx as usize] = true;
                }
            }
        }
    }
    let (mut bx0, mut by0, mut bx1, mut by1) = (l, t, r, b);
    for i in (0..n).filter(|i| grown[*i]) {
        let px = x0 + ((i % sw) as f32 * k) as u32;
        let py = y0 + ((i / sw) as f32 * k) as u32;
        (bx0, by0) = (bx0.min(px), by0.min(py));
        (bx1, by1) = (bx1.max((px + k.ceil() as u32 + 1).min(w)), by1.max((py + k.ceil() as u32 + 1).min(h)));
    }
    let mask = GrayImage::from_fn(bx1 - bx0, by1 - by0, |x, y| {
        let (px, py) = (bx0 + x, by0 + y);
        let own = px >= l && py >= t && px < r && py < b && person.mask.get_pixel(px - l, py - t)[0] > 0;
        let cast = px >= x0 && py >= y0 && px < x1 && py < y1 && {
            let sx = (((px - x0) as f32 / k) as usize).min(sw - 1);
            let sy = (((py - y0) as f32 / k) as usize).min(sh - 1);
            grown[sy * sw + sx]
        };
        image::Luma([if own || cast { 255 } else { 0 }])
    });
    Some(Person { rect: [bx0, by0, bx1, by1], score: person.score, mask })
}

/// Distractions → People: the people in `img` (the photo at full size, upright), down to
/// a detector score of `threshold`, with their outlines.
///
/// The detector (RT-DETR, on the GPU when `gpu`) looks at the whole photo and at
/// overlapping 1280 px tiles of it, so small, distant people are found too; boxes cut by a
/// tile's inner edge are left to the tile holding them, the rest merged. Bags, umbrellas,
/// boards and the like overlapping a person go with them. The outlines come from Segment
/// Anything on crops around each person (several close people share one), on the CPU
/// (faster there than DirectML for its image encoder). `progress` hears the share done and
/// what is happening.
pub fn find_people(
    img: &RgbImage,
    threshold: f32,
    gpu: bool,
    progress: &dyn Fn(f32, &str),
) -> Result<Vec<Person>> {
    const TILE: u32 = 1280;
    const SIDE: u32 = 640;
    let (w, h) = img.dimensions();
    let mut views = vec![(0, 0, w, h)];
    let step = TILE * 3 / 4;
    if w > TILE || h > TILE {
        let starts = |n: u32| -> Vec<u32> {
            let mut v: Vec<u32> = (0..n.saturating_sub(TILE)).step_by(step as usize).collect();
            v.push(n.saturating_sub(TILE));
            v.dedup();
            v
        };
        for y in starts(h) {
            for x in starts(w) {
                views.push((x, y, TILE.min(w), TILE.min(h)));
            }
        }
    }
    // (left, top, right, bottom, score, class)
    let mut found: Vec<([f32; 4], f32, usize)> = Vec::new();
    let mut detect = |model: &mut Model| -> Result<()> {
        found.clear();
        for (k, &(vx, vy, vw, vh)) in views.iter().enumerate() {
            progress(0.6 * k as f32 / views.len() as f32, "Finding people…");
            let crop = imageops::crop_imm(img, vx, vy, vw, vh).to_image();
            let small = imageops::resize(&crop, SIDE, SIDE, imageops::FilterType::Triangle);
            let n = (SIDE * SIDE) as usize;
            let mut pixels = vec![0.0f32; 3 * n];
            for (i, p) in small.pixels().enumerate() {
                for c in 0..3 {
                    pixels[c * n + i] = f32::from(p[c]) / 255.0;
                }
            }
            let input = Tensor::from_array(([1usize, 3, SIDE as usize, SIDE as usize], pixels))?;
            let outputs = model.session.run(ort::inputs!["pixel_values" => input])?;
            let (shape, logits) = outputs["logits"].try_extract_tensor::<f32>()?;
            let (_, boxes) = outputs["pred_boxes"].try_extract_tensor::<f32>()?;
            let classes = shape[shape.len() - 1] as usize;
            for q in 0..logits.len() / classes {
                let row = &logits[q * classes..(q + 1) * classes];
                let (c, best) = row
                    .iter()
                    .enumerate()
                    .fold((0, f32::MIN), |m, (i, v)| if *v > m.1 { (i, *v) } else { m });
                let score = sigmoid(best);
                if score < threshold || (c != 0 && !BELONGINGS.contains(&c)) {
                    continue;
                }
                let b = &boxes[q * 4..q * 4 + 4];
                let (vx, vy, vw, vh) = (vx as f32, vy as f32, vw as f32, vh as f32);
                let r = [
                    vx + (b[0] - b[2] / 2.0) * vw,
                    vy + (b[1] - b[3] / 2.0) * vh,
                    vx + (b[0] + b[2] / 2.0) * vw,
                    vy + (b[1] + b[3] / 2.0) * vh,
                ];
                // Cut by a tile's inner edge: another tile holds it whole.
                const EDGE: f32 = 4.0;
                let cut = (r[0] < vx + EDGE && vx > 0.0)
                    || (r[1] < vy + EDGE && vy > 0.0)
                    || (r[2] > vx + vw - EDGE && vx + vw < w as f32)
                    || (r[3] > vy + vh - EDGE && vy + vh < h as f32);
                if !cut {
                    found.push((r, score, c));
                }
            }
        }
        Ok(())
    };
    let devices: &[Device] =
        if gpu && cfg!(windows) { &[Device::DirectMl, Device::Cpu] } else { &[Device::Cpu] };
    let mut done = false;
    let mut last = None;
    for &device in devices {
        let mut model = match Model::load(&AiModel::People.path(), device) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("AI: {} on {}: {e:#}", AiModel::People.label(), device.label());
                last = Some(e);
                continue;
            }
        };
        match detect(&mut model) {
            Ok(()) => {
                done = true;
                break;
            }
            Err(e) => {
                log::warn!("AI: {} failed on {}: {e:#}", AiModel::People.label(), device.label());
                last = Some(e);
            }
        }
    }
    if !done {
        return Err(last.unwrap_or_else(|| anyhow!("no device to find people on")));
    }
    // One box per thing: the surest of those overlapping much (by the smaller's area).
    let overlap = |a: &[f32; 4], b: &[f32; 4]| {
        let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
        let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
        let area = |r: &[f32; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(1e-6);
        ix * iy / area(a).min(area(b))
    };
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut kept: Vec<([f32; 4], f32, usize)> = Vec::new();
    for f in found {
        if kept.iter().all(|k| k.2 != f.2 || overlap(&k.0, &f.0) <= 0.6) {
            kept.push(f);
        }
    }
    let (people, things): (Vec<_>, Vec<_>) = kept.into_iter().partition(|k| k.2 == 0);
    // Each belonging goes with the person its box overlaps most (at least 30% of it, with
    // the person's box grown by a tenth).
    let mut groups: Vec<(Vec<[f32; 4]>, f32)> = people.iter().map(|p| (vec![p.0], p.1)).collect();
    for t in &things {
        let grown = |r: &[f32; 4]| {
            let (gx, gy) = ((r[2] - r[0]) * 0.1, (r[3] - r[1]) * 0.1);
            [r[0] - gx, r[1] - gy, r[2] + gx, r[3] + gy]
        };
        let share = |p: &[f32; 4]| {
            let g = grown(p);
            let ix = (g[2].min(t.0[2]) - g[0].max(t.0[0])).max(0.0);
            let iy = (g[3].min(t.0[3]) - g[1].max(t.0[1])).max(0.0);
            ix * iy / ((t.0[2] - t.0[0]) * (t.0[3] - t.0[1])).max(1e-6)
        };
        if let Some((i, s)) =
            people.iter().enumerate().map(|(i, p)| (i, share(&p.0))).max_by(|a, b| a.1.total_cmp(&b.1))
            && s >= 0.3
        {
            groups[i].0.push(t.0);
        }
    }
    // Outlines: Segment Anything with each box on a crop around it (the box three times
    // over, at least 256 px, square), crops of nearby people merged while they stay small
    // enough to keep their detail (1024 px: the encoder's size).
    let crop_of = |r: &[f32; 4]| {
        let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let s = ((r[2] - r[0]).max(r[3] - r[1]) * 3.0).max(256.0);
        [cx - s / 2.0, cy - s / 2.0, cx + s / 2.0, cy + s / 2.0]
    };
    let union = |a: &[f32; 4], b: &[f32; 4]| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
    let mut crops: Vec<([f32; 4], Vec<usize>)> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        let c = g.0.iter().map(crop_of).reduce(|a, b| union(&a, &b)).unwrap_or([0.0; 4]);
        let joined = crops.iter_mut().find(|(k, _)| {
            let u = union(k, &c);
            overlap(k, &c) > 0.0 && (u[2] - u[0]).max(u[3] - u[1]) <= 1024.0
        });
        match joined {
            Some((k, members)) => {
                *k = union(k, &c);
                members.push(i);
            }
            None => crops.push((c, vec![i])),
        }
    }
    let mut out = Vec::new();
    let photo_key = {
        use std::hash::{Hash, Hasher};
        let mut hsh = std::collections::hash_map::DefaultHasher::new();
        (w, h, img.as_raw().len(), &img.as_raw()[..img.as_raw().len().min(4096)]).hash(&mut hsh);
        hsh.finish()
    };
    for (n, (c, members)) in crops.iter().enumerate() {
        progress(0.6 + 0.4 * n as f32 / crops.len() as f32, "Outlining people…");
        let x0 = c[0].max(0.0) as u32;
        let y0 = c[1].max(0.0) as u32;
        let x1 = (c[2].ceil() as u32).min(w);
        let y1 = (c[3].ceil() as u32).min(h);
        if x1 <= x0 + 1 || y1 <= y0 + 1 {
            continue;
        }
        let crop = imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
        let key = photo_key ^ (u64::from(x0) << 40) ^ (u64::from(y0) << 20) ^ u64::from(x1 * 31 + y1);
        for &i in members {
            let (boxes, score) = &groups[i];
            // The person and what they carry, each outlined, together.
            let mut mask = GrayImage::new(x1 - x0, y1 - y0);
            for b in boxes {
                let rect = [b[0] - x0 as f32, b[1] - y0 as f32, b[2] - x0 as f32, b[3] - y0 as f32];
                let (m, _) = select_object(key, &crop, &[], Some(rect), false)?;
                for (o, v) in mask.pixels_mut().zip(m.pixels()) {
                    if v[0] >= 128 {
                        o[0] = 255;
                    }
                }
            }
            // Larger by a share of the person's size, and cut to its box. The outline found
            // sits on the person's edge, and a soft edge, hair, motion blur and a close
            // shadow reach past it: left outside, they stay in the photo and the fill is made
            // from them too, a pale shape of the person where they stood.
            let all = boxes.iter().copied().reduce(|a, b| union(&a, &b)).unwrap_or([0.0; 4]);
            let reach = ((all[2] - all[0]).max(all[3] - all[1]) * 0.02).clamp(2.0, 32.0);
            let grown = dilate(&mask, reach.round() as u32);
            // Room for what the outline grew by.
            let room = |side: f32| (side * 0.15 + 4.0).max(reach + 2.0);
            let (gx, gy) = (room(all[2] - all[0]), room(all[3] - all[1]));
            let bx0 = ((all[0] - gx).max(x0 as f32) as u32).max(x0);
            let by0 = ((all[1] - gy).max(y0 as f32) as u32).max(y0);
            let bx1 = ((all[2] + gx).ceil() as u32).min(x1);
            let by1 = ((all[3] + gy).ceil() as u32).min(y1);
            if bx1 <= bx0 || by1 <= by0 {
                continue;
            }
            let cut = imageops::crop_imm(&grown, bx0 - x0, by0 - y0, bx1 - bx0, by1 - by0).to_image();
            if cut.pixels().all(|p| p[0] == 0) {
                continue;
            }
            out.push(Person { rect: [bx0, by0, bx1, by1], score: *score, mask: cut });
        }
    }
    progress(1.0, "Done");
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// `mask` (0 or 255) with everything set made larger by `r` pixels each way (a square
/// reach: two passes of counts along the rows, then the columns).
fn dilate(mask: &GrayImage, r: u32) -> GrayImage {
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let r = r as usize;
    // One direction: is anything set within `r` along a line of `n` values?
    let pass = |src: &[bool], n: usize, lines: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut out = vec![false; src.len()];
        let mut sums = vec![0u32; n + 1];
        for line in 0..lines {
            for i in 0..n {
                sums[i + 1] = sums[i] + u32::from(src[at(line, i)]);
            }
            for i in 0..n {
                let (lo, hi) = (i.saturating_sub(r), (i + r + 1).min(n));
                out[at(line, i)] = sums[hi] > sums[lo];
            }
        }
        out
    };
    let set: Vec<bool> = mask.as_raw().iter().map(|v| *v >= 128).collect();
    let rows = pass(&set, w, h, &|y, x| y * w + x);
    let both = pass(&rows, h, w, &|x, y| y * w + x);
    GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        image::Luma([if both[y as usize * w + x as usize] { 255 } else { 0 }])
    })
}

/// The scratch detector, kept loaded while it is used.
static SCRATCH: std::sync::Mutex<Option<(Device, Model)>> = std::sync::Mutex::new(None);

/// Dust & Scratches with the AI model: the chance (0..255) that each pixel of a photo's
/// lightness (`gray` 0..1, `w × h`) is dust, a hair or a scratch. The model looks at the
/// photo made `scale` times smaller (it was trained on small prints; film specks are a few
/// pixels), in 512 px tiles overlapping by 32; `progress` hears the share done. On the GPU
/// (DirectML) when `gpu`, else (or if that fails) the CPU.
pub fn scratches(
    gray: &[f32],
    w: usize,
    h: usize,
    scale: f32,
    gpu: bool,
    progress: &dyn Fn(f32),
) -> Result<(Vec<u8>, Device)> {
    use image::{ImageBuffer, Luma};
    const TILE: usize = 512;
    const PAD: usize = 32;
    let full: ImageBuffer<Luma<f32>, Vec<f32>> =
        ImageBuffer::from_raw(w as u32, h as u32, gray.to_vec()).context("the photo's size")?;
    let (sw, sh) =
        (((w as f32 * scale).round() as usize).max(1), ((h as f32 * scale).round() as usize).max(1));
    let small = imageops::resize(&full, sw as u32, sh as u32, imageops::FilterType::Triangle);
    let px = small.as_raw();
    // Reflected at the borders, so every tile is the same size (DirectML compiles once).
    let at = |x: isize, y: isize| {
        let r = |v: isize, n: usize| -> usize {
            let n = n as isize;
            let v = if v < 0 { -v } else { v };
            let v = v % (2 * n);
            (if v >= n { 2 * n - 1 - v } else { v }) as usize
        };
        px[r(y, sh) * sw + r(x, sw)]
    };
    let step = TILE - 2 * PAD;
    let tiles: Vec<(usize, usize)> =
        (0..sh).step_by(step).flat_map(|y| (0..sw).step_by(step).map(move |x| (x, y))).collect();
    let run = |model: &mut Model| -> Result<Vec<f32>> {
        let mut prob = vec![0.0f32; sw * sh];
        for (k, &(x0, y0)) in tiles.iter().enumerate() {
            let mut input = vec![0.0f32; TILE * TILE];
            for ty in 0..TILE {
                for tx in 0..TILE {
                    let v = at(
                        x0 as isize + tx as isize - PAD as isize,
                        y0 as isize + ty as isize - PAD as isize,
                    );
                    input[ty * TILE + tx] = v * 2.0 - 1.0;
                }
            }
            let tensor = Tensor::from_array(([1usize, 1, TILE, TILE], input))?;
            let outputs = model.session.run(ort::inputs![tensor])?;
            let (_, out) = outputs[0].try_extract_tensor::<f32>()?;
            let (tw, th) = (step.min(sw - x0), step.min(sh - y0));
            for ty in 0..th {
                for tx in 0..tw {
                    prob[(y0 + ty) * sw + x0 + tx] = out[(ty + PAD) * TILE + tx + PAD];
                }
            }
            progress((k + 1) as f32 / tiles.len() as f32);
        }
        Ok(prob)
    };
    let devices: &[Device] =
        if gpu && cfg!(windows) { &[Device::DirectMl, Device::Cpu] } else { &[Device::Cpu] };
    let model = AiModel::Scratches;
    let mut last = None;
    for &device in devices {
        let mut loaded = SCRATCH.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(&*loaded, Some((d, _)) if *d == device) {
            *loaded = None;
            match Model::load(&model.path(), device) {
                Ok(m) => *loaded = Some((device, m)),
                Err(e) => {
                    log::warn!("AI: {} on {}: {e:#}", model.label(), device.label());
                    last = Some(e);
                    continue;
                }
            }
        }
        let Some((_, session)) = loaded.as_mut() else { continue };
        match run(session) {
            Ok(prob) => {
                *loaded = None;
                let small: ImageBuffer<Luma<f32>, Vec<f32>> =
                    ImageBuffer::from_raw(sw as u32, sh as u32, prob).context("the map's size")?;
                let back = imageops::resize(&small, w as u32, h as u32, imageops::FilterType::Triangle);
                let map = back.as_raw().iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
                return Ok((map, device));
            }
            Err(e) => {
                log::warn!("AI: {} failed on {}: {e:#}", model.label(), device.label());
                *loaded = None;
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no device to run {} on", model.label())))
}

/// Unload the Remove models (they hold GPU memory another model may need).
pub fn unload_remove() {
    *REMOVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *REMOVE_BEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_persons_shadow_is_found_when_it_is_one() {
        // Light ground, a dark figure standing on it, and (when `cast`) their shadow lying
        // to the right of their feet; `floor` darkens all the ground below the knees.
        let scene = |cast: bool, floor: bool| {
            RgbImage::from_fn(400, 300, |x, y| {
                let ground = [200.0f32, 190.0, 170.0];
                let k = if (180..220).contains(&x) && (60..200).contains(&y) {
                    return image::Rgb([60, 40, 40]);
                } else if floor && y > 150 {
                    0.6
                } else if cast && (220..300).contains(&x) && (184..204).contains(&y) {
                    0.62
                } else {
                    1.0
                };
                image::Rgb(ground.map(|v| (v * k) as u8))
            })
        };
        let person = Person {
            rect: [180, 60, 220, 200],
            score: 0.9,
            mask: GrayImage::from_pixel(40, 140, image::Luma([255])),
        };
        let found = with_shadow(&scene(true, false), &person).expect("a shadow");
        let [l, t, r, b] = found.rect;
        assert!(l <= 180 && t <= 60 && r >= 296 && b >= 202, "{:?}", found.rect);
        assert!(r <= 312 && b <= 216, "no more than the shadow: {:?}", found.rect);
        assert_eq!(found.mask.dimensions(), (r - l, b - t));
        let at = |x: u32, y: u32| found.mask.get_pixel(x - l, y - t)[0];
        assert_eq!((at(260, 194), at(200, 100), at(260, 150)), (255, 255, 0));
        // No shadow, and a floor dark all over: nothing to be sure of.
        assert!(with_shadow(&scene(false, false), &person).is_none());
        assert!(with_shadow(&scene(false, true), &person).is_none());
    }

    #[test]
    fn dilate_grows_by_its_reach_each_way() {
        let mut mask = GrayImage::new(20, 12);
        mask.put_pixel(10, 6, image::Luma([255]));
        mask.put_pixel(0, 0, image::Luma([255]));
        let grown = dilate(&mask, 3);
        let on = |x: u32, y: u32| grown.get_pixel(x, y)[0] == 255;
        // A square of 7 around the pixel, no further.
        assert!(on(7, 3) && on(13, 9) && on(10, 6));
        assert!(!on(6, 6) && !on(14, 6) && !on(10, 2) && !on(10, 10));
        // At the corner it stops at the picture's edge.
        assert!(on(3, 3) && !on(4, 0) && !on(0, 4));
        // No reach: unchanged.
        assert_eq!(dilate(&mask, 0), mask);
    }
}
