//! The photo catalog: SQLite database, folder sync and scanning. No GUI code here.

mod catalog;
pub mod scan;
mod schema;
mod sync;

pub use catalog::{
    CATALOG_EXTENSION, Catalog, Flag, Folder, PhotoMetadata, PhotoRow, SortKey, SortOrder, Source,
};
pub use scan::{FileKind, Photo, file_kind, is_photo, scan};
pub use schema::LATEST_VERSION as SCHEMA_VERSION;
pub use sync::{MetadataReader, SyncReport};
