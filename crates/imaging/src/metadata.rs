//! Camera and exposure metadata (RAW via LibRaw, other images via EXIF).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result};
use exif::{In, Tag, Value};

use crate::decode::SourceKind;

/// Capture times are stored as the camera's local wall-clock time expressed as
/// seconds since 1970 *as if it were UTC* ("naive local"), so RAW and JPEG files
/// from the same camera sort together regardless of the computer's time zone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<f64>,
    /// Exposure time in seconds.
    pub shutter: Option<f64>,
    pub aperture: Option<f64>,
    /// Focal length in mm.
    pub focal_len: Option<f64>,
    /// Naive-local capture time, see type docs.
    pub capture_time: Option<i64>,
    /// Final (oriented) pixel dimensions of the full image.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim().trim_matches('\0').trim();
    (!s.is_empty()).then(|| s.to_owned())
}

fn positive(v: f32) -> Option<f64> {
    (v.is_finite() && v > 0.0).then_some(f64::from(v))
}

pub(crate) fn from_raw_info(info: &libraw::RawInfo) -> Metadata {
    use chrono::TimeZone;
    let rotated =
        matches!(info.orientation, libraw::Orientation::Rotate90Cw | libraw::Orientation::Rotate90Ccw);
    let (w, h) = if rotated { (info.height, info.width) } else { (info.width, info.height) };
    Metadata {
        make: non_empty(&info.make),
        model: non_empty(&info.model),
        lens: non_empty(&info.lens),
        iso: positive(info.iso),
        shutter: positive(info.shutter),
        aperture: positive(info.aperture),
        focal_len: positive(info.focal_len),
        // LibRaw converts the camera's wall-clock time with mktime(); undo that.
        capture_time: info.timestamp.and_then(|ts| {
            chrono::Local.timestamp_opt(ts, 0).single().map(|t| t.naive_local().and_utc().timestamp())
        }),
        width: (w > 0).then_some(w),
        height: (h > 0).then_some(h),
    }
}

/// Read metadata only (no pixel decoding).
pub fn read_metadata(path: &Path, kind: SourceKind) -> Result<Metadata> {
    match kind {
        SourceKind::Raw => {
            let raw = libraw::RawFile::open(path).with_context(|| format!("opening {}", path.display()))?;
            Ok(from_raw_info(&raw.info()))
        }
        SourceKind::Image => read_exif(path),
    }
}

/// EXIF for JPEG/TIFF/PNG/WebP/HEIF. Files without EXIF yield empty metadata.
pub(crate) fn read_exif(path: &Path) -> Result<Metadata> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let exif = match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(e) => e,
        Err(exif::Error::NotFound(_)) => return Ok(Metadata::default()),
        Err(e) => {
            log::debug!("no EXIF in {}: {e}", path.display());
            return Ok(Metadata::default());
        }
    };
    let field = |tag| exif.get_field(tag, In::PRIMARY);
    let text = |tag| match field(tag).map(|f| &f.value) {
        Some(Value::Ascii(parts)) => parts.first().and_then(|b| non_empty(&String::from_utf8_lossy(b))),
        _ => None,
    };
    let number = |tag| {
        field(tag).and_then(|f| match &f.value {
            Value::Rational(v) => v.first().map(|r| r.to_f64()),
            Value::SRational(v) => v.first().map(|r| r.to_f64()),
            _ => f.value.get_uint(0).map(f64::from),
        })
    };
    let capture_time = text(Tag::DateTimeOriginal)
        .or_else(|| text(Tag::DateTime))
        .and_then(|s| chrono::NaiveDateTime::parse_from_str(&s, "%Y:%m:%d %H:%M:%S").ok())
        .map(|t| t.and_utc().timestamp());

    Ok(Metadata {
        make: text(Tag::Make),
        model: text(Tag::Model),
        lens: text(Tag::LensModel),
        iso: number(Tag::PhotographicSensitivity),
        shutter: number(Tag::ExposureTime),
        aperture: number(Tag::FNumber),
        focal_len: number(Tag::FocalLength),
        capture_time,
        width: None,
        height: None,
    })
}
