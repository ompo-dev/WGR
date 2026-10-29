//! WGR's bounded native asset transport. No HTML, JavaScript, or browser runtime.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use wgr_core::assets::{sha256, DiskCache};
use wgr_core::security::{rooted_file, safe_relative, HttpClient, NetworkPolicy};

const DISK_BUDGET: u64 = 2 << 30;
const HOT_BUDGET: usize = 48 << 20;
const BLOCK: u64 = 128 << 10;
const BUILD: &str = "8b0b5899ed";
static ASSETS: OnceLock<Arc<Assets>> = OnceLock::new();
static HOT_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DISK_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FETCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DECODED_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HINTS_QUEUED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HINTS_DONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn metrics() -> Value {
    use std::sync::atomic::Ordering::Relaxed;
    serde_json::json!({"hot_hits":HOT_HITS.load(Relaxed),"disk_hits":DISK_HITS.load(Relaxed),"transport_fetches":FETCHES.load(Relaxed),"decoded_asset_bytes":DECODED_BYTES.load(Relaxed),"hints_queued":HINTS_QUEUED.load(Relaxed),"hints_completed":HINTS_DONE.load(Relaxed)})
}
struct Hot {
    entries: HashMap<String, (Arc<Vec<u8>>, u64)>,
    bytes: usize,
    clock: u64,
}
struct Assets {
    runtime: tokio::runtime::Runtime,
    http: HttpClient,
    cache: Mutex<DiskCache>,
    cache_root: PathBuf,
    hot: Mutex<Hot>,
    locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    prefetching: Mutex<HashSet<String>>,
    hinting: Mutex<HashSet<String>>,
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
        .on_thread_start(|| crate::bridge::thread_started("http"))
        .build()?;
    let http = HttpClient::new(NetworkPolicy {
        allowed_origins: vec![
            "https://fiveplay.lol".into(),
            "https://cdn.fiveplay.lol".into(),
        ],
        development_loopback: false,
    });
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
    for (path, url) in [
        (
            data_dir.join("manifest.json"),
            descriptor["assets"]["manifest"]
                .as_str()
                .context("Manifesto dos dados ausente")?
                .to_string(),
        ),
        (
            shader_dir.join("index.json"),
            format!("{}index.json", shader_base),
        ),
        (
            data_dir.join("bootset.json"),
            format!("{}bootset.json", data_base),
        ),
    ] {
        if !path.is_file() {
            eprintln!("[native-assets] metadata {url}");
            let reply = runtime.block_on(http.get(&url, 8 << 20))?;
            ensure!(reply.status.is_success(), "Metadados HTTP {}", reply.status);
            let _: Value = serde_json::from_slice(&reply.body)?;
            let temporary = path.with_extension("partial");
            std::fs::write(&temporary, &reply.body)?;
            std::fs::rename(temporary, path)?;
        }
    }
    let index: Value = serde_json::from_slice(&std::fs::read(shader_dir.join("index.json"))?)?;
    ensure!(
        index.is_object() && index.get("_packs").is_some(),
        "Índice de shaders inválido"
    );
    let manifest: Value = serde_json::from_slice(&std::fs::read(data_dir.join("manifest.json"))?)?;
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
        cache: Mutex::new(DiskCache::open(cache_root.clone(), DISK_BUDGET)?),
        cache_root,
        hot: Mutex::new(Hot {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
        }),
        locks: Mutex::new(HashMap::new()),
        prefetching: Mutex::new(HashSet::new()),
        hinting: Mutex::new(HashSet::new()),
        hint_slots: tokio::sync::Semaphore::new(4),
        background: rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .thread_name(|_| "native-prefetch".into())
            .start_handler(|_| crate::bridge::thread_started("prefetch"))
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
    // The origin's batch response and individual GET disagree for some FXC
    // files (same length, different shader data). Old batches can trap the
    // guest loader. Repair that derived cache once, retaining all map assets.
    let shader_marker = data_dir.join("fxc-get-v1");
    let shader_revision = sha256(format!("{BUILD}:{}", assets.version).as_bytes());
    let repair = std::fs::metadata(&shader_marker)
        .ok()
        .filter(|m| m.len() == 64)
        .and_then(|_| std::fs::read_to_string(&shader_marker).ok())
        .as_deref()
        != Some(shader_revision.as_str());
    assets.prewarm_shaders(files, repair)?;
    if repair {
        let temporary = shader_marker.with_extension("partial");
        std::fs::write(&temporary, &shader_revision)?;
        std::fs::rename(temporary, shader_marker)?;
    }
    assets.prewarm_boot(files, &data_dir.join("bootset.json"))?;
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
    let budget = DISK_BUDGET
        .checked_sub(overhead)
        .filter(|n| *n >= 128 << 20)
        .context("Código compilado excede o orçamento total de cache de 2 GiB")?;
    *assets.cache.lock().unwrap() = DiskCache::open(assets.cache_root.clone(), budget)?;
    eprintln!(
        "[native-assets] cache total <= 2 GiB; dados <= {} MiB; RAM de transporte <= 48 MiB",
        budget >> 20
    );
    Ok(())
}

impl Assets {
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
                    ".meta", ".xml", ".dat", ".ymt", ".txt", ".csv", ".gfx", ".json", ".sps",
                    ".cfg",
                ]
                .iter()
                .any(|e| name.ends_with(e))
            {
                let first = format!("{BUILD}:data:{name}:0:{size}");
                let last = format!("{BUILD}:data:{name}:{}:{size}", (size - 1) / BLOCK * BLOCK);
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
                    while position < end {
                        let finish = (position + (1 << 20)).min(end);
                        ranges.push((name.clone(), size, position, finish));
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
            "[native-assets] boot: {} metadata batches, {} range jobs",
            batches.len(),
            ranges.len()
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
                self.data_block(path,*size,position,*end,&url)?;
                position=(position+BLOCK).min(*end);
            }
            let done=completed.fetch_add(1,std::sync::atomic::Ordering::Relaxed)+1;
            if done%16==0||done==ranges.len(){crate::bridge::emit(serde_json::json!({"type":"game-progress","payload":{"label":format!("Preparando dados do jogo: {done}/{}",ranges.len())}}));}
            Ok(())
        }))?;
        eprintln!("[native-assets] original boot set ready");
        Ok(())
    }
    fn prewarm_shaders(&self, files: &[Value], repair: bool) -> Result<()> {
        use rayon::prelude::*;
        let mut pending = Vec::new();
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
            let first = format!("{BUILD}:data:{path}:0:{size}");
            let last = format!("{BUILD}:data:{path}:{}:{size}", (size - 1) / BLOCK * BLOCK);
            if !repair && self.cached(&first)?.is_some() && self.cached(&last)?.is_some() {
                continue;
            }
            pending.push((path.to_string(), size));
        }
        eprintln!(
            "[native-assets] preparing shaders via canonical GET: {} files, repair={repair}",
            pending.len()
        );
        let completed = std::sync::atomic::AtomicUsize::new(0);
        self.background.install(|| {
            pending.par_iter().try_for_each(|(path, size)| {
                let url = format!("{}{path}?v={}", self.data_base, self.version);
                self.data_block_inner(path, *size, 0, *size, &url, true)?;
                let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if done % 16 == 0 || done == pending.len() {
                    crate::bridge::emit(serde_json::json!({"type":"game-progress","payload":{"label":format!("Preparando shaders: {done}/{}", pending.len())}}));
                }
                Ok::<(), anyhow::Error>(())
            })
        })?;
        eprintln!("[native-assets] shader files ready in persistent cache");
        Ok(())
    }
    fn fetch_batch(&self, files: &[(String, u64)]) -> Result<()> {
        ensure!(
            !files.iter().any(|(path, _)| path.ends_with(".fxc")),
            "FXC requires individual GET"
        );
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
        let cache = self.cache.lock().unwrap();
        let mut position = 0;
        for (path, size) in files {
            for (number, chunk) in bytes[position..position + *size as usize]
                .chunks(BLOCK as usize)
                .enumerate()
            {
                let key = format!("{BUILD}:data:{path}:{}:{size}", number as u64 * BLOCK);
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
    fn cached(&self, key: &str) -> Result<Option<Arc<Vec<u8>>>> {
        if let Some(data) = self.hot(key) {
            HOT_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(Some(data));
        }
        let cache = self.cache.lock().unwrap();
        let result = cache.read(key, 8 << 20);
        cache.unpin_all();
        drop(cache);
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
        end: u64,
        url: &str,
    ) -> Result<Arc<Vec<u8>>> {
        self.data_block_inner(path, size, start, end, url, false)
    }
    fn data_block_inner(
        &self,
        path: &str,
        size: u64,
        start: u64,
        end: u64,
        url: &str,
        refresh: bool,
    ) -> Result<Arc<Vec<u8>>> {
        let key = format!("{BUILD}:data:{path}:{start}:{size}");
        if !refresh {
            if let Some(data) = self.cached(&key)? {
                return Ok(data);
            }
        }
        let lock = {
            let mut locks = self.locks.lock().unwrap();
            if locks.len() > 1024 {
                locks.retain(|_, value| value.strong_count() != 0);
            }
            let lock_key = format!("data-file:{path}:{start}");
            let lock = locks
                .get(&lock_key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            locks.insert(lock_key, Arc::downgrade(&lock));
            lock
        };
        let _guard = lock.lock().unwrap();
        if !refresh {
            if let Some(data) = self.cached(&key)? {
                return Ok(data);
            }
        }
        // One validated HTTP range for adjacent demand; retain the old 128 KiB
        // persistent block keys so previous downloads remain usable.
        let finish = end
            .div_ceil(BLOCK)
            .saturating_mul(BLOCK)
            .min(size)
            .min(start + (4 << 20));
        let whole = (path.ends_with(".fxc") && size <= 8 << 20)
            || size <= 2 << 20
                && [
                    ".fxc", ".xml", ".meta", ".json", ".ymt", ".csv", ".dat", ".gfx", ".txt",
                    ".sps", ".cfg",
                ]
                .iter()
                .any(|extension| path.ends_with(extension));
        let origin = if whole { 0 } else { start };
        FETCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            self.http.range(url, start, finish - 1, size).await
        })?;
        DECODED_BYTES.fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        let mut first = None;
        let cache = self.cache.lock().unwrap();
        for (number, chunk) in bytes.chunks(BLOCK as usize).enumerate() {
            let at = origin + number as u64 * BLOCK;
            let block_key = format!("{BUILD}:data:{path}:{at}:{size}");
            cache.put(&block_key, chunk)?;
            let data = Arc::new(chunk.to_vec());
            if at == start {
                first = Some(data.clone());
            }
            self.remember(block_key, data);
        }
        cache.unpin_all();
        first.context("Intervalo remoto vazio")
    }
    fn hot(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        let mut hot = self.hot.lock().unwrap();
        hot.clock += 1;
        let clock = hot.clock;
        hot.entries.get_mut(key).map(|(data, age)| {
            *age = clock;
            data.clone()
        })
    }
    fn remember(&self, key: String, data: Arc<Vec<u8>>) {
        if data.len() > HOT_BUDGET {
            return;
        }
        let mut hot = self.hot.lock().unwrap();
        if let Some((old, _)) = hot.entries.remove(&key) {
            hot.bytes -= old.len();
        }
        while hot.bytes + data.len() > HOT_BUDGET {
            let Some(key) = hot
                .entries
                .iter()
                .min_by_key(|(_, (_, age))| *age)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some((old, _)) = hot.entries.remove(&key) {
                hot.bytes -= old.len();
            }
        }
        hot.clock += 1;
        let age = hot.clock;
        hot.bytes += data.len();
        hot.entries.insert(key, (data, age));
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
            let cache = self.cache.lock().unwrap();
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
            let cache = self.cache.lock().unwrap();
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
    let pack = assets.pack(record)?;
    let offset = record["p"][1].as_u64().context("Offset inválido")? as usize;
    let wgsl = record["p"][2].as_u64().context("Comprimento inválido")? as usize;
    let count = record["p"][3].as_u64().context("Constantes inválidas")? as usize;
    let (start, length) = if constants {
        (offset.checked_add(wgsl).context("Overflow de pack")?, count)
    } else {
        (offset, wgsl)
    };
    let end = start.checked_add(length).context("Overflow de pack")?;
    Ok(std::str::from_utf8(pack.get(start..end).context("Pack truncado")?)?.to_string())
}

pub fn prefetch_shader(hash: &str) {
    let Some(assets) = ASSETS.get() else { return };
    let Some(record) = assets.index.get(hash).filter(|r| r["ok"] == true).cloned() else {
        return;
    };
    let key = record["p"][0].to_string();
    {
        let mut pending = assets.prefetching.lock().unwrap();
        if pending.len() >= 8 || !pending.insert(key.clone()) {
            return;
        }
    }
    let owned = assets.clone();
    assets.background.spawn(move || {
        let _ = owned.pack(&record);
        owned.prefetching.lock().unwrap().remove(&key);
    });
}

pub fn prefetch_data(path: &str, size: u64, offset: u64, length: u64, low: bool) {
    let Some(assets) = ASSETS.get() else {
        return;
    };
    if length == 0
        || offset >= size
        || safe_relative(path).is_err()
        || (low && assets.network.available_permits() < 8)
    {
        return;
    }
    let start = offset / BLOCK * BLOCK;
    let end = offset.saturating_add(length.min(1 << 20)).min(size);
    let key = format!("{}:{path}:{start}", if low { "low" } else { "high" });
    {
        let mut pending = assets.hinting.lock().unwrap();
        if pending.len() >= 64
            || (low && pending.iter().filter(|key| key.starts_with("low:")).count() >= 2)
            || !pending.insert(key.clone())
        {
            return;
        }
    }
    HINTS_QUEUED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let owned = assets.clone();
    let path = path.to_owned();
    assets.background.spawn_fifo(move || {
        let result = (|| -> Result<()> {
            let _permit = owned.runtime.block_on(owned.hint_slots.acquire())?;
            let url = format!("{}{path}?v={}", owned.data_base, owned.version);
            let mut at = start;
            while at < end {
                owned.data_block(&path, size, at, end, &url)?;
                at += BLOCK;
            }
            Ok(())
        })();
        if result.is_ok() {
            HINTS_DONE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        owned.hinting.lock().unwrap().remove(&key);
    });
}

pub fn read_data(path: &str, size: u64, offset: u64, length: u64) -> Result<Vec<u8>> {
    safe_relative(path)?;
    ensure!(
        length <= 16 << 20 && offset <= size && length <= size - offset,
        "Intervalo de leitura inválido"
    );
    let assets = ASSETS.get().context("Transporte remoto desativado")?;
    let url = format!("{}{path}?v={}", assets.data_base, assets.version);
    let end = offset + length;
    let mut position = offset;
    let mut output = Vec::with_capacity(length as usize);
    while position < end {
        let start = position / BLOCK * BLOCK;
        let finish = (start + BLOCK).min(size);
        let data = assets.data_block(path, size, start, end, &url)?;
        ensure!(
            data.len() as u64 == finish - start,
            "Bloco de dados truncado"
        );
        let take = (end - position).min(finish - position) as usize;
        let from = (position - start) as usize;
        output.extend_from_slice(&data[from..from + take]);
        position += take as u64;
    }
    Ok(output)
}

pub fn enabled() -> bool {
    ASSETS.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn canonical_shader_refresh_replaces_same_length_legacy_cache() {
        let expected = b"rgxe_WGR_ORIGINAL";
        let legacy = b"rgxe_WGR_BAD_DATA";
        assert_eq!(expected.len(), legacy.len());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Ok((mut socket, _)) = listener.accept() {
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                        .unwrap();
                    let mut request = [0u8; 4096];
                    let bytes = socket.read(&mut request).unwrap();
                    assert!(request[..bytes].starts_with(b"GET /fixture.fxc "));
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        expected.len()
                    )
                    .unwrap();
                    socket.write_all(expected).unwrap();
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "canonical GET was not requested"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let root = std::env::temp_dir().join(format!(
            "wgr-fxc-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        {
            let assets = Assets {
                runtime: tokio::runtime::Runtime::new().unwrap(),
                http: HttpClient::new(NetworkPolicy {
                    allowed_origins: vec![origin.clone()],
                    development_loopback: true,
                }),
                cache: Mutex::new(DiskCache::open(root.clone(), 8 << 20).unwrap()),
                cache_root: root.clone(),
                hot: Mutex::new(Hot {
                    entries: HashMap::new(),
                    bytes: 0,
                    clock: 0,
                }),
                locks: Mutex::new(HashMap::new()),
                prefetching: Mutex::new(HashSet::new()),
                hinting: Mutex::new(HashSet::new()),
                hint_slots: tokio::sync::Semaphore::new(1),
                background: rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .unwrap(),
                network: tokio::sync::Semaphore::new(1),
                index: serde_json::json!({}),
                data_base: format!("{origin}/"),
                shader_base: format!("{origin}/"),
                version: "fixture".into(),
            };
            let key = format!("{BUILD}:data:fixture.fxc:0:{}", legacy.len());
            assets.cache.lock().unwrap().put(&key, legacy).unwrap();
            assert_eq!(assets.cached(&key).unwrap().unwrap().as_slice(), legacy);
            let bytes = assets
                .data_block_inner(
                    "fixture.fxc",
                    expected.len() as u64,
                    0,
                    expected.len() as u64,
                    &format!("{origin}/fixture.fxc"),
                    true,
                )
                .unwrap();
            assert_eq!(bytes.as_slice(), expected);
            assert_eq!(assets.cached(&key).unwrap().unwrap().as_slice(), expected);
            assert!(assets
                .fetch_batch(&[("fixture.fxc".into(), expected.len() as u64)])
                .is_err());
        }
        server.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
