//! Local allocation benchmark for the two warm-queue representations.
//! Run in isolation: WGR_RECIPE_FIXTURE=<local jsonl> cargo test --test recipe_memory -- --ignored --nocapture
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let n = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(n, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(p, layout) };
    }
}

#[test]
#[ignore = "local allocation benchmark, optional cached catalog"]
fn compact_recipe_queue_memory() {
    let text = if let Some(path) = std::env::var_os("WGR_RECIPE_FIXTURE") {
        std::fs::read_to_string(path).unwrap()
    } else {
        // Original synthetic records with representative numeric arrays.
        let mut text = String::new();
        for n in 0..34000 {
            text.push_str(&format!("{{\"vs\":\"{n:016x}\",\"ps\":\"p\",\"strides\":[32,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],\"els\":[[1,0,6,0,0,0]],\"blend\":[0,0,1,5,6,1,5,6,1,15],\"rt\":[\"Bgra8Unorm\"],\"samples\":1}}\n"));
        }
        text
    };
    let lines: Vec<_> = text
        .lines()
        .filter(|s| !s.is_empty() && s.len() <= 256 << 10)
        .collect();
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let t = Instant::now();
    let parsed: VecDeque<(u64, serde_json::Value)> = lines
        .iter()
        .enumerate()
        .filter_map(|(id, line)| serde_json::from_str(line).ok().map(|v| (id as u64, v)))
        .collect();
    let parsed_ms = t.elapsed().as_secs_f64() * 1000.;
    let parsed_bytes = LIVE.load(Relaxed).saturating_sub(base);
    let parsed_peak = PEAK.load(Relaxed).saturating_sub(base);
    let count = parsed.len();
    drop(parsed);
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let t = Instant::now();
    let mut compact: VecDeque<(u64, Box<str>)> = lines
        .iter()
        .enumerate()
        .map(|(id, line)| (id as u64, Box::from(*line)))
        .collect();
    let compact_ms = t.elapsed().as_secs_f64() * 1000.;
    let compact_bytes = LIVE.load(Relaxed).saturating_sub(base);
    let compact_peak = PEAK.load(Relaxed).saturating_sub(base);
    let mut valid = 0;
    let t = Instant::now();
    while let Some((_, line)) = compact.pop_front() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            std::hint::black_box(value);
            valid += 1;
        }
    }
    let decode_ms = t.elapsed().as_secs_f64() * 1000.;
    assert_eq!(valid, count);
    assert!(compact_bytes < parsed_bytes);
    println!(
        "WGR_RECIPE_MEMORY {}",
        serde_json::json!({"records":lines.len(),"valid":valid,"parsed_bytes":parsed_bytes,"compact_bytes":compact_bytes,"parsed_peak_bytes":parsed_peak,"compact_peak_bytes":compact_peak,"parsed_queue_ms":parsed_ms,"compact_queue_ms":compact_ms,"compact_decode_ms":decode_ms})
    );
}
