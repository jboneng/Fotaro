//! The pixels Develop works from: linear, full precision, oriented.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use image::{DynamicImage, ImageBuffer, Rgb, Rgb32FImage};
use lcms2::{ColorSpaceSignature, PixelFormat, Profile};
use rayon::prelude::*;

use super::heal::HealCache;
use super::masks::MaskCache;
use super::matrix::{self, Mat3, Mat3f};
use super::settings::{DevelopSettings, WbPreset};
use super::transform::{Mapping, TransformCache};
use super::wb;
use crate::decode::{SourceKind, fit_image, open_image, orient_raw};
use crate::export::space;
use crate::metadata::{self, Metadata};

/// Colour handling of a RAW file with a known camera matrix.
#[derive(Debug, Clone)]
pub struct CameraColor {
    pub as_shot: Option<[f32; 3]>,
    pub daylight: [f32; 3],
    /// White-balanced camera RGB → linear working space.
    pub cam_to_work: Mat3f,
    /// XYZ → camera RGB.
    pub cam_xyz: Mat3,
}

#[derive(Debug, Clone)]
pub enum SourceColor {
    /// Camera RGB of a RAW with a colour matrix: white balance in Kelvin.
    Camera(CameraColor),
    /// Already in the working space, scene-linear (RAW files without a matrix, such as
    /// scanner DNGs): relative white balance, RAW tone rendering.
    LinearRaw,
    /// A rendered image (JPEG, TIFF…) linearized into the working space: relative white
    /// balance, no base tone curve.
    Rendered,
}

/// Pixels plus what's needed to render them.
#[derive(Debug, Clone)]
pub struct WorkingImage {
    /// Camera RGB for [`SourceColor::Camera`], otherwise the linear working space.
    pub pixels: Rgb32FImage,
    pub color: SourceColor,
    /// The file's full (oriented) size, whatever size `pixels` is.
    pub full_size: (u32, u32),
    /// After a Transform: how much of each pixel the transformed photo covers (the rest
    /// renders white).
    pub(super) coverage: Option<Arc<Vec<f32>>>,
    /// The transformed copy last rendered (see [`super::transform`]).
    pub(super) transform_cache: TransformCache,
    /// After a Transform: how this image's pixels map to the source's (masks are drawn
    /// on the source).
    pub(super) warp_source: Option<Mapping>,
    /// Brush masks rasterized for this image (see [`super::masks`]).
    pub(super) mask_cache: MaskCache,
    /// The photo with the Remove tool's spots healed (see [`super::heal`]).
    pub(super) heal_cache: HealCache,
    /// Which pixels these are among the copies sharing a mask cache: 0 for the photo as
    /// loaded, another number for each healed copy (masks made from the pixels are kept
    /// per copy).
    pub(super) pixels_id: u64,
    /// Render stages kept for the next render (see [`super::cache`]).
    pub(super) render_cache: super::cache::RenderCache,
}

/// The resolved white balance: multipliers to apply and the Temp/Tint the panel shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedWb {
    pub multipliers: [f32; 3],
    pub temp: f32,
    pub tint: f32,
}

impl WorkingImage {
    fn new(pixels: Rgb32FImage, color: SourceColor, full_size: (u32, u32)) -> WorkingImage {
        WorkingImage {
            pixels,
            color,
            full_size,
            coverage: None,
            transform_cache: TransformCache::default(),
            warp_source: None,
            mask_cache: MaskCache::default(),
            heal_cache: HealCache::default(),
            render_cache: Default::default(),
            pixels_id: 0,
        }
    }

    /// Linear working-space pixels as a rendered photo of their own size.
    #[cfg(test)]
    pub(super) fn for_tests(pixels: Rgb32FImage) -> WorkingImage {
        let size = (pixels.width(), pixels.height());
        WorkingImage::new(pixels, SourceColor::Rendered, size)
    }

    /// Load `path` for developing. `max_px` limits the long side (a proxy for interactive
    /// editing); `None` loads full size.
    pub fn load(path: &Path, kind: SourceKind, max_px: Option<u32>) -> Result<(Self, Metadata)> {
        let r = match kind {
            SourceKind::Raw => load_raw(path, max_px),
            SourceKind::Image => load_image(path, max_px),
        };
        let (img, meta) = r.with_context(|| format!("loading {} for Develop", path.display()))?;
        Ok((img, metadata::with_file_time_fallback(meta, path)))
    }

    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    /// Whether Temp/Tint are in Kelvin (else relative −100..+100).
    pub fn kelvin(&self) -> bool {
        matches!(self.color, SourceColor::Camera(_))
    }

    /// Whether the RAW base rendering (tone curve, baseline exposure) applies.
    pub fn scene_referred(&self) -> bool {
        !matches!(self.color, SourceColor::Rendered)
    }

    /// The presets the White Balance menu offers for this photo.
    pub fn wb_presets(&self) -> &'static [WbPreset] {
        if self.kelvin() { &WbPreset::RAW } else { &WbPreset::RELATIVE }
    }

    /// These pixels as an image like this one (the Remove tool's healed copy). It shares
    /// this image's brush rasters, which depend on the strokes and the size only.
    pub(super) fn with_pixels(&self, pixels: Vec<f32>) -> WorkingImage {
        static HEALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        WorkingImage {
            pixels: image::ImageBuffer::from_raw(self.width(), self.height(), pixels).expect("buffer size"),
            color: self.color.clone(),
            full_size: self.full_size,
            coverage: self.coverage.clone(),
            transform_cache: TransformCache::default(),
            warp_source: self.warp_source,
            mask_cache: self.mask_cache.shared(),
            heal_cache: HealCache::default(),
            render_cache: Default::default(),
            pixels_id: HEALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// A smaller copy (longest side at most `max_px`).
    pub fn downscaled(&self, max_px: u32) -> WorkingImage {
        let pixels = fit_image(DynamicImage::ImageRgb32F(self.pixels.clone()), max_px).into_rgb32f();
        WorkingImage {
            pixels,
            color: self.color.clone(),
            full_size: self.full_size,
            coverage: None,
            transform_cache: TransformCache::default(),
            warp_source: None,
            mask_cache: MaskCache::default(),
            heal_cache: HealCache::default(),
            render_cache: Default::default(),
            pixels_id: 0,
        }
    }

    /// The white balance `s` asks for, as multipliers (camera RGB for RAW files, the
    /// working space otherwise) and the Temp/Tint to show.
    pub fn resolve_wb(&self, s: &DevelopSettings) -> ResolvedWb {
        match &self.color {
            SourceColor::Camera(c) => {
                let (temp, tint) = match s.wb {
                    WbPreset::AsShot => {
                        let m = c.as_shot.unwrap_or(c.daylight);
                        return self.camera_wb_from_multipliers(c, m);
                    }
                    WbPreset::Auto => return self.camera_wb_from_multipliers(c, self.auto_multipliers()),
                    WbPreset::Custom => (s.temp, s.tint),
                    p => p.kelvin().unwrap_or((5500.0, 0.0)),
                };
                let m =
                    wb::xy_to_multipliers(&c.cam_xyz, wb::temp_tint_to_xy(f64::from(temp), f64::from(tint)));
                ResolvedWb { multipliers: m, temp, tint }
            }
            SourceColor::LinearRaw | SourceColor::Rendered => {
                let (temp, tint) = match s.wb {
                    WbPreset::Custom => (s.temp, s.tint),
                    WbPreset::Auto => wb::relative_from_multipliers(self.auto_multipliers()),
                    _ => (0.0, 0.0),
                };
                ResolvedWb { multipliers: wb::relative_multipliers(temp, tint), temp, tint }
            }
        }
    }

    fn camera_wb_from_multipliers(&self, c: &CameraColor, m: [f32; 3]) -> ResolvedWb {
        let (temp, tint) =
            wb::multipliers_to_xy(&c.cam_xyz, m).map(wb::xy_to_temp_tint).unwrap_or((5500.0, 0.0));
        ResolvedWb { multipliers: m, temp: temp as f32, tint: tint as f32 }
    }

    /// Gray-world white balance over well-exposed pixels: multipliers that make their
    /// average neutral (G = 1).
    pub fn auto_multipliers(&self) -> [f32; 3] {
        let sum = self
            .pixels
            .as_raw()
            .par_chunks(3 * 4096)
            .map(|chunk| {
                let mut acc = [0f64; 4];
                for p in chunk.as_chunks::<3>().0 {
                    let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
                    if lo > 0.002 && hi < 0.95 {
                        for c in 0..3 {
                            acc[c] += f64::from(p[c]);
                        }
                        acc[3] += 1.0;
                    }
                }
                acc
            })
            .reduce(|| [0f64; 4], |a, b| std::array::from_fn(|i| a[i] + b[i]));
        if sum[3] < 1.0 || sum[0] <= 0.0 || sum[2] <= 0.0 {
            return [1.0; 3];
        }
        let m = [sum[1] / sum[0], 1.0, sum[1] / sum[2]];
        // The average is in the working space for non-camera images; relative WB there
        // is applied as gains, so the same multipliers work.
        m.map(|v| (v as f32).clamp(0.1, 10.0))
    }

    /// White balance that makes the pixel at (x, y) (0..1 of the image) neutral, averaged
    /// over a small area: the eyedropper. `None` over clipped or black pixels.
    pub fn picked_multipliers(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        let (w, h) = (self.width() as i64, self.height() as i64);
        let (cx, cy) = ((x * w as f32) as i64, (y * h as f32) as i64);
        let r = (w.max(h) / 400).max(1);
        let mut acc = [0f64; 3];
        let mut n = 0.0f64;
        for yy in (cy - r).max(0)..=(cy + r).min(h - 1) {
            for xx in (cx - r).max(0)..=(cx + r).min(w - 1) {
                let p = self.pixels.get_pixel(xx as u32, yy as u32).0;
                for c in 0..3 {
                    acc[c] += f64::from(p[c]);
                }
                n += 1.0;
            }
        }
        let avg = acc.map(|v| v / n.max(1.0));
        let (lo, hi) = (avg[0].min(avg[1]).min(avg[2]), avg[0].max(avg[1]).max(avg[2]));
        (n > 0.0 && lo > 1e-4 && hi < 0.98).then(|| [(avg[1] / avg[0]) as f32, 1.0, (avg[1] / avg[2]) as f32])
    }

    /// Settings with the white balance set from multipliers (Auto, eyedropper): a Custom
    /// Temp/Tint.
    pub fn settings_with_multipliers(&self, s: &DevelopSettings, m: [f32; 3]) -> DevelopSettings {
        let (temp, tint) = match &self.color {
            SourceColor::Camera(c) => {
                let r = self.camera_wb_from_multipliers(c, m);
                (r.temp, r.tint)
            }
            _ => wb::relative_from_multipliers(m),
        };
        DevelopSettings { wb: WbPreset::Custom, temp: temp.round(), tint: tint.round(), ..s.clone() }
    }
}

fn load_raw(path: &Path, max_px: Option<u32>) -> Result<(WorkingImage, Metadata)> {
    let mut raw = libraw::RawFile::open(path)?;
    let info = raw.info();
    let meta = metadata::from_raw_info(&info);
    let color = raw.color();
    let long = info.width.max(info.height);
    let half = max_px.is_some_and(|m| m <= long / 2);
    let cam = raw.render_camera_linear(half)?;
    let flat: Vec<f32> = cam.data.into_iter().flatten().collect();
    let pixels: Rgb32FImage = ImageBuffer::from_raw(cam.width, cam.height, flat)
        .context("camera render has an inconsistent size")?;
    let mut img = DynamicImage::ImageRgb32F(pixels);
    if let Some(m) = max_px {
        img = fit_image(img, m);
    }
    let img = orient_raw(img, info.orientation).into_rgb32f();
    let full_size = match info.orientation {
        libraw::Orientation::Rotate90Cw | libraw::Orientation::Rotate90Ccw => (info.height, info.width),
        _ => (info.width, info.height),
    };

    let rgb_cam: Mat3 = std::array::from_fn(|i| std::array::from_fn(|j| f64::from(color.rgb_cam[i][j])));
    let cam_to_work = matrix::mul(&matrix::srgb_to_work(), &rgb_cam);
    let source = match color.cam_xyz {
        Some(cx) if color.colors == 3 => SourceColor::Camera(CameraColor {
            as_shot: color.as_shot,
            daylight: color.daylight,
            cam_to_work: matrix::to_f32(&cam_to_work),
            cam_xyz: std::array::from_fn(|i| std::array::from_fn(|j| f64::from(cx[i][j]))),
        }),
        _ => {
            // No matrix: apply LibRaw's (usually identity) conversion and the as-shot
            // balance now, then treat the result as linear working-space data.
            let mut img = img;
            let m = color.as_shot.unwrap_or([1.0; 3]);
            let mf = matrix::to_f32(&cam_to_work);
            img.as_mut().par_chunks_mut(3).for_each(|p| {
                let q = [(p[0] * m[0]).min(1.0), (p[1] * m[1]).min(1.0), (p[2] * m[2]).min(1.0)];
                p.copy_from_slice(&matrix::apply(&mf, q));
            });
            return Ok((WorkingImage::new(img, SourceColor::LinearRaw, full_size), meta));
        }
    };
    Ok((WorkingImage::new(img, source, full_size), meta))
}

/// sRGB-encoded 0..1 → linear.
fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn load_image(path: &Path, max_px: Option<u32>) -> Result<(WorkingImage, Metadata)> {
    let (img, icc) = open_image(path)?;
    let full_size = (img.width(), img.height());
    let img = match max_px {
        Some(m) => fit_image(img, m),
        None => img,
    };
    let mut meta = metadata::read_exif(path).unwrap_or_default();
    (meta.width, meta.height) = (Some(full_size.0), Some(full_size.1));

    let profile = icc.as_deref().and_then(|b| Profile::new_icc(b).ok()).filter(|p| {
        p.color_space() == ColorSpaceSignature::RgbData
            && !crate::color::is_srgb(p)
            && img.color().has_color()
    });
    let pixels = match profile {
        Some(src) => {
            // Through lcms at 16 bits into linear ProPhoto (the working space).
            let mut rgb16 = img.into_rgb16();
            let dst = space::linear_prophoto();
            space::transform_parallel(
                &src,
                &dst,
                PixelFormat::RGB_16,
                bytemuck::cast_slice_mut(rgb16.as_mut()),
            );
            let (w, h) = (rgb16.width(), rgb16.height());
            let data: Vec<f32> = rgb16.into_raw().into_par_iter().map(|v| f32::from(v) / 65535.0).collect();
            ImageBuffer::<Rgb<f32>, _>::from_raw(w, h, data).context("inconsistent image size")?
        }
        None => {
            let mut f = img.into_rgb32f();
            let m = matrix::to_f32(&matrix::srgb_to_work());
            f.as_mut().par_chunks_mut(3).for_each(|p| {
                let lin = [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2])];
                p.copy_from_slice(&matrix::apply(&m, lin));
            });
            f
        }
    };
    Ok((WorkingImage::new(pixels, SourceColor::Rendered, full_size), meta))
}
