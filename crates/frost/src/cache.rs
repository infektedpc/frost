//! Bounded in-memory cache (Round 34), backed by `moka`.
//!
//! Frost's first real cache layer: a small generic wrapper around
//! `moka::sync::Cache` for parsed on-disk data that is read far more
//! often than it is written. Values are loaded once per key and then
//! served from memory until they are replaced by a write-through
//! save, explicitly invalidated, or expire.
//!
//! Bounds (the reason to use moka instead of a `HashMap`): the cache
//! holds at most `max_entries` entries — moka evicts past the cap —
//! and entries expire 60 s after their last write, so an external
//! edit to the underlying file surfaces within a minute instead of
//! living stale for the whole process. The sync API needs no async
//! runtime; the cache is `Clone + Send + Sync`, so one instance can
//! live in a `static`.
//!
//! Current consumer: the Deep Freeze archive index (`archive.rs`).
//! `list_archives` runs on every Archives-window open, and the index
//! file does not change between Frost-side writes — so the parsed
//! index is cached per directory instead of being re-read and
//! re-parsed every time. The directory scan in `list_archives`
//! still runs every time (orphan recovery + stale pruning), so the
//! list stays honest if files are moved by hand; only the JSON
//! read/parse is cached.

use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use moka::sync::Cache;

/// A bounded cache for parsed on-disk data, keyed by path (or any
/// hashable key). `V` is stored behind an `Arc` so a hit returns a
/// cheap shared reference instead of cloning the value.
pub struct DiskCache<K, V> {
    inner: Cache<K, Arc<V>>,
}

impl<K, V> DiskCache<K, V>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    /// A cache holding at most `max_entries` entries; entries expire
    /// 60 s after their last write.
    pub fn new(max_entries: u64) -> Self {
        Self::with_ttl(max_entries, Duration::from_secs(60))
    }

    /// Same as `new`, with an explicit TTL (tests use a short one to
    /// prove expiry without waiting a minute).
    fn with_ttl(max_entries: u64, ttl: Duration) -> Self {
        DiskCache {
            inner: Cache::builder()
                .max_capacity(max_entries)
                .time_to_live(ttl)
                .build(),
        }
    }

    /// The cached value for `key`, loading it (exactly once) on a
    /// miss. Concurrent loads for the same key collapse to one call.
    pub fn get_or_load(&self, key: K, load: impl FnOnce() -> V) -> Arc<V> {
        self.inner.get_with(key, move || Arc::new(load()))
    }

    /// Replace the cached value for `key` (write-through after a
    /// save) or insert it fresh.
    pub fn insert(&self, key: K, value: V) {
        self.inner.insert(key, Arc::new(value));
    }

    /// Drop the cached value for `key`. (API surface, exercised
    /// by the tests; the archive wiring uses write-through
    /// `insert` instead, so this is unused in production code.)
    #[allow(dead_code)]
    pub fn invalidate(&self, key: &K) {
        self.inner.invalidate(key);
    }

    /// Live entry count (tests + diagnostics). Moka applies writes
    /// lazily, so the count only reflects them after maintenance
    /// runs (`run_maintenance`) — never assert it on a fresh cache
    /// without running maintenance first.
    #[allow(dead_code)]
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Run moka's pending maintenance (eviction, expiry) now, so a
    /// test can observe the bounded size deterministically instead
    /// of waiting for a background pass.
    #[allow(dead_code)]
    pub fn run_maintenance(&self) {
        self.inner.run_pending_tasks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn counter_cache() -> (DiskCache<String, usize>, Arc<AtomicUsize>) {
        (DiskCache::new(64), Arc::new(AtomicUsize::new(0)))
    }

    /// Load through the cache, counting real loads.
    fn load(cache: &DiskCache<String, usize>, key: &str, loads: &Arc<AtomicUsize>) -> Arc<usize> {
        let loads = Arc::clone(loads);
        cache.get_or_load(key.to_string(), move || {
            loads.fetch_add(1, Ordering::SeqCst) + 1
        })
    }

    #[test]
    fn second_get_is_a_hit() {
        let (cache, loads) = counter_cache();
        assert_eq!(*load(&cache, "a", &loads), 1);
        assert_eq!(*load(&cache, "a", &loads), 1);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        // moka applies writes lazily: the entry count only reflects
        // them after maintenance runs.
        cache.run_maintenance();
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn different_keys_load_separately() {
        let (cache, loads) = counter_cache();
        assert_eq!(*load(&cache, "a", &loads), 1);
        assert_eq!(*load(&cache, "b", &loads), 2);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        cache.run_maintenance();
        assert_eq!(cache.entry_count(), 2);
    }

    #[test]
    fn invalidate_forces_a_reload() {
        let (cache, loads) = counter_cache();
        assert_eq!(*load(&cache, "a", &loads), 1);
        cache.invalidate(&"a".to_string());
        assert_eq!(*load(&cache, "a", &loads), 2);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn insert_overwrites_without_loading() {
        let (cache, loads) = counter_cache();
        cache.insert("a".to_string(), 42);
        assert_eq!(*load(&cache, "a", &loads), 42);
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        cache.insert("a".to_string(), 43);
        assert_eq!(*load(&cache, "a", &loads), 43);
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn capacity_bounds_entries_and_evicts() {
        let (cache, loads) = (DiskCache::new(2), Arc::new(AtomicUsize::new(0)));
        assert_eq!(*load(&cache, "a", &loads), 1);
        assert_eq!(*load(&cache, "b", &loads), 2);
        assert_eq!(*load(&cache, "c", &loads), 3);
        cache.run_maintenance();
        assert!(cache.entry_count() <= 2);
        // Three distinct keys cannot all fit in a capacity-2 cache,
        // so re-reading all three must reload at least one of them
        // (which one moka evicted is its TinyLFU policy's choice —
        // the test asserts eviction happened, not the victim).
        let before = loads.load(Ordering::SeqCst);
        let _ = load(&cache, "a", &loads);
        let _ = load(&cache, "b", &loads);
        let _ = load(&cache, "c", &loads);
        assert!(loads.load(Ordering::SeqCst) > before);
    }

    #[test]
    fn ttl_expiry_reloads() {
        let cache = DiskCache::with_ttl(64, Duration::from_millis(50));
        let loads = Arc::new(AtomicUsize::new(0));
        assert_eq!(*load(&cache, "a", &loads), 1);
        assert_eq!(*load(&cache, "a", &loads), 1);
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(*load(&cache, "a", &loads), 2);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }
}
