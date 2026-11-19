//! Which GPU DirectML runs on. Laptops often have two: the integrated one is the system's
//! first adapter (DirectML's default), the discrete one is several times faster. We pick
//! the adapter with the most dedicated memory.

use std::sync::OnceLock;

/// A graphics adapter: its DXGI index (DirectML's device id), name and dedicated memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub index: i32,
    pub name: String,
    pub memory: u64,
}

/// The hardware adapters, in DXGI order (software renderers left out).
#[cfg(windows)]
pub fn gpus() -> Vec<Gpu> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
    let mut out = Vec::new();
    // SAFETY: plain COM calls; the factory and adapters are released when dropped.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        for i in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let len = desc.Description.iter().position(|c| *c == 0).unwrap_or(desc.Description.len());
            out.push(Gpu {
                index: i as i32,
                name: String::from_utf16_lossy(&desc.Description[..len]).trim().to_owned(),
                memory: desc.DedicatedVideoMemory as u64,
            });
        }
    }
    out
}

#[cfg(not(windows))]
pub fn gpus() -> Vec<Gpu> {
    Vec::new()
}

/// The adapter with the most dedicated memory (the discrete GPU on a hybrid laptop).
pub fn pick(gpus: &[Gpu]) -> Option<&Gpu> {
    gpus.iter().max_by_key(|g| (g.memory, -g.index))
}

/// The GPU DirectML runs on: `PL_DML_DEVICE` (an adapter index), else [`pick`]'s, else
/// the first adapter. Looked up once.
pub fn chosen() -> &'static Gpu {
    static CHOSEN: OnceLock<Gpu> = OnceLock::new();
    CHOSEN.get_or_init(|| {
        let all = gpus();
        let forced = std::env::var("PL_DML_DEVICE").ok().and_then(|v| v.parse::<i32>().ok());
        let gpu = match forced {
            Some(i) => all.iter().find(|g| g.index == i).cloned().unwrap_or(Gpu {
                index: i,
                name: format!("adapter {i}"),
                memory: 0,
            }),
            None => pick(&all).cloned().unwrap_or(Gpu { index: 0, name: "GPU".into(), memory: 0 }),
        };
        log::info!(
            "AI: DirectML on {} ({} MB); adapters: {}",
            gpu.name,
            gpu.memory >> 20,
            all.iter()
                .map(|g| format!("{} {} ({} MB)", g.index, g.name, g.memory >> 20))
                .collect::<Vec<_>>()
                .join(", ")
        );
        gpu
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_discrete_gpu_is_picked() {
        let g = |index, name: &str, mb: u64| Gpu { index, name: name.into(), memory: mb << 20 };
        let laptop = [g(0, "Intel UHD", 128), g(1, "NVIDIA T550", 4096)];
        assert_eq!(pick(&laptop).map(|g| g.index), Some(1));
        // Equal memory: the first adapter.
        assert_eq!(pick(&[g(0, "A", 0), g(1, "B", 0)]).map(|g| g.index), Some(0));
        assert!(pick(&[]).is_none());
        // On this machine: listing works (whatever the adapters).
        let _ = gpus();
    }
}
