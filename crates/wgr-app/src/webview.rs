//! WebView2 filho HWND: vida útil COM confinada à thread da janela GPUI.
use anyhow::{Context, Result, ensure};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, mpsc},
    time::Instant,
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use wgr_core::security::HttpClient;
use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
        UI::{
            Shell::{SHCreateMemStream, SHCreateStreamOnFileEx},
            WindowsAndMessaging::*,
        },
    },
    core::{Interface, PCWSTR, PWSTR, w},
};

use crate::player::{PlayerEvent, PlayerOptions};
struct Pending {
    args: ICoreWebView2WebResourceRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
}
struct BrokerReply {
    id: u64,
    result: Result<BrokerResponse>,
}
struct BrokerResponse {
    status: u16,
    headers: String,
    body: BrokerBody,
}
enum BrokerBody {
    Bytes(Vec<u8>),
    File(PathBuf),
}
struct Controller {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    environment: ICoreWebView2Environment,
    _log_receiver: Option<ICoreWebView2DevToolsProtocolEventReceiver>,
}
struct Inner {
    child: HWND,
    options: PlayerOptions,
    controller: RefCell<Option<Controller>>,
    events: RefCell<Vec<PlayerEvent>>,
    pending: RefCell<HashMap<u64, Pending>>,
    tx: mpsc::Sender<BrokerReply>,
    rx: mpsc::Receiver<BrokerReply>,
    next: Cell<u64>,
    closed: Cell<bool>,
    bounds: Cell<Option<[i32; 4]>>,
    trace_receiver: RefCell<Option<ICoreWebView2DevToolsProtocolEventReceiver>>,
    cpu_profiles: RefCell<Option<crate::cpu_profile::CpuProfiles>>,
    started: Instant,
    spooled: Arc<std::sync::atomic::AtomicU64>,
    slots: Arc<tokio::sync::Semaphore>,
}
pub struct GameWebView {
    inner: Rc<Inner>,
    _apartment: ComApartment,
}
struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        if !self.closed.get() {
            let _ = unsafe { DestroyWindow(self.child) };
        }
    }
}
impl GameWebView {
    pub fn is_native(&self) -> bool {
        false
    }
    pub fn create(parent: HWND, options: PlayerOptions) -> Result<Self> {
        unsafe {
            let style = GetWindowLongPtrW(parent, GWL_STYLE) as u32;
            SetWindowLongPtrW(parent, GWL_STYLE, (style | WS_CLIPCHILDREN.0) as isize);
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        let apartment = ComApartment;
        std::fs::create_dir_all(&options.profile)?;
        std::fs::create_dir_all(&options.spool)?;
        let child = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("WGR Player"),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                None,
                None,
            )
        }?;
        let (tx, rx) = mpsc::channel();
        let inner = Rc::new(Inner {
            child,
            options,
            controller: RefCell::new(None),
            events: RefCell::new(vec![]),
            pending: RefCell::new(HashMap::new()),
            tx,
            rx,
            next: Cell::new(0),
            closed: Cell::new(false),
            bounds: Cell::new(None),
            trace_receiver: RefCell::new(None),
            cpu_profiles: RefCell::new(None),
            started: Instant::now(),
            spooled: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        });
        let weak = Rc::downgrade(&inner);
        let user_data = CoTaskMemPWSTR::from(inner.options.profile.to_string_lossy().as_ref());
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |error, environment| {
                let Some(inner) = weak.upgrade().filter(|i| !i.closed.get()) else {
                    return Ok(());
                };
                let environment = match error.and_then(|_| {
                    environment.ok_or_else(|| {
                        windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER)
                    })
                }) {
                    Ok(v) => v,
                    Err(e) => {
                        inner.fail(format!("WebView2 indisponível: {e}"));
                        return Ok(());
                    }
                };
                let weak = Rc::downgrade(&inner);
                let environment_copy = environment.clone();
                let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |error, controller| {
                        let Some(inner) = weak.upgrade().filter(|i| !i.closed.get()) else {
                            return Ok(());
                        };
                        let controller = match error.and_then(|_| {
                            controller.ok_or_else(|| {
                                windows::core::Error::from_hresult(
                                    windows::Win32::Foundation::E_POINTER,
                                )
                            })
                        }) {
                            Ok(v) => v,
                            Err(e) => {
                                inner.fail(format!("Falha no player: {e}"));
                                return Ok(());
                            }
                        };
                        if let Err(e) = configure(&inner, controller, environment_copy) {
                            inner.fail(format!("Falha ao configurar player: {e:#}"));
                        }
                        Ok(())
                    },
                ));
                unsafe {
                    environment.CreateCoreWebView2Controller(inner.child, &handler)?;
                }
                Ok(())
            },
        ));
        unsafe {
            let settings = CoreWebView2EnvironmentOptions::default();
            settings.set_additional_browser_arguments("--disk-cache-size=268435456".into());
            let settings: ICoreWebView2EnvironmentOptions = settings.into();
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                *user_data.as_ref().as_pcwstr(),
                Some(&settings),
                &handler,
            )?;
        }
        Ok(Self {
            inner,
            _apartment: apartment,
        })
    }
    pub fn poll(&self) -> Vec<PlayerEvent> {
        while let Ok(reply) = self.inner.rx.try_recv() {
            if let Some(pending) = self.inner.pending.borrow_mut().remove(&reply.id) {
                let result = match reply.result {
                    Ok(response) => self.inner.respond(&pending.args, response),
                    Err(e) => {
                        let message = format!("Recurso não carregado: {e:#}");
                        self.inner.fail(message);
                        self.inner.respond(&pending.args,BrokerResponse{status:502,headers:"Content-Type: text/plain; charset=utf-8\r\nCross-Origin-Resource-Policy: same-origin\r\n".into(),body:BrokerBody::Bytes(b"WGR: recurso indisponivel; veja o diagnostico nativo".to_vec())})
                    }
                };
                if let Err(e) = result {
                    self.inner.fail(format!("Resposta do player: {e}"));
                }
                let _ = unsafe { pending.deferral.Complete() };
            }
        }
        std::mem::take(&mut *self.inner.events.borrow_mut())
    }
    pub fn load_game(&self) -> Result<()> {
        self.navigate(&self.inner.options.entry)
    }
    fn navigate(&self, url: &str) -> Result<()> {
        let state = self.inner.controller.borrow();
        let state = state.as_ref().context("Player inicializando")?;
        unsafe {
            state
                .webview
                .Navigate(*CoTaskMemPWSTR::from(url).as_ref().as_pcwstr())?;
        }
        Ok(())
    }
    pub fn resize(&self, x: i32, y: i32, width: i32, height: i32) -> Result<()> {
        let bounds = [x, y, width.max(1), height.max(1)];
        if self.inner.bounds.get() == Some(bounds) {
            return Ok(());
        }
        unsafe {
            SetWindowPos(
                self.inner.child,
                Some(HWND_TOP),
                x,
                y,
                width.max(1),
                height.max(1),
                SWP_NOACTIVATE,
            )?;
            let _ = ShowWindow(self.inner.child, SW_SHOWNA);
        }
        if let Some(state) = self.inner.controller.borrow().as_ref() {
            unsafe {
                state.controller.SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: width.max(1),
                    bottom: height.max(1),
                })?;
                state.controller.SetIsVisible(true)?;
            }
            self.inner.bounds.set(Some(bounds));
        }
        Ok(())
    }
    pub fn focus(&self) {
        if let Some(state) = self.inner.controller.borrow().as_ref() {
            let _ = unsafe {
                state
                    .controller
                    .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
            };
        }
    }
    pub fn mute(&self, muted: bool) -> Result<()> {
        if let Some(state) = self.inner.controller.borrow().as_ref() {
            let webview: ICoreWebView2_8 = state.webview.cast()?;
            unsafe {
                webview.SetIsMuted(muted)?;
            }
        }
        Ok(())
    }
    pub fn script(&self, script: &str) -> Result<()> {
        if let Some(state) = self.inner.controller.borrow().as_ref() {
            unsafe {
                state
                    .webview
                    .ExecuteScript(*CoTaskMemPWSTR::from(script).as_ref().as_pcwstr(), None)?;
            }
        }
        Ok(())
    }
    pub fn runtime_version(&self) -> String {
        let mut version = PWSTR::null();
        if let Some(state) = self.inner.controller.borrow().as_ref()
            && unsafe { state.environment.BrowserVersionString(&mut version) }.is_ok()
        {
            return CoTaskMemPWSTR::from(version).to_string();
        }
        "Não disponível".into()
    }
    pub fn elapsed_ms(&self) -> u128 {
        self.inner.started.elapsed().as_millis()
    }
    pub fn start_trace(&self, path: PathBuf) -> Result<()> {
        let webview = self
            .inner
            .controller
            .borrow()
            .as_ref()
            .context("Player indisponível")?
            .webview
            .clone();
        *self.inner.cpu_profiles.borrow_mut() = Some(crate::cpu_profile::CpuProfiles::start(
            webview.clone(),
            path.clone(),
        )?);
        let receiver =
            unsafe { webview.GetDevToolsProtocolEventReceiver(w!("Tracing.tracingComplete"))? };
        let weak = Rc::downgrade(&self.inner);
        let mut token = 0;
        unsafe {
            receiver.add_DevToolsProtocolEventReceived(
                &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                    if let (Some(inner), Some(args)) = (weak.upgrade(), args) {
                        let mut raw = PWSTR::null();
                        args.ParameterObjectAsJson(&mut raw)?;
                        let value = serde_json::from_str::<serde_json::Value>(
                            &CoTaskMemPWSTR::from(raw).to_string(),
                        )
                        .unwrap_or_default();
                        if let Some(stream) = value["stream"].as_str() {
                            match std::fs::File::create_new(&path) {
                                Ok(file) => read_trace_chunk(
                                    Rc::downgrade(&inner),
                                    stream.into(),
                                    Rc::new(RefCell::new(file)),
                                    Rc::new(Cell::new(0)),
                                ),
                                Err(error) => trace_report(&inner, "error", &error.to_string(), 0),
                            }
                        } else {
                            trace_report(&inner, "error", "Tracing não forneceu stream", 0);
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        *self.inner.trace_receiver.borrow_mut() = Some(receiver);
        let weak = Rc::downgrade(&self.inner);
        let options=serde_json::json!({"transferMode":"ReturnAsStream","streamFormat":"json","traceConfig":{"recordMode":"recordContinuously","traceBufferSizeInKb":4096,"includedCategories":["v8","v8.wasm","disabled-by-default-v8.gc","devtools.timeline","blink.user_timing","renderer.scheduler"]}}).to_string();
        unsafe {
            webview.CallDevToolsProtocolMethod(
                w!("Tracing.start"),
                *CoTaskMemPWSTR::from(options.as_str()).as_ref().as_pcwstr(),
                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, _| {
                    if let Some(inner) = weak.upgrade() {
                        match result {
                            Ok(()) => trace_report(&inner, "recording", "", 0),
                            Err(error) => trace_report(&inner, "error", &error.to_string(), 0),
                        }
                    }
                    Ok(())
                })),
            )?;
        }
        Ok(())
    }
    pub fn end_trace(&self) -> Result<()> {
        if let Some(profiles) = self.inner.cpu_profiles.borrow().as_ref() {
            profiles.stop();
        }
        if let Some(state) = self.inner.controller.borrow().as_ref() {
            unsafe {
                state.webview.CallDevToolsProtocolMethod(
                    w!("Tracing.end"),
                    w!("{}"),
                    &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
                )?;
            }
        }
        Ok(())
    }
    pub fn cpu_profile_status(&self) -> serde_json::Value {
        self.inner
            .cpu_profiles
            .borrow()
            .as_ref()
            .map(|profiles| profiles.status())
            .unwrap_or_default()
    }
    pub fn hide(&self) {
        self.inner.bounds.set(None);
        if let Some(state) = self.inner.controller.borrow().as_ref() {
            let _ = unsafe { state.controller.SetIsVisible(false) };
        }
        let _ = unsafe { ShowWindow(self.inner.child, SW_HIDE) };
    }
    pub fn capture(&self, path: PathBuf) -> Result<()> {
        if let Some(state) = self.inner.controller.borrow().as_ref() {
            let stream = unsafe {
                SHCreateStreamOnFileEx(
                    *CoTaskMemPWSTR::from(path.to_string_lossy().as_ref())
                        .as_ref()
                        .as_pcwstr(),
                    0x1001,
                    0,
                    true,
                    None,
                )
            }?;
            let keep = stream.clone();
            let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
                let _ = keep;
                result
            }));
            unsafe {
                state.webview.CapturePreview(
                    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                    &stream,
                    &handler,
                )?;
            }
        }
        Ok(())
    }
    /// Sends only to the currently focused descendant of this game's own HWND.
    pub fn smoke_key(&self) -> Result<()> {
        use windows::Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::Input::KeyboardAndMouse::GetFocus,
        };
        self.focus();
        let focus = unsafe { GetFocus() };
        ensure!(
            unsafe { IsChild(self.inner.child, focus) }.as_bool(),
            "Foco fora do player"
        );
        unsafe {
            PostMessageW(Some(focus), WM_KEYDOWN, WPARAM(0x27), LPARAM(1))?;
        }
        let raw = focus.0 as isize;
        self.inner.options.runtime.spawn(async move {
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
    pub fn smoke_click(&self, x: i32, y: i32) -> Result<()> {
        use windows::Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::Input::KeyboardAndMouse::GetFocus,
        };
        self.focus();
        let focus = unsafe { GetFocus() };
        ensure!(
            unsafe { IsChild(self.inner.child, focus) }.as_bool(),
            "Foco fora do player"
        );
        let mut bounds = RECT::default();
        unsafe {
            GetClientRect(focus, &mut bounds)?;
        }
        ensure!(
            x >= 0 && y >= 0 && x < bounds.right && y < bounds.bottom,
            "Clique de teste fora da viewport"
        );
        let position = LPARAM(((y as u32) << 16 | (x as u32 & 0xffff)) as isize);
        unsafe {
            PostMessageW(Some(focus), WM_MOUSEMOVE, WPARAM(0), position)?;
            PostMessageW(Some(focus), WM_LBUTTONDOWN, WPARAM(1), position)?;
            PostMessageW(Some(focus), WM_LBUTTONUP, WPARAM(0), position)?;
        }
        Ok(())
    }
}
impl Drop for GameWebView {
    fn drop(&mut self) {
        self.inner.closed.set(true);
        if let Some(state) = self.inner.controller.borrow_mut().take() {
            let _ = unsafe { state.webview.Stop() };
            let _ = unsafe { state.controller.Close() };
        }
        for (_, p) in self.inner.pending.borrow_mut().drain() {
            let _ = unsafe { p.deferral.Complete() };
        }
        let _ = unsafe { DestroyWindow(self.inner.child) };
    }
}
impl Inner {
    fn fail(&self, message: String) {
        self.events.borrow_mut().push(PlayerEvent::Failure(message));
    }
    fn respond(
        &self,
        args: &ICoreWebView2WebResourceRequestedEventArgs,
        response: BrokerResponse,
    ) -> Result<()> {
        let state = self.controller.borrow();
        let state = state.as_ref().context("Player encerrado")?;
        let stream = match response.body {
            BrokerBody::Bytes(bytes) => {
                unsafe { SHCreateMemStream(Some(&bytes)) }.context("Falha ao criar stream")?
            }
            BrokerBody::File(path) => unsafe {
                SHCreateStreamOnFileEx(
                    *CoTaskMemPWSTR::from(path.to_string_lossy().as_ref())
                        .as_ref()
                        .as_pcwstr(),
                    0x40,
                    0,
                    false,
                    None,
                )
            }?,
        };
        let headers = CoTaskMemPWSTR::from(response.headers.as_str());
        let reason = CoTaskMemPWSTR::from(if response.status < 400 {
            "OK"
        } else {
            "WGR error"
        });
        unsafe {
            let resource = state.environment.CreateWebResourceResponse(
                &stream,
                i32::from(response.status),
                *reason.as_ref().as_pcwstr(),
                *headers.as_ref().as_pcwstr(),
            )?;
            args.SetResponse(&resource)?;
        }
        Ok(())
    }
}
fn configure(
    inner: &Rc<Inner>,
    controller: ICoreWebView2Controller,
    environment: ICoreWebView2Environment,
) -> Result<()> {
    let webview = unsafe { controller.CoreWebView2()? };
    if inner.options.cleanup_cache {
        let profile: ICoreWebView2Profile2 =
            unsafe { webview.cast::<ICoreWebView2_13>()?.Profile()? }.cast()?;
        let weak = Rc::downgrade(inner);
        unsafe {
            profile.ClearBrowsingData(COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE|COREWEBVIEW2_BROWSING_DATA_KINDS_CACHE_STORAGE,&ClearBrowsingDataCompletedHandler::create(Box::new(move|result|{if let Some(inner)=weak.upgrade(){inner.events.borrow_mut().push(PlayerEvent::Report(serde_json::json!({"type":"profile_native_cache_cleared","ok":result.is_ok()})));}result})))?;
        }
    }
    let settings = unsafe { webview.Settings()? };
    unsafe {
        settings.SetAreDevToolsEnabled(false)?;
        settings.SetAreHostObjectsAllowed(false)?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        settings.SetIsZoomControlEnabled(false)?;
        settings.SetIsWebMessageEnabled(true)?;
    }
    if let Ok(settings) = settings.cast::<ICoreWebView2Settings4>() {
        unsafe {
            settings.SetIsPasswordAutosaveEnabled(false)?;
            settings.SetIsGeneralAutofillEnabled(false)?;
        }
    }
    if let Ok(view) = webview.cast::<ICoreWebView2_8>() {
        unsafe {
            view.SetIsMuted(inner.options.muted)?;
        }
    }
    let weak = Rc::downgrade(inner);
    let mut token = 0;
    let log_receiver = if inner.options.gta && inner.options.diagnostic {
        let receiver = unsafe { webview.GetDevToolsProtocolEventReceiver(w!("Log.entryAdded"))? };
        let weak = Rc::downgrade(inner);
        unsafe {
            receiver.add_DevToolsProtocolEventReceived(
                &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                    if let (Some(inner), Some(args)) = (weak.upgrade(), args) {
                        let mut raw = PWSTR::null();
                        args.ParameterObjectAsJson(&mut raw)?;
                        let raw = CoTaskMemPWSTR::from(raw).to_string();
                        if raw.len() < 16384
                            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
                        {
                            let text = value["entry"]["text"].as_str().unwrap_or("");
                            let relevant = text.starts_with("[wgpu]")
                                || text.contains("[wasm] mem:")
                                || text.contains("engine waits on the GPU")
                                || value["entry"]["level"] == "error";
                            if !relevant {
                                return Ok(());
                            }
                            inner.events.borrow_mut().push(PlayerEvent::Report(
                                serde_json::json!({"type":"browser_log","payload":value}),
                            ));
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            webview.CallDevToolsProtocolMethod(
                w!("Log.enable"),
                w!("{}"),
                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
            )?;
        }
        Some(receiver)
    } else {
        None
    };
    if inner.options.gta {
        unsafe {
            webview.AddScriptToExecuteOnDocumentCreated(
                *CoTaskMemPWSTR::from(include_str!("session_observer.js"))
                    .as_ref()
                    .as_pcwstr(),
                None,
            )?;
        }
    }
    unsafe {
        webview.add_NavigationStarting(&NavigationStartingEventHandler::create(Box::new(move|_,args|{
        if let(Some(inner),Some(args))=(weak.upgrade(),args){let mut ptr=PWSTR::null();args.Uri(&mut ptr)?;let uri=CoTaskMemPWSTR::from(ptr).to_string();
            let allowed=url::Url::parse(&uri).is_ok_and(|u|u.origin().ascii_serialization()==inner.options.origin);
            if !allowed {args.SetCancel(true)?;inner.fail("Navegação externa bloqueada. Adicione a origem como uma fonte separada.".into());}
        }Ok(())})),&mut token)?;
    }
    let weak = Rc::downgrade(inner);
    unsafe {
        webview.add_NavigationCompleted(&NavigationCompletedEventHandler::create(Box::new(move|sender,args|{
        if let(Some(inner),Some(sender),Some(args))=(weak.upgrade(),sender,args){let mut ok=windows::core::BOOL::default();args.IsSuccess(&mut ok)?;
            if !ok.as_bool(){inner.fail("A página não pôde ser carregada. Consulte origem, runtime e recursos.".into());return Ok(())}
            let mut ptr=PWSTR::null();sender.Source(&mut ptr)?;let uri=CoTaskMemPWSTR::from(ptr).to_string();
            if url::Url::parse(&uri).is_ok_and(|u|u.path()=="/__wgr_probe") {
                let script=format!("{}.then(payload=>chrome.webview.postMessage(JSON.stringify({{type:'capabilities',payload}}))).catch(e=>chrome.webview.postMessage(JSON.stringify({{type:'probe_error',error:String(e)}})))",include_str!("probe.js").trim().trim_end_matches(';'));
                sender.ExecuteScript(*CoTaskMemPWSTR::from(script.as_str()).as_ref().as_pcwstr(),None)?;
            }else{inner.events.borrow_mut().push(PlayerEvent::Loaded);
                let script=format!("{}.then(payload=>chrome.webview.postMessage(JSON.stringify({{type:'page_capabilities',payload}})))",include_str!("probe.js").trim().trim_end_matches(';'));
                sender.ExecuteScript(*CoTaskMemPWSTR::from(script.as_str()).as_ref().as_pcwstr(),None)?;
            }
        }Ok(())})),&mut token)?;
    }
    let weak = Rc::downgrade(inner);
    unsafe {
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                if let (Some(inner), Some(args)) = (weak.upgrade(), args) {
                    let mut ptr = PWSTR::null();
                    args.Source(&mut ptr)?;
                    let source = CoTaskMemPWSTR::from(ptr).to_string();
                    let Ok(url) = url::Url::parse(&source) else {
                        return Ok(());
                    };
                    if url.origin().ascii_serialization() != inner.options.origin {
                        return Ok(());
                    }
                    let mut ptr = PWSTR::null();
                    if args.TryGetWebMessageAsString(&mut ptr).is_err() {
                        return Ok(());
                    }
                    let message = CoTaskMemPWSTR::from(ptr).to_string();
                    if message.len() > 65536 {
                        return Ok(());
                    }
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&message) {
                        let kind = value["type"].as_str().unwrap_or("");
                        if kind == "capabilities" && url.path() == "/__wgr_probe" {
                            inner
                                .events
                                .borrow_mut()
                                .push(PlayerEvent::Capabilities(value["payload"].clone()));
                        } else if kind == "profile_cache_cleared" && url.path() == "/__wgr_cleanup"
                        {
                            inner.events.borrow_mut().push(PlayerEvent::Report(value));
                        } else if kind == "page_capabilities" && url.path() != "/__wgr_probe" {
                            inner
                                .events
                                .borrow_mut()
                                .push(PlayerEvent::PageCapabilities(value["payload"].clone()));
                        } else if kind == "probe_error" {
                            inner.fail("Falha ao verificar capacidades".into());
                        } else if (inner.options.fixture
                            && [
                                "first_frame",
                                "smoke_target",
                                "fixture_input",
                                "fixture_fps",
                                "fixture_audio",
                                "fixture_webgpu",
                                "fixture_error",
                            ]
                            .contains(&kind))
                            || (inner.options.gta
                                && [
                                    "game-fps",
                                    "game-progress",
                                    "game-crash",
                                    "port_state",
                                    "page_error",
                                ]
                                .contains(&kind))
                        {
                            inner.events.borrow_mut().push(PlayerEvent::Report(value));
                        }
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    if let Ok(v) = webview.cast::<ICoreWebView2_4>() {
        unsafe {
            v.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
    }
    let weak = Rc::downgrade(inner);
    unsafe {
        webview.add_ProcessFailed(&ProcessFailedEventHandler::create(Box::new(move|_,_|{if let Some(inner)=weak.upgrade(){inner.fail("O processo WebView2 foi interrompido. Retorne à biblioteca para reiniciar a sessão.".into());}Ok(())})),&mut token)?;
    }
    let weak = Rc::downgrade(inner);
    unsafe {
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                if let (Some(inner), Some(args)) = (weak.upgrade(), args) {
                    let mut key = 0;
                    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
                    args.VirtualKey(&mut key)?;
                    args.KeyEventKind(&mut kind)?;
                    if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                        let ctrl =
                            windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x11) < 0;
                        let shift =
                            windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) < 0;
                        if key == 0x42 && ctrl && shift {
                            args.SetHandled(true)?;
                            inner.events.borrow_mut().push(PlayerEvent::Back);
                        }
                        if key == 0x7a {
                            args.SetHandled(true)?;
                            inner.events.borrow_mut().push(PlayerEvent::Fullscreen);
                        }
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    // Requests retain a stable per-game HTTPS origin while the Rust server uses port :0.
    // All broker work runs off the UI thread. No native commands are exposed to game JS.
    let filter: ICoreWebView2_22 = webview
        .cast()
        .context("Atualize WebView2: o isolamento de Workers exige a interface de filtros v22")?;
    unsafe {
        filter.AddWebResourceRequestedFilterWithRequestSourceKinds(
            w!("*"),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        )?;
    }
    let weak = Rc::downgrade(inner);
    unsafe {
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                if let (Some(inner), Some(args)) = (weak.upgrade(), args)
                    && let Err(error) = request(&inner, args)
                {
                    inner.fail(format!("Requisição bloqueada: {error:#}"));
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    let url = format!("{}/__wgr_probe", inner.options.origin);
    unsafe {
        controller.SetIsVisible(true)?;
        webview.Navigate(*CoTaskMemPWSTR::from(url.as_str()).as_ref().as_pcwstr())?;
    }
    *inner.controller.borrow_mut() = Some(Controller {
        controller,
        webview,
        environment,
        _log_receiver: log_receiver,
    });
    inner.events.borrow_mut().push(PlayerEvent::Ready);
    Ok(())
}

fn request(inner: &Rc<Inner>, args: ICoreWebView2WebResourceRequestedEventArgs) -> Result<()> {
    let request = unsafe { args.Request()? };
    let mut ptr = PWSTR::null();
    unsafe {
        request.Uri(&mut ptr)?;
    }
    let uri = CoTaskMemPWSTR::from(ptr).to_string();
    if inner.options.cleanup_cache
        && url::Url::parse(&uri)
            .is_ok_and(|u| !["/__wgr_probe", "/__wgr_cleanup"].contains(&u.path()))
    {
        return inner.respond(
            &args,
            BrokerResponse {
                status: 404,
                headers: "Cache-Control: no-store\r\n".into(),
                body: BrokerBody::Bytes(vec![]),
            },
        );
    }
    // Local game resources have a real, stable loopback origin. Let Chromium
    // stream them directly; synchronous importScripts must not wait on a COM
    // response deferral while its parent worker is blocked.
    if inner.options.loopback.is_some()
        && url::Url::parse(&uri).is_ok_and(|u| {
            u.origin().ascii_serialization() == inner.options.origin
                && !["/__wgr_probe", "/__wgr_cleanup"].contains(&u.path())
        })
    {
        return Ok(());
    }
    let url = url::Url::parse(&uri)?;
    if url.scheme() == "blob" && url.origin().ascii_serialization() == inner.options.origin {
        return Ok(());
    }
    if url.origin().ascii_serialization() == inner.options.origin && url.path() == "/__wgr_probe" {
        return inner.respond(&args,BrokerResponse{status:200,headers:"Content-Type: text/html; charset=utf-8\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nCross-Origin-Resource-Policy: same-origin\r\nCache-Control: no-store\r\n".into(),body:BrokerBody::Bytes(b"<!doctype html><html lang=pt-BR><title>WGR - verificando runtime</title><body style='background:#11151c;color:#e5eaf1;font:16px Segoe UI;padding:40px'>Verificando capacidades do runtime...</body></html>".to_vec())});
    }
    if inner.options.cleanup_cache && url.path() == "/__wgr_cleanup" {
        return inner.respond(&args,BrokerResponse{status:200,headers:"Content-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\n".into(),body:BrokerBody::Bytes(include_bytes!("clear_cache.html").to_vec())});
    }
    if inner.options.gta
        && inner.options.loopback.is_none()
        && inner.options.origin == "https://fiveplay.lol"
    {
        if inner.options.policy.validate(&uri).is_err() {
            return inner.respond(
                &args,
                BrokerResponse {
                    status: 403,
                    headers: "Cache-Control: no-store\r\n".into(),
                    body: BrokerBody::Bytes(vec![]),
                },
            );
        }
        ensure!(
            ["https://fiveplay.lol", "https://cdn.fiveplay.lol"]
                .contains(&url.origin().ascii_serialization().as_str()),
            "Origem não autorizada para o jogo"
        );
        // Preserve native HTTP/2, redirects and immutable cache. Only this
        // hash-verified IO worker is adapted to bound persistent data.
        if !(url.origin().ascii_serialization() == inner.options.origin
            && (url.path().ends_with("/io_worker.js")
                || url.path().ends_with("/loader.js")
                || (url.path().ends_with("/game.wasm")
                    && url
                        .query_pairs()
                        .any(|(key, value)| key == "wgr_native" && value == "1"))))
        {
            return Ok(());
        }
    }
    let result = (|| -> Result<_> {
        let mut ptr = PWSTR::null();
        unsafe {
            request.Method(&mut ptr)?;
        }
        let method = CoTaskMemPWSTR::from(ptr).to_string();
        ensure!(
            ["GET", "HEAD", "POST"].contains(&method.as_str()),
            "Método não permitido no player"
        );
        let destination = if let Some(loopback) = &inner.options.loopback {
            ensure!(
                url.origin().ascii_serialization() == inner.options.origin,
                "O pacote local não tem permissão para acessar origens externas"
            );
            format!(
                "{loopback}{}{}",
                url.path(),
                url.query().map_or(String::new(), |q| format!("?{q}"))
            )
        } else {
            inner.options.policy.validate(&uri)?;
            uri.clone()
        };
        let request_headers = unsafe { request.Headers()? };
        let mut headers = reqwest::header::HeaderMap::new();
        for name in [
            "range",
            "if-none-match",
            "content-type",
            "accept",
            "accept-encoding",
        ] {
            let mut ptr = PWSTR::null();
            if unsafe {
                request_headers
                    .GetHeader(*CoTaskMemPWSTR::from(name).as_ref().as_pcwstr(), &mut ptr)
            }
            .is_ok()
            {
                let value = CoTaskMemPWSTR::from(ptr).to_string();
                if !value.is_empty() {
                    headers.insert(
                        reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
                        reqwest::header::HeaderValue::from_str(&value)?,
                    );
                }
            }
        }
        headers.insert(
            "accept-encoding",
            reqwest::header::HeaderValue::from_static("identity"),
        );
        let mut body = vec![];
        if method == "POST" {
            let content = unsafe { request.Content()? };
            let mut buffer = [0u8; 8192];
            loop {
                let mut count = 0;
                unsafe {
                    content
                        .Read(
                            buffer.as_mut_ptr().cast(),
                            buffer.len() as u32,
                            Some(&mut count),
                        )
                        .ok()?;
                }
                if count == 0 {
                    break;
                }
                ensure!(
                    body.len() + count as usize <= 1024 * 1024,
                    "Corpo do pedido excede 1 MiB"
                );
                body.extend_from_slice(&buffer[..count as usize]);
            }
        }
        Ok((method, destination, headers, body))
    })();
    let (method, destination, headers, body) = match result {
        Ok(v) => v,
        Err(error) => {
            inner.respond(
                &args,
                BrokerResponse {
                    status: 403,
                    headers: "Content-Type: text/plain\r\n".into(),
                    body: BrokerBody::Bytes(b"WGR: origem ou operacao nao autorizada".to_vec()),
                },
            )?;
            return Err(error);
        }
    };
    ensure!(
        inner.pending.borrow().len() < 256,
        "Requisições pendentes em excesso"
    );
    let id = inner.next.get();
    inner.next.set(id + 1);
    let deferral = unsafe { args.GetDeferral()? };
    inner
        .pending
        .borrow_mut()
        .insert(id, Pending { args, deferral });
    let tx = inner.tx.clone();
    let spool = inner.options.spool.clone();
    let local = inner.options.loopback.is_some();
    let native_gta = inner.options.gta && !local && inner.options.origin == "https://fiveplay.lol";
    let native_engine = native_gta && url.path().ends_with("/game.wasm");
    let native_loader = native_gta && url.path().ends_with("/loader.js");
    let cache = inner.options.cache.clone();
    let mut headers = headers;
    if native_gta {
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("identity"),
        );
    }
    let policy = inner.options.policy.clone();
    let used = inner.spooled.clone();
    let slots = inner.slots.clone();
    inner.options.runtime.spawn(async move {
        let result = async {
            let _permit = slots.acquire().await?;
            if local {
                use futures_util::StreamExt;
                use tokio::io::AsyncWriteExt;
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(std::time::Duration::from_secs(60))
                    .build()?;
                let response = client
                    .request(reqwest::Method::from_bytes(method.as_bytes())?, destination)
                    .headers(headers)
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
                let status = response.status().as_u16();
                let headers = headers_string(response.headers());
                let path = spool.join(format!("{id}.response"));
                let mut file = tokio::fs::File::create(&path).await?;
                let mut stream = response.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
                    let previous =
                        used.fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    ensure!(
                        previous + chunk.len() as u64 <= 2 * 1024 * 1024 * 1024,
                        "Spool da sessão excede 2 GiB; reinicie a sessão"
                    );
                    file.write_all(&chunk).await?;
                }
                drop(file);
                Ok(BrokerResponse {
                    status,
                    headers,
                    body: BrokerBody::File(path),
                })
            } else {
                let client = HttpClient::new(policy);
                if native_engine {
                    let path = url::Url::parse(&destination)?.path().to_string();
                    let key = format!("gta-native-wasm:{path}");
                    let reader = cache.clone(); let read_key = key.clone();
                    let cached = tokio::task::spawn_blocking(move || reader.get(&read_key)).await??;
                    let file = if let Some(file) = cached { file } else {
                        let reply = client.request(reqwest::Method::GET, &format!("https://cdn.fiveplay.lol{path}"), reqwest::header::HeaderMap::new(), 128 * 1024 * 1024).await?;
                        ensure!(reply.status.is_success() && reply.body.starts_with(b"\0asm\x01\0\0\0"), "A engine não retornou um módulo WebAssembly válido");
                        let writer = cache.clone();
                        tokio::task::spawn_blocking(move || writer.put(&key, &reply.body)).await??
                    };
                    let size = std::fs::metadata(&file)?.len();
                    return Ok(BrokerResponse { status:200, headers:format!("Content-Type: application/wasm\r\nContent-Length: {size}\r\nCache-Control: private, max-age=31536000, immutable\r\n"), body:BrokerBody::File(file) });
                }
                let reply = client
                    .request_body(
                        reqwest::Method::from_bytes(method.as_bytes())?,
                        &destination,
                        headers,
                        if native_gta { 256 * 1024 } else { 128 * 1024 * 1024 },
                        body,
                    )
                    .await?;
                if native_gta {
                    if native_loader {
                        let io_url = reply.final_url.join("io_worker.js")?;
                        let io = client.request(reqwest::Method::GET, io_url.as_str(), reqwest::header::HeaderMap::new(), 256 * 1024).await?;
                        let gpu_url = reply.final_url.join("wgpu_worker.js")?;
                        let gpu = client.request(reqwest::Method::GET, gpu_url.as_str(), reqwest::header::HeaderMap::new(), 256 * 1024).await?;
                        let bytes = crate::gta_browser::loader(&reply.body, &io.body, &gpu.body)?;
                        return Ok(BrokerResponse {status:200,headers:"Content-Type: text/javascript; charset=utf-8\r\nCache-Control: no-store\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nCross-Origin-Resource-Policy: same-origin\r\n".into(),body:BrokerBody::Bytes(bytes)});
                    }
                    ensure!(reply.status.is_success() && wgr_core::assets::sha256(&reply.body) == "d91c89e64f7f280666b5c9d1669ed6d65d66605077264569a49146ed79fe0bae", "Worker da origem original incompatível com o cache WGR");
                    let mut bytes = b"'use strict';self.WGR_PERSISTENT_CACHE=true;\n".to_vec();
                    bytes.extend_from_slice(&reply.body);
                    bytes.extend_from_slice(b"\n");
                    bytes.extend_from_slice(wgr_http::RAM_IO_SCRIPT.as_bytes());
                    return Ok(BrokerResponse {status:200,headers:"Content-Type: text/javascript; charset=utf-8\r\nCache-Control: no-store\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nCross-Origin-Resource-Policy: same-origin\r\n".into(),body:BrokerBody::Bytes(bytes)});
                }
                Ok(BrokerResponse {
                    status: reply.status.as_u16(),
                    headers: headers_string(&reply.headers),
                    body: BrokerBody::Bytes(reply.body),
                })
            }
        }
        .await;
        let _ = tx.send(BrokerReply { id, result });
    });
    Ok(())
}
fn trace_report(inner: &Inner, status: &str, message: &str, bytes: usize) {
    inner.events.borrow_mut().push(PlayerEvent::Report(serde_json::json!({"type":"trace_state","payload":{"status":status,"message":message,"bytes":bytes}})));
}
fn read_trace_chunk(
    weak: std::rc::Weak<Inner>,
    stream: String,
    file: Rc<RefCell<std::fs::File>>,
    used: Rc<Cell<usize>>,
) {
    let Some(inner) = weak.upgrade().filter(|i| !i.closed.get()) else {
        return;
    };
    let Some(webview) = inner
        .controller
        .borrow()
        .as_ref()
        .map(|c| c.webview.clone())
    else {
        return;
    };
    let parameters = serde_json::json!({"handle":stream,"size":65536}).to_string();
    let close_view = webview.clone();
    let used_at_call = used.get();
    let result = unsafe {
        webview.CallDevToolsProtocolMethod(
            w!("IO.read"),
            *CoTaskMemPWSTR::from(parameters.as_str())
                .as_ref()
                .as_pcwstr(),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, raw| {
                let Some(inner) = weak.upgrade() else {
                    return Ok(());
                };
                let chunk = (|| -> Result<(String, bool)> {
                    result?;
                    let value: serde_json::Value = serde_json::from_str(&raw)?;
                    ensure!(
                        value["base64Encoded"] != true,
                        "Encoding inesperado do trace JSON"
                    );
                    let data = value["data"].as_str().context("Stream sem dados")?;
                    ensure!(
                        used.get() + data.len() <= 32 * 1024 * 1024,
                        "Trace excedeu limite de 32 MiB"
                    );
                    Ok((data.into(), value["eof"] == true))
                })();
                let finished = match chunk {
                    Ok((data, eof)) => {
                        use std::io::Write;
                        match file.borrow_mut().write_all(data.as_bytes()) {
                            Ok(()) => {
                                used.set(used.get() + data.len());
                                if eof {
                                    trace_report(&inner, "complete", "", used.get());
                                }
                                eof
                            }
                            Err(error) => {
                                trace_report(&inner, "error", &error.to_string(), used.get());
                                true
                            }
                        }
                    }
                    Err(error) => {
                        trace_report(&inner, "error", &error.to_string(), used.get());
                        true
                    }
                };
                if finished {
                    let parameters = serde_json::json!({"handle":stream}).to_string();
                    let _ = close_view.CallDevToolsProtocolMethod(
                        w!("IO.close"),
                        *CoTaskMemPWSTR::from(parameters.as_str())
                            .as_ref()
                            .as_pcwstr(),
                        &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                            |_, _| Ok(()),
                        )),
                    );
                } else {
                    read_trace_chunk(weak.clone(), stream.clone(), file.clone(), used.clone());
                }
                Ok(())
            })),
        )
    };
    if let Err(error) = result {
        trace_report(&inner, "error", &error.to_string(), used_at_call);
    }
}
fn headers_string(headers: &reqwest::header::HeaderMap) -> String {
    let mut text = String::new();
    for (name, value) in headers {
        if !["connection", "transfer-encoding", "keep-alive"].contains(&name.as_str())
            && let Ok(value) = value.to_str()
        {
            text.push_str(name.as_str());
            text.push_str(": ");
            text.push_str(value);
            text.push_str("\r\n");
        }
    }
    text
}
