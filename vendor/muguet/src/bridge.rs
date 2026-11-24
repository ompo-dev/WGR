//! Bounded commands from the owning WGR process, and native presentation telemetry.
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use std::time::Instant;
static COMMANDS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);
static WORLD_READY: AtomicBool = AtomicBool::new(false);
static WORLD_AT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
pub static PRESENTED: AtomicU64 = AtomicU64::new(0);
// Measure completed present calls without serializing JSON on the GPU thread.
// Exact durations are retained for the next sample (bounded for safety).
#[derive(Default)]
struct FrameIntervals {
    last: Option<Instant>,
    micros: Vec<u64>,
    overflow: u64,
}
static FRAME_INTERVALS: std::sync::LazyLock<Mutex<FrameIntervals>> =
    std::sync::LazyLock::new(|| Mutex::new(FrameIntervals::default()));
static FRAME_PACING: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("WGR_FRAME_PACING").as_deref() == Ok("1"));

fn summarize_intervals(mut micros: Vec<u64>, overflow: u64) -> Value {
    micros.sort_unstable();
    let count = micros.len();
    let percentile = |p: usize| {
        micros
            .get((count.saturating_mul(p).div_ceil(100)).saturating_sub(1))
            .copied()
    };
    json!({"count":count,"overflow":overflow,"p99_us":percentile(99),"max_us":micros.last(),"over_60fps_budget":micros.iter().filter(|&&us| us > 16667).count(),"over_40ms":micros.iter().filter(|&&us| us >= 40000).count()})
}

#[cfg(test)]
mod frame_interval_tests {
    use super::*;
    #[test]
    fn frame_intervals_detect_long_frames_even_with_fast_average() {
        let value = summarize_intervals(vec![10000, 12000, 16667, 16668, 45000], 0);
        assert_eq!(value["count"], 5);
        assert_eq!(value["over_60fps_budget"], 2);
        assert_eq!(value["p99_us"], 45000);
        assert_eq!(value["over_40ms"], 1);
        assert!(summarize_intervals(vec![], 1)["max_us"].is_null());
    }
}
pub fn emit(value: Value) {
    if value["type"] == "game-progress" && value["payload"]["world"] == true {
        WORLD_AT.get_or_init(Instant::now);
        WORLD_READY.store(true, Ordering::Release);
    }
    println!("WGR_EVENT {value}");
}
pub fn world_ready() -> bool {
    WORLD_READY.load(Ordering::Acquire)
}
pub fn start() {
    if STARTED.swap(true, Ordering::Relaxed) || std::env::var_os("WGR_CONTROL_PIPE").is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("native-control".into())
        .spawn(|| {
            use std::io::{BufRead, Read};
            let input = std::io::stdin();
            let mut input = input.lock();
            loop {
                let mut bytes = Vec::with_capacity(256);
                let result = input.by_ref().take(4097).read_until(b'\n', &mut bytes);
                match result {
                    Ok(0) | Err(_) => std::process::exit(0), // Owning process closed its pipe.
                    Ok(_) if bytes.len() > 4096 || bytes.last() != Some(&b'\n') => {
                        std::process::exit(74)
                    }
                    _ => {}
                }
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    let mut commands = COMMANDS.lock().unwrap();
                    if commands.len() < 16 {
                        commands.push(value);
                    }
                }
            }
        })
        .expect("native control thread");
}
pub fn commands(window: &winit::window::Window) {
    for command in std::mem::take(&mut *COMMANDS.lock().unwrap()) {
        match command["command"].as_str() {
            Some("focus") => crate::app::focus_window(window),
            Some("mute") => crate::audio::MUTED.store(
                command["value"].as_bool().unwrap_or(false),
                Ordering::Relaxed,
            ),
            Some("stop") => std::process::exit(0),
            _ => {}
        }
    }
}
pub fn window_created(window: &winit::window::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = window.window_handle() {
        if let RawWindowHandle::Win32(handle) = handle.as_raw() {
            emit(
                json!({"type":"native_window","hwnd":handle.hwnd.get(),"pid":std::process::id(),"backend":"native"}),
            );
        }
    }
}
pub fn presented() {
    let now = Instant::now();
    if !*FRAME_PACING {
        static LAST_FRAME: Mutex<Option<Instant>> = Mutex::new(None);
        let previous = LAST_FRAME.lock().unwrap().replace(now);
        if let Some(previous) = previous {
            let ms = now.duration_since(previous).as_secs_f64() * 1000.0;
            if ms >= 40.0 {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                emit(
                    json!({"type":"native_slow_frame","duration_ms":ms,"timestamp_ms":timestamp,"frames":PRESENTED.load(Ordering::Relaxed)}),
                );
            }
        }
        if PRESENTED.fetch_add(1, Ordering::Relaxed) == 0 {
            emit(
                json!({"type":"first_frame","backend":"native","engine":"wasmtime-wgpu-native","browser":false,"frames":1}),
            );
        }
        return;
    }
    let first = {
        let mut intervals = FRAME_INTERVALS.lock().unwrap();
        if let Some(previous) = intervals.last.replace(now) {
            let us = now
                .duration_since(previous)
                .as_micros()
                .min(u64::MAX as u128) as u64;
            if intervals.micros.len() < 4096 {
                intervals.micros.push(us);
            } else {
                intervals.overflow += 1;
            }
        }
        PRESENTED.fetch_add(1, Ordering::Relaxed) == 0
    };
    if first {
        emit(
            json!({"type":"first_frame","backend":"native","engine":"wasmtime-wgpu-native","browser":false,"frames":1}),
        );
    }
}
pub fn sample() {
    static LAST: Mutex<Option<(Instant, u64)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    let now = Instant::now();
    match *last {
        Some((at, previous)) if now.duration_since(at).as_secs_f64() >= 1.0 => {
            let (frames, micros, overflow) = if *FRAME_PACING {
                let mut intervals = FRAME_INTERVALS.lock().unwrap();
                (
                    PRESENTED.load(Ordering::Relaxed),
                    std::mem::take(&mut intervals.micros),
                    std::mem::take(&mut intervals.overflow),
                )
            } else {
                (PRESENTED.load(Ordering::Relaxed), Vec::new(), 0)
            };
            let fps = (frames - previous) as f64 / now.duration_since(at).as_secs_f64();
            let frame_intervals = if *FRAME_PACING {
                summarize_intervals(micros, overflow)
            } else {
                Value::Null
            };
            let world_elapsed_ms = WORLD_AT
                .get()
                .map(|at| now.saturating_duration_since(*at).as_millis() as u64);
            emit(
                json!({"type":"game-fps","payload":{"fps":fps,"frames":frames,"frame_intervals":frame_intervals,"world_elapsed_ms":world_elapsed_ms,"backend":"native","process_working_set_mib":crate::footprint_mb(),"input":crate::input::snapshot(),"io":crate::remote::metrics()}}),
            );
            *last = Some((now, frames));
        }
        None => {
            if *FRAME_PACING {
                let mut intervals = FRAME_INTERVALS.lock().unwrap();
                intervals.micros.clear();
                intervals.overflow = 0;
            }
            *last = Some((now, PRESENTED.load(Ordering::Relaxed)));
        }
        _ => {}
    }
}
