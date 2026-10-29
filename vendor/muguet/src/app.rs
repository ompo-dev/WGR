// muguet - app.rs
// Constantin Clerc (c22dev) - 0.1

use crate::{gpu, input, Runtime};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop};
use winit::keyboard::{ModifiersState, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

const TITLE: &str = "WGR — GTA · execução nativa";
const PICK_TITLE: &str = "WGR — carregando o binário do jogo";

pub fn content_viewport(
    width: u32,
    height: u32,
    render_width: u32,
    render_height: u32,
) -> (f32, f32, f32, f32) {
    if width == 0 || height == 0 || render_width == 0 || render_height == 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let scale = (width as f32 / render_width as f32).min(height as f32 / render_height as f32);
    let w = (render_width as f32 * scale).min(width as f32);
    let h = (render_height as f32 * scale).min(height as f32);
    ((width as f32 - w) * 0.5, (height as f32 - h) * 0.5, w, h)
}

fn double_escape(previous: &mut Option<std::time::Instant>, now: std::time::Instant) -> bool {
    let release =
        previous.is_some_and(|at| now.duration_since(at) <= std::time::Duration::from_millis(350));
    *previous = if release { None } else { Some(now) };
    release
}

#[cfg(test)]
mod capture_tests {
    use super::double_escape;
    use std::time::{Duration, Instant};
    #[test]
    fn viewport_fits_and_maps_letterboxed_windows() {
        assert_eq!(
            super::content_viewport(1920, 1200, 1280, 720),
            (0.0, 60.0, 1920.0, 1080.0)
        );
        assert_eq!(
            super::content_viewport(0, 0, 1280, 720),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(
            super::content_viewport(1280, 720, 1280, 720),
            (0.0, 0.0, 1280.0, 720.0)
        );
    }
    #[test]
    fn one_escape_keeps_capture_and_only_a_rapid_pair_releases_it() {
        let now = Instant::now();
        let mut previous = None;
        assert!(!double_escape(&mut previous, now));
        assert!(!double_escape(
            &mut previous,
            now + Duration::from_millis(500)
        ));
        assert!(double_escape(
            &mut previous,
            now + Duration::from_millis(700)
        ));
        assert!(!double_escape(
            &mut previous,
            now + Duration::from_millis(800)
        ));
        previous = None; // A focus change or another key cancels the gesture.
        assert!(!double_escape(
            &mut previous,
            now + Duration::from_millis(900)
        ));
    }
}

#[cfg(windows)]
fn native_handle(window: &Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as _),
        _ => None,
    }
}
fn has_input_focus(window: &Window) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::{
            Input::KeyboardAndMouse::GetFocus,
            WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOT},
        };
        native_handle(window).is_some_and(|handle| unsafe {
            GetForegroundWindow() == GetAncestor(handle, GA_ROOT) && GetFocus() == handle
        })
    }
    #[cfg(not(windows))]
    window.has_focus()
}
pub fn focus_window(window: &Window) {
    #[cfg(windows)]
    if let Some(handle) = native_handle(window) {
        use windows_sys::Win32::UI::{
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOT},
        };
        // Focus belongs to the child; activation belongs to WGR's top-level HWND.
        unsafe {
            if GetForegroundWindow() == GetAncestor(handle, GA_ROOT) {
                SetFocus(handle);
            }
        }
    }
    #[cfg(not(windows))]
    window.focus_window();
}

struct App {
    mode_chosen: bool,
    rt: Option<Arc<Runtime>>,
    startup: crate::config::Startup,
    args: Vec<String>,
    window: Option<Arc<Window>>,
    started: bool,

    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    launcher_shown: bool,
    locked: bool,
    focused: bool,
    last_escape: Option<std::time::Instant>,
    mods: ModifiersState,
    wheel_accum: f64,
}

fn backbuffer_size(window: &Window) -> (i32, i32) {
    let s = if cfg!(target_os = "ios") {
        window.outer_size()
    } else {
        window.inner_size()
    };
    let scale: f64 = std::env::var("MUGUET_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    if let Some((w, h)) = std::env::var("MUGUET_RES").ok().and_then(|r| {
        let (a, b) = r.split_once('x')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }) {
        return (w, h);
    }
    let (mut w, mut h) = (s.width as f64 * scale, s.height as f64 * scale);
    let k = (2560.0 / w).min(1440.0 / h).min(1.0);
    w *= k;
    h *= k;
    if cfg!(target_os = "ios") && w <= h * 1.5 {
        w = h * 1.5 + 2.0;
    }
    (((w as i32) & !1).max(320), ((h as i32) & !1).max(240))
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        gpu::SUSPENDED.store(false, std::sync::atomic::Ordering::Relaxed);

        #[cfg(target_os = "ios")]
        input::set_focus(true);
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(
                if self.args.iter().any(|a| a.starts_with("-wasmstartchoice"))
                    || std::env::var("MUGUET_START")
                        .map(|v| v != "0")
                        .unwrap_or(false)
                {
                    TITLE
                } else {
                    PICK_TITLE
                },
            )
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        #[cfg(windows)]
        let attrs = if let Some(parent) = std::env::var("WGR_PARENT_HWND")
            .ok()
            .and_then(|s| s.parse::<isize>().ok())
            .and_then(std::num::NonZeroIsize::new)
        {
            let handle = raw_window_handle::Win32WindowHandle::new(parent);
            // WGR owns the parent HWND throughout this child process's lifetime.
            unsafe {
                attrs
                    .with_parent_window(Some(raw_window_handle::RawWindowHandle::Win32(handle)))
                    .with_decorations(false)
            }
        } else {
            attrs
        };

        #[cfg(target_os = "ios")]
        let attrs = {
            use winit::platform::ios::{ScreenEdge, WindowAttributesExtIOS};
            attrs
                .with_prefers_status_bar_hidden(true)
                .with_prefers_home_indicator_hidden(true)
                .with_preferred_screen_edges_deferring_system_gestures(ScreenEdge::ALL)
        };
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        crate::bridge::window_created(&window);

        if std::env::var("MUGUET_FULLSCREEN")
            .map(|v| v != "0")
            .unwrap_or(false)
        {
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = gpu::native_backends();
        let instance = wgpu::Instance::new(desc);
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        *gpu::SURFACE.lock().unwrap() = Some(gpu::SurfaceHandoff {
            instance,
            surface,
            window: window.clone(),
        });
        let (w, h) = backbuffer_size(&window);
        input::set_size(w, h);
        eprintln!(
            "[app] window {}x{} px, back buffer {w}x{h}",
            window.inner_size().width,
            window.inner_size().height
        );
        #[cfg(target_os = "ios")]
        crate::touch::install(&window);
        self.window = Some(window);

        if !cfg!(target_os = "ios") {
            self.start_engine();
        }
    }

    fn suspended(&mut self, _el: &ActiveEventLoop) {
        gpu::SUSPENDED.store(true, std::sync::atomic::Ordering::Relaxed);
        input::set_focus(false);
        #[cfg(target_os = "ios")]
        crate::touch::release();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                crate::mp::shutdown();
                std::process::exit(0);
            }
            WindowEvent::Resized(_) => {
                // Windows' game-side DeviceLost/ResizeBuffers path corrupts its
                // allocator. Resize the native presentation surface instead.
                #[cfg(not(windows))]
                {
                    let (w, h) = backbuffer_size(&window);
                    input::set_size(w, h);
                }
                if self.locked {
                    self.lock(el, &window);
                }
            }
            WindowEvent::Occluded(hidden) => {
                gpu::OCCLUDED.store(hidden, std::sync::atomic::Ordering::Relaxed);
            }
            WindowEvent::Focused(_) => self.sync_focus(el, &window),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                self.sync_focus(el, &window);
                if !self.focused {
                    return;
                }
                if !self.mode_chosen && input::start_mode() != 0 {
                    self.mode_chosen = true;
                    window.set_title(TITLE);
                }

                if cfg!(target_vendor = "apple") && self.mods.super_key() {
                    if let PhysicalKey::Code(winit::keyboard::KeyCode::KeyQ) = event.physical_key {
                        crate::mp::shutdown();
                        std::process::exit(0);
                    }

                    if let PhysicalKey::Code(winit::keyboard::KeyCode::KeyF) = event.physical_key {
                        if event.state == ElementState::Pressed && !event.repeat {
                            let fs = window.fullscreen().is_some();
                            window.set_fullscreen(if fs {
                                None
                            } else {
                                Some(Fullscreen::Borderless(None))
                            });
                        }
                        return;
                    }
                    if event.state == ElementState::Pressed {
                        self.unlock(el, &window);
                    }
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    let name = format!("{code:?}");
                    if event.state == ElementState::Pressed && !event.repeat {
                        if code == winit::keyboard::KeyCode::Escape {
                            if double_escape(&mut self.last_escape, std::time::Instant::now()) {
                                self.unlock(el, &window);
                                return;
                            }
                        } else {
                            self.last_escape = None;
                        }
                        if code == winit::keyboard::KeyCode::F11 {
                            crate::bridge::emit(
                                serde_json::json!({"type":"native_hotkey","action":"fullscreen"}),
                            );
                            return;
                        }
                        if code == winit::keyboard::KeyCode::KeyB
                            && self.mods.control_key()
                            && self.mods.shift_key()
                        {
                            self.unlock(el, &window);
                            crate::bridge::emit(
                                serde_json::json!({"type":"native_hotkey","action":"back"}),
                            );
                            return;
                        }
                    }
                    if event.state == ElementState::Pressed && !event.repeat {
                        crate::mp::key_pressed(&name);
                    }
                    input::key(
                        &name,
                        event.state == ElementState::Pressed,
                        event.repeat,
                        event.text.as_deref(),
                        self.mods.control_key(),
                    );
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    self.last_escape = None;
                    focus_window(&window);
                    self.sync_focus(el, &window);
                    if self.focused && !self.locked {
                        self.lock(el, &window);
                    }
                }
                if !self.focused {
                    return;
                }
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    MouseButton::Back => 3,
                    MouseButton::Forward => 4,
                    MouseButton::Other(_) => return,
                };
                input::button(b, state == ElementState::Pressed);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if !self.locked {
                    self.cursor(&window, position);
                }
            }
            #[cfg(target_os = "ios")]
            WindowEvent::Touch(t) => crate::touch::touch(&t),
            WindowEvent::MouseWheel { delta, .. } => {
                self.wheel_accum += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64 / 3.0 * 3.0,
                    MouseScrollDelta::PixelDelta(p) => p.y / 100.0,
                };
                let n = self.wheel_accum.trunc();
                self.wheel_accum -= n;
                input::wheel(n as i32);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(window) = self.window.clone() {
            crate::bridge::commands(&window);
            self.sync_focus(el, &window);
        }
        crate::bridge::sample();
        if let Some(reason) = crate::host::stop_reason() {
            eprintln!("[native-error] {reason}");
            std::process::exit(75);
        }
        #[allow(unused_mut)]
        let mut wait = if crate::pad::poll() {
            8
        } else if cfg!(target_os = "ios") {
            250
        } else {
            50
        };
        #[cfg(target_os = "ios")]
        if crate::touch::refresh() {
            wait = 15;
        }
        #[cfg(target_os = "ios")]
        if crate::keyboard::poll() {
            wait = wait.min(8);
        }
        #[cfg(target_os = "ios")]
        if !self.started {
            self.launcher();
            wait = 50;
        }
        #[cfg(target_os = "ios")]
        if self.started && self.window.as_deref().is_some_and(crate::mouse::poll) {
            wait = wait.min(8);
        }
        el.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(wait),
        ));
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if self.locked && self.window.as_deref().is_some_and(has_input_focus) {
                input::mouse_delta_f(delta.0, delta.1);
            }
        }
    }
}

impl App {
    fn sync_focus(&mut self, el: &ActiveEventLoop, window: &Window) {
        let focused = has_input_focus(window);
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        input::set_focus(focused);
        if !focused {
            self.last_escape = None;
            self.mods = ModifiersState::empty();
            self.wheel_accum = 0.0;
            self.unlock(el, window);
        }
        crate::bridge::emit(
            serde_json::json!({"type":"native_input_focus","focused":focused,"captured":self.locked}),
        );
    }
    fn start_engine(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if self.started {
            return;
        }
        self.started = true;
        let (w, h) = backbuffer_size(&window);
        input::set_size(w, h);
        let (rt, startup) = (self.rt.clone(), self.startup.clone());
        let mut args = self.args.clone();
        args.extend([
            "-wgpu".to_string(),
            format!("-width={w}"),
            format!("-height={h}"),
        ]);
        std::thread::Builder::new()
            .name("engine-main".into())
            .stack_size(64 << 20)
            .spawn(move || {
                let rt = match rt.map(Ok).unwrap_or_else(|| crate::make_runtime(&startup)) {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("[host] {e:#}");
                        return;
                    }
                };
                if let Err(e) = crate::run_engine(&rt, args) {
                    eprintln!("[host] engine failed: {e:?}");
                }
                std::process::exit(0);
            })
            .expect("spawn engine thread");
    }

    #[cfg(target_os = "ios")]
    fn launcher(&mut self) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static PLAY: AtomicBool = AtomicBool::new(false);
        extern "C" fn played() {
            PLAY.store(true, Ordering::SeqCst);
        }
        extern "C" {
            fn muguet_show_launcher(view_controller: *mut std::ffi::c_void, done: extern "C" fn());
            fn muguet_hold_refresh(hz: f32);
            fn muguet_memory_limit() -> u64;
            fn muguet_memory_entitled() -> i32;
        }
        if self.started {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        if !self.launcher_shown {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let vc = match window.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::UiKit(h)) => h.ui_view_controller,
                _ => None,
            };
            self.launcher_shown = true;
            match vc {
                Some(vc) => unsafe { muguet_show_launcher(vc.as_ptr(), played) },
                None => PLAY.store(true, Ordering::SeqCst),
            }
        }
        if PLAY.load(Ordering::SeqCst) {
            let settings = crate::config::reload();
            let (limit, entitled) = unsafe { (muguet_memory_limit(), muguet_memory_entitled()) };
            eprintln!(
                "[host] iOS gives the game {} MB, increased memory limit entitlement: {}",
                limit >> 20,
                match entitled {
                    1 => "yes",
                    0 => "no",
                    _ => "unknown",
                }
            );
            let uncapped = settings.iter().any(|a| a == "-frameLimit=0");
            unsafe { muguet_hold_refresh(if uncapped { 120.0 } else { 60.0 }) };
            self.args.retain(|a| {
                !a.starts_with("-frameLimit=")
                    && !a.starts_with("-wasmstartchoice=")
                    && !a.starts_with("-FrameQueueLimit=")
            });
            self.args.extend(settings);
            self.start_engine();
        }
    }

    fn cursor(&self, window: &Window, p: PhysicalPosition<f64>) {
        let s = window.inner_size();
        let (w, h) = input::size();
        let (x, y, vw, vh) = content_viewport(s.width, s.height, w as u32, h as u32);
        input::mouse_pos(
            ((p.x - x as f64) * w as f64 / vw.max(1.0) as f64).round() as i32,
            ((p.y - y as f64) * h as f64 / vh.max(1.0) as f64).round() as i32,
        );
    }
    fn lock(&mut self, el: &ActiveEventLoop, window: &Window) {
        let ok = window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
            .is_ok();
        window.set_cursor_visible(!ok);
        self.locked = ok;
        input::set_locked(ok);
        // The foreground process owns the parent HWND. Receive raw mouse motion
        // for its embedded child, and discard it whenever that child loses focus.
        #[cfg(windows)]
        el.listen_device_events(if ok {
            DeviceEvents::Always
        } else {
            DeviceEvents::WhenFocused
        });
        crate::bridge::emit(serde_json::json!({"type":"native_input_capture","captured":ok}));
    }
    fn unlock(&mut self, el: &ActiveEventLoop, window: &Window) {
        if !self.locked {
            return;
        }
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ClipCursor(std::ptr::null());
        }
        window.set_cursor_visible(true);
        self.locked = false;
        input::set_locked(false);
        #[cfg(windows)]
        el.listen_device_events(DeviceEvents::WhenFocused);
        crate::bridge::emit(serde_json::json!({"type":"native_input_capture","captured":false}));
    }
}

pub fn run(
    rt: Option<Arc<Runtime>>,
    startup: crate::config::Startup,
    args: Vec<String>,
) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    input::set_focus(false);
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        mode_chosen: false,
        rt,
        startup,
        args,
        window: None,
        started: false,
        launcher_shown: false,
        locked: false,
        focused: false,
        last_escape: None,
        mods: ModifiersState::empty(),
        wheel_accum: 0.0,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
