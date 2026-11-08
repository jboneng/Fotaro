# Photo Library

A desktop photo library written in Rust with [eframe/egui](https://github.com/emilk/egui).

## Run

```sh
cargo run            # debug build
cargo run --release  # optimized; no console window on Windows
cargo test           # unit tests for the non-GUI core
```

## Layout

| Path             | Purpose                                                  |
|------------------|----------------------------------------------------------|
| `src/main.rs`    | Entry point: logging, window options, launches the app   |
| `src/app.rs`     | egui UI: menu bar, photo list, details panel, persistence |
| `src/library.rs` | Photo discovery on disk (no GUI deps, unit-tested)       |
| `src/lib.rs`     | Crate root exposing the modules                          |

The last opened folder is saved between runs through eframe's `persistence` feature.
Set `RUST_LOG=debug` for more verbose logging.
