//! Process-global cache of *decoded* image buffers, keyed by the same
//! [`ResourceId`](crate::resource) handle a media file registers under (item 5,
//! option 2 of `docs/cards/editor-resource-model.md`).
//!
//! A native `Compute` card used to hand its result to the next card as a *path*
//! only: the producer encoded + wrote a PNG, and every downstream compute card
//! re-opened that PNG and decoded it again from scratch. For the `crop →
//! subjectMask (→ matte)` chain on a full-resolution photo that re-decode is
//! the dominant per-node cost and is pure waste — the bytes were live in the
//! producer moments earlier.
//!
//! This module removes the *downstream* half of that round-trip. When a compute
//! card writes an output PNG it also **publishes** the decoded surface here,
//! keyed by the output path's `ResourceId`. The shared loaders
//! ([`super::studio_image::load_rgba`] / [`load_mask`]) consult the cache first
//! and, on a fresh hit, return the in-memory buffer instead of re-reading and
//! re-decoding the file. The PNG is still written (the frontend preview, the
//! Python-bridge cards, and PSD export all read it from disk), so this is a
//! transparent optimisation, not a new contract: a miss always falls back to
//! the identical disk decode.
//!
//! **Correctness.** An entry is only served when the file on disk still matches
//! the `(mtime, len)` captured at publish time, so an edited / replaced output
//! invalidates its own entry (mirroring the thumbnail LRU's freshness key). A
//! cached surface is also re-checked against the caller's decode budget, so a
//! tighter `--max-decode-pixels` still rejects an oversized buffer exactly as a
//! disk decode would. Originals a user drags in are never published, so they
//! always decode from disk. Like the ONNX warm pool and the thumbnail LRU this
//! is a plain process-global `static` guarded by a `Mutex`, bounded to a small
//! entry count so a long session cannot grow it without limit.
//!
//! [`load_mask`]: super::studio_image::load_mask

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use image::{DynamicImage, GrayImage, RgbaImage};

use super::studio_image::{LoadMeta, LoadedRgba};

/// A decoded surface kept resident. RGBA carries its [`LoadMeta`] so a hit
/// reproduces the same provenance a disk decode would report; a single-channel
/// mask has no surfaced provenance (mirroring [`super::studio_image::load_mask`]).
#[derive(Clone)]
enum DecodedImage {
    Rgba {
        image: Arc<RgbaImage>,
        meta: LoadMeta,
    },
    Gray(Arc<GrayImage>),
}

/// A published buffer plus the disk `(mtime, len)` it was captured with, so a
/// later on-disk change invalidates it.
#[derive(Clone)]
struct Entry {
    image: DecodedImage,
    mtime: Option<SystemTime>,
    len: u64,
}

/// Fixed-capacity LRU from `ResourceId` -> published buffer. `order` lists ids
/// oldest-first; the back is most-recently used (same shape as
/// [`super::super::thumb_cache`]'s LRU).
struct Lru {
    capacity: usize,
    entries: HashMap<String, Entry>,
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

    fn get(&mut self, key: &str) -> Option<Entry> {
        if self.entries.contains_key(key) {
            self.touch(key);
            self.entries.get(key).cloned()
        } else {
            None
        }
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
    }

    fn insert(&mut self, key: String, value: Entry) {
        self.entries.insert(key.clone(), value);
        self.touch(&key);
        while self.entries.len() > self.capacity {
            let oldest = self.order.remove(0);
            self.entries.remove(&oldest);
        }
    }
}

/// How many decoded surfaces to keep resident. These are full-resolution
/// buffers (megabytes each), so the cap is small — the compute chain only needs
/// the handful of most-recent producer outputs, and older ones fall back to a
/// disk decode.
const CAPACITY: usize = 8;

static CACHE: OnceLock<Mutex<Lru>> = OnceLock::new();

fn cache() -> &'static Mutex<Lru> {
    CACHE.get_or_init(|| Mutex::new(Lru::new(CAPACITY)))
}

/// Resolve `path` to its stable cache key plus the current disk `(mtime, len)`.
/// Returns `None` when the file cannot be resolved (missing / unreadable), so
/// an unresolvable path is simply never cached.
fn key_for(path: &Path) -> Option<(String, Option<SystemTime>, u64)> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::metadata(&canonical).ok()?;
    let id = crate::resource::id_for(&canonical.to_string_lossy());
    Some((id, meta.modified().ok(), meta.len()))
}

/// Whether `width * height` overflows a non-zero decode budget (`0` disables
/// the guard, matching [`super::studio_image`]).
fn exceeds_budget(width: u32, height: u32, max_pixels: u64) -> bool {
    max_pixels != 0 && u64::from(width) * u64::from(height) > max_pixels
}

/// Publish a freshly-written RGBA output so the next compute card that loads
/// this path skips the decode. `meta` is the provenance a reload of the written
/// PNG would report (see [`super::studio_image::png_output_meta`]).
pub(crate) fn publish_rgba(path: &Path, image: &RgbaImage, meta: LoadMeta) {
    if let Some((id, mtime, len)) = key_for(path) {
        store(
            id,
            Entry {
                image: DecodedImage::Rgba {
                    image: Arc::new(image.clone()),
                    meta,
                },
                mtime,
                len,
            },
        );
    }
}

/// Publish a freshly-written single-channel output (mask / trimap).
pub(crate) fn publish_gray(path: &Path, image: &GrayImage) {
    if let Some((id, mtime, len)) = key_for(path) {
        store(
            id,
            Entry {
                image: DecodedImage::Gray(Arc::new(image.clone())),
                mtime,
                len,
            },
        );
    }
}

fn store(id: String, entry: Entry) {
    if let Ok(mut lru) = cache().lock() {
        lru.insert(id, entry);
    }
}

/// Look up a published RGBA surface for `path`, or `None` (a miss) when it was
/// never published, went stale on disk, exceeds `max_pixels`, or is a
/// single-channel entry. A stale entry is dropped so it stops shadowing the
/// file. A hit returns an owned clone, exactly like a disk decode.
pub(crate) fn lookup_rgba(path: &Path, max_pixels: u64) -> Option<LoadedRgba> {
    let entry = fetch_fresh(path)?;
    match entry.image {
        DecodedImage::Rgba { image, meta } => {
            if exceeds_budget(image.width(), image.height(), max_pixels) {
                return None;
            }
            Some(LoadedRgba {
                image: (*image).clone(),
                meta,
            })
        }
        DecodedImage::Gray(_) => None,
    }
}

/// Look up a published single-channel surface for `path`. Mirrors
/// [`lookup_rgba`] for the mask loaders.
pub(crate) fn lookup_gray(path: &Path, max_pixels: u64) -> Option<GrayImage> {
    let entry = fetch_fresh(path)?;
    match entry.image {
        DecodedImage::Gray(image) => {
            if exceeds_budget(image.width(), image.height(), max_pixels) {
                return None;
            }
            Some((*image).clone())
        }
        DecodedImage::Rgba { .. } => None,
    }
}

/// Fetch a published surface for `path` as a [`DynamicImage`] (RGBA or luma),
/// or `None` on a miss / stale entry. Unlike [`lookup_rgba`] there is no decode
/// budget: the only caller is the thumbnail path, which always downsamples, so
/// resizing even a large surface is bounded and cheaper than a PNG re-decode.
/// This is the display half of the buffer handoff — a compute card's output
/// thumbnail is produced from the buffer the card already decoded.
pub(crate) fn lookup_dynamic(path: &Path) -> Option<DynamicImage> {
    let entry = fetch_fresh(path)?;
    Some(match entry.image {
        DecodedImage::Rgba { image, .. } => DynamicImage::ImageRgba8((*image).clone()),
        DecodedImage::Gray(image) => DynamicImage::ImageLuma8((*image).clone()),
    })
}

/// Fetch the entry for `path` only when it is still fresh against disk; a stale
/// entry is evicted so it stops shadowing the file. An unresolvable /
/// unregistered path yields `None`.
fn fetch_fresh(path: &Path) -> Option<Entry> {
    let (id, mtime, len) = key_for(path)?;
    let mut lru = cache().lock().ok()?;
    match lru.get(&id) {
        Some(entry) if entry.mtime == mtime && entry.len == len => Some(entry),
        Some(_) => {
            lru.remove(&id);
            None
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba};
    use std::path::PathBuf;

    fn unique_tmp(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hgripe_image_buffer_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn rgba_meta() -> LoadMeta {
        LoadMeta {
            source_mode: "RGBA".to_string(),
            exif_transposed: false,
        }
    }

    #[test]
    fn a_hit_is_served_from_memory_not_disk() {
        let path = unique_tmp("rgba.png");
        // On disk: red. Never published yet -> miss.
        RgbaImage::from_pixel(4, 3, Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        assert!(lookup_rgba(&path, 0).is_none());

        // Publish a *green* buffer for the same (unchanged) file. A hit that
        // returns green proves the pixels came from the cache, not the red PNG.
        let published = RgbaImage::from_pixel(4, 3, Rgba([0, 255, 0, 255]));
        publish_rgba(&path, &published, rgba_meta());
        let hit = lookup_rgba(&path, 0).expect("published buffer is a hit");
        assert_eq!(hit.image.dimensions(), (4, 3));
        assert_eq!(hit.image.get_pixel(0, 0).0, [0, 255, 0, 255]);
        assert_eq!(hit.meta.source_mode, "RGBA");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lookup_dynamic_returns_the_published_surface_by_kind() {
        // RGBA publish -> ImageRgba8 carrying the published pixels.
        let rgba_path = unique_tmp("dyn_rgba.png");
        RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 4]))
            .save(&rgba_path)
            .unwrap();
        publish_rgba(
            &rgba_path,
            &RgbaImage::from_pixel(3, 2, Rgba([9, 8, 7, 255])),
            rgba_meta(),
        );
        match lookup_dynamic(&rgba_path).expect("rgba hit") {
            DynamicImage::ImageRgba8(img) => {
                assert_eq!(img.dimensions(), (3, 2));
                assert_eq!(img.get_pixel(0, 0).0, [9, 8, 7, 255]);
            }
            other => panic!("expected ImageRgba8, got {other:?}"),
        }

        // Gray publish -> ImageLuma8.
        let gray_path = unique_tmp("dyn_gray.png");
        GrayImage::from_pixel(2, 2, Luma([5])).save(&gray_path).unwrap();
        publish_gray(&gray_path, &GrayImage::from_pixel(2, 2, Luma([222])));
        match lookup_dynamic(&gray_path).expect("gray hit") {
            DynamicImage::ImageLuma8(img) => {
                assert_eq!(img.get_pixel(0, 0).0[0], 222);
            }
            other => panic!("expected ImageLuma8, got {other:?}"),
        }

        // A path that was never published is a miss.
        let missing = unique_tmp("dyn_missing.png");
        GrayImage::from_pixel(1, 1, Luma([0])).save(&missing).unwrap();
        assert!(lookup_dynamic(&missing).is_none());

        let _ = std::fs::remove_file(&rgba_path);
        let _ = std::fs::remove_file(&gray_path);
        let _ = std::fs::remove_file(&missing);
    }

    #[test]
    fn a_disk_change_invalidates_the_entry() {
        let path = unique_tmp("stale.png");
        let img = RgbaImage::from_pixel(2, 2, Rgba([9, 9, 9, 255]));
        img.save(&path).unwrap();
        publish_rgba(&path, &img, rgba_meta());
        assert!(lookup_rgba(&path, 0).is_some());

        // Rewrite the file with a different length so (mtime, len) no longer
        // matches what was captured at publish.
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        assert!(
            lookup_rgba(&path, 0).is_none(),
            "an edited file must not be served from the cache"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_tighter_budget_falls_back_to_disk() {
        let path = unique_tmp("budget.png");
        let img = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255]));
        img.save(&path).unwrap();
        publish_rgba(&path, &img, rgba_meta());
        // 64 px buffer, 1 px budget -> miss (the disk path would reject it too).
        assert!(lookup_rgba(&path, 1).is_none());
        // A budget that fits still hits.
        assert!(lookup_rgba(&path, 64).is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_kinds_do_not_cross_serve() {
        let rgba_path = unique_tmp("kind_rgba.png");
        let img = RgbaImage::from_pixel(2, 2, Rgba([5, 5, 5, 255]));
        img.save(&rgba_path).unwrap();
        publish_rgba(&rgba_path, &img, rgba_meta());
        // An RGBA entry is not a mask hit.
        assert!(lookup_gray(&rgba_path, 0).is_none());

        let gray_path = unique_tmp("kind_gray.png");
        let mask = GrayImage::from_pixel(2, 2, Luma([200]));
        mask.save(&gray_path).unwrap();
        publish_gray(&gray_path, &mask);
        // A mask entry is not an RGBA hit, but is a mask hit.
        assert!(lookup_rgba(&gray_path, 0).is_none());
        let hit = lookup_gray(&gray_path, 0).expect("mask hit");
        assert_eq!(hit.get_pixel(0, 0).0, [200]);
        let _ = std::fs::remove_file(&rgba_path);
        let _ = std::fs::remove_file(&gray_path);
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut lru = Lru::new(2);
        let entry = |v: u8| Entry {
            image: DecodedImage::Gray(Arc::new(GrayImage::from_pixel(1, 1, Luma([v])))),
            mtime: None,
            len: 0,
        };
        lru.insert("a".to_string(), entry(1));
        lru.insert("b".to_string(), entry(2));
        assert!(lru.get("a").is_some()); // a is now most-recent
        lru.insert("c".to_string(), entry(3)); // evicts b
        assert!(lru.get("b").is_none());
        assert!(lru.get("a").is_some());
        assert!(lru.get("c").is_some());
        assert_eq!(lru.entries.len(), 2);
    }
}
