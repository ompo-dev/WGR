//! Bounded commands from the owning WGR process, and native presentation telemetry.
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use std::time::Instant;
static COMMANDS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);
pub static PRESENTED: AtomicU64 = AtomicU64::new(0);
pub fn perf_enabled() -> bool {
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("WGR_PERF_DIAGNOSTICS").as_deref() == Ok("1"));
    *ENABLED
}
pub fn timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
pub fn thread_started(role: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, GetCurrentThreadId, SetThreadDescription,
        };
        let thread = std::thread::current();
        let name = format!("{role}: {}", thread.name().unwrap_or(role));
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            SetThreadDescription(GetCurrentThread(), wide.as_ptr());
        }
        if perf_enabled() {
            emit(
                json!({"type":"native_thread","role":role,"name":name,"tid":unsafe {GetCurrentThreadId()}}),
            );
        }
    }
    #[cfg(not(windows))]
    let _ = role;
}
pub fn emit(value: Value) {
    println!("WGR_EVENT {value}");
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
    static LAST_FRAME: Mutex<Option<Instant>> = Mutex::new(None);
    let now = Instant::now();
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
}
pub fn sample() {
    static LAST: Mutex<Option<(Instant, u64)>> = Mutex::new(None);
    let frames = PRESENTED.load(Ordering::Relaxed);
    let mut last = LAST.lock().unwrap();
    let now = Instant::now();
    match *last {
        Some((at, previous)) if now.duration_since(at).as_secs_f64() >= 1.0 => {
            let fps = (frames - previous) as f64 / now.duration_since(at).as_secs_f64();
            emit(
                json!({"type":"game-fps","payload":{"fps":fps,"frames":frames,"backend":"native","process_working_set_mib":crate::footprint_mb(),"input":crate::input::snapshot(),"io":crate::remote::metrics()}}),
            );
            *last = Some((now, frames));
        }
        None => *last = Some((now, frames)),
        _ => {}
    }
}
