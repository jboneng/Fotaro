//! GPU texture cache for thumbnails and previews, with LRU eviction and
//! request tracking so each image is asked for once while it stays on screen.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use eframe::egui;

/// Re-request an image if nothing arrived after this long (e.g. it was dropped
/// from the capped visible queue while scrolled off-screen).
const RETRY_AFTER: Duration = Duration::from_secs(3);

struct Entry {
    texture: egui::TextureHandle,
    last_used: u64,
}

pub struct TextureCache {
    name: &'static str,
    capacity: usize,
    entries: HashMap<i64, Entry>,
    requested: HashMap<i64, Instant>,
    failed: HashSet<i64>,
    frame: u64,
}

impl TextureCache {
    pub fn new(name: &'static str, capacity: usize) -> Self {
        TextureCache {
            name,
            capacity,
            entries: HashMap::new(),
            requested: HashMap::new(),
            failed: HashSet::new(),
            frame: 0,
        }
    }

    /// Call once per frame before drawing.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub fn get(&mut self, id: i64) -> Option<egui::TextureHandle> {
        let frame = self.frame;
        self.entries.get_mut(&id).map(|e| {
            e.last_used = frame;
            e.texture.clone()
        })
    }

    pub fn is_failed(&self, id: i64) -> bool {
        self.failed.contains(&id)
    }

    /// True if the caller should submit a load job for `id` now.
    pub fn should_request(&mut self, id: i64) -> bool {
        if self.entries.contains_key(&id) || self.failed.contains(&id) {
            return false;
        }
        let now = Instant::now();
        match self.requested.get(&id) {
            Some(t) if now.duration_since(*t) < RETRY_AFTER => false,
            _ => {
                self.requested.insert(id, now);
                true
            }
        }
    }

    pub fn insert(&mut self, ctx: &egui::Context, id: i64, image: egui::ColorImage) {
        self.requested.remove(&id);
        self.failed.remove(&id);
        let texture = ctx.load_texture(format!("{}-{id}", self.name), image, egui::TextureOptions::LINEAR);
        self.entries.insert(id, Entry { texture, last_used: self.frame });
        self.evict();
    }

    pub fn mark_failed(&mut self, id: i64) {
        self.requested.remove(&id);
        self.failed.insert(id);
    }

    /// Forget an image so it is reloaded (e.g. its file changed).
    pub fn invalidate(&mut self, id: i64) {
        self.entries.remove(&id);
        self.requested.remove(&id);
        self.failed.remove(&id);
    }

    fn evict(&mut self) {
        if self.entries.len() <= self.capacity {
            return;
        }
        let mut by_age: Vec<(u64, i64)> = self.entries.iter().map(|(id, e)| (e.last_used, *id)).collect();
        by_age.sort_unstable();
        let excess = self.entries.len() - self.capacity;
        for (last_used, id) in by_age.into_iter().take(excess) {
            if last_used == self.frame {
                break; // never evict something drawn this frame
            }
            self.entries.remove(&id);
        }
    }
}
