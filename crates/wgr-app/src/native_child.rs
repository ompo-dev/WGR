//! Owns one native game process and its native child HWND. Never navigates a page.
use crate::player::{PlayerEvent, PlayerOptions};
use anyhow::{Context, Result, ensure};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, GetWindowThreadProcessId, IsChild, MoveWindow, SW_HIDE,
        SW_SHOW, SetWindowLongPtrW, ShowWindow, WS_CLIPCHILDREN,
    },
};

pub struct NativeChild {
    process: RefCell<Child>,
    input: RefCell<ChildStdin>,
    events: Arc<Mutex<VecDeque<PlayerEvent>>>,
    window: Cell<isize>,
    parent: HWND,
    bounds: Cell<[i32; 4]>,
    ended: Cell<bool>,
}
impl NativeChild {
    pub fn spawn(parent: HWND, options: &PlayerOptions) -> Result<Self> {
        // Reuse the parent clipping contract required by every native child HWND.
        unsafe {
            let style = GetWindowLongPtrW(parent, GWL_STYLE) as u32;
            SetWindowLongPtrW(parent, GWL_STYLE, (style | WS_CLIPCHILDREN.0) as isize);
        }
        let executable = std::env::current_exe()?
            .parent()
            .context("Pasta do executável ausente")?
            .join("wgr-rage.exe");
        ensure!(
            executable.is_file(),
            "O runtime nativo wgr-rage.exe não está instalado ao lado do WGR.exe"
        );
        let descriptor = options.spool.join("native-port.json");
        std::fs::write(
            &descriptor,
            include_bytes!("../../../ports/fiveplay-gta5.port.json"),
        )?;
        let log_root = options.profile.join("native-logs");
        std::fs::create_dir_all(&log_root)?;
        let mut command = Command::new(executable);
        command
            .creation_flags(0x0800_0000)
            .env("WGR_PORT_DESCRIPTOR", descriptor)
            .env(
                "WGR_WASM_FILE",
                options.binary_path.as_ref().context("Binário ausente")?,
            )
            .env("WGR_PARENT_HWND", (parent.0 as isize).to_string())
            .env("WGR_CONTROL_PIPE", "1")
            .env("WGR_NATIVE_ROOT", &options.native_root)
            .env("MUGUET_FSROOT", options.profile.join("native-saves"))
            .env("WGR_NONINTERACTIVE", "1")
            .env(
                "WGR_INPUT_DIAGNOSTICS",
                if options.diagnostic || std::env::args().any(|arg| arg == "--gta-smoke") {
                    "1"
                } else {
                    "0"
                },
            )
            .env("MUGUET_FULLSCREEN", "0")
            .env("MUGUET_SCALE", "1")
            .env("MUGUET_FPS", "0")
            .env("MUGUET_START_MODE", "2")
            .env("MUGUET_MUTE", if options.muted { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = command
            .spawn()
            .context("Não foi possível iniciar o runtime nativo")?;
        let input = process
            .stdin
            .take()
            .context("Canal de controle nativo ausente")?;
        let events = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(output) = process.stdout.take() {
            read_output(output, events.clone(), log_root.join("output.log"), true)?;
        }
        if let Some(output) = process.stderr.take() {
            read_output(output, events.clone(), log_root.join("engine.log"), false)?;
        }
        Ok(Self {
            process: RefCell::new(process),
            input: RefCell::new(input),
            events,
            window: Cell::new(0),
            parent,
            bounds: Cell::new([0, 0, 1280, 720]),
            ended: Cell::new(false),
        })
    }
    pub fn command(&self, command: &str, value: bool) -> Result<()> {
        let mut input = self.input.borrow_mut();
        writeln!(
            input,
            "{}",
            serde_json::json!({"command":command,"value":value})
        )?;
        input.flush()?;
        Ok(())
    }
    pub fn poll(&self) -> Vec<PlayerEvent> {
        let mut output: Vec<_> = self.events.lock().unwrap().drain(..).collect();
        let shortcuts: Vec<_> = output
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Report(report) if report["type"] == "native_hotkey" => {
                    match report["action"].as_str() {
                        Some("fullscreen") => Some(PlayerEvent::Fullscreen),
                        Some("back") => Some(PlayerEvent::Back),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect();
        output.extend(shortcuts);
        for event in &output {
            if let PlayerEvent::Report(report) = event
                && report["type"] == "native_window"
                && let Some(handle) = report["hwnd"].as_i64()
            {
                let window = HWND(handle as *mut _);
                let mut owner = 0;
                unsafe {
                    GetWindowThreadProcessId(window, Some(&mut owner));
                }
                if owner == self.process.borrow().id()
                    && unsafe { IsChild(self.parent, window) }.as_bool()
                {
                    self.window.set(handle as isize);
                    let _ = self.resize(self.bounds.get());
                    let _ = self.command("focus", false);
                }
            }
        }
        if let Ok(Some(status)) = self.process.borrow_mut().try_wait()
            && !self.ended.replace(true)
        {
            if status.success() {
                output.push(PlayerEvent::Back);
            } else {
                output.push(PlayerEvent::Failure(format!("O runtime nativo encerrou com {status}. Consulte native-logs/engine.log no perfil do jogo.")));
            }
        }
        output
    }
    pub fn resize(&self, bounds: [i32; 4]) -> Result<()> {
        self.bounds.set(bounds);
        if self.window.get() != 0 {
            unsafe {
                MoveWindow(
                    HWND(self.window.get() as *mut _),
                    bounds[0],
                    bounds[1],
                    bounds[2].max(1),
                    bounds[3].max(1),
                    true,
                )?;
            }
        }
        Ok(())
    }
    pub fn hide(&self) {
        if self.window.get() != 0 {
            let _ = unsafe { ShowWindow(HWND(self.window.get() as *mut _), SW_HIDE) };
        }
    }
    pub fn show(&self) {
        if self.window.get() != 0 {
            let _ = unsafe { ShowWindow(HWND(self.window.get() as *mut _), SW_SHOW) };
        }
    }
    pub fn pid(&self) -> u32 {
        self.process.borrow().id()
    }
}
impl Drop for NativeChild {
    fn drop(&mut self) {
        let _ = self.command("stop", false);
        let process = self.process.get_mut();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        while std::time::Instant::now() < deadline {
            if matches!(process.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !matches!(process.try_wait(), Ok(Some(_))) {
            let _ = process.kill();
        }
        let _ = process.wait();
    }
}
fn read_output(
    output: impl Read + Send + 'static,
    events: Arc<Mutex<VecDeque<PlayerEvent>>>,
    log: PathBuf,
    protocol: bool,
) -> Result<()> {
    std::thread::Builder::new()
        .name("native-report".into())
        .spawn(move || {
            let mut reader = BufReader::new(output);
            let mut log = std::fs::File::create(log).ok();
            let mut logged = 0usize;
            loop {
                let mut line = Vec::with_capacity(512);
                let Ok(count) = reader.by_ref().take(8192).read_until(b'\n', &mut line) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                if logged + line.len() > 2 << 20 {
                    // Keep current failures visible after verbose engine startup,
                    // with the same bounded disk footprint.
                    if let Some(file) = &mut log {
                        let _ = file.set_len(0);
                        let _ = file.seek(SeekFrom::Start(0));
                    }
                    logged = 0;
                }
                if logged + line.len() <= 2 << 20 {
                    if let Some(file) = &mut log {
                        let _ = file.write_all(&line);
                    }
                    logged += line.len();
                }
                if line.last() != Some(&b'\n') {
                    continue;
                }
                if protocol
                    && let Some(json) = line.strip_prefix(b"WGR_EVENT ")
                    && let Ok(report) = serde_json::from_slice::<serde_json::Value>(json)
                {
                    let mut queue = events.lock().unwrap();
                    if queue.len() >= 128 {
                        queue.pop_front();
                    }
                    if report["type"] == "native_window" {
                        queue.push_back(PlayerEvent::Loaded);
                    }
                    queue.push_back(PlayerEvent::Report(report));
                }
            }
        })?;
    Ok(())
}
