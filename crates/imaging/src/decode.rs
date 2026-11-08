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

fn decode_raw(path: &Path, max_px: u32) -> Result<(RgbImage, Metadata)> {
    let mut raw = libraw::RawFile::open(path)?;
    let info = raw.info();
    let meta = metadata::from_raw_info(&info);

    // Fast path: the camera's embedded preview (what LrC calls "embedded previews").
    let wanted = (max_px as f32 * MIN_EMBEDDED_FRACTION) as u32;
    let embedded = match raw.thumbnail() {
        Ok(libraw::Thumbnail::Jpeg(bytes)) => image::load_from_memory(&bytes).ok(),
        Ok(libraw::Thumbnail::Bitmap(b)) => bitmap_to_image(b).ok(),
        Ok(libraw::Thumbnail::JpegXl(_)) | Err(_) => None,
    }
    .filter(|img| img.width().max(img.height()) >= wanted);

    let img = match embedded {
        Some(img) => img,
        None => {
            log::debug!("{}: no usable embedded preview, rendering", path.display());
            bitmap_to_image(raw.render(true)?)?
        }
    };
    let img = orient_raw(fit(img, max_px), info.orientation);
    Ok((img.into_rgb8(), meta))
}

fn decode_image(path: &Path, max_px: u32) -> Result<(RgbImage, Metadata)> {
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
