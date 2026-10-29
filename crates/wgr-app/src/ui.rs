use crate::{
    input::TextInput,
    player::{GamePlayer, PlayerEvent, PlayerOptions},
    service::{ImportRequest, Launch, Preview, Services},
};
use anyhow::{Context as _, Result, ensure};
use gpui::{prelude::*, *};
use momor_ui::{
    Button, ButtonCommon, ButtonSize, ButtonStyle, Clickable, Color as MomorColor, Label,
    LabelCommon, TintColor, Toggleable, Tooltip,
};
use raw_window_handle::HasWindowHandle;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use uuid::Uuid;
use wgr_core::{
    credentials::{CredentialStore, Secret, WindowsVault},
    domain::*,
    security::{NetworkPolicy, rooted_file, validate_url},
    storage::Library,
};

mod theme {
    pub const BG: u32 = 0x3b414d;
    pub const SIDE: u32 = 0x2f343e;
    pub const PANEL: u32 = 0x2e343e;
    pub const BORDER: u32 = 0x464b57;
    pub const TEXT: u32 = 0xdce0e5;
    pub const MUTED: u32 = 0xa9afbc;
    pub const ACCENT: u32 = 0x74ade8;
    pub const DANGER: u32 = 0xf48771;
    pub const HOVER: u32 = 0x363c46;
}
use theme::*;

trait ButtonAppearance: Sized {
    fn border_0(self) -> Self;
    fn border_color(self, color: gpui::Rgba) -> Self;
    fn text_color(self, color: gpui::Rgba) -> Self;
    fn bg(self, color: gpui::Rgba) -> Self;
}
impl ButtonAppearance for Button {
    fn border_0(self) -> Self {
        self.style(ButtonStyle::Subtle)
    }
    fn border_color(self, color: gpui::Rgba) -> Self {
        self.style(ButtonStyle::OutlinedCustom(color.into()))
    }
    fn text_color(self, color: gpui::Rgba) -> Self {
        self.color(MomorColor::Custom(color.into()))
    }
    fn bg(self, _color: gpui::Rgba) -> Self {
        self.style(ButtonStyle::Filled)
    }
}

#[derive(Clone)]
enum Action {
    Filter(String),
    SortBy(String),
    Add,
    Kind(SourceKind),
    Analyze,
    SaveImport,
    Cancel,
    Details(Uuid),
    Play(Uuid),
    Back,
    Favorite(Uuid),
    GtaGraphics(Uuid, &'static str),
    Edit(Uuid),
    SaveEdit,
    Verify(Uuid),
    Remove(Uuid),
    ConfirmRemove(Uuid),
    Reorder(Uuid, i32),
    Sources,
    Sync(Uuid),
    EditSource(Uuid),
    RemoveSource(Uuid),
    ConfirmRemoveSource(Uuid),
    ReorderSource(Uuid, i32),
    FavoriteSource(Uuid),
    Settings,
    Diagnostics,
    ClearCache,
    CacheLimit,
    ToggleFullscreen,
    ToggleMute,
    Export,
    Reveal,
    PickFolder,
    PickZip,
    PickCover,
    Dev,
    Network,
    Auth,
}
enum Message {
    Stats(Uuid, serde_json::Value),
    Job(u64, Box<Message>),
    Reload(Result<String>),
    Preview(Result<Box<Preview>>),
    Launch(Result<Box<Launch>>),
    Report(Result<String>),
    Cover(Uuid, Result<Option<String>>),
}
#[derive(PartialEq)]
enum View {
    Library,
    Sources,
    Settings,
    Diagnostics,
    Detail(Uuid),
}
enum Modal {
    Wizard,
    Edit(Uuid),
    Remove(Uuid),
    RemoveSource(Uuid),
}
struct SessionUi {
    caption: crate::caption::Caption,
    stats_poll: Instant,
    launch: Launch,
    player: GamePlayer,
    capabilities: Option<serde_json::Value>,
    frames: bool,
    input: bool,
    fps: Option<f64>,
    muted: bool,
    loaded: bool,
}

pub struct Shell {
    pub services: Arc<Services>,
    runtime: tokio::runtime::Handle,
    games: Vec<Game>,
    sources: Vec<Source>,
    settings: Settings,
    view: View,
    modal: Option<Modal>,
    kind: SourceKind,
    preview: Option<Preview>,
    editing_source: Option<Uuid>,
    search: Entity<TextInput>,
    location: Entity<TextInput>,
    title: Entity<TextInput>,
    description: Entity<TextInput>,
    secret: Entity<TextInput>,
    extra: Entity<TextInput>,
    header: Entity<TextInput>,
    input_widgets: std::collections::HashMap<EntityId, Entity<momor_input::InputField>>,
    covers_requested: std::collections::HashSet<Uuid>,
    covers_loading: usize,
    filter: String,
    query: String,
    focus: FocusHandle,
    busy: bool,
    operation_id: u64,
    active_job: Option<tokio::task::JoinHandle<()>>,
    polling: bool,
    notice: String,
    error: bool,
    tx: mpsc::Sender<Message>,
    rx: mpsc::Receiver<Message>,
    session: Option<SessionUi>,
    fullscreen: bool,
    development: bool,
    network: bool,
    auth: u8,
    diagnostic: serde_json::Value,
    subscriptions: Vec<Subscription>,
    smoke: Option<PathBuf>,
    gta_smoke: bool,
    smoke_started: Instant,
    smoke_stage: u8,
    smoke_report: serde_json::Value,
    page: usize,
}
impl Shell {
    pub fn new(
        services: Arc<Services>,
        runtime: tokio::runtime::Handle,
        smoke: Option<PathBuf>,
        gta_smoke: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let search = cx.new(|cx| TextInput::new("Buscar jogos…  Ctrl+K", cx));
        let location = cx.new(|cx| TextInput::new("https://… ou caminho da pasta/ZIP", cx));
        let title = cx.new(|cx| TextInput::new("Nome do jogo ou da fonte", cx));
        let description = cx.new(|cx| TextInput::new("Descrição (opcional)", cx));
        let secret = cx.new(|cx| {
            let mut field = TextInput::new("Segredo · protegido pelo Windows", cx);
            field.secret = true;
            field
        });
        let extra = cx.new(|cx| TextInput::new("Origens HTTPS extras, separadas por vírgula", cx));
        let header = cx.new(|cx| TextInput::new("Nome do header da API key", cx));
        let input_widgets = [
            &search,
            &location,
            &title,
            &description,
            &secret,
            &extra,
            &header,
        ]
        .into_iter()
        .map(|input| {
            let placeholder = input.read(cx).placeholder();
            let masked = input.read(cx).secret;
            let editor: Arc<dyn momor_input::ErasedEditor> =
                Arc::new(crate::input::EditorBridge(input.clone()));
            (
                input.entity_id(),
                cx.new(|_| {
                    let field =
                        momor_input::InputField::from_editor(editor, &placeholder).tab_index(0);
                    if masked { field.masked(true) } else { field }
                }),
            )
        })
        .collect();
        let mut shell = Self {
            services,
            runtime,
            games: vec![],
            sources: vec![],
            settings: Settings::default(),
            view: View::Library,
            modal: None,
            kind: SourceKind::RemoteWeb,
            preview: None,
            editing_source: None,
            search: search.clone(),
            location,
            title,
            description,
            secret,
            extra,
            header,
            input_widgets,
            covers_requested: Default::default(),
            covers_loading: 0,
            filter: "Todos".into(),
            query: String::new(),
            focus: cx.focus_handle(),
            busy: false,
            operation_id: 0,
            active_job: None,
            polling: false,
            notice: String::new(),
            error: false,
            tx,
            rx,
            session: None,
            fullscreen: false,
            development: false,
            network: false,
            auth: 0,
            diagnostic: serde_json::json!({}),
            subscriptions: vec![],
            smoke,
            gta_smoke,
            smoke_started: Instant::now(),
            smoke_stage: 0,
            smoke_report: serde_json::json!({}),
            page: 0,
        };
        if let Err(e) = shell.reload() {
            shell.notice = format!("Falha na biblioteca: {e:#}");
            shell.error = true;
        }
        shell.subscriptions.push(cx.observe(&search, |this, _, cx| {
            let query = this.search.read(cx).content.to_lowercase();
            cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(120)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.search.read(cx).content.to_lowercase() == query {
                        this.query = query;
                        this.page = 0;
                        cx.notify();
                    }
                });
            })
            .detach();
        }));
        shell
            .subscriptions
            .push(cx.observe_window_bounds(window, |this, window, _| this.resize_player(window)));
        if shell.smoke.is_some() {
            shell.fullscreen = false;
            cx.spawn_in(window, async move |view, cx| {
                Timer::after(Duration::from_millis(600)).await;
                let target = view
                    .update_in(cx, |this, window, _| {
                        if let Some(path) = &this.smoke {
                            let library = path.with_file_name(format!(
                                "{}-library.png",
                                path.file_stem().unwrap_or_default().to_string_lossy()
                            ));
                            if let Ok(raw) = HasWindowHandle::window_handle(window)
                                && let raw_window_handle::RawWindowHandle::Win32(handle) =
                                    raw.as_raw()
                            {
                                return Some((this.runtime.clone(), handle.hwnd.get(), library));
                            }
                        }
                        None
                    })
                    .ok()
                    .flatten();
                let captured = if let Some((runtime, handle, path)) = target {
                    runtime
                        .spawn_blocking(move || {
                            crate::screenshot::capture_owned_handle(handle, &path)
                        })
                        .await
                        .is_ok_and(|r| r.is_ok())
                } else {
                    false
                };
                let _ = view.update_in(cx, |this, window, cx| {
                    this.smoke_report["library_capture"] = serde_json::json!(captured);
                    if let Some(game) = this.games.iter().find(|g| {
                        g.external_id.as_deref()
                            == Some(if this.gta_smoke {
                                "gta-legacy"
                            } else {
                                "wgr-orbit"
                            })
                    }) {
                        let id = game.id;
                        this.act(Action::Play(id), window, cx);
                    }
                });
            })
            .detach();
        }
        shell
    }
    fn reload(&mut self) -> Result<()> {
        let db = Library::open(&self.services.paths.database)?;
        self.games = db.games()?;
        self.sources = db.sources()?;
        self.settings = db.settings()?;
        self.filter = self.settings.filter.clone();
        self.diagnostic = db
            .value("last_diagnostic")?
            .unwrap_or_else(|| serde_json::json!({}));
        Ok(())
    }
    fn message(&mut self, result: Result<String>) {
        match result {
            Ok(text) => {
                self.notice = text;
                self.error = false
            }
            Err(e) => {
                self.notice = format!("{e:#}");
                self.error = true
            }
        }
    }
    fn job(
        &mut self,
        future: impl std::future::Future<Output = Message> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = false;
        self.notice = "Processando…".into();
        let tx = self.tx.clone();
        self.operation_id += 1;
        let operation_id = self.operation_id;
        self.active_job = Some(self.runtime.spawn(async move {
            let _ = tx.send(Message::Job(operation_id, Box::new(future.await)));
        }));
        self.poll(window, cx);
        cx.notify();
    }
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn_in(window, async move |view, cx| {
            loop {
                Timer::after(Duration::from_millis(80)).await;
                let keep = view
                    .update_in(cx, |this, window, cx| this.tick(window, cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        while let Ok(message) = self.rx.try_recv() {
            let message = if let Message::Job(operation_id, result) = message {
                if operation_id != self.operation_id {
                    continue;
                }
                self.busy = false;
                self.active_job = None;
                *result
            } else {
                message
            };
            match message {
                Message::Stats(id, stats) => {
                    if let Some(session) = self.session.as_mut()
                        && session.launch.session.id == id
                    {
                        let previous = self.diagnostic["asset_server"]["archive_bytes"]
                            .as_u64()
                            .unwrap_or(0);
                        let current = stats["archive_bytes"].as_u64().unwrap_or(0);
                        if current > previous
                            && session.launch.source.kind == SourceKind::GtaLegacyZip
                        {
                            let line = format!(
                                "Recebidos da origem: {:.1} MiB; preparando dados do jogo",
                                current as f64 / 1048576.
                            );
                            let payload = serde_json::json!({"lines":[line]});
                            let _=session.player.script(&format!("{{const channel=new BroadcastChannel('game-progress');channel.postMessage({payload});channel.close();}}"));
                        }
                        self.diagnostic["asset_server"] = stats;
                    }
                }
                Message::Job(_, _) => {}
                Message::Reload(result) => {
                    self.message(result);
                    if let Err(e) = self.reload() {
                        self.message(Err(e));
                    }
                }
                Message::Report(result) => self.message(result),
                Message::Cover(id, result) => {
                    self.covers_loading = self.covers_loading.saturating_sub(1);
                    match result {
                        Ok(Some(path)) => {
                            if let Some(game) = self.games.iter_mut().find(|g| g.id == id) {
                                game.cover_uri = Some(path);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(game_id=%id,error=%error,"cover_unavailable");
                        }
                    }
                }
                Message::Preview(result) => match result {
                    Ok(preview) => {
                        if preview.games.len() == 1 {
                            self.title
                                .update(cx, |f, cx| f.set(preview.games[0].title.clone(), cx));
                        }
                        self.notice = preview.report.clone();
                        self.preview = Some(*preview);
                    }
                    Err(e) => self.message(Err(e)),
                },
                Message::Launch(result) => match result {
                    Ok(launch) => {
                        if let Err(e) = self.open_player(*launch, window, cx) {
                            self.message(Err(e));
                        }
                    }
                    Err(e) => self.message(Err(e)),
                },
            }
            cx.notify();
        }
        let events = self
            .session
            .as_ref()
            .map(|s| s.player.poll())
            .unwrap_or_default();
        for event in events {
            match event {
                PlayerEvent::PageCapabilities(capabilities) => {
                    self.diagnostic["capabilities"] = capabilities.clone();
                    self.diagnostic["probe_context"] = serde_json::json!("página do jogo");
                    if let Some(session) = self.session.as_mut() {
                        session.capabilities = Some(capabilities.clone());
                        let missing = if std::env::args().any(|arg| arg == "--clear-profile-cache")
                        {
                            vec![]
                        } else {
                            missing_capabilities(&session.launch.game.requirements, &capabilities)
                        };
                        if !missing.is_empty() {
                            session.player.hide();
                            let _ = session
                                .launch
                                .session
                                .state
                                .transition(SessionState::Failure);
                            self.error = true;
                            self.notice = format!(
                                "A página do jogo não oferece os requisitos: {}",
                                missing.join(", ")
                            );
                        }
                    }
                    cx.notify();
                }
                PlayerEvent::Ready => {
                    self.resize_player(window);
                }
                PlayerEvent::Capabilities(capabilities) => {
                    if let Some(session) = self.session.as_mut() {
                        session.capabilities = Some(capabilities.clone());
                        self.diagnostic = serde_json::json!({"game_id":session.launch.game.id,"session_id":session.launch.session.id,"runtime":session.player.runtime_version(),"capabilities":capabilities,"timestamp":now(),"page_ready_ms":session.player.elapsed_ms(),"first_frame_ms":null,"fps":null});
                        let missing =
                            missing_capabilities(&session.launch.game.requirements, &capabilities);
                        if missing.is_empty() {
                            let _ = session
                                .launch
                                .session
                                .state
                                .transition(SessionState::Preparing);
                            let _ = session
                                .launch
                                .session
                                .state
                                .transition(SessionState::Loading);
                            if let Err(e) = session.player.load_game() {
                                self.message(Err(e));
                            }
                        } else {
                            self.notice =
                                format!("Incompatível com runtime: {}", missing.join(", "));
                            self.error = true;
                            session.player.hide();
                            let _ = session
                                .launch
                                .session
                                .state
                                .transition(SessionState::Failure);
                        }
                        if let Ok(db) = Library::open(&self.services.paths.database) {
                            let _ = db.set_value("last_diagnostic", &self.diagnostic);
                        }
                    }
                    cx.notify();
                }
                PlayerEvent::Loaded => {
                    if let Some(session) = self.session.as_mut() {
                        session.loaded = true;
                        session.player.focus();
                        self.notice = if session.player.is_native() {
                            "Binário carregado · aguardando primeiro quadro nativo.".into()
                        } else {
                            "Página carregada · aguardando sinal de execução do jogo. Ctrl+Shift+B volta à biblioteca; F11 alterna tela cheia.".into()
                        };
                    }
                    cx.notify();
                }
                PlayerEvent::Report(value) => {
                    if let Some(session) = self.session.as_mut() {
                        match value["type"].as_str().unwrap_or("") {
                            "first_frame" => {
                                session.frames = true;
                                let _ = session
                                    .launch
                                    .session
                                    .state
                                    .transition(SessionState::Running);
                                self.diagnostic["first_frame_ms"] =
                                    serde_json::json!(session.player.elapsed_ms());
                                self.smoke_report["first_frame"] = value.clone();
                                self.notice =
                                    "Em execução · Ctrl+Shift+B para voltar · F11 tela cheia"
                                        .into();
                            }
                            "fixture_input" => {
                                session.input = true;
                                self.smoke_report["input"] = value.clone();
                            }
                            "fixture_fps" | "gta_fps" => {
                                session.fps = value["fps"].as_f64();
                                self.diagnostic["fps"] = value["fps"].clone();
                            }
                            "fixture_audio" => self.smoke_report["audio"] = value.clone(),
                            "native_input_probe" => {
                                self.smoke_report["native_input_probe"] = value.clone()
                            }
                            "smoke_target" if self.smoke.is_some() => {
                                let scale = window.scale_factor();
                                if let (Some(x), Some(y)) =
                                    (value["x"].as_f64(), value["y"].as_f64())
                                    && let Err(error) = session.player.smoke_click(
                                        (x * scale as f64) as i32,
                                        (y * scale as f64) as i32,
                                    )
                                {
                                    self.smoke_report["audio_input_error"] =
                                        serde_json::json!(error.to_string());
                                }
                            }
                            "fixture_webgpu" => {
                                self.smoke_report["webgpu"] = value.clone();
                                if self.smoke.is_some() {
                                    let _=session.player.script("{const r=document.querySelector('#audio').getBoundingClientRect();chrome.player.postMessage(JSON.stringify({type:'smoke_target',x:r.x+r.width/2,y:r.y+r.height/2}));}");
                                }
                            }
                            "fixture_error" | "game-crash" => {
                                self.notice = format!(
                                    "Engine interrompida: {}",
                                    value
                                        .get("error")
                                        .or_else(|| value.get("payload"))
                                        .unwrap_or(&serde_json::Value::Null)
                                );
                                self.error = true;
                                let _ = session
                                    .launch
                                    .session
                                    .state
                                    .transition(SessionState::Crashed);
                            }
                            "profile_native_cache_cleared" => {
                                self.smoke_report["native_cache_cleared"] = value["ok"].clone();
                            }
                            "profile_cache_cleared" => {
                                self.smoke_report["profile_cache_cleared"] = value["ok"].clone();
                            }
                            "game-fps" => {
                                session.fps = value["payload"]["fps"].as_f64();
                                if session.fps.is_some_and(|fps| fps > 0.) {
                                    if !session.frames {
                                        self.diagnostic["first_frame_ms"] =
                                            serde_json::json!(session.player.elapsed_ms());
                                        let _ = session
                                            .launch
                                            .session
                                            .state
                                            .transition(SessionState::Running);
                                    }
                                    session.frames = true;
                                    self.smoke_report["port_frame_report"] = value.clone();
                                    if self.gta_smoke
                                        && self.smoke_report["world_at_ms"].is_number()
                                    {
                                        let samples = self
                                            .smoke_report
                                            .as_object_mut()
                                            .unwrap()
                                            .entry("fps_samples")
                                            .or_insert_with(|| serde_json::json!([]));
                                        if let Some(samples) = samples.as_array_mut() {
                                            if samples.len() == 240 {
                                                samples.remove(0);
                                            }
                                            samples.push(serde_json::json!({"at_ms":session.player.elapsed_ms(),"fps":session.fps}));
                                        }
                                    }
                                }
                            }
                            "game-progress" => {
                                self.notice = value["payload"]["label"]
                                    .as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| format!("Port: {}", value["payload"]));
                                if let Some(line) = value["payload"]["gpu_log"].as_str() {
                                    self.diagnostic["gpu_last"] = serde_json::json!(line);
                                    if self.gta_smoke {
                                        let logs = self
                                            .smoke_report
                                            .as_object_mut()
                                            .unwrap()
                                            .entry("gpu_logs")
                                            .or_insert_with(|| serde_json::json!([]));
                                        if let Some(logs) = logs.as_array_mut() {
                                            if logs.len() == 96 {
                                                logs.remove(0);
                                            }
                                            logs.push(serde_json::json!(line));
                                        }
                                    }
                                }
                                for key in ["configuration", "memory", "gpu_memory", "gpu_window"] {
                                    if value["payload"][key].is_object() {
                                        self.diagnostic[key] = value["payload"][key].clone();
                                    }
                                }
                                if self.gta_smoke {
                                    let mut payload = serde_json::Map::new();
                                    for key in [
                                        "gpu_frame",
                                        "gpu_async",
                                        "gpu_memory",
                                        "gpu_window",
                                        "cache_stall",
                                        "io_stall",
                                        "io_window",
                                        "cache",
                                        "configuration",
                                        "memory",
                                    ] {
                                        if value["payload"][key].is_object() {
                                            payload
                                                .insert(key.into(), value["payload"][key].clone());
                                        }
                                    }
                                    if !payload.is_empty()
                                        && serde_json::to_vec(&payload)
                                            .is_ok_and(|bytes| bytes.len() <= 8192)
                                    {
                                        let seq =
                                            self.smoke_report["trace_seq"].as_u64().unwrap_or(0)
                                                + 1;
                                        self.smoke_report["trace_seq"] = serde_json::json!(seq);
                                        let timeline = self
                                            .smoke_report
                                            .as_object_mut()
                                            .unwrap()
                                            .entry("timeline")
                                            .or_insert_with(|| serde_json::json!([]))
                                            .as_array_mut()
                                            .unwrap();
                                        if timeline.len() == 512 {
                                            timeline.remove(0);
                                        }
                                        timeline.push(serde_json::json!({"seq":seq,"host_elapsed_ms":session.player.elapsed_ms(),"payload":payload}));
                                    }
                                    self.smoke_report["progress"] = value.clone();
                                    if value["payload"]["cache"].is_object() {
                                        self.smoke_report["cache"] =
                                            value["payload"]["cache"].clone();
                                    }
                                    if value["payload"]["world"] == true {
                                        self.smoke_report["world_report"] = value.clone();
                                        if self.smoke_report["world_at_ms"].is_null() {
                                            self.smoke_report["world_at_ms"] =
                                                serde_json::json!(session.player.elapsed_ms());
                                        }
                                    }
                                }
                            }
                            "trace_state" => self.smoke_report["trace"] = value["payload"].clone(),
                            "browser_log" => {
                                let logs = self
                                    .smoke_report
                                    .as_object_mut()
                                    .unwrap()
                                    .entry("browser_logs")
                                    .or_insert_with(|| serde_json::json!([]));
                                if let Some(logs) = logs.as_array_mut() {
                                    if logs.len() == 40 {
                                        logs.remove(0);
                                    }
                                    logs.push(value["payload"].clone());
                                }
                            }
                            "port_state" | "page_error" => {
                                self.diagnostic["port_state"] = value.clone();
                                self.smoke_report["port_state"] = value.clone();
                                if value["type"] == "page_error" {
                                    self.notice =
                                        format!("Erro na página do jogo: {}", value["message"]);
                                }
                            }
                            _ => {}
                        }
                    }
                    // Metrics update the native caption independently. The
                    // GPUI content behind the child HWND stays unchanged.
                    if self.error {
                        cx.notify();
                    }
                }
                PlayerEvent::Failure(message) => {
                    self.notice = message;
                    self.error = true;
                    if let Some(session) = self.session.as_mut() {
                        session.player.hide();
                        let _ = session
                            .launch
                            .session
                            .state
                            .transition(SessionState::Failure);
                    }
                    if self.fullscreen {
                        self.fullscreen = false;
                        window.toggle_fullscreen();
                    }
                    cx.notify();
                }
                PlayerEvent::Back => self.stop_player(window, cx),
                PlayerEvent::Fullscreen => self.act(Action::ToggleFullscreen, window, cx),
            }
        }
        if let Some(session) = self.session.as_mut()
            && session.stats_poll.elapsed().as_secs() >= 2
            && let Some(server) = &session.launch.server
        {
            session.stats_poll = Instant::now();
            let url = format!("{}/__status", server.origin);
            let id = session.launch.session.id;
            let tx = self.tx.clone();
            self.runtime.spawn(async move {
                let result = async {
                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(Duration::from_secs(3))
                        .build()?;
                    let response = client.get(url).send().await?;
                    let bytes = response.bytes().await?;
                    anyhow::ensure!(bytes.len() < 256 * 1024, "Status inválido");
                    Ok::<serde_json::Value, anyhow::Error>(serde_json::from_slice(&bytes)?)
                }
                .await;
                if let Ok(stats) = result {
                    let _ = tx.send(Message::Stats(id, stats));
                }
            });
        }
        let caption_actions = self
            .session
            .as_ref()
            .map(|session| session.caption.actions())
            .unwrap_or_default();
        for action in caption_actions {
            self.act(
                match action {
                    crate::caption::CaptionAction::Back => Action::Back,
                    crate::caption::CaptionAction::Mute => Action::ToggleMute,
                    crate::caption::CaptionAction::Fullscreen => Action::ToggleFullscreen,
                },
                window,
                cx,
            );
        }
        if let Some(session) = &self.session {
            let title: String = session.launch.game.title.chars().take(38).collect();
            let bytes = self.diagnostic["asset_server"]["archive_bytes"]
                .as_u64()
                .unwrap_or(0);
            let status = session
                .fps
                .map(|fps| format!("{fps:.0} FPS"))
                .unwrap_or_else(|| {
                    if bytes > 0 {
                        format!("{:.0} MiB", bytes as f64 / 1048576.)
                    } else {
                        "Carregando".into()
                    }
                });
            session
                .caption
                .update(&format!("WGR — {title} · {status}"), session.muted);
        }
        if self.smoke.is_some() {
            self.smoke_tick(window, cx);
        }
        let keep = self.busy || self.session.is_some() || self.covers_loading > 0;
        if !keep {
            self.polling = false;
        }
        keep
    }
    fn open_player(
        &mut self,
        launch: Launch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let handle = window
            .window_handle()
            .map_err(|e| anyhow::anyhow!("Janela nativa indisponível: {e:?}"))?
            .as_raw();
        let raw_window_handle::RawWindowHandle::Win32(handle) = handle else {
            anyhow::bail!("WGR requer janela Windows")
        };
        let parent = windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _);
        let policy = NetworkPolicy {
            allowed_origins: launch.source.allowed_origins.clone(),
            development_loopback: launch.source.development_loopback,
        };
        let mut options = PlayerOptions {
            native_root: self.services.paths.root.join("native-rage"),
            binary_path: launch.binary.clone(),
            entry: launch.entry.clone(),
            origin: launch.origin.clone(),
            loopback: launch.server.as_ref().map(|s| s.origin.clone()),
            policy,
            profile: self
                .services
                .paths
                .profiles
                .join(launch.game.id.to_string()),
            spool: self
                .services
                .paths
                .cache
                .join("sessions")
                .join(launch.session.id.to_string()),
            fixture: launch.fixture,
            gta: launch.source.kind == SourceKind::GtaLegacyZip
                || launch.game.external_id.as_deref() == Some("gta-legacy"),
            diagnostic: std::env::args().any(|arg| arg == "--gta-logs"),
            cleanup_cache: std::env::args().any(|arg| arg == "--clear-profile-cache"),
            muted: launch.game.profile.muted,
            runtime: self.runtime.clone(),
            cache: self.services.cache.clone(),
        };
        if options.cleanup_cache {
            options.entry = format!("{}/__wgr_cleanup", options.origin);
        } else if options.gta && self.gta_smoke {
            let mut url = url::Url::parse(&options.entry)?;
            let has_mode = url.query_pairs().any(|(key, _)| key == "mode");
            if !has_mode {
                url.query_pairs_mut().append_pair("mode", "sandbox");
            }
            url.query_pairs_mut().append_pair("res", "1280x720");
            let args: Vec<String> = std::env::args().collect();
            if let Some(fps) = args
                .windows(2)
                .find(|pair| pair[0] == "--gta-fps")
                .map(|pair| pair[1].as_str())
                .filter(|fps| ["0", "30", "60"].contains(fps))
            {
                let pairs: Vec<_> = url
                    .query_pairs()
                    .filter(|(key, _)| key != "fps")
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect();
                url.set_query(None);
                url.query_pairs_mut()
                    .extend_pairs(pairs)
                    .append_pair("fps", fps);
            }
            if args
                .windows(2)
                .any(|pair| pair[0] == "--gta-map" && pair[1] == "env_test")
            {
                url.query_pairs_mut().append_pair("map", "env_test");
            }
            options.entry = url.to_string();
            self.smoke_report["test_entry"] = serde_json::json!(options.entry);
        }
        let player = GamePlayer::create(parent, options)?;
        if player.is_native() {
            window.focus(&self.focus);
        }
        let caption = crate::caption::Caption::new(parent)?;
        let muted = launch.game.profile.muted;
        self.session = Some(SessionUi {
            caption,
            stats_poll: Instant::now(),
            launch,
            player,
            capabilities: None,
            frames: false,
            input: false,
            fps: None,
            muted,
            loaded: false,
        });
        if self.smoke.is_none() && self.settings.fullscreen {
            self.fullscreen = true;
            window.toggle_fullscreen();
        }
        self.modal = None;
        self.resize_player(window);
        self.poll(window, cx);
        Ok(())
    }
    fn resize_player(&self, window: &Window) {
        if let Some(session) = &self.session {
            if self.error {
                session.player.hide();
                return;
            }
            let scale = window.scale_factor();
            let viewport = window.viewport_size();
            let header = 0.;
            let _ = session.player.resize(
                0,
                (header * scale) as i32,
                (f32::from(viewport.width) * scale) as i32,
                ((f32::from(viewport.height) - header) * scale) as i32,
            );
        }
    }
    fn stop_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut session) = self.session.take() {
            let _=session.player.script("document.exitPointerLock?.();window.dispatchEvent(new Event('pagehide'));for(const name of ['game-io']){try{new BroadcastChannel(name).postMessage('close')}catch{}}");
            session.launch.session.end_at = Some(now());
            session.launch.session.metrics = self.diagnostic.clone();
            if !matches!(
                session.launch.session.state,
                SessionState::Failure | SessionState::Crashed
            ) {
                let _ = session
                    .launch
                    .session
                    .state
                    .transition(SessionState::Stopping);
                let _ = session
                    .launch
                    .session
                    .state
                    .transition(SessionState::Stopped);
            }
            let paths = self.services.paths.clone();
            let cache = self.services.cache.clone();
            let record = session.launch.session.clone();
            let spool = paths.cache.join("sessions").join(record.id.to_string());
            drop(session.player);
            self.runtime.spawn(async move {
                if let Ok(mut db) = Library::open(&paths.database) {
                    let _ = db.save_session(&record);
                    let _ = db.set_value("last_diagnostic", &record.metrics);
                }
                if let Some(server) = session.launch.server {
                    server.shutdown().await;
                }
                cache.unpin_all();
                if spool.starts_with(paths.cache.join("sessions")) {
                    let base = paths.cache.join("sessions");
                    let _ = tokio::task::spawn_blocking(move || {
                        wgr_core::security::remove_owned_directory(&base, &spool)
                    })
                    .await;
                }
            });
        }
        if self.fullscreen {
            self.fullscreen = false;
            window.toggle_fullscreen();
        }
        self.notice = "Sessão encerrada. Saves preservados no perfil do jogo.".into();
        self.view = View::Library;
        let _ = self.reload();
        window.focus(&self.focus);
        cx.notify();
    }
    fn act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy && matches!(action, Action::Cancel | Action::Back) {
            if let Some(job) = self.active_job.take() {
                job.abort();
            }
            self.operation_id += 1;
            self.busy = false;
        }
        if self.busy && !matches!(action, Action::Back | Action::Cancel) {
            return;
        }
        let services = self.services.clone();
        let database = services.paths.database.clone();
        match action {
            Action::Filter(filter) => {
                self.filter = filter;
                self.settings.filter = self.filter.clone();
                self.page = 0;
                self.view = View::Library;
                self.persist_settings();
            }
            Action::SortBy(sort) => {
                self.settings.sort = sort;
                self.persist_settings();
            }
            Action::Add => {
                self.reset_form(cx);
                self.modal = Some(Modal::Wizard);
                window.focus(&self.location.read(cx).focus_handle);
            }
            Action::Kind(kind) => {
                self.kind = kind;
                self.preview = None;
            }
            Action::Dev => {
                self.development = !self.development;
                self.preview = None;
            }
            Action::Network => {
                self.network = !self.network;
                self.preview = None;
            }
            Action::Auth => {
                self.auth = (self.auth + 1) % 4;
                self.preview = None;
            }
            Action::Analyze => {
                let location = self.location.read(cx).content.to_string();
                let title = self.title.read(cx).content.to_string();
                let raw = self.secret.read(cx).content.to_string();
                let mut secret = (!raw.is_empty()).then_some(Secret(raw));
                if secret.is_none()
                    && let Some(id) = self.editing_source
                    && let Some(source) = self.sources.iter().find(|s| s.id == id)
                    && let Some(key) = &source.credential_ref
                {
                    secret = WindowsVault.get(key).ok().flatten();
                }
                let auth = match self.auth {
                    1 => AuthScheme::Bearer,
                    2 => AuthScheme::Header {
                        name: self.header.read(cx).content.to_string(),
                    },
                    3 => AuthScheme::Basic,
                    _ => AuthScheme::None,
                };
                let allowed_origins = self
                    .extra
                    .read(cx)
                    .content
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                let request = ImportRequest {
                    kind: self.kind,
                    location,
                    title,
                    auth,
                    secret,
                    development: self.development,
                    network: self.network,
                    allowed_origins,
                };
                self.job(
                    async move { Message::Preview(services.preview(request).await.map(Box::new)) },
                    window,
                    cx,
                );
            }
            Action::SaveImport => {
                if let Some(mut preview) = self.preview.take() {
                    let title = self.title.read(cx).content.to_string();
                    if let Some(id) = self.editing_source
                        && let Some(old) = self.sources.iter().find(|s| s.id == id)
                    {
                        preview.source.id = id;
                        preview.source.credential_ref = old.credential_ref.clone();
                        preview.source.favorite = old.favorite;
                        for game in &mut preview.games {
                            game.source_id = id;
                            if let Some(previous) = self.games.iter().find(|g| {
                                g.source_id == id
                                    && (g.external_id == game.external_id
                                        || preview.source.kind != SourceKind::RemoteCatalogApi)
                            }) {
                                let new_entry = game.entry_uri.clone();
                                let new_bundle = game.local_bundle_path.clone();
                                let new_status = game.compat_status.clone();
                                *game = previous.clone();
                                game.entry_uri = new_entry;
                                game.local_bundle_path = new_bundle;
                                game.compat_status = new_status;
                            }
                        }
                    }
                    self.modal = None;
                    self.secret.update(cx, |f, _| f.reset());
                    self.job(
                        async move {
                            Message::Reload(
                                tokio::task::spawn_blocking(move || {
                                    services.commit_preview(preview, &title).map(|count| {
                                        format!("{count} jogo(s) salvo(s) na biblioteca.")
                                    })
                                })
                                .await
                                .map_err(anyhow::Error::from)
                                .and_then(|r| r),
                            )
                        },
                        window,
                        cx,
                    );
                }
            }
            Action::Cancel => {
                self.modal = None;
                self.preview = None;
                self.secret.update(cx, |f, _| f.reset());
            }
            Action::Details(id) => self.view = View::Detail(id),
            Action::Back => {
                if self.session.is_some() {
                    self.stop_player(window, cx)
                } else {
                    self.modal = None;
                    self.view = View::Library;
                }
            }
            Action::Play(id) => {
                if self.smoke.is_some() {
                    let expected = if self.gta_smoke {
                        "gta-legacy"
                    } else {
                        "wgr-orbit"
                    };
                    if !self
                        .games
                        .iter()
                        .any(|game| game.id == id && game.external_id.as_deref() == Some(expected))
                    {
                        return;
                    }
                }
                self.job(
                    async move { Message::Launch(services.launch(id).await.map(Box::new)) },
                    window,
                    cx,
                );
            }
            Action::Favorite(id) => self.db_job(
                move |db| {
                    let mut game = db.game(id)?.context("Jogo ausente")?;
                    game.favorite = !game.favorite;
                    db.save_game(&game)?;
                    Ok("Favorito atualizado.".into())
                },
                window,
                cx,
            ),
            Action::Edit(id) => {
                if let Some(game) = self.games.iter().find(|g| g.id == id) {
                    self.title.update(cx, |f, cx| f.set(game.title.clone(), cx));
                    self.location
                        .update(cx, |f, cx| f.set(game.entry_uri.clone(), cx));
                    self.description
                        .update(cx, |f, cx| f.set(game.description.clone(), cx));
                    self.extra.update(cx, |f, cx| {
                        f.set(game.cover_uri.clone().unwrap_or_default(), cx)
                    });
                    self.modal = Some(Modal::Edit(id));
                }
            }
            Action::SaveEdit => {
                if let Some(Modal::Edit(id)) = self.modal {
                    let title = self.title.read(cx).content.to_string();
                    let entry = self.location.read(cx).content.to_string();
                    let description = self.description.read(cx).content.to_string();
                    let cover = self.extra.read(cx).content.to_string();
                    self.modal = None;
                    self.db_job(
                        move |db| {
                            let mut game = db.game(id)?.context("Jogo ausente")?;
                            let source = db.source(game.source_id)?.context("Fonte ausente")?;
                            match source.kind {
                                SourceKind::RemoteWeb | SourceKind::RemoteCatalogApi => {
                                    let u = validate_url(&entry, source.development_loopback)?;
                                    ensure!(
                                        source
                                            .allowed_origins
                                            .contains(&u.origin().ascii_serialization()),
                                        "Origem não autorizada na fonte"
                                    );
                                }
                                SourceKind::LocalWebBundle => {
                                    rooted_file(
                                        game.local_bundle_path.as_ref().context("Pasta ausente")?,
                                        &entry,
                                    )?;
                                }
                                SourceKind::GtaLegacyZip => {
                                    wgr_core::security::safe_relative(&entry)?;
                                }
                            }
                            game.title = title;
                            game.entry_uri = entry;
                            game.description = description;
                            if !cover.is_empty() {
                                ensure!(
                                    PathBuf::from(&cover).is_file(),
                                    "Capa precisa ser um arquivo selecionado no computador"
                                );
                            }
                            game.cover_uri = (!cover.is_empty()).then_some(cover);
                            db.save_game(&game)?;
                            Ok("Metadados salvos.".into())
                        },
                        window,
                        cx,
                    );
                }
            }
            Action::Verify(id) => self.job(
                async move {
                    Message::Report(
                        tokio::task::spawn_blocking(move || services.verify(id))
                            .await
                            .map_err(anyhow::Error::from)
                            .and_then(|r| r),
                    )
                },
                window,
                cx,
            ),
            Action::Remove(id) => self.modal = Some(Modal::Remove(id)),
            Action::ConfirmRemove(id) => {
                self.modal = None;
                self.view = View::Library;
                self.db_job(
                    move |db| {
                        db.remove_game(id)?;
                        Ok("Jogo removido da biblioteca. Arquivos e saves preservados.".into())
                    },
                    window,
                    cx,
                );
            }
            Action::Reorder(id, direction) => {
                let mut ids: Vec<_> = self.games.iter().map(|g| g.id).collect();
                move_item(&mut ids, id, direction);
                self.db_job(
                    move |db| {
                        db.reorder(false, &ids)?;
                        Ok("Ordem salva.".into())
                    },
                    window,
                    cx,
                );
            }
            Action::Sources => self.view = View::Sources,
            Action::Sync(id) => {
                if let Some(source) = self.sources.iter().find(|s| s.id == id).cloned() {
                    self.job(async move{let result=async{let secret=source.credential_ref.as_ref().map(|key|WindowsVault.get(key)).transpose()?.flatten();let(source,games)=wgr_core::catalog::sync(&source,secret.as_ref()).await?;let mut db=Library::open(&database)?;let count=db.import(&source,&games)?;Ok(format!("{count} jogo(s) sincronizado(s). Favoritos e histórico preservados."))}.await;Message::Reload(result)},window,cx);
                }
            }
            Action::EditSource(id) => {
                if let Some(source) = self.sources.iter().find(|s| s.id == id).cloned() {
                    self.reset_form(cx);
                    self.kind = source.kind;
                    self.development = source.development_loopback;
                    self.auth = match source.auth {
                        AuthScheme::None => 0,
                        AuthScheme::Bearer => 1,
                        AuthScheme::Header { .. } => 2,
                        AuthScheme::Basic => 3,
                    };
                    self.editing_source = Some(id);
                    if let AuthScheme::Header { name } = &source.auth {
                        self.header.update(cx, |f, cx| f.set(name.clone(), cx));
                    }
                    self.title
                        .update(cx, |f, cx| f.set(source.display_name, cx));
                    let location = if source.kind == SourceKind::GtaLegacyZip {
                        self.games
                            .iter()
                            .find(|g| g.source_id == id)
                            .and_then(|g| g.local_bundle_path.as_ref())
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    } else {
                        source.location
                    };
                    self.location.update(cx, |f, cx| f.set(location, cx));
                    self.extra
                        .update(cx, |f, cx| f.set(source.allowed_origins.join(", "), cx));
                    self.modal = Some(Modal::Wizard);
                }
            }
            Action::RemoveSource(id) => self.modal = Some(Modal::RemoveSource(id)),
            Action::ConfirmRemoveSource(id) => {
                self.modal = None;
                self.db_job(
                    move |db| {
                        if let Some(source) = db.source(id)?
                            && let Some(key) = source.credential_ref
                        {
                            WindowsVault.delete(&key)?;
                        }
                        db.remove_source(id)?;
                        Ok("Fonte e registros vinculados removidos; arquivos preservados.".into())
                    },
                    window,
                    cx,
                );
            }
            Action::ReorderSource(id, d) => {
                let mut ids: Vec<_> = self.sources.iter().map(|s| s.id).collect();
                move_item(&mut ids, id, d);
                self.db_job(
                    move |db| {
                        db.reorder(true, &ids)?;
                        Ok("Ordem das fontes salva.".into())
                    },
                    window,
                    cx,
                );
            }
            Action::FavoriteSource(id) => self.db_job(
                move |db| {
                    let mut s = db.source(id)?.context("Fonte ausente")?;
                    s.favorite = !s.favorite;
                    db.save_source(&s)?;
                    Ok("Favorito da fonte atualizado.".into())
                },
                window,
                cx,
            ),
            Action::Settings => self.view = View::Settings,
            Action::Diagnostics => self.view = View::Diagnostics,
            Action::ClearCache => self.job(
                async move {
                    let result = tokio::task::spawn_blocking(move || {
                        services.cache.clear().map(|bytes| {
                            format!(
                                "{:.2} MiB de cache removidos. Saves preservados.",
                                bytes as f64 / 1048576.
                            )
                        })
                    })
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r);
                    Message::Reload(result)
                },
                window,
                cx,
            ),
            Action::CacheLimit => {
                self.settings.cache_limit_mib = match self.settings.cache_limit_mib {
                    64 => 128,
                    128 => 256,
                    _ => 64,
                };
                self.persist_settings();
                self.notice = "Limite salvo. Será aplicado ao reiniciar o WGR.".into();
            }
            Action::ToggleFullscreen => {
                if self.session.is_some() {
                    self.fullscreen = !self.fullscreen;
                    window.toggle_fullscreen();
                    self.resize_player(window);
                } else {
                    self.settings.fullscreen = !self.settings.fullscreen;
                    self.persist_settings();
                }
            }
            Action::ToggleMute => {
                if let Some(session) = self.session.as_mut() {
                    session.muted = !session.muted;
                    let result = session.player.mute(session.muted);
                    if let Err(e) = result {
                        self.message(Err(e));
                    }
                }
            }
            Action::Export => {
                let report = self.diagnostic.clone();
                let paths = self.services.paths.clone();
                self.job(
                    async move {
                        let result = (|| -> Result<String> {
                            let dir = paths.root.join("reports");
                            std::fs::create_dir_all(&dir)?;
                            let path = dir.join(format!("diagnostico-{}.json", now()));
                            std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
                            Ok(format!("Relatório salvo em {}", path.display()))
                        })();
                        Message::Report(result)
                    },
                    window,
                    cx,
                );
            }
            Action::GtaGraphics(id, profile) => {
                self.db_job(
                    move |db| {
                        let mut game = db.game(id)?.context("Jogo não encontrado")?;
                        game.entry_uri =
                            crate::service::gta_graphics_entry(&game.entry_uri, profile)?;
                        db.save_game(&game)?;
                        Ok("Perfil gráfico salvo para a próxima abertura.".into())
                    },
                    window,
                    cx,
                );
            }
            Action::Reveal => {
                let _ = std::process::Command::new("explorer.exe")
                    .arg(&self.services.paths.root)
                    .spawn();
            }
            Action::PickFolder | Action::PickZip | Action::PickCover => {
                let folder = matches!(action, Action::PickFolder);
                let cover = matches!(action, Action::PickCover);
                let receiver = cx.prompt_for_paths(PathPromptOptions {
                    files: !folder,
                    directories: folder,
                    multiple: false,
                    prompt: None,
                });
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = receiver.await
                        && let Some(path) = paths.first()
                    {
                        let value = path.to_string_lossy().into_owned();
                        let _ = this.update_in(cx, |this, _, cx| {
                            if cover {
                                this.extra.update(cx, |f, cx| f.set(value, cx));
                            } else {
                                this.location.update(cx, |f, cx| f.set(value, cx));
                            }
                            this.preview = None;
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
        }
        cx.notify();
    }
    fn persist_settings(&mut self) {
        let result = Library::open(&self.services.paths.database)
            .and_then(|db| db.save_settings(&self.settings));
        if let Err(e) = result {
            self.message(Err(e));
        }
    }
    fn db_job(
        &mut self,
        operation: impl FnOnce(&mut Library) -> Result<String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.services.paths.database.clone();
        self.job(
            async move {
                Message::Reload(
                    tokio::task::spawn_blocking(move || operation(&mut Library::open(&path)?))
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|r| r),
                )
            },
            window,
            cx,
        );
    }
    fn reset_form(&mut self, cx: &mut Context<Self>) {
        for field in [
            &self.location,
            &self.title,
            &self.description,
            &self.secret,
            &self.extra,
            &self.header,
        ] {
            field.update(cx, |f, _| f.reset());
        }
        self.preview = None;
        self.editing_source = None;
        self.kind = SourceKind::RemoteWeb;
        self.development = false;
        self.network = false;
        self.auth = 0;
        self.header
            .update(cx, |f, cx| f.set("X-API-Key".into(), cx));
        self.notice.clear();
    }
    fn button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Button {
        let label = label.into();
        Button::new(ElementId::Name(id.into()), label.clone())
            .style(ButtonStyle::Outlined)
            .size(ButtonSize::Medium)
            .tab_index(0_isize)
            .tooltip(Tooltip::text(label))
            .on_click(cx.listener(move |this, _, window, cx| this.act(action.clone(), window, cx)))
    }
    fn primary(&self, id: &str, label: &str, action: Action, cx: &mut Context<Self>) -> Button {
        self.button(id.to_string(), label.to_string(), action, cx)
            .style(ButtonStyle::Tinted(TintColor::Accent))
            .color(MomorColor::Custom(rgb(0x142536).into()))
    }
    fn field(&self, label: &str, entity: &Entity<TextInput>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(Label::new(label.to_string()).color(MomorColor::Muted)),
            )
            .child(
                div()
                    .w_full()
                    .children(self.input_widgets.get(&entity.entity_id()).cloned()),
            )
    }
    fn filtered(&self) -> Vec<Game> {
        let mut games: Vec<_> = self
            .games
            .iter()
            .filter(|g| g.title.to_lowercase().contains(&self.query))
            .filter(|g| match self.filter.as_str() {
                "Favoritos" => g.favorite,
                "Instalados" => g.local_bundle_path.is_some(),
                "Online" => g.local_bundle_path.is_none(),
                "Recentes" => g.last_played_at.is_some(),
                _ => true,
            })
            .cloned()
            .collect();
        match self.settings.sort.as_str() {
            "name" => games.sort_by_key(|g| g.title.to_lowercase()),
            "recent" => games.sort_by_key(|g| std::cmp::Reverse(g.last_played_at)),
            "added" => games.sort_by_key(|g| std::cmp::Reverse(g.created_at)),
            "favorite" => games.sort_by_key(|g| std::cmp::Reverse(g.favorite)),
            _ => {}
        }
        games
    }
    fn queue_cover(&mut self, game: &Game, window: &mut Window, cx: &mut Context<Self>) {
        let Some(uri) = &game.cover_uri else { return };
        if PathBuf::from(uri).starts_with(self.services.paths.root.join("covers"))
            && PathBuf::from(uri).is_file()
        {
            return;
        }
        if self.covers_loading >= 4 || !self.covers_requested.insert(game.id) {
            return;
        }
        let Some(source) = self
            .sources
            .iter()
            .find(|s| s.id == game.source_id)
            .cloned()
        else {
            return;
        };
        let game = game.clone();
        let id = game.id;
        let services = self.services.clone();
        let tx = self.tx.clone();
        self.covers_loading += 1;
        self.runtime.spawn(async move {
            let result = async {
                let path = services.thumbnail(game, source).await?;
                if let Some(path) = &path {
                    let db = Library::open(&services.paths.database)?;
                    if let Some(mut game) = db.game(id)? {
                        game.cover_uri = Some(path.clone());
                        db.save_game(&game)?;
                    }
                }
                Ok(path)
            }
            .await;
            let _ = tx.send(Message::Cover(id, result));
        });
        self.poll(window, cx);
    }
    fn card(&self, game: &Game, cx: &mut Context<Self>) -> AnyElement {
        let id = game.id;
        let source = self.sources.iter().find(|s| s.id == game.source_id);
        let local = game.local_bundle_path.is_some();
        let art =
            if let Some(cover) = game
                .cover_uri
                .as_ref()
                .filter(|p| PathBuf::from(p).is_file())
            {
                img(PathBuf::from(cover))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            } else {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .p_4()
                    .bg(rgb(if local { 0x23362f } else { 0x252d3b }))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(if local {
                        "PACOTE LOCAL"
                    } else {
                        "FONTE EXTERNA"
                    }))
                    .child(
                        div()
                            .text_3xl()
                            .text_color(rgb(if local { ACCENT } else { 0x9fb5dc }))
                            .child(if game.external_id.as_deref() == Some("wgr-orbit") {
                                "◌  ÓRBITA"
                            } else {
                                "WGR"
                            }),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(
                        if game.cover_uri.is_none() {
                            "Sem capa fornecida pela fonte"
                        } else {
                            "Capa remota · não armazenada"
                        },
                    ))
                    .into_any_element()
            };
        div()
            .flex_1()
            .min_w_0()
            .h(px(300.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .overflow_hidden()
            .child(div().h(px(138.)).w_full().child(art))
            .child(
                div()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(game.title.clone()),
                    )
                    .child(
                        div().text_xs().text_color(rgb(MUTED)).truncate().child(
                            source
                                .map(|s| s.display_name.clone())
                                .unwrap_or_else(|| "Fonte ausente".into()),
                        ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if game.compat_status.starts_with("Pronto") {
                                ACCENT
                            } else {
                                MUTED
                            }))
                            .truncate()
                            .child(game.compat_status.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .mt_2()
                            .child(self.primary(
                                &format!("play-{id}"),
                                "Jogar",
                                Action::Play(id),
                                cx,
                            ))
                            .child(self.button(
                                format!("favorite-{id}"),
                                if game.favorite { "★" } else { "☆" },
                                Action::Favorite(id),
                                cx,
                            ))
                            .child(self.button(
                                format!("details-{id}"),
                                "Detalhes",
                                Action::Details(id),
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }
    fn library(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        let menu = momor_ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
            for (label, key) in [
                ("Ordem manual", "manual"),
                ("Nome", "name"),
                ("Adicionados", "added"),
                ("Jogadas recentes", "recent"),
                ("Favoritos primeiro", "favorite"),
            ] {
                let view = view.clone();
                menu = menu.entry(label, None, move |window, cx| {
                    view.update(cx, |this, cx| {
                        this.act(Action::SortBy(key.into()), window, cx)
                    })
                });
            }
            menu
        });
        let sort_menu = momor_ui::DropdownMenu::new("sort-menu", "Ordenar", menu);
        let games = self.filtered();
        let columns = if f32::from(window.viewport_size().width) > 1120. {
            3
        } else {
            2
        };
        let rows = games.len().div_ceil(columns);
        let entity = cx.entity();
        let list = uniform_list("game-cards", rows, move |range, window, cx| {
            entity.update(cx, |this, cx| {
                let games = this.filtered();
                range
                    .map(|row| {
                        div()
                            .flex()
                            .gap_4()
                            .pb_4()
                            .children((0..columns).map(|col| {
                                if let Some(game) = games.get(row * columns + col) {
                                    this.queue_cover(game, window, cx);
                                    this.card(game, cx)
                                } else {
                                    div().flex_1().into_any_element()
                                }
                            }))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .flex_1()
        .w_full();
        div()
            .flex()
            .flex_col()
            .gap_5()
            .size_full()
            .p_6()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Meus jogos"),
                            )
                            .child(div().text_sm().text_color(rgb(MUTED)).child(format!(
                                "{} jogos · sua biblioteca privada, neste computador",
                                games.len()
                            ))),
                    )
                    .child(self.primary("add-game", "+ Adicionar jogo", Action::Add, cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div().flex().gap_2().children(
                            ["Todos", "Favoritos", "Instalados", "Online"]
                                .into_iter()
                                .map(|filter| {
                                    self.button(
                                        format!("filter-{filter}"),
                                        filter,
                                        Action::Filter(filter.into()),
                                        cx,
                                    )
                                    .when(self.filter == filter, |b| {
                                        b.border_color(rgb(ACCENT)).text_color(rgb(ACCENT))
                                    })
                                }),
                        ),
                    )
                    .child(sort_menu),
            )
            .when(games.is_empty(), |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .items_center()
                        .gap_3()
                        .text_color(rgb(MUTED))
                        .child("Sua próxima sessão começa aqui.")
                        .child("Adicione um link, uma API de biblioteca ou um pacote local."),
                )
            })
            .when(!games.is_empty(), |d| d.child(list))
            .into_any_element()
    }
    fn detail(&self, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let Some(game) = self.games.iter().find(|g| g.id == id) else {
            return div().child("Jogo removido").into_any_element();
        };
        let source = self.sources.iter().find(|s| s.id == game.source_id);
        div().id("detail-scroll").overflow_y_scroll().flex().flex_col().gap_5().p_6().size_full().child(self.button("detail-back","← Biblioteca",Action::Back,cx))
            .child(div().rounded_lg().bg(rgb(PANEL)).border_1().border_color(rgb(BORDER)).p_6().flex().flex_col().gap_4().child(div().text_xs().text_color(rgb(ACCENT)).child("BIBLIOTECA / DETALHES"))
                .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(game.title.clone())).child(div().text_color(rgb(MUTED)).child(if game.description.is_empty(){"Descrição não informada".into()}else{game.description.clone()}))
                .child(div().flex().gap_2().flex_wrap().child(self.primary("detail-play","Jogar",Action::Play(id),cx)).child(self.button("edit-game","Editar",Action::Edit(id),cx)).child(self.button("verify-game","Verificar arquivos",Action::Verify(id),cx)).child(self.button("star-game",if game.favorite{"★ Favorito"}else{"☆ Favoritar"},Action::Favorite(id),cx))))
            .child(self.row("Disponibilidade",&game.compat_status)).child(self.row("Tipo",source.map(|s|s.kind.label()).unwrap_or("Não informado"))).child(self.row("Entrada",&game.entry_uri))
            .child(self.row("Origem",source.map(|s|s.location.as_str()).unwrap_or("Não informado"))).child(self.row("Versão",game.version.as_deref().unwrap_or("Não informado")))
            .child(self.row("Sessões",&game.play_count.to_string())).child(self.row("Saves","Perfil WebView2 separado por jogo. IndexedDB/localStorage mantêm a origem estável."))
            .when(game.external_id.as_deref()==Some("gta-legacy")&&game.entry_uri.starts_with("https://fiveplay.lol/"),|d|d
                .child(self.row("Qualidade",if game.entry_uri.contains("wgr_quality=economy"){"Economia: reduz texturas e distância de detalhe"}else{"Equilibrado: texturas e distância de detalhe intermediárias"}))
                .child(div().flex().gap_2().child(self.button("gta-balanced","Equilibrado",Action::GtaGraphics(id,"balanced"),cx)).child(self.button("gta-economy","Economia de memória",Action::GtaGraphics(id,"economy"),cx))))
            .child(self.row("Requisitos",&format!("WebGPU: {} · memory64: {} · memória compartilhada: {}",game.requirements.webgpu,game.requirements.wasm_memory64,game.requirements.shared_array_buffer)))
            .child(div().flex().gap_2().flex_wrap().when_some(source,|d,s|d.child(self.button("edit-source","Configurar fonte / arquivos",Action::EditSource(s.id),cx))).child(self.button("move-up","↑ Mover",Action::Reorder(id,-1),cx)).child(self.button("move-down","↓ Mover",Action::Reorder(id,1),cx)).child(self.button("remove-game","Remover registro",Action::Remove(id),cx).text_color(rgb(DANGER))))
            .into_any_element()
    }
    fn row(&self, label: &str, value: &str) -> Div {
        div()
            .flex()
            .gap_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .w(px(150.))
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(label.to_string()),
            )
            .child(div().flex_1().min_w_0().text_sm().child(value.to_string()))
    }
    fn sources_view(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("sources-list")
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .size_full()
            .child(div().text_2xl().child("Fontes da biblioteca"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Links, catálogos e arquivos escolhidos por você. Sem login do WGR."),
            )
            .child(self.primary("add-source", "+ Adicionar fonte", Action::Add, cx))
            .children(self.sources.iter().map(|source| {
                let id = source.id;
                div()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(PANEL))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                        "{}{}",
                        if source.favorite { "★ " } else { "" },
                        source.display_name
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(source.kind.label()),
                    )
                    .child(div().text_sm().child(source.location.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(source.status.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .flex_wrap()
                            .child(self.button(
                                format!("source-edit-{id}"),
                                "Editar / testar",
                                Action::EditSource(id),
                                cx,
                            ))
                            .when(source.kind == SourceKind::RemoteCatalogApi, |d| {
                                d.child(self.button(
                                    format!("sync-{id}"),
                                    "Sincronizar",
                                    Action::Sync(id),
                                    cx,
                                ))
                            })
                            .child(self.button(
                                format!("source-star-{id}"),
                                "☆",
                                Action::FavoriteSource(id),
                                cx,
                            ))
                            .child(self.button(
                                format!("source-up-{id}"),
                                "↑",
                                Action::ReorderSource(id, -1),
                                cx,
                            ))
                            .child(self.button(
                                format!("source-down-{id}"),
                                "↓",
                                Action::ReorderSource(id, 1),
                                cx,
                            ))
                            .child(self.button(
                                format!("source-remove-{id}"),
                                "Remover",
                                Action::RemoveSource(id),
                                cx,
                            )),
                    )
            }))
            .into_any_element()
    }
    fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        div().p_6().flex().flex_col().gap_5().size_full().child(div().text_2xl().child("Configurações"))
            .child(self.row("Privacidade","Telemetria desligada. Sem conta, upload ou servidor remoto obrigatório."))
            .child(self.row("Armazenamento",&self.services.paths.root.to_string_lossy()))
            .child(self.button("fullscreen-setting",format!("Iniciar em tela cheia: {}",if self.settings.fullscreen{"sim"}else{"não"}),Action::ToggleFullscreen,cx))
            .child(self.button("cache-limit",format!("Limite do cache: {} MiB · clicar para alterar",self.settings.cache_limit_mib),Action::CacheLimit,cx))
            .child(div().text_sm().text_color(rgb(MUTED)).child("O limite do cache Rust é aplicado ao iniciar. Perfis WebView2 e saves ocupam armazenamento separado."))
            .child(div().flex().gap_2().child(self.button("clear-cache","Limpar cache Rust",Action::ClearCache,cx)).child(self.button("reveal","Abrir pasta de dados",Action::Reveal,cx)))
            .child(self.row("GPU e qualidade","Usamos o adaptador escolhido pelo WebView2. Qualidade e FPS são controlados pelo jogo quando ele oferece essas opções."))
            .child(self.row("Atalhos","Ctrl+K buscar · Ctrl+N adicionar · Ctrl+Shift+B voltar do player · F11 tela cheia · Tab/Shift+Tab navegar"))
            .into_any_element()
    }
    fn diagnostics_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let caps = self.diagnostic.get("capabilities");
        let rows = [
            "WebAssembly",
            "memory64",
            "SharedArrayBuffer",
            "crossOriginIsolated",
            "Atomics",
            "Worker",
            "OffscreenCanvas",
            "transferControlToOffscreen",
            "WebGPU",
            "AudioWorklet",
            "IndexedDB",
            "Gamepad",
        ]
        .into_iter()
        .map(|key| {
            let value = caps.and_then(|c| c.get(key));
            let status = match value {
                Some(serde_json::Value::Bool(true)) => "Disponível".into(),
                Some(serde_json::Value::Bool(false)) => "Não disponível".into(),
                Some(v) if v["available"] == true => v.to_string(),
                Some(v) => format!("Não disponível · {v}"),
                None => "Não testado".into(),
            };
            self.row(key, &status)
        });
        div().id("diagnostics-scroll").overflow_y_scroll().p_6().flex().flex_col().gap_3().size_full().child(div().text_2xl().child("Diagnóstico real do runtime"))
            .child(div().text_sm().text_color(rgb(MUTED)).child("Os testes rodam no contexto da sessão. Abra Órbita para coletar o primeiro diagnóstico; recursos indisponíveis não são simulados."))
            .child(self.row("WebView2",self.diagnostic["runtime"].as_str().unwrap_or("Não testado"))).children(rows)
            .child(self.row("Primeiro frame",&self.diagnostic.get("first_frame_ms").filter(|v|!v.is_null()).map(|v|format!("{v} ms · reportado pelo jogo")).unwrap_or_else(||"Não informado".into())))
            .child(self.row("FPS",&self.diagnostic.get("fps").filter(|v|!v.is_null()).map(|v|format!("{v} · reportado pela fixture")).unwrap_or_else(||"Não informado".into())))
            .child(self.button("export-diagnostic","Exportar relatório JSON",Action::Export,cx)).into_any_element()
    }
    fn modal_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(modal) = &self.modal else {
            return div().into_any_element();
        };
        let content=match modal{
            Modal::Wizard=>{let is_preview=self.preview.is_some();div().flex().flex_col().gap_3().child(div().text_xs().text_color(rgb(ACCENT)).child(if is_preview{"3 / 3 · REVISAR E SALVAR"}else{"1 / 3 · ESCOLHER FONTE → 2 / 3 · TESTAR"}))
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Adicionar jogo ou fonte"))
                .when(!is_preview,|d|d.child(div().flex().gap_2().flex_wrap().children([SourceKind::RemoteWeb,SourceKind::RemoteCatalogApi,SourceKind::LocalWebBundle,SourceKind::GtaLegacyZip].into_iter().map(|kind|self.button(format!("kind-{kind:?}"),kind.label(),Action::Kind(kind),cx).when(kind==self.kind,|b|b.text_color(rgb(ACCENT)).border_color(rgb(ACCENT))))))
                    .child(self.field("URL ou pasta autorizada",&self.location)).when(matches!(self.kind,SourceKind::LocalWebBundle|SourceKind::GtaLegacyZip),|d|d.child(div().flex().gap_2().child(self.button("pick-folder","Selecionar pasta",Action::PickFolder,cx)).when(self.kind==SourceKind::LocalWebBundle,|d|d.child(self.button("pick-zip","Selecionar ZIP",Action::PickZip,cx)))))
                    .when(self.kind==SourceKind::RemoteCatalogApi,|d|d.child(self.button("auth-kind",format!("Autenticação: {}",["sem autenticação","Bearer token","API key · X-API-Key","Basic · usuário:senha"][self.auth as usize]),Action::Auth,cx)).when(self.auth!=0,|d|d.child(self.field("Credencial (guardada no Windows)",&self.secret))).when(self.auth==2,|d|d.child(self.field("Header da API key",&self.header))))
                    .when(matches!(self.kind,SourceKind::RemoteWeb|SourceKind::RemoteCatalogApi),|d|d.child(self.button("dev-mode",format!("{} Permitir HTTP somente em loopback (desenvolvimento)",if self.development{"☑"}else{"☐"}),Action::Dev,cx)))
                    .when(matches!(self.kind,SourceKind::RemoteCatalogApi|SourceKind::GtaLegacyZip),|d|d.child(self.field("Origens externas explicitamente autorizadas (vírgulas)",&self.extra)))
                    .when(self.kind==SourceKind::GtaLegacyZip,|d|d.child(div().text_sm().text_color(rgb(MUTED)).child("O jogo não está incluído. A pasta deve conter b/ e data/. O ZIP remoto será consultado somente se você autorizar abaixo."))
                        .child(self.button("gta-consent",format!("{} Tenho autorização para os arquivos e autorizo consultar a fonte ZIP indicada no índice",if self.network{"☑"}else{"☐"}),Action::Network,cx))))
                .when(is_preview,|d|d.child(div().text_sm().text_color(rgb(MUTED)).child(self.preview.as_ref().map(|p|p.report.clone()).unwrap_or_default()))
                    .child(div().text_sm().child(self.preview.as_ref().map(|p|p.games.iter().take(8).map(|g|g.title.clone()).collect::<Vec<_>>().join(" · ")).unwrap_or_default())))
                .child(self.field("Título (catálogos preservam os títulos de cada jogo)",&self.title))
                .child(div().flex().justify_between().child(self.button("wizard-cancel","Cancelar",Action::Cancel,cx)).child(if is_preview{self.primary("wizard-save","Salvar na biblioteca",Action::SaveImport,cx)}else{self.primary("wizard-analyze","Testar e analisar",Action::Analyze,cx)}))
            },
            Modal::Edit(id)=>div().flex().flex_col().gap_3().child(div().text_xl().child("Editar jogo")).child(self.field("Título",&self.title)).child(self.field("Entrada (HTML / URL)",&self.location)).child(self.field("Descrição",&self.description)).child(self.field("Capa local (opcional)",&self.extra))
                .child(self.button("pick-cover","Selecionar capa",Action::PickCover,cx)).child(div().flex().gap_2().child(self.primary("edit-save","Salvar alterações",Action::SaveEdit,cx)).child(self.button(format!("edit-cancel-{id}"),"Cancelar",Action::Cancel,cx))),
            Modal::Remove(id)=>div().flex().flex_col().gap_4().child(div().text_xl().child("Remover este jogo da biblioteca?" )).child("O pacote original e os saves serão preservados.").child(div().flex().gap_2().child(self.button("remove-confirm","Remover registro",Action::ConfirmRemove(*id),cx)).child(self.button("remove-cancel","Cancelar",Action::Cancel,cx))),
            Modal::RemoveSource(id)=>div().flex().flex_col().gap_4().child(div().text_xl().child("Remover a fonte e seus jogos?" )).child("Os registros e a credencial desta fonte serão removidos. Arquivos e saves serão preservados.").child(div().flex().gap_2().child(self.button("remove-source-confirm","Remover fonte",Action::ConfirmRemoveSource(*id),cx)).child(self.button("remove-source-cancel","Cancelar",Action::Cancel,cx))),
        };
        div()
            .absolute()
            .inset_0()
            .bg(rgba(0x000000b0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(690.))
                    .max_h(relative(0.93))
                    .id("modal-scroll")
                    .overflow_y_scroll()
                    .bg(rgb(BG))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .rounded_lg()
                    .shadow_xl()
                    .p_6()
                    .h(px(550.))
                    .child(
                        momor_ui::Modal::new("wgr-dialog", None)
                            .show_dismiss(false)
                            .child(content),
                    )
                    .when(!self.notice.is_empty(), |d| {
                        d.child(
                            div()
                                .mt_4()
                                .text_sm()
                                .text_color(rgb(if self.error { DANGER } else { MUTED }))
                                .child(self.notice.clone()),
                        )
                    }),
            )
            .into_any_element()
    }
    fn smoke_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let elapsed = self.smoke_started.elapsed().as_secs();
        if self.gta_smoke {
            if std::env::args().any(|arg| arg == "--clear-profile-cache")
                && self.smoke_report["native_cache_cleared"] == true
                && self.smoke_report["profile_cache_cleared"] == true
            {
                self.smoke_report["saves_preserved"] = serde_json::json!(true);
                self.smoke_report["diagnostic"] = self.diagnostic.clone();
                if let Some(session) = &self.session {
                    self.smoke_report["cpu_profiles"] = session.player.cpu_profile_status();
                }
                self.smoke_report["profile_game_id"] =
                    serde_json::json!(self.session.as_ref().map(|session| session.launch.game.id));
                self.smoke_report["in_progress"] = serde_json::json!(false);
                if let Some(path) = &self.smoke {
                    let _ = std::fs::write(
                        path,
                        serde_json::to_vec_pretty(&self.smoke_report).unwrap_or_default(),
                    );
                }
                self.stop_player(window, cx);
                cx.quit();
                return;
            }
            if elapsed.is_multiple_of(5)
                && self.smoke_report["elapsed_seconds"].as_u64() != Some(elapsed)
            {
                self.smoke_report["elapsed_seconds"] = serde_json::json!(elapsed);
                self.smoke_report["in_progress"] = serde_json::json!(true);
                self.smoke_report["diagnostic"] = self.diagnostic.clone();
                if let Some(session) = &self.session {
                    self.smoke_report["cpu_profiles"] = session.player.cpu_profile_status();
                }
                if let Some(path) = &self.smoke {
                    let _ = std::fs::write(
                        path,
                        serde_json::to_vec_pretty(&self.smoke_report).unwrap_or_default(),
                    );
                }
            }
            if self.smoke_stage == 0 && elapsed >= 10 {
                if let Some(session) = &self.session {
                    session.player.focus();
                    let result = session.player.smoke_click(320, 240);
                    self.smoke_report["native_focus_click"] = serde_json::json!(result.is_ok());
                }
                self.smoke_stage = 1;
            }
            let reported = self.session.as_ref().is_some_and(|s| s.frames);
            // A positive FPS can still be the loading scene. Benchmark only
            // after the GPU worker reports the world, allowing the overlay to fade.
            let world_ms = self.smoke_report["world_at_ms"].as_u64();
            let world_elapsed = self.session.as_ref().zip(world_ms).map(|(session, world)| {
                session
                    .player
                    .elapsed_ms()
                    .saturating_sub(u128::from(world))
            });
            if (world_elapsed.is_some_and(|ms| ms >= 4000) || elapsed >= 890)
                && self.smoke_stage < 2
            {
                if let Some(session) = &self.session
                    && let Some(path) = &self.smoke
                {
                    let _ = session.player.capture(path.with_extension("png"));
                }
                self.smoke_stage = 2;
                self.smoke_report["capture_requested_at"] = serde_json::json!(elapsed);
            }
            let keep_open = std::env::args().any(|arg| arg == "--gta-keep-open");
            if std::env::args().any(|arg| arg == "--gta-trace")
                && let Some(session) = &self.session
            {
                if world_elapsed.is_some_and(|ms| ms >= 5000)
                    && self.smoke_report["trace_started_ms"].is_null()
                    && let Some(path) = &self.smoke
                {
                    self.smoke_report["trace_started_ms"] =
                        serde_json::json!(session.player.elapsed_ms());
                    if let Err(error) = session
                        .player
                        .start_trace(path.with_extension("trace.json"))
                    {
                        self.smoke_report["trace"] =
                            serde_json::json!({"status":"error","message":error.to_string()});
                    }
                }
                if let Some(start) = self.smoke_report["trace_started_ms"].as_u64()
                    && session
                        .player
                        .elapsed_ms()
                        .saturating_sub(u128::from(start))
                        >= 20000
                    && self.smoke_report["trace_stop_requested"] != true
                {
                    self.smoke_report["trace_stop_requested"] = serde_json::json!(true);
                    let _ = session.player.end_trace();
                }
            }
            if self.error
                || (!keep_open && (elapsed >= 900 || world_elapsed.is_some_and(|ms| ms >= 30000)))
            {
                self.smoke_report["port_reports_frames"] = serde_json::json!(reported);
                self.smoke_report["world_confirmed"] = serde_json::json!(world_ms.is_some());
                self.smoke_report["in_progress"] = serde_json::json!(false);
                self.smoke_report["diagnostic"] = self.diagnostic.clone();
                self.smoke_report["last_notice"] = serde_json::json!(self.notice);
                self.smoke_report["gta_playability_confirmed"] = serde_json::json!(false);
                self.smoke_report["input_tested"] = serde_json::json!(false);
                self.smoke_report["audio_tested"] = serde_json::json!(false);
                if let Some(path) = &self.smoke {
                    let _ = std::fs::write(
                        path,
                        serde_json::to_vec_pretty(&self.smoke_report).unwrap_or_default(),
                    );
                }
                let failure_notice = self.notice.clone();
                self.stop_player(window, cx);
                if keep_open {
                    self.smoke = None;
                    self.gta_smoke = false;
                    self.notice = failure_notice;
                    self.error = true;
                    cx.notify();
                } else {
                    cx.quit();
                }
            }
            return;
        }
        if let Some(session) = &self.session {
            if session.frames && self.smoke_stage == 0 {
                self.smoke_stage = 1;
                if let Some(path) = &self.smoke
                    && let Ok(raw) = HasWindowHandle::window_handle(window)
                    && let raw_window_handle::RawWindowHandle::Win32(handle) = raw.as_raw()
                {
                    let handle = handle.hwnd.get();
                    let path = path.with_extension("window.png");
                    self.runtime.spawn_blocking(move || {
                        crate::screenshot::capture_owned_frame(handle, &path)
                    });
                }
                if let Some(path) = &self.smoke {
                    let _ = session.player.capture(path.with_extension("png"));
                }
                let native = session.player.smoke_key();
                self.smoke_report["native_key_sent"] = serde_json::json!(native.is_ok());
                if let Err(error) = native {
                    self.smoke_report["native_key_error"] = serde_json::json!(error.to_string());
                }
                if !session.player.is_native() {
                    let _ = session
                        .player
                        .script("document.querySelector('#gpu').click();");
                }
            }
            if session.frames && session.input && elapsed >= 5 && self.smoke_stage == 1 {
                self.smoke_stage = 2;
                self.smoke_report["diagnostic"] = self.diagnostic.clone();
                self.smoke_report["frames"] = serde_json::json!(session.frames);
                self.smoke_report["input_received"] = serde_json::json!(session.input);
                self.smoke_report["library_games"] = serde_json::json!(self.games.len());
                self.smoke_report["elapsed_ms"] =
                    serde_json::json!(self.smoke_started.elapsed().as_millis());
            }
        }
        if self.smoke_stage == 2 || elapsed >= 45 {
            let success = self.smoke_stage == 2 && !self.error;
            self.smoke_report["passed"] = serde_json::json!(success);
            self.smoke_report["last_notice"] = serde_json::json!(self.notice);
            if let Some(path) = &self.smoke {
                let _ = std::fs::write(
                    path,
                    serde_json::to_vec_pretty(&self.smoke_report).unwrap_or_default(),
                );
            }
            self.stop_player(window, cx);
            cx.quit();
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(session) = &self.session {
            if self.error {
                return div()
                    .size_full()
                    .bg(rgb(BG))
                    .text_color(rgb(TEXT))
                    .font_family("Segoe UI")
                    .p_8()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(DANGER))
                            .child("SESSÃO INTERROMPIDA"),
                    )
                    .child(div().text_2xl().child(session.launch.game.title.clone()))
                    .child(
                        div()
                            .max_w(px(820.))
                            .text_color(rgb(DANGER))
                            .child(self.notice.clone()),
                    )
                    .child(self.primary(
                        "player-error-back",
                        "Voltar à biblioteca",
                        Action::Back,
                        cx,
                    ))
                    .into_any_element();
            }
            #[cfg(not(feature = "web-compat"))]
            {
                if session.player.is_external() {
                    return div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgb(0x000000))
                        .text_color(rgb(TEXT))
                        .child(self.notice.clone())
                        .into_any_element();
                }
                let state = session.player.frame();
                window.request_animation_frame();
                let width = f32::from(window.viewport_size().width);
                let height = f32::from(window.viewport_size().height);
                return div()
                    .size_full()
                    .relative()
                    .bg(rgb(0x161d28))
                    .text_color(rgb(TEXT))
                    .font_family("Segoe UI")
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.act(Action::Back, window, cx)
                        } else if let Some(session) = &this.session {
                            session.player.key(&event.keystroke.key, true);
                        }
                    }))
                    .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| {
                        if let Some(session) = &this.session {
                            session.player.key(&event.keystroke.key, false);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, _| {
                            if let Some(session) = &this.session {
                                let size = window.viewport_size();
                                session.player.click(
                                    (f32::from(event.position.x) / f32::from(size.width) * 960.)
                                        as i32,
                                    (f32::from(event.position.y) / f32::from(size.height) * 420.)
                                        as i32,
                                );
                            }
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(width * state.x as f32 / 960. - 12.))
                            .top(px(height * state.y as f32 / 420. - 12.))
                            .size(px(24.))
                            .rounded_full()
                            .bg(rgb(0xa9c8ff)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(width * 0.72))
                            .top(px(height * 0.5))
                            .size(px(16.))
                            .rounded_full()
                            .bg(rgb(0x8bebc6)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_6()
                            .top_6()
                            .child("Órbita · WASM executado por Rust · setas ou WASD para mover"),
                    )
                    .into_any_element();
            }
            #[cfg(feature = "web-compat")]
            return div()
                .size_full()
                .bg(rgb(0x000000))
                .text_color(rgb(TEXT))
                .font_family("Segoe UI")
                .into_any_element();
        }
        let view = match self.view {
            View::Library => self.library(window, cx),
            View::Sources => self.sources_view(cx),
            View::Settings => self.settings_view(cx),
            View::Diagnostics => self.diagnostics_view(cx),
            View::Detail(id) => self.detail(id, cx),
        };
        let sidebar = div()
            .w(px(222.))
            .flex_shrink_0()
            .bg(rgb(SIDE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .mt_3()
                    .mb_2()
                    .child("BIBLIOTECA"),
            )
            .children(
                ["Todos", "Favoritos", "Recentes", "Instalados"]
                    .into_iter()
                    .map(|label| {
                        momor_ui::ListItem::new(ElementId::Name(format!("nav-{label}").into()))
                            .rounded()
                            .toggle_state(self.filter == label && self.view == View::Library)
                            .child(
                                self.button(
                                    format!("side-{label}"),
                                    label,
                                    Action::Filter(label.into()),
                                    cx,
                                )
                                .border_0()
                                .when(self.filter == label && self.view == View::Library, |b| {
                                    b.bg(rgb(HOVER)).text_color(rgb(ACCENT))
                                }),
                            )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .mt_5()
                    .mb_2()
                    .child("FONTES"),
            )
            .child(
                self.button("sources", "Minhas fontes", Action::Sources, cx)
                    .border_0(),
            )
            .child(
                self.button("add-sidebar", "+ Adicionar fonte", Action::Add, cx)
                    .border_0(),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .mt_5()
                    .mb_2()
                    .child("SISTEMA"),
            )
            .child(
                self.button("diagnostics", "Diagnóstico", Action::Diagnostics, cx)
                    .border_0(),
            )
            .child(
                self.button("settings", "Configurações", Action::Settings, cx)
                    .border_0(),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(ACCENT))
                    .child("●  Biblioteca local"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("WGR 0.1.0 · Windows"),
            );
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Segoe UI")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.modifiers.control && key.key == "k" {
                    window.focus(&this.search.read(cx).focus_handle);
                } else if key.modifiers.control && key.key == "n" {
                    this.act(Action::Add, window, cx);
                } else if key.key == "escape" {
                    this.act(Action::Back, window, cx);
                } else if key.key == "tab" {
                    if key.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .px_6()
                    .bg(rgb(SIDE))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(ACCENT))
                            .child("WGR"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("WEB GAME RUNTIME"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(
                        div()
                            .w(px(330.))
                            .rounded_md()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(BORDER))
                            .children(self.input_widgets.get(&self.search.entity_id()).cloned()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(div().flex_1().min_w_0().child(view)),
            )
            .child(
                div()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_4()
                    .bg(rgb(SIDE))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(if self.error { DANGER } else { MUTED }))
                    .child(if self.busy {
                        "Processando…".into()
                    } else if self.notice.is_empty() {
                        "Seus jogos. Suas fontes. Sem conta.  ·  Ctrl+N adicionar".into()
                    } else {
                        self.notice.clone()
                    }),
            )
            .when(self.modal.is_some(), |d| d.child(self.modal_view(cx)))
            .into_any_element()
    }
}
impl Drop for Shell {
    fn drop(&mut self) {
        if let Some(job) = self.active_job.take() {
            job.abort();
        }
        if let Some(mut session) = self.session.take() {
            session.launch.session.end_at = Some(now());
            session.launch.session.reason = Some("Janela fechada pelo usuário".into());
            session.launch.session.metrics = self.diagnostic.clone();
            if !matches!(
                session.launch.session.state,
                SessionState::Crashed | SessionState::Failure
            ) {
                session.launch.session.state = SessionState::Stopped;
            }
            if let Ok(mut db) = Library::open(&self.services.paths.database) {
                let _ = db.save_session(&session.launch.session);
                let _ = db.set_value("last_diagnostic", &self.diagnostic);
            }
            drop(session.player);
            self.services.cache.unpin_all();
        }
    }
}
fn move_item(ids: &mut [Uuid], id: Uuid, direction: i32) {
    if let Some(i) = ids.iter().position(|v| *v == id) {
        let next = i
            .saturating_add_signed(direction as isize)
            .min(ids.len().saturating_sub(1));
        ids.swap(i, next);
    }
}
fn missing_capabilities(
    requirements: &Requirements,
    capabilities: &serde_json::Value,
) -> Vec<&'static str> {
    if capabilities["backend"] == "native" {
        return vec![];
    }
    let mut missing = vec![];
    if capabilities["WebAssembly"] != true {
        missing.push("WebAssembly");
    }
    if requirements.webgpu && capabilities["WebGPU"]["available"] != true {
        missing.push("WebGPU / requestDevice");
    }
    if requirements.wasm_memory64 && capabilities["memory64"] != true {
        missing.push("WebAssembly memory64");
    }
    if requirements.shared_array_buffer
        && (capabilities["SharedArrayBuffer"] != true
            || capabilities["crossOriginIsolated"] != true)
    {
        missing.push("memória compartilhada / isolamento");
    }
    if !requirements.unknown.is_empty() {
        missing.push("requisitos desconhecidos no manifesto");
    }
    missing
}
