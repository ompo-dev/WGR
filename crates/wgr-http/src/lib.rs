//! Servidor de assets restrito a loopback.
pub const RAM_IO_SCRIPT: &str = include_str!("ram_io.js");
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::{StreamExt, stream};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use wgr_core::{
    assets::{DiskCache, sha256},
    gta::{RemoteArchive, read_file},
    security::{rooted_file, safe_relative},
};

#[derive(Clone)]
pub struct ServerConfig {
    pub root: PathBuf,
    pub archive: Option<Arc<RemoteArchive>>,
    pub cache: Arc<DiskCache>,
}
#[derive(Default)]
struct Metrics {
    requests: AtomicU64,
    bytes: AtomicU64,
    latencies: Mutex<std::collections::VecDeque<f64>>,
    logs: Mutex<std::collections::VecDeque<String>>,
}
struct ServerState {
    config: ServerConfig,
    host: String,
    origin: String,
    namespace: String,
    metrics: Metrics,
    stop: CancellationToken,
}
pub struct AssetServer {
    pub origin: String,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl AssetServer {
    pub async fn start(config: ServerConfig) -> Result<Self> {
        Self::start_at(config, None).await
    }
    pub async fn start_at(config: ServerConfig, port: Option<u16>) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port.unwrap_or(0))).await.context("Não foi possível abrir a porta local deste jogo. Outra sessão pode estar usando a porta reservada.")?;
        let address = listener.local_addr()?;
        let origin = format!("http://{address}");
        let host = address.to_string();
        let stop = CancellationToken::new();
        let namespace = config.archive.as_ref().map_or_else(
            || sha256(config.root.to_string_lossy().as_bytes()),
            |a| a.namespace(),
        );
        let state = Arc::new(ServerState {
            config,
            host,
            origin: origin.clone(),
            namespace,
            metrics: Metrics::default(),
            stop: stop.clone(),
        });
        let router = Router::new()
            .fallback(any(handle))
            .layer(DefaultBodyLimit::max(1024 * 1024))
            .with_state(state);
        let cancel = stop.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router)
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
            {
                tracing::error!(%error,"asset_server_stopped");
            }
        });
        Ok(Self {
            origin,
            stop,
            task: Some(task),
        })
    }
    pub async fn shutdown(mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
        }
    }
}
impl Drop for AssetServer {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn handle(State(state): State<Arc<ServerState>>, request: Request) -> Response {
    tracing::debug!(path=%request.uri().path(),method=%request.method(),"asset_request");
    let started = Instant::now();
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let result = tokio::select! {_ = state.stop.cancelled()=>Ok(StatusCode::GONE.into_response()),result=dispatch(state.clone(),request)=>result};
    let mut response = match result {
        Ok(r) => r,
        Err(error) => {
            tracing::warn!(error=%error,"asset_fetch_failed");
            (StatusCode::BAD_REQUEST, error.to_string()).into_response()
        }
    };
    for (key, value) in [
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-embedder-policy", "require-corp"),
        ("cross-origin-resource-policy", "same-origin"),
        ("x-content-type-options", "nosniff"),
    ] {
        response
            .headers_mut()
            .insert(key, HeaderValue::from_static(value));
    }
    if let Ok(mut latencies) = state.metrics.latencies.lock() {
        if latencies.len() >= 512 {
            latencies.pop_front();
        }
        latencies.push_back(started.elapsed().as_secs_f64() * 1000.);
    }
    response
}
async fn dispatch(state: Arc<ServerState>, request: Request) -> Result<Response> {
    ensure!(
        request.headers().get("host").and_then(|h| h.to_str().ok()) == Some(state.host.as_str()),
        "Host inválido"
    );
    if let Some(origin) = request.headers().get("origin") {
        ensure!(origin.to_str()? == state.origin, "Origem não autorizada");
    }
    if let Some(site) = request.headers().get("sec-fetch-site") {
        ensure!(site != "cross-site", "Requisição entre sites bloqueada");
    }
    if let Some(referer) = request.headers().get("referer") {
        let url = wgr_core::security::validate_url(referer.to_str()?, true)?;
        ensure!(
            url.origin().ascii_serialization() == state.origin,
            "Referer não autorizado"
        );
    }
    let method = request.method().clone();
    let head = method == Method::HEAD;
    let path = percent_encoding::percent_decode_str(request.uri().path())
        .decode_utf8()?
        .into_owned();
    let query = request.uri().query().unwrap_or("").to_string();
    if path == "/__wgr_ram_io.js" && method == Method::GET {
        return Ok(Response::builder()
            .header("content-type", "text/javascript")
            .header("cache-control", "no-store")
            .body(Body::from(RAM_IO_SCRIPT))?);
    }
    if path == "/__status" && (method == Method::GET || head) {
        let mut latencies: Vec<f64> = state
            .metrics
            .latencies
            .lock()
            .map_err(|_| anyhow::anyhow!("Métricas ocupadas"))?
            .iter()
            .copied()
            .collect();
        latencies.sort_by(f64::total_cmp);
        let p = |percent: usize| {
            latencies
                .get(latencies.len().saturating_sub(1) * percent / 100)
                .copied()
        };
        let archive = state.config.archive.as_ref();
        let cache = state.config.cache.clone();
        let stats = tokio::task::spawn_blocking(move || cache.stats()).await??;
        let json = serde_json::json!({"requests":state.metrics.requests.load(Ordering::Relaxed),"bytes":state.metrics.bytes.load(Ordering::Relaxed),
            "mode":if archive.is_some(){"remote"}else{"local"},"cache_bytes":stats.bytes,"cache_limit":stats.limit,
            "io_logs":state.metrics.logs.lock().map_err(|_| anyhow::anyhow!("Log ocupado"))?.iter().cloned().collect::<Vec<_>>(),
            "archive_bytes":archive.map_or(0,|a|a.network_bytes.load(Ordering::Relaxed)),"cache_hits":archive.map_or(0,|a|a.hits.load(Ordering::Relaxed)),"cache_misses":archive.map_or(0,|a|a.misses.load(Ordering::Relaxed)),"requested_archive_bytes":archive.map_or(0,|a|a.requested_bytes.load(Ordering::Relaxed)),"inflated_bytes":archive.map_or(0,|a|a.inflated_bytes.load(Ordering::Relaxed)),"checkpoint_hits":archive.map_or(0,|a|a.checkpoint_hits.load(Ordering::Relaxed)),"request_headers_p50_ms":p(50),"request_headers_p95_ms":p(95)});
        return json_response(json, head);
    }
    if path == "/log" && method == Method::POST {
        let body = axum::body::to_bytes(request.into_body(), 4096)
            .await
            .context("Log maior que 4 KiB")?;
        let line: String = String::from_utf8_lossy(&body).chars().take(1024).collect();
        // Diagnostics stay in bounded RAM and never enter the persistent application log.
        let mut logs = state
            .metrics
            .logs
            .lock()
            .map_err(|_| anyhow::anyhow!("Log ocupado"))?;
        if logs.len() >= 32 {
            logs.pop_front();
        }
        logs.push_back(line);
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    if path == "/data/batch" && method == Method::POST {
        ensure!(
            request
                .headers()
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .is_none_or(|v| v.starts_with("application/json") || v.starts_with("text/plain")),
            "Batch exige JSON (application/json ou texto compatível com o loader legado)"
        );
        let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
            .await
            .context("Batch maior que 1 MiB")?;
        return batch(state, &query, body).await;
    }
    if method != Method::GET && !head {
        return Ok((
            StatusCode::METHOD_NOT_ALLOWED,
            [("allow", "GET, HEAD, POST")],
            "Método não permitido",
        )
            .into_response());
    }
    if let Some(hash) = path
        .strip_prefix("/data/batchc/")
        .and_then(|v| v.strip_suffix(".bin"))
    {
        if hash.len() != 40
            || !hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Ok(StatusCode::NOT_FOUND.into_response());
        }
        let key = format!("{}:batch:{hash}", state.namespace);
        if let Some(archive) = &state.config.archive {
            return if let Some(bytes) = archive.cached_batch(hash).await {
                let size = bytes.len() as u64;
                file_response(
                    state,
                    Asset::Memory(Arc::new(bytes)),
                    size,
                    head,
                    request.headers(),
                    "application/octet-stream",
                    "no-store",
                )
                .await
            } else {
                Ok(StatusCode::NOT_FOUND.into_response())
            };
        }
        let cache = state.config.cache.clone();
        let file = tokio::task::spawn_blocking(move || cache.get(&key)).await??;
        return if let Some(path) = file {
            let size = tokio::fs::metadata(&path).await?.len();
            file_response(
                state,
                Asset::File(path),
                size,
                head,
                request.headers(),
                "application/octet-stream",
                "public, max-age=31536000, immutable",
            )
            .await
        } else {
            Ok(StatusCode::NOT_FOUND.into_response())
        };
    }
    let relative = if path == "/" {
        "index.html"
    } else {
        path.strip_prefix('/').context("Caminho inválido")?
    };
    safe_relative(relative)?;
    let target = resolve(&state, relative).await;
    let (mut asset, mut size) = match target {
        Ok(t) => t,
        Err(_) => {
            return Ok(
                (StatusCode::NOT_FOUND, "Arquivo ausente na fonte autorizada").into_response(),
            );
        }
    };
    if state.config.archive.is_some() && relative == "b/8b0b5899ed/io_worker.js" {
        ensure!(
            size > 0 && size <= 256 * 1024,
            "Worker IO excede o limite do adaptador"
        );
        let mut bytes = read(&asset, 0, size - 1).await?;
        ensure!(
            sha256(&bytes) == "d91c89e64f7f280666b5c9d1669ed6d65d66605077264569a49146ed79fe0bae",
            "Versão do worker IO incompatível com o adaptador RAM"
        );
        bytes.extend_from_slice(b"\n");
        bytes.extend_from_slice(include_bytes!("ram_io.js"));
        size = bytes.len() as u64;
        asset = Asset::Memory(Arc::new(bytes));
    }
    let extension = std::path::Path::new(relative)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "wasm" => "application/wasm".into(),
        "html" | "htm" => "text/html; charset=utf-8".into(),
        "js" | "mjs" => "text/javascript; charset=utf-8".into(),
        "json" => "application/json; charset=utf-8".into(),
        "css" => "text/css; charset=utf-8".into(),
        "txt" | "wgsl" => "text/plain; charset=utf-8".into(),
        "webmanifest" => "application/manifest+json".into(),
        _ => mime_guess::from_path(relative)
            .first_or_octet_stream()
            .to_string(),
    };
    file_response(
        state,
        asset,
        size,
        head,
        request.headers(),
        &mime,
        if size > 1024 * 1024 {
            "no-store"
        } else {
            "private, max-age=0, must-revalidate"
        },
    )
    .await
}
#[derive(Clone)]
enum Asset {
    Memory(Arc<Vec<u8>>),
    File(PathBuf),
    Zip {
        archive: Arc<RemoteArchive>,
        path: String,
    },
}
async fn resolve(state: &ServerState, path: &str) -> Result<(Asset, u64)> {
    safe_relative(path)?;
    if let Ok(file) = rooted_file(&state.config.root, path) {
        let size = tokio::fs::metadata(&file).await?.len();
        return Ok((Asset::File(file), size));
    }
    if let Some(archive) = &state.config.archive {
        let (_, entry) = archive.member(path)?;
        return Ok((
            Asset::Zip {
                archive: archive.clone(),
                path: path.into(),
            },
            entry.usize,
        ));
    }
    anyhow::bail!("Arquivo ausente")
}
async fn read(asset: &Asset, start: u64, end: u64) -> Result<Vec<u8>> {
    match asset {
        Asset::Memory(bytes) => Ok(bytes[start as usize..=end as usize].to_vec()),
        Asset::File(path) => read_file(path, start, end).await,
        Asset::Zip { archive, path } => archive.read(path, start, end).await,
    }
}

/// Inclusive single HTTP byte range; unsupported multipart receives 416.
pub fn parse_range(value: &str, size: u64) -> Result<(u64, u64)> {
    ensure!(size > 0, "Arquivo vazio");
    let rest = value
        .trim()
        .strip_prefix("bytes=")
        .context("Unidade inválida")?;
    let (a, b) = rest.split_once('-').context("Intervalo inválido")?;
    ensure!(!a.is_empty() || !b.is_empty(), "Intervalo vazio");
    let (start, end) = if a.is_empty() {
        let suffix: u64 = b.parse()?;
        ensure!(suffix > 0, "Sufixo vazio");
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = a.parse()?;
        let end = if b.is_empty() {
            size - 1
        } else {
            b.parse::<u64>()?.min(size - 1)
        };
        (start, end)
    };
    ensure!(start < size && end >= start, "Intervalo fora do arquivo");
    Ok((start, end))
}
async fn file_response(
    state: Arc<ServerState>,
    asset: Asset,
    size: u64,
    head: bool,
    headers: &HeaderMap,
    mime: &str,
    cache: &str,
) -> Result<Response> {
    let etag = match &asset {
        Asset::Memory(bytes) => format!("\"{}\"", sha256(bytes)),
        Asset::File(path) => {
            let metadata = tokio::fs::metadata(path).await?;
            format!(
                "W/\"{:x}-{:x}\"",
                size,
                metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            )
        }
        Asset::Zip { path, .. } => format!(
            "\"{}\"",
            sha256(format!("{}:{path}:{size}", state.namespace).as_bytes())
        ),
    };
    let mut builder = Response::builder()
        .header("content-type", mime)
        .header("accept-ranges", "bytes")
        .header("etag", &etag)
        .header("cache-control", cache);
    if headers.get("range").is_none()
        && headers.get("if-none-match").and_then(|h| h.to_str().ok()) == Some(etag.as_str())
    {
        return Ok(builder
            .status(StatusCode::NOT_MODIFIED)
            .body(Body::empty())?);
    }
    let partial = headers.get("range");
    let (start, end) = if let Some(range) = partial {
        match range.to_str().ok().and_then(|r| parse_range(r, size).ok()) {
            Some(r) => r,
            None => {
                return Ok(builder
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header("content-range", format!("bytes */{size}"))
                    .header("content-length", "0")
                    .body(Body::empty())?);
            }
        }
    } else {
        (0, size.saturating_sub(1))
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    builder = builder.header("content-length", length);
    if partial.is_some() {
        builder = builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header("content-range", format!("bytes {start}-{end}/{size}"));
    }
    if head || size == 0 {
        return Ok(builder.body(Body::empty())?);
    }
    // Materialize the first chunk before committing headers, so upstream errors remain visible.
    let first_end = end.min(start + 1024 * 1024 - 1);
    let first = read(&asset, start, first_end).await?;
    state
        .metrics
        .bytes
        .fetch_add(first.len() as u64, Ordering::Relaxed);
    let first = stream::once(async { Ok::<Bytes, std::io::Error>(Bytes::from(first)) });
    let rest = stream::try_unfold(
        (asset, first_end + 1, state),
        move |(asset, cursor, state)| async move {
            if cursor > end || state.stop.is_cancelled() {
                return Ok(None);
            }
            let last = end.min(cursor + 1024 * 1024 - 1);
            let bytes = tokio::select! {_ = state.stop.cancelled()=>return Ok(None),result=read(&asset,cursor,last)=>result.map_err(std::io::Error::other)?};
            state
                .metrics
                .bytes
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
            Ok(Some((Bytes::from(bytes), (asset, last + 1, state))))
        },
    );
    Ok(builder.body(Body::from_stream(first.chain(rest)))?)
}
async fn batch(state: Arc<ServerState>, query: &str, body: Bytes) -> Result<Response> {
    let runs: Vec<(String, u64, u64)> =
        serde_json::from_slice(&body).context("JSON de batch inválido")?;
    ensure!(runs.len() <= 2000, "Batch com runs demais");
    let mut total = 0u64;
    let mut targets = vec![];
    for (i, (reference, start, end)) in runs.into_iter().enumerate() {
        let reference = reference.trim_start_matches('/');
        safe_relative(reference)?;
        let (asset, size) = resolve(&state, &format!("data/{reference}")).await?;
        ensure!(
            size > 0 && start < size && start <= end,
            "Run fora do arquivo"
        );
        let end = end.min(size - 1);
        let length = end - start + 1;
        total = total.checked_add(length).context("Batch excedido")?;
        ensure!(total <= 64 * 1024 * 1024, "Batch excede 64 MiB");
        targets.push((i, asset, start, end));
    }
    let work = stream::iter(
        targets
            .into_iter()
            .map(|(i, asset, start, end)| async move {
                let mut part = Vec::new();
                let mut cursor = start;
                while cursor <= end {
                    let last = end.min(cursor + 4 * 1024 * 1024 - 1);
                    part.extend_from_slice(&read(&asset, cursor, last).await?);
                    cursor = last + 1;
                }
                Ok::<_, anyhow::Error>((i, part))
            }),
    )
    .buffer_unordered(4);
    let mut parts = vec![];
    tokio::pin!(work);
    while let Some(item) = work.next().await {
        parts.push(item?);
    }
    parts.sort_by_key(|(i, _)| *i);
    let lengths = parts
        .iter()
        .map(|(_, p)| p.len().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut bytes = Vec::with_capacity(total as usize);
    for (_, part) in parts {
        bytes.extend(part);
    }
    let params: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let gzip = params.get("gz").is_some_and(|v| v == "1");
    if gzip {
        bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            use std::io::Write;
            let mut encoder = flate2::GzBuilder::new()
                .operating_system(if cfg!(windows) { 10 } else { 3 })
                .write(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&bytes)?;
            Ok(encoder.finish()?)
        })
        .await??;
    }
    if gzip
        && params.get("c").is_some_and(|v| v == "1")
        && let Some(version) = params.get("v").filter(|v| !v.is_empty() && v.len() <= 256)
    {
        let hash = RemoteArchive::batch_key(version, &body);
        let key = format!("{}:batch:{hash}", state.namespace);
        if let Some(archive) = &state.config.archive {
            archive.cache_batch(&hash, &bytes).await;
        } else {
            let cache = state.config.cache.clone();
            let copy = bytes.clone();
            tokio::task::spawn_blocking(move || cache.put(&key, &copy)).await??;
        }
    }
    state
        .metrics
        .bytes
        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
    let mut builder = Response::builder()
        .header("content-type", "application/octet-stream")
        .header("content-length", bytes.len())
        .header("x-run-lengths", lengths);
    if gzip {
        builder = builder.header("content-encoding", "gzip");
    }
    Ok(builder.body(Body::from(bytes))?)
}
fn json_response<T: Serialize>(value: T, head: bool) -> Result<Response> {
    let bytes = serde_json::to_vec(&value)?;
    Ok(Response::builder()
        .header("content-type", "application/json")
        .header("content-length", bytes.len())
        .body(if head {
            Body::empty()
        } else {
            Body::from(bytes)
        })?)
}
