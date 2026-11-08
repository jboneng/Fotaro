# Photo Library

A desktop photo library in Rust ([eframe/egui](https://github.com/emilk/egui)) modelled on the
Lightroom Classic *Library* module. See [docs/PLAN.md](docs/PLAN.md) for the roadmap.

## Getting the code

LibRaw is vendored as a git submodule:

```sh
git clone --recurse-submodules <repo>
# or, in an existing clone:
git submodule update --init
```

A C++ compiler is required (MSVC on Windows, clang on macOS, gcc/clang on Linux).

## Run

```sh
cargo run -p photo_library            # the app
cargo test --workspace                # tests
cargo run -p libraw --example rawinfo --release -- <raw files>   # RAW decode check
```

## Layout

| Path             | Purpose                                                  |
|------------------|----------------------------------------------------------|
| `crates/app`     | egui application (binary `photo_library`)               |
| `crates/catalog` | SQLite catalog: schema, migrations, folder sync, queries |
| `crates/imaging` | Decoding (RAW/images), ICC to sRGB, EXIF, preview cache |
| `crates/libraw`  | Safe Rust wrapper + C++ shim over vendored LibRaw 0.22.3 |
| `crates/xmp`     | XMP sidecars: read and update in place (LrC, Bridge, darktable) |

## Third-party licences

LibRaw is dual-licensed LGPL-2.1 / CDDL-1.0; this project uses and statically links it under
**CDDL-1.0**. Its unmodified source is in `crates/libraw/vendor/LibRaw`.

Development flags: `cargo run -p photo_library -- --catalog <dir.plcat> --add <folder>` opens (or
creates) a catalog and imports a folder at startup. Keyboard shortcuts: [docs/SHORTCUTS.md](docs/SHORTCUTS.md).
