//! AI masks for Develop's masking: the photo's subject (BiRefNet), its sky (SegFormer
//! trained on ADE20K) and objects picked with points (Segment Anything), from ONNX models
//! run with ONNX Runtime on the CPU or, through DirectML, on any DirectX 12 GPU.
//!
//! Each function takes an 8-bit RGB image (the photo at a working size) and returns a
//! mask of the same size (0..255).

use std::path::Path;

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
                .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
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
    let (inputs, _) = model.io();
    let n = points.len();
    let coords: Vec<f32> =
        points.iter().flat_map(|&(x, y, _)| [x * image.scale.0, y * image.scale.1]).collect();
    let labels: Vec<i64> = points.iter().map(|&(.., on)| i64::from(on)).collect();
    let mut values: Vec<(String, ort::value::DynValue)> = Vec::new();
    for name in &inputs {
        let value: ort::value::DynValue = match name.as_str() {
            "input_points" => Tensor::from_array(([1usize, 1, n, 2], coords.clone()))?.into_dyn(),
            "input_labels" => Tensor::from_array(([1usize, 1, n], labels.clone()))?.into_dyn(),
            // SAM 2 also takes boxes: none.
            "input_boxes" => Tensor::from_array(([1usize, 0, 4], Vec::<f32>::new()))?.into_dyn(),
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
        let result = match model {
            AiModel::Subject => subject_rmbg(session, img),
            AiModel::Sky => sky(session, img),
        };
        match result {
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
