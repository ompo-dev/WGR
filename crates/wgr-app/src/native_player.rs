//! Native core-port session. GPUI paints the frame; Wasmtime executes the binary.
use crate::player::{PlayerEvent, PlayerOptions};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};
use wgr_runtime::{BinaryContract, CoreInstance, NativeRuntime};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{GetFocus, SetFocus},
        WindowsAndMessaging::{IsChild, PostMessageW, WM_KEYDOWN, WM_KEYUP},
    },
};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct OrbitFrame {
    pub x: i32,
    pub y: i32,
    pub score: u32,
}
pub struct NativePlayer {
    parent: HWND,
    options: PlayerOptions,
    runtime: Option<Arc<NativeRuntime>>,
    core: Option<RefCell<CoreInstance>>,
    child: Option<crate::native_child::NativeChild>,
    state: Cell<OrbitFrame>,
    keys: RefCell<HashSet<String>>,
    events: RefCell<Vec<PlayerEvent>>,
    started: Instant,
    last_frame: Cell<Instant>,
    fps_at: Cell<Instant>,
    fps_frames: Cell<u64>,
    frames: Cell<u64>,
    loaded: Cell<bool>,
    sha256: String,
    restored: bool,
}
impl NativePlayer {
    pub fn create(parent: HWND, options: PlayerOptions) -> Result<Self> {
        ensure!(
            !options.cleanup_cache,
            "A limpeza do cache legado exige o backend de compatibilidade; nenhuma engine foi iniciada."
        );
        let path=options.binary_path.as_ref().context("Esta entrada não possui binário ou manifesto de port nativo. O player não abre páginas web.")?;
        let (contract, bytes) = BinaryContract::read(path)?;
        std::fs::create_dir_all(&options.spool)?;
        std::fs::write(
            options.spool.join("native-contract.json"),
            serde_json::to_vec_pretty(&contract)?,
        )?;
        if options.gta
            && contract.sha256 == "11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0"
        {
            let child = crate::native_child::NativeChild::spawn(parent, &options)?;
            let now = Instant::now();
            return Ok(Self {
                parent,
                options,
                runtime: None,
                core: None,
                child: Some(child),
                state: Cell::new(OrbitFrame {
                    x: 0,
                    y: 0,
                    score: 0,
                }),
                keys: RefCell::new(HashSet::new()),
                events: RefCell::new(vec![
                    PlayerEvent::Ready,
                    PlayerEvent::Capabilities(
                        serde_json::json!({"backend":"native","browser":false,"WebAssembly":true,"memory64":true}),
                    ),
                ]),
                started: now,
                last_frame: Cell::new(now),
                fps_at: Cell::new(now),
                fps_frames: Cell::new(0),
                frames: Cell::new(0),
                loaded: Cell::new(false),
                sha256: contract.sha256,
                restored: false,
            });
        }
        ensure!(
            contract.imports.is_empty(),
            "Port WASM reconhecido. Seu adaptador nativo ainda não está implementado: {} importações de host. Relatório: {}",
            contract.imports.len(),
            options.spool.join("native-contract.json").display()
        );
        ensure!(
            options.fixture
                && contract.sha256
                    == wgr_core::assets::sha256(include_bytes!(
                        "../../../fixtures/orbit/math.wasm"
                    )),
            "Este port ainda precisa de um adaptador de apresentação nativa. Ele não será substituído pela fixture Órbita."
        );
        ensure!(
            contract
                .exports
                .iter()
                .any(|e| e.name == "step" && e.kind == "function"),
            "O binário não implementa o contrato de execução core-step-v1"
        );
        let runtime = NativeRuntime::new()?;
        let module = runtime.compile(&bytes)?;
        let core = module.instantiate_core(64 * 1024 * 1024)?;
        std::fs::create_dir_all(&options.profile)?;
        let saved = std::fs::read(options.profile.join("native-orbit-save.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<OrbitFrame>(&b).ok());
        let mut state = saved.unwrap_or(OrbitFrame {
            x: 480,
            y: 210,
            score: 0,
        });
        state.x = state.x.clamp(14, 946);
        state.y = state.y.clamp(14, 406);
        let now = Instant::now();
        Ok(Self {
            parent,
            options,
            runtime: Some(runtime),
            core: Some(RefCell::new(core)),
            child: None,
            state: Cell::new(state),
            keys: RefCell::new(HashSet::new()),
            events: RefCell::new(vec![
                PlayerEvent::Ready,
                PlayerEvent::Capabilities(
                    serde_json::json!({"backend":"native","WebAssembly":true,"memory64":true,"native_threads":true,"native_gpu":true,"WebGPU":{"available":false},"AudioWorklet":false,"browser":false}),
                ),
            ]),
            started: now,
            last_frame: Cell::new(now),
            fps_at: Cell::new(now),
            fps_frames: Cell::new(0),
            frames: Cell::new(0),
            loaded: Cell::new(false),
            sha256: contract.sha256,
            restored: saved.is_some(),
        })
    }
    pub fn poll(&self) -> Vec<PlayerEvent> {
        let mut events = self.events.take();
        if let Some(child) = &self.child {
            events.extend(child.poll());
        }
        events
    }
    pub fn load_game(&self) -> Result<()> {
        if let Some(child) = &self.child {
            return child.command("focus", false);
        }
        self.loaded.set(true);
        self.events.borrow_mut().push(PlayerEvent::Loaded);
        Ok(())
    }
    pub fn frame(&self) -> OrbitFrame {
        let mut state = self.state.get();
        if !self.loaded.get() {
            return state;
        }
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_frame.replace(now))
            .as_secs_f32()
            .min(0.05);
        let keys = self.keys.borrow();
        let amount = (dt * 240.).round() as i32;
        let dx = (i32::from(keys.contains("right") || keys.contains("d"))
            - i32::from(keys.contains("left") || keys.contains("a")))
            * amount;
        let dy = (i32::from(keys.contains("down") || keys.contains("s"))
            - i32::from(keys.contains("up") || keys.contains("w")))
            * amount;
        if dx != 0 || dy != 0 {
            let result = (|| -> Result<()> {
                let mut core = self
                    .core
                    .as_ref()
                    .context("Core nativo ausente")?
                    .borrow_mut();
                state.x = core.step(state.x, dx)?.clamp(14, 946);
                state.y = core.step(state.y, dy)?.clamp(14, 406);
                Ok(())
            })();
            if let Err(error) = result {
                self.events
                    .borrow_mut()
                    .push(PlayerEvent::Failure(error.to_string()));
                self.loaded.set(false);
            }
            self.state.set(state);
            self.report_input();
        }
        self.frames.set(self.frames.get() + 1);
        self.fps_frames.set(self.fps_frames.get() + 1);
        if self.frames.get() == 2 {
            self.events.borrow_mut().push(PlayerEvent::Report(serde_json::json!({"type":"first_frame","engine":"wasmtime-native-gpui","backend":"native","binary_sha256":self.sha256,"frames":2,"position":state,"save":true,"restored":self.restored})));
        }
        let span = now.duration_since(self.fps_at.get()).as_secs_f64();
        if span >= 0.5 {
            self.events.borrow_mut().push(PlayerEvent::Report(serde_json::json!({"type":"fixture_fps","fps":self.fps_frames.replace(0) as f64/span,"frames":self.frames.get()})));
            self.fps_at.set(now);
        }
        state
    }
    fn report_input(&self) {
        self.events.borrow_mut().push(PlayerEvent::Report(serde_json::json!({"type":"fixture_input","position":self.state.get(),"backend":"native"})));
    }
    pub fn key(&self, key: &str, pressed: bool) {
        if pressed {
            self.keys.borrow_mut().insert(key.into());
        } else {
            self.keys.borrow_mut().remove(key);
        }
    }
    pub fn click(&self, x: i32, y: i32) {
        let mut state = self.state.get();
        state.x = x.clamp(14, 946);
        state.y = y.clamp(14, 406);
        self.state.set(state);
        self.report_input();
    }
    pub fn resize(&self, x: i32, y: i32, width: i32, height: i32) -> Result<()> {
        if let Some(child) = &self.child {
            return child.resize([x, y, width, height]);
        }
        Ok(())
    }
    pub fn focus(&self) {
        if let Some(child) = &self.child {
            child.show();
            let _ = child.command("focus", false);
            return;
        }
        let _ = unsafe { SetFocus(Some(self.parent)) };
    }
    pub fn hide(&self) {
        if let Some(child) = &self.child {
            child.hide();
        }
        self.keys.borrow_mut().clear();
    }
    pub fn mute(&self, muted: bool) -> Result<()> {
        if let Some(child) = &self.child {
            return child.command("mute", muted);
        }
        Ok(())
    }
    pub fn is_native(&self) -> bool {
        true
    }
    pub fn is_external(&self) -> bool {
        self.child.is_some()
    }
    pub fn runtime_version(&self) -> String {
        if self.child.is_some() {
            return "Rust / Wasmtime 50.0.0 / wgpu 30 / CPAL".into();
        }
        "Rust / Wasmtime 50.0.0 / GPUI".into()
    }
    pub fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }
    pub fn script(&self, _script: &str) -> Result<()> {
        bail!("O runtime nativo não executa JavaScript")
    }
    pub fn start_trace(&self, _path: PathBuf) -> Result<()> {
        bail!("Captura CDP é exclusiva do backend de compatibilidade")
    }
    pub fn end_trace(&self) -> Result<()> {
        bail!("O player nativo não inicia sessões CDP")
    }
    pub fn cpu_profile_status(&self) -> serde_json::Value {
        serde_json::json!({"backend":"native","frames":self.frames.get(),"browser":false,"native_pid":self.child.as_ref().map(|p|p.pid())})
    }
    pub fn capture(&self, path: PathBuf) -> Result<()> {
        crate::screenshot::capture_owned_frame(self.parent.0 as isize, &path)
    }
    pub fn smoke_key(&self) -> Result<()> {
        self.focus();
        let focus = unsafe { GetFocus() };
        ensure!(
            focus == self.parent || unsafe { IsChild(self.parent, focus) }.as_bool(),
            "Foco fora do player nativo"
        );
        unsafe {
            PostMessageW(Some(focus), WM_KEYDOWN, WPARAM(0x27), LPARAM(1))?;
        }
        let raw = focus.0 as isize;
        self.options.runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(raw as *mut _)),
                    WM_KEYUP,
                    WPARAM(0x27),
                    LPARAM(0xc0000001u32 as isize),
                )
            };
        });
        Ok(())
    }
    pub fn smoke_click(&self, _x: i32, _y: i32) -> Result<()> {
        bail!("Use o input nativo da janela; não há alvo DOM")
    }
}
impl Drop for NativePlayer {
    fn drop(&mut self) {
        if let Some(runtime) = &self.runtime {
            runtime.interrupt();
        }
        if self.child.is_some() {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec(&self.state.get()) {
            let path = self.options.profile.join("native-orbit-save.json");
            let temp = path.with_extension("partial");
            if std::fs::write(&temp, bytes).is_ok() {
                let _ = std::fs::rename(temp, path);
            }
        }
    }
}
