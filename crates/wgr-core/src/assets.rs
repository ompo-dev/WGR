use crate::security::{rooted_file, safe_relative};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;

pub const MAX_EXPANDED: u64 = 4 * 1024 * 1024 * 1024;
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
}
#[derive(serde::Serialize)]
pub struct CacheStats {
    pub bytes: u64,
    pub objects: u64,
    pub limit: u64,
}
impl DiskCache {
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
        db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=NORMAL;PRAGMA journal_size_limit=8388608;CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY,size INTEGER NOT NULL,last_access INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS refs(key TEXT PRIMARY KEY,hash TEXT NOT NULL);CREATE INDEX IF NOT EXISTS cache_lru ON objects(last_access);")?;
        Ok(Self {
            root,
            db: Mutex::new(db),
            limit,
            pinned: Mutex::new(HashSet::new()),
        })
    }
    fn path(&self, hash: &str) -> PathBuf {
        self.root.join("objects").join(&hash[..2]).join(hash)
    }
    pub fn temporary(&self) -> PathBuf {
        self.root
            .join("staging")
            .join(format!("{}.partial", Uuid::new_v4()))
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
        self.lookup(key, false, |path, hash| {
            let file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            ensure!(
                file.metadata()?.len() <= limit as u64,
                "Objeto de cache excede o limite de leitura"
            );
            let mut bytes = Vec::new();
            file.take(limit.saturating_add(1) as u64)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= limit,
                "Objeto de cache excede o limite de leitura"
            );
            Ok((sha256(&bytes) == hash).then_some(bytes))
        })
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
            let path = self.path(&hash);
            if let Some(value) = load(&path, &hash)? {
                db.execute(
                    "UPDATE objects SET last_access=?1 WHERE hash=?2 AND last_access<>?1",
                    params![crate::domain::now(), hash],
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
        let destination = self.path(&hash);
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM objects WHERE hash=?1)",
            [&hash],
            |r| r.get(0),
        )?;
        let pinned = self
            .pinned
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache ocupado"))?;
        let mut used: u64 = db.query_row("SELECT COALESCE(SUM(size),0) FROM objects", [], |r| {
            r.get(0)
        })?;
        if !exists && used.saturating_add(size) > self.limit {
            let candidates: Vec<(String, u64)> = db
                .prepare("SELECT hash,size FROM objects ORDER BY last_access,hash")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (old, bytes) in candidates {
                if used + size <= self.limit {
                    break;
                }
                if pinned.contains(&old) {
                    continue;
                }
                let path = self.path(&old);
                if path.exists() {
                    fs::remove_file(path)?;
                }
                db.execute("DELETE FROM refs WHERE hash=?1", [&old])?;
                db.execute("DELETE FROM objects WHERE hash=?1", [&old])?;
                used = used.saturating_sub(bytes);
            }
            ensure!(
                used + size <= self.limit,
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
        tx.execute("INSERT INTO objects(hash,size,last_access) VALUES(?1,?2,?3) ON CONFLICT(hash) DO UPDATE SET last_access=excluded.last_access",params![hash,size,crate::domain::now()])?;
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
            "SELECT COALESCE(SUM(size),0),count(*) FROM objects",
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
            let p = self.path(&hash);
            if p.is_file() {
                n += fs::metadata(&p)?.len();
                fs::remove_file(p)?;
            }
        }
        db.execute_batch("DELETE FROM refs;DELETE FROM objects;")?;
        Ok(n)
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
