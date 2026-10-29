use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;
use wgr_core::{
    assets::{self, DiskCache},
    catalog,
    credentials::{CredentialStore, Secret, WindowsVault},
    domain::*,
    gta::{self, RemoteArchive, ZipIndex},
    security::{NetworkPolicy, validate_url},
    storage::Library,
};
use wgr_http::{AssetServer, ServerConfig};

pub struct Services {
    pub paths: Paths,
    pub cache: Arc<DiskCache>,
    cover_slots: tokio::sync::Semaphore,
}
pub struct Preview {
    pub source: Source,
    pub games: Vec<Game>,
    pub report: String,
    pub secret: Option<Secret>,
}
pub struct ImportRequest {
    pub kind: SourceKind,
    pub location: String,
    pub title: String,
    pub auth: AuthScheme,
    pub secret: Option<Secret>,
    pub development: bool,
    pub network: bool,
    pub allowed_origins: Vec<String>,
}
pub struct Launch {
    pub binary: Option<PathBuf>,
    pub game: Game,
    pub source: Source,
    pub entry: String,
    pub origin: String,
    pub server: Option<AssetServer>,
    pub session: Session,
    pub fixture: bool,
}
impl Services {
    pub async fn configure_gta(
        &self,
        reference: &Path,
        origins: Vec<String>,
    ) -> Result<serde_json::Value> {
        let index_path = reference.join("assets/zip_index.json");
        let index = ZipIndex::load(&index_path)?;
        let mut allowed = vec![
            validate_url(&index.url, false)?
                .origin()
                .ascii_serialization(),
        ];
        for origin in origins {
            let origin = validate_url(&origin, false)?.origin().ascii_serialization();
            if !allowed.contains(&origin) {
                allowed.push(origin);
            }
        }
        let policy = NetworkPolicy {
            allowed_origins: allowed.clone(),
            development_loopback: false,
        };
        let archive = Arc::new(RemoteArchive::new(
            index.clone(),
            policy,
            self.cache.clone(),
        )?);
        let build = index
            .builds()?
            .into_iter()
            .next()
            .context("Índice sem build")?;
        let loader = format!("b/{build}/loader.js");
        let bytes = archive
            .read(&loader, 0, 7)
            .await
            .context("O loader indexado não pôde ser carregado da fonte configurada")?;
        let adapter = self.paths.root.join("adapters/gta");
        std::fs::create_dir_all(adapter.join("assets"))?;
        std::fs::create_dir_all(adapter.join("public"))?;
        std::fs::copy(index_path, adapter.join("assets/zip_index.json"))?;
        std::fs::write(
            adapter.join("public/play.html"),
            include_bytes!("../../../assets/gta/play.html"),
        )?;
        let mut db = Library::open(&self.paths.database)?;
        let existing = db
            .sources()?
            .into_iter()
            .find(|s| s.kind == SourceKind::GtaLegacyZip);
        let mut source = existing.unwrap_or_else(|| {
            Source::new(
                SourceKind::GtaLegacyZip,
                "5-web-port",
                &adapter.to_string_lossy(),
            )
        });
        source.location = adapter.to_string_lossy().into_owned();
        source.display_name = "5-web-port · streaming".into();
        source.allowed_origins = allowed;
        source.status = "Streaming verificado · loader acessível".into();
        source.last_sync_at = Some(now());
        let mut game = db
            .games()?
            .into_iter()
            .find(|g| g.source_id == source.id)
            .unwrap_or_else(|| {
                Game::new(
                    &source,
                    "Grand Theft Auto V — Web Port (teste)",
                    "play.html",
                )
            });
        game.external_id = Some("gta-legacy".into());
        game.profile.safe_mode = true;
        game.compat_status = "Streaming configurado · iniciar player".into();
        game.description="Fonte 5-web-port configurada para streaming por intervalos a partir do ZIP do índice. Loader verificado; execução da engine ainda precisa ser confirmada.".into();
        game.requirements = Requirements {
            webgpu: true,
            shared_array_buffer: true,
            wasm_memory64: true,
            ..Default::default()
        };
        db.import(&source, &[game.clone()])?;
        db.save_game(&game)?;
        let report = serde_json::json!({"configured":true,"game_id":game.id,"entries":index.entries.len(),"zip_size":index.size,"build":build,"loader_prefix":String::from_utf8_lossy(&bytes),"origins":source.allowed_origins,"mode":"remote_zip_streaming"});
        std::fs::write(
            self.paths.root.join("gta-configuration.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        archive.cache.unpin_all();
        Ok(report)
    }
    pub async fn thumbnail(&self, game: Game, source: Source) -> Result<Option<String>> {
        let _slot = self.cover_slots.acquire().await?;
        let Some(uri) = game.cover_uri else {
            return Ok(None);
        };
        let bytes = if uri.starts_with("https://") || uri.starts_with("http://") {
            let client = wgr_core::security::HttpClient::new(NetworkPolicy {
                allowed_origins: source.allowed_origins,
                development_loopback: source.development_loopback,
            });
            let reply = client.get(&uri, 8 * 1024 * 1024).await?;
            ensure!(
                reply.status.is_success(),
                "Capa indisponível: HTTP {}",
                reply.status
            );
            reply.body
        } else {
            let path = PathBuf::from(&uri);
            ensure!(
                std::fs::metadata(&path)?.len() <= 8 * 1024 * 1024,
                "Capa maior que 8 MiB"
            );
            std::fs::read(path)?
        };
        let folder = self.paths.root.join("covers");
        let path = tokio::task::spawn_blocking(move || make_thumbnail(&bytes, &folder)).await??;
        Ok(Some(path.to_string_lossy().into_owned()))
    }
    pub fn new(paths: Paths) -> Result<Self> {
        let db = Library::open(&paths.database)?;
        let mut settings = db.settings()?;
        settings.cache_limit_mib = settings.cache_limit_mib.clamp(1, 256);
        db.save_settings(&settings)?;
        let limit = settings.cache_limit_mib * 1024 * 1024;
        Ok(Self {
            cover_slots: tokio::sync::Semaphore::new(3),
            cache: Arc::new(DiskCache::open(paths.cache.clone(), limit)?),
            paths,
        })
    }
    pub fn seed(&self) -> Result<()> {
        let mut db = Library::open(&self.paths.database)?;
        db.recover_sessions()?;
        let fixture = self.paths.root.join("fixtures/orbit");
        std::fs::create_dir_all(fixture.join("data"))?;
        for (name, bytes) in [
            (
                "index.html",
                include_bytes!("../../../fixtures/orbit/index.html").as_slice(),
            ),
            (
                "style.css",
                include_bytes!("../../../fixtures/orbit/style.css").as_slice(),
            ),
            (
                "game.js",
                include_bytes!("../../../fixtures/orbit/game.js").as_slice(),
            ),
            (
                "worker.js",
                include_bytes!("../../../fixtures/orbit/worker.js").as_slice(),
            ),
            (
                "nested.js",
                include_bytes!("../../../fixtures/orbit/nested.js").as_slice(),
            ),
            (
                "tone.js",
                include_bytes!("../../../fixtures/orbit/tone.js").as_slice(),
            ),
            (
                "math.wasm",
                include_bytes!("../../../fixtures/orbit/math.wasm").as_slice(),
            ),
            (
                "data/probe.bin",
                include_bytes!("../../../fixtures/orbit/data/probe.bin").as_slice(),
            ),
            (
                "wgr.game.json",
                include_bytes!("../../../fixtures/orbit/wgr.game.json").as_slice(),
            ),
            (
                "LICENSE",
                include_bytes!("../../../fixtures/orbit/LICENSE").as_slice(),
            ),
        ] {
            let target = fixture.join(name);
            if !target.exists() || std::fs::read(&target)? != bytes {
                std::fs::write(target, bytes)?;
            }
        }
        let gta = self.paths.root.join("adapters/gta");
        std::fs::create_dir_all(gta.join("assets"))?;
        std::fs::create_dir_all(gta.join("public"))?;
        for (name, bytes) in [
            (
                "assets/zip_index.json",
                include_bytes!("../../../assets/gta/zip_index.json").as_slice(),
            ),
            (
                "public/play.html",
                include_bytes!("../../../assets/gta/play.html").as_slice(),
            ),
        ] {
            if name == "public/play.html" || !gta.join(name).exists() {
                std::fs::write(gta.join(name), bytes)?;
            }
        }
        if let Some(previous) = db.value::<PathBuf>("data_root")?
            && previous != self.paths.root
        {
            let relocate = |path: &PathBuf| -> PathBuf {
                path.strip_prefix(&previous)
                    .ok()
                    .filter(|relative| {
                        relative
                            .components()
                            .all(|part| matches!(part, std::path::Component::Normal(_)))
                    })
                    .map(|relative| self.paths.root.join(relative))
                    .unwrap_or_else(|| path.clone())
            };
            let games = db.games()?;
            for mut source in db.sources()? {
                let location = PathBuf::from(&source.location);
                let next = relocate(&location);
                if next != location {
                    source.location = next.to_string_lossy().into_owned();
                }
                let relocated = games
                    .iter()
                    .filter(|g| g.source_id == source.id)
                    .cloned()
                    .map(|mut game| {
                        game.local_bundle_path = game.local_bundle_path.as_ref().map(&relocate);
                        game.cover_uri = game.cover_uri.as_ref().map(|uri| {
                            relocate(&PathBuf::from(uri)).to_string_lossy().into_owned()
                        });
                        game
                    })
                    .collect::<Vec<_>>();
                db.import(&source, &relocated)?;
            }
        }
        db.set_value("data_root", &self.paths.root)?;
        // Upgrade only the launcher-managed remote entry, preserving its UUID,
        // favorites, history, profile and the previous source configuration.
        for mut game in db.games()? {
            if game.external_id.as_deref() != Some("gta-legacy") || game.local_bundle_path.is_some()
            {
                continue;
            }
            let Some(mut source) = db.source(game.source_id)? else {
                continue;
            };
            if source.kind == SourceKind::GtaLegacyZip
                && std::path::Path::new(&source.location) == gta
            {
                db.set_value(
                    "gta_zip_before_original_origin",
                    &serde_json::json!({"source":source,"game":game}),
                )?;
                original_gta_source(&mut source, &mut game);
                db.import(&source, &[game])?;
            } else if source.kind == SourceKind::RemoteWeb
                && game.description
                    == "Usa o site/CDN original com perfil persistente. Cache de dados limitado a 1 GiB e cache HTTP do navegador limitado a 256 MiB; saves ficam separados."
            {
                game.description = game
                    .description
                    .replace("limitado a 1 GiB", "limitado a 2 GiB");
                db.save_game(&game)?;
            }
        }
        if db.value::<bool>("defaults_installed")?.unwrap_or(false) {
            return Ok(());
        }
        let source = Source::new(
            SourceKind::LocalWebBundle,
            "Incluído no WGR",
            &fixture.to_string_lossy(),
        );
        let mut game = Game::new(&source, "Órbita", "index.html");
        game.external_id = Some("wgr-orbit".into());
        game.description="Encontre as luzes. Um jogo original com WebAssembly, Worker e save local. Nenhuma conexão necessária.".into();
        game.local_bundle_path = Some(fixture);
        game.compat_status = "Pronto · teste offline".into();
        game.version = Some("1.0.0".into());
        db.import(&source, &[game])?;
        let mut source = Source::new(
            SourceKind::RemoteWeb,
            "Fiveplay · origem original",
            "https://fiveplay.lol",
        );
        source.allowed_origins = vec![
            "https://fiveplay.lol".into(),
            "https://cdn.fiveplay.lol".into(),
        ];
        source.status = "Fonte de streaming configurada".into();
        let mut game = Game::new(
            &source,
            "Grand Theft Auto V — Web Port (teste)",
            "https://fiveplay.lol/?mode=sandbox",
        );
        game.external_id = Some("gta-legacy".into());
        game.profile.safe_mode = true;
        game.requirements = Requirements {
            webgpu: true,
            shared_array_buffer: true,
            wasm_memory64: true,
            ..Default::default()
        };
        game.compat_status = "Streaming configurado · verificar ao iniciar".into();
        game.description="Fonte 5-web-port: carregamento por intervalos do ZIP indicado no índice. Não exige instalar o ZIP completo. A execução da engine será verificada no player.".into();
        original_gta_source(&mut source, &mut game);
        db.import(&source, &[game])?;
        db.set_value("defaults_installed", &true)?;
        Ok(())
    }
    pub async fn preview(&self, request: ImportRequest) -> Result<Preview> {
        let mut source = Source::new(
            request.kind,
            if request.title.trim().is_empty() {
                request.kind.label()
            } else {
                &request.title
            },
            request.location.trim(),
        );
        source.auth = request.auth;
        source.development_loopback = request.development;
        match request.kind {
            SourceKind::RemoteWeb | SourceKind::RemoteCatalogApi => {
                let url = validate_url(&source.location, request.development)?;
                source.allowed_origins = vec![url.origin().ascii_serialization()];
                ensure!(
                    source.auth == AuthScheme::None || url.scheme() == "https",
                    "Autenticação exige HTTPS"
                );
                for input in request.allowed_origins {
                    let url = validate_url(&input, request.development)?;
                    let origin = url.origin().ascii_serialization();
                    if !source.allowed_origins.contains(&origin) {
                        source.allowed_origins.push(origin);
                    }
                }
                if source.kind == SourceKind::RemoteCatalogApi {
                    let (updated, games) = catalog::sync(&source, request.secret.as_ref()).await?;
                    return Ok(Preview{source:updated,games,report:"Catálogo validado. Revise os títulos e salve para importar. Nenhum token será enviado às páginas dos jogos.".into(),secret:request.secret});
                }
                let title = catalog::probe_web(&source.location, request.development).await?;
                source.display_name = title.clone();
                source.status = "Página HTML acessível · engine não confirmada".into();
                let game = Game::new(
                    &source,
                    if request.title.trim().is_empty() {
                        &title
                    } else {
                        &request.title
                    },
                    &source.location,
                );
                Ok(Preview{source,games:vec![game],report:"Página HTML acessível. WebAssembly, WebGPU e a execução do jogo serão verificados no player; uma página de marketing não comprova um port.".into(),secret:None})
            }
            SourceKind::LocalWebBundle => {
                let input = PathBuf::from(&source.location);
                let bundles = self.paths.bundles.clone();
                let info = tokio::task::spawn_blocking(move || -> Result<_> {
                    let root = if input
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
                    {
                        assets::import_zip(&input, &bundles)?
                    } else {
                        input
                    };
                    assets::inspect_bundle(&root)
                })
                .await??;
                source.location = info.root.to_string_lossy().into_owned();
                source.display_name = if request.title.trim().is_empty() {
                    info.title.clone()
                } else {
                    request.title.clone()
                };
                source.status = "Arquivos locais verificados".into();
                let mut game = Game::new(&source, &source.display_name, &info.entry);
                game.local_bundle_path = Some(info.root);
                game.requirements = info.requirements;
                game.description = info.description;
                game.version = info.version;
                game.compat_status = "Pronto · arquivo local".into();
                Ok(Preview {
                    source,
                    games: vec![game],
                    report: format!(
                        "Entrada HTML encontrada. {:.2} MiB de arquivos locais; compatibilidade do runtime ainda será testada.",
                        info.size as f64 / 1048576.
                    ),
                    secret: None,
                })
            }
            SourceKind::GtaLegacyZip => {
                let adapter = self.paths.root.join("adapters/gta");
                let mut game = Game::new(
                    &source,
                    "Grand Theft Auto V — Web Port (teste)",
                    "play.html",
                );
                game.external_id = Some("gta-legacy".into());
                game.requirements = Requirements {
                    webgpu: true,
                    shared_array_buffer: true,
                    wasm_memory64: true,
                    ..Default::default()
                };
                let selected = PathBuf::from(&source.location);
                let report = if selected.is_dir()
                    && selected.join("b").is_dir()
                    && selected.join("data").is_dir()
                {
                    game.local_bundle_path = Some(selected);
                    game.compat_status = "Mirror local · verificar engine no player".into();
                    "Mirror selecionado. O player verificará requisitos e carregará os arquivos disponíveis.".into()
                } else {
                    ensure!(
                        request.network,
                        "Selecione um mirror com b/ e data/ ou autorize explicitamente a fonte ZIP legítima."
                    );
                    let index = ZipIndex::load(&adapter.join("assets/zip_index.json"))?;
                    let url = validate_url(&index.url, false)?;
                    source.allowed_origins = vec![url.origin().ascii_serialization()];
                    for input in request.allowed_origins {
                        let origin = validate_url(&input, false)?.origin().ascii_serialization();
                        if !source.allowed_origins.contains(&origin) {
                            source.allowed_origins.push(origin);
                        }
                    }
                    let client = wgr_core::security::HttpClient::new(NetworkPolicy {
                        allowed_origins: source.allowed_origins.clone(),
                        development_loopback: false,
                    });
                    client.range(&index.url,0,3,index.size).await.context("ZIP externo indisponível ou redirecionamento não autorizado; use um mirror local legítimo")?;
                    game.compat_status = "ZIP acessível · engine ainda não verificada".into();
                    "Fonte ZIP acessível por Range. Nenhum download integral foi feito. A execução ainda depende da engine, dos limites da GPU e do runtime.".into()
                };
                source.location = adapter.to_string_lossy().into_owned();
                source.display_name = "GTA · fonte autorizada".into();
                source.status = game.compat_status.clone();
                Ok(Preview {
                    source,
                    games: vec![game],
                    report,
                    secret: None,
                })
            }
        }
    }
    pub fn commit_preview(&self, mut preview: Preview, title: &str) -> Result<usize> {
        if preview.games.len() == 1 && !title.trim().is_empty() {
            preview.games[0].title = title.trim().into();
        }
        let mut db = Library::open(&self.paths.database)?;
        if let Some(secret) = &preview.secret {
            let key = preview.source.id.to_string();
            WindowsVault.put(&key, secret)?;
            preview.source.credential_ref = Some(key);
        }
        let result = db.import(&preview.source, &preview.games);
        if result.is_err()
            && let Some(key) = &preview.source.credential_ref
        {
            let _ = WindowsVault.delete(key);
        }
        result
    }
    pub async fn launch(&self, id: Uuid) -> Result<Launch> {
        let db = Library::open(&self.paths.database)?;
        let game = db.game(id)?.context("Jogo removido")?;
        let source = db.source(game.source_id)?.context("Fonte removida")?;
        let port_key = format!("game_port:{}", game.id);
        let port = db.value::<u16>(&port_key)?;
        drop(db);
        let mut session = Session::new(game.id);
        session.state.transition(SessionState::Resolving)?;
        let fixture = game.external_id.as_deref() == Some("wgr-orbit")
            && game.local_bundle_path.as_ref() == Some(&self.paths.root.join("fixtures/orbit"));
        let binary = if cfg!(feature = "web-compat") {
            None
        } else if fixture {
            Some(self.paths.root.join("fixtures/orbit/math.wasm"))
        } else if game.external_id.as_deref() == Some("gta-legacy") {
            let manifest: serde_json::Value =
                serde_json::from_str(include_str!("../../../ports/fiveplay-gta5.port.json"))?;
            let uri = manifest["binary"]["url"]
                .as_str()
                .context("Manifesto nativo sem binário")?;
            let hash = manifest["binary"]["sha256"]
                .as_str()
                .context("Manifesto nativo sem SHA-256")?;
            let url = url::Url::parse(uri)?;
            let key = format!("gta-native-wasm:{}", url.path());
            let cache = self.cache.clone();
            let read_key = key.clone();
            let cached = tokio::task::spawn_blocking(move || cache.get(&read_key)).await??;
            Some(if let Some(path) = cached {
                path
            } else {
                let client = wgr_core::security::HttpClient::new(NetworkPolicy {
                    allowed_origins: source.allowed_origins.clone(),
                    development_loopback: false,
                });
                let reply = client
                    .request(
                        reqwest::Method::GET,
                        uri,
                        reqwest::header::HeaderMap::new(),
                        128 * 1024 * 1024,
                    )
                    .await?;
                ensure!(
                    reply.status.is_success() && assets::sha256(&reply.body) == hash,
                    "Binário diferente do manifesto do port"
                );
                let cache = self.cache.clone();
                tokio::task::spawn_blocking(move || cache.put(&key, &reply.body)).await??
            })
        } else if game
            .entry_uri
            .split('?')
            .next()
            .is_some_and(|uri| uri.ends_with(".wasm"))
        {
            let root = game
                .local_bundle_path
                .as_ref()
                .context("O port local precisa informar sua pasta")?;
            Some(wgr_core::security::rooted_file(root, &game.entry_uri)?)
        } else {
            None
        };
        let server = if !cfg!(feature = "web-compat") {
            None
        } else {
            match source.kind {
                SourceKind::RemoteWeb | SourceKind::RemoteCatalogApi => None,
                SourceKind::LocalWebBundle => {
                    let root = game
                        .local_bundle_path
                        .clone()
                        .context("Pasta do jogo não informada")?;
                    assets::inspect_bundle(&root)?;
                    Some(
                        AssetServer::start_at(
                            ServerConfig {
                                root,
                                archive: None,
                                cache: self.cache.clone(),
                            },
                            port,
                        )
                        .await?,
                    )
                }
                SourceKind::GtaLegacyZip => {
                    let root = PathBuf::from(&source.location).join("public");
                    if let Some(mirror) = &game.local_bundle_path {
                        // The authorized mirror supplies engine/data. A copied landing lives in its own root overlay.
                        // No user mirror writes: the HTTP server uses a fallback root.
                        Some(
                            AssetServer::start_at(
                                ServerConfig {
                                    root: mirror.clone(),
                                    archive: None,
                                    cache: self.cache.clone(),
                                },
                                port,
                            )
                            .await?,
                        )
                    } else {
                        ensure!(
                            !source.allowed_origins.is_empty(),
                            "A fonte do jogo não tem origens de streaming autorizadas. Configure a fonte ou use uma pasta local nos detalhes do jogo."
                        );
                        let index = ZipIndex::load(
                            &PathBuf::from(&source.location).join("assets/zip_index.json"),
                        )?;
                        let archive = Arc::new(RemoteArchive::new(
                            index,
                            NetworkPolicy {
                                allowed_origins: source.allowed_origins.clone(),
                                development_loopback: false,
                            },
                            self.cache.clone(),
                        )?);
                        Some(
                            AssetServer::start_at(
                                ServerConfig {
                                    root,
                                    archive: Some(archive),
                                    cache: self.cache.clone(),
                                },
                                port,
                            )
                            .await?,
                        )
                    }
                }
            }
        };
        session.state.transition(SessionState::Checking)?;
        let (origin, entry) = if !cfg!(feature = "web-compat") {
            (format!("native://{}", game.id), game.entry_uri.clone())
        } else if let Some(server) = &server {
            let origin = server.origin.clone();
            Library::open(&self.paths.database)?.set_value(
                &port_key,
                &url::Url::parse(&origin)?
                    .port()
                    .context("Porta local não informada")?,
            )?;
            let entry =
                if source.kind == SourceKind::GtaLegacyZip && game.local_bundle_path.is_some() {
                    "index.html"
                } else {
                    &game.entry_uri
                };
            let mut entry = format!("{origin}/{entry}");
            if source.kind == SourceKind::GtaLegacyZip {
                entry.push_str(if entry.contains('?') {
                    "&nocache=1"
                } else {
                    "?nocache=1"
                });
            }
            if source.kind == SourceKind::GtaLegacyZip && game.profile.safe_mode {
                entry.push_str(if entry.contains('?') {
                    "&low=1"
                } else {
                    "?low=1"
                });
            }
            (origin.clone(), entry)
        } else {
            let mut url = validate_url(&game.entry_uri, source.development_loopback)?;
            ensure!(
                source
                    .allowed_origins
                    .contains(&url.origin().ascii_serialization()),
                "Origem do jogo não autorizada"
            );
            if game.external_id.as_deref() == Some("gta-legacy")
                && url.origin().ascii_serialization() == "https://fiveplay.lol"
            {
                gta_launch_options(&mut url, game.profile.safe_mode);
            }
            (url.origin().ascii_serialization(), url.to_string())
        };
        let mut db = Library::open(&self.paths.database)?;
        db.save_session(&session)?;
        Ok(Launch {
            binary,
            game,
            source,
            entry,
            origin,
            server,
            session,
            fixture,
        })
    }
    pub fn verify(&self, id: Uuid) -> Result<String> {
        let db = Library::open(&self.paths.database)?;
        let mut game = db.game(id)?.context("Jogo ausente")?;
        let source = db.source(game.source_id)?.context("Fonte ausente")?;
        let message = match source.kind {
            SourceKind::LocalWebBundle => {
                let info =
                    assets::inspect_bundle(game.local_bundle_path.as_ref().context("Sem pasta")?)?;
                format!(
                    "Arquivos presentes: {} bytes. Entrada: {}. Integridade do jogo não tem hash declarado.",
                    info.size, info.entry
                )
            }
            SourceKind::GtaLegacyZip => {
                let a = gta::audit(&PathBuf::from(&source.location))?;
                serde_json::to_string_pretty(&a)?
            }
            _ => "Fonte remota: use Testar no formulário para revalidar a conexão e a API.".into(),
        };
        if source.kind == SourceKind::LocalWebBundle {
            game.compat_status = "Pronto · arquivos verificados".into();
            db.save_game(&game)?;
        }
        Ok(message)
    }
}
pub fn make_thumbnail(bytes: &[u8], folder: &Path) -> Result<PathBuf> {
    ensure!(bytes.len() <= 8 * 1024 * 1024, "Capa maior que 8 MiB");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let picture = reader
        .decode()
        .context("Capa inválida. Use PNG, JPEG ou WebP com até 8192 pixels por lado.")?;
    std::fs::create_dir_all(folder)?;
    let hash = assets::sha256(bytes);
    let path = folder.join(format!("{hash}-640.png"));
    if !path.exists() {
        picture.thumbnail(640, 640).save(&path)?;
        picture
            .thumbnail(320, 320)
            .save(folder.join(format!("{hash}-320.png")))?;
    }
    Ok(path)
}
pub fn default_paths() -> Result<Paths> {
    let executable = std::env::current_exe()?;
    let parent = executable.parent().context("Executável sem diretório")?;
    let root = if parent.join("portable.flag").is_file() {
        parent.join("portable-data")
    } else {
        Path::new(&std::env::var("LOCALAPPDATA").context("LOCALAPPDATA não definido")?).join("WGR")
    };
    Paths::at(root)
}

fn gta_launch_options(url: &mut url::Url, safe_mode: bool) {
    // The upstream page otherwise forces -frameLimit=1 (~60 FPS), even on a
    // high-refresh display. Preserve explicit per-game choices.
    if !url.query_pairs().any(|(key, _)| key == "fps") {
        url.query_pairs_mut().append_pair("fps", "0");
    }
    if !url.query_pairs().any(|(key, _)| key == "wgr_quality") {
        url.query_pairs_mut().append_pair("wgr_quality", "balanced");
    }
    if safe_mode && !url.query_pairs().any(|(key, _)| key == "low") {
        url.query_pairs_mut().append_pair("low", "1");
    }
}
pub fn gta_graphics_entry(entry: &str, profile: &str) -> Result<String> {
    ensure!(
        ["balanced", "economy"].contains(&profile),
        "Perfil gráfico inválido"
    );
    let mut url = validate_url(entry, false)?;
    ensure!(
        url.origin().ascii_serialization() == "https://fiveplay.lol",
        "Este perfil pertence ao port GTA configurado"
    );
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "wgr_quality")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(pairs)
        .append_pair("wgr_quality", profile);
    Ok(url.to_string())
}
fn original_gta_source(source: &mut Source, game: &mut Game) {
    source.kind = SourceKind::RemoteWeb;
    source.location = "https://fiveplay.lol".into();
    source.display_name = "Fiveplay · origem original".into();
    source.allowed_origins = vec![
        "https://fiveplay.lol".into(),
        "https://cdn.fiveplay.lol".into(),
    ];
    source.status = "Origem original · cache persistente".into();
    game.entry_uri = "https://fiveplay.lol/?mode=sandbox".into();
    game.compat_status = "Origem original configurada · verificar ao iniciar".into();
    game.description = "Usa o site/CDN original com perfil persistente. Cache de dados limitado a 2 GiB e cache HTTP do navegador limitado a 256 MiB; saves ficam separados.".into();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn seed_preview_import_and_restart_preserve_user_choices() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let services = Services::new(Paths::at(temp.path().join("data"))?)?;
        services.seed()?;
        let db = Library::open(&services.paths.database)?;
        assert_eq!(db.games()?.len(), 2);
        let fixture = db
            .games()?
            .into_iter()
            .find(|g| g.external_id.as_deref() == Some("wgr-orbit"))
            .context("fixture")?;
        db.remove_game(fixture.id)?;
        drop(db);
        services.seed()?;
        let db = Library::open(&services.paths.database)?;
        assert_eq!(db.games()?.len(), 1);
        drop(db);
        let preview = services
            .preview(ImportRequest {
                kind: SourceKind::LocalWebBundle,
                location: services
                    .paths
                    .root
                    .join("fixtures/orbit")
                    .to_string_lossy()
                    .into_owned(),
                title: "Importado".into(),
                auth: AuthScheme::None,
                secret: None,
                development: false,
                network: false,
                allowed_origins: vec![],
            })
            .await?;
        assert_eq!(
            Library::open(&services.paths.database)?.games()?.len(),
            1,
            "prévia não grava biblioteca"
        );
        services.commit_preview(preview, "Meu jogo")?;
        let db = Library::open(&services.paths.database)?;
        assert_eq!(db.games()?.len(), 2);
        let gta = db
            .games()?
            .into_iter()
            .find(|g| g.external_id.as_deref() == Some("gta-legacy"))
            .context("gta")?;
        let mut source = db.source(gta.source_id)?.context("fonte GTA")?;
        source.allowed_origins.clear();
        db.save_source(&source)?;
        assert!(
            services.launch(gta.id).await.is_err(),
            "GTA sem origens autorizadas nunca inicia streaming"
        );
        Ok(())
    }
    #[test]
    fn cover_is_resized_and_corrupt_images_fail() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let image = image::RgbImage::new(1600, 900);
        let mut buffer = std::io::Cursor::new(Vec::new());
        image.write_to(&mut buffer, image::ImageFormat::Png)?;
        let path = make_thumbnail(buffer.get_ref(), temp.path())?;
        let output = image::open(path)?;
        assert_eq!(output.width(), 640);
        assert!(make_thumbnail(b"not an image", temp.path()).is_err());
        Ok(())
    }
    #[test]
    fn managed_gta_migration_keeps_id_history_favorite_and_profile() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let services = Services::new(Paths::at(temp.path().join("data"))?)?;
        services.seed()?;
        let db = Library::open(&services.paths.database)?;
        let mut game = db
            .games()?
            .into_iter()
            .find(|g| g.external_id.as_deref() == Some("gta-legacy"))
            .context("GTA")?;
        let mut source = db.source(game.source_id)?.context("fonte")?;
        source.kind = SourceKind::GtaLegacyZip;
        source.location = services
            .paths
            .root
            .join("adapters/gta")
            .to_string_lossy()
            .into_owned();
        game.entry_uri = "play.html?nocache=1".into();
        game.favorite = true;
        game.play_count = 7;
        db.save_source(&source)?;
        db.save_game(&game)?;
        services.seed()?;
        let migrated = db.game(game.id)?.context("GTA preservado")?;
        assert!(migrated.favorite);
        assert_eq!(migrated.play_count, 7);
        assert_eq!(migrated.source_id, source.id);
        assert_eq!(migrated.profile.safe_mode, game.profile.safe_mode);
        assert_eq!(migrated.entry_uri, "https://fiveplay.lol/?mode=sandbox");
        assert!(
            db.value::<serde_json::Value>("gta_zip_before_original_origin")?
                .is_some()
        );
        services.seed()?;
        assert_eq!(db.games()?.len(), 2);
        Ok(())
    }
    #[test]
    fn gta_launch_removes_default_cap_and_preserves_explicit_options() -> Result<()> {
        let mut url = url::Url::parse("https://fiveplay.lol/?mode=sandbox")?;
        gta_launch_options(&mut url, true);
        let once = url.clone();
        gta_launch_options(&mut url, true);
        assert_eq!(url, once);
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "fps" && value == "0")
        );
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "low" && value == "1")
        );
        let mut explicit =
            url::Url::parse("https://fiveplay.lol/?fps=30&low=0&wgr_quality=economy")?;
        let before = explicit.clone();
        gta_launch_options(&mut explicit, true);
        assert_eq!(explicit, before);
        let selected = gta_graphics_entry(
            "https://fiveplay.lol/?fps=30&wgr_quality=balanced",
            "economy",
        )?;
        assert!(selected.contains("fps=30") && selected.contains("wgr_quality=economy"));
        assert_eq!(selected.matches("wgr_quality").count(), 1);
        assert!(gta_graphics_entry("https://example.com", "balanced").is_err());
        assert!(gta_graphics_entry("https://fiveplay.lol", "unknown").is_err());
        Ok(())
    }
    #[test]
    fn portable_move_relocates_owned_paths_and_preserves_favorites() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let before = temp.path().join("before");
        let after = temp.path().join("after");
        let services = Services::new(Paths::at(before.clone())?)?;
        services.seed()?;
        let db = Library::open(&services.paths.database)?;
        let mut game = db
            .games()?
            .into_iter()
            .find(|game| game.external_id.as_deref() == Some("wgr-orbit"))
            .context("fixture")?;
        game.favorite = true;
        game.cover_uri = Some(
            before
                .join("covers/test.png")
                .to_string_lossy()
                .into_owned(),
        );
        db.save_game(&game)?;
        drop(db);
        drop(services);
        std::fs::rename(&before, &after)?;
        let services = Services::new(Paths::at(after.clone())?)?;
        services.seed()?;
        let db = Library::open(&services.paths.database)?;
        let relocated = db.game(game.id)?.context("jogo preservado")?;
        assert!(relocated.favorite);
        assert_eq!(
            relocated.local_bundle_path,
            Some(after.join("fixtures/orbit"))
        );
        assert_eq!(
            relocated.cover_uri,
            Some(after.join("covers/test.png").to_string_lossy().into_owned())
        );
        assert_eq!(
            db.source(game.source_id)?.context("fonte")?.location,
            after.join("fixtures/orbit").to_string_lossy()
        );
        Ok(())
    }
}
