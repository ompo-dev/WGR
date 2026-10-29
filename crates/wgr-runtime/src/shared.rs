//! Bounds-checked shared guest memory; normal Rust loads would race with Wasm.
use anyhow::{Context, Result, ensure};
use std::{
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
    time::Duration,
};
use wasmtime::{Engine, MemoryTypeBuilder, SharedMemory, WaitResult};
#[derive(Clone)]
pub struct GuestMemory {
    inner: SharedMemory,
}
impl GuestMemory {
    pub fn new64(
        engine: &Engine,
        minimum_pages: u64,
        maximum_pages: u64,
        budget_bytes: u64,
    ) -> Result<Self> {
        ensure!(
            minimum_pages
                .checked_mul(65536)
                .is_some_and(|n| n <= budget_bytes),
            "Memória mínima excede o orçamento"
        );
        let maximum_pages = maximum_pages.min(budget_bytes / 65536);
        ensure!(maximum_pages >= minimum_pages, "Limite de memória inválido");
        let ty = MemoryTypeBuilder::new()
            .memory64(true)
            .shared(true)
            .min(minimum_pages)
            .max(Some(maximum_pages))
            .build()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let inner = SharedMemory::new(engine, ty).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(Self { inner })
    }
    pub fn shared(&self) -> SharedMemory {
        self.inner.clone()
    }
    pub fn len(&self) -> usize {
        self.inner.data_size()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn range(&self, address: u64, length: usize) -> Result<usize> {
        let start = usize::try_from(address).context("Endereço fora do host")?;
        ensure!(
            start
                .checked_add(length)
                .is_some_and(|end| end <= self.len()),
            "Acesso fora da memória do port"
        );
        Ok(start)
    }
    fn word(&self, address: u64) -> Result<&AtomicU32> {
        ensure!(address.is_multiple_of(4), "Word compartilhada desalinhada");
        let start = self.range(address, 4)?;
        // The shared base is page-aligned and never moves. This reference is
        // bounded by the SharedMemory owner and the checked four-byte region.
        Ok(unsafe {
            AtomicU32::from_ptr(
                self.inner
                    .data()
                    .as_ptr()
                    .cast::<u8>()
                    .add(start)
                    .cast_mut()
                    .cast(),
            )
        })
    }
    pub fn load_u32(&self, address: u64) -> Result<u32> {
        Ok(u32::from_le(self.word(address)?.load(Ordering::Acquire)))
    }
    pub fn store_u32(&self, address: u64, value: u32) -> Result<()> {
        self.word(address)?.store(value.to_le(), Ordering::Release);
        Ok(())
    }
    pub fn add_u32(&self, address: u64, value: u32) -> Result<()> {
        let _ = self
            .word(address)?
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                Some(u32::from_le(old).wrapping_add(value).to_le())
            });
        Ok(())
    }
    pub fn read(&self, address: u64, length: usize) -> Result<Vec<u8>> {
        ensure!(
            length <= 16 * 1024 * 1024,
            "Uma cópia do host excede 16 MiB"
        );
        let start = self.range(address, length)?;
        Ok(self.inner.data()[start..start + length]
            .iter()
            .map(|cell| unsafe { AtomicU8::from_ptr(cell.get()) }.load(Ordering::Relaxed))
            .collect())
    }
    pub fn write(&self, address: u64, bytes: &[u8]) -> Result<()> {
        let start = self.range(address, bytes.len())?;
        for (cell, byte) in self.inner.data()[start..start + bytes.len()]
            .iter()
            .zip(bytes)
        {
            unsafe { AtomicU8::from_ptr(cell.get()) }.store(*byte, Ordering::Relaxed);
        }
        Ok(())
    }
    pub fn c_string(&self, address: u64, maximum: usize) -> Result<String> {
        ensure!(maximum <= 65536, "Limite de string inválido");
        let start = self.range(address, 1)?;
        let end = self.len().min(start.saturating_add(maximum));
        let mut bytes = Vec::with_capacity(128.min(end - start));
        for cell in &self.inner.data()[start..end] {
            let byte = unsafe { AtomicU8::from_ptr(cell.get()) }.load(Ordering::Relaxed);
            if byte == 0 {
                return String::from_utf8(bytes).context("String UTF-8 inválida");
            }
            bytes.push(byte);
        }
        anyhow::bail!("String do port sem terminador dentro do limite")
    }
    pub fn wait32(&self, address: u64, expected: u32, timeout: Duration) -> Result<WaitResult> {
        self.word(address)?;
        self.inner
            .atomic_wait32(address, expected, Some(timeout))
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
    pub fn notify(&self, address: u64) -> Result<()> {
        self.word(address)?;
        self.inner
            .atomic_notify(address, u32::MAX)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_threads_share_one_memory64() {
        let runtime = crate::NativeRuntime::new().unwrap();
        let memory = GuestMemory::new64(&runtime.engine, 1, 1, 65536).unwrap();
        let mut threads = vec![];
        for _ in 0..4 {
            let memory = memory.clone();
            threads.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    memory.add_u32(0, 1).unwrap();
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(memory.load_u32(0).unwrap(), 4000);
        assert!(memory.store_u32(1, 7).is_err());
        assert!(memory.read(65535, 2).is_err());
        memory.write(24, &[3, 4, 5]).unwrap();
        assert_eq!(memory.read(24, 3).unwrap(), [3, 4, 5]);
        let bytes=wat::parse_str("(module (import \"env\" \"memory\" (memory i64 1 1 shared)) (func (export \"add\") (param i32) (result i32) i64.const 0 local.get 0 i32.atomic.rmw.add))").unwrap();
        let module = runtime.compile(&bytes).unwrap();
        let mut store = wasmtime::Store::new(&runtime.engine, ());
        store.set_epoch_deadline(1);
        let instance =
            wasmtime::Instance::new(&mut store, &module.module, &[memory.shared().into()]).unwrap();
        let add = instance
            .get_typed_func::<i32, i32>(&mut store, "add")
            .unwrap();
        assert_eq!(add.call(&mut store, 7).unwrap(), 4000);
        assert_eq!(memory.load_u32(0).unwrap(), 4007);
    }
}
