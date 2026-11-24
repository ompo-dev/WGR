//! WGR's bounded native asset transport. No HTML, JavaScript, or browser runtime.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use wgr_core::assets::{sha256, DiskCache};
use wgr_core::security::{rooted_file, safe_relative, HttpClient, NetworkPolicy};

const DISK_BUDGET: u64 = 6 << 30;
const MIN_DISK_BUDGET: u64 = 2 << 30;
const FREE_DISK_RESERVE: u64 = 12 << 30;
const DEFAULT_HOT_BUDGET_MIB: usize = 256;
const MAX_HOT_BUDGET_MIB: usize = 256;
const BLOCK: u64 = 128 << 10;
const FETCH_WINDOW: u64 = 1 << 20;
const BOOT_PREFETCH_BUDGET: u64 = 64 << 20;
const BUILD: &str = "8b0b5899ed";
const SHADER_INDEX_REFRESH: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

fn cache_budget_for_free_space(free_bytes: u64) -> u64 {
    MIN_DISK_BUDGET
        + free_bytes
            .saturating_sub(FREE_DISK_RESERVE)
            .min(DISK_BUDGET - MIN_DISK_BUDGET)
}

fn safe_cache_budget(support: &Path) -> u64 {
    #[cfg(windows)]
    {
        if let Ok(bytes) = crate::platform_windows::statfs_bytes(support) {
            let word = |offset: usize| {
                u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0; 8]))
            };
            return cache_budget_for_free_space(word(32).saturating_mul(word(8)));
        }
    }
    #[cfg(not(windows))]
    let _ = support;
    MIN_DISK_BUDGET
}
const HEADER_PREFETCH_WORKERS: usize = 2;
const MAX_HIGH_HINTS: usize = 16;
const MAX_LOW_HINTS: usize = 1;
const HINT_RECENT_TTL: std::time::Duration = std::time::Duration::from_secs(30);

fn hot_budget() -> usize {
    std::env::var("WGR_ASSET_HOT_CACHE_MIB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|value| (48..=MAX_HOT_BUDGET_MIB).contains(value))
        .unwrap_or(DEFAULT_HOT_BUDGET_MIB)
        << 20
}

/// The browser loader sends its I/O worker origin + /data/.
/// The v2 dataset differs from the old fiveplay.lol dataset. Never reuse
/// old bytes merely because an asset name and length happen to match.
fn data_cache_key(base: &str, version: &str, path: &str, start: u64, size: u64) -> String {
    format!("{BUILD}:data:{base}:{version}:{path}:{start}:{size}")
}

fn manifest_valid(manifest: &Value) -> bool {
    let Some(version) = manifest["version"].as_str() else {
        return false;
    };
    let Some(files) = manifest["files"].as_array() else {
        return false;
    };
    !version.is_empty()
        && version.len() <= 128
        && !files.is_empty()
        && files.len() <= 100_000
        && files.iter().all(|file| {
            file.as_array().is_some_and(|entry| {
                entry.len() >= 2
                    && entry[0]
                        .as_str()
                        .is_some_and(|path| safe_relative(path).is_ok())
                    && entry[1].as_u64().is_some_and(|size| size <= 1 << 40)
            })
        })
}

fn bootset_valid(value: &Value) -> bool {
    value.as_array().is_some_and(|rows| {
        rows.len() <= 100_000
            && rows.iter().all(|row| {
                row.as_array().is_some_and(|entry| {
                    entry.len() == 2
                        && entry[0]
                            .as_str()
                            .is_some_and(|path| safe_relative(path).is_ok())
                        && entry[1].is_array()
                })
            })
    })
}

fn refresh_data_metadata(
    runtime: &tokio::runtime::Runtime,
    http: &HttpClient,
    directory: &Path,
    filename: &str,
    url: &str,
    validate: fn(&Value) -> bool,
) -> Result<Value> {
    let path = directory.join(filename);
    let marker = directory.join(format!("{filename}.source"));
    let old = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(validate);
    let source_matches = std::fs::read_to_string(&marker)
        .ok()
        .is_some_and(|source| source == url);
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age < SHADER_INDEX_REFRESH);
    if old.is_some() && source_matches && fresh {
        return Ok(old.unwrap());
    }
    let latest = runtime.block_on(http.get(url, 8 << 20)).and_then(|reply| {
        ensure!(reply.status.is_success(), "Metadados HTTP {}", reply.status);
        let parsed: Value = serde_json::from_slice(&reply.body)?;
        ensure!(validate(&parsed), "Metadados inválidos de {url}");
        Ok((reply.body, parsed))
    });
    match latest {
        Ok((body, parsed)) => {
            if old.as_ref() != Some(&parsed) || !source_matches {
                if path.is_file() {
                    let backup = directory.join(format!("{filename}.previous"));
                    if !backup.exists() {
                        let _ = std::fs::copy(&path, &backup);
                    }
                }
                let temp = directory.join(format!("{filename}.partial"));
                std::fs::write(&temp, body)?;
                std::fs::rename(&temp, &path)?;
                eprintln!("[native-assets] refreshed {filename} from {url}");
            }
            std::fs::write(marker, url)?;
            Ok(parsed)
        }
        Err(error) if source_matches && old.is_some() => {
            eprintln!("[native-assets] {filename}: offline validated cache: {error}");
            Ok(old.unwrap())
        }
        Err(error) => Err(error.context(format!(
            "{filename}: catálogo local pertence a outra origem; versão v2 requer sincronização inicial"
        ))),
    }
}

fn shader_index_valid(index: &Value) -> bool {
    index.is_object()
        && index["_packs"]
            .as_array()
            .is_some_and(|packs| packs.len() <= 1024)
}

/// Maintain one bounded, browser-build-matched shader catalog. An old index
/// from the previous hostname can omit shaders the Chromium port already
/// knows about (including ped skin/deferred and SSAO passes). Offline launch
/// keeps the last validated catalog; no unchecked download replaces it.
fn refresh_shader_index(
    runtime: &tokio::runtime::Runtime,
    http: &HttpClient,
    directory: &Path,
    base: &str,
) -> Result<Value> {
    let path = directory.join("index.json");
    let source_marker = directory.join("index.source");
    let old = std::fs::read(&path)
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
        .filter(shader_index_valid);
    let same_origin = std::fs::read_to_string(&source_marker)
        .ok()
        .is_some_and(|source| source == base);
    let recent = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < SHADER_INDEX_REFRESH);
    if old.is_some() && same_origin && recent {
        return Ok(old.unwrap());
    }
    let result = runtime
        .block_on(http.get(&format!("{base}index.json"), 8 << 20))
        .and_then(|reply| {
            ensure!(
                reply.status.is_success(),
                "HTTP {} for shader index",
                reply.status
            );
            let updated: Value = serde_json::from_slice(&reply.body)?;
            ensure!(
                shader_index_valid(&updated),
                "Índice de shaders remoto inválido"
            );
            Ok((reply.body, updated))
        });
    match result {
        Ok((body, updated)) => {
            if old.as_ref() != Some(&updated) {
                if path.exists() {
                    let backup = directory.join("index.previous.json");
                    if !backup.exists() {
                        let _ = std::fs::copy(&path, &backup);
                    }
                }
                let temporary = directory.join("index.partial");
                std::fs::write(&temporary, body)?;
                std::fs::rename(&temporary, &path)?;
                eprintln!(
                    "[native-assets] shader catalog refreshed: {} records",
                    updated.as_object().map(|x| x.len()).unwrap_or(0)
                );
            }
            let _ = std::fs::write(&source_marker, base);
            Ok(updated)
        }
        Err(error) => {
            if let Some(old) = old.filter(|_| same_origin) {
                eprintln!("[native-assets] shader catalog refresh unavailable ({error}); using validated offline copy");
                Ok(old)
            } else {
                Err(error.context(
                    "Catálogo de shaders indisponível para esta origem; não misturar versões",
                ))
            }
        }
    }
}

fn fetch_window(start: u64, size: u64) -> (u64, u64) {
    let window_start = start / FETCH_WINDOW * FETCH_WINDOW;
    (
        window_start,
        window_start.saturating_add(FETCH_WINDOW).min(size),
    )
}

fn fetch_asset_window(path: &str, start: u64, size: u64, bulk: bool) -> (u64, u64) {
    if path.ends_with(".rpf") && !bulk {
        let begin = start / BLOCK * BLOCK;
        (begin, (begin + BLOCK).min(size))
    } else {
        fetch_window(start, size)
    }
}

fn boot_range_end(position: u64, end: u64, budget_left: u64) -> u64 {
    position
        .saturating_add(1 << 20)
        .min(end)
        .min(position.saturating_add(budget_left))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbundled_valid_shaders_are_not_mistaken_for_missing_packs() {
        assert_eq!(
            shader_pack_slice(
                &serde_json::json!({"ok":true,"effect":"SSAO","program":"VS_screenTransformSSAO"}),
                false,
            ),
            None,
        );
        let packed = serde_json::json!({"p":[2,256,8192,64]});
        assert_eq!(shader_pack_slice(&packed, false), Some((256, 8192)));
        assert_eq!(shader_pack_slice(&packed, true), Some((8448, 64)));
        assert_eq!(
            shader_pack_slice(&serde_json::json!({"p":[2]}), false),
            None
        );
    }

    #[test]
    fn never_reuses_old_origin_or_version_asset_blocks() {
        let old = data_cache_key(
            "https://fiveplay.lol/data/",
            "1791169379-5814",
            "common/data/gameconfig.xml",
            0,
            58602,
        );
        let current = data_cache_key(
            "https://v2.fiveplay.lol/data/",
            "grandv-r5-mobile-core-20261023",
            "common/data/gameconfig.xml",
            0,
            58602,
        );
        let next = data_cache_key(
            "https://v2.fiveplay.lol/data/",
            "grandv-r5-mobile-core-20261024",
            "common/data/gameconfig.xml",
            0,
            58602,
        );
        assert_ne!(old, current);
        assert_ne!(current, next);
        assert_eq!(
            current,
            data_cache_key(
                "https://v2.fiveplay.lol/data/",
                "grandv-r5-mobile-core-20261023",
                "common/data/gameconfig.xml",
                0,
                58602,
            )
        );
    }

    #[test]
    fn rejects_malformed_data_manifests_and_bootsets() {
        assert!(!manifest_valid(&serde_json::json!({"files": []})));
        assert!(!manifest_valid(&serde_json::json!({
            "version":"v2", "files":[["../outside.bin",20]]
        })));
        assert!(manifest_valid(&serde_json::json!({
            "version":"v2", "files":[["common/data/gameconfig.xml",58602,42]]
        })));
        assert!(!bootset_valid(&serde_json::json!({"unexpected":"object"})));
        assert!(!bootset_valid(&serde_json::json!([[
            "../../secret",
            [[0, 2]]
        ]])));
        assert!(bootset_valid(&serde_json::json!([[
            "common/data/gameconfig.xml",
            [[0, 4095]]
        ]])));
    }

    #[test]
    fn rejects_invalid_shader_catalogs_before_replacing_offline_cache() {
        assert!(!shader_index_valid(&serde_json::json!({})));
        assert!(!shader_index_valid(&serde_json::json!({"_packs":{}})));
        assert!(shader_index_valid(&serde_json::json!({
            "_packs":[{"file":"pack0.bin"}],
            "0123456789abcdef":{"stage":"ps","ok":true}
        })));
        assert!(!shader_index_valid(&serde_json::json!({
            "_packs": vec![serde_json::json!(null); 1025]
        })));
    }

    #[test]
    fn groups_adjacent_blocks_in_one_bounded_window() {
        assert_eq!(fetch_window(0, 3 << 20), (0, 1 << 20));
        assert_eq!(fetch_window(128 << 10, 3 << 20), (0, 1 << 20));
        assert_eq!(fetch_window(1 << 20, 3 << 20), (1 << 20, 2 << 20));
        assert_eq!(fetch_window((3 << 20) - 64, 3 << 20), (2 << 20, 3 << 20));
    }

    #[test]
    fn archive_headers_do_not_trigger_one_megabyte_downloads() {
        assert_eq!(
            fetch_asset_window("x64/anim/clip.rpf", 0, 10 << 20, false),
            (0, BLOCK)
        );
        assert_eq!(
            fetch_asset_window("x64/anim/short.rpf", 0, 8000, false),
            (0, 8000)
        );
        assert_eq!(
            fetch_asset_window("x64/anim/clip.rpf", BLOCK, 10 << 20, false),
            (BLOCK, BLOCK * 2)
        );
        assert_eq!(
            fetch_asset_window("x64/anim/clip.rpf", BLOCK, 10 << 20, true),
            (0, FETCH_WINDOW)
        );
        assert_eq!(
            fetch_asset_window("common/data/gameconfig.xml", 0, 10 << 20, false),
            (0, FETCH_WINDOW)
        );
    }

    #[test]
    fn caps_boot_preload_without_splitting_the_final_range() {
        assert_eq!(boot_range_end(63 << 20, 100 << 20, 1 << 20), 64 << 20);
        assert_eq!(boot_range_end(60 << 20, 61 << 20, 4 << 20), 61 << 20);
    }

    #[test]
    fn urgent_streaming_suppresses_speculative_hints_but_not_nearby_hints() {
        assert!(allow_hint(false, 30, 0, 0));
        assert!(allow_hint(false, 30, MAX_HIGH_HINTS - 1, 0));
        assert!(!allow_hint(false, 30, MAX_HIGH_HINTS, 0));
        assert!(!allow_hint(true, 12, 0, 0));
        assert!(allow_hint(true, 11, 0, 0));
        assert!(!allow_hint(true, 0, 0, MAX_LOW_HINTS));
        assert!(allow_hint(true, 0, 0, 0));
    }

    #[test]
    fn memory_cache_evicts_oldest_bytes_without_eviction_scans() {
        let mut cache = Hot::new(6);
        for (key, byte) in [("a", 1), ("b", 2), ("c", 3)] {
            cache.insert(key.into(), Arc::new(vec![byte; 2]));
        }
        assert_eq!(&cache.get("a").unwrap()[..], &[1, 1]);
        cache.insert("d".into(), Arc::new(vec![4; 2]));
        assert!(cache.get("b").is_none(), "b is the least recently used");
        assert_eq!(cache.bytes, 6);
        assert_eq!(cache.oldest.len(), cache.entries.len());
        cache.insert("a".into(), Arc::new(vec![8; 3]));
        assert_eq!(&cache.get("a").unwrap()[..], &[8, 8, 8]);
        assert!(cache.bytes <= 6);
        assert_eq!(cache.oldest.len(), cache.entries.len());
        cache.insert("oversize".into(), Arc::new(vec![0; 7]));
        assert!(cache.get("oversize").is_none());
    }

    #[test]
    fn recently_loaded_hints_are_coalesced_without_permanently_suppressing_retries() {
        let now = std::time::Instant::now();
        assert!(!hint_recently_satisfied(None, now));
        assert!(hint_recently_satisfied(
            Some(&(now - std::time::Duration::from_secs(2))),
            now
        ));
        assert!(!hint_recently_satisfied(
            Some(&(now - HINT_RECENT_TTL)),
            now
        ));
    }

    #[test]
    fn cache_budget_keeps_free_disk_reserve_and_stays_bounded() {
        assert_eq!(cache_budget_for_free_space(0), MIN_DISK_BUDGET);
        assert_eq!(cache_budget_for_free_space(10 << 30), MIN_DISK_BUDGET);
        assert_eq!(cache_budget_for_free_space(14 << 30), 4 << 30);
        assert_eq!(cache_budget_for_free_space(80 << 30), DISK_BUDGET);
    }
}

static ASSETS: OnceLock<Arc<Assets>> = OnceLock::new();
static HOT_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DISK_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FETCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DECODED_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HINTS_QUEUED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HINTS_DONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LIVE_IO_ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static HINTS_HIGH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HINTS_LOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HINTS_DEFERRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HINTS_DUPLICATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DISK_LOOKUP_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DISK_STORE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static NETWORK_FETCH_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn metrics() -> Value {
    use std::sync::atomic::Ordering::Relaxed;
    let (hot_bytes, hot_entries, hot_capacity_bytes) = ASSETS
        .get()
        .and_then(|assets| assets.hot.lock().ok())
        .map(|hot| (hot.bytes, hot.entries.len(), hot.budget))
        .unwrap_or((0, 0, 0));
    let (dns_lookups, dns_cache_hits) = ASSETS
        .get()
        .map(|assets| assets.http.dns_cache_stats())
        .unwrap_or((0, 0));
    let (cache_db_wait_ms, cache_verify_ms, cache_lru_batches, cache_lru_entries) = ASSETS
        .get()
        .and_then(|assets| assets.cache.read().ok())
        .map(|cache| cache.read_timings())
        .unwrap_or((0, 0, 0, 0));
    serde_json::json!({
        "hot_hits":HOT_HITS.load(Relaxed),"disk_hits":DISK_HITS.load(Relaxed),
        "transport_fetches":FETCHES.load(Relaxed),"decoded_asset_bytes":DECODED_BYTES.load(Relaxed),
        "hints_queued":HINTS_QUEUED.load(Relaxed),"hints_completed":HINTS_DONE.load(Relaxed),
        "hints_high":HINTS_HIGH.load(Relaxed),"hints_low":HINTS_LOW.load(Relaxed),
        "hints_deferred":HINTS_DEFERRED.load(Relaxed),"hints_duplicate":HINTS_DUPLICATE.load(Relaxed),
        "hints_source_coalesced":crate::io::source_hints_coalesced(),
        "live_io_active":LIVE_IO_ACTIVE.load(Relaxed),"hot_cache_bytes":hot_bytes,
        "hot_cache_entries":hot_entries,"hot_cache_capacity_bytes":hot_capacity_bytes,
        "disk_lookup_ms":DISK_LOOKUP_NS.load(Relaxed)/1_000_000,
        "disk_db_wait_ms":cache_db_wait_ms,"disk_verify_ms":cache_verify_ms,
        "disk_lru_batches":cache_lru_batches,"disk_lru_entries":cache_lru_entries,
        "disk_store_ms":DISK_STORE_NS.load(Relaxed)/1_000_000,
        "network_fetch_ms":NETWORK_FETCH_NS.load(Relaxed)/1_000_000,
        "dns_lookups":dns_lookups,"dns_cache_hits":dns_cache_hits,
    })
}

struct LiveRead;
impl LiveRead {
    fn begin() -> Self {
        LIVE_IO_ACTIVE.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self
    }
}
impl Drop for LiveRead {
    fn drop(&mut self) {
        LIVE_IO_ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

fn allow_hint(low: bool, active: usize, pending: usize, low_pending: usize) -> bool {
    if low {
        // Speculative streaming is useful before the player moves. Reserve
        // at least twenty live I/O lanes and permit just one low hint at once.
        active < 12 && pending < MAX_HIGH_HINTS && low_pending < MAX_LOW_HINTS
    } else {
        pending < MAX_HIGH_HINTS
    }
}

fn hint_recently_satisfied(since: Option<&std::time::Instant>, now: std::time::Instant) -> bool {
    since.is_some_and(|last| now.saturating_duration_since(*last) < HINT_RECENT_TTL)
}
struct Hot {
    entries: HashMap<Arc<str>, (Arc<Vec<u8>>, u64)>,
    oldest: BTreeMap<u64, Arc<str>>,
    bytes: usize,
    clock: u64,
    budget: usize,
}
impl Hot {
    fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            oldest: BTreeMap::new(),
            bytes: 0,
            clock: 0,
            budget,
        }
    }

    fn get(&mut self, key: &str) -> Option<Arc<Vec<u8>>> {
        let (owned_key, data, old_age) = self
            .entries
            .get_key_value(key)
            .map(|(owned_key, (data, age))| (owned_key.clone(), data.clone(), *age))?;
        self.oldest.remove(&old_age);
        self.clock = self.clock.saturating_add(1);
        if let Some((_, age)) = self.entries.get_mut(key) {
            *age = self.clock;
        }
        self.oldest.insert(self.clock, owned_key);
        Some(data)
    }

    fn insert(&mut self, key: String, data: Arc<Vec<u8>>) {
        if data.len() > self.budget {
            return;
        }
        if let Some((old, age)) = self.entries.remove(key.as_str()) {
            self.bytes -= old.len();
            self.oldest.remove(&age);
        }
        while self.bytes + data.len() > self.budget {
            let Some((_, victim)) = self.oldest.pop_first() else {
                break;
            };
            if let Some((old, _)) = self.entries.remove(victim.as_ref()) {
                self.bytes -= old.len();
            }
        }
        self.clock = self.clock.saturating_add(1);
        self.bytes += data.len();
        let key: Arc<str> = key.into();
        self.oldest.insert(self.clock, key.clone());
        self.entries.insert(key, (data, self.clock));
    }
}
struct Assets {
    runtime: tokio::runtime::Runtime,
    http: HttpClient,
    cache: RwLock<DiskCache>,
    hot: Mutex<Hot>,
    locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    prefetching: Mutex<HashSet<String>>,
    hinting: Mutex<HashMap<String, bool>>,
    recent_hints: Mutex<HashMap<String, std::time::Instant>>,
    hint_slots: tokio::sync::Semaphore,
    background: rayon::ThreadPool,
    network: tokio::sync::Semaphore,
    index: Value,
    data_base: String,
    shader_base: String,
    version: String,
}

pub fn prepare(support: &Path) -> Result<Option<PathBuf>> {
    let Some(port) = std::env::var_os("WGR_PORT_DESCRIPTOR") else {
        return Ok(None);
    };
    ensure!(
        std::fs::metadata(&port)?.len() <= 64 << 10,
        "Manifesto de port excede 64 KiB"
    );
    let descriptor: Value = serde_json::from_slice(&std::fs::read(port)?)?;
    ensure!(
        descriptor["format"] == "wgr.port.v1" && descriptor["abi"] == "emscripten-rage64-v1",
        "ABI de port incompatível"
    );
    let wasm = PathBuf::from(std::env::var_os("WGR_WASM_FILE").context("Binário do port ausente")?);
    ensure!(
        std::fs::metadata(&wasm)?.len() <= 256 << 20,
        "Binário excede 256 MiB"
    );
    ensure!(
        sha256(&std::fs::read(&wasm)?)
            == descriptor["binary"]["sha256"]
                .as_str()
                .context("SHA-256 ausente")?,
        "Binário do port difere do SHA-256 autorizado"
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .thread_name("native-net")
        .build()?;
    let http = HttpClient::new(NetworkPolicy {
        allowed_origins: vec![
            "https://fiveplay.lol".into(),
            "https://cdn.fiveplay.lol".into(),
            "https://v2.fiveplay.lol".into(),
        ],
        development_loopback: false,
    })
    .with_dns_pin_ttl(std::time::Duration::from_secs(30));
    let data_base = descriptor["assets"]["base"]
        .as_str()
        .context("Origem dos dados ausente")?
        .to_string();
    let shader_base = descriptor["shaders"]["base"]
        .as_str()
        .context("Origem dos shaders ausente")?
        .to_string();
    http.policy.validate(&data_base)?;
    http.policy.validate(&shader_base)?;
    let data_dir = support.join("metadata/data");
    let shader_dir = support.join("metadata/shaders");
    for directory in [&data_dir, &shader_dir] {
        std::fs::create_dir_all(directory)?;
    }
    let manifest_url = descriptor["assets"]["manifest"]
        .as_str()
        .context("Manifesto dos dados ausente")?;
    http.policy.validate(manifest_url)?;
    let manifest = refresh_data_metadata(
        &runtime,
        &http,
        &data_dir,
        "manifest.json",
        manifest_url,
        manifest_valid,
    )?;
    let bootset_url = format!("{data_base}bootset.json");
    http.policy.validate(&bootset_url)?;
    let bootset = refresh_data_metadata(
        &runtime,
        &http,
        &data_dir,
        "bootset.json",
        &bootset_url,
        bootset_valid,
    )?;
    let index = refresh_shader_index(&runtime, &http, &shader_dir, &shader_base)?;
    ensure!(
        index.is_object() && index.get("_packs").is_some(),
        "Índice de shaders inválido"
    );
    let files = manifest["files"]
        .as_array()
        .context("Manifesto sem arquivos")?;
    ensure!(files.len() <= 100_000, "Manifesto excessivo");
    for file in files {
        safe_relative(file[0].as_str().context("Nome de arquivo inválido")?)?;
    }
    std::env::set_var("MUGUET_DATA", &data_dir);
    std::env::set_var("MUGUET_SHADERS", &shader_dir);
    let cache_root = support.join("asset-cache");
    let assets = Arc::new(Assets {
        runtime,
        http,
        cache: RwLock::new(DiskCache::open(
            cache_root.clone(),
            safe_cache_budget(support),
        )?),
        hot: Mutex::new(Hot::new(hot_budget())),
        locks: Mutex::new(HashMap::new()),
        prefetching: Mutex::new(HashSet::new()),
        hinting: Mutex::new(HashMap::new()),
        recent_hints: Mutex::new(HashMap::new()),
        hint_slots: tokio::sync::Semaphore::new(4),
        background: rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .thread_name(|_| "native-prefetch".into())
            .build()?,
        network: tokio::sync::Semaphore::new(32),
        index,
        data_base,
        shader_base,
        version: manifest["version"]
            .as_str()
            .context("Versão dos dados ausente")?
            .to_string(),
    });
    ASSETS
        .set(assets.clone())
        .map_err(|_| anyhow::anyhow!("Assets nativos já inicializados"))?;
    assets.prewarm_gpu_shaders()?;
    if assets
        .data_base
        .starts_with("https://v2.fiveplay.lol/data/")
    {
        // v2 has no reliable batch endpoint. Downloading thousands of game
        // shaders via individual HTTP ranges BEFORE entering Wasmtime can
        // stall launch for minutes. The native I/O lane serves these resources
        // on demand and handles the game's prefetch-hint ring separately.
        eprintln!(
            "[native-assets] v2: shader and boot data stream on demand via bounded HTTP ranges"
        );
        assets.prewarm_archive_headers(files, &bootset);
    } else {
        assets.prewarm_shaders(files)?;
        assets.prewarm_boot(files, &data_dir.join("bootset.json"))?;
    }
    Ok(Some(wasm))
}

pub fn configure_budget(support: &Path, wasm: &Path) -> Result<()> {
    let Some(assets) = ASSETS.get() else {
        return Ok(());
    };
    let mut overhead = std::fs::metadata(wasm)?.len() + (16 << 20);
    for entry in std::fs::read_dir(support)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|x| x == "cwasm") {
            overhead = overhead.saturating_add(entry.metadata()?.len());
        }
    }
    let total_budget = safe_cache_budget(support);
    let budget = total_budget
        .checked_sub(overhead)
        .filter(|n| *n >= 128 << 20)
        .context("Código compilado excede o orçamento disponível de cache")?;
    // Keep the validated SQLite connection and any active prewarm state.
    // Reopening here made the hot startup path redo database setup and dropped
    // diagnostic counters while background I/O was warming archive headers.
    assets.cache.write().unwrap().set_limit(budget);
    eprintln!(
        "[native-assets] cache total <= {} GiB; dados <= {} MiB; RAM de transporte <= 48 MiB",
        total_budget >> 30,
        budget >> 20
    );
    Ok(())
}

impl Assets {
    fn prewarm_archive_headers(self: &Arc<Self>, files: &[Value], bootset: &Value) {
        // GTA mounts thousands of archives by first reading just 16 bytes.
        // Prefer bootset archives, which are actually requested during the
        // initial load. Limit background I/O to two workers so it cannot
        // saturate the 30 live streaming lanes with distant archive headers.
        let boot_paths: HashSet<String> = bootset
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row[0].as_str().map(str::to_string))
            .collect();
        let mut archives: Vec<(String, u64)> = files
            .iter()
            .filter_map(|file| {
                let path = file[0].as_str()?;
                let size = file[1].as_u64()?;
                (size > 0 && path.ends_with(".rpf")).then(|| (path.to_string(), size))
            })
            .collect();
        archives.sort_by_key(|(path, _)| !boot_paths.contains(path));
        archives.truncate(2048);
        let count = archives.len();
        let archives = Arc::new(archives);
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let skipped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for worker in 0..HEADER_PREFETCH_WORKERS {
            let assets = Arc::clone(self);
            let archives = Arc::clone(&archives);
            let done = Arc::clone(&done);
            let failures = Arc::clone(&failures);
            let skipped = Arc::clone(&skipped);
            let boot_paths = boot_paths.clone();
            self.background.spawn(move || {
                for (path, size) in archives.iter().skip(worker).step_by(HEADER_PREFETCH_WORKERS) {
                    // An unrelated archive must not compete with urgent model
                    // and texture requests when the engine is saturated.
                    if !boot_paths.contains(path)
                        && LIVE_IO_ACTIVE.load(std::sync::atomic::Ordering::Acquire) >= 16
                    {
                        skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                    let url = format!("{}{path}?v={}", assets.data_base, assets.version);
                    if assets.data_block(path, *size, 0, &url, false).is_err() {
                        failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    let ready = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if ready % 256 == 0 || ready == count {
                        eprintln!("[native-assets] archive headers {ready}/{count}");
                    }
                }
                if done.load(std::sync::atomic::Ordering::Relaxed) >= count {
                    eprintln!(
                        "[native-assets] RPF header prefetch: {count} candidates; {} deferred to live I/O; {} failed (retried on demand)",
                        skipped.load(std::sync::atomic::Ordering::Relaxed),
                        failures.load(std::sync::atomic::Ordering::Relaxed)
                    );
                }
            });
        }
    }

    fn prewarm_gpu_shaders(&self) -> Result<()> {
        use rayon::prelude::*;
        let packs = self.index["_packs"]
            .as_array()
            .context("Packs de shaders ausentes")?;
        ensure!(packs.len() <= 1024, "Índice de packs excessivo");
        crate::bridge::emit(
            serde_json::json!({"type":"game-progress","payload":{"label":"Preparando shaders gráficos em cache"}}),
        );
        self.background.install(|| {
            (0..packs.len())
                .into_par_iter()
                .try_for_each(|number| self.pack(&serde_json::json!({"p":[number]})).map(|_| ()))
        })?;
        eprintln!(
            "[native-assets] {} GPU shader packs ready before rendering",
            packs.len()
        );
        Ok(())
    }
    fn prewarm_boot(&self, files: &[Value], path: &Path) -> Result<()> {
        use rayon::prelude::*;
        let boot: Vec<(String, Vec<(u64, u64)>)> = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(boot.len() <= 100_000, "Boot set excessivo");
        let sizes: HashMap<_, _> = files
            .iter()
            .filter_map(|f| Some((f[0].as_str()?.to_string(), f[1].as_u64()?)))
            .collect();
        let mut batches = Vec::new();
        let mut batch = Vec::new();
        let mut batch_bytes = 0;
        let mut ranges = Vec::new();
        let mut range_bytes = 0;
        for (name, mut spans) in boot {
            safe_relative(&name)?;
            let Some(&size) = sizes.get(&name) else {
                continue;
            };
            if size == 0 {
                continue;
            }
            if size <= 8 << 20
                && [
                    ".fxc", ".meta", ".xml", ".dat", ".ymt", ".txt", ".csv", ".gfx", ".json",
                    ".sps", ".cfg",
                ]
                .iter()
                .any(|e| name.ends_with(e))
            {
                let first = data_cache_key(&self.data_base, &self.version, &name, 0, size);
                let last = data_cache_key(
                    &self.data_base,
                    &self.version,
                    &name,
                    (size - 1) / BLOCK * BLOCK,
                    size,
                );
                if self.cached(&first)?.is_some() && self.cached(&last)?.is_some() {
                    continue;
                }
                batch.push((name, size));
                batch_bytes += size;
                if batch_bytes >= 4 << 20 || batch.len() >= 300 {
                    batches.push(std::mem::take(&mut batch));
                    batch_bytes = 0;
                }
            } else {
                spans.sort_unstable();
                let mut covered = 0;
                for (first, last) in spans {
                    ensure!(first <= last, "Intervalo de boot inválido");
                    if first >= size {
                        continue;
                    }
                    let start = (first / BLOCK * BLOCK).max(covered);
                    let end = last
                        .saturating_add(1)
                        .min(size)
                        .div_ceil(BLOCK)
                        .saturating_mul(BLOCK)
                        .min(size);
                    let mut position = start;
                    while position < end && range_bytes < BOOT_PREFETCH_BUDGET {
                        let remaining = BOOT_PREFETCH_BUDGET - range_bytes;
                        let finish = boot_range_end(position, end, remaining);
                        ranges.push((name.clone(), size, position, finish));
                        range_bytes += finish - position;
                        position = finish;
                    }
                    covered = covered.max(end);
                }
            }
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        crate::bridge::emit(
            serde_json::json!({"type":"game-progress","payload":{"label":"Preparando os dados iniciais do jogo"}}),
        );
        eprintln!(
            "[native-assets] boot: {} metadata batches, {} range jobs ({} MiB initial; rest on demand)",
            batches.len(),
            ranges.len(),
            range_bytes >> 20
        );
        self.background.install(|| {
            batches
                .par_iter()
                .try_for_each(|batch| self.fetch_batch(batch))
        })?;
        let completed = std::sync::atomic::AtomicUsize::new(0);
        self.background.install(||ranges.par_iter().try_for_each(|(path,size,start,end)|->Result<()> {
            let url=format!("{}{path}?v={}",self.data_base,self.version);
            let mut position=*start;
            while position<*end {
                self.data_block(path, *size, position, &url, true)?;
                position=(position+BLOCK).min(*end);
            }
            let done=completed.fetch_add(1,std::sync::atomic::Ordering::Relaxed)+1;
            if done%16==0||done==ranges.len(){crate::bridge::emit(serde_json::json!({"type":"game-progress","payload":{"label":format!("Preparando dados do jogo: {done}/{}",ranges.len())}}));}
            Ok(())
        }))?;
        eprintln!("[native-assets] original boot set ready");
        Ok(())
    }
    fn prewarm_shaders(&self, files: &[Value]) -> Result<()> {
        use rayon::prelude::*;
        let mut batches = Vec::new();
        let mut batch = Vec::new();
        let mut bytes = 0u64;
        for file in files {
            let Some(path) = file[0].as_str() else {
                continue;
            };
            let Some(size) = file[1].as_u64().filter(|n| *n > 0 && *n <= 8 << 20) else {
                continue;
            };
            if !path.starts_with("common/shaders/") || !path.ends_with(".fxc") {
                continue;
            }
            let first = data_cache_key(&self.data_base, &self.version, path, 0, size);
            let last = data_cache_key(
                &self.data_base,
                &self.version,
                path,
                (size - 1) / BLOCK * BLOCK,
                size,
            );
            if self.cached(&first)?.is_some() && self.cached(&last)?.is_some() {
                continue;
            }
            batch.push((path.to_string(), size));
            bytes += size;
            if bytes >= 4 << 20 || batch.len() >= 300 {
                batches.push(std::mem::take(&mut batch));
                bytes = 0;
            }
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        eprintln!(
            "[native-assets] preparing shaders: {} compressed batches",
            batches.len()
        );
        self.background.install(|| {
            batches
                .par_iter()
                .try_for_each(|batch| self.fetch_batch(batch))
        })?;
        eprintln!("[native-assets] shader files ready in persistent cache");
        Ok(())
    }
    fn fetch_batch(&self, files: &[(String, u64)]) -> Result<()> {
        // The v2 origin's POST /data/batch endpoint currently times out or
        // returns an undecodable body. The same origin serves byte ranges
        // correctly (206 with an exact Content-Range). Use identity-encoded
        // bounded range requests rather than blocking GTA startup.
        if self.data_base.starts_with("https://v2.fiveplay.lol/data/") {
            return self.fetch_files_by_range(files);
        }
        let body = serde_json::to_vec(
            &files
                .iter()
                .map(|(path, size)| serde_json::json!([path, 0, size - 1]))
                .collect::<Vec<_>>(),
        )?;
        let expected = files.iter().map(|(_, size)| *size as usize).sum::<usize>();
        ensure!(expected <= 12 << 20, "Lote excessivo");
        let url = format!("{}batch?gz=1&c=1&v={}", self.data_base, self.version);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "accept-encoding",
            reqwest::header::HeaderValue::from_static("gzip"),
        );
        headers.insert(
            "content-type",
            reqwest::header::HeaderValue::from_static("text/plain;charset=UTF-8"),
        );
        let reply = self.runtime.block_on(async {
            let _permit = self.network.acquire().await?;
            self.http
                .request_body(reqwest::Method::POST, &url, headers, expected + 4096, body)
                .await
        })?;
        ensure!(
            reply.status.is_success(),
            "Lote de shaders HTTP {}",
            reply.status
        );
        let bytes = if reply.body.starts_with(&[0x1f, 0x8b]) {
            wgr_core::security::decode_gzip_bounded(&reply.body, expected)?
        } else {
            reply.body
        };
        ensure!(
            bytes.len() == expected,
            "Lote truncado: {} de {expected}",
            bytes.len()
        );
        let cache = self.cache.read().unwrap();
        let mut position = 0;
        for (path, size) in files {
            for (number, chunk) in bytes[position..position + *size as usize]
                .chunks(BLOCK as usize)
                .enumerate()
            {
                let key = data_cache_key(
                    &self.data_base,
                    &self.version,
                    path,
                    number as u64 * BLOCK,
                    *size,
                );
                cache.put(&key, chunk)?;
            }
            position += *size as usize;
        }
        cache.unpin_all();
        eprintln!(
            "[native-assets] batch cached: {} files, {} KiB",
            files.len(),
            expected >> 10
        );
        Ok(())
    }

    fn fetch_files_by_range(&self, files: &[(String, u64)]) -> Result<()> {
        for (path, size) in files {
            safe_relative(path)?;
            if *size == 0 {
                continue;
            }
            let url = format!("{}{path}?v={}", self.data_base, self.version);
            let mut offset = 0;
            while offset < *size {
                self.data_block(path, *size, offset, &url, true)?;
                offset = (offset + FETCH_WINDOW).min(*size);
            }
        }
        Ok(())
    }
    fn cached(&self, key: &str) -> Result<Option<Arc<Vec<u8>>>> {
        if let Some(data) = self.hot(key) {
            HOT_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(Some(data));
        }
        let cache = self.cache.read().unwrap();
        let disk_started = std::time::Instant::now();
        let result = cache.read(key, 8 << 20);
        cache.unpin_all();
        drop(cache);
        DISK_LOOKUP_NS.fetch_add(
            disk_started.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        if let Some(bytes) = result? {
            DISK_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let data = Arc::new(bytes);
            self.remember(key.to_string(), data.clone());
            return Ok(Some(data));
        }
        Ok(None)
    }
    fn data_block(
        &self,
        path: &str,
        size: u64,
        start: u64,
        url: &str,
        bulk: bool,
    ) -> Result<Arc<Vec<u8>>> {
        let key = data_cache_key(&self.data_base, &self.version, path, start, size);
        if let Some(data) = self.cached(&key)? {
            return Ok(data);
        }
        let (window_start, finish) = fetch_asset_window(path, start, size, bulk);
        let lock = {
            let mut locks = self.locks.lock().unwrap();
            if locks.len() > 1024 {
                locks.retain(|_, value| value.strong_count() != 0);
            }
            let lock_key = format!("data-file:{path}:{window_start}");
            let lock = locks
                .get(&lock_key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            locks.insert(lock_key, Arc::downgrade(&lock));
            lock
        };
        let _guard = lock.lock().unwrap();
        if let Some(data) = self.cached(&key)? {
            return Ok(data);
        }
        // One shared, bounded range per window prevents concurrent neighbouring
        // reads from fetching the same data repeatedly.
        let whole = !self.data_base.starts_with("https://v2.fiveplay.lol/data/")
            && size <= 2 << 20
            && [
                ".fxc", ".xml", ".meta", ".json", ".ymt", ".csv", ".dat", ".gfx", ".txt", ".sps",
                ".cfg",
            ]
            .iter()
            .any(|extension| path.ends_with(extension));
        let origin = if whole { 0 } else { window_start };
        FETCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let network_started = std::time::Instant::now();
        let bytes = self.runtime.block_on(async {
            let _permit = self.network.acquire().await?;
            if whole {
                let reply = self.http.get_gzip(url, size as usize).await?;
                ensure!(
                    reply.status.is_success() && reply.body.len() as u64 == size,
                    "Arquivo comprimido truncado ou HTTP {}",
                    reply.status
                );
                return Ok(reply.body);
            }
            self.http.range(url, window_start, finish - 1, size).await
        })?;
        NETWORK_FETCH_NS.fetch_add(
            network_started.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        DECODED_BYTES.fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        let mut first = None;
        let store_started = std::time::Instant::now();
        let blocks: Vec<(String, &[u8])> = bytes
            .chunks(BLOCK as usize)
            .enumerate()
            .map(|(number, chunk)| {
                let at = origin + number as u64 * BLOCK;
                (
                    data_cache_key(&self.data_base, &self.version, path, at, size),
                    chunk,
                )
            })
            .collect();
        let cache = self.cache.read().unwrap();
        cache.put_batch(&blocks)?;
        cache.unpin_all();
        drop(cache);
        DISK_STORE_NS.fetch_add(
            store_started.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        for (number, (block_key, chunk)) in blocks.into_iter().enumerate() {
            let at = origin + number as u64 * BLOCK;
            let data = Arc::new(chunk.to_vec());
            if at == start {
                first = Some(data.clone());
            }
            self.remember(block_key, data);
        }
        first.context("Intervalo remoto vazio")
    }
    fn hot(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.hot.lock().unwrap().get(key)
    }
    fn remember(&self, key: String, data: Arc<Vec<u8>>) {
        self.hot.lock().unwrap().insert(key, data);
    }
    fn fetch(&self, key: &str, url: &str, range: Option<(u64, u64, u64)>) -> Result<Arc<Vec<u8>>> {
        if let Some(data) = self.hot(key) {
            return Ok(data);
        }
        let lock = {
            let mut locks = self.locks.lock().unwrap();
            if locks.len() > 1024 {
                locks.retain(|_, value| value.strong_count() != 0);
            }
            let lock = locks
                .get(key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            locks.insert(key.to_string(), Arc::downgrade(&lock));
            lock
        };
        let _guard = lock.lock().unwrap();
        if let Some(data) = self.hot(key) {
            return Ok(data);
        }
        let cached = {
            let cache = self.cache.read().unwrap();
            let result = cache.read(key, 8 << 20);
            cache.unpin_all();
            result?
        };
        let data = if let Some(bytes) = cached {
            DISK_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            bytes
        } else {
            FETCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let bytes = self.runtime.block_on(async {
                let _permit = self.network.acquire().await?;
                if let Some((start, end, total)) = range {
                    return self.http.range(url, start, end, total).await;
                }
                let reply = self.http.get_gzip(url, 8 << 20).await?;
                ensure!(reply.status.is_success(), "Asset HTTP {}", reply.status);
                Ok(reply.body)
            })?;
            DECODED_BYTES.fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
            let cache = self.cache.read().unwrap();
            cache.put(key, &bytes)?;
            cache.unpin_all();
            bytes
        };
        let data = Arc::new(data);
        self.remember(key.to_string(), data.clone());
        Ok(data)
    }
    fn pack(&self, record: &Value) -> Result<Arc<Vec<u8>>> {
        let number = record["p"][0].as_u64().context("Pack ausente")? as usize;
        let file = self.index["_packs"][number]["file"]
            .as_str()
            .context("Arquivo de pack ausente")?;
        safe_relative(file)?;
        let url = format!("{}{file}", self.shader_base);
        self.fetch(&format!("{BUILD}:shader:{url}"), &url, None)
    }
}

pub fn shader_text(directory: &Path, hash: &str, constants: bool) -> Result<String> {
    ensure!(
        hash.len() == 16 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
        "Hash de shader inválido"
    );
    let filename = if constants {
        format!("{hash}.consts.json")
    } else {
        format!("{hash}.wgsl")
    };
    if let Ok(path) = rooted_file(directory, &filename) {
        return Ok(std::fs::read_to_string(path)?);
    }
    let assets = ASSETS
        .get()
        .context("Shader local ausente e transporte remoto desativado")?;
    let record = assets.index.get(hash).context("Shader fora do índice")?;
    ensure!(record["ok"] == true, "Shader sem tradução disponível");
    // The v2 index deliberately has valid shaders without a 'p' pack entry
    // (e.g. SSAO::VS_screenTransformSSAO and ped HQ skin copy). The browser
    // worker falls back to /shaders/<hash>.wgsl for those entries; the native
    // runtime must not permanently skip the affected lighting draws.
    if let Some((start, length)) = shader_pack_slice(record, constants) {
        if let Ok(pack) = assets.pack(record) {
            if let Some(end) = start.checked_add(length) {
                if let Some(bytes) = pack.get(start..end) {
                    if let Ok(source) = std::str::from_utf8(bytes) {
                        return Ok(source.to_string());
                    }
                }
            }
        }
    }
    let url = format!("{}{}", assets.shader_base, filename);
    let bytes = assets.fetch(&format!("{BUILD}:shader:{url}"), &url, None)?;
    Ok(std::str::from_utf8(&bytes)?.to_owned())
}

fn shader_pack_slice(record: &Value, constants: bool) -> Option<(usize, usize)> {
    let p = record["p"].as_array()?;
    if p.len() < 4 {
        return None;
    }
    let offset = usize::try_from(p[1].as_u64()?).ok()?;
    let wgsl = usize::try_from(p[2].as_u64()?).ok()?;
    let count = usize::try_from(p[3].as_u64()?).ok()?;
    if constants {
        Some((offset.checked_add(wgsl)?, count))
    } else {
        Some((offset, wgsl))
    }
}

pub fn prefetch_shader(hash: &str) {
    let Some(assets) = ASSETS.get() else { return };
    let Some(record) = assets.index.get(hash).filter(|r| r["ok"] == true).cloned() else {
        return;
    };
    let packed = record["p"][0].as_u64().is_some();
    let key = if packed {
        record["p"][0].to_string()
    } else {
        format!("individual:{hash}")
    };
    {
        let mut pending = assets.prefetching.lock().unwrap();
        if pending.len() >= 8 || !pending.insert(key.clone()) {
            return;
        }
    }
    let owned = assets.clone();
    let hash = hash.to_owned();
    assets.background.spawn(move || {
        if packed {
            let _ = owned.pack(&record);
        } else {
            let url = format!("{}{hash}.wgsl", owned.shader_base);
            let _ = owned.fetch(&format!("{BUILD}:shader:{url}"), &url, None);
            if record["consts"] != Value::Null && record["consts"] != Value::Bool(false) {
                let url = format!("{}{hash}.consts.json", owned.shader_base);
                let _ = owned.fetch(&format!("{BUILD}:shader:{url}"), &url, None);
            }
        }
        owned.prefetching.lock().unwrap().remove(&key);
    });
}

pub fn prefetch_data(path: &str, size: u64, offset: u64, length: u64, low: bool) {
    let Some(assets) = ASSETS.get() else {
        return;
    };
    if length == 0 || offset >= size || safe_relative(path).is_err() {
        return;
    }
    let start = offset / BLOCK * BLOCK;
    let end = offset.saturating_add(length.min(1 << 20)).min(size);
    let end_block = end.saturating_add(BLOCK - 1) / BLOCK;
    let key = format!("{path}:{start}:{end_block}");
    {
        let now = std::time::Instant::now();
        let mut recent = assets.recent_hints.lock().unwrap();
        if recent.len() > 8192 {
            recent.retain(|_, last| now.saturating_duration_since(*last) < HINT_RECENT_TTL);
            if recent.len() > 8192 {
                recent.clear();
            }
        }
        if hint_recently_satisfied(recent.get(&key), now) {
            HINTS_DUPLICATE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    }
    {
        let mut pending = assets.hinting.lock().unwrap();
        if pending.contains_key(&key) {
            HINTS_DUPLICATE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        let low_pending = pending.values().filter(|&&pending_low| pending_low).count();
        let active = LIVE_IO_ACTIVE.load(std::sync::atomic::Ordering::Acquire);
        if !allow_hint(low, active, pending.len(), low_pending)
            || (low && assets.network.available_permits() < 8)
        {
            HINTS_DEFERRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        pending.insert(key.clone(), low);
    }
    HINTS_QUEUED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if low {
        HINTS_LOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    } else {
        HINTS_HIGH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let owned = assets.clone();
    let path = path.to_owned();
    let pending_key = key;
    let job = move || {
        let result = (|| -> Result<()> {
            let _permit = owned.runtime.block_on(owned.hint_slots.acquire())?;
            let url = format!("{}{path}?v={}", owned.data_base, owned.version);
            let mut at = start;
            while at < end {
                owned.data_block(&path, size, at, &url, length >= 256 << 10)?;
                at += BLOCK;
            }
            Ok(())
        })();
        if result.is_ok() {
            HINTS_DONE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            owned
                .recent_hints
                .lock()
                .unwrap()
                .insert(pending_key.clone(), std::time::Instant::now());
        }
        owned.hinting.lock().unwrap().remove(&pending_key);
    };
    if low {
        assets.background.spawn_fifo(job);
    } else {
        // Urgent hints are admitted even when live I/O is saturated; low
        // priority speculative requests have a separate strict admission cap.
        assets.background.spawn(job);
    }
}

pub fn read_data_chunks(
    path: &str,
    size: u64,
    offset: u64,
    length: u64,
    mut consume: impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<()> {
    safe_relative(path)?;
    ensure!(
        length <= 16 << 20 && offset <= size && length <= size - offset,
        "Intervalo de leitura inválido"
    );
    let assets = ASSETS.get().context("Transporte remoto desativado")?;
    let _live = LiveRead::begin();
    let url = format!("{}{path}?v={}", assets.data_base, assets.version);
    let end = offset + length;
    let mut position = offset;
    while position < end {
        let start = position / BLOCK * BLOCK;
        let finish = (start + BLOCK).min(size);
        let data = assets.data_block(path, size, start, &url, length >= 256 << 10)?;
        ensure!(
            data.len() as u64 == finish - start,
            "Bloco de dados truncado"
        );
        let take = (end - position).min(finish - position) as usize;
        let from = (position - start) as usize;
        consume(position - offset, &data[from..from + take])?;
        position += take as u64;
    }
    Ok(())
}

pub fn enabled() -> bool {
    ASSETS.get().is_some()
}
