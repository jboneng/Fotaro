//! Generative Remove: Stable Diffusion 1.5 inpainting (ONNX, fp16) with a DDIM sampler.
//!
//! The photo crop (512 × 512) is encoded to latents (64 × 64 × 4); the UNet sees noisy
//! latents, the mask and the masked photo's latents (9 channels) and predicts the noise,
//! with classifier-free guidance between fixed prompt embeddings ("background" against
//! "an object"); the VAE decodes the result. Each seed gives a different fill.

use anyhow::{Context, Result, bail};
use half::f16;
use image::{GrayImage, RgbImage};
use ort::value::{DynValue, Tensor};

use crate::Model;
use crate::inpaint::SIZE;

/// Latents per side, and the VAE's latent scale.
const LAT: usize = (SIZE / 8) as usize;
const VAE_SCALE: f32 = 0.18215;

/// The three models.
pub struct Sd {
    pub unet: Model,
    pub vae_encoder: Model,
    pub vae_decoder: Model,
}

/// Prompt embeddings (77 × 768 each): what to make, and what not to.
pub struct Prompts {
    pub positive: Vec<f32>,
    pub negative: Vec<f32>,
}

/// How to sample.
#[derive(Debug, Clone, Copy)]
pub struct Sampling {
    pub steps: usize,
    pub guidance: f32,
    pub seed: u64,
}

/// A float tensor in the model input's own type (fp16 exports want f16).
fn tensor(model: &Model, name: &str, shape: Vec<usize>, data: Vec<f32>) -> Result<DynValue> {
    let fp16 = model
        .session
        .inputs()
        .iter()
        .find(|i| i.name() == name)
        .is_some_and(|i| format!("{:?}", i.dtype()).contains("Float16"));
    Ok(if fp16 {
        Tensor::from_array((shape, data.into_iter().map(f16::from_f32).collect::<Vec<_>>()))?.into_dyn()
    } else {
        Tensor::from_array((shape, data))?.into_dyn()
    })
}

/// The first output as f32 (whatever the model's float type).
fn output(outputs: &ort::session::SessionOutputs<'_>, i: usize) -> Result<(Vec<usize>, Vec<f32>)> {
    let v = &outputs[i];
    if let Ok((shape, data)) = v.try_extract_tensor::<f32>() {
        return Ok((shape.iter().map(|d| *d as usize).collect(), data.to_vec()));
    }
    let (shape, data) = v.try_extract_tensor::<f16>().context("model output type")?;
    Ok((shape.iter().map(|d| *d as usize).collect(), data.iter().map(|v| v.to_f32()).collect()))
}

/// A small, seedable normal-distribution source (SplitMix64 and Box–Muller).
struct Noise(u64);

impl Noise {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn normal(&mut self) -> f32 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
}

/// SD 1.5's noise schedule (scaled linear betas, 1000 steps): ᾱ per timestep.
fn alphas_cumprod() -> Vec<f32> {
    let (b0, b1) = (0.00085f64.sqrt(), 0.012f64.sqrt());
    let mut acc = 1.0f64;
    (0..1000)
        .map(|i| {
            let b = b0 + (b1 - b0) * i as f64 / 999.0;
            acc *= 1.0 - b * b;
            acc as f32
        })
        .collect()
}

/// The crop as −1..1 NCHW, with the mask's area zeroed when `masked`.
fn image_input(img: &RgbImage, mask: &GrayImage, masked: bool) -> Vec<f32> {
    let n = (SIZE * SIZE) as usize;
    let mut out = vec![0.0f32; 3 * n];
    for (i, (p, m)) in img.pixels().zip(mask.pixels()).enumerate() {
        let keep = !masked || m[0] <= 127;
        for c in 0..3 {
            out[c * n + i] = if keep { f32::from(p[c]) / 127.5 - 1.0 } else { 0.0 };
        }
    }
    out
}

impl Sd {
    /// Latents of a −1..1 image (the distribution's mean if the encoder gives both).
    fn encode(&mut self, pixels: Vec<f32>) -> Result<Vec<f32>> {
        let s = SIZE as usize;
        let name = self.vae_encoder.io().0[0].clone();
        let x = tensor(&self.vae_encoder, &name, vec![1, 3, s, s], pixels)?;
        let outputs = self.vae_encoder.session.run(vec![(name, x)])?;
        let (shape, data) = output(&outputs, 0)?;
        let n = LAT * LAT;
        if shape.len() != 4 || shape[2] != LAT || shape[3] != LAT || (shape[1] != 4 && shape[1] != 8) {
            bail!("unexpected VAE encoder output {shape:?}");
        }
        Ok(data[..4 * n].iter().map(|v| v * VAE_SCALE).collect())
    }

    fn decode(&mut self, latents: &[f32]) -> Result<Vec<f32>> {
        let name = self.vae_decoder.io().0[0].clone();
        let x = tensor(
            &self.vae_decoder,
            &name,
            vec![1, 4, LAT, LAT],
            latents.iter().map(|v| v / VAE_SCALE).collect(),
        )?;
        let outputs = self.vae_decoder.session.run(vec![(name, x)])?;
        Ok(output(&outputs, 0)?.1)
    }

    /// Fill the mask of `img` (both 512 × 512); `progress(step, steps)` after each step.
    pub fn inpaint(
        &mut self,
        img: &RgbImage,
        mask: &GrayImage,
        prompts: &Prompts,
        sampling: Sampling,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<RgbImage> {
        if img.dimensions() != (SIZE, SIZE) || mask.dimensions() != (SIZE, SIZE) {
            bail!("inpainting needs {SIZE} × {SIZE} images");
        }
        let n = LAT * LAT;
        let masked_latents = self.encode(image_input(img, mask, true))?;
        // The mask at latent size: 1 where to fill (any of the 8 × 8 pixels).
        let mask_lat: Vec<f32> = (0..n)
            .map(|i| {
                let (lx, ly) = ((i % LAT) as u32 * 8, (i / LAT) as u32 * 8);
                let any = (0..8).any(|dy| (0..8).any(|dx| mask.get_pixel(lx + dx, ly + dy)[0] > 127));
                if any { 1.0 } else { 0.0 }
            })
            .collect();
        let ac = alphas_cumprod();
        let steps = sampling.steps.clamp(1, 1000);
        let ratio = 1000 / steps;
        let timesteps: Vec<usize> = (0..steps).rev().map(|i| i * ratio + 1).collect();
        let mut noise = Noise(sampling.seed);
        let mut latents: Vec<f32> = (0..4 * n).map(|_| noise.normal()).collect();
        let cfg = sampling.guidance > 1.0;
        let batch = if cfg { 2 } else { 1 };
        let hidden: Vec<f32> = if cfg {
            prompts.negative.iter().chain(&prompts.positive).copied().collect()
        } else {
            prompts.positive.clone()
        };
        let names = self.unet.io().0;
        for (k, &t) in timesteps.iter().enumerate() {
            // UNet input: [latents, mask, masked latents] per batch entry.
            let mut sample = Vec::with_capacity(batch * 9 * n);
            for _ in 0..batch {
                sample.extend_from_slice(&latents);
                sample.extend_from_slice(&mask_lat);
                sample.extend_from_slice(&masked_latents);
            }
            let mut inputs: Vec<(String, DynValue)> = Vec::new();
            for name in &names {
                let v = match name.as_str() {
                    "sample" => tensor(&self.unet, name, vec![batch, 9, LAT, LAT], sample.clone())?,
                    "encoder_hidden_states" => {
                        tensor(&self.unet, name, vec![batch, 77, 768], hidden.clone())?
                    }
                    "timestep" => {
                        let dims = self
                            .unet
                            .session
                            .inputs()
                            .iter()
                            .find(|i| i.name() == "timestep")
                            .map(|i| format!("{:?}", i.dtype()))
                            .unwrap_or_default();
                        if dims.contains("Int64") {
                            Tensor::from_array(([1usize], vec![t as i64]))?.into_dyn()
                        } else {
                            tensor(&self.unet, name, vec![1], vec![t as f32])?
                        }
                    }
                    other => bail!("the UNet wants {other}"),
                };
                inputs.push((name.clone(), v));
            }
            let outputs = self.unet.session.run(inputs)?;
            let (_, eps_all) = output(&outputs, 0)?;
            let eps: Vec<f32> = if cfg {
                let (uncond, cond) = eps_all.split_at(4 * n);
                uncond.iter().zip(cond).map(|(u, c)| u + sampling.guidance * (c - u)).collect()
            } else {
                eps_all[..4 * n].to_vec()
            };
            // DDIM (η = 0).
            let a_t = ac[t];
            let a_prev = timesteps.get(k + 1).map_or(1.0, |&p| ac[p]);
            for (x, e) in latents.iter_mut().zip(&eps) {
                let x0 = (*x - (1.0 - a_t).sqrt() * e) / a_t.sqrt();
                *x = a_prev.sqrt() * x0 + (1.0 - a_prev).sqrt() * e;
            }
            progress(k + 1, steps);
        }
        let out = self.decode(&latents)?;
        let n_px = (SIZE * SIZE) as usize;
        Ok(crate::inpaint::composite(img, mask, |x, y| {
            let i = (y * SIZE + x) as usize;
            std::array::from_fn(|c| (out[c * n_px + i] + 1.0) * 127.5)
        }))
    }
}

/// CLIP token ids for simple prompts of whole words (each must be one token in `vocab`,
/// the tokenizer's `vocab.json`): start, words, end, padded with end tokens to 77.
pub fn tokens(prompt: &str, vocab: &serde_json_lite::Vocab) -> Result<Vec<i64>> {
    const START: i64 = 49406;
    const END: i64 = 49407;
    let mut ids = vec![START];
    for word in prompt.to_lowercase().split(|c: char| c.is_whitespace()).filter(|w| !w.is_empty()) {
        let (word, comma) = match word.strip_suffix(',') {
            Some(w) => (w, true),
            None => (word, false),
        };
        let id = vocab.get(&format!("{word}</w>")).with_context(|| format!("'{word}' isn't one token"))?;
        ids.push(id);
        if comma {
            ids.push(vocab.get(",</w>").context("no comma token")?);
        }
    }
    if ids.len() > 76 {
        bail!("prompt too long");
    }
    ids.resize(77, END);
    Ok(ids)
}

/// A minimal reader for the tokenizer's `vocab.json` (a flat object of strings to ids).
pub mod serde_json_lite {
    use std::collections::HashMap;

    pub struct Vocab(HashMap<String, i64>);

    impl Vocab {
        pub fn parse(text: &str) -> Vocab {
            let mut map = HashMap::new();
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '"' {
                    continue;
                }
                let mut key = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(e) = chars.next() {
                                key.push(match e {
                                    'n' => '\n',
                                    't' => '\t',
                                    other => other,
                                });
                            }
                        }
                        '"' => break,
                        other => key.push(other),
                    }
                }
                // After the key: ':' then a number.
                while chars.peek().is_some_and(|c| *c == ':' || c.is_whitespace()) {
                    chars.next();
                }
                let mut num = String::new();
                while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    num.push(chars.next().unwrap_or('0'));
                }
                if let Ok(v) = num.parse() {
                    map.insert(key, v);
                }
            }
            Vocab(map)
        }

        pub fn get(&self, key: &str) -> Option<i64> {
            self.0.get(key).copied()
        }
    }
}

/// Prompt embeddings from the text encoder (the benchmark makes them once; the app ships
/// them).
pub fn embed(text_encoder: &mut Model, ids: &[i64]) -> Result<Vec<f32>> {
    let name = text_encoder.io().0[0].clone();
    let wants_i32 =
        text_encoder.session.inputs().iter().any(|i| format!("{:?}", i.dtype()).contains("Int32"));
    let x: DynValue = if wants_i32 {
        Tensor::from_array(([1usize, ids.len()], ids.iter().map(|v| *v as i32).collect::<Vec<_>>()))?
            .into_dyn()
    } else {
        Tensor::from_array(([1usize, ids.len()], ids.to_vec()))?.into_dyn()
    };
    let outputs = text_encoder.session.run(vec![(name, x)])?;
    let (shape, data) = output(&outputs, 0)?;
    if shape != [1, 77, 768] {
        bail!("unexpected text encoder output {shape:?}");
    }
    Ok(data)
}
