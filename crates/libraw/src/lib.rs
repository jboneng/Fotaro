//! Safe wrapper around the vendored LibRaw (see `shim.cpp` for the C interface).
//!
//! ```no_run
//! let mut raw = libraw::RawFile::open("photo.NEF")?;
//! println!("{:?}", raw.info());
//! let preview = raw.thumbnail()?; // fast: embedded JPEG
//! let full = raw.render(false)?;  // slow: full demosaic to 8-bit sRGB
//! # Ok::<(), libraw::Error>(())
//! ```

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fmt;
use std::fs::File;
use std::path::Path;
use std::ptr::NonNull;

// Make sure the zlib that LibRaw's DNG decoder uses gets linked.
use libz_sys as _;

mod ffi {
    use super::*;

    #[repr(C)]
    pub struct Info {
        pub make: [c_char; 64],
        pub model: [c_char; 64],
        pub lens: [c_char; 128],
        pub iso: f32,
        pub shutter: f32,
        pub aperture: f32,
        pub focal_len: f32,
        pub timestamp: i64,
        pub width: i32,
        pub height: i32,
        pub flip: i32,
        pub thumb_format: i32,
        pub thumb_width: i32,
        pub thumb_height: i32,
    }

    #[repr(C)]
    pub struct Image {
        pub kind: i32,
        pub width: i32,
        pub height: i32,
        pub colors: i32,
        pub bits: i32,
        pub data: *const u8,
        pub len: usize,
        pub handle: *mut c_void,
    }

    unsafe extern "C" {
        pub fn pl_new() -> *mut c_void;
        pub fn pl_free(raw: *mut c_void);
        pub fn pl_open_buffer(raw: *mut c_void, buffer: *const c_void, size: usize) -> c_int;
        pub fn pl_get_info(raw: *mut c_void, out: *mut Info);
        pub fn pl_thumbnail(raw: *mut c_void, out: *mut Image) -> c_int;
        pub fn pl_render(raw: *mut c_void, half_size: c_int, out: *mut Image) -> c_int;
        pub fn pl_image_free(img: *mut Image);
        pub fn pl_strerror(code: c_int) -> *const c_char;
        pub fn pl_version() -> *const c_char;
    }
}

/// Version of the linked LibRaw, e.g. `"0.22.3-Release"`.
pub fn version() -> &'static str {
    // SAFETY: LibRaw returns a pointer to a static NUL-terminated string.
    unsafe { CStr::from_ptr(ffi::pl_version()) }.to_str().unwrap_or("unknown")
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    LibRaw { code: i32, message: String },
    OutOfMemory,
}

impl Error {
    fn from_code(code: c_int) -> Self {
        // SAFETY: LibRaw::strerror returns a static string for any code.
        let message = unsafe { CStr::from_ptr(ffi::pl_strerror(code)) }
            .to_string_lossy()
            .into_owned();
        Error::LibRaw { code, message }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::LibRaw { code, message } => write!(f, "LibRaw error {code}: {message}"),
            Error::OutOfMemory => f.write_str("LibRaw could not allocate a processor"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn check(code: c_int) -> Result<()> {
    if code == 0 { Ok(()) } else { Err(Error::from_code(code)) }
}

/// How the image must be rotated for display (from LibRaw's `flip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Normal,
    Rotate180,
    Rotate90Ccw,
    Rotate90Cw,
}

impl Orientation {
    fn from_flip(flip: i32) -> Self {
        match flip {
            3 => Self::Rotate180,
            5 => Self::Rotate90Ccw,
            6 => Self::Rotate90Cw,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawInfo {
    pub make: String,
    pub model: String,
    pub lens: String,
    pub iso: f32,
    /// Exposure time in seconds.
    pub shutter: f32,
    pub aperture: f32,
    /// Focal length in mm.
    pub focal_len: f32,
    /// Capture time as Unix seconds (camera clock, usually local time).
    pub timestamp: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
    pub thumb_width: u32,
    pub thumb_height: u32,
}

/// An embedded preview as stored by the camera.
#[derive(Debug, Clone)]
pub enum Thumbnail {
    Jpeg(Vec<u8>),
    JpegXl(Vec<u8>),
    Bitmap(Bitmap),
}

/// Uncompressed interleaved pixels (`colors` channels of `bits` bits each).
#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub colors: u32,
    pub bits: u32,
    pub data: Vec<u8>,
}

/// An open RAW file. Not thread-safe to share, but can be moved between threads.
pub struct RawFile {
    raw: NonNull<c_void>,
    // LibRaw reads from this mapping; it must outlive `raw` (see Drop).
    _map: memmap2::Mmap,
}

// SAFETY: a LibRaw instance has no thread affinity; we never share it (&mut for work).
unsafe impl Send for RawFile {}

impl RawFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the mapping is read-only. If another process truncates the file
        // while we read it the OS may raise a fault; this is the standard mmap caveat.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        // SAFETY: plain allocation.
        let raw = NonNull::new(unsafe { ffi::pl_new() }).ok_or(Error::OutOfMemory)?;
        let this = RawFile { raw, _map: map };
        // SAFETY: `raw` is valid and the buffer lives as long as `this`.
        check(unsafe {
            ffi::pl_open_buffer(this.raw.as_ptr(), this._map.as_ptr().cast(), this._map.len())
        })?;
        Ok(this)
    }

    pub fn info(&self) -> RawInfo {
        // SAFETY: Info is plain-old-data; the shim fills every field.
        let mut i: ffi::Info = unsafe { std::mem::zeroed() };
        unsafe { ffi::pl_get_info(self.raw.as_ptr(), &mut i) };
        let s = |b: &[c_char]| {
            // SAFETY: the shim always NUL-terminates these buffers.
            unsafe { CStr::from_ptr(b.as_ptr()) }.to_string_lossy().trim().to_owned()
        };
        RawInfo {
            make: s(&i.make),
            model: s(&i.model),
            lens: s(&i.lens),
            iso: i.iso,
            shutter: i.shutter,
            aperture: i.aperture,
            focal_len: i.focal_len,
            timestamp: (i.timestamp != 0).then_some(i.timestamp),
            width: i.width.max(0) as u32,
            height: i.height.max(0) as u32,
            orientation: Orientation::from_flip(i.flip),
            thumb_width: i.thumb_width.max(0) as u32,
            thumb_height: i.thumb_height.max(0) as u32,
        }
    }

    /// Extract the camera's embedded preview. Fast; no demosaicing.
    pub fn thumbnail(&mut self) -> Result<Thumbnail> {
        let img = self.call_image(|raw, out| unsafe { ffi::pl_thumbnail(raw, out) })?;
        Ok(match img.0.kind {
            1 => Thumbnail::Jpeg(img.bytes().to_vec()),
            3 => Thumbnail::JpegXl(img.bytes().to_vec()),
            _ => Thumbnail::Bitmap(img.bitmap()),
        })
    }

    /// Fully decode to 8-bit sRGB with as-shot white balance.
    /// `half_size` skips demosaicing and halves each dimension (much faster).
    pub fn render(&mut self, half_size: bool) -> Result<Bitmap> {
        let img = self.call_image(|raw, out| unsafe { ffi::pl_render(raw, half_size.into(), out) })?;
        Ok(img.bitmap())
    }

    fn call_image(
        &mut self,
        f: impl FnOnce(*mut c_void, *mut ffi::Image) -> c_int,
    ) -> Result<OwnedImage> {
        // SAFETY: Image is plain-old-data.
        let mut img = OwnedImage(unsafe { std::mem::zeroed() });
        check(f(self.raw.as_ptr(), &mut img.0))?;
        Ok(img)
    }
}

impl Drop for RawFile {
    fn drop(&mut self) {
        // Runs before fields drop, so LibRaw is gone before the mapping is unmapped.
        unsafe { ffi::pl_free(self.raw.as_ptr()) };
    }
}

/// A LibRaw-allocated image, freed on drop.
struct OwnedImage(ffi::Image);

impl OwnedImage {
    fn bytes(&self) -> &[u8] {
        if self.0.data.is_null() {
            return &[];
        }
        // SAFETY: LibRaw guarantees `len` readable bytes at `data` until freed.
        unsafe { std::slice::from_raw_parts(self.0.data, self.0.len) }
    }

    fn bitmap(&self) -> Bitmap {
        Bitmap {
            width: self.0.width.max(0) as u32,
            height: self.0.height.max(0) as u32,
            colors: self.0.colors.max(0) as u32,
            bits: self.0.bits.max(0) as u32,
            data: self.bytes().to_vec(),
        }
    }
}

impl Drop for OwnedImage {
    fn drop(&mut self) {
        unsafe { ffi::pl_image_free(&mut self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reports_vendored_version() {
        assert!(version().starts_with("0.22.3"), "got {}", version());
    }

    #[test]
    fn non_raw_file_is_an_error_not_a_crash() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"definitely not a raw file").unwrap();
        assert!(matches!(RawFile::open(f.path()), Err(Error::LibRaw { .. })));
    }

    #[test]
    fn missing_file_is_io_error() {
        assert!(matches!(RawFile::open("does/not/exist.nef"), Err(Error::Io(_))));
    }
}
