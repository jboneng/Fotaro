//! What a render can keep for the next one. Dragging a slider renders the same photo over
//! and over with one setting changed, and most of a render's stages don't depend on that
//! setting: the masks' values, the brightness maps Highlights / Shadows, Clarity and
//! Texture work from, the gains they give, the photo developed up to Detail. Each is kept
//! with the image it was made from, under a key of just what it depends on, and made again
//! only when that changes.
//!
//! One entry per kind: a slider drag alternates nothing, and a full-size image's regions
//! (which change as the view pans) simply replace one another.

use std::sync::{Arc, Mutex};

use super::dehaze::HazeMap;
use super::local::BaseMap;
use super::masks::{Component, MaskField};
use super::pipeline::Region;
use super::settings::DevelopSettings;

/// One kept value and the key it was made for.
#[derive(Debug)]
pub(super) struct Slot<K, V>(Mutex<Option<(K, Arc<V>)>>);

impl<K, V> Default for Slot<K, V> {
    fn default() -> Self {
        Slot(Mutex::new(None))
    }
}

impl<K: PartialEq, V> Slot<K, V> {
    /// The kept value, if it was made for `key`.
    pub(super) fn get(&self, key: &K) -> Option<Arc<V>> {
        let kept = self.0.lock().unwrap_or_else(|e| e.into_inner());
        kept.as_ref().filter(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// Keep `value` for `key` (in place of what was kept).
    pub(super) fn put(&self, key: K, value: Arc<V>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, value));
    }

    /// The value for `key`: the kept one, or `make`'s (kept from now on). Two threads
    /// asking at once may both make it; either result is right.
    pub(super) fn get_or(&self, key: K, make: impl FnOnce() -> V) -> Arc<V> {
        if let Some(v) = self.get(&key) {
            return v;
        }
        let v = Arc::new(make());
        self.put(key, v.clone());
        v
    }
}

/// White balance multipliers, as a key (bit for bit).
pub(super) type WbKey = [u32; 3];

/// The masks a field was made for: each active mask's index, amount and shape.
pub(super) type FieldKey = (Region, Vec<(usize, u32, Vec<Component>)>);

/// What the gains of a region depend on.
pub(super) struct GainsKey {
    pub wb: WbKey,
    pub region: Region,
    /// Exposure gain, Highlights, Shadows, Clarity, Texture.
    pub amounts: [u32; 5],
    /// The masks' values (this very field), and each mask's Clarity and Texture.
    pub field: Option<Arc<MaskField>>,
    pub local: Vec<[u32; 2]>,
}

impl PartialEq for GainsKey {
    fn eq(&self, o: &Self) -> bool {
        let same_field = match (&self.field, &o.field) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_field
            && self.wb == o.wb
            && self.region == o.region
            && self.amounts == o.amounts
            && self.local == o.local
    }
}

/// An image's kept render stages.
#[derive(Default)]
pub(super) struct RenderCache {
    pub field: Slot<FieldKey, MaskField>,
    /// Every pixel's luminance (whole-image renders).
    pub luminance: Slot<WbKey, Vec<f32>>,
    /// The Highlights / Shadows and Clarity brightness maps.
    pub tone_map: Slot<WbKey, BaseMap>,
    pub clarity_map: Slot<WbKey, BaseMap>,
    /// log2 luminance over a region with Texture's margin (left, top, width, height), and
    /// its blur.
    pub log_luminance: Slot<(WbKey, [usize; 4]), Vec<f32>>,
    pub texture: Slot<(WbKey, [usize; 4]), Vec<f32>>,
    pub haze: Slot<WbKey, HazeMap>,
    pub gains: Slot<GainsKey, Vec<f32>>,
    /// The area developed up to Detail, as Oklab planes: the settings without Detail (and
    /// grain, which comes after), and the area.
    pub planes: Slot<(DevelopSettings, Region), [Vec<f32>; 3]>,
}

impl std::fmt::Debug for RenderCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RenderCache")
    }
}

impl Clone for RenderCache {
    /// A copy starts empty (it is only a cache).
    fn clone(&self) -> Self {
        RenderCache::default()
    }
}
