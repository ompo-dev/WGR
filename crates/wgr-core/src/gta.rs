use crate::{
    assets::{DiskCache, sha256},
    security::{HttpClient, NetworkPolicy, safe_relative},
};
use anyhow::{Context, Result, ensure};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex, Semaphore};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ZipEntry {
    pub csize: u64,
    pub usize: u64,
    pub method: u16,
    pub lho: u64,
    #[serde(default)]
    pub crc: Option<u32>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ZipIndex {
    pub url: String,
    pub size: u64,
    pub entries: BTreeMap<String, ZipEntry>,
}
impl ZipIndex {
    pub fn load(path: &Path) -> Result<Self> {
        ensure!(
            std::fs::metadata(path)?.len() <= 8 * 1024 * 1024,
            "Índice ZIP maior que 8 MiB"
        );
        let index: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            index.size > 30 && index.entries.len() <= 100_000,
            "Índice ZIP inválido"
        );
        for (name, entry) in &index.entries {
            safe_relative(name.trim_end_matches('/'))?;
            ensure!(
                matches!(entry.method, 0 | 8)
                    && entry.lho.checked_add(30).is_some_and(|n| n <= index.size)
                    && entry.csize <= index.size
                    && entry.usize <= 4 * 1024 * 1024 * 1024,
                "Entrada ZIP inválida"
            );
        }
        Ok(index)
    }
    pub fn prefix(&self) -> Result<String> {
        let key = self
            .entries
            .keys()
            .find(|name| name.ends_with("/mirror/playgta5.com/index.html"))
            .or_else(|| {
                self.entries
                    .keys()
                    .find(|name| name.contains("/mirror/") && name.ends_with("/index.html"))
            })
            .context("Índice sem raiz do mirror")?;
        Ok(key.trim_end_matches("index.html").to_string())
    }
    pub fn builds(&self) -> Result<Vec<String>> {
        let prefix = self.prefix()?;
        Ok(self
            .entries
            .keys()
            .filter_map(|key| {
                key.strip_prefix(&format!("{prefix}b/"))
                    .and_then(|s| s.split('/').next())
                    .filter(|build| !build.is_empty())
                    .map(str::to_string)
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect())
    }
}

#[derive(Debug, Serialize)]
pub struct GtaAudit {
    pub entries: usize,
    pub zip_size: u64,
    pub builds: Vec<String>,
    pub local_engine: bool,
    pub status: String,
    pub archive_origin: String,
}
pub fn audit(reference: &Path) -> Result<GtaAudit> {
    let index = ZipIndex::load(&reference.join("assets/zip_index.json"))?;
    let builds = index.builds()?;
    let local_engine = reference.join("mirror/playgta5.com").is_dir();
    let origin = url::Url::parse(&index.url)?.origin().ascii_serialization();
    Ok(GtaAudit {
        entries: index.entries.len(),
        zip_size: index.size,
        builds,
        local_engine,
        status: if local_engine {
            "Mirror local encontrado · verificar arquivos".into()
        } else {
            "Aguardando bundle/licença · nenhum game.wasm local".into()
        },
        archive_origin: origin,
    })
}

#[derive(Clone)]
struct InflateCursor {
    decoder: Box<miniz_oxide::inflate::stream::InflateState>,
    encoded: u64,
    pending: Arc<Vec<u8>>,
    consumed: usize,
    total_in: u64,
    total_out: u64,
}
impl InflateCursor {
    fn new() -> Self {
        Self {
            decoder: miniz_oxide::inflate::stream::InflateState::new_boxed(
                miniz_oxide::DataFormat::Raw,
            ),
            encoded: 0,
            pending: Arc::new(vec![]),
            consumed: 0,
            total_in: 0,
            total_out: 0,
        }
    }
}
#[derive(Default)]
struct MemoryRanges {
    bytes: usize,
    entries: BTreeMap<String, Vec<u8>>,
    order: VecDeque<String>,
}
pub struct RemoteArchive {
    pub index: ZipIndex,
    pub prefix: String,
    pub cache: Arc<DiskCache>,
    client: HttpClient,
    locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    ready: Mutex<BTreeMap<String, PathBuf>>,
    offsets: Mutex<BTreeMap<String, (u64, Option<u32>)>>,
    heads: Mutex<VecDeque<(String, Arc<Vec<u8>>)>>,
    checkpoints: Mutex<VecDeque<(String, InflateCursor)>>,
    identity: String,
    slots: Semaphore,
    streams: Mutex<VecDeque<(String, Arc<Mutex<InflateCursor>>)>>,
    memory: Mutex<MemoryRanges>,
    active_reads: Semaphore,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub network_bytes: AtomicU64,
    pub requested_bytes: AtomicU64,
    pub inflated_bytes: AtomicU64,
    pub checkpoint_hits: AtomicU64,
}
impl RemoteArchive {
    pub fn new(index: ZipIndex, policy: NetworkPolicy, cache: Arc<DiskCache>) -> Result<Self> {
        policy.validate(&index.url)?;
        let prefix = index.prefix()?;
        let identity = sha256(&serde_json::to_vec(&index)?);
        Ok(Self {
            index,
            prefix,
            cache,
            client: HttpClient::new(policy),
            locks: Mutex::new(BTreeMap::new()),
            ready: Mutex::new(BTreeMap::new()),
            offsets: Mutex::new(BTreeMap::new()),
            heads: Mutex::new(VecDeque::new()),
            checkpoints: Mutex::new(VecDeque::new()),
            identity,
            slots: Semaphore::new(12),
            streams: Mutex::new(VecDeque::new()),
            memory: Mutex::new(MemoryRanges::default()),
            active_reads: Semaphore::new(12),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            network_bytes: AtomicU64::new(0),
            requested_bytes: AtomicU64::new(0),
            inflated_bytes: AtomicU64::new(0),
            checkpoint_hits: AtomicU64::new(0),
        })
    }
    pub fn member(&self, path: &str) -> Result<(String, ZipEntry)> {
        safe_relative(path)?;
        let name = format!("{}{path}", self.prefix);
        let entry = self
            .index
            .entries
            .get(&name)
            .context("Recurso não está no índice ZIP")?;
        Ok((name, entry.clone()))
    }
    async fn bytes(&self, start: u64, end: u64) -> Result<Vec<u8>> {
        let parts = (start..=end)
            .step_by(128 * 1024)
            .map(|at| async move { self.bytes_piece(at, end.min(at + 128 * 1024 - 1)).await });
        let buffers: Vec<Vec<u8>> = stream::iter(parts).buffered(4).try_collect().await?;
        Ok(buffers.concat())
    }
    async fn bytes_piece(&self, start: u64, end: u64) -> Result<Vec<u8>> {
        let _slot = self.slots.acquire().await?;
        let bytes = self
            .client
            .range(&self.index.url, start, end, self.index.size)
            .await?;
        self.network_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(bytes)
    }
    async fn header(&self, name: &str, entry: &ZipEntry) -> Result<(u64, Option<u32>)> {
        if let Some(value) = self.offsets.lock().await.get(name).copied() {
            return Ok(value);
        }
        // One round trip contains the validated header, filename and first small read.
        let header = self
            .bytes(
                entry.lho,
                entry
                    .lho
                    .saturating_add(30 + name.len() as u64 + 8192 - 1)
                    .min(self.index.size - 1),
            )
            .await?;
        ensure!(
            header[..4] == [0x50, 0x4b, 0x03, 0x04],
            "Assinatura ZIP local inválida"
        );
        let word = |i| u16::from_le_bytes([header[i], header[i + 1]]);
        ensure!(
            word(6) & 1 == 0 && word(8) == entry.method,
            "ZIP criptografado ou método divergente"
        );
        let name_len = word(26) as u64;
        let extra_len = word(28) as u64;
        ensure!(
            name_len > 0 && name_len < 4096 && name_len as usize == name.len(),
            "Nome ZIP inválido"
        );
        let filename = &header[30..30 + name_len as usize];
        ensure!(
            filename == name.as_bytes(),
            "Nome do header ZIP difere do índice"
        );
        let offset = entry
            .lho
            .checked_add(30 + name_len + extra_len)
            .context("Offset ZIP excedido")?;
        ensure!(
            offset
                .checked_add(entry.csize)
                .is_some_and(|n| n <= self.index.size),
            "Entrada fora dos limites do ZIP"
        );
        let crc = u32::from_le_bytes(header[14..18].try_into()?);
        let result = (
            offset,
            entry.crc.or_else(|| (word(6) & 8 == 0).then_some(crc)),
        );
        self.offsets.lock().await.insert(name.to_string(), result);
        let local = (offset - entry.lho) as usize;
        if local < header.len() {
            let end = header.len().min(local + entry.csize as usize);
            let mut heads = self.heads.lock().await;
            if heads.len() >= 256 {
                heads.pop_front();
            }
            heads.push_back((name.into(), Arc::new(header[local..end].to_vec())));
        }
        Ok(result)
    }
    pub async fn read(self: &Arc<Self>, path: &str, start: u64, end: u64) -> Result<Vec<u8>> {
        let (name, entry) = self.member(path)?;
        ensure!(
            start <= end && end < entry.usize && end - start < 8 * 1024 * 1024,
            "Intervalo de membro inválido"
        );
        self.requested_bytes
            .fetch_add(end - start + 1, Ordering::Relaxed);
        if entry.method == 0 {
            let (off, _) = self.header(&name, &entry).await?;
            if let Some((_, bytes)) = self
                .heads
                .lock()
                .await
                .iter()
                .rev()
                .find(|(key, bytes)| key == &name && end < bytes.len() as u64)
            {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(bytes[start as usize..=end as usize].to_vec());
            }
            return self.bytes(off + start, off + end).await;
        }
        if path.starts_with("data/") || entry.usize > 1024 * 1024 || entry.csize > 2 * 1024 * 1024 {
            return self.stream_range(&name, &entry, start, end).await;
        }
        let cached = self.materialize(&name, &entry).await?;
        read_file(&cached, start, end).await
    }
    async fn stream_range(
        &self,
        name: &str,
        entry: &ZipEntry,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>> {
        ensure!(
            entry.usize <= 1024 * 1024 * 1024
                && (entry.usize <= 1024 * 1024 || entry.usize / entry.csize.max(1) <= 500),
            "Membro DEFLATE excede limites de tamanho/expansão"
        );
        const BLOCK: u64 = 4096;
        let first = start / BLOCK * BLOCK;
        let last = ((end + 1).div_ceil(BLOCK) * BLOCK).min(entry.usize) - 1;
        let key = format!("{name}:{first}:{last}");
        {
            let mut memory = self.memory.lock().await;
            if let Some(bytes) = memory.entries.get(&key) {
                let result = bytes[(start - first) as usize..=(end - first) as usize].to_vec();
                if let Some(index) = memory.order.iter().position(|value| value == &key) {
                    memory.order.remove(index);
                }
                memory.order.push_back(key);
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(result);
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let _active = self.active_reads.acquire().await?;
        let stream = {
            let mut streams = self.streams.lock().await;
            if let Some(index) = streams.iter().position(|(value, _)| value == name) {
                let pair = streams.remove(index).expect("existing cursor");
                let stream = pair.1.clone();
                streams.push_back(pair);
                stream
            } else {
                let stream = Arc::new(Mutex::new(InflateCursor::new()));
                if streams.len() == 16 {
                    streams.pop_front();
                }
                streams.push_back((name.into(), stream.clone()));
                stream
            }
        };
        let mut cursor = stream.lock().await;
        if let Some(bytes) = self.memory.lock().await.entries.get(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(bytes[(start - first) as usize..=(end - first) as usize].to_vec());
        }
        if first < cursor.total_out {
            *cursor = InflateCursor::new();
        }
        {
            let checkpoints = self.checkpoints.lock().await;
            if let Some((_, checkpoint)) = checkpoints
                .iter()
                .filter(|(key, value)| key == name && value.total_out <= first)
                .max_by_key(|(_, value)| value.total_out)
                .filter(|(_, value)| value.total_out > cursor.total_out)
            {
                *cursor = checkpoint.clone();
                self.checkpoint_hits.fetch_add(1, Ordering::Relaxed);
            }
        }
        let (offset, _crc) = self.header(name, entry).await?;
        if cursor.encoded == 0
            && let Some((_, bytes)) = self
                .heads
                .lock()
                .await
                .iter()
                .rev()
                .find(|(key, _)| key == name)
        {
            cursor.pending = bytes.clone();
            cursor.encoded = bytes.len() as u64;
        }
        let mut output = Vec::with_capacity((last - first + 1) as usize);
        let mut scratch = vec![0u8; 65536];
        let mut checkpoint_at = cursor.total_out / (1024 * 1024);
        while cursor.total_out <= last {
            if cursor.consumed == cursor.pending.len() && cursor.encoded < entry.csize {
                let count =
                    (entry.csize - cursor.encoded).min(if last - cursor.total_out < 65536 {
                        16 * 1024
                    } else {
                        1024 * 1024
                    });
                cursor.pending = Arc::new(
                    self.bytes(offset + cursor.encoded, offset + cursor.encoded + count - 1)
                        .await?,
                );
                cursor.encoded += count;
                cursor.consumed = 0;
            }
            let before_out = cursor.total_out;
            let capacity = scratch.len().min((last + 1 - before_out) as usize);
            let InflateCursor {
                decoder,
                pending,
                consumed,
                ..
            } = &mut *cursor;
            let result = miniz_oxide::inflate::stream::inflate(
                decoder,
                &pending[*consumed..],
                &mut scratch[..capacity],
                miniz_oxide::MZFlush::None,
            );
            ensure!(
                result.status.is_ok() || result.status == Err(miniz_oxide::MZError::Buf),
                "DEFLATE inválido: {:?}",
                result.status
            );
            let used = result.bytes_consumed;
            let produced = result.bytes_written;
            *consumed += used;
            cursor.total_in += used as u64;
            cursor.total_out += produced as u64;
            self.inflated_bytes
                .fetch_add(produced as u64, Ordering::Relaxed);
            if before_out + produced as u64 > first {
                let skip = first.saturating_sub(before_out) as usize;
                output.extend_from_slice(&scratch[skip..produced]);
            }
            if cursor.total_out / (1024 * 1024) > checkpoint_at {
                checkpoint_at = cursor.total_out / (1024 * 1024);
                let mut snapshot = cursor.clone();
                snapshot.encoded = snapshot.total_in;
                snapshot.pending = Arc::new(vec![]);
                snapshot.consumed = 0;
                let mut checkpoints = self.checkpoints.lock().await;
                checkpoints
                    .retain(|(key, value)| key != name || value.total_out != snapshot.total_out);
                if checkpoints.len() >= 128 {
                    checkpoints.pop_front();
                }
                checkpoints.push_back((name.into(), snapshot));
            }
            if result.status == Ok(miniz_oxide::MZStatus::StreamEnd) {
                ensure!(cursor.total_out > last, "ZIP truncado durante streaming");
                break;
            }
            if used == 0 && produced == 0 {
                ensure!(
                    cursor.consumed == cursor.pending.len() && cursor.encoded < entry.csize,
                    "DEFLATE sem progresso ou truncado"
                );
            }
        }
        ensure!(
            output.len() == (last - first + 1) as usize,
            "Intervalo ZIP incompleto"
        );
        let result = output[(start - first) as usize..=(end - first) as usize].to_vec();
        let mut memory = self.memory.lock().await;
        if let Some(old) = memory.entries.remove(&key) {
            memory.bytes -= old.len();
            memory.order.retain(|value| value != &key);
        }
        while memory.bytes + output.len() > 128 * 1024 * 1024 || memory.entries.len() >= 8192 {
            let Some(old) = memory.order.pop_front() else {
                break;
            };
            if let Some(bytes) = memory.entries.remove(&old) {
                memory.bytes -= bytes.len();
            }
        }
        memory.bytes += output.len();
        memory.entries.insert(key.clone(), output);
        memory.order.push_back(key);
        Ok(result)
    }
    pub async fn materialize(self: &Arc<Self>, name: &str, entry: &ZipEntry) -> Result<PathBuf> {
        ensure!(
            entry.usize <= 1024 * 1024 && entry.csize <= 2 * 1024 * 1024,
            "Objeto grande exige streaming em memória"
        );
        let key = format!(
            "zip:{}:{}:{}:{}:{}",
            self.identity, name, entry.lho, entry.csize, entry.usize
        );
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        if let Some(path) = self.ready.lock().await.get(&key).cloned()
            && tokio::fs::metadata(&path)
                .await
                .is_ok_and(|m| m.len() == entry.usize)
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(path);
        }
        let cache = self.cache.clone();
        let lookup = key.clone();
        if let Some(path) = tokio::task::spawn_blocking(move || cache.get(&lookup)).await?? {
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.ready.lock().await.insert(key.clone(), path.clone());
            return Ok(path);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        ensure!(
            entry.usize <= 1024 * 1024 * 1024
                && (entry.usize <= 1024 * 1024 || entry.usize / entry.csize.max(1) <= 500),
            "Membro excede limites de descompressão (1 GiB / taxa 500)"
        );
        let (offset, crc) = self.header(name, entry).await?;
        let compressed = self.cache.temporary();
        let expanded = self.cache.temporary();
        let outcome = async {
            let mut output = tokio::fs::File::create(&compressed).await?;
            use tokio::io::AsyncWriteExt;
            let mut cursor = 0;
            while cursor < entry.csize {
                let count = (entry.csize - cursor).min(4 * 1024 * 1024);
                let bytes = self
                    .bytes(offset + cursor, offset + cursor + count - 1)
                    .await?;
                output.write_all(&bytes).await?;
                cursor += count;
            }
            output.sync_all().await?;
            drop(output);
            let input = compressed.clone();
            let target = expanded.clone();
            let entry = entry.clone();
            let cache = self.cache.clone();
            let key = key.clone();
            tokio::task::spawn_blocking(move || -> Result<PathBuf> {
                let file = std::fs::File::open(input)?;
                let mut decoder: Box<dyn Read> = if entry.method == 8 {
                    Box::new(flate2::read::DeflateDecoder::new(file))
                } else {
                    Box::new(file)
                };
                let mut output = std::fs::File::create(&target)?;
                let mut buffer = [0; 65536];
                let mut total = 0u64;
                let mut hash = crc32fast::Hasher::new();
                loop {
                    let n = decoder.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    total += n as u64;
                    ensure!(total <= entry.usize, "ZIP excede tamanho declarado");
                    hash.update(&buffer[..n]);
                    output.write_all(&buffer[..n])?;
                }
                ensure!(total == entry.usize, "Membro ZIP truncado");
                if let Some(crc) = crc {
                    ensure!(crc == hash.finalize(), "CRC32 divergente");
                }
                output.sync_all()?;
                drop(output);
                cache.promote(&key, &target)
            })
            .await?
        }
        .await;
        let _ = tokio::fs::remove_file(compressed).await;
        let _ = tokio::fs::remove_file(expanded).await;
        if let Ok(path) = &outcome {
            self.ready.lock().await.insert(key, path.clone());
        }
        outcome
    }
    pub fn batch_key(version: &str, raw_body: &[u8]) -> String {
        use sha1::Digest;
        let mut h = sha1::Sha1::new();
        h.update(version);
        h.update(b":");
        h.update(raw_body);
        format!("{:x}", h.finalize())
    }
    pub async fn cached_batch(&self, hash: &str) -> Option<Vec<u8>> {
        self.memory
            .lock()
            .await
            .entries
            .get(&format!("batch:{hash}"))
            .cloned()
    }
    pub async fn cache_batch(&self, hash: &str, bytes: &[u8]) {
        let key = format!("batch:{hash}");
        let mut memory = self.memory.lock().await;
        if let Some(old) = memory.entries.remove(&key) {
            memory.bytes -= old.len();
            memory.order.retain(|value| value != &key);
        }
        while memory.bytes + bytes.len() > 128 * 1024 * 1024 || memory.entries.len() >= 128 {
            let Some(old) = memory.order.pop_front() else {
                break;
            };
            if let Some(bytes) = memory.entries.remove(&old) {
                memory.bytes -= bytes.len();
            }
        }
        memory.bytes += bytes.len();
        memory.entries.insert(key.clone(), bytes.to_vec());
        memory.order.push_back(key);
    }
    pub fn namespace(&self) -> String {
        self.identity.clone()
    }
}
pub async fn read_file(path: &Path, start: u64, end: u64) -> Result<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    ensure!(
        end >= start && end - start < 8 * 1024 * 1024,
        "Intervalo local inválido"
    );
    let mut f = tokio::fs::File::open(path).await?;
    f.seek(std::io::SeekFrom::Start(start)).await?;
    let mut bytes = vec![0; (end - start + 1) as usize];
    f.read_exact(&mut bytes).await?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn directory_entry_is_not_a_build_id() -> anyhow::Result<()> {
        let index: super::ZipIndex =
            serde_json::from_str(include_str!("../../../assets/gta/zip_index.json"))?;
        assert_eq!(index.builds()?, vec!["8b0b5899ed"]);
        Ok(())
    }
}
