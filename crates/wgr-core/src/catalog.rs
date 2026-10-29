use crate::{credentials::Secret, domain::*, security::*};
use anyhow::{Context, Result, ensure};
use reqwest::{
    Method,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::Deserialize;
use std::collections::HashSet;
use url::Url;

#[derive(Deserialize)]
struct Discovery {
    schema_version: String,
    publisher: String,
    catalog_url: String,
    #[serde(default)]
    allowed_asset_origins: Vec<String>,
}
#[derive(Deserialize)]
struct Page {
    schema_version: String,
    items: Vec<Item>,
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
struct Item {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    entry_url: String,
    cover_url: Option<String>,
    runtime: String,
    #[serde(default)]
    requirements: Requirements,
    version: Option<String>,
}

pub async fn sync(source: &Source, secret: Option<&Secret>) -> Result<(Source, Vec<Game>)> {
    let base = validate_url(&source.location, source.development_loopback)?;
    let policy = NetworkPolicy {
        allowed_origins: source.allowed_origins.clone(),
        development_loopback: source.development_loopback,
    };
    let client = HttpClient::new(policy);
    let headers = auth_headers(&source.auth, secret)?;
    let discovery = base.join("/.well-known/wgr.json")?;
    let reply = client
        .request(
            Method::GET,
            discovery.as_str(),
            headers.clone(),
            1024 * 1024,
        )
        .await?;
    ensure!(
        reply.status.is_success(),
        "API discovery retornou HTTP {}. Esperado WGR Catalog API v1.",
        reply.status
    );
    let discovery: Discovery = serde_json::from_slice(&reply.body).context(
        "Esta API não oferece WGR Catalog API v1; use importação de link para uma página de jogo.",
    )?;
    ensure!(
        discovery.schema_version == "1.0" && discovery.publisher.len() <= 256,
        "Versão ou publisher do discovery inválidos"
    );
    ensure!(
        discovery.allowed_asset_origins.len() <= 32,
        "Origens em excesso"
    );
    for origin in discovery.allowed_asset_origins {
        client
            .policy
            .validate(&origin)
            .context("API declara uma origem ainda não autorizada")?;
    }
    let catalog = client.policy.validate(&discovery.catalog_url)?;
    ensure!(
        headers.is_empty() || catalog.origin() == base.origin(),
        "Credenciais só podem ser enviadas para a origem da API"
    );
    let mut cursor: Option<String> = None;
    let mut cursors = HashSet::new();
    let mut ids = HashSet::new();
    let mut games = vec![];
    let mut total_bytes = 0;
    for _ in 0..200 {
        let mut url = catalog.clone();
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", "50");
            if let Some(c) = &cursor {
                query.append_pair("cursor", c);
            }
        }
        let reply = client
            .request(Method::GET, url.as_str(), headers.clone(), 2 * 1024 * 1024)
            .await?;
        total_bytes += reply.body.len();
        ensure!(total_bytes <= 16 * 1024 * 1024, "Catálogo excede 16 MiB");
        ensure!(
            reply.status.is_success(),
            "Catálogo retornou HTTP {}",
            reply.status
        );
        let page: Page = serde_json::from_slice(&reply.body).context("Página WGR inválida")?;
        ensure!(
            page.schema_version == "1.0" && page.items.len() <= 200,
            "Versão ou tamanho da página inválidos"
        );
        for item in page.items {
            ensure!(
                !item.id.is_empty()
                    && item.id.len() <= 256
                    && !item.title.trim().is_empty()
                    && item.title.len() <= 256
                    && item.description.len() <= 8192,
                "Item com metadados inválidos"
            );
            ensure!(
                item.runtime == "webview2",
                "Runtime de catálogo não suportado"
            );
            ensure!(
                item.version.as_ref().is_none_or(|v| v.len() <= 256),
                "Versão de jogo longa demais"
            );
            ensure!(
                item.requirements
                    .min_memory_gib
                    .is_none_or(|v| v.is_finite() && v > 0. && v <= 256.),
                "Memória mínima declarada inválida"
            );
            client.policy.validate(&item.entry_url)?;
            if let Some(cover) = &item.cover_url {
                client.policy.validate(cover)?;
            }
            ensure!(ids.insert(item.id.clone()), "ID duplicado no catálogo");
            let mut game = Game::new(source, &item.title, &item.entry_url);
            game.external_id = Some(item.id);
            game.description = item.description;
            game.cover_uri = item.cover_url;
            game.requirements = item.requirements;
            game.version = item.version;
            if !game.requirements.unknown.is_empty() {
                game.compat_status = "Requisito desconhecido · não testado".into();
            }
            games.push(game);
            ensure!(games.len() <= 10_000, "Catálogo excede 10 mil jogos");
        }
        match page.next_cursor {
            None => {
                let mut source = source.clone();
                source.last_sync_at = Some(now());
                source.status = format!("{} jogos sincronizados", games.len());
                return Ok((source, games));
            }
            Some(next) => {
                ensure!(
                    next.len() <= 2048 && cursors.insert(next.clone()),
                    "Cursor inválido ou paginação em ciclo"
                );
                cursor = Some(next);
            }
        }
    }
    anyhow::bail!("Catálogo excede 200 páginas")
}
pub fn auth_headers(auth: &AuthScheme, secret: Option<&Secret>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    match auth {
        AuthScheme::None => {}
        AuthScheme::Bearer => {
            let s = &secret
                .context("Token não encontrado no cofre do Windows")?
                .0;
            let mut v = HeaderValue::from_str(&format!("Bearer {s}"))?;
            v.set_sensitive(true);
            headers.insert("authorization", v);
        }
        AuthScheme::Header { name } => {
            let name = HeaderName::from_bytes(name.as_bytes())?;
            ensure!(
                ![
                    "host",
                    "cookie",
                    "content-length",
                    "connection",
                    "origin",
                    "referer",
                    "range"
                ]
                .contains(&name.as_str()),
                "Header de autenticação reservado"
            );
            let mut v =
                HeaderValue::from_str(&secret.context("API key não encontrada no cofre")?.0)?;
            v.set_sensitive(true);
            headers.insert(name, v);
        }
        AuthScheme::Basic => {
            use base64::Engine;
            let s = &secret
                .context("Credencial Basic não encontrada no cofre")?
                .0;
            ensure!(s.contains(':'), "Basic exige usuário:senha");
            let mut v = HeaderValue::from_str(&format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(s)
            ))?;
            v.set_sensitive(true);
            headers.insert("authorization", v);
        }
    }
    Ok(headers)
}
pub async fn probe_web(input: &str, development: bool) -> Result<String> {
    let client = HttpClient::new(NetworkPolicy::for_url(input, development)?);
    let reply = client
        .request(Method::GET, input, HeaderMap::new(), 2 * 1024 * 1024)
        .await?;
    ensure!(
        reply.status.is_success(),
        "Página retornou HTTP {}",
        reply.status
    );
    let mime = reply
        .headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    ensure!(
        mime.contains("text/html") || mime.contains("application/xhtml"),
        "A entrada deve ser uma página HTML, recebido {mime}"
    );
    let text = String::from_utf8_lossy(&reply.body);
    let lower = text.to_ascii_lowercase();
    let title = lower
        .find("<title>")
        .and_then(|a| {
            lower[a + 7..]
                .find("</title>")
                .map(|b| text[a + 7..a + 7 + b].trim().to_string())
        })
        .filter(|t| !t.is_empty() && t.len() <= 256)
        .unwrap_or_else(|| {
            Url::parse(input)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "Jogo web".into())
        });
    Ok(title)
}
