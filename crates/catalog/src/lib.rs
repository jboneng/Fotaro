//! The photo catalog. No GUI code lives in this crate.

pub mod scan;

pub use scan::{Photo, is_photo, scan};
