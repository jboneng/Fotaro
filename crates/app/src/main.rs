// Hide the console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod develop;
mod editors;
mod export;
mod filter;
mod import;
mod jobs;
mod lens;
mod migrate;
mod perf;
mod platform;
mod presets;
mod textures;
mod ui;
mod update;

use std::path::PathBuf;

/// The app's name: in the title bar, and its folders (settings, presets, AI models).
pub const APP_NAME: &str = "Fotaro";
/// The app's version (the workspace's, in `Cargo.toml`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// mimalloc avoids allocator lock contention between the UI thread and the many
// worker threads that allocate large image buffers.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use app::FotaroApp;

/// Command line: `fotaro [--catalog <dir.plcat>] [--add <folder>]`.
/// `--catalog` opens (or creates) that catalog instead of the last one;
/// `--add` imports a folder into it at startup.
#[derive(Debug, Default)]
pub struct Args {
    pub catalog: Option<PathBuf>,
    pub add: Option<PathBuf>,
    /// Open the Import dialog on this folder.
    pub import: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--catalog") => args.catalog = it.next().map(PathBuf::from),
            Some("--add") => args.add = it.next().map(PathBuf::from),
            Some("--import") => args.import = it.next().map(PathBuf::from),
            _ => log::warn!("ignoring unknown argument {a:?}"),
        }
    }
    args
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // Before anything reads them: the folders the app had under its old name.
    migrate::move_old_folders();
    let args = parse_args();

    // egui-wgpu caps textures at 8192 px; ask for what the GPU supports (often 16384) so
    // 1:1 previews of large scans and medium-format raws show at full resolution.
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut wgpu_options.wgpu_setup {
        let default_descriptor = std::sync::Arc::clone(&create.device_descriptor);
        create.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = default_descriptor(adapter);
            let limits = &mut descriptor.required_limits;
            limits.max_texture_dimension_2d =
                limits.max_texture_dimension_2d.max(adapter.limits().max_texture_dimension_2d);
            descriptor
        });
    }
    let options = eframe::NativeOptions {
        wgpu_options,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1400.0, 880.0])
            .with_min_inner_size([900.0, 560.0])
            .with_maximized(true),
        ..Default::default()
    };

    eframe::run_native(APP_NAME, options, Box::new(move |cc| Ok(Box::new(FotaroApp::new(cc, args)))))
}
