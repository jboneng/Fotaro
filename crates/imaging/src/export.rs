//! Export (LrC File ▸ Export…): render a photo at full quality, resize it, and write
//! a JPEG, TIFF or PNG with an embedded sRGB profile, EXIF and XMP.
//!
//! The pixels are what the app shows: RAW files use their full-size embedded JPEG
//! (or a LibRaw render), other images are colour-managed to sRGB. The catalog
//! rotation is applied, so exports are always upright (EXIF orientation 1).

use std::io::{Cursor, Write};
use std::path::Path;

use anyhow::{Context, Result};
use exif::experimental::Writer as ExifWriter;
use exif::{Field, In, Rational, Tag, Value};
use fast_image_resize as fir;
use image::{DynamicImage, RgbImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::decode::{SourceKind, decode_full};
use crate::metadata::Metadata;

/// LrC's File Settings ▸ Image Format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FileFormat {
    #[default]
    Jpeg,
    Tiff,
    Png,
    /// The original file, byte for byte (plus its XMP sidecar).
    Original,
}

impl FileFormat {
    pub const ALL: [FileFormat; 4] = [Self::Jpeg, Self::Tiff, Self::Png, Self::Original];

    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
            Self::Original => "Original",
        }
    }

    /// File extension for rendered formats (`None` for Original: it keeps its own).
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Jpeg => Some("jpg"),
            Self::Tiff => Some("tif"),
            Self::Png => Some("png"),
            Self::Original => None,
        }
    }
}

/// LrC's Image Sizing ▸ Resize to Fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ResizeMode {
    /// Fit inside W × H as given.
    WidthHeight,
    /// Fit inside W × H in either orientation (long side ≤ the larger number).
    Dimensions,
    #[default]
    LongEdge,
    ShortEdge,
    Megapixels,
    Percentage,
}

impl ResizeMode {
    pub const ALL: [ResizeMode; 6] = [
        Self::WidthHeight,
        Self::Dimensions,
        Self::LongEdge,
        Self::ShortEdge,
        Self::Megapixels,
        Self::Percentage,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::WidthHeight => "Width & Height",
            Self::Dimensions => "Dimensions",
            Self::LongEdge => "Long Edge",
            Self::ShortEdge => "Short Edge",
            Self::Megapixels => "Megapixels",
            Self::Percentage => "Percentage",
        }
    }
}

/// How to encode and size the exported pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageOptions {
    pub format: FileFormat,
    /// JPEG quality, 0–100.
    pub jpeg_quality: u8,
    /// TIFF: ZIP (Deflate) compression instead of none.
    pub tiff_zip: bool,
    pub resize: bool,
    pub resize_mode: ResizeMode,
    /// Width & Height / Dimensions, in pixels.
    pub width: u32,
    pub height: u32,
    /// Long Edge / Short Edge, in pixels.
    pub edge: u32,
    pub megapixels: f32,
    pub percent: u32,
    pub dont_enlarge: bool,
    /// Resolution written to the file (pixels per inch); it doesn't change the pixels.
    pub ppi: u32,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions {
            format: FileFormat::Jpeg,
            jpeg_quality: 85,
            tiff_zip: true,
            resize: false,
            resize_mode: ResizeMode::LongEdge,
            width: 2048,
            height: 2048,
            edge: 2048,
            megapixels: 12.0,
            percent: 50,
            dont_enlarge: true,
            ppi: 240,
        }
    }
}

impl ImageOptions {
    /// The output size for a `w` × `h` image.
    pub fn target_size(&self, w: u32, h: u32) -> (u32, u32) {
        if !self.resize || w == 0 || h == 0 {
            return (w, h);
        }
        let (wf, hf) = (f64::from(w), f64::from(h));
        let fit = |bw: u32, bh: u32| -> f64 {
            // Zero means "no limit" in that direction, as in LrC.
            let sx = if bw == 0 { f64::INFINITY } else { f64::from(bw) / wf };
            let sy = if bh == 0 { f64::INFINITY } else { f64::from(bh) / hf };
            let s = sx.min(sy);
            if s.is_finite() { s } else { 1.0 }
        };
        let scale = match self.resize_mode {
            ResizeMode::WidthHeight => fit(self.width, self.height),
            ResizeMode::Dimensions => {
                let (big, small) = (self.width.max(self.height), self.width.min(self.height));
                if w >= h { fit(big, small) } else { fit(small, big) }
            }
            ResizeMode::LongEdge => f64::from(self.edge.max(1)) / wf.max(hf),
            ResizeMode::ShortEdge => f64::from(self.edge.max(1)) / wf.min(hf),
            ResizeMode::Megapixels => (f64::from(self.megapixels.max(0.01)) * 1e6 / (wf * hf)).sqrt(),
            ResizeMode::Percentage => f64::from(self.percent.max(1)) / 100.0,
        };
        let scale = if self.dont_enlarge { scale.min(1.0) } else { scale };
        let px = |v: f64| ((v * scale).round() as u32).clamp(1, 65_000);
        (px(wf), px(hf))
    }
}

/// What to embed besides the pixels. Empty fields are left out.
#[derive(Debug, Clone, Default)]
pub struct EmbeddedMetadata {
    /// Camera and exposure EXIF (make, model, lens, exposure, capture time).
    pub camera: Option<Metadata>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub description: Option<String>,
    /// A complete XMP packet (`<x:xmpmeta>…`).
    pub xmp: Option<String>,
}

/// Decode `path` at full size, apply `turns` clockwise quarter turns (the catalog
/// rotation) and resize per `opts`. Returns the pixels and the file's metadata.
pub fn render(path: &Path, kind: SourceKind, turns: u8, opts: &ImageOptions) -> Result<(RgbImage, Metadata)> {
    let (img, meta) = decode_full(path, kind)?;
    let img = match turns % 4 {
        1 => image::imageops::rotate90(&img),
        2 => image::imageops::rotate180(&img),
        3 => image::imageops::rotate270(&img),
        _ => img,
    };
    let (w, h) = opts.target_size(img.width(), img.height());
    Ok((resize(img, w, h)?, meta))
}

fn resize(img: RgbImage, w: u32, h: u32) -> Result<RgbImage> {
    if (w, h) == img.dimensions() {
        return Ok(img);
    }
    let src = DynamicImage::ImageRgb8(img);
    let mut dst = DynamicImage::new_rgb8(w, h);
    // Lanczos3 is a touch crisper than the previews' CatmullRom; exports are final output.
    let opts = fir::ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3));
    fir::Resizer::new().resize(&src, &mut dst, &opts).context("resizing")?;
    Ok(dst.into_rgb8())
}

fn srgb_icc() -> Vec<u8> {
    lcms2::Profile::new_srgb().icc().unwrap_or_default()
}

fn ascii(tag: Tag, s: &str) -> Field {
    Field { tag, ifd_num: In::PRIMARY, value: Value::Ascii(vec![s.as_bytes().to_vec()]) }
}

fn rational(tag: Tag, v: f64, denom: u32) -> Field {
    let r = Rational { num: (v * f64::from(denom)).round().max(0.0) as u32, denom };
    Field { tag, ifd_num: In::PRIMARY, value: Value::Rational(vec![r]) }
}

fn short(tag: Tag, v: u16) -> Field {
    Field { tag, ifd_num: In::PRIMARY, value: Value::Short(vec![v]) }
}

/// EXIF `ExposureTime`: 1/250 rather than 0.004.
fn exposure_time(t: f64) -> Rational {
    if t < 1.0 {
        Rational { num: 1, denom: (1.0 / t).round().max(1.0) as u32 }
    } else {
        Rational { num: (t * 10.0).round() as u32, denom: 10 }
    }
}

/// "YYYY:MM:DD HH:MM:SS" from naive-local seconds (see [`Metadata`]).
fn exif_time(naive_local: i64) -> Option<String> {
    let t = chrono::DateTime::from_timestamp(naive_local, 0)?;
    Some(t.format("%Y:%m:%d %H:%M:%S").to_string())
}

/// The EXIF/TIFF fields to write (everything goes in the primary image's IFDs).
fn exif_fields(meta: &EmbeddedMetadata, ppi: u32) -> Vec<Field> {
    let mut f = vec![
        short(Tag::Orientation, 1),
        rational(Tag::XResolution, f64::from(ppi.max(1)), 1),
        rational(Tag::YResolution, f64::from(ppi.max(1)), 1),
        short(Tag::ResolutionUnit, 2), // inches
        ascii(Tag::Software, concat!("PhotoLibrary ", env!("CARGO_PKG_VERSION"))),
    ];
    let text = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    if let Some(a) = text(&meta.artist) {
        f.push(ascii(Tag::Artist, &a));
    }
    if let Some(c) = text(&meta.copyright) {
        f.push(ascii(Tag::Copyright, &c));
    }
    if let Some(d) = text(&meta.description) {
        f.push(ascii(Tag::ImageDescription, &d));
    }
    if let Some(m) = &meta.camera {
        if let Some(v) = &m.make {
            f.push(ascii(Tag::Make, v));
        }
        if let Some(v) = &m.model {
            f.push(ascii(Tag::Model, v));
        }
        if let Some(v) = &m.lens {
            f.push(ascii(Tag::LensModel, v));
        }
        if let Some(v) = m.iso {
            f.push(short(Tag::PhotographicSensitivity, v.round().clamp(0.0, 65535.0) as u16));
        }
        if let Some(t) = m.shutter {
            f.push(Field {
                tag: Tag::ExposureTime,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![exposure_time(t)]),
            });
        }
        if let Some(a) = m.aperture {
            f.push(rational(Tag::FNumber, a, 10));
        }
        if let Some(l) = m.focal_len {
            f.push(rational(Tag::FocalLength, l, 10));
        }
        if let Some(t) = m.capture_time.and_then(exif_time) {
            f.push(ascii(Tag::DateTimeOriginal, &t));
            f.push(ascii(Tag::DateTime, &t));
        }
    }
    f
}

fn exif_block(fields: &[Field]) -> Result<Vec<u8>> {
    let mut w = ExifWriter::new();
    for field in fields {
        w.push_field(field);
    }
    let mut buf = Cursor::new(Vec::new());
    w.write(&mut buf, false).context("encoding EXIF")?;
    Ok(buf.into_inner())
}

/// JPEG APP1 segments are limited to 64 KB including the header.
const APP1_MAX: usize = 65_533;
const XMP_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn encode_jpeg(img: &RgbImage, opts: &ImageOptions, meta: &EmbeddedMetadata) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    let (w16, h16) = (u16::try_from(w), u16::try_from(h));
    let (Ok(w16), Ok(h16)) = (w16, h16) else { anyhow::bail!("{w}×{h} is too large for JPEG") };
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, opts.jpeg_quality.clamp(1, 100));
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend(exif_block(&exif_fields(meta, opts.ppi))?);
    if exif.len() <= APP1_MAX {
        enc.add_app_segment(1, exif).context("adding EXIF")?;
    }
    if let Some(xmp) = &meta.xmp {
        let mut seg = XMP_NS.to_vec();
        seg.extend_from_slice(xmp.as_bytes());
        if seg.len() <= APP1_MAX {
            enc.add_app_segment(1, seg).context("adding XMP")?;
        } else {
            log::warn!("XMP packet too large for JPEG ({} bytes); left out", seg.len());
        }
    }
    enc.add_icc_profile(&srgb_icc()).context("adding ICC profile")?;
    enc.encode(img.as_raw(), w16, h16, jpeg_encoder::ColorType::Rgb).context("encoding JPEG")?;
    Ok(out)
}

/// A baseline 8-bit RGB TIFF, written with the EXIF writer so camera tags land in a
/// proper Exif IFD. ZIP uses Deflate with horizontal differencing (Predictor 2).
fn encode_tiff(img: &RgbImage, opts: &ImageOptions, meta: &EmbeddedMetadata) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    let row = w as usize * 3;
    // ~256 KB strips.
    let rows_per_strip = ((256 * 1024) / row.max(1)).clamp(1, h as usize);
    let strips: Vec<Vec<u8>> = img
        .as_raw()
        .par_chunks(row * rows_per_strip)
        .map(|strip| {
            if !opts.tiff_zip {
                return Ok(strip.to_vec());
            }
            let mut diff = strip.to_vec();
            for line in diff.chunks_mut(row) {
                for i in (3..line.len()).rev() {
                    line[i] = line[i].wrapping_sub(line[i - 3]);
                }
            }
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
            z.write_all(&diff)?;
            z.finish()
        })
        .collect::<std::io::Result<_>>()?;
    let strip_refs: Vec<&[u8]> = strips.iter().map(Vec::as_slice).collect();

    let tiff = |n: u16, value: Value| Field { tag: Tag(exif::Context::Tiff, n), ifd_num: In::PRIMARY, value };
    let mut fields = vec![
        Field { tag: Tag::ImageWidth, ifd_num: In::PRIMARY, value: Value::Long(vec![w]) },
        Field { tag: Tag::ImageLength, ifd_num: In::PRIMARY, value: Value::Long(vec![h]) },
        Field { tag: Tag::BitsPerSample, ifd_num: In::PRIMARY, value: Value::Short(vec![8, 8, 8]) },
        short(Tag::Compression, if opts.tiff_zip { 8 } else { 1 }),
        short(Tag::PhotometricInterpretation, 2),
        short(Tag::SamplesPerPixel, 3),
        Field {
            tag: Tag::RowsPerStrip,
            ifd_num: In::PRIMARY,
            value: Value::Long(vec![rows_per_strip as u32]),
        },
        short(Tag::PlanarConfiguration, 1),
        tiff(34675, Value::Undefined(srgb_icc(), 0)), // ICC profile
    ];
    if opts.tiff_zip {
        fields.push(tiff(317, Value::Short(vec![2]))); // Predictor: horizontal differencing
    }
    if let Some(xmp) = &meta.xmp {
        fields.push(tiff(700, Value::Byte(xmp.as_bytes().to_vec())));
    }
    fields.extend(exif_fields(meta, opts.ppi));

    let mut writer = ExifWriter::new();
    for f in &fields {
        writer.push_field(f);
    }
    writer.set_strips(&strip_refs, In::PRIMARY);
    let mut buf = Cursor::new(Vec::new());
    writer.write(&mut buf, true).context("encoding TIFF")?;
    Ok(buf.into_inner())
}

fn encode_png(img: &RgbImage, opts: &ImageOptions, meta: &EmbeddedMetadata) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    let mut info = png::Info::with_size(w, h);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(srgb_icc().into());
    info.exif_metadata = Some(exif_block(&exif_fields(meta, opts.ppi))?.into());
    let ppm = (f64::from(opts.ppi.max(1)) / 0.0254).round() as u32;
    info.pixel_dims = Some(png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter });
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::with_info(&mut out, info).context("PNG header")?;
        // fdeflate: several times faster than Balanced for a few percent bigger files.
        enc.set_compression(png::Compression::Fast);
        if let Some(xmp) = &meta.xmp {
            enc.add_itxt_chunk("XML:com.adobe.xmp".to_owned(), xmp.clone()).context("adding XMP")?;
        }
        let mut writer = enc.write_header().context("PNG header")?;
        writer.write_image_data(img.as_raw()).context("encoding PNG")?;
        writer.finish().context("encoding PNG")?;
    }
    Ok(out)
}

/// Encode `img` per `opts` (not `Original`).
pub fn encode(img: &RgbImage, opts: &ImageOptions, meta: &EmbeddedMetadata) -> Result<Vec<u8>> {
    match opts.format {
        FileFormat::Jpeg => encode_jpeg(img, opts, meta),
        FileFormat::Tiff => encode_tiff(img, opts, meta),
        FileFormat::Png => encode_png(img, opts, meta),
        FileFormat::Original => anyhow::bail!("Original is copied, not encoded"),
    }
}

/// Write `bytes` to `out` through a temporary file, so a failed or cancelled export
/// never leaves a half-written file under the real name.
pub fn write_atomically(out: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = out.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, out) {
        // Windows can't rename over an existing file (Overwrite).
        if out.exists() && std::fs::remove_file(out).is_ok() && std::fs::rename(&tmp, out).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", out.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
