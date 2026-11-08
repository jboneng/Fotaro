//! Compiles the vendored LibRaw sources plus our C++ shim into a static library.

use std::path::{Path, PathBuf};

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read LibRaw src dir") {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "cpp")
            // *_ph.cpp are placeholder stubs for builds without postprocessing.
            && !path.file_stem().unwrap().to_string_lossy().ends_with("_ph")
        {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let libraw = manifest.join("vendor/LibRaw");
    let src = libraw.join("src");
    assert!(src.is_dir(), "LibRaw sources missing at {}. Run: git submodule update --init", src.display());

    let mut sources = Vec::new();
    collect_sources(&src, &mut sources);
    sources.sort();

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include(&libraw)
        .files(&sources)
        .file(manifest.join("src/shim.cpp"))
        .define("LIBRAW_NODLL", None)
        .define("USE_ZLIB", None)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .warnings(false)
        .extra_warnings(false);

    if let Ok(zlib_include) = std::env::var("DEP_Z_INCLUDE") {
        build.include(zlib_include);
    }

    let target = std::env::var("TARGET").unwrap();
    if target.contains("msvc") {
        build.flag("/EHsc").flag("/utf-8");
    } else {
        build.flag_if_supported("-std=c++14");
        build.flag_if_supported("-pthread");
    }

    build.compile("raw_vendored");

    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-changed=src/shim.h");
    println!("cargo:rerun-if-changed=vendor/LibRaw/libraw/libraw_version.h");
}
