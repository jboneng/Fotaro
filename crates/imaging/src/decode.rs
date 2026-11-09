//! Decode any supported file into an oriented sRGB image no larger than requested.

use std::path::Path;

use anyhow::{Context, Result};
use fast_image_resize as fir;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage};

use crate::color;
use crate::metadata::{self, Metadata};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Raw,
    Image,
}

/// An embedded RAW preview is used when its longest side is at least this fraction of the
/// requested size. Smaller ones (e.g. the 1024 px previews in converted DNGs) look soft in
/// the loupe, so the RAW is rendered instead, as LrC does for standard previews.
const MIN_EMBEDDED_FRACTION: f32 = 0.75;

/// Decode `path` to an sRGB image whose longest side is at most `max_px`,
/// rotated for display. Also returns the file's metadata (read while decoding).
pub fn decode(path: &Path, kind: SourceKind, max_px: u32) -> Result<(RgbImage, Metadata)> {
    let (img, meta) = match kind {
        SourceKind::Raw => decode_raw(path, max_px),
        SourceKind::Image => decode_image(path, max_px),
    }
    .with_context(|| format!("decoding {}", path.display()))?;
    Ok((img, metadata::with_file_time_fallback(meta, path)))
}

fn fit(img: DynamicImage, max_px: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let longest = w.max(h);
    if longest <= max_px {
        return img;
    }
    let scale = max_px as f64 / longest as f64;
    let (dw, dh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));

    // fast_image_resize (SIMD) is ~20x faster than `image`'s resize: a 24 MP
    // CatmullRom reduction drops from ~750 ms to a few tens of ms.
    let mut dst = DynamicImage::new(dw, dh, img.color());
    let opts = fir::ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom));
    match fir::Resizer::new().resize(&img, &mut dst, &opts) {
        Ok(()) => dst,
        Err(e) => {
            log::debug!("fast resize unavailable for {:?} ({e}), using fallback", img.color());
            img.resize(max_px, max_px, FilterType::CatmullRom)
        }
    }
}

fn bitmap_to_image(b: libraw::Bitmap) -> Result<DynamicImage> {
    anyhow::ensure!(b.bits == 8, "unsupported {}-bit RAW bitmap", b.bits);
    match b.colors {
        3 => RgbImage::from_raw(b.width, b.height, b.data).map(DynamicImage::ImageRgb8),
        1 => image::GrayImage::from_raw(b.width, b.height, b.data).map(DynamicImage::ImageLuma8),
        n => anyhow::bail!("unsupported {n}-channel RAW bitmap"),
    }
    .context("RAW bitmap has an inconsistent size")
}

fn orient_raw(img: DynamicImage, o: libraw::Orientation) -> DynamicImage {
    match o {
        libraw::Orientation::Normal => img,
        libraw::Orientation::Rotate180 => img.rotate180(),
        libraw::Orientation::Rotate90Cw => img.rotate90(),
        libraw::Orientation::Rotate90Ccw => img.rotate270(),
    }
}

/// Decode at full resolution, for 1:1 viewing. RAW files use the camera's embedded
/// JPEG when it is (nearly) full size, so the colours match the standard preview;
/// otherwise LibRaw renders the full image.
pub fn decode_full(path: &Path, kind: SourceKind) -> Result<(RgbImage, Metadata)> {
    let (img, meta) = match kind {
        SourceKind::Raw => decode_raw_with(path, u32::MAX, true),
        SourceKind::Image => decode_image(path, u32::MAX),
    }
    .with_context(|| format!("decoding {} at full size", path.display()))?;
    Ok((img, metadata::with_file_time_fallback(meta, path)))
}

/// Downscale an image so its longest side is at most `max_px` (never upscales).
pub fn fit_image(img: DynamicImage, max_px: u32) -> DynamicImage {
    fit(img, max_px)
}

fn decode_raw(path: &Path, max_px: u32) -> Result<(RgbImage, Metadata)> {
    decode_raw_with(path, max_px, false)
}

/// Decode a RAW file from its embedded preview when that is big enough, else with a
/// LibRaw render (half size unless more than half the sensor is wanted).
/// `full_size` asks for the whole image (1:1 previews).
/// The long side an embedded preview needs to count as full size: 90% of the
/// sensor's (embedded JPEGs are often a few pixels smaller).
fn full_size_preview(info: &libraw::RawInfo) -> u32 {
    info.width.max(info.height) * 9 / 10
}

/// The camera's embedded preview (what LrC calls "embedded previews"), if its long side
/// is at least `wanted` pixels. Not yet oriented.
fn embedded_preview(raw: &mut libraw::RawFile, wanted: u32) -> Option<DynamicImage> {
    match raw.thumbnail() {
        Ok(libraw::Thumbnail::Jpeg(bytes)) => image::load_from_memory(&bytes).ok(),
        Ok(libraw::Thumbnail::Bitmap(b)) => bitmap_to_image(b).ok(),
        Ok(libraw::Thumbnail::JpegXl(_)) | Err(_) => None,
    }
    .filter(|img| img.width().max(img.height()) >= wanted)
}

fn decode_raw_with(path: &Path, max_px: u32, full_size: bool) -> Result<(RgbImage, Metadata)> {
    let mut raw = libraw::RawFile::open(path)?;
    let info = raw.info();
    let meta = metadata::from_raw_info(&info);
    let wanted =
        if full_size { full_size_preview(&info) } else { (max_px as f32 * MIN_EMBEDDED_FRACTION) as u32 };
    let embedded = embedded_preview(&mut raw, wanted);

    let img = match embedded {
        Some(img) => img,
        None => {
            // A half-size render is plenty unless more than half the sensor is wanted.
            let half = info.width.max(info.height) / 2;
            let half_size = max_px <= half;
            log::debug!("{}: no usable embedded preview, rendering (half size: {half_size})", path.display());
            bitmap_to_image(raw.render(half_size)?)?
        }
    };
    let img = orient_raw(fit(img, max_px), info.orientation);
    Ok((img.into_rgb8(), meta))
}

/// Decode a non-RAW image at full size and native bit depth, oriented, with its ICC
/// profile (if any).
fn open_image(path: &Path) -> Result<(DynamicImage, Option<Vec<u8>>)> {
    // Memory-map instead of `ImageReader::open`: TIFF decoding seeks a lot, and every seek
    // discards a BufReader's buffer (a 184 MB 16-bit TIFF decoded in 2 s that way vs 1 s).
    let file = std::fs::File::open(path)?;
    // SAFETY: the map is read-only and dropped before returning. If another process
    // truncates the file meanwhile we may read garbage (or fault), the same risk every
    // mmap-based viewer accepts; LibRaw files are opened the same way.
    let map = unsafe { memmap2::Mmap::map(&file)? };
    let mut decoder =
        ImageReader::new(std::io::Cursor::new(&map[..])).with_guessed_format()?.into_decoder()?;
    let icc = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok((img, icc))
}

/// The colour space of decoded pixels, for export colour management.
pub(crate) enum SourceSpace {
    Srgb,
    /// An embedded ICC profile (RGB or gray).
    Icc(Vec<u8>),
    /// LibRaw's 16-bit linear ProPhoto render.
    LinearProPhoto,
}

/// Decode for export: full size, oriented, in the source's own colour space.
/// RAW files give what the app shows at 1:1, their full-size camera preview, so an
/// export looks like the Library; only RAWs without one are rendered, at 16 bits when
/// `sixteen`. Other `sixteen` sources keep their 16 bits.
pub(crate) fn decode_for_export(
    path: &Path,
    kind: SourceKind,
    sixteen: bool,
) -> Result<(DynamicImage, SourceSpace, Metadata)> {
    let (img, space, meta) = match kind {
        SourceKind::Raw if sixteen => (|| {
            let mut raw = libraw::RawFile::open(path)?;
            let info = raw.info();
            if let Some(img) = embedded_preview(&mut raw, full_size_preview(&info)) {
                let img = DynamicImage::ImageRgb8(orient_raw(img, info.orientation).into_rgb8());
                return Ok((img, SourceSpace::Srgb, metadata::from_raw_info(&info)));
            }
            let b = raw.render16_linear_prophoto(false)?;
            anyhow::ensure!(b.bits == 16 && b.colors == 3, "unexpected {}x{}-bit render", b.colors, b.bits);
            let samples: Vec<u16> =
                b.data.as_chunks::<2>().0.iter().map(|c| u16::from_ne_bytes(*c)).collect();
            let img = image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(b.width, b.height, samples)
                .context("RAW render has an inconsistent size")?;
            let img = orient_raw(DynamicImage::ImageRgb16(img), info.orientation);
            Ok((img, SourceSpace::LinearProPhoto, metadata::from_raw_info(&info)))
        })(),
        SourceKind::Raw => decode_raw_with(path, u32::MAX, true)
            .map(|(img, meta)| (DynamicImage::ImageRgb8(img), SourceSpace::Srgb, meta)),
        SourceKind::Image => open_image(path).map(|(img, icc)| {
            let mut meta = metadata::read_exif(path).unwrap_or_default();
            (meta.width, meta.height) = (Some(img.width()), Some(img.height()));
            let img = match img {
                // Keep the precision of 16-bit (and float) sources only when asked.
                DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) => img,
                DynamicImage::ImageLuma16(_) | DynamicImage::ImageLumaA16(_) if sixteen => {
                    DynamicImage::ImageLuma16(img.to_luma16())
                }
                DynamicImage::ImageLumaA8(_) => DynamicImage::ImageLuma8(img.to_luma8()),
                DynamicImage::ImageLuma16(_) | DynamicImage::ImageLumaA16(_) => {
                    DynamicImage::ImageLuma8(img.to_luma8())
                }
                _ if sixteen && img.color().bytes_per_pixel() / img.color().channel_count() > 1 => {
                    DynamicImage::ImageRgb16(img.to_rgb16())
                }
                _ => DynamicImage::ImageRgb8(img.to_rgb8()),
            };
            (img, icc.map_or(SourceSpace::Srgb, SourceSpace::Icc), meta)
        }),
    }
    .with_context(|| format!("decoding {} for export", path.display()))?;
    Ok((img, space, metadata::with_file_time_fallback(meta, path)))
}

fn decode_image(path: &Path, max_px: u32) -> Result<(RgbImage, Metadata)> {
    let (img, icc) = open_image(path)?;
    let (full_w, full_h) = (img.width(), img.height());

    // Resize while still at full bit depth (16-bit scans), then convert to 8-bit sRGB.
    let img = fit(img, max_px);
    let is_gray = !img.color().has_color();
    let rgb = match (&icc, is_gray) {
        (Some(icc), true) => {
            let gray = img.to_luma8();
            color::gray_to_srgb(&gray, icc).unwrap_or_else(|| DynamicImage::ImageLuma8(gray).into_rgb8())
        }
        (Some(icc), false) => {
            let mut rgb = img.into_rgb8();
            color::rgb_to_srgb(&mut rgb, icc);
            rgb
        }
        (None, _) => img.into_rgb8(),
    };

    let mut meta = metadata::read_exif(path).unwrap_or_default();
    meta.width = Some(full_w);
    meta.height = Some(full_h);
    Ok((rgb, meta))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Luma, Rgb};

    #[test]
    fn decodes_and_downsizes_16bit_tiff() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("scan.tif");
        let img: ImageBuffer<Rgb<u16>, Vec<u16>> =
            ImageBuffer::from_fn(1200, 800, |x, _| Rgb([(x * 50) as u16, 32768, 65535]));
        img.save(&path).unwrap();

        let (out, meta) = decode(&path, SourceKind::Image, 300).unwrap();
        assert_eq!((out.width(), out.height()), (300, 200));
        assert_eq!((meta.width, meta.height), (Some(1200), Some(800)));
        let p = out.get_pixel(150, 100).0;
        assert_eq!(p[2], 255, "16-bit white must map to 8-bit 255, got {p:?}");
        assert!((127..=129).contains(&p[1]), "mid-grey should map to ~128, got {p:?}");
    }

    #[test]
    fn decodes_16bit_grayscale_tiff_to_rgb() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("neg.tif");
        let img: ImageBuffer<Luma<u16>, Vec<u16>> = ImageBuffer::from_pixel(100, 50, Luma([65535]));
        img.save(&path).unwrap();
        let (out, _) = decode(&path, SourceKind::Image, 2048).unwrap();
        assert_eq!((out.width(), out.height()), (100, 50));
        assert_eq!(out.get_pixel(0, 0).0, [255, 255, 255]);
    }

    #[test]
    fn small_images_are_not_upscaled() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("small.png");
        RgbImage::from_pixel(40, 30, Rgb([1, 2, 3])).save(&path).unwrap();
        let (out, meta) = decode(&path, SourceKind::Image, 256).unwrap();
        assert_eq!((out.width(), out.height()), (40, 30));
        assert!(meta.capture_time.is_some(), "files without EXIF fall back to the file date");
        assert_eq!(crate::read_metadata(&path, SourceKind::Image).unwrap().capture_time, meta.capture_time);
    }

    #[test]
    fn corrupt_files_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.jpg");
        std::fs::write(&path, b"not a jpeg").unwrap();
        assert!(decode(&path, SourceKind::Image, 256).is_err());
        let raw = tmp.path().join("broken.NEF");
        std::fs::write(&raw, b"not a raw").unwrap();
        assert!(decode(&raw, SourceKind::Raw, 256).is_err());
    }
}
