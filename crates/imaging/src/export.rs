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

use crate::decode::{SourceKind, decode_for_export};
use crate::metadata::{Gps, Metadata};

mod effects;
mod space;

pub use effects::{Anchor, SharpenAmount, SharpenFor, Watermark, WatermarkStyle};
pub use space::ColorSpace;

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
    pub color_space: ColorSpace,
    /// TIFF and PNG: 16 bits per channel instead of 8.
    pub sixteen_bit: bool,
    /// JPEG: Limit File Size To `limit_kb` K (the quality setting is ignored).
    pub limit_size: bool,
    pub limit_kb: u32,
    /// Output Sharpening.
    pub sharpen: bool,
    pub sharpen_for: SharpenFor,
    pub sharpen_amount: SharpenAmount,
    pub watermark: Watermark,
}

/// Bit depths TIFF and PNG can be written with.
pub const BIT_DEPTH_FORMATS: [FileFormat; 2] = [FileFormat::Tiff, FileFormat::Png];

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
            color_space: ColorSpace::Srgb,
            sixteen_bit: false,
            limit_size: false,
            limit_kb: 500,
            sharpen: false,
            sharpen_for: SharpenFor::Screen,
            sharpen_amount: SharpenAmount::Standard,
            watermark: Watermark::default(),
        }
    }
}

impl ImageOptions {
    /// 16 bits per channel actually used (only TIFF and PNG have it).
    pub fn sixteen(&self) -> bool {
        self.sixteen_bit && BIT_DEPTH_FORMATS.contains(&self.format)
    }

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
    /// Where the photo was taken (left out for "Remove Location Info").
    pub gps: Option<Gps>,
}

/// Decode `path` at full size and turn it into the export's pixels: converted to the
/// output colour space, turned upright by `turns` clockwise quarter turns (the catalog
/// rotation), resized, sharpened and watermarked per `opts`. `copyright` is the photo's
/// copyright, for the simple copyright watermark. Returns 8-bit RGB, or 16-bit when
/// [`ImageOptions::sixteen`], and the file's metadata.
pub fn render(
    path: &Path,
    kind: SourceKind,
    turns: u8,
    opts: &ImageOptions,
    copyright: Option<&str>,
) -> Result<(DynamicImage, Metadata)> {
    let sixteen = opts.sixteen();
    let (img, source, meta) = decode_for_export(path, kind, sixteen)?;
    let img = space::to_target(img, source, opts.color_space, sixteen);
    let img = match turns % 4 {
        1 => img.rotate90(),
        2 => img.rotate180(),
        3 => img.rotate270(),
        _ => img,
    };
    let (w, h) = opts.target_size(img.width(), img.height());
    let mut img = resize(img, w, h)?;
    if opts.sharpen {
        let (sigma, amount) = effects::sharpen_params(opts.sharpen_for, opts.sharpen_amount, opts.ppi);
        effects::sharpen(&mut img, sigma, amount);
    }
    opts.watermark.apply(&mut img, copyright, opts.color_space)?;
    Ok((img, meta))
}

fn resize(img: DynamicImage, w: u32, h: u32) -> Result<DynamicImage> {
    if (w, h) == (img.width(), img.height()) {
        return Ok(img);
    }
    let mut dst = DynamicImage::new(w, h, img.color());
    // Lanczos3 is a touch crisper than the previews' CatmullRom; exports are final output.
    let opts = fir::ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3));
    fir::Resizer::new().resize(&img, &mut dst, &opts).context("resizing")?;
    Ok(dst)
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
fn exif_fields(meta: &EmbeddedMetadata, opts: &ImageOptions) -> Vec<Field> {
    let ppi = opts.ppi;
    let mut f = vec![
        short(Tag::ColorSpace, opts.color_space.exif_code()),
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
    if let Some(g) = &meta.gps {
        f.extend(gps_fields(g));
    }
    f
}

/// Degrees as EXIF's degrees / minutes / seconds rationals.
fn dms(v: f64) -> Vec<Rational> {
    let v = v.abs();
    let deg = v.trunc();
    let min = ((v - deg) * 60.0).trunc();
    let sec = ((v - deg) * 60.0 - min) * 60.0;
    vec![
        Rational { num: deg as u32, denom: 1 },
        Rational { num: min as u32, denom: 1 },
        Rational { num: (sec * 10_000.0).round() as u32, denom: 10_000 },
    ]
}

fn gps_fields(g: &Gps) -> Vec<Field> {
    let field = |tag, value| Field { tag, ifd_num: In::PRIMARY, value };
    let mut f = vec![
        field(Tag::GPSVersionID, Value::Byte(vec![2, 3, 0, 0])),
        ascii(Tag::GPSLatitudeRef, if g.latitude < 0.0 { "S" } else { "N" }),
        field(Tag::GPSLatitude, Value::Rational(dms(g.latitude))),
        ascii(Tag::GPSLongitudeRef, if g.longitude < 0.0 { "W" } else { "E" }),
        field(Tag::GPSLongitude, Value::Rational(dms(g.longitude))),
    ];
    if let Some(alt) = g.altitude.filter(|a| a.is_finite()) {
        f.push(field(Tag::GPSAltitudeRef, Value::Byte(vec![u8::from(alt < 0.0)])));
        f.push(rational(Tag::GPSAltitude, alt.abs(), 100));
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
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend(exif_block(&exif_fields(meta, opts))?);
    let icc = opts.color_space.icc();
    let at_quality = |quality: u8| -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
        if exif.len() <= APP1_MAX {
            enc.add_app_segment(1, exif.clone()).context("adding EXIF")?;
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
        enc.add_icc_profile(&icc).context("adding ICC profile")?;
        enc.encode(img.as_raw(), w16, h16, jpeg_encoder::ColorType::Rgb).context("encoding JPEG")?;
        Ok(out)
    };
    if !opts.limit_size {
        return at_quality(opts.jpeg_quality);
    }
    // Limit File Size: the highest quality that fits, by bisection (about 7 encodes).
    let limit = opts.limit_kb.max(1) as usize * 1024;
    let (mut lo, mut hi, mut best) = (1u8, 100u8, None);
    while lo <= hi {
        let q = lo + (hi - lo) / 2;
        let bytes = at_quality(q)?;
        if bytes.len() <= limit {
            best = Some(bytes);
            lo = q + 1;
        } else if q == 1 {
            break;
        } else {
            hi = q - 1;
        }
    }
    best.with_context(|| {
        format!(
            "can't fit {w}×{h} pixels in {} K; choose a smaller image size or a larger limit",
            opts.limit_kb
        )
    })
}

#[derive(Clone, Copy)]
enum TiffSamples<'a> {
    Eight(&'a [u8]),
    Sixteen(&'a [u16]),
}

/// Pixel rows as bytes in the file's byte order, with TIFF horizontal differencing
/// (Predictor 2) when `predict` (differences are taken per sample, 8 or 16 bit).
fn tiff_strip(samples: TiffSamples<'_>, row: usize, predict: bool) -> Vec<u8> {
    fn differences<T: Copy + std::ops::Sub<Output = T>>(
        v: &mut [T],
        row: usize,
        wrapping_sub: fn(T, T) -> T,
    ) {
        for line in v.chunks_mut(row) {
            for i in (3..line.len()).rev() {
                line[i] = wrapping_sub(line[i], line[i - 3]);
            }
        }
    }
    match samples {
        TiffSamples::Eight(s) => {
            let mut out = s.to_vec();
            if predict {
                differences(&mut out, row, u8::wrapping_sub);
            }
            out
        }
        TiffSamples::Sixteen(s) => {
            let mut v = s.to_vec();
            if predict {
                differences(&mut v, row, u16::wrapping_sub);
            }
            // Little-endian, matching the "II" header the writer emits.
            v.iter().flat_map(|s| s.to_le_bytes()).collect()
        }
    }
}

/// A baseline RGB TIFF (8 or 16 bit), written with the EXIF writer so camera tags land
/// in a proper Exif IFD. ZIP uses Deflate with horizontal differencing (Predictor 2).
fn encode_tiff(img: &DynamicImage, opts: &ImageOptions, meta: &EmbeddedMetadata) -> Result<Vec<u8>> {
    let (w, h) = (img.width(), img.height());
    let row = w as usize * 3;
    let bits: u16 = if matches!(img, DynamicImage::ImageRgb16(_)) { 16 } else { 8 };
    let row_bytes = row * usize::from(bits / 8);
    // ~256 KB strips.
    let rows_per_strip = ((256 * 1024) / row_bytes.max(1)).clamp(1, h.max(1) as usize);
    let compress = |samples: TiffSamples<'_>| -> std::io::Result<Vec<u8>> {
        let bytes = tiff_strip(samples, row, opts.tiff_zip);
        if !opts.tiff_zip {
            return Ok(bytes);
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        z.write_all(&bytes)?;
        z.finish()
    };
    let strips: Vec<Vec<u8>> = match img {
        DynamicImage::ImageRgb16(b) => b
            .as_raw()
            .par_chunks(row * rows_per_strip)
            .map(|s| compress(TiffSamples::Sixteen(s)))
            .collect::<std::io::Result<_>>()?,
        DynamicImage::ImageRgb8(b) => b
            .as_raw()
            .par_chunks(row * rows_per_strip)
            .map(|s| compress(TiffSamples::Eight(s)))
            .collect::<std::io::Result<_>>()?,
        _ => anyhow::bail!("unexpected pixel format for TIFF"),
    };
    let strip_refs: Vec<&[u8]> = strips.iter().map(Vec::as_slice).collect();

    let tiff = |n: u16, value: Value| Field { tag: Tag(exif::Context::Tiff, n), ifd_num: In::PRIMARY, value };
    let mut fields = vec![
        Field { tag: Tag::ImageWidth, ifd_num: In::PRIMARY, value: Value::Long(vec![w]) },
        Field { tag: Tag::ImageLength, ifd_num: In::PRIMARY, value: Value::Long(vec![h]) },
        Field { tag: Tag::BitsPerSample, ifd_num: In::PRIMARY, value: Value::Short(vec![bits; 3]) },
        short(Tag::Compression, if opts.tiff_zip { 8 } else { 1 }),
        short(Tag::PhotometricInterpretation, 2),
        short(Tag::SamplesPerPixel, 3),
        Field {
            tag: Tag::RowsPerStrip,
            ifd_num: In::PRIMARY,
            value: Value::Long(vec![rows_per_strip as u32]),
        },
        short(Tag::PlanarConfiguration, 1),
        tiff(34675, Value::Undefined(opts.color_space.icc(), 0)), // ICC profile
    ];
    if opts.tiff_zip {
        fields.push(tiff(317, Value::Short(vec![2]))); // Predictor: horizontal differencing
    }
    if let Some(xmp) = &meta.xmp {
        fields.push(tiff(700, Value::Byte(xmp.as_bytes().to_vec())));
    }
    fields.extend(exif_fields(meta, opts));

    let mut writer = ExifWriter::new();
    for f in &fields {
        writer.push_field(f);
    }
    writer.set_strips(&strip_refs, In::PRIMARY);
    let mut buf = Cursor::new(Vec::new());
    writer.write(&mut buf, true).context("encoding TIFF")?;
    Ok(buf.into_inner())
}

fn encode_png(img: &DynamicImage, opts: &ImageOptions, meta: &EmbeddedMetadata) -> Result<Vec<u8>> {
    let (w, h) = (img.width(), img.height());
    let mut info = png::Info::with_size(w, h);
    info.color_type = png::ColorType::Rgb;
    let data: std::borrow::Cow<'_, [u8]> = match img {
        DynamicImage::ImageRgb8(b) => {
            info.bit_depth = png::BitDepth::Eight;
            b.as_raw().into()
        }
        DynamicImage::ImageRgb16(b) => {
            info.bit_depth = png::BitDepth::Sixteen;
            // PNG samples are big-endian.
            b.as_raw().iter().flat_map(|s| s.to_be_bytes()).collect::<Vec<u8>>().into()
        }
        _ => anyhow::bail!("unexpected pixel format for PNG"),
    };
    info.icc_profile = Some(opts.color_space.icc().into());
    info.exif_metadata = Some(exif_block(&exif_fields(meta, opts))?.into());
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
        writer.write_image_data(&data).context("encoding PNG")?;
        writer.finish().context("encoding PNG")?;
    }
    Ok(out)
}

/// Encode `img` (from [`render`]) per `opts` (not `Original`).
pub fn encode(img: &DynamicImage, opts: &ImageOptions, meta: &EmbeddedMetadata) -> Result<Vec<u8>> {
    match opts.format {
        FileFormat::Jpeg => match img {
            DynamicImage::ImageRgb8(rgb) => encode_jpeg(rgb, opts, meta),
            other => encode_jpeg(&other.to_rgb8(), opts, meta),
        },
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
