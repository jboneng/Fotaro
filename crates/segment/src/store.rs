//! Where the AI files live and how they get there: ONNX Runtime (with DirectML on
//! Windows) and the models, downloaded on first use into the user's local app data,
//! each checked against its SHA-256 before it is used.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// A model the app can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiModel {
    /// Select Subject / Background: BRIA's RMBG-1.4.
    Subject,
    /// Select Sky: SegFormer-B2 trained on ADE20K.
    Sky,
    /// Select Object: Segment Anything 2.1 (tiny), an image encoder and a prompt decoder.
    Object,
    /// Remove: LaMa-Dilated (dilated convolutions instead of Fourier units: runs on GPUs).
    Remove,
    /// Remove at best quality: big-LaMa (int4/int8 weights; DirectML runs a copy with them
    /// decoded, see `folded`).
    RemoveBest,
    /// Dust & Scratches: the scratch detector of "Bringing Old Photos Back to Life".
    Scratches,
    /// Distractions → People: RT-DETR (R18) trained on COCO, finding people and what
    /// they carry.
    People,
}

impl AiModel {
    pub fn label(self) -> &'static str {
        match self {
            AiModel::Subject => "Subject (RMBG-1.4)",
            AiModel::Sky => "Sky (SegFormer-B2, ADE20K)",
            AiModel::Object => "Objects (SAM 2.1 tiny)",
            AiModel::Remove => "Remove (LaMa-Dilated)",
            AiModel::RemoveBest => "Remove, best quality (big-LaMa)",
            AiModel::Scratches => "Dust & Scratches (old-photo scratch detector)",
            AiModel::People => "People (RT-DETR)",
        }
    }

    fn assets(self) -> &'static [Asset] {
        match self {
            AiModel::Subject => &SUBJECT,
            AiModel::Sky => &SKY,
            AiModel::Object => &OBJECT,
            AiModel::Remove => &REMOVE,
            AiModel::RemoveBest => &REMOVE_BEST,
            AiModel::Scratches => &SCRATCHES,
            AiModel::People => &PEOPLE,
        }
    }

    /// The model file (once downloaded): the first of its files (SAM: the encoder).
    pub fn path(self) -> PathBuf {
        data_dir().join(self.assets()[0].file)
    }

    /// Its `n`th file (SAM: 0 the encoder, 1 the decoder; their weights sit beside them).
    pub fn file(self, n: usize) -> PathBuf {
        data_dir().join(self.assets()[n].file)
    }

    fn present(self) -> bool {
        self.assets().iter().all(|a| data_dir().join(a.file).exists())
    }
}

/// A file to download, with what it must hash to.
struct Asset {
    url: &'static str,
    sha256: &'static str,
    size: u64,
    /// Where it goes, under the AI folder.
    file: &'static str,
}

const SUBJECT: [Asset; 1] = [Asset {
    url: "https://huggingface.co/briaai/RMBG-1.4/resolve/main/onnx/model.onnx",
    sha256: "8cafcf770b06757c4eaced21b1a88e57fd2b66de01b8045f35f01535ba742e0f",
    size: 176_153_355,
    file: "rmbg-1.4.onnx",
}];

const SKY: [Asset; 1] = [Asset {
    url: "https://huggingface.co/Xenova/segformer-b2-finetuned-ade-512-512/resolve/main/onnx/model.onnx",
    sha256: "819c15e6af8c4de3359c1de7ab0a17d0dde495df1d16f8908a7163f8038e0fa0",
    size: 110_445_327,
    file: "segformer-b2-ade.onnx",
}];

/// LaMa-Dilated for Remove (BSD-3-Clause): Qualcomm AI Hub's export of the LaMa variant with
/// dilated convolutions.
const REMOVE: [Asset; 1] = [Asset {
    url: "https://huggingface.co/pxGeniusAI/lama-dilated/resolve/main/lama_dilated.onnx",
    sha256: "591973938bfdd6102c82404d02d3d9473b4935cd2a8f4cf6748112d78ddc5800",
    size: 182_789_617,
    file: "lama-dilated.onnx",
}];

/// Big-LaMa for Remove's best quality (Apache-2.0): Carve's ONNX export, quantized and
/// rewritten so no tensor has more than 4 dimensions.
const REMOVE_BEST: [Asset; 1] = [Asset {
    url: "https://huggingface.co/othytenk/lama-int4-int8/resolve/main/lama_int4_int8_r4.onnx",
    sha256: "4e3b78a253eb4abf21896811c928ffa91214b916e5d6ced24cecdd7999b4aaa1",
    size: 36_931_151,
    file: "lama-r4.onnx",
}];

/// Dust & Scratches' detector (MIT): the scratch-detection U-Net of Wan et al., "Bringing
/// Old Photos Back to Life" (Microsoft), as artiva-ai export it to ONNX: grey in, the
/// chance of damage out, any size (multiples of 16).
const SCRATCHES: [Asset; 1] = [Asset {
    url: "https://huggingface.co/artiva-ai/modelos/resolve/main/rasgunos_old_photos.onnx",
    sha256: "7fde08b6af282f5b577ae1ef296a0614d7f3541637051df1a552da12bf78ec6f",
    size: 150_605_103,
    file: "scratches.onnx",
}];

/// People's detector (Apache-2.0): RT-DETR R18 by PekingU (trained on COCO), as
/// onnx-community export it, int8. Any input size; 640 × 640 as trained.
const PEOPLE: [Asset; 1] = [Asset {
    url: "https://huggingface.co/onnx-community/rtdetr_r18vd/resolve/main/onnx/model_int8.onnx",
    sha256: "328dfaa2505a376b2316a8fec5dcbc52125c477d8cd744977db5922c1b4d4486",
    size: 21_713_196,
    file: "rtdetr-r18vd-int8.onnx",
}];

/// SAM 2.1 tiny: the graphs keep their weights in `.onnx_data` files, found by name next
/// to them, so they keep their names in a folder of their own.
const OBJECT: [Asset; 4] = [
    Asset {
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/vision_encoder.onnx",
        sha256: "4f30aacd3aaefbca81a0b7fe4c1fc96345570ea0a6f80ced599493d1b3be2e8c",
        size: 354_238,
        file: "sam2.1-tiny/vision_encoder.onnx",
    },
    Asset {
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/prompt_encoder_mask_decoder.onnx",
        sha256: "874414704c5d686db7d206a35f6e15d26563d50c8c4468fccc6739bd7e491dcf",
        size: 213_114,
        file: "sam2.1-tiny/prompt_encoder_mask_decoder.onnx",
    },
    Asset {
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/vision_encoder.onnx_data",
        sha256: "e83df9866a5afe68ea7f0f721f18f65137fc3acbf0da1c74e946d363e09c69cc",
        size: 134_084_864,
        file: "sam2.1-tiny/vision_encoder.onnx_data",
    },
    Asset {
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/main/onnx/prompt_encoder_mask_decoder.onnx_data",
        sha256: "e9874d900dd4134ed60eab1e97910327c2419e0b2954485d8fd6e7f1a1470f47",
        size: 20_958_208,
        file: "sam2.1-tiny/prompt_encoder_mask_decoder.onnx_data",
    },
];

/// ONNX Runtime with DirectML, and DirectML itself, as Microsoft publishes them (NuGet
/// packages: zip files); the files to take out of each, and where they go.
#[cfg(windows)]
const RUNTIME: [(Asset, &[(&str, &str)]); 2] = [
    // DirectML first: the runtime counts as there once onnxruntime.dll is.
    (
        Asset {
            url: "https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/1.15.4/microsoft.ai.directml.1.15.4.nupkg",
            sha256: "4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9",
            size: 202_292_617,
            file: "directml.nupkg",
        },
        &[("bin/x64-win/DirectML.dll", "DirectML.dll")],
    ),
    (
        Asset {
            url: "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg",
            sha256: "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681",
            size: 12_458_649,
            file: "onnxruntime.nupkg",
        },
        &[
            ("runtimes/win-x64/native/onnxruntime_providers_shared.dll", "onnxruntime_providers_shared.dll"),
            ("runtimes/win-x64/native/onnxruntime.dll", "onnxruntime.dll"),
        ],
    ),
];

/// ONNX Runtime's library file name on this system.
#[cfg(windows)]
const RUNTIME_LIB: &str = "onnxruntime.dll";
#[cfg(target_os = "macos")]
const RUNTIME_LIB: &str = "libonnxruntime.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const RUNTIME_LIB: &str = "libonnxruntime.so";

/// The folder the AI files are kept in (`%LOCALAPPDATA%\PhotoLibrary\data\ai` on Windows).
pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "PhotoLibrary")
        .map(|d| d.data_local_dir().join("ai"))
        .unwrap_or_else(|| std::env::temp_dir().join("PhotoLibrary-ai"))
}

/// ONNX Runtime's library: `ORT_DYLIB_PATH`, else next to the program (a packaged app),
/// else the downloaded one.
pub fn runtime_lib() -> PathBuf {
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH") {
        return PathBuf::from(p);
    }
    let next_to_exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(RUNTIME_LIB)));
    match next_to_exe {
        Some(p) if p.exists() => p,
        _ => data_dir().join("runtime").join(RUNTIME_LIB),
    }
}

fn runtime_ready() -> bool {
    runtime_lib().exists()
}

/// Is everything there to run `models`?
pub fn ready(models: &[AiModel]) -> bool {
    runtime_ready() && models.iter().all(|m| m.present())
}

/// Bytes still to download for `models` (the runtime included).
pub fn download_size(models: &[AiModel]) -> u64 {
    let dir = data_dir();
    let mut total = 0;
    #[cfg(windows)]
    if !runtime_ready() {
        total += RUNTIME.iter().map(|(a, _)| a.size).sum::<u64>();
    }
    for m in models {
        for a in m.assets() {
            if !dir.join(a.file).exists() {
                total += a.size;
            }
        }
    }
    total
}

/// Download what `models` need, telling `progress` the share done (0..1) and what is
/// being fetched. Files appear only once complete and checked.
pub fn ensure(models: &[AiModel], progress: &mut dyn FnMut(f32, &str)) -> Result<()> {
    let total = download_size(models).max(1) as f64;
    let mut done = 0u64;
    let dir = data_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    if !runtime_ready() {
        #[cfg(windows)]
        {
            let runtime = dir.join("runtime");
            fs::create_dir_all(&runtime)?;
            for (asset, files) in &RUNTIME {
                let package = dir.join(asset.file);
                let what = "ONNX Runtime and DirectML";
                download(asset, &package, &mut |n| progress(((done + n) as f64 / total) as f32, what))?;
                done += asset.size;
                extract(&package, files, &runtime)?;
                let _ = fs::remove_file(&package);
            }
        }
        #[cfg(not(windows))]
        bail!(
            "AI masks need ONNX Runtime ({RUNTIME_LIB}) next to the program or in {}",
            dir.join("runtime").display()
        );
    }
    for m in models {
        for asset in m.assets() {
            let path = dir.join(asset.file);
            if path.exists() {
                continue;
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            download(asset, &path, &mut |n| progress(((done + n) as f64 / total) as f32, m.label()))?;
            done += asset.size;
        }
    }
    progress(1.0, "Done");
    Ok(())
}

/// Fetch `asset` to `to` (through a partial file), checking its size and SHA-256.
fn download(asset: &Asset, to: &Path, progress: &mut dyn FnMut(u64)) -> Result<()> {
    let part = PathBuf::from(format!("{}.part", to.display()));
    let mut response = ureq::get(asset.url).call().with_context(|| format!("downloading {}", asset.url))?;
    let mut reader = response.body_mut().with_config().limit(asset.size + 1).reader();
    let mut file = fs::File::create(&part).with_context(|| format!("writing {}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = reader.read(&mut buf)?;
        if k == 0 {
            break;
        }
        hasher.update(&buf[..k]);
        file.write_all(&buf[..k])?;
        n += k as u64;
        progress(n);
    }
    file.flush()?;
    drop(file);
    let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if n != asset.size || hash != asset.sha256 {
        let _ = fs::remove_file(&part);
        bail!("{} didn't download intact (got {n} bytes, SHA-256 {hash})", asset.url);
    }
    fs::rename(&part, to).with_context(|| format!("saving {}", to.display()))?;
    Ok(())
}

/// Take `files` (path in the zip, file name) out of a NuGet package into `dir`.
#[cfg(windows)]
fn extract(package: &Path, files: &[(&str, &str)], dir: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(package)?).context("reading a NuGet package")?;
    for (inner, name) in files {
        let mut entry = zip.by_name(inner).with_context(|| format!("{inner} in {}", package.display()))?;
        let part = dir.join(format!("{name}.part"));
        let mut out = fs::File::create(&part)?;
        std::io::copy(&mut entry, &mut out)?;
        drop(out);
        fs::rename(&part, dir.join(name))?;
    }
    Ok(())
}
