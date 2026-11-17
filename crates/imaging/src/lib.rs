//! Turning photo files into displayable pixels, and caching the results.
//!
//! - [`decode`]: RAW (embedded preview via LibRaw, full render as fallback) or
//!   regular images (8/16-bit, ICC-managed to sRGB), oriented and size-limited.
//! - [`read_metadata`]: camera/exposure metadata without decoding pixels.
//! - [`PreviewCache`]: thumbnail + standard JPEG previews on disk, keyed by a file fingerprint.

mod color;
mod decode;
pub mod develop;
pub mod export;
mod metadata;
mod preview;

pub use decode::{SourceKind, decode, decode_full, fit_image};
pub use image::RgbImage;
pub use metadata::{Gps, Metadata, read_metadata};
pub use preview::{BuiltPreviews, PreviewCache, PreviewSize, PurgeReport, fingerprint};
