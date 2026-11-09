//! Output sharpening and watermarks, applied to the final-size pixels (8 or 16 bit).

use std::path::PathBuf;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use anyhow::{Context, Result};
use image::DynamicImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::space::{self, ColorSpace};

/// One colour channel value, 8 or 16 bit.
trait Sample: Copy + Send + Sync {
    const MAX: f32;
    fn to_f32(self) -> f32;
    fn from_f32(v: f32) -> Self;
}

impl Sample for u8 {
    const MAX: f32 = 255.0;
    fn to_f32(self) -> f32 {
        f32::from(self)
    }
    fn from_f32(v: f32) -> Self {
        v.round().clamp(0.0, 255.0) as u8
    }
}

impl Sample for u16 {
    const MAX: f32 = 65535.0;
    fn to_f32(self) -> f32 {
        f32::from(self)
    }
    fn from_f32(v: f32) -> Self {
        v.round().clamp(0.0, 65535.0) as u16
    }
}

// ---------------------------------------------------------------------------------
// Output sharpening

/// LrC's Output Sharpening ▸ Sharpen For.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SharpenFor {
    #[default]
    Screen,
    MattePaper,
    GlossyPaper,
}

impl SharpenFor {
    pub const ALL: [SharpenFor; 3] = [Self::Screen, Self::MattePaper, Self::GlossyPaper];

    pub fn label(self) -> &'static str {
        match self {
            Self::Screen => "Screen",
            Self::MattePaper => "Matte Paper",
            Self::GlossyPaper => "Glossy Paper",
        }
    }
}

/// LrC's Output Sharpening ▸ Amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SharpenAmount {
    Low,
    #[default]
    Standard,
    High,
}

impl SharpenAmount {
    pub const ALL: [SharpenAmount; 3] = [Self::Low, Self::Standard, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Standard => "Standard",
            Self::High => "High",
        }
    }
}

/// Unsharp mask radius (Gaussian sigma, pixels) and strength for a medium.
/// Screens get a fine radius; prints a wider one that grows with the print resolution
/// (more pixels per inch, so detail that survives the paper is more pixels wide), and
/// matte paper, which softens more than glossy, gets more of it.
pub(crate) fn sharpen_params(medium: SharpenFor, amount: SharpenAmount, ppi: u32) -> (f32, f32) {
    let level = match amount {
        SharpenAmount::Low => 0,
        SharpenAmount::Standard => 1,
        SharpenAmount::High => 2,
    };
    let print_scale = (ppi.max(72) as f32 / 300.0).clamp(0.5, 2.5);
    match medium {
        SharpenFor::Screen => (0.5, [0.4, 0.7, 1.0][level]),
        SharpenFor::MattePaper => ((1.0 * print_scale).max(0.6), [0.7, 1.0, 1.4][level]),
        SharpenFor::GlossyPaper => ((0.8 * print_scale).max(0.5), [0.5, 0.8, 1.1][level]),
    }
}

fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil().max(1.0) as i32;
    let k: Vec<f32> = (-radius..=radius).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.into_iter().map(|v| v / sum).collect()
}

/// Unsharp mask on luminance (no colour fringes), applied equally to R, G and B.
pub(crate) fn sharpen(img: &mut DynamicImage, sigma: f32, amount: f32) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    match img {
        DynamicImage::ImageRgb8(b) => sharpen_rgb(b.as_mut(), w, h, sigma, amount),
        DynamicImage::ImageRgb16(b) => sharpen_rgb(b.as_mut(), w, h, sigma, amount),
        _ => log::warn!("sharpening skipped: unexpected pixel format"),
    }
}

fn sharpen_rgb<T: Sample>(px: &mut [T], w: usize, h: usize, sigma: f32, amount: f32) {
    if w == 0 || h == 0 {
        return;
    }
    // Luminance and the blur passes in 16 bits: 2 bytes per pixel per plane, so a 45 MP
    // export needs ~270 MB here rather than ~540 MB in f32.
    let to16 = 65535.0 / T::MAX;
    let luma: Vec<u16> = px
        .par_chunks_exact(3)
        .map(|p| {
            let l = 0.2126 * p[0].to_f32() + 0.7152 * p[1].to_f32() + 0.0722 * p[2].to_f32();
            (l * to16).round().clamp(0.0, 65535.0) as u16
        })
        .collect();
    let k = gaussian_kernel(sigma);
    let r = (k.len() / 2) as isize;

    let mut horiz = vec![0u16; w * h];
    horiz.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let row = &luma[y * w..][..w];
        for (x, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let sx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                acc += kv * f32::from(row[sx]);
            }
            *o = acc.round() as u16;
        }
    });

    let threshold = 0.004 * 65535.0;
    let from16 = T::MAX / 65535.0;
    px.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        // Vertical blur of this row, then the unsharp mask.
        let mut blur = vec![0.0f32; w];
        for (i, kv) in k.iter().enumerate() {
            let sy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
            for (b, v) in blur.iter_mut().zip(&horiz[sy * w..][..w]) {
                *b += kv * f32::from(*v);
            }
        }
        for (x, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let d = f32::from(luma[y * w + x]) - blur[x];
            if d.abs() < threshold {
                continue;
            }
            let delta = amount * d * from16;
            for c in p {
                *c = T::from_f32(c.to_f32() + delta);
            }
        }
    });
}

// ---------------------------------------------------------------------------------
// Watermarks

/// What the watermark shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WatermarkStyle {
    /// The photo's Copyright metadata (LrC's "Simple Copyright Watermark").
    #[default]
    Copyright,
    Text,
    /// A PNG (with transparency) or JPEG logo.
    Graphic,
}

impl WatermarkStyle {
    pub const ALL: [WatermarkStyle; 3] = [Self::Copyright, Self::Text, Self::Graphic];

    pub fn label(self) -> &'static str {
        match self {
            Self::Copyright => "Simple Copyright Watermark",
            Self::Text => "Text",
            Self::Graphic => "Graphic",
        }
    }
}

/// Where the watermark sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    #[default]
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// Row by row, as drawn in the dialog's 3 × 3 picker.
    pub const ALL: [Anchor; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top left",
            Self::Top => "Top",
            Self::TopRight => "Top right",
            Self::Left => "Left",
            Self::Center => "Center",
            Self::Right => "Right",
            Self::BottomLeft => "Bottom left",
            Self::Bottom => "Bottom",
            Self::BottomRight => "Bottom right",
        }
    }

    /// (column, row), each 0 = start, 1 = middle, 2 = end.
    fn grid(self) -> (u8, u8) {
        let i = Self::ALL.iter().position(|a| *a == self).unwrap_or(0) as u8;
        (i % 3, i / 3)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub enabled: bool,
    pub style: WatermarkStyle,
    /// Text style: what to write.
    pub text: String,
    /// Graphic style: the image file.
    pub graphic: PathBuf,
    /// 0–100 %.
    pub opacity: u8,
    /// Watermark width as a percentage of the photo's width.
    pub size: u8,
    pub anchor: Anchor,
    /// Distance from the edges, as a percentage of the photo's short side.
    pub inset: u8,
    /// Text: black instead of white.
    pub black: bool,
    /// Text: a soft drop shadow, so it reads on light and dark areas.
    pub shadow: bool,
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark {
            enabled: false,
            style: WatermarkStyle::Copyright,
            text: String::new(),
            graphic: PathBuf::new(),
            opacity: 70,
            size: 25,
            anchor: Anchor::BottomLeft,
            inset: 2,
            black: false,
            shadow: true,
        }
    }
}

/// Straight-alpha RGBA overlay, colours already in the photo's colour space (0–1).
struct Overlay {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Watermark {
    /// The text this watermark writes on a photo with `copyright` metadata.
    fn text_for<'a>(&'a self, copyright: Option<&'a str>) -> Option<&'a str> {
        let t = match self.style {
            WatermarkStyle::Copyright => copyright?,
            WatermarkStyle::Text => &self.text,
            WatermarkStyle::Graphic => return None,
        };
        Some(t.trim()).filter(|t| !t.is_empty())
    }

    /// Draw onto `img` (in `cs`). Copyright watermarks on photos without a copyright,
    /// and empty text, draw nothing.
    pub(crate) fn apply(
        &self,
        img: &mut DynamicImage,
        copyright: Option<&str>,
        cs: ColorSpace,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let (iw, ih) = (img.width() as usize, img.height() as usize);
        let target_w = ((iw as f32 * f32::from(self.size.clamp(1, 100)) / 100.0).round() as usize).max(1);
        let overlay = match self.style {
            WatermarkStyle::Graphic => graphic_overlay(&self.graphic, target_w, cs)?,
            _ => match self.text_for(copyright) {
                Some(text) => text_overlay(text, target_w, self.black, self.shadow),
                None => return Ok(()),
            },
        };
        let Some(overlay) = overlay else { return Ok(()) };
        let inset = (iw.min(ih) as f32 * f32::from(self.inset.min(50)) / 100.0).round() as isize;
        let (col, row) = self.anchor.grid();
        let place = |outer: usize, inner: usize, pos: u8| -> isize {
            match pos {
                0 => inset,
                1 => (outer as isize - inner as isize) / 2,
                _ => outer as isize - inner as isize - inset,
            }
        };
        let (x0, y0) = (place(iw, overlay.w, col), place(ih, overlay.h, row));
        let opacity = f32::from(self.opacity.min(100)) / 100.0;
        match img {
            DynamicImage::ImageRgb8(b) => blend(b.as_mut(), iw, &overlay, x0, y0, opacity),
            DynamicImage::ImageRgb16(b) => blend(b.as_mut(), iw, &overlay, x0, y0, opacity),
            _ => log::warn!("watermark skipped: unexpected pixel format"),
        }
        Ok(())
    }
}

fn blend<T: Sample>(px: &mut [T], w: usize, o: &Overlay, x0: isize, y0: isize, opacity: f32) {
    px.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let oy = y as isize - y0;
        if oy < 0 || oy >= o.h as isize {
            return;
        }
        for ox in 0..o.w {
            let x = x0 + ox as isize;
            if x < 0 || x >= w as isize {
                continue;
            }
            let [r, g, b, a] = o.px[oy as usize * o.w + ox];
            let a = a * opacity;
            if a <= 0.0 {
                continue;
            }
            let p = &mut row[x as usize * 3..][..3];
            for (c, v) in p.iter_mut().zip([r, g, b]) {
                *c = T::from_f32(c.to_f32() * (1.0 - a) + v * T::MAX * a);
            }
        }
    });
}

/// Text rasterised at `width` pixels wide (Ubuntu Light, egui's own UI font).
fn text_overlay(text: &str, width: usize, black: bool, shadow: bool) -> Option<Overlay> {
    let font = FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).ok()?;
    let line = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("  ");
    let measure = |scale: f32| {
        let f = font.as_scaled(PxScale::from(scale));
        let mut x = 0.0;
        let mut prev = None;
        for c in line.chars() {
            let id = f.glyph_id(c);
            if let Some(p) = prev {
                x += f.kern(p, id);
            }
            x += f.h_advance(id);
            prev = Some(id);
        }
        x
    };
    let at_100 = measure(100.0);
    if at_100 <= 0.0 {
        return None;
    }
    let scale = (100.0 * width as f32 / at_100).clamp(4.0, 4000.0);
    let f = font.as_scaled(PxScale::from(scale));
    let shadow_off = if shadow { (scale * 0.04).max(1.0).round() as usize } else { 0 };
    let w = measure(scale).ceil() as usize + 2 + shadow_off;
    let h = (f.ascent() - f.descent()).ceil() as usize + 2 + shadow_off;
    let mut mask = vec![0.0f32; w * h];
    let mut x = 1.0;
    let mut prev = None;
    for c in line.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            x += f.kern(p, id);
        }
        let glyph = id.with_scale_and_position(f.scale(), point(x, 1.0 + f.ascent()));
        x += f.h_advance(id);
        prev = Some(id);
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i64 + i64::from(gx), b.min.y as i64 + i64::from(gy));
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    let m = &mut mask[py as usize * w + px as usize];
                    *m = m.max(cov.clamp(0.0, 1.0));
                }
            });
        }
    }
    let fg = if black { 0.0 } else { 1.0 };
    let sh = 1.0 - fg;
    let px = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let text = mask[i];
            let shade = if shadow_off > 0 && x >= shadow_off && y >= shadow_off {
                mask[(y - shadow_off) * w + (x - shadow_off)] * 0.5
            } else {
                0.0
            };
            // Text over its shadow.
            let a = text + shade * (1.0 - text);
            if a <= 0.0 {
                return [0.0; 4];
            }
            let c = (fg * text + sh * shade * (1.0 - text)) / a;
            [c, c, c, a]
        })
        .collect();
    Some(Overlay { w, h, px })
}

fn graphic_overlay(path: &std::path::Path, width: usize, cs: ColorSpace) -> Result<Option<Overlay>> {
    anyhow::ensure!(!path.as_os_str().is_empty(), "choose a watermark graphic");
    let img = image::open(path).with_context(|| format!("reading watermark {}", path.display()))?;
    let (gw, gh) = (img.width().max(1), img.height().max(1));
    let h = ((width as f64 * f64::from(gh) / f64::from(gw)).round() as u32).max(1);
    let rgba =
        image::imageops::resize(&img.to_rgba8(), width as u32, h, image::imageops::FilterType::Lanczos3);
    let mut rgb: Vec<u8> = rgba.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    space::srgb_to(&mut rgb, cs);
    let px = rgba
        .pixels()
        .zip(rgb.as_chunks::<3>().0)
        .map(|(p, c)| {
            [
                f32::from(c[0]) / 255.0,
                f32::from(c[1]) / 255.0,
                f32::from(c[2]) / 255.0,
                f32::from(p[3]) / 255.0,
            ]
        })
        .collect();
    Ok(Some(Overlay { w: width, h: h as usize, px }))
}
