//! Small images from big uncompressed TIFFs without reading the whole file.
//!
//! Film scans are often 100 MB or more of uncompressed 16-bit RGB with no embedded
//! preview. Decoding one for a 256 px thumbnail means reading all of it, which over a
//! network share takes a second or more per photo. Uncompressed strips can instead be
//! read row by row: only every n-th row is read, and each is averaged down along the
//! row, so a thumbnail costs a twentieth of the reads.

use std::io::{Read, Seek, SeekFrom};

use image::{DynamicImage, ImageBuffer, Luma, Rgb};

/// The rows read give this many times the requested size. Each row is its own read
/// (over a network share, a round trip and often a disk seek), so this sets the cost:
/// 1 reads about one row in twenty of a 5000 px scan for a 256 px thumbnail, and each
/// row is averaged along its length, which looks as good as a full decode at that size.
const OVERSAMPLE: u32 = 1;
/// Only worth it when it skips most of the file.
const MIN_STEP: u32 = 3;

/// A reduced copy of an uncompressed, strip-organised 8- or 16-bit gray or RGB TIFF,
/// at least `OVERSAMPLE` times `max_px` on its longest side (before orientation), with
/// the file's ICC profile, orientation (EXIF numbering) and full size. `None` for any
/// other kind of file, which is then decoded the usual way.
pub(crate) struct Sampled {
    pub image: DynamicImage,
    pub icc: Option<Vec<u8>>,
    pub orientation: u8,
    pub full_size: (u32, u32),
}

struct Layout {
    little: bool,
    width: u32,
    height: u32,
    channels: u32,
    /// Bytes per sample: 1 or 2.
    depth: u32,
    rows_per_strip: u32,
    strip_offsets: Vec<u64>,
    orientation: u8,
    icc: Option<(u64, u64)>,
}

pub(crate) fn sample<R: Read + Seek>(r: &mut R, max_px: u32) -> Option<Sampled> {
    let layout = read_layout(r).ok()??;
    let step = layout.width.max(layout.height) / (max_px.saturating_mul(OVERSAMPLE)).max(1);
    if step < MIN_STEP {
        return None;
    }
    let (out_w, out_h) = (layout.width.div_ceil(step), layout.height.div_ceil(step));
    let row_bytes = layout.width as u64 * layout.channels as u64 * layout.depth as u64;
    // Gray stays gray; RGB (plus any extra channels such as alpha) becomes RGB.
    let keep = if layout.channels >= 3 { 3 } else { 1 };
    let mut out: Vec<u16> = Vec::with_capacity((out_w * out_h * keep) as usize);
    let mut row = vec![0u8; row_bytes as usize];
    for oy in 0..out_h {
        // The middle row of each block of `step` rows.
        let y = (oy * step + step / 2).min(layout.height - 1);
        let strip = (y / layout.rows_per_strip) as usize;
        let start = *layout.strip_offsets.get(strip)? + (y % layout.rows_per_strip) as u64 * row_bytes;
        r.seek(SeekFrom::Start(start)).ok()?;
        r.read_exact(&mut row).ok()?;
        for ox in 0..out_w {
            let x0 = ox * step;
            let x1 = (x0 + step).min(layout.width);
            for c in 0..keep {
                let mut sum = 0u32;
                for x in x0..x1 {
                    let i = ((x * layout.channels + c) * layout.depth) as usize;
                    sum += match (layout.depth, layout.little) {
                        (1, _) => u32::from(row[i]),
                        (_, true) => u32::from(u16::from_le_bytes([row[i], row[i + 1]])),
                        (_, false) => u32::from(u16::from_be_bytes([row[i], row[i + 1]])),
                    };
                }
                out.push((sum / (x1 - x0)) as u16);
            }
        }
    }
    let image = match (keep, layout.depth) {
        (3, 2) => DynamicImage::ImageRgb16(ImageBuffer::<Rgb<u16>, _>::from_raw(out_w, out_h, out)?),
        (3, _) => DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_raw(
            out_w,
            out_h,
            out.into_iter().map(|v| v as u8).collect(),
        )?),
        (_, 2) => DynamicImage::ImageLuma16(ImageBuffer::<Luma<u16>, _>::from_raw(out_w, out_h, out)?),
        _ => DynamicImage::ImageLuma8(ImageBuffer::<Luma<u8>, _>::from_raw(
            out_w,
            out_h,
            out.into_iter().map(|v| v as u8).collect(),
        )?),
    };
    let icc = layout.icc.and_then(|(at, len)| {
        let mut bytes = vec![0u8; len as usize];
        r.seek(SeekFrom::Start(at)).ok()?;
        r.read_exact(&mut bytes).ok()?;
        Some(bytes)
    });
    Some(Sampled { image, icc, orientation: layout.orientation, full_size: (layout.width, layout.height) })
}

/// IFD0 of a classic TIFF, if its image is one [`sample`] can read.
fn read_layout<R: Read + Seek>(r: &mut R) -> std::io::Result<Option<Layout>> {
    let mut header = [0u8; 8];
    r.read_exact(&mut header)?;
    let little = match &header[..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return Ok(None),
    };
    let u16_at =
        |b: &[u8]| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) };
    let u32_at = |b: &[u8]| {
        let b = [b[0], b[1], b[2], b[3]];
        if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let len = r.seek(SeekFrom::End(0))?;
    let ifd = u64::from(u32_at(&header[4..]));
    if ifd + 2 > len {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(ifd))?;
    let mut count = [0u8; 2];
    r.read_exact(&mut count)?;
    let count = u16_at(&count) as usize;
    let mut entries = vec![0u8; count * 12];
    r.read_exact(&mut entries)?;

    // Values of SHORT/LONG tags (inline or not); `None` for other types.
    let mut values = |e: &[u8], max: u64| -> std::io::Result<Option<Vec<u64>>> {
        let (kind, n) = (u16_at(&e[2..]), u64::from(u32_at(&e[4..])));
        let unit = match kind {
            3 => 2,
            4 => 4,
            _ => return Ok(None),
        };
        if n > max {
            return Ok(None);
        }
        let bytes = if unit * n <= 4 {
            e[8..12].to_vec()
        } else {
            let at = u64::from(u32_at(&e[8..]));
            if at + unit * n > len {
                return Ok(None);
            }
            let mut b = vec![0u8; (unit * n) as usize];
            r.seek(SeekFrom::Start(at))?;
            r.read_exact(&mut b)?;
            b
        };
        Ok(Some(
            bytes
                .chunks_exact(unit as usize)
                .take(n as usize)
                .map(|c| if unit == 2 { u64::from(u16_at(c)) } else { u64::from(u32_at(c)) })
                .collect(),
        ))
    };
    let (mut width, mut height, mut bits, mut channels) = (0, 0, vec![1], 1);
    let (mut compression, mut photometric, mut planar, mut format) = (1, u64::MAX, 1, vec![1]);
    let (mut rows_per_strip, mut strip_offsets, mut orientation, mut icc) = (u64::MAX, None, 1, None);
    for e in entries.as_chunks::<12>().0 {
        let tag = u16_at(e);
        let first = |v: Option<Vec<u64>>| v.and_then(|v| v.first().copied());
        match tag {
            0x100 => width = first(values(e, 1)?).unwrap_or(0),
            0x101 => height = first(values(e, 1)?).unwrap_or(0),
            0x102 => bits = values(e, 16)?.unwrap_or_default(),
            0x103 => compression = first(values(e, 1)?).unwrap_or(0),
            0x106 => photometric = first(values(e, 1)?).unwrap_or(u64::MAX),
            0x111 => strip_offsets = values(e, 1 << 20)?,
            0x112 => orientation = first(values(e, 1)?).unwrap_or(1),
            0x115 => channels = first(values(e, 1)?).unwrap_or(0),
            0x116 => rows_per_strip = first(values(e, 1)?).unwrap_or(0),
            0x11c => planar = first(values(e, 1)?).unwrap_or(0),
            0x142 | 0x144 => return Ok(None), // tiled
            0x153 => format = values(e, 16)?.unwrap_or_default(),
            0x8773 => {
                let (n, at) = (u64::from(u32_at(&e[4..])), u64::from(u32_at(&e[8..])));
                if u16_at(&e[2..]) == 7 && n > 4 && at + n <= len && n <= 1 << 22 {
                    icc = Some((at, n));
                }
            }
            _ => {}
        }
    }
    let depth = match bits.first() {
        Some(8) => 1,
        Some(16) => 2,
        _ => return Ok(None),
    };
    let readable = width > 0
        && height > 0
        && width <= u64::from(u32::MAX)
        && height <= u64::from(u32::MAX)
        && compression == 1
        && planar == 1
        && matches!((photometric, channels), (1, 1 | 2) | (2, 3 | 4))
        && bits.iter().all(|b| *b == bits[0])
        && format.iter().all(|f| *f == 1)
        && rows_per_strip > 0;
    let Some(strip_offsets) = strip_offsets.filter(|_| readable) else { return Ok(None) };
    let rows_per_strip = rows_per_strip.min(height) as u32;
    // Every strip must lie inside the file (the last one may hold fewer rows).
    let row_bytes = width * channels * depth;
    let strips = height.div_ceil(u64::from(rows_per_strip));
    if (strip_offsets.len() as u64) < strips
        || strip_offsets.iter().take(strips as usize).enumerate().any(|(i, at)| {
            let rows = (height - i as u64 * u64::from(rows_per_strip)).min(u64::from(rows_per_strip));
            at + rows * row_bytes > len
        })
    {
        return Ok(None);
    }
    Ok(Some(Layout {
        little,
        width: width as u32,
        height: height as u32,
        channels: channels as u32,
        depth: depth as u32,
        rows_per_strip,
        strip_offsets,
        orientation: orientation.clamp(1, 8) as u8,
        icc,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Counts the bytes read, to check most of the file is skipped.
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

    fn encode(img: &DynamicImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Tiff).unwrap();
        out.into_inner()
    }

    #[test]
    fn samples_a_16bit_rgb_tiff_reading_a_fraction() {
        // Left half dark, right half bright, in 16 bits.
        let img = ImageBuffer::from_fn(3000, 2000, |x, _| {
            if x < 1500 { Rgb([1000u16, 2000, 3000]) } else { Rgb([60000u16, 50000, 40000]) }
        });
        let file = encode(&DynamicImage::ImageRgb16(img));
        let mut r = Counting { inner: Cursor::new(&file), read: 0 };
        let s = sample(&mut r, 256).expect("an uncompressed TIFF");
        assert!(r.read * 4 < file.len() as u64, "read {} of {} bytes", r.read, file.len());
        assert_eq!(s.full_size, (3000, 2000));
        let DynamicImage::ImageRgb16(small) = &s.image else { panic!("not 16-bit RGB") };
        assert!(small.width() >= 256 && small.width() < 3000 / MIN_STEP, "width {}", small.width());
        assert_eq!(small.get_pixel(0, 0).0, [1000, 2000, 3000]);
        assert_eq!(small.get_pixel(small.width() - 1, small.height() - 1).0, [60000, 50000, 40000]);
    }

    #[test]
    fn samples_8bit_gray() {
        let img = ImageBuffer::from_fn(2400, 1600, |_, y| Luma([if y < 800 { 10u8 } else { 200 }]));
        let s = sample(&mut Cursor::new(encode(&DynamicImage::ImageLuma8(img))), 256).unwrap();
        let DynamicImage::ImageLuma8(small) = &s.image else { panic!("not 8-bit gray") };
        assert_eq!(small.get_pixel(0, 0).0, [10]);
        assert_eq!(small.get_pixel(0, small.height() - 1).0, [200]);
    }

    #[test]
    fn leaves_small_and_other_files_alone() {
        let small = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(600, 400, Rgb([1u8, 2, 3])));
        assert!(sample(&mut Cursor::new(encode(&small)), 256).is_none(), "too small to be worth it");
        let mut png = Cursor::new(Vec::new());
        small.write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.set_position(0);
        assert!(sample(&mut png, 64).is_none());
    }
}
