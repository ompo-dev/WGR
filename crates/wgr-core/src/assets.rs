use crate::security::{rooted_file, safe_relative};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub const MAX_EXPANDED: u64 = 4 * 1024 * 1024 * 1024;
static CACHE_ACCESS_CLOCK: AtomicI64 = AtomicI64::new(0);

// Previous cache files recorded seconds since epoch; this monotonic
// microsecond-scale timestamp is compatible with the same SQLite INTEGER
// column, while ensuring two hot objects touched in one second have an
// unambiguous order. Legacy seconds naturally rank as older than new reads.
fn next_cache_access_stamp() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    let mut previous = CACHE_ACCESS_CLOCK.load(Ordering::Relaxed);
    loop {
        let next = now.max(previous.saturating_add(1));
        match CACHE_ACCESS_CLOCK.compare_exchange_weak(
            previous,
            next,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return next,
            Err(latest) => previous = latest,
        }
    }
}
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_hash(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Persistent content-addressed cache. Neither user bundles nor saves live here.
pub struct DiskCache {
    root: PathBuf,
    db: Mutex<Connection>,
    limit: u64,
    pinned: Mutex<HashSet<String>>,
    // Defer repetitive cache-LRU writes into short SQLite transactions.
    // Reads still validate the current key and SHA-256 before returning.
    accessed: Mutex<HashMap<String, i64>>,
    read_lock_wait_ns: AtomicU64,
    read_verify_ns: AtomicU64,
    lru_batch_count: AtomicU64,
    lru_entries_written: AtomicU64,
}
#[derive(serde::Serialize)]
pub struct CacheStats {
    pub bytes: u64,
    pub objects: u64,
    pub limit: u64,
}
impl DiskCache {
    /// Reconfigure an existing cache budget without reopening SQLite. The
    /// previous implementation recreated the connection after GPU boot and
    /// rescanned staging, while background archive prefetch had already
    /// started. Future writes enforce the new limit exactly as on open.
    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    pub fn open(root: PathBuf, limit: u64) -> Result<Self> {
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("staging"))?;
        for entry in fs::read_dir(root.join("staging"))? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".partial"))
                    .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
            {
                fs::remove_file(entry.path())?;
            }
        }
        let db = Connection::open(root.join("cache.sqlite"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        // Evictable downloads only: WAL/NORMAL avoids a disk flush on every LRU
        // touch. A power loss may discard recent cache entries, never saves.
        db.execute_batch("PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA journal_size_limit=8388608;
            CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY,size INTEGER NOT NULL,last_access INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS refs(key TEXT PRIMARY KEY,hash TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS cache_lru ON objects(last_access);
            -- This total is read for every downloaded 128-KiB asset block.
            -- Computing SUM(size) over thousands of files per INSERT made
            -- the entire 32-lane streaming queue serialize on disk writes.
            -- Seed older caches once and maintain the total transactionally.
            CREATE TABLE IF NOT EXISTS cache_totals(id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL);
            INSERT OR IGNORE INTO cache_totals(id,bytes)
                SELECT 1,COALESCE(SUM(size),0) FROM objects;
            CREATE TRIGGER IF NOT EXISTS cache_objects_insert AFTER INSERT ON objects
                BEGIN UPDATE cache_totals SET bytes=bytes+NEW.size WHERE id=1; END;
            CREATE TRIGGER IF NOT EXISTS cache_objects_delete AFTER DELETE ON objects
                BEGIN UPDATE cache_totals SET bytes=bytes-OLD.size WHERE id=1; END;
            CREATE TRIGGER IF NOT EXISTS cache_objects_size AFTER UPDATE OF size ON objects
                BEGIN UPDATE cache_totals SET bytes=bytes+NEW.size-OLD.size WHERE id=1; END;")?;
        Ok(Self {
            root,
            db: Mutex::new(db),
            limit,
            pinned: Mutex::new(HashSet::new()),
            accessed: Mutex::new(HashMap::new()),
            read_lock_wait_ns: AtomicU64::new(0),
            read_verify_ns: AtomicU64::new(0),
            lru_batch_count: AtomicU64::new(0),
            lru_entries_written: AtomicU64::new(0),
        })
    }
    fn path(&self, hash: &str) -> Result<PathBuf> {
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "SHA-256 inválido no índice do cache"
        );
        Ok(self.root.join("objects").join(&hash[..2]).join(hash))
    }
    pub fn temporary(&self) -> PathBuf {
        self.root
            .join("staging")
            .join(format!("{}.partial", Uuid::new_v4()))
    }

    fn flush_accesses(&self, db: &mut Connection) -> Result<()> {
        let mut accessed = self
            .accessed
            .lock()
            .map_err(|_| anyhow::anyhow!("Fila LRU ocupada"))?;
        if accessed.is_empty() {
            return Ok(());
        }
        let tx = db.transaction()?;
        {
            let mut update = tx.prepare_cached(
                "UPDATE objects SET last_access=?1 WHERE hash=?2 AND last_access<>?1",
            )?;
            for (hash, access) in accessed.iter() {
                update.execute(params![access, hash])?;
            }
        }
        tx.commit()?;
        self.lru_batch_count.fetch_add(1, Ordering::Relaxed);
        self.lru_entries_written
            .fetch_add(accessed.len() as u64, Ordering::Relaxed);
        accessed.clear();
        Ok(())
    }

    /// Diagnostic counters are cumulative, including parallel readers.
    /// Milliseconds here are *not* the same as wall-clock game loading time.
    pub fn read_timings(&self) -> (u64, u64, u64, u64) {
        (
            self.read_lock_wait_ns.load(Ordering::Relaxed) / 1_000_000,
            self.read_verify_ns.load(Ordering::Relaxed) / 1_000_000,
            self.lru_batch_count.load(Ordering::Relaxed),
            self.lru_entries_written.load(Ordering::Relaxed),
        )
    }
    pub fn get(&self, key: &str) -> Result<Option<PathBuf>> {
        self.lookup(key, true, |path, hash| {
            Ok(file_hash(path)
                .is_ok_and(|actual| actual == hash)
                .then(|| path.to_path_buf()))
        })
    }
    /// Bounded, verified bytes in one file pass; callers need no path pin.
    pub fn read(&self, key: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        // The original shared SQLite lock covered disk I/O and SHA-256 of
        // the entire 128-KiB object. During gameplay dozens of parallel
        // readers therefore queued on *one* file read/hash. Look up a stable
        // content-addressed hash first; verify bytes outside the DB lock;
        // then revalidate the key before touching the LRU timestamp.
        let waited = Instant::now();
        let hash: Option<String> = {
            let db = self
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
            self.read_lock_wait_ns
                .fetch_add(waited.elapsed().as_nanos() as u64, Ordering::Relaxed);
            db.query_row("SELECT hash FROM refs WHERE key=?1", [key], |r| r.get(0))
                .optional()?
        };
        let Some(hash) = hash else { return Ok(None) };
        let path = self.path(&hash)?;
        let started_verify = Instant::now();
        let bytes = (|| -> Result<Option<Vec<u8>>> {
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            // The bounded take below already rejects an oversized file.
            // Avoid a separate metadata syscall on every content-addressed
            // 128-KiB block; Windows may otherwise incur another filesystem
            // and antivirus roundtrip before it starts reading the bytes.
            let mut bytes = Vec::with_capacity(limit.min(128 << 10));
            file.take(limit.saturating_add(1) as u64)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= limit,
                "Objeto de cache excede o limite de leitura"
            );
            Ok((sha256(&bytes) == hash).then_some(bytes))
        })()?;
        self.read_verify_ns.fetch_add(
            started_verify.elapsed().as_nanos() as u64,
            Ordering::Relaxed,
        );
        let waited = Instant::now();
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        self.read_lock_wait_ns
            .fetch_add(waited.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let current: Option<String> = db
            .query_row("SELECT hash FROM refs WHERE key=?1", [key], |r| r.get(0))
            .optional()?;
        if current.as_deref() != Some(hash.as_str()) {
            // A writer/evictor changed the mapping while the file was being
            // verified. Do not overwrite its new mapping or return old bytes.
            return Ok(None);
        }
        if let Some(bytes) = bytes {
            let should_flush = {
                let mut accessed = self
                    .accessed
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Fila LRU ocupada"))?;
                accessed.insert(hash, next_cache_access_stamp());
                accessed.len() >= 64
            };
            if should_flush {
                self.flush_accesses(&mut db)?;
            }
            return Ok(Some(bytes));
        }
        // A parallel writer may have replaced the damaged/missing file just
        // before we acquired the lock; never delete its repaired contents.
        if path.is_file() && file_hash(&path).is_ok_and(|valid| valid == hash) {
            return Ok(None);
        }
        db.execute("DELETE FROM refs WHERE hash=?1", [&hash])?;
        db.execute("DELETE FROM objects WHERE hash=?1", [&hash])?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(None)
    }
    fn lookup<T>(
        &self,
        key: &str,
        pin: bool,
        load: impl FnOnce(&Path, &str) -> Result<Option<T>>,
    ) -> Result<Option<T>> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        let hash: Option<String> = db
            .query_row("SELECT hash FROM refs WHERE key=?1", [key], |r| r.get(0))
            .optional()?;
        if let Some(hash) = hash {
            let path = self.path(&hash)?;
            if let Some(value) = load(&path, &hash)? {
                db.execute(
                    "UPDATE objects SET last_access=?1 WHERE hash=?2 AND last_access<>?1",
                    params![next_cache_access_stamp(), hash],
                )?;
                if pin {
                    self.pinned
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Cache ocupado"))?
                        .insert(hash);
                }
                return Ok(Some(value));
            }
            db.execute("DELETE FROM refs WHERE hash=?1", [&hash])?;
            db.execute("DELETE FROM objects WHERE hash=?1", [&hash])?;
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        Ok(None)
    }
    pub fn put(&self, key: &str, bytes: &[u8]) -> Result<PathBuf> {
        let temporary = self.temporary();
        fs::write(&temporary, bytes)?;
        match self.promote(key, &temporary) {
            Ok(path) => Ok(path),
            Err(error) => {
                let _ = fs::remove_file(temporary);
                Err(error)
            }
        }
    }

    /// Stores a *bounded* HTTP range (up to 8 blocks of 128 KiB) as one
    /// SQLite transaction, instead of serializing one transaction per block.
    /// Each object remains content-addressed and SHA-256 verified on reads.
    /// Existing single-object writers and the cache's pin/eviction policy
    /// continue to operate on the same schema.
    pub fn put_batch(&self, entries: &[(String, &[u8])]) -> Result<()> {
        ensure!(entries.len() <= 64, "Lote de cache excessivo");
        if entries.is_empty() {
            return Ok(());
        }
        ensure!(
            entries.iter().all(|(_, bytes)| bytes.len() <= 128 << 10),
            "Bloco de cache maior que 128 KiB"
        );
        struct Prepared<'a> {
            key: &'a str,
            hash: String,
            size: u64,
            temporary: PathBuf,
        }
        let mut items = Vec::with_capacity(entries.len());
        let result: Result<()> = (|| {
            for (key, bytes) in entries {
                let size = bytes.len() as u64;
                ensure!(size <= self.limit, "Bloco excede o limite do cache");
                let temporary = self.temporary();
                fs::write(&temporary, bytes)?;
                items.push(Prepared {
                    key,
                    hash: sha256(bytes),
                    size,
                    temporary,
                });
            }
            let mut db = self
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
            // Eviction must see all recent verified reads, even when the
            // normal 64-item batch has not filled yet.
            self.flush_accesses(&mut db)?;
            let mut pinned = self
                .pinned
                .lock()
                .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
            let mut used: u64 =
                db.query_row("SELECT bytes FROM cache_totals WHERE id=1", [], |row| {
                    row.get(0)
                })?;
            let mut requested = HashSet::<String>::new();
            let mut needed = 0u64;
            for item in &items {
                if !requested.insert(item.hash.clone()) {
                    continue;
                }
                let present: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM objects WHERE hash=?1)",
                    [&item.hash],
                    |row| row.get(0),
                )?;
                if !present {
                    needed = needed.saturating_add(item.size);
                }
            }
            ensure!(needed <= self.limit, "Lote excede orçamento do cache");
            if used.saturating_add(needed) > self.limit {
                let candidates = db
                    .prepare("SELECT hash,size FROM objects ORDER BY last_access,hash")?
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (hash, size) in candidates {
                    if used.saturating_add(needed) <= self.limit {
                        break;
                    }
                    if pinned.contains(&hash) || requested.contains(&hash) {
                        continue;
                    }
                    let path = self.path(&hash)?;
                    if path.exists() {
                        fs::remove_file(path)?;
                    }
                    db.execute("DELETE FROM refs WHERE hash=?1", [&hash])?;
                    db.execute("DELETE FROM objects WHERE hash=?1", [&hash])?;
                    used = used.saturating_sub(size);
                }
                ensure!(
                    used.saturating_add(needed) <= self.limit,
                    "Cache em uso pela sessão; não há espaço para este lote"
                );
            }
            let tx = db.transaction()?;
            for item in &items {
                let destination = self.path(&item.hash)?;
                fs::create_dir_all(destination.parent().context("Cache sem diretório")?)?;
                if destination.exists() && file_hash(&destination)? != item.hash {
                    fs::remove_file(&destination)?;
                }
                if !destination.exists() {
                    fs::rename(&item.temporary, &destination)?;
                }
                tx.execute(
                    "INSERT INTO objects(hash,size,last_access) VALUES(?1,?2,?3) ON CONFLICT(hash) DO UPDATE SET last_access=excluded.last_access",
                    params![item.hash,item.size,next_cache_access_stamp()],
                )?;
                tx.execute(
                    "INSERT INTO refs(key,hash) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET hash=excluded.hash",
                    params![item.key,item.hash],
                )?;
            }
            tx.commit()?;
            pinned.extend(requested);
            Ok(())
        })();
        // On success, all staging files were moved or replaced by an existing
        // content-addressed object. On failure remove only our own staging.
        for item in &items {
            let _ = fs::remove_file(&item.temporary);
        }
        result
    }
    pub fn promote(&self, key: &str, temporary: &Path) -> Result<PathBuf> {
        ensure!(
            temporary.parent() == Some(self.root.join("staging").as_path()),
            "Staging de cache inválido"
        );
        let size = fs::metadata(temporary)?.len();
        ensure!(
            size <= self.limit,
            "Objeto maior que o limite do cache; aumente o limite nas configurações"
        );
        let hash = file_hash(temporary)?;
        let destination = self.path(&hash)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        self.flush_accesses(&mut db)?;
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM objects WHERE hash=?1)",
            [&hash],
            |r| r.get(0),
        )?;
        let pinned = self
            .pinned
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        let mut used: u64 = db.query_row("SELECT bytes FROM cache_totals WHERE id=1", [], |r| {
            r.get(0)
        })?;
        if !exists && used.saturating_add(size) > self.limit {
            let candidates: Vec<(String, u64)> = db
                .prepare("SELECT hash,size FROM objects ORDER BY last_access,hash")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (old, bytes) in candidates {
                if used.saturating_add(size) <= self.limit {
                    break;
                }
                if pinned.contains(&old) {
                    continue;
                }
                let path = self.path(&old)?;
                if path.exists() {
                    fs::remove_file(path)?;
                }
                db.execute("DELETE FROM refs WHERE hash=?1", [&old])?;
                db.execute("DELETE FROM objects WHERE hash=?1", [&old])?;
                used = used.saturating_sub(bytes);
            }
            ensure!(
                used.saturating_add(size) <= self.limit,
                "Cache em uso pela sessão; não há espaço para este objeto"
            );
        }
        drop(pinned);
        fs::create_dir_all(destination.parent().context("Cache sem diretório")?)?;
        if destination.exists() && file_hash(&destination)? != hash {
            fs::remove_file(&destination)?;
        }
        if !destination.exists() {
            fs::rename(temporary, &destination)?;
        } else {
            fs::remove_file(temporary)?;
        }
        let tx = db.transaction()?;
        tx.execute("INSERT INTO objects(hash,size,last_access) VALUES(?1,?2,?3) ON CONFLICT(hash) DO UPDATE SET last_access=excluded.last_access",params![hash,size,next_cache_access_stamp()])?;
        tx.execute("INSERT INTO refs(key,hash) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET hash=excluded.hash",params![key,hash])?;
        tx.commit()?;
        self.pinned
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?
            .insert(hash);
        Ok(destination)
    }
    pub fn unpin_all(&self) {
        if let Ok(mut pinned) = self.pinned.lock() {
            pinned.clear();
        }
    }
    pub fn stats(&self) -> Result<CacheStats> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        let (bytes, objects) = db.query_row(
            "SELECT (SELECT bytes FROM cache_totals WHERE id=1), count(*) FROM objects",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(CacheStats {
            bytes,
            objects,
            limit: self.limit,
        })
    }
    pub fn clear(&self) -> Result<u64> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        let pinned = self
            .pinned
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        ensure!(
            pinned.is_empty(),
            "Encerre a sessão antes de limpar o cache"
        );
        let hashes: Vec<String> = db
            .prepare("SELECT hash FROM objects")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut n = 0;
        for hash in hashes {
            let p = self.path(&hash)?;
            if p.is_file() {
                n += fs::metadata(&p)?.len();
                fs::remove_file(p)?;
            }
        }
        db.execute_batch("DELETE FROM refs;DELETE FROM objects;")?;
        self.accessed
            .lock()
            .map_err(|_| anyhow::anyhow!("Fila LRU ocupada"))?
            .clear();
        Ok(n)
    }
}

impl Drop for DiskCache {
    fn drop(&mut self) {
        // Preserve the last partial touch batch across normal clean exits.
        // The cache is disposable; failure to update recency must not panic.
        if let Ok(mut db) = self.db.lock() {
            let _ = self.flush_accesses(&mut db);
        }
    }
}

#[derive(Default, Deserialize)]
pub struct BundleManifest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub entry_url: Option<String>,
    pub requirements: Option<crate::domain::Requirements>,
    pub version: Option<String>,
}
pub struct BundleInfo {
    pub root: PathBuf,
    pub entry: String,
    pub title: String,
    pub description: String,
    pub requirements: crate::domain::Requirements,
    pub version: Option<String>,
    pub size: u64,
}
pub fn inspect_bundle(root: &Path) -> Result<BundleInfo> {
    let root = root.canonicalize().context("Pasta de jogo ausente")?;
    ensure!(root.is_dir(), "Selecione uma pasta de jogo");
    let manifest_path = root.join("wgr.game.json");
    let manifest = if manifest_path.exists() {
        let path = rooted_file(&root, "wgr.game.json")?;
        ensure!(
            fs::metadata(&path)?.len() <= 65536,
            "Manifesto local maior que 64 KiB"
        );
        serde_json::from_slice::<BundleManifest>(&fs::read(path)?)?
    } else {
        BundleManifest::default()
    };
    let entry = manifest.entry_url.unwrap_or_else(|| "index.html".into());
    rooted_file(&root, &entry)?;
    ensure!(
        entry.ends_with(".html") || entry.ends_with(".htm"),
        "Entrada do pacote deve ser HTML"
    );
    let title = manifest.title.unwrap_or_else(|| {
        root.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    ensure!(
        !title.trim().is_empty() && title.len() <= 256,
        "Título do manifesto inválido"
    );
    let size = bundle_size(&root, &root, 0)?;
    Ok(BundleInfo {
        root,
        entry,
        title,
        description: manifest.description.unwrap_or_default(),
        requirements: manifest.requirements.unwrap_or_default(),
        version: manifest.version,
        size,
    })
}
fn bundle_size(root: &Path, folder: &Path, depth: usize) -> Result<u64> {
    ensure!(depth < 64, "Pacote com diretórios profundos demais");
    let mut total = 0u64;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let metadata = entry.file_type()?;
        ensure!(
            !metadata.is_symlink(),
            "Links simbólicos não são aceitos no pacote"
        );
        let path = entry.path();
        ensure!(
            path.canonicalize()?.starts_with(root),
            "Reparse point fora da pasta do jogo"
        );
        total = total
            .checked_add(if metadata.is_dir() {
                bundle_size(root, &path, depth + 1)?
            } else {
                entry.metadata()?.len()
            })
            .context("Tamanho do pacote excedido")?;
    }
    Ok(total)
}
pub fn import_zip(zip_path: &Path, bundles: &Path) -> Result<PathBuf> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?)?;
    ensure!(archive.len() <= 50_000, "ZIP com arquivos demais");
    let stage = bundles.join(format!("{}.partial", Uuid::new_v4()));
    fs::create_dir_all(&stage)?;
    let result = (|| -> Result<PathBuf> {
        let mut expanded = 0u64;
        let mut paths = HashSet::new();
        for i in 0..archive.len() {
            let mut member = archive.by_index(i)?;
            let relative = safe_relative(member.name().trim_end_matches('/'))?;
            ensure!(
                paths.insert(relative.to_string_lossy().to_ascii_lowercase()),
                "ZIP contém nomes duplicados/ambíguos"
            );
            ensure!(
                !member.is_symlink() && !member.encrypted(),
                "ZIP com symlink ou criptografia não suportado"
            );
            expanded = expanded
                .checked_add(member.size())
                .context("Tamanho ZIP excedido")?;
            ensure!(
                expanded <= MAX_EXPANDED && member.size() <= 1024 * 1024 * 1024,
                "ZIP excede limite de expansão (4 GiB total / 1 GiB por arquivo)"
            );
            ensure!(
                member.size() <= 1024 * 1024
                    || member.size() / member.compressed_size().max(1) <= 500,
                "ZIP com taxa de expansão perigosa"
            );
            let target = stage.join(relative);
            if member.is_dir() {
                fs::create_dir_all(&target)?;
                continue;
            }
            fs::create_dir_all(target.parent().context("ZIP sem diretório")?)?;
            let mut output = File::create(&target)?;
            let size = member.size();
            let copied = std::io::copy(&mut (&mut member).take(size + 1), &mut output)?;
            ensure!(copied == size, "ZIP truncado ou expansão divergente");
            output.flush()?;
        }
        let effective = if stage.join("index.html").exists() || stage.join("wgr.game.json").exists()
        {
            stage.clone()
        } else {
            let dirs: Vec<_> = fs::read_dir(&stage)?.collect::<std::io::Result<_>>()?;
            ensure!(
                dirs.len() == 1 && dirs[0].path().is_dir(),
                "ZIP precisa conter index.html ou uma única pasta de jogo"
            );
            dirs[0].path()
        };
        inspect_bundle(&effective)?;
        let destination = bundles.join(Uuid::new_v4().to_string());
        let sub = effective.strip_prefix(&stage)?.to_path_buf();
        fs::rename(&stage, &destination)?;
        Ok(destination.join(sub))
    })();
    if result.is_err() {
        // Only this freshly generated staging directory is removed.
        if stage.parent() == Some(bundles) && stage.extension().is_some_and(|e| e == "partial") {
            let _ = crate::security::remove_owned_directory(bundles, &stage);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manual, synthetic I/O comparison. Not a substitute for GTA cold/warm
    /// scene timings: intentionally ignored by ordinary CI tests.
    #[test]
    #[ignore = "perfil de cache: benchmark manual de I/O local"]
    fn compare_verified_lru_reads_under_contention() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache = std::sync::Arc::new(DiskCache::open(temp.path().into(), 32 << 20)?);
        let mut keys = Vec::new();
        for chunk in 0..12 {
            let owned: Vec<(String, Vec<u8>)> = (0..8)
                .map(|n| {
                    let k = chunk * 8 + n;
                    let bytes: Vec<u8> = (0..128 << 10)
                        .map(|j| ((j * 37 + k * 113 + j / 79) % 251) as u8)
                        .collect();
                    (format!("block-{k}"), bytes)
                })
                .collect();
            let borrowed: Vec<(String, &[u8])> = owned
                .iter()
                .map(|(key, value)| (key.clone(), value.as_slice()))
                .collect();
            cache.put_batch(&borrowed)?;
            cache.unpin_all();
            keys.extend(owned.into_iter().map(|(key, _)| key));
        }
        let compare = |old: bool| {
            let started = Instant::now();
            std::thread::scope(|scope| {
                for worker in 0..16 {
                    let cache = cache.clone();
                    let keys = &keys;
                    scope.spawn(move || {
                        for n in 0..450usize {
                            let key = &keys[(n * 17 + worker * 13) % keys.len()];
                            if old {
                                // Reference implementation from 7e79295:
                                // identical two SQL lookups, verify each
                                // 128-KiB file, immediate LRU update.
                                let hash: String = cache
                                    .db
                                    .lock()
                                    .unwrap()
                                    .query_row("SELECT hash FROM refs WHERE key=?1", [key], |r| r.get(0))
                                    .unwrap();
                                let path = cache.path(&hash).unwrap();
                                let file = File::open(path).unwrap();
                                assert!(file.metadata().unwrap().len() <= 128 << 10);
                                let mut bytes = Vec::new();
                                file.take((128 << 10) + 1).read_to_end(&mut bytes).unwrap();
                                assert_eq!(sha256(&bytes), hash);
                                let db = cache.db.lock().unwrap();
                                let current: String = db
                                    .query_row("SELECT hash FROM refs WHERE key=?1", [key], |r| r.get(0))
                                    .unwrap();
                                assert_eq!(hash, current);
                                db.execute(
                                    "UPDATE objects SET last_access=?1 WHERE hash=?2 AND last_access<>?1",
                                    params![crate::domain::now(), hash],
                                )
                                .unwrap();
                            } else {
                                assert_eq!(cache.read(key, 128 << 10).unwrap().unwrap().len(), 128 << 10);
                            }
                        }
                    });
                }
            });
            started.elapsed()
        };
        let legacy_a = compare(true);
        let new = compare(false);
        let legacy_b = compare(true);
        eprintln!(
            "cache synthetic benchmark: 7200 verified reads, legacy A={:.2}s, batched={:.2}s, legacy B={:.2}s",
            legacy_a.as_secs_f64(),
            new.as_secs_f64(),
            legacy_b.as_secs_f64()
        );
        Ok(())
    }
    #[test]
    fn batched_lru_touches_are_applied_before_eviction() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache = DiskCache::open(temp.path().into(), 8)?;
        cache.put("old", b"aaaa")?;
        cache.put("recent", b"bbbb")?;
        cache.unpin_all();
        // Force deterministic recency ordering instead of sleeping for a
        // wall-clock second (crate::domain::now uses seconds).
        {
            let db = cache.db.lock().unwrap();
            db.execute(
                "UPDATE objects SET last_access=1 WHERE hash=?1",
                [sha256(b"bbbb")],
            )?;
            db.execute(
                "UPDATE objects SET last_access=0 WHERE hash=?1",
                [sha256(b"aaaa")],
            )?;
        }
        assert_eq!(cache.read("old", 4)?, Some(b"aaaa".to_vec()));
        assert_eq!(cache.read_timings().2, 0, "one touch stays pending");
        cache.put("new", b"cccc")?;
        assert_eq!(cache.read("old", 4)?, Some(b"aaaa".to_vec()));
        assert_eq!(
            cache.read("recent", 4)?,
            None,
            "untouched older object evicted"
        );
        assert_eq!(cache.read("new", 4)?, Some(b"cccc".to_vec()));
        assert!(cache.read_timings().2 >= 1);
        Ok(())
    }
    #[test]
    fn cache_budget_change_preserves_content_and_enforces_next_eviction() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut cache = DiskCache::open(temp.path().into(), 16)?;
        cache.put("first", b"aaaa")?;
        cache.put("second", b"bbbb")?;
        cache.unpin_all();
        cache.set_limit(8);
        assert_eq!(cache.stats()?.limit, 8);
        assert_eq!(cache.read("first", 4)?, Some(b"aaaa".to_vec()));
        cache.put("third", b"cccc")?;
        assert_eq!(cache.stats()?.bytes, 8);
        assert_eq!(cache.read("first", 4)?, Some(b"aaaa".to_vec()));
        assert!(cache.read("second", 4)?.is_none());
        assert_eq!(cache.read("third", 4)?, Some(b"cccc".to_vec()));
        Ok(())
    }
    #[test]
    fn concurrent_verified_readers_and_batch_writers_keep_cache_consistent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache = std::sync::Arc::new(DiskCache::open(temp.path().into(), 16 << 20)?);
        let contents: Vec<u8> = (0..128 << 10).map(|i| (i % 251) as u8).collect();
        cache.put("constant", &contents)?;
        cache.unpin_all();
        std::thread::scope(|scope| {
            for _ in 0..6 {
                let cache = cache.clone();
                let expected = &contents;
                scope.spawn(move || {
                    for _ in 0..30 {
                        assert_eq!(
                            cache.read("constant", 128 << 10).unwrap().as_deref(),
                            Some(expected.as_slice())
                        );
                    }
                });
            }
            let cache = cache.clone();
            scope.spawn(move || {
                for number in 0..30 {
                    let key = format!("other-{number}");
                    let block = vec![number as u8; 256];
                    cache.put_batch(&[(key, block.as_slice())]).unwrap();
                    cache.unpin_all();
                }
            });
        });
        assert_eq!(cache.read("constant", 128 << 10)?, Some(contents));
        assert!(cache.stats()?.bytes <= cache.stats()?.limit);
        Ok(())
    }
    #[test]
    fn bounded_cache_range_batch_is_atomic_for_refs_and_quota() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache = DiskCache::open(temp.path().into(), 12)?;
        cache.put_batch(&[
            ("a".into(), b"abcd"),
            ("b".into(), b"efgh"),
            ("c".into(), b"abcd"),
        ])?;
        assert_eq!(cache.stats()?.objects, 2, "identical chunks share a hash");
        assert_eq!(cache.stats()?.bytes, 8);
        assert_eq!(cache.read("c", 8)?.as_deref(), Some(&b"abcd"[..]));
        assert!(cache.put_batch(&[("too-big".into(), &[0; 13])]).is_err());
        assert_eq!(cache.stats()?.bytes, 8);
        assert!(
            cache.put_batch(&[("d".into(), b"ijklmn")]).is_err(),
            "pinned batch cannot exceed quota"
        );
        assert_eq!(cache.stats()?.bytes, 8);
        cache.unpin_all();
        cache.put_batch(&[("d".into(), b"ijklmn")])?;
        assert!(cache.stats()?.bytes <= 12);
        assert_eq!(cache.read("d", 12)?.as_deref(), Some(&b"ijklmn"[..]));
        cache.unpin_all();
        cache.put_batch(&[("d".into(), b"ijklmn")])?;
        assert_eq!(
            cache.stats()?.bytes,
            10,
            "reusing a hash cannot double-account bytes"
        );
        Ok(())
    }
    #[test]
    fn cache_total_migrates_and_tracks_evictions_and_corruption() -> Result<()> {
        let temp = tempfile::tempdir()?;
        // Model a database created by a previous WGR version.
        {
            let db = Connection::open(temp.path().join("cache.sqlite"))?;
            db.execute_batch("CREATE TABLE objects(hash TEXT PRIMARY KEY,size INTEGER NOT NULL,last_access INTEGER NOT NULL);
                CREATE TABLE refs(key TEXT PRIMARY KEY,hash TEXT NOT NULL);")?;
        }
        let cache = DiskCache::open(temp.path().into(), 12)?;
        cache.put("first", b"aaaa")?;
        cache.put("second", b"bbbb")?;
        assert_eq!(cache.stats()?.bytes, 8);
        cache.unpin_all();
        cache.put("third", b"cccccccccccc")?;
        assert_eq!(cache.stats()?.bytes, 12);
        assert_eq!(cache.stats()?.objects, 1);
        assert_eq!(cache.read("third", 12)?, Some(b"cccccccccccc".to_vec()));
        drop(cache);
        let cache = DiskCache::open(temp.path().into(), 12)?;
        assert_eq!(
            cache.stats()?.bytes,
            12,
            "reopening must preserve the total"
        );
        cache.unpin_all();
        let path = cache.get("third")?.context("cached object")?;
        cache.unpin_all();
        fs::write(path, b"corrupt")?;
        assert_eq!(cache.read("third", 12)?, None);
        assert_eq!(
            cache.stats()?.bytes,
            0,
            "corrupt deletions update the total"
        );
        cache.put("last", b"ok")?;
        cache.unpin_all();
        assert_eq!(cache.stats()?.bytes, 2);
        cache.clear()?;
        assert_eq!(cache.stats()?.bytes, 0);
        Ok(())
    }
    #[test]
    fn corrupt_cache_hashes_are_rejected_without_poisoning_the_cache() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cache = DiskCache::open(temp.path().into(), 64)?;
        for hash in ["", "a", "é", "../outside", &"g".repeat(64), &"é".repeat(32)] {
            cache.db.lock().unwrap().execute(
                "INSERT OR REPLACE INTO refs(key,hash) VALUES('corrupt',?1)",
                [hash],
            )?;
            assert!(cache.get("corrupt").is_err(), "{hash:?}");
        }
        cache.put("valid", b"original")?;
        assert_eq!(cache.read("valid", 8)?, Some(b"original".to_vec()));
        cache.unpin_all();
        cache.db.lock().unwrap().execute(
            "INSERT INTO objects(hash,size,last_access) VALUES('',64,0)",
            [],
        )?;
        assert!(cache.clear().is_err());
        assert!(cache.put("evict", &[0; 64]).is_err());
        assert!(cache.stats().is_ok(), "erro não envenena o mutex");
        Ok(())
    }

    #[test]
    fn bounded_cache_reads_verify_content_without_pinning_or_deleting_on_limit_error() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let cache = DiskCache::open(temp.path().into(), 8)?;
        let first = cache.put("a", b"1234")?;
        cache.unpin_all();
        assert!(cache.read("a", 3).is_err());
        assert!(first.exists());
        assert_eq!(cache.read("a", 4)?, Some(b"1234".to_vec()));
        let second = cache.put("b", b"56789012")?;
        assert!(!first.exists());
        fs::write(&second, b"bad data")?;
        assert!(cache.read("b", 8)?.is_none());
        assert_eq!(cache.stats()?.objects, 0);
        Ok(())
    }
    #[test]
    fn cache_integrity_quota_and_pins() -> Result<()> {
        let t = tempfile::tempdir()?;
        let c = DiskCache::open(t.path().into(), 6)?;
        let a = c.put("a", b"1234")?;
        assert!(c.put("b", b"5678").is_err());
        c.unpin_all();
        c.put("b", b"5678")?;
        assert!(!a.exists());
        let b = c.get("b")?.context("objeto")?;
        fs::write(&b, b"bad!")?;
        assert!(c.get("b")?.is_none());
        assert_eq!(c.stats()?.objects, 0);
        Ok(())
    }
    #[test]
    fn zip_slip_is_rejected_without_escape() -> Result<()> {
        let t = tempfile::tempdir()?;
        let path = t.path().join("bad.zip");
        let mut z = zip::ZipWriter::new(File::create(&path)?);
        z.start_file("../escaped", zip::write::SimpleFileOptions::default())?;
        z.write_all(b"no")?;
        z.finish()?;
        let bundles = t.path().join("bundles");
        fs::create_dir(&bundles)?;
        assert!(import_zip(&path, &bundles).is_err());
        assert!(!t.path().join("escaped").exists());
        Ok(())
    }
}
