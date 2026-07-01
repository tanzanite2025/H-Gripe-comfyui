//! Bounded LRU cache of decoded video frames (`Media` lane, step 5 of
//! `docs/cards/editor-resource-model.md`).
//!
//! The clip editor scrubs back and forth over the same handful of timestamps,
//! so the media engine keeps a small ring of recently decoded frames instead of
//! asking the decoder for them again. Frames are decoded to PNGs on disk (the
//! same still the video card already renders through the thumbnail pipeline), so
//! the cache maps a **quantised timestamp** (milliseconds, so equal seeks hit
//! the same slot) to that frame's on-disk path and evicts least-recently-used
//! when it grows past `capacity`.
//!
//! This type is deliberately pure and decoder-agnostic — it holds no file
//! handles and does no I/O — so it is unit-testable without ffmpeg and is reused
//! unchanged when a native-Rust ffmpeg decoder replaces the PyAV worker behind
//! the [`super::video_engine`] seam. Eviction *returns* the dropped path rather
//! than deleting it, leaving the file lifecycle to the owner (posters live in a
//! project cache dir that is cleared wholesale).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Quantise a timestamp in seconds to the cache's integer-millisecond key, so
/// two seeks to the "same" time collapse to one slot. Negative/NaN clamp to 0.
pub(crate) fn frame_key(timestamp_sec: f64) -> i64 {
    if !(timestamp_sec > 0.0) {
        // Covers <= 0 and NaN (NaN fails every comparison).
        return 0;
    }
    // Round ties to even so a timestamp landing exactly on a half-millisecond
    // (e.g. 1.2345 s -> 1234.5 ms) is deterministic and matches the documented
    // quantisation; plain `round()` breaks ties away from zero (1235).
    (timestamp_sec * 1000.0).round_ties_even() as i64
}

/// A fixed-capacity LRU map from frame key -> decoded poster path.
///
/// `order` lists keys oldest-first; the back is the most recently used. Both
/// `get` and `insert` mark their key most-recent, and `insert` past capacity
/// drops the front (oldest) entry and returns its path.
pub(crate) struct FrameCache {
    capacity: usize,
    entries: HashMap<i64, PathBuf>,
    order: Vec<i64>,
}

impl FrameCache {
    /// Create a cache holding at most `capacity` frames. A `capacity` of 0 is
    /// bumped to 1 so the cache always holds the frame just decoded.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Move `key` to the most-recently-used position (back of `order`).
    fn touch(&mut self, key: i64) {
        if let Some(pos) = self.order.iter().position(|&k| k == key) {
            self.order.remove(pos);
        }
        self.order.push(key);
    }

    /// Look up a frame, marking it most-recently-used on a hit.
    pub(crate) fn get(&mut self, key: i64) -> Option<&Path> {
        if self.entries.contains_key(&key) {
            self.touch(key);
            self.entries.get(&key).map(PathBuf::as_path)
        } else {
            None
        }
    }

    /// Insert (or update) a frame, marking it most-recently-used. If this grows
    /// the cache past `capacity`, the least-recently-used entry is evicted and
    /// its path returned so the caller can reclaim the file if it wants.
    pub(crate) fn insert(&mut self, key: i64, path: PathBuf) -> Option<PathBuf> {
        self.entries.insert(key, path);
        self.touch(key);
        if self.entries.len() > self.capacity {
            // The oldest live key is the first `order` entry still present.
            let oldest = self.order.remove(0);
            return self.entries.remove(&oldest);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_key_quantises_to_milliseconds() {
        assert_eq!(frame_key(1.2345), 1234);
        assert_eq!(frame_key(1.2346), 1235);
        assert_eq!(frame_key(0.0), 0);
    }

    #[test]
    fn frame_key_clamps_negative_and_nan_to_zero() {
        assert_eq!(frame_key(-5.0), 0);
        assert_eq!(frame_key(f64::NAN), 0);
    }

    #[test]
    fn capacity_is_at_least_one() {
        assert_eq!(FrameCache::new(0).capacity(), 1);
    }

    #[test]
    fn get_misses_then_hits_after_insert() {
        let mut cache = FrameCache::new(4);
        assert!(cache.get(100).is_none());
        assert!(cache.insert(100, PathBuf::from("/f/100.png")).is_none());
        assert_eq!(cache.get(100), Some(Path::new("/f/100.png")));
    }

    #[test]
    fn eviction_drops_least_recently_used() {
        let mut cache = FrameCache::new(2);
        cache.insert(1, PathBuf::from("/f/1.png"));
        cache.insert(2, PathBuf::from("/f/2.png"));
        // Touch 1 so 2 becomes the least-recently-used.
        assert_eq!(cache.get(1), Some(Path::new("/f/1.png")));
        let evicted = cache.insert(3, PathBuf::from("/f/3.png"));
        assert_eq!(evicted, Some(PathBuf::from("/f/2.png")));
        assert!(cache.get(2).is_none());
        assert_eq!(cache.get(1), Some(Path::new("/f/1.png")));
        assert_eq!(cache.get(3), Some(Path::new("/f/3.png")));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn reinserting_a_key_updates_without_growing() {
        let mut cache = FrameCache::new(2);
        cache.insert(1, PathBuf::from("/f/1.png"));
        let evicted = cache.insert(1, PathBuf::from("/f/1b.png"));
        assert!(evicted.is_none());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(1), Some(Path::new("/f/1b.png")));
    }
}
