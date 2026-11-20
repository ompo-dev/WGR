//! HTTPFS memory64: manifesto e lanes de leitura de 64 bytes do port.
//! Cada cópia é limitada; nenhum arquivo de jogo inteiro é materializado.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};
use wgr_core::security::{HttpClient, rooted_file, safe_relative};
use wgr_runtime::shared::GuestMemory;

const BLOCK: u64 = 1 << 20;
const MAX_MANIFEST: usize = 8 << 20;
const MAX_LANES: u32 = 256;
const MAX_WORKERS: usize = 4;
const TEMPORARY_BUDGET: usize = MAX_WORKERS * BLOCK as usize;

type Reader = dyn Fn(&str, u64, u64, u64) -> Result<Vec<u8>> + Send + Sync;

#[derive(Deserialize)]
struct Manifest {
    files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
struct ManifestFile(String, u64, #[serde(default)] Option<u64>);

pub struct HttpFs {
    manifest: Vec<u8>,
    files: Vec<(String, u64)>,
    reader: Box<Reader>,
}

impl HttpFs {
    pub fn new(
        bytes: Vec<u8>,
        reader: impl Fn(&str, u64, u64, u64) -> Result<Vec<u8>> + Send + Sync + 'static,
    ) -> Result<Arc<Self>> {
        ensure!(bytes.len() <= MAX_MANIFEST, "Manifesto HTTPFS excede 8 MiB");
        let manifest: Manifest =
            serde_json::from_slice(&bytes).context("Manifesto HTTPFS inválido")?;
        ensure!(
            manifest.files.len() <= 100_000,
            "Manifesto HTTPFS excessivo"
        );
        let mut names = HashSet::new();
        for ManifestFile(name, size, _modified) in &manifest.files {
            safe_relative(name)?;
            ensure!(
                names.insert(name) && *size <= i64::MAX as u64,
                "Entrada HTTPFS duplicada ou excessiva"
            );
        }
        Ok(Arc::new(Self {
            manifest: bytes,
            files: manifest
                .files
                .into_iter()
                .map(|ManifestFile(name, size, _modified)| (name, size))
                .collect(),
            reader: Box::new(reader),
        }))
    }

    pub fn local(root: &Path) -> Result<Arc<Self>> {
        let root = root.canonicalize().context("Pasta HTTPFS ausente")?;
        let path = rooted_file(&root, "manifest.json")?;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_MANIFEST as u64 + 1)
            .read_to_end(&mut bytes)?;
        Self::new(bytes, move |name, size, offset, length| {
            let mut file = std::fs::File::open(rooted_file(&root, name)?)?;
            ensure!(
                file.metadata()?.len() == size,
                "Tamanho local diverge do manifesto HTTPFS"
            );
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; length as usize];
            file.read_exact(&mut bytes)?;
            Ok(bytes)
        })
    }

    /// O runtime deve permanecer vivo enquanto a sessão estiver ativa.
    pub fn remote(
        bytes: Vec<u8>,
        base: &str,
        client: HttpClient,
        runtime: tokio::runtime::Handle,
    ) -> Result<Arc<Self>> {
        let base = client.policy.validate(base)?;
        ensure!(
            base.path().ends_with('/') && base.query().is_none() && base.fragment().is_none(),
            "Base HTTPFS inválida"
        );
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).context("Manifesto HTTPFS inválido")?;
        let version = json
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Self::new(bytes, move |name, size, offset, length| {
            let mut url = base.clone();
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Base HTTPFS sem segmentos"))?
                .pop_if_empty()
                .extend(name.split('/'));
            if let Some(version) = &version {
                url.query_pairs_mut().append_pair("v", version);
            }
            runtime.block_on(client.range(url.as_str(), offset, offset + length - 1, size))
        })
    }

    pub fn manifest(&self, memory: &GuestMemory, address: u64, capacity: u32) -> Result<i32> {
        if capacity as usize > self.manifest.len() {
            ensure!(
                address
                    .checked_add(self.manifest.len() as u64 + 1)
                    .is_some_and(|end| end <= memory.len() as u64),
                "Destino do manifesto fora da memória"
            );
            memory.write(address, &self.manifest)?;
            memory.write(address + self.manifest.len() as u64, &[0])?;
        }
        Ok(self.manifest.len() as i32)
    }

    pub fn read(
        &self,
        memory: &GuestMemory,
        id: usize,
        offset: u64,
        destination: u64,
        requested: u64,
    ) -> Result<u32> {
        let (name, size) = self.files.get(id).context("ID HTTPFS desconhecido")?;
        let length = requested.min(size.saturating_sub(offset));
        ensure!(
            length <= i32::MAX as u64
                && destination
                    .checked_add(length)
                    .is_some_and(|end| end <= memory.len() as u64),
            "Destino HTTPFS fora da memória ou leitura excessiva"
        );
        let mut done = 0;
        while done < length {
            let amount = BLOCK.min(length - done);
            let bytes = (self.reader)(name, *size, offset + done, amount)?;
            ensure!(bytes.len() as u64 == amount, "Leitura HTTPFS truncada");
            memory.write(destination + done, &bytes)?;
            done += amount;
        }
        Ok(length as u32)
    }
}

pub struct IoRing {
    memory: GuestMemory,
    table: u64,
}

impl IoRing {
    pub fn new(memory: GuestMemory, table: u64) -> Result<Self> {
        ensure!(
            table.is_multiple_of(4)
                && table
                    .checked_add(64)
                    .is_some_and(|end| end <= memory.len() as u64),
            "Tabela HTTPFS inválida"
        );
        let ring = Self { memory, table };
        ring.lanes()?;
        Ok(ring)
    }

    fn lanes(&self) -> Result<u32> {
        let count = self.memory.load_u32(self.table + 4)?;
        ensure!(
            count <= MAX_LANES
                && self
                    .table
                    .checked_add(64 + u64::from(count) * 64)
                    .is_some_and(|end| end <= self.memory.len() as u64),
            "Lanes HTTPFS fora da memória"
        );
        Ok(count)
    }

    pub fn pump(&self, filesystem: &HttpFs) -> Result<u32> {
        let mut served = 0;
        for slot in 0..self.lanes()? {
            if self.pump_slot(filesystem, slot)? {
                served += 1;
            }
        }
        Ok(served)
    }

    fn pump_slot(&self, filesystem: &HttpFs, slot: u32) -> Result<bool> {
        let row = self.table + 64 + u64::from(slot) * 64;
        if !self.memory.compare_exchange_u32(row, 1, 3)? {
            return Ok(false);
        }
        let word = |at| self.memory.load_u32(row + at).map(u64::from);
        let result = (|| {
            let id = word(4)? as usize;
            let offset = word(8)? | word(12)? << 32;
            let destination = word(16)? | word(20)? << 32;
            filesystem.read(&self.memory, id, offset, destination, word(24)?)
        })();
        let value = match result {
            Ok(length) => length,
            Err(error) => {
                tracing::warn!("Leitura HTTPFS falhou: {error:#}");
                u32::MAX
            }
        };
        self.memory.store_u32(row + 28, value)?;
        self.memory.store_u32(row, 2)?;
        self.memory.notify(row)?;
        Ok(true)
    }

    fn pump_one(&self, filesystem: &HttpFs) -> Result<bool> {
        for slot in 0..self.lanes()? {
            if self.pump_slot(filesystem, slot)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub(crate) fn spawn(
    memory: GuestMemory,
    table: u64,
    filesystem: Arc<HttpFs>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<Result<()>>> {
    let ring = IoRing::new(memory, table)?;
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(MAX_WORKERS)
        .min(TEMPORARY_BUDGET / BLOCK as usize);
    let shutdown = Arc::new(AtomicBool::new(false));
    Ok(std::thread::Builder::new()
        .name("wgr-httpfs".into())
        .spawn(move || {
            let mut handles = Vec::with_capacity(workers);
            let mut worker_error = None;
            for _ in 0..workers {
                let worker_ring = IoRing {
                    memory: ring.memory.clone(),
                    table: ring.table,
                };
                let worker_filesystem = filesystem.clone();
                let worker_stop = stop.clone();
                let worker_shutdown = shutdown.clone();
                let handle = std::thread::Builder::new()
                    .name("wgr-httpfs-lane".into())
                    .spawn(move || {
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            (|| {
                                while !worker_stop.load(Ordering::Acquire)
                                    && !worker_shutdown.load(Ordering::Acquire)
                                {
                                    let seen = worker_ring.memory.load_u32(table)?;
                                    if worker_ring.pump_one(&worker_filesystem)? {
                                        continue;
                                    }
                                    worker_ring.memory.wait32(
                                        table,
                                        seen,
                                        Duration::from_millis(20),
                                    )?;
                                }
                                Ok::<(), anyhow::Error>(())
                            })()
                        }))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("worker HTTPFS entrou em pânico")));
                        if result.is_err() {
                            worker_shutdown.store(true, Ordering::Release);
                            let _ = worker_ring.memory.notify(table);
                        }
                        result
                    });
                match handle {
                    Ok(handle) => handles.push(handle),
                    Err(error) => {
                        shutdown.store(true, Ordering::Release);
                        let _ = ring.memory.notify(table);
                        for handle in handles {
                            let _ = handle.join();
                        }
                        return Err(error.into());
                    }
                }
            }
            for handle in handles {
                match handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        shutdown.store(true, Ordering::Release);
                        let _ = ring.memory.notify(table);
                        if worker_error.is_none() {
                            worker_error = Some(error);
                        }
                    }
                    Err(error) => {
                        shutdown.store(true, Ordering::Release);
                        let _ = ring.memory.notify(table);
                        if worker_error.is_none() {
                            worker_error =
                                Some(anyhow::anyhow!("worker HTTPFS entrou em pânico: {error:?}"));
                        }
                    }
                }
            }
            if let Some(error) = worker_error {
                return Err(error);
            }
            Ok(())
        })?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn memory(pages: u64) -> GuestMemory {
        let runtime = wgr_runtime::NativeRuntime::new().unwrap();
        GuestMemory::new64(runtime.engine(), pages, pages, pages * 65536).unwrap()
    }

    #[test]
    fn validates_manifest_names_sizes_and_duplicates() {
        for bytes in [
            r#"{}"#,
            r#"{"files":[["../escape",1]]}"#,
            r#"{"files":[["ok",1],["ok",2]]}"#,
            r#"{"files":[["ok",-1]]}"#,
            r#"{"files":[["ok",18446744073709551615]]}"#,
        ] {
            assert!(HttpFs::new(bytes.as_bytes().to_vec(), |_, _, _, _| unreachable!()).is_err());
        }
        assert!(
            HttpFs::new(
                br#"{"files":[["original.bin",8,1790860560]]}"#.to_vec(),
                |_, _, _, _| unreachable!()
            )
            .is_ok()
        );
    }

    #[test]
    fn manifest_copy_uses_strict_capacity_and_checks_before_writing() {
        let bytes = br#"{"files":[["original.bin",8]]}"#;
        let fs = HttpFs::new(bytes.to_vec(), |_, _, _, _| unreachable!()).unwrap();
        let memory = memory(1);
        memory.write(128, &[0xab; 64]).unwrap();
        assert_eq!(
            fs.manifest(&memory, u64::MAX, 0).unwrap(),
            bytes.len() as i32
        );
        fs.manifest(&memory, 128, bytes.len() as u32).unwrap();
        assert_eq!(memory.read(128, 1).unwrap(), [0xab]);
        fs.manifest(&memory, 128, bytes.len() as u32 + 1).unwrap();
        assert_eq!(memory.read(128, bytes.len()).unwrap(), bytes);
        assert_eq!(memory.read(128 + bytes.len() as u64, 1).unwrap(), [0]);
        assert!(fs.manifest(&memory, u64::MAX, 100).is_err());
    }

    #[test]
    fn copies_large_reads_in_bounded_blocks_and_clamps_at_eof() {
        let calls = Arc::new(AtomicU64::new(0));
        let observed = calls.clone();
        let fs = HttpFs::new(
            br#"{"files":[["original.bin",3145728]]}"#.to_vec(),
            move |_, _, offset, length| {
                assert!(length <= BLOCK);
                observed.fetch_add(1, Ordering::Relaxed);
                Ok((offset..offset + length).map(|n| (n % 251) as u8).collect())
            },
        )
        .unwrap();
        let memory = memory(64);
        assert!(fs.read(&memory, 0, 0, u64::MAX, 2).is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(fs.read(&memory, 0, 7, 128, 3 << 20).unwrap(), (3 << 20) - 7);
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        for offset in [0, BLOCK - 1, BLOCK, (3 << 20) - 8] {
            assert_eq!(
                memory.read(128 + offset, 1).unwrap(),
                [((offset + 7) % 251) as u8]
            );
        }
        assert_eq!(fs.read(&memory, 0, u64::MAX, 0, 100).unwrap(), 0);
        assert!(fs.read(&memory, 99, 0, 0, 1).is_err());
    }

    #[test]
    fn lanes_complete_success_and_failure_without_repeating_a_request() {
        let memory = memory(1);
        memory.store_u32(68, 2).unwrap();
        let fs = HttpFs::new(
            br#"{"files":[["original.bin",8]]}"#.to_vec(),
            |_, _, offset, length| {
                Ok(b"original"[offset as usize..(offset + length) as usize].to_vec())
            },
        )
        .unwrap();
        let ring = IoRing::new(memory.clone(), 64).unwrap();
        for (row, id) in [(128, 99), (192, 0)] {
            for (at, value) in [(4, id), (8, 2), (16, 1024), (24, 100)] {
                memory.store_u32(row + at, value).unwrap();
            }
            memory.store_u32(row, 1).unwrap();
        }
        assert_eq!(ring.pump(&fs).unwrap(), 2);
        assert_eq!(memory.load_u32(128).unwrap(), 2);
        assert_eq!(memory.load_u32(156).unwrap(), u32::MAX);
        assert_eq!(memory.load_u32(220).unwrap(), 6);
        assert_eq!(memory.read(1024, 6).unwrap(), b"iginal");
        assert_eq!(ring.pump(&fs).unwrap(), 0);
        memory.store_u32(68, u32::MAX).unwrap();
        assert!(ring.pump(&fs).is_err());
        assert!(IoRing::new(memory.clone(), 1).is_err());
        assert!(IoRing::new(memory, u64::MAX - 3).is_err());
    }

    #[test]
    fn short_sources_fail_instead_of_claiming_a_full_read() {
        let memory = memory(1);
        let fs = HttpFs::new(
            br#"{"files":[["original.bin",8]]}"#.to_vec(),
            |_, _, _, _| Ok(vec![0]),
        )
        .unwrap();
        assert!(fs.read(&memory, 0, 0, 128, 8).is_err());
        assert_eq!(memory.read(128, 8).unwrap(), [0; 8]);
    }

    #[test]
    fn concurrent_workers_claim_each_lane_once() {
        let memory = memory(1);
        memory.store_u32(68, 4).unwrap();
        for slot in 0..4u64 {
            let row = 128 + slot * 64;
            memory.store_u32(row + 4, 0).unwrap();
            memory.store_u32(row + 16, (1024 + slot) as u32).unwrap();
            memory.store_u32(row + 24, 1).unwrap();
            memory.store_u32(row, 1).unwrap();
        }
        let entered = Arc::new(std::sync::Barrier::new(4));
        let calls = Arc::new(AtomicU64::new(0));
        let fs = HttpFs::new(br#"{"files":[["original.bin",1]]}"#.to_vec(), {
            let entered = entered.clone();
            let calls = calls.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::AcqRel);
                entered.wait();
                Ok(vec![7])
            }
        })
        .unwrap();
        let ring = Arc::new(IoRing::new(memory.clone(), 64).unwrap());
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let ring = ring.clone();
                let fs = fs.clone();
                scope.spawn(move || assert!(ring.pump_one(&fs).unwrap()));
            }
        });
        assert_eq!(calls.load(Ordering::Acquire), 4);
        for slot in 0..4u64 {
            assert_eq!(memory.load_u32(128 + slot * 64).unwrap(), 2);
        }
        assert_eq!(TEMPORARY_BUDGET, 4 * BLOCK as usize);
    }

    #[test]
    fn read_error_is_published_without_becoming_fatal() {
        let memory = memory(1);
        memory.store_u32(68, 1).unwrap();
        memory.store_u32(132, 99).unwrap();
        memory.store_u32(144, 1024).unwrap();
        memory.store_u32(152, 1).unwrap();
        memory.store_u32(128, 1).unwrap();
        let fs = HttpFs::new(
            br#"{"files":[["original.bin",1]]}"#.to_vec(),
            |_, _, _, _| anyhow::bail!("fixture failure"),
        )
        .unwrap();
        let ring = IoRing::new(memory.clone(), 64).unwrap();
        assert!(ring.pump_one(&fs).unwrap());
        assert_eq!(memory.load_u32(128).unwrap(), 2);
        assert_eq!(memory.load_u32(156).unwrap(), u32::MAX);
    }

    fn finish_spawn(
        handle: JoinHandle<Result<()>>,
        stop: Arc<AtomicBool>,
        memory: GuestMemory,
        table: u64,
        request_stop: bool,
    ) -> bool {
        let (done, received) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let result = handle.join().map(|result| result.is_ok()).unwrap_or(false);
            let _ = done.send(result);
        });
        if request_stop {
            stop.store(true, Ordering::Release);
            let _ = memory.notify(table);
        }
        let completed = received
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or(false);
        stop.store(true, Ordering::Release);
        let _ = memory.notify(table);
        let joined = waiter.join().is_ok();
        completed && joined
    }

    #[test]
    fn spawned_pool_keeps_serving_after_one_read_failure() {
        let memory = memory(1);
        memory.store_u32(68, 2).unwrap();
        for (row, id, destination) in [(128u64, 0u32, 1024u32), (192, 1, 1025)] {
            memory.store_u32(row + 4, id).unwrap();
            memory.store_u32(row + 16, destination).unwrap();
            memory.store_u32(row + 24, 1).unwrap();
            memory.store_u32(row, 1).unwrap();
        }
        let calls = Arc::new(AtomicU64::new(0));
        let observed = calls.clone();
        let fs = HttpFs::new(
            br#"{"files":[["bad.bin",1],["good.bin",1]]}"#.to_vec(),
            move |name, _, _, _| {
                observed.fetch_add(1, Ordering::AcqRel);
                if name == "bad.bin" {
                    anyhow::bail!("one lane failed")
                }
                Ok(vec![9])
            },
        )
        .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(memory.clone(), 64, fs, stop.clone()).unwrap();
        let _ = memory.wait32(128, 1, Duration::from_secs(1));
        let _ = memory.wait32(192, 1, Duration::from_secs(1));
        let finished = finish_spawn(handle, stop, memory.clone(), 64, true);
        assert!(finished);
        assert_eq!(calls.load(Ordering::Acquire), 2);
        assert_eq!(memory.load_u32(128).unwrap(), 2);
        assert_eq!(memory.load_u32(156).unwrap(), u32::MAX);
        assert_eq!(memory.load_u32(192).unwrap(), 2);
        assert_eq!(memory.read(1025, 1).unwrap(), [9]);
    }

    #[test]
    fn spawned_pool_stops_and_joins_without_requests() {
        let memory = memory(1);
        memory.store_u32(68, 0).unwrap();
        let fs = HttpFs::new(br#"{"files":[]}"#.to_vec(), |_, _, _, _| unreachable!()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(memory.clone(), 64, fs, stop.clone()).unwrap();
        assert!(finish_spawn(handle, stop, memory, 64, true));
    }

    #[test]
    fn ring_corruption_shuts_down_and_joins_every_worker() {
        let memory = memory(1);
        memory.store_u32(68, 0).unwrap();
        let fs = HttpFs::new(br#"{"files":[]}"#.to_vec(), |_, _, _, _| unreachable!()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(memory.clone(), 64, fs, stop.clone()).unwrap();
        memory.store_u32(68, MAX_LANES + 1).unwrap();
        memory.notify(64).unwrap();
        let finished = finish_spawn(handle, stop, memory, 64, false);
        assert!(!finished);
    }

    #[test]
    fn worker_panic_shuts_down_and_joins_every_worker() {
        let memory = memory(1);
        memory.store_u32(68, 1).unwrap();
        memory.store_u32(132, 0).unwrap();
        memory.store_u32(144, 1024).unwrap();
        memory.store_u32(152, 1).unwrap();
        memory.store_u32(128, 1).unwrap();
        let fs = HttpFs::new(br#"{"files":[["panic.bin",1]]}"#.to_vec(), |_, _, _, _| {
            panic!("fixture panic")
        })
        .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(memory.clone(), 64, fs, stop.clone()).unwrap();
        let _ = memory.wait32(128, 1, Duration::from_secs(1));
        let finished = finish_spawn(handle, stop, memory, 64, false);
        assert!(!finished);
    }
}
