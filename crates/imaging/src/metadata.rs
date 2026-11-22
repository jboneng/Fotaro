//! Camera and exposure metadata (RAW via LibRaw, other images via EXIF).

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
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
    /// The file's own orientation in clockwise quarter turns (EXIF / RAW flip).
    /// Mirrored EXIF orientations are not represented.
    pub orientation: Option<u8>,
    pub gps: Option<Gps>,
}

pub use libraw::Gps;

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
        orientation: Some(match info.orientation {
            libraw::Orientation::Normal => 0,
            libraw::Orientation::Rotate90Cw => 1,
            libraw::Orientation::Rotate180 => 2,
            libraw::Orientation::Rotate90Ccw => 3,
        }),
        gps: info.gps,
    }
}

/// Read metadata only (no pixel decoding).
pub fn read_metadata(path: &Path, kind: SourceKind) -> Result<Metadata> {
    let meta = match kind {
        SourceKind::Raw => {
            let raw =
                crate::storage::open_raw(path).with_context(|| format!("opening {}", path.display()))?;
            from_raw_info(&raw.info())
        }
        SourceKind::Image => read_exif(path)?,
    };
    Ok(with_file_time_fallback(meta, path))
}

/// Like LrC, use the file's modification time as the capture time when the file has
/// none (scanner output, exports, screenshots), so these sort sensibly by Capture Time.
pub(crate) fn with_file_time_fallback(mut meta: Metadata, path: &Path) -> Metadata {
    if meta.capture_time.is_none() {
        meta.capture_time = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .map(|t| chrono::DateTime::<chrono::Local>::from(t).naive_local().and_utc().timestamp());
    }
    meta
}

/// EXIF for JPEG/TIFF/PNG/WebP/HEIF. Files without EXIF yield empty metadata.
pub(crate) fn read_exif(path: &Path) -> Result<Metadata> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(file);
    // The exif crate reads a whole TIFF into memory to find its tags: 100 MB per film
    // scan, which over a network share takes seconds per photo. Read just the tags.
    let read = match tiff_tags(&mut reader) {
        Ok(Some(tags)) => exif::Reader::new().read_raw(tags),
        Ok(None) => {
            reader.seek(SeekFrom::Start(0))?;
            exif::Reader::new().read_from_container(&mut reader)
        }
        Err(e) => {
            log::debug!("no EXIF in {}: {e}", path.display());
            return Ok(Metadata::default());
        }
    };
    let exif = match read {
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
        orientation: Some(match field(Tag::Orientation).and_then(|f| f.value.get_uint(0)) {
            Some(6) => 1,
            Some(3) => 2,
            Some(8) => 3,
            _ => 0,
        }),
        gps: read_gps(&exif),
    })
}

/// The tag data of a classic TIFF (IFD0 and its EXIF, GPS and interoperability IFDs),
/// read by following the offsets rather than reading the image data. The result is a
/// copy of the file with only those bytes filled in (the rest stays zero, and the
/// allocation is only committed where written), ready for `exif::Reader::read_raw`.
/// `None` when the file is not a classic TIFF (then read it the usual way).
fn tiff_tags<R: Read + Seek>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::{Error, ErrorKind};
    /// A tag value this large is image data (strip tables, ICC, XMP blobs): not needed.
    const MAX_VALUE: u64 = 1 << 20;
    /// Enough for any real tag layout; protects against loops in a damaged file.
    const MAX_IFDS: usize = 8;
    const MAX_ENTRIES: u16 = 4096;

    let mut header = [0u8; 8];
    if r.read_exact(&mut header).is_err() {
        return Ok(None);
    }
    let little = match &header[..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return Ok(None), // not TIFF, or BigTIFF (left to the exif crate)
    };
    let u16_at =
        |b: &[u8]| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) };
    let u32_at = |b: &[u8]| {
        let b = [b[0], b[1], b[2], b[3]];
        if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let len = r.seek(SeekFrom::End(0))?;

    // (offset, bytes) pieces of the file to copy; IFD0's link to the next IFD is cut,
    // as only the primary image's tags are used.
    let mut pieces: Vec<(u64, Vec<u8>)> = vec![(0, header.to_vec())];
    let mut queue = vec![(u64::from(u32_at(&header[4..])), true)];
    let mut seen = 0;
    while let Some((offset, primary)) = queue.pop() {
        seen += 1;
        if seen > MAX_IFDS || offset < 8 || offset + 2 > len {
            continue;
        }
        r.seek(SeekFrom::Start(offset))?;
        let mut count = [0u8; 2];
        r.read_exact(&mut count)?;
        let count = u16_at(&count).min(MAX_ENTRIES);
        let size = 2 + u64::from(count) * 12 + 4;
        if offset + size > len {
            return Err(Error::new(ErrorKind::UnexpectedEof, "truncated TIFF directory"));
        }
        let mut ifd = vec![0u8; size as usize];
        r.seek(SeekFrom::Start(offset))?;
        r.read_exact(&mut ifd)?;
        for e in ifd[2..2 + count as usize * 12].as_chunks::<12>().0 {
            let (tag, kind, n) = (u16_at(&e[0..]), u16_at(&e[2..]), u64::from(u32_at(&e[4..])));
            let unit = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => continue,
            };
            let bytes = unit * n;
            let at = u64::from(u32_at(&e[8..]));
            // Exif, GPS and Interoperability IFD pointers.
            if matches!(tag, 0x8769 | 0x8825 | 0xa005) {
                queue.push((at, false));
            }
            if bytes > 4 && bytes <= MAX_VALUE && at + bytes <= len {
                let mut value = vec![0u8; bytes as usize];
                r.seek(SeekFrom::Start(at))?;
                r.read_exact(&mut value)?;
                pieces.push((at, value));
            }
        }
        if primary {
            ifd[size as usize - 4..].fill(0);
        }
        pieces.push((offset, ifd));
    }
    let end = pieces.iter().map(|(at, b)| at + b.len() as u64).max().unwrap_or(8);
    let mut out = vec![0u8; end as usize];
    for (at, bytes) in pieces {
        out[at as usize..at as usize + bytes.len()].copy_from_slice(&bytes);
    }
    Ok(Some(out))
}

/// EXIF GPS position (degrees/minutes/seconds rationals plus N/S, E/W references).
fn read_gps(exif: &exif::Exif) -> Option<Gps> {
    let field = |tag| exif.get_field(tag, In::PRIMARY);
    let degrees = |tag| match field(tag).map(|f| &f.value) {
        Some(Value::Rational(v)) if !v.is_empty() => {
            let part = |i: usize| v.get(i).map_or(0.0, |r| if r.denom == 0 { 0.0 } else { r.to_f64() });
            Some(part(0) + part(1) / 60.0 + part(2) / 3600.0)
        }
        _ => None,
    };
    let reference = |tag| match field(tag).map(|f| &f.value) {
        Some(Value::Ascii(parts)) => parts.first().and_then(|b| b.first()).map(|c| c.to_ascii_uppercase()),
        _ => None,
    };
    let lat = degrees(Tag::GPSLatitude)?;
    let lon = degrees(Tag::GPSLongitude)?;
    let (lat_ref, lon_ref) = (reference(Tag::GPSLatitudeRef)?, reference(Tag::GPSLongitudeRef)?);
    if !(lat.is_finite() && lon.is_finite()) || lat > 90.0 || lon > 180.0 || (lat == 0.0 && lon == 0.0) {
        return None;
    }
    let altitude = match field(Tag::GPSAltitude).map(|f| &f.value) {
        Some(Value::Rational(v)) => v.first().filter(|r| r.denom != 0).map(|r| {
            let below = field(Tag::GPSAltitudeRef).and_then(|f| f.value.get_uint(0)) == Some(1);
            if below { -r.to_f64() } else { r.to_f64() }
        }),
        _ => None,
    };
    Some(Gps {
        latitude: if lat_ref == b'S' { -lat } else { lat },
        longitude: if lon_ref == b'W' { -lon } else { lon },
        altitude,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Counts the bytes read, to check the image data is skipped.
    struct Counting<R> {
        inner: R,
        read: u64,
    }
    impl<R: Read> Read for Counting<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read += n as u64;
            Ok(n)
        }
    }
    impl<R: Seek> Seek for Counting<R> {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    /// A little-endian TIFF with `pixels` bytes of image data first and the tags after
    /// it (as Photoshop and Lightroom write them): IFD0 with Make and an Exif IFD with
    /// DateTimeOriginal, and IFD0 linking to a second IFD the reader must not need.
    fn tiff_with_tags_at_end(pixels: usize) -> Vec<u8> {
        let mut f = b"II\x2a\x00\0\0\0\0".to_vec();
        f.resize(8 + pixels, 0x55);
        let entry = |f: &mut Vec<u8>, tag: u16, kind: u16, n: u32, value: u32| {
            f.extend(tag.to_le_bytes());
            f.extend(kind.to_le_bytes());
            f.extend(n.to_le_bytes());
            f.extend(value.to_le_bytes());
        };
        let ifd0 = f.len() as u32;
        let make_at = ifd0 + 2 + 2 * 12 + 4;
        let exif_at = make_at + 8;
        let date_at = exif_at + 2 + 12 + 4;
        f[4..8].copy_from_slice(&ifd0.to_le_bytes());
        f.extend(2u16.to_le_bytes());
        entry(&mut f, 0x010f, 2, 8, make_at); // Make
        entry(&mut f, 0x8769, 4, 1, exif_at); // Exif IFD
        f.extend(0xffff_fff0u32.to_le_bytes()); // a next IFD past the end of the file
        f.extend(b"Hasseld\0");
        f.extend(1u16.to_le_bytes());
        entry(&mut f, 0x9003, 2, 20, date_at); // DateTimeOriginal
        f.extend(0u32.to_le_bytes());
        f.extend(b"2026:09:24 18:30:00\0");
        f
    }

    #[test]
    fn tiff_tags_skip_the_image_data() {
        let file = tiff_with_tags_at_end(8 << 20);
        let mut r = Counting { inner: Cursor::new(&file), read: 0 };
        let tags = tiff_tags(&mut r).unwrap().expect("a TIFF");
        assert!(r.read < 4096, "read {} bytes", r.read);
        let exif = exif::Reader::new().read_raw(tags).unwrap();
        let text = |tag| exif.get_field(tag, In::PRIMARY).map(|f| f.display_value().to_string());
        assert_eq!(text(Tag::Make).as_deref(), Some("\"Hasseld\""));
        assert_eq!(text(Tag::DateTimeOriginal).as_deref(), Some("2026-09-24 18:30:00"));
    }

    #[test]
    fn tiff_capture_time_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.tif");
        std::fs::write(&path, tiff_with_tags_at_end(1000)).unwrap();
        let m = read_exif(&path).unwrap();
        let expected = chrono::NaiveDate::from_ymd_opt(2026, 9, 24)
            .unwrap()
            .and_hms_opt(18, 30, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(m.capture_time, Some(expected));
        assert_eq!(m.make.as_deref(), Some("Hasseld"));
    }

    #[test]
    fn non_tiff_is_left_to_the_exif_crate() {
        assert!(tiff_tags(&mut Cursor::new(b"\xff\xd8\xff\xe0 jpeg".to_vec())).unwrap().is_none());
    }
}
