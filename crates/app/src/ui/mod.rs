//! Lightroom Classic–style user interface.

mod collections;
mod crop;
mod develop;
mod masking;
mod remove;
pub use crop::{set_cropping, swap_orientation};
pub use masking::{delete_selected, new_mask_tool, set_masking};
pub(crate) use remove::prepare_person;
pub use remove::{delete_spot, new_source, set_removing, step_size as step_spot_size};
pub mod edit_in;
pub mod export;
mod filter_bar;
mod icons;
mod import;
mod keywords;
pub mod layout;
mod lens;
pub mod panels;
pub mod presets;
pub mod shortcuts;
pub mod sidebars;
pub mod theme;
mod views;
