//! Remove: fill a hole in a photo from its surroundings. Content-aware models (LaMa,
//! MI-GAN) fill in one pass; generative Stable Diffusion 1.5 inpainting (see `sd`) makes
//! new content, a variation per seed.
//!
//! All take a 512 × 512 8-bit RGB crop and a mask of the same size (255: fill, 0: keep)
//! and return the crop filled (the kept pixels unchanged).

use anyhow::{Result, bail};
use image::{GrayImage, Rgb, RgbImage};
use ort::value::Tensor;

use crate::Model;

/// The models' input size.
pub const SIZE: u32 = 512;

fn check(img: &RgbImage, mask: &GrayImage) -> Result<()> {
    if img.dimensions() != (SIZE, SIZE) || mask.dimensions() != (SIZE, SIZE) {
        bail!("inpainting needs {SIZE} × {SIZE} images");
    }
    Ok(())
}

/// The model's output where the mask is, the input elsewhere (models soften what they
/// keep).
pub(crate) fn composite(img: &RgbImage, mask: &GrayImage, out: impl Fn(u32, u32) -> [f32; 3]) -> RgbImage {
    RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let a = f32::from(mask.get_pixel(x, y)[0]) / 255.0;
        let p = img.get_pixel(x, y);
        let o = out(x, y);
        Rgb(std::array::from_fn(|c| {
            let v = f32::from(p[c]) + a * (o[c] - f32::from(p[c]));
            v.round().clamp(0.0, 255.0) as u8
        }))
    })
}

/// LaMa (big-lama, Carve's ONNX export): image 0..1 and a binary mask (1: fill) in,
/// 0..255 out.
pub fn lama(model: &mut Model, img: &RgbImage, mask: &GrayImage) -> Result<RgbImage> {
    check(img, mask)?;
    let n = (SIZE * SIZE) as usize;
    let mut pixels = vec![0.0f32; 3 * n];
    for (i, p) in img.pixels().enumerate() {
        for c in 0..3 {
            pixels[c * n + i] = f32::from(p[c]) / 255.0;
        }
    }
    let holes: Vec<f32> = mask.pixels().map(|m| if m[0] > 127 { 1.0 } else { 0.0 }).collect();
    let s = SIZE as usize;
    let image = Tensor::from_array(([1usize, 3, s, s], pixels))?;
    let mask_t = Tensor::from_array(([1usize, 1, s, s], holes))?;
    let outputs = model.session.run(ort::inputs!["image" => image, "mask" => mask_t])?;
    let (_, out) = outputs[0].try_extract_tensor::<f32>()?;
    // Some exports give 0..1, Carve's 0..255.
    let k = if out.iter().fold(0.0f32, |m, v| m.max(*v)) > 2.0 { 1.0 } else { 255.0 };
    Ok(composite(img, mask, |x, y| {
        let i = (y * SIZE + x) as usize;
        [out[i] * k, out[n + i] * k, out[2 * n + i] * k]
    }))
}

/// MI-GAN's generator: [mask − 0.5, image × mask] in (mask 1: keep; image −1..1), −1..1
/// out.
pub fn migan(model: &mut Model, img: &RgbImage, mask: &GrayImage) -> Result<RgbImage> {
    check(img, mask)?;
    let n = (SIZE * SIZE) as usize;
    let mut input = vec![0.0f32; 4 * n];
    for (i, (p, m)) in img.pixels().zip(mask.pixels()).enumerate() {
        let keep = if m[0] > 127 { 0.0 } else { 1.0 };
        input[i] = keep - 0.5;
        for c in 0..3 {
            input[(c + 1) * n + i] = (f32::from(p[c]) / 127.5 - 1.0) * keep;
        }
    }
    let s = SIZE as usize;
    let x = Tensor::from_array(([1usize, 4, s, s], input))?;
    let outputs = model.session.run(ort::inputs![x])?;
    let (_, out) = outputs[0].try_extract_tensor::<f32>()?;
    Ok(composite(img, mask, |x, y| {
        let i = (y * SIZE + x) as usize;
        std::array::from_fn(|c| (out[c * n + i] + 1.0) * 127.5)
    }))
}
