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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use image::{DynamicImage, GrayImage, RgbaImage};

use super::studio_image::{LoadMeta, LoadedRgba, LoadedWorking};
use super::working_image::WorkingImage;

/// A decoded surface kept resident. RGBA / [`WorkingImage`] carry their
/// [`LoadMeta`] so a hit reproduces the same provenance a disk decode would
/// report; a single-channel mask has no surfaced provenance (mirroring
/// [`super::studio_image::load_mask`]).
#[derive(Clone)]
enum DecodedImage {
    Rgba {
        image: Arc<RgbaImage>,
        meta: LoadMeta,
    },
    /// A 16-bit canonical [`WorkingImage`] (space-tagged, ICC-carrying). The
    /// manual chain publishes this so wide-gamut pixels survive card-to-card
    /// without a lossy 8-bit round-trip; 8-bit consumers ([`lookup_rgba`],
    /// [`lookup_dynamic`], disk materialisation) get [`WorkingImage::to_srgb_rgba8`]
    /// egress so their contract is unchanged.
    Working {
        image: Arc<WorkingImage>,
        meta: LoadMeta,
    },
    Gray(Arc<GrayImage>),
}

/// How a published buffer is validated against the outside world.
#[derive(Clone)]
enum Freshness {
    /// File-backed: the producer wrote a PNG, so the buffer is only served
    /// while the file's `(mtime, len)` still match what was captured at publish
    /// time (an edited / replaced output invalidates its own entry).
    File {
        mtime: Option<SystemTime>,
        len: u64,
    },
    /// Deferred: the producer *skipped* the PNG write (its output is consumed
    /// only by in-process compute cards), so there is no file to check against.
    /// The entry is served unconditionally, and if it is ever evicted it is
    /// first **materialised** — written to `path` — so any later reader that
    /// only knows the file (a thumbnail fallback, a disk decode) still resolves.
    Deferred { path: PathBuf },
}

/// A published buffer plus how to validate / persist it (see [`Freshness`]).
#[derive(Clone)]
struct Entry {
    image: DecodedImage,
    freshness: Freshness,
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

    /// Whether `key` is currently resident. A read-only probe that does not
    /// touch the LRU order (used by [`is_available`] to answer "does this
    /// output resolve?" without cloning the surface or promoting it).
    fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Insert `value`, returning any entries dropped to stay within capacity so
    /// the caller can materialise deferred surfaces *after* releasing the lock
    /// (an overwrite of the same key is not an eviction and is not returned —
    /// its replacement carries the same, more recent, pixels).
    fn insert(&mut self, key: String, value: Entry) -> Vec<Entry> {
        self.entries.insert(key.clone(), value);
        self.touch(&key);
        let mut evicted = Vec::new();
        while self.entries.len() > self.capacity {
            let oldest = self.order.remove(0);
            if let Some(entry) = self.entries.remove(&oldest) {
                evicted.push(entry);
            }
        }
        evicted
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
/// an unresolvable path is simply never cached under a file-backed key.
fn key_for(path: &Path) -> Option<(String, Option<SystemTime>, u64)> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::metadata(&canonical).ok()?;
    let id = crate::resource::id_for(&canonical.to_string_lossy());
    Some((id, meta.modified().ok(), meta.len()))
}

/// Resolve the cache key for a *lookup*, along with the current disk
/// `(mtime, len)` when the file exists. Unlike [`key_for`] this always yields a
/// key: when the file is present it uses the canonical-path id (matching the
/// file-backed publish); when it is absent it falls back to the id of the raw
/// path string, which is exactly what a deferred publish keyed itself under.
/// The producer emits that same string as the output value, so a downstream
/// consumer re-derives the identical id even though no file was ever written.
fn key_for_lookup(path: &Path) -> (String, Option<(Option<SystemTime>, u64)>) {
    match key_for(path) {
        Some((id, mtime, len)) => (id, Some((mtime, len))),
        None => (
            crate::resource::id_for(&path.to_string_lossy()),
            None,
        ),
    }
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
                freshness: Freshness::File { mtime, len },
            },
        );
    }
}

/// Publish an RGBA surface whose PNG was **not** written to disk (the producer
/// skipped the write because every consumer of this output is an in-process
/// compute card). The entry is keyed by the raw output path so a downstream
/// loader that receives that path as its input resolves it from memory, and it
/// is materialised to that path if it is ever evicted (so a later thumbnail or
/// disk decode still works). This is the producer half of the write-skip
/// (item 5, option 2): the buffer, not the file, is the source of truth while
/// it stays resident. Callers must only use this when the file does not already
/// exist, so the on-disk state can never go stale behind the buffer.
pub(crate) fn publish_rgba_deferred(path: &Path, image: &RgbaImage, meta: LoadMeta) {
    let id = crate::resource::id_for(&path.to_string_lossy());
    store(
        id,
        Entry {
            image: DecodedImage::Rgba {
                image: Arc::new(image.clone()),
                meta,
            },
            freshness: Freshness::Deferred {
                path: path.to_path_buf(),
            },
        },
    );
}

/// Publish a freshly-written 16-bit [`WorkingImage`] output so the next manual
/// card that loads this path via [`super::studio_image::load_working`] gets the
/// wide-gamut surface straight from memory (no 8-bit round-trip). 8-bit readers
/// of the same path still get the egressed sRGB surface. `meta` is the
/// provenance a reload would report.
pub(crate) fn publish_working(path: &Path, image: &WorkingImage, meta: LoadMeta) {
    if let Some((id, mtime, len)) = key_for(path) {
        store(
            id,
            Entry {
                image: DecodedImage::Working {
                    image: Arc::new(image.clone()),
                    meta,
                },
                freshness: Freshness::File { mtime, len },
            },
        );
    }
}

/// Publish a 16-bit [`WorkingImage`] whose file was **not** written (the
/// write-skip analogue of [`publish_rgba_deferred`] for the manual chain).
/// Keyed by the raw output path and materialised (egressed to 8-bit) on
/// eviction; callers must only use this when the file does not already exist.
pub(crate) fn publish_working_deferred(path: &Path, image: &WorkingImage, meta: LoadMeta) {
    let id = crate::resource::id_for(&path.to_string_lossy());
    store(
        id,
        Entry {
            image: DecodedImage::Working {
                image: Arc::new(image.clone()),
                meta,
            },
            freshness: Freshness::Deferred {
                path: path.to_path_buf(),
            },
        },
    );
}

/// Publish a freshly-written single-channel output (mask / trimap).
pub(crate) fn publish_gray(path: &Path, image: &GrayImage) {
    if let Some((id, mtime, len)) = key_for(path) {
        store(
            id,
            Entry {
                image: DecodedImage::Gray(Arc::new(image.clone())),
                freshness: Freshness::File { mtime, len },
            },
        );
    }
}

/// Publish a single-channel surface whose PNG was **not** written (the mask /
/// trimap analogue of [`publish_rgba_deferred`]). Keyed by the raw output path
/// and materialised to it on eviction; callers must only use this when the file
/// does not already exist.
pub(crate) fn publish_gray_deferred(path: &Path, image: &GrayImage) {
    let id = crate::resource::id_for(&path.to_string_lossy());
    store(
        id,
        Entry {
            image: DecodedImage::Gray(Arc::new(image.clone())),
            freshness: Freshness::Deferred {
                path: path.to_path_buf(),
            },
        },
    );
}

/// Whether an output is resolvable by any reader — a file exists on disk, or a
/// surface is published for it (including a *deferred*, file-less entry). Lets a
/// producer's report state "this output is available" truthfully after a
/// write-skip, where a bare `path.is_file()` would wrongly read `false`. Cheap:
/// no surface is cloned and the LRU order is untouched.
pub(crate) fn is_available(path: &Path) -> bool {
    if path.is_file() {
        return true;
    }
    let (id, _) = key_for_lookup(path);
    cache().lock().map(|lru| lru.contains(&id)).unwrap_or(false)
}

fn store(id: String, entry: Entry) {
    let evicted = match cache().lock() {
        Ok(mut lru) => lru.insert(id, entry),
        Err(_) => Vec::new(),
    };
    // Materialise any evicted deferred surface *after* dropping the lock — the
    // PNG write is best-effort I/O and must not block other cache users.
    for entry in &evicted {
        materialize(entry);
    }
}

/// Write a deferred entry's surface to its path so a reader that only knows the
/// file still resolves after the buffer is gone. Best-effort: a failure here
/// just means the (rare) evicted-then-read case falls back to a miss, exactly
/// as an un-published output would. A file-backed entry already has its file
/// and is a no-op.
fn materialize(entry: &Entry) {
    let Freshness::Deferred { path } = &entry.freshness else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match &entry.image {
        DecodedImage::Rgba { image, .. } => {
            let _ = image.save(path);
        }
        // Same encoder the producing card would have used on a direct write:
        // an Srgb surface lands as the exact 8-bit narrow, a ProPhoto surface
        // as 16-bit PNG with the ProPhoto profile embedded, so a deferred
        // output materialises byte-identical to its non-deferred twin.
        DecodedImage::Working { image, .. } => {
            let _ = super::studio_image::write_working_png(path, image);
        }
        DecodedImage::Gray(image) => {
            let _ = image.save(path);
        }
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
        // A published 16-bit surface serves 8-bit consumers via the egress, so
        // `load_rgba` still gets an sRGB surface identical to a disk decode.
        DecodedImage::Working { image, meta } => {
            if exceeds_budget(image.width, image.height, max_pixels) {
                return None;
            }
            Some(LoadedRgba {
                image: image.to_srgb_rgba8(),
                meta,
            })
        }
        DecodedImage::Gray(_) => None,
    }
}

/// Look up a published 16-bit [`WorkingImage`] for `path`, or `None` on a miss /
/// stale entry / budget overflow / wrong-kind entry. The native half of the
/// manual-chain handoff: a manual card that published its wide-gamut surface is
/// re-read at full precision by the next manual card. An 8-bit `Rgba` entry is
/// deliberately *not* widened here — only a genuine `Working` publish carries a
/// space tag + ICC, so a plain RGBA output falls back to a disk decode.
pub(crate) fn lookup_working(path: &Path, max_pixels: u64) -> Option<LoadedWorking> {
    let entry = fetch_fresh(path)?;
    match entry.image {
        DecodedImage::Working { image, meta } => {
            if exceeds_budget(image.width, image.height, max_pixels) {
                return None;
            }
            Some(LoadedWorking {
                image: (*image).clone(),
                meta,
            })
        }
        DecodedImage::Rgba { .. } | DecodedImage::Gray(_) => None,
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
        DecodedImage::Working { image, .. } => DynamicImage::ImageRgba8(image.to_srgb_rgba8()),
        DecodedImage::Gray(image) => DynamicImage::ImageLuma8((*image).clone()),
    })
}

/// Fetch the entry for `path` only when it is still fresh against disk; a stale
/// entry is evicted so it stops shadowing the file. An unresolvable /
/// unregistered path yields `None`.
fn fetch_fresh(path: &Path) -> Option<Entry> {
    let (id, disk) = key_for_lookup(path);
    let mut lru = cache().lock().ok()?;
    let entry = lru.get(&id)?;
    match &entry.freshness {
        // A deferred entry has no file to check; it is served until evicted.
        Freshness::Deferred { .. } => Some(entry),
        Freshness::File { mtime, len } => match disk {
            Some((disk_mtime, disk_len)) if *mtime == disk_mtime && *len == disk_len => {
                Some(entry)
            }
            _ => {
                lru.remove(&id);
                None
            }
        },
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
    fn a_published_working_surface_serves_both_precisions() {
        use super::super::working_image::{WorkingImage, WorkingSpace};

        let path = unique_tmp("working.png");
        // On disk: an unrelated 8-bit PNG (proves hits come from the buffer).
        RgbaImage::from_pixel(2, 2, Rgba([1, 1, 1, 255]))
            .save(&path)
            .unwrap();

        // Publish a 16-bit Srgb working surface whose low bytes differ from any
        // 8-bit rounding, so a native 16-bit hit is distinguishable.
        let mut work = WorkingImage::from_rgba8(
            &RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255])),
            WorkingSpace::Srgb,
            Some(vec![1, 2, 3, 4]),
        );
        work.pixels[0] = 12_345; // an odd 16-bit R that is not `widen(any u8)`
        publish_working(&path, &work, rgba_meta());

        // Native half: the manual chain gets the exact 16-bit pixels + ICC back.
        let native = lookup_working(&path, 0).expect("working hit");
        assert_eq!(native.image.pixels[0], 12_345);
        assert_eq!(native.image.space, WorkingSpace::Srgb);
        assert_eq!(native.image.icc.as_deref(), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(native.meta.source_mode, "RGBA");

        // 8-bit half: lookup_rgba / lookup_dynamic egress to sRGB, identical to
        // what a disk decode of the materialised file would yield.
        let expected = work.to_srgb_rgba8();
        let egressed = lookup_rgba(&path, 0).expect("rgba egress hit");
        assert_eq!(egressed.image, expected);
        match lookup_dynamic(&path).expect("dynamic egress hit") {
            DynamicImage::ImageRgba8(img) => assert_eq!(img, expected),
            other => panic!("expected ImageRgba8, got {other:?}"),
        }

        // Cross-kind: a working entry is not a mask hit.
        assert!(lookup_gray(&path, 0).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_rgba_entry_is_not_a_working_hit() {
        let path = unique_tmp("rgba_not_working.png");
        let img = RgbaImage::from_pixel(2, 2, Rgba([4, 5, 6, 255]));
        img.save(&path).unwrap();
        publish_rgba(&path, &img, rgba_meta());
        // Only a genuine Working publish carries a space tag + ICC, so a plain
        // 8-bit output is never silently widened into the 16-bit lookup.
        assert!(lookup_working(&path, 0).is_none());
        assert!(lookup_rgba(&path, 0).is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_deferred_working_surface_materialises_as_srgb_png() {
        use super::super::working_image::{WorkingImage, WorkingSpace};

        let mut lru = Lru::new(1);
        let path = unique_tmp("working_evict.png");
        let work = WorkingImage::from_rgba8(
            &RgbaImage::from_pixel(3, 3, Rgba([40, 80, 120, 255])),
            WorkingSpace::Srgb,
            None,
        );
        let deferred = Entry {
            image: DecodedImage::Working {
                image: Arc::new(work.clone()),
                meta: rgba_meta(),
            },
            freshness: Freshness::Deferred { path: path.clone() },
        };
        assert!(lru.insert("w".to_string(), deferred).is_empty());
        assert!(!path.exists());

        let filler = Entry {
            image: DecodedImage::Gray(Arc::new(GrayImage::from_pixel(1, 1, Luma([0])))),
            freshness: Freshness::File { mtime: None, len: 0 },
        };
        for entry in &lru.insert("filler".to_string(), filler) {
            materialize(entry);
        }
        // An evicted Srgb-space surface lands on disk as the exact 8-bit
        // narrow, same as the producing card's direct write.
        let reloaded = image::open(&path).expect("materialised png decodes").to_rgba8();
        assert_eq!(reloaded, work.to_srgb_rgba8());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut lru = Lru::new(2);
        let entry = |v: u8| Entry {
            image: DecodedImage::Gray(Arc::new(GrayImage::from_pixel(1, 1, Luma([v])))),
            freshness: Freshness::File { mtime: None, len: 0 },
        };
        lru.insert("a".to_string(), entry(1));
        lru.insert("b".to_string(), entry(2));
        assert!(lru.get("a").is_some()); // a is now most-recent
        let evicted = lru.insert("c".to_string(), entry(3)); // evicts b
        assert_eq!(evicted.len(), 1, "one entry dropped for capacity");
        assert!(lru.get("b").is_none());
        assert!(lru.get("a").is_some());
        assert!(lru.get("c").is_some());
        assert_eq!(lru.entries.len(), 2);
    }

    #[test]
    fn a_deferred_entry_is_served_without_a_file() {
        // A path with no file on disk: a normal publish cannot key it (there is
        // nothing to canonicalise), but a deferred publish keys it by the raw
        // path and serves it unconditionally — the write-skip display path.
        let path = unique_tmp("deferred_rgba.png");
        assert!(!path.exists(), "the deferred output is never written");
        assert!(lookup_rgba(&path, 0).is_none());

        publish_rgba_deferred(
            &path,
            &RgbaImage::from_pixel(5, 4, Rgba([1, 2, 3, 255])),
            rgba_meta(),
        );
        // The downstream compute loader resolves the surface from memory.
        let hit = lookup_rgba(&path, 0).expect("deferred buffer is a hit");
        assert_eq!(hit.image.dimensions(), (5, 4));
        assert_eq!(hit.image.get_pixel(0, 0).0, [1, 2, 3, 255]);
        // ...and so does the thumbnail path, even though no file exists.
        match lookup_dynamic(&path).expect("deferred dynamic hit") {
            DynamicImage::ImageRgba8(img) => assert_eq!(img.dimensions(), (5, 4)),
            other => panic!("expected ImageRgba8, got {other:?}"),
        }
        assert!(!path.exists(), "serving the buffer never writes the file");
    }

    #[test]
    fn evicting_a_deferred_entry_materialises_it() {
        // A deferred entry that is pushed out of the cache must leave its pixels
        // on disk so a later reader (a thumbnail fallback, a disk decode) still
        // resolves. Exercised on a local LRU so eviction is deterministic.
        let mut lru = Lru::new(1);
        let path = unique_tmp("materialise_on_evict.png");
        let deferred = Entry {
            image: DecodedImage::Rgba {
                image: Arc::new(RgbaImage::from_pixel(3, 3, Rgba([7, 8, 9, 255]))),
                meta: rgba_meta(),
            },
            freshness: Freshness::Deferred { path: path.clone() },
        };
        assert!(lru.insert("deferred".to_string(), deferred).is_empty());
        assert!(!path.exists());

        // A second insert evicts the deferred entry; the caller materialises it.
        let filler = Entry {
            image: DecodedImage::Gray(Arc::new(GrayImage::from_pixel(1, 1, Luma([0])))),
            freshness: Freshness::File { mtime: None, len: 0 },
        };
        let evicted = lru.insert("filler".to_string(), filler);
        assert_eq!(evicted.len(), 1);
        for entry in &evicted {
            materialize(entry);
        }
        assert!(path.exists(), "the evicted deferred surface is written to disk");
        let reloaded = image::open(&path).expect("materialised png decodes").to_rgba8();
        assert_eq!(reloaded.dimensions(), (3, 3));
        assert_eq!(reloaded.get_pixel(0, 0).0, [7, 8, 9, 255]);
        let _ = std::fs::remove_file(&path);
    }
}
