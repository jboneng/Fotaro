//! The photo catalog: SQLite database, folder sync and scanning. No GUI code here.

mod catalog;
mod collections;
mod folders;
pub mod import;
mod keywords;
pub mod query;
pub mod scan;
mod schema;
mod sync;
mod virtual_copies;
mod xmp_sync;

pub use catalog::{
    CATALOG_EXTENSION, Catalog, Flag, Folder, PhotoMetadata, PhotoRow, SortKey, SortOrder, Source, StackInfo,
};
pub use collections::{Collection, CollectionKind};
pub use folders::{Relocated, validate_folder_name};
pub use import::{
    Candidate, DateFolders, ImportMode, ImportReport, ImportSettings, RENAME_PRESETS, RENAME_TOKENS,
};
pub use keywords::{Keyword, parse_keyword_input};
pub use query::{Criterion, Facet, MetaColumn, NumOp, SmartRules, TextField, TextOp};
pub use scan::{FileKind, Photo, file_kind, is_photo, scan};
pub use schema::LATEST_VERSION as SCHEMA_VERSION;
pub use sync::{MetadataReader, SyncOptions, SyncReport};
pub use xmp_sync::{Iptc, IptcField, OrientationReader};

#[cfg(test)]
mod find_tests;
