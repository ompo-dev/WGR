// muguet - io.rs
// Constantin Clerc (c22dev) - 0.1

use crate::mem_base_len;
#[cfg(windows)]
use crate::platform_windows::PositionedFile;
use std::collections::HashMap;
use std::fs::File;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;
use wasmtime::SharedMemory;

static STARTED: AtomicBool = AtomicBool::new(false);
const MAX_OPEN: usize = 1024;

#[derive(Debug, PartialEq)]
struct DataHint {
    file: usize,
    offset: u64,
    length: u64,
    low: bool,
}
fn take_hints(mem: &SharedMemory, table: u64) -> Vec<DataHint> {
    let (_, bytes) = mem_base_len(mem);
    if table % 4 != 0 || table.checked_add(24).is_none_or(|end| end > bytes as u64) {
        return Vec::new();
    }
    let word = |offset| atomic_i32(mem, table + offset).load(Ordering::Acquire) as u32;
    let ring = word(8) as u64;
    let cap = word(12);
    if ring == 0 || cap == 0 || cap > 1 << 20 {
        return Vec::new();
    }
    let Some(base) = table.checked_add(ring * 4).filter(|base| {
        base.checked_add(cap as u64 * 16)
            .is_some_and(|end| end <= bytes as u64)
    }) else {
        return Vec::new();
    };
    let head = word(16);
    let mut tail = word(20);
    if head.wrapping_sub(tail) > cap {
        tail = head.wrapping_sub(cap);
    }
    let mut hints = Vec::new();
    for _ in 0..cap.min(256) {
        if tail == head {
            break;
        }
        let row = base + (tail % cap) as u64 * 16;
        let raw = atomic_i32(mem, row + 12).load(Ordering::Acquire) as u32;
        if raw == 0 {
            break;
        }
        let get = |at| atomic_i32(mem, row + at).load(Ordering::Relaxed) as u32;
        hints.push(DataHint {
            file: get(0) as usize,
            offset: get(4) as u64 | ((get(8) as u64) << 32),
            length: (raw & 0x3fff_ffff) as u64,
            low: raw & 0x4000_0000 != 0,
        });
        atomic_i32(mem, row + 12).store(0, Ordering::Release);
        tail = tail.wrapping_add(1);
    }
    atomic_i32(mem, table + 20).store(tail as i32, Ordering::Release);
    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    fn memory() -> SharedMemory {
        let mut config = wasmtime::Config::new();
        config.shared_memory(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        SharedMemory::new(&engine, wasmtime::MemoryType::shared(512, 512)).unwrap()
    }
    #[test]
    fn streams_large_map_read_without_an_aggregate_allocation() {
        let mem = memory();
        let length = 29_749_248;
        let largest = std::sync::atomic::AtomicU64::new(0);
        let calls = std::sync::atomic::AtomicU64::new(0);
        let copied = copy_to_guest(&mem, 128, 37, length, |offset, bytes| {
            largest.fetch_max(bytes, Ordering::Relaxed);
            calls.fetch_add(1, Ordering::Relaxed);
            Ok((offset..offset + bytes).map(|n| (n % 251) as u8).collect())
        })
        .unwrap();
        assert_eq!(copied as u64, length);
        assert!(calls.load(Ordering::Relaxed) > 1);
        assert!(largest.load(Ordering::Relaxed) <= 1 << 20);
        for offset in [0, (4 << 20) - 1, 4 << 20, 16 << 20, length - 1] {
            assert_eq!(
                crate::read_bytes(&mem, 128 + offset, 1),
                vec![((37 + offset) % 251) as u8]
            );
        }
    }
    #[test]
    fn consumes_engine_hint_ring_with_relative_word_offsets_and_priority() {
        let mem = memory();
        let table = 64;
        let base = table + 64 * 4;
        for (offset, value) in [(8, 64), (12, 2), (16, 2), (20, 0)] {
            crate::write_u32(&mem, table + offset, value);
        }
        for (offset, value) in [
            (0, 7),
            (4, 123),
            (8, 1),
            (12, 0x4000_0200),
            (16, 8),
            (20, 456),
            (24, 0),
            (28, 1024),
        ] {
            crate::write_u32(&mem, base + offset, value);
        }
        let hints = take_hints(&mem, table);
        assert_eq!(
            hints,
            vec![
                DataHint {
                    file: 7,
                    offset: (1 << 32) + 123,
                    length: 512,
                    low: true
                },
                DataHint {
                    file: 8,
                    offset: 456,
                    length: 1024,
                    low: false
                }
            ]
        );
        assert!(take_hints(&mem, table).is_empty());
        crate::write_u32(&mem, table + 8, u32::MAX);
        assert!(take_hints(&mem, table).is_empty());
    }
    #[test]
    fn rejects_out_of_bounds_before_read_and_reports_short_source() {
        let mem = memory();
        assert!(copy_to_guest(&mem, (32 << 20) - 8, 0, 16, |_, _| panic!(
            "must validate before I/O"
        ))
        .is_err());
        assert!(copy_to_guest(&mem, 0, 0, 1024, |_, n| Ok(vec![0; n as usize - 1])).is_err());
    }
}

pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("MUGUET_DATA").unwrap_or_default())
}

fn manifest_files() -> Vec<(String, u64)> {
    let text = std::fs::read_to_string(data_dir().join("manifest.json")).expect("manifest.json");
    let v: serde_json::Value = serde_json::from_str(&text).expect("manifest json");
    v["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| {
            (
                f[0].as_str().unwrap_or("").to_string(),
                f[1].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

fn atomic_i32(mem: &SharedMemory, addr: u64) -> &AtomicI32 {
    let (base, len) = mem_base_len(mem);
    assert!((addr as usize) + 4 <= len && addr % 4 == 0);
    unsafe { &*(base.add(addr as usize) as *const AtomicI32) }
}

pub fn start(mem: &SharedMemory, table: u64) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mem = mem.clone();
    std::thread::Builder::new()
        .name("io".into())
        .spawn(move || {
            if crate::remote::enabled() {
                serve_remote(mem, table)
            } else {
                serve_loop(mem, table)
            }
        })
        .expect("spawn io thread");
    eprintln!("[io] native I/O thread started (table=0x{table:x})");
}

fn serve_remote(mem: SharedMemory, table: u64) {
    let files = std::sync::Arc::new(manifest_files());
    let workers = rayon::ThreadPoolBuilder::new()
        .num_threads(32)
        .thread_name(|n| format!("native-io-{n}"))
        .build()
        .expect("I/O workers");
    loop {
        let seen = atomic_i32(&mem, table).load(Ordering::Acquire);
        let count = atomic_i32(&mem, table + 4).load(Ordering::Acquire);
        if !(0..=256).contains(&count) {
            eprintln!("[native-error] Invalid I/O lane count {count}");
            std::process::exit(72);
        }
        for slot in 0..count as u64 {
            let row = table + 64 + 64 * slot;
            if atomic_i32(&mem, row)
                .compare_exchange(1, 3, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            let mem = mem.clone();
            let files = files.clone();
            workers.spawn(move || {
                let word = |n: u64| atomic_i32(&mem, row + n*4).load(Ordering::Acquire) as u32 as u64;
                let id = word(1) as usize;
                let offset = word(2) | word(3) << 32;
                let destination = word(4) | word(5) << 32;
                let requested = word(6);
                let began = std::time::Instant::now();
                let file_path = files.get(id).map(|(p, _)| p.clone()).unwrap_or_default();
                let result = (|| -> anyhow::Result<i32> {
                    let (path,size) = files.get(id).ok_or_else(|| anyhow::anyhow!("Unknown file id {id}"))?;
                    if offset >= *size { return Ok(0); }
                                        let length = requested.min(size-offset);
                    copy_to_guest(&mem,destination,offset,length,|position,bytes|crate::remote::read_data(path,*size,position,bytes))
                })();
                let value = match result { Ok(n) => n, Err(e) => { eprintln!("[native-io-error] file={id} offset={offset} length={requested}: {e:#}"); -1 } };
                let elapsed = began.elapsed().as_secs_f64()*1000.0;
                if elapsed >= 100.0 { eprintln!("[native-io-slow] file={id} path={file_path} offset={offset} length={requested} ms={elapsed:.1}"); }
                atomic_i32(&mem,row+28).store(value,Ordering::Release);
                atomic_i32(&mem,row).store(2,Ordering::Release);
                let _ = mem.atomic_notify(row,1);
            });
        }
        let mut hints = take_hints(&mem, table);
        hints.sort_by_key(|hint| hint.low);
        for hint in hints {
            if let Some((path, size)) = files.get(hint.file) {
                crate::remote::prefetch_data(path, *size, hint.offset, hint.length, hint.low);
            }
        }
        let _ = mem.atomic_wait32(table, seen as u32, Some(Duration::from_millis(20)));
    }
}

fn copy_to_guest(
    mem: &SharedMemory,
    destination: u64,
    offset: u64,
    length: u64,
    read: impl Fn(u64, u64) -> anyhow::Result<Vec<u8>> + Sync,
) -> anyhow::Result<i32> {
    let (_, memory_bytes) = mem_base_len(mem);
    let memory_bytes = memory_bytes as u64;
    anyhow::ensure!(
        destination <= memory_bytes
            && length <= memory_bytes - destination
            && length <= i32::MAX as u64,
        "I/O destination outside guest memory"
    );
    anyhow::ensure!(
        offset.checked_add(length).is_some(),
        "I/O source offset overflow"
    );
    use rayon::prelude::*;
    // Nested work uses the same bounded I/O pool. A single large RPF request
    // must not serialize all of its network segments before releasing the game.
    (0..length.div_ceil(1 << 20))
        .into_par_iter()
        .try_for_each(|part| -> anyhow::Result<()> {
            let done = part * (1 << 20);
            let amount = (length - done).min(1 << 20);
            let data = read(offset + done, amount)?;
            anyhow::ensure!(
                data.len() as u64 == amount,
                "I/O source returned a short block"
            );
            anyhow::ensure!(
                crate::write_bytes(mem, destination + done, &data),
                "I/O destination outside guest memory"
            );
            Ok(())
        })?;
    Ok(length as i32)
}

fn serve_loop(mem: SharedMemory, table: u64) {
    let files = manifest_files();
    let root = data_dir();
    let mut handles: HashMap<i32, File> = HashMap::new();
    let mut served: u64 = 0;
    let mut bytes: u64 = 0;
    loop {
        let seen = atomic_i32(&mem, table).load(Ordering::SeqCst);
        let n = atomic_i32(&mem, table + 4).load(Ordering::SeqCst).max(0) as u64;
        for slot in 0..n {
            let w = table + 64 + 64 * slot;
            if atomic_i32(&mem, w)
                .compare_exchange(1, 3, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                continue;
            }
            let word = |k: u64| atomic_i32(&mem, w + 4 * k).load(Ordering::SeqCst) as u32 as u64;
            let id = word(1) as i32;
            let off = word(2) | (word(3) << 32);
            let dst = word(4) | (word(5) << 32);
            let mut len = word(6);
            let result: i64 = match files.get(id as usize) {
                None => -1,
                Some((path, size)) => {
                    if off >= *size {
                        0
                    } else {
                        len = len.min(size - off);

                        if !handles.contains_key(&id) {
                            if handles.len() >= MAX_OPEN {
                                handles.clear();
                            }
                            match File::open(root.join(path)) {
                                Ok(f) => {
                                    handles.insert(id, f);
                                }
                                Err(e) => eprintln!("[io] open {path}: {e}"),
                            }
                        }
                        let (base, mlen) = mem_base_len(&mem);
                        let Some(f) = handles.get(&id) else {
                            atomic_i32(&mem, w + 28).store(-1, Ordering::SeqCst);
                            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
                            let _ = mem.atomic_notify(w, 1);
                            continue;
                        };
                        if (dst + len) as usize > mlen {
                            -1
                        } else {
                            crate::heapcheck::note(dst, len, "file read");
                            let buf = unsafe {
                                std::slice::from_raw_parts_mut(base.add(dst as usize), len as usize)
                            };
                            match f.read_exact_at(buf, off) {
                                Ok(()) => len as i64,
                                Err(e) => {
                                    eprintln!("[io] read {path}@{off}+{len}: {e}");
                                    -1
                                }
                            }
                        }
                    }
                }
            };
            served += 1;
            if result > 0 {
                bytes += result as u64;
            }
            if served % 2000 == 0 {
                eprintln!("[io] {served} reads, {} MB", bytes >> 20);
            }
            atomic_i32(&mem, w + 28).store(result as i32, Ordering::SeqCst);
            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
            let _ = mem.atomic_notify(w, 1);
        }

        let _ = mem.atomic_wait32(table, seen as u32, Some(Duration::from_millis(20)));
    }
}
