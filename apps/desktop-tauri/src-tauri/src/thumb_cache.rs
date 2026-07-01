//! Process-global LRU of recently generated image thumbnails.
//!
//! `generate_thumbnail` already caches encoded PNGs on disk, but every call
//! still `fs::read`s the *whole* source file to hash it — for a 4K/8K source
//! that read (and, on a miss, the full decode) is the actual cost. When the
//! canvas re-renders the same media card (scroll, pan, re-mount) it asks for
//! the same `(path, size)` again and again, so this keeps the finished
//! [`CachedThumb`] in memory: a hit returns the ready `data:` URL and
//! dimensions without touching the disk at all.
//!
//! The cache is keyed by `path + target-size + the source's mtime/len`, so an
//! edited or replaced source invalidates its own entry. Like [`super::studio`]'s
//! ONNX warm pool it is a plain process-global `static` (the `generate_thumbnail`
//! command is handle-free) guarded by a `Mutex`, and it is bounded to a fixed
//! entry count so long sessions cannot grow it without limit.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A finished thumbnail kept in memory, mirroring the fields the
/// `generate_thumbnail` command returns to the webview.
#[derive(Clone)]
pub(crate) struct CachedThumb {
    pub data_url: String,
    pub cache_path: String,
    pub width: u32,
    pub height: u32,
    pub source_hash: String,
    pub mime: String,
}

/// Fixed-capacity LRU map from cache key -> finished thumbnail.
///
/// `order` lists keys oldest-first; the back is most-recently used. Both `get`
/// and `insert` mark their key most-recent, and inserting past `capacity` drops
/// the front (oldest) entry.
struct Lru {
    capacity: usize,
    entries: HashMap<String, CachedThumb>,
    order: Vec<String>,
}

impl Lru {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push(key.to_string());
    }

    fn get(&mut self, key: &str) -> Option<CachedThumb> {
        if self.entries.contains_key(key) {
            self.touch(key);
            self.entries.get(key).cloned()
        } else {
            None
        }
    }

    fn insert(&mut self, key: String, value: CachedThumb) {
        self.entries.insert(key.clone(), value);
        self.touch(&key);
        while self.entries.len() > self.capacity {
            let oldest = self.order.remove(0);
            self.entries.remove(&oldest);
        }
    }
}

/// How many finished thumbnails to keep resident. Thumbnails are small (a
/// 256px PNG data URL is tens of KB), so a few dozen covers a busy canvas while
/// staying well bounded.
const CAPACITY: usize = 96;

static CACHE: OnceLock<Mutex<Lru>> = OnceLock::new();

fn cache() -> &'static Mutex<Lru> {
    CACHE.get_or_init(|| Mutex::new(Lru::new(CAPACITY)))
}

/// Fetch a cached thumbnail, marking it most-recently-used on a hit.
pub(crate) fn get(key: &str) -> Option<CachedThumb> {
    cache().lock().ok()?.get(key)
}

/// Store a finished thumbnail, evicting the least-recently-used entry if full.
pub(crate) fn put(key: String, value: CachedThumb) {
    if let Ok(mut lru) = cache().lock() {
        lru.insert(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(tag: &str) -> CachedThumb {
        CachedThumb {
            data_url: format!("data:image/png;base64,{tag}"),
            cache_path: format!("/cache/{tag}.png"),
            width: 256,
            height: 256,
            source_hash: tag.to_string(),
            mime: "image/png".to_string(),
        }
    }

    #[test]
    fn miss_then_hit_after_insert() {
        let mut lru = Lru::new(4);
        assert!(lru.get("a").is_none());
        lru.insert("a".to_string(), thumb("a"));
        assert_eq!(lru.get("a").unwrap().source_hash, "a");
    }

    #[test]
    fn eviction_drops_least_recently_used() {
        let mut lru = Lru::new(2);
        lru.insert("1".to_string(), thumb("1"));
        lru.insert("2".to_string(), thumb("2"));
        // Touch 1 so 2 becomes least-recently-used.
        assert!(lru.get("1").is_some());
        lru.insert("3".to_string(), thumb("3"));
        assert!(lru.get("2").is_none());
        assert!(lru.get("1").is_some());
        assert!(lru.get("3").is_some());
        assert_eq!(lru.entries.len(), 2);
    }

    #[test]
    fn reinserting_a_key_updates_without_growing() {
        let mut lru = Lru::new(2);
        lru.insert("k".to_string(), thumb("v1"));
        lru.insert("k".to_string(), thumb("v2"));
        assert_eq!(lru.entries.len(), 1);
        assert_eq!(lru.get("k").unwrap().source_hash, "v2");
    }

    #[test]
    fn capacity_is_at_least_one() {
        let lru = Lru::new(0);
        assert_eq!(lru.capacity, 1);
    }
}
