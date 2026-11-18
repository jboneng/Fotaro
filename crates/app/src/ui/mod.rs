//! Lightroom Classic–style user interface.

mod collections;
mod develop;
mod masking;
pub use masking::{delete_selected, new_mask_tool, set_masking};
pub mod edit_in;
pub mod export;
mod filter_bar;
mod icons;
mod import;
mod keywords;
pub mod layout;
pub mod panels;
pub mod shortcuts;
pub mod sidebars;
pub mod theme;
mod views;
