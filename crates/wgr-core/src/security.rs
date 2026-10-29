use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use reqwest::{
    Method,
    header::{HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkPolicy {
    pub allowed_origins: Vec<String>,
    pub development_loopback: bool,
}
impl NetworkPolicy {
    pub fn for_url(input: &str, development_loopback: bool) -> Result<Self> {
        let url = validate_url(input, development_loopback)?;
        Ok(Self {
            allowed_origins: vec![url.origin().ascii_serialization()],
            development_loopback,
        })
    }
    pub fn validate(&self, input: &str) -> Result<Url> {
        let url = validate_url(input, self.development_loopback)?;
        ensure!(
            self.allowed_origins
                .contains(&url.origin().ascii_serialization()),
            "Origem externa não autorizada: {}",
            url.origin().ascii_serialization()
        );
        Ok(url)
    }
}
pub fn validate_url(input: &str, development: bool) -> Result<Url> {
    ensure!(input.len() <= 4096, "URL excede 4096 caracteres");
    let url = Url::parse(input).context("URL inválida")?;
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Credenciais na URL são proibidas"
    );
    let host = url.host_str().context("URL sem servidor")?;
    let loopback = host == "127.0.0.1" || host == "localhost" || host == "[::1]";
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && development && loopback),
        "Use HTTPS. HTTP de loopback exige consentimento de desenvolvimento."
    );
    ensure!(
        !host.ends_with('.')
            && !host.ends_with(".local")
            && !host.ends_with(".internal")
            && !host.ends_with(".localhost"),
        "Servidor local não permitido"
    );
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        ensure!(
            public_ip(ip) || (development && ip.is_loopback()),
            "Endereço de rede privada/reservada não permitido"
        );
    }
    ensure!(
        host != "localhost" || development,
        "Loopback exige consentimento"
    );
    for (k, _) in url.query_pairs() {
        ensure!(
            ![
                "token",
                "access_token",
                "api_key",
                "apikey",
                "password",
                "secret",
                "authorization"
            ]
            .contains(&k.to_ascii_lowercase().as_str()),
            "Guarde credenciais no cofre do Windows, nunca na URL"
        );
    }
    Ok(url)
}
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && !ip.is_multicast()
                && !ip.is_unspecified()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 192 && b == 0 && c == 0)
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            ip.to_ipv4_mapped().map(public_ip_v4).unwrap_or_else(|| {
                (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
            })
        }
    }
}
fn public_ip_v4(ip: std::net::Ipv4Addr) -> bool {
    public_ip(IpAddr::V4(ip))
}

pub fn safe_relative(input: &str) -> Result<PathBuf> {
    ensure!(
        !input.contains(['\\', ':', '\0']) && input.len() <= 2048,
        "Cheminho de recurso inválido"
    );
    let path = Path::new(input);
    ensure!(!path.is_absolute(), "Caminho absoluto proibido");
    for component in path.components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "Traversal de diretório proibido"
        );
        if let Component::Normal(part) = component {
            let name = part.to_string_lossy();
            ensure!(!name.ends_with(['.', ' ']), "Nome Windows ambíguo");
            let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
            ensure!(
                ![
                    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
                    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
                    "LPT8", "LPT9"
                ]
                .contains(&base.as_str()),
                "Nome de dispositivo Windows proibido"
            );
        }
    }
    ensure!(!path.as_os_str().is_empty(), "Caminho vazio");
    Ok(path.to_path_buf())
}
pub fn rooted_file(root: &Path, relative: &str) -> Result<PathBuf> {
    let rel = safe_relative(relative)?;
    let base = root.canonicalize()?;
    let target = base.join(rel).canonicalize().context("Arquivo ausente")?;
    ensure!(
        target.starts_with(&base) && target.is_file(),
        "Arquivo fora da pasta autorizada"
    );
    Ok(target)
}
pub fn remove_owned_directory(root: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    ensure!(
        !std::fs::symlink_metadata(target)?.file_type().is_symlink(),
        "Limpeza não segue links simbólicos"
    );
    let base = root.canonicalize()?;
    let resolved = target.canonicalize()?;
    ensure!(
        resolved != base && resolved.starts_with(&base),
        "Limpeza fora da pasta própria bloqueada"
    );
    std::fs::remove_dir_all(resolved)?;
    Ok(())
}

#[derive(Clone)]
pub struct HttpClient {
    pub policy: NetworkPolicy,
    clients: Arc<Mutex<PinnedClients>>,
}
type PinnedClients = BTreeMap<String, (Vec<SocketAddr>, reqwest::Client)>;
pub struct HttpReply {
    pub status: reqwest::StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub final_url: Url,
}
impl HttpClient {
    pub fn new(policy: NetworkPolicy) -> Self {
        Self {
            policy,
            clients: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    pub async fn request(
        &self,
        method: Method,
        input: &str,
        headers: HeaderMap,
        limit: usize,
    ) -> Result<HttpReply> {
        self.request_body(method, input, headers, limit, Vec::new())
            .await
    }
    pub async fn request_body(
        &self,
        method: Method,
        input: &str,
        headers: HeaderMap,
        limit: usize,
        body: Vec<u8>,
    ) -> Result<HttpReply> {
        ensure!(
            body.len() <= 1024 * 1024,
            "Corpo de requisição excede 1 MiB"
        );
        let mut url = self.policy.validate(input)?;
        for _ in 0..=5 {
            let host = url
                .host_str()
                .context("Servidor ausente")?
                .trim_matches(['[', ']']);
            let mut addresses: Vec<SocketAddr> = tokio::time::timeout(
                Duration::from_secs(8),
                tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(443))),
            )
            .await
            .context("Timeout DNS")??
            .collect();
            addresses.sort_unstable();
            addresses.dedup();
            ensure!(!addresses.is_empty(), "DNS sem endereços");
            validate_addresses(&addresses, self.policy.development_loopback)?;
            // Pin the verified DNS answer to this connection; redirects re-enter validation.
            let key = url.origin().ascii_serialization();
            let client = {
                let mut clients = self
                    .clients
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Pool HTTP ocupado"))?;
                if let Some((_, client)) =
                    clients.get(&key).filter(|(pinned, _)| *pinned == addresses)
                {
                    client.clone()
                } else {
                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .tcp_nodelay(true)
                        .http2_adaptive_window(true)
                        .pool_max_idle_per_host(32)
                        .redirect(reqwest::redirect::Policy::none())
                        .connect_timeout(Duration::from_secs(10))
                        .timeout(Duration::from_secs(45))
                        .resolve_to_addrs(host, &addresses)
                        .build()?;
                    if clients.len() >= 32 {
                        clients.clear();
                    }
                    clients.insert(key, (addresses, client.clone()));
                    client
                }
            };
            let response = client
                .request(method.clone(), url.clone())
                .headers(headers.clone())
                .body(body.clone())
                .header("User-Agent", "WGR/0.1")
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Falha HTTP: {}", e.without_url()))?;
            if response.status().is_redirection()
                && response.status() != reqwest::StatusCode::NOT_MODIFIED
            {
                let location = response
                    .headers()
                    .get("location")
                    .context("Redirecionamento sem destino")?
                    .to_str()?;
                let next = url.join(location)?;
                self.policy.validate(next.as_str())?;
                ensure!(
                    !headers.values().any(|value| value.is_sensitive())
                        || next.origin() == url.origin(),
                    "Redirecionamento entre origens com headers não permitido"
                );
                ensure!(
                    method == Method::GET
                        || method == Method::HEAD
                        || matches!(response.status().as_u16(), 307 | 308),
                    "Redirecionamento de POST deve preservar explicitamente o método (307/308)"
                );
                url = next;
                continue;
            }
            if let Some(size) = response.content_length() {
                ensure!(
                    size <= limit as u64 || method == Method::HEAD,
                    "Resposta excede o limite de {limit} bytes"
                );
            }
            let status = response.status();
            let reply_headers = response.headers().clone();
            let mut body = vec![];
            if method != Method::HEAD {
                let mut stream = response.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|e| {
                        anyhow::anyhow!("Falha ao ler resposta: {}", e.without_url())
                    })?;
                    ensure!(
                        body.len()
                            .checked_add(chunk.len())
                            .is_some_and(|n| n <= limit),
                        "Resposta excede limite de segurança"
                    );
                    body.extend_from_slice(&chunk);
                }
            }
            return Ok(HttpReply {
                status,
                headers: reply_headers,
                body,
                final_url: url,
            });
        }
        bail!("Redirecionamentos em excesso")
    }
    pub async fn get(&self, url: &str, limit: usize) -> Result<HttpReply> {
        self.request(Method::GET, url, HeaderMap::new(), limit)
            .await
    }
    /// Whole compressible files, with explicit bounded decoding. Range requests
    /// retain identity encoding and their exact Content-Range validation.
    pub async fn get_gzip(&self, url: &str, limit: usize) -> Result<HttpReply> {
        let mut headers = HeaderMap::new();
        headers.insert("accept-encoding", HeaderValue::from_static("gzip"));
        let mut reply = self
            .request(Method::GET, url, headers, limit.saturating_add(4096))
            .await?;
        if reply
            .headers
            .get("content-encoding")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.eq_ignore_ascii_case("gzip"))
        {
            reply.body = decode_gzip_bounded(&reply.body, limit)?;
            reply.headers.remove("content-encoding");
            reply.headers.remove("content-length");
        }
        ensure!(
            reply.body.len() <= limit,
            "Resposta descomprimida excede o limite"
        );
        Ok(reply)
    }
    pub async fn range(&self, url: &str, start: u64, end: u64, total: u64) -> Result<Vec<u8>> {
        ensure!(
            end >= start && end < total && end - start < 8 * 1024 * 1024,
            "Intervalo remoto inválido ou maior que 8 MiB"
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "range",
            HeaderValue::from_str(&format!("bytes={start}-{end}"))?,
        );
        headers.insert("accept-encoding", HeaderValue::from_static("identity"));
        let expected = format!("bytes {start}-{end}/{total}");
        let mut last = None;
        for attempt in 0..3 {
            match self
                .request(
                    Method::GET,
                    url,
                    headers.clone(),
                    (end - start + 1) as usize,
                )
                .await
            {
                Ok(reply) => {
                    if reply.status.as_u16() == 429 || reply.status.is_server_error() {
                        let wait = reply
                            .headers
                            .get("retry-after")
                            .and_then(|h| h.to_str().ok())
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(1 << attempt)
                            .min(10);
                        last = Some(anyhow::anyhow!("Upstream HTTP {}", reply.status));
                        tokio::time::sleep(Duration::from_millis(
                            wait * 1000 + u64::from(uuid::Uuid::new_v4().as_bytes()[0]),
                        ))
                        .await;
                        continue;
                    }
                    ensure!(
                        reply.status.as_u16() == 206,
                        "Upstream deve responder 206; recebido {}",
                        reply.status
                    );
                    ensure!(
                        reply
                            .headers
                            .get("content-range")
                            .and_then(|v| v.to_str().ok())
                            == Some(expected.as_str()),
                        "Content-Range divergente"
                    );
                    ensure!(
                        reply.body.len() as u64 == end - start + 1,
                        "Intervalo remoto truncado"
                    );
                    return Ok(reply.body);
                }
                Err(error) => {
                    last = Some(error);
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                    }
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("Falha na leitura remota")))
    }
}
pub fn validate_addresses(addresses: &[SocketAddr], development: bool) -> Result<()> {
    ensure!(
        addresses
            .iter()
            .all(|a| public_ip(a.ip()) || (development && a.ip().is_loopback())),
        "DNS resolveu para rede privada/reservada; conexão bloqueada"
    );
    Ok(())
}

pub fn decode_gzip_bounded(bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut output = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut output)?;
    ensure!(output.len() <= limit, "Resposta gzip excede o limite");
    Ok(output)
}

#[cfg(test)]
mod tests {
    #[test]
    fn gzip_decoding_enforces_expanded_limit_and_integrity() {
        use std::io::Write;
        let source = vec![b'a'; 64 * 1024];
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&source).unwrap();
        let gzip = encoder.finish().unwrap();
        assert_eq!(
            super::decode_gzip_bounded(&gzip, source.len()).unwrap(),
            source
        );
        assert!(super::decode_gzip_bounded(&gzip, 1024).is_err());
        assert!(super::decode_gzip_bounded(&gzip[..gzip.len() - 4], source.len()).is_err());
    }
    use super::*;
    #[test]
    fn rejects_ssrf_and_ambiguous_paths() {
        for url in [
            "file:///C:/Windows/x",
            "javascript:alert(1)",
            "https://u:p@example.com",
            "http://example.com",
            "https://169.254.169.254/x",
            "https://[::ffff:127.0.0.1]/",
            "https://10.0.0.1/",
            "https://example.com/?token=x",
        ] {
            assert!(validate_url(url, false).is_err(), "{url}");
        }
        assert!(validate_url("http://127.0.0.1:1234", true).is_ok());
        assert!(validate_url("https://example.com/play", false).is_ok());
        for path in [
            "../x",
            "/absolute",
            "C:/secret",
            "foo\\bar",
            "x:ads",
            "CON",
            "folder./x",
            "a/NUL.txt",
        ] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        let mixed = [
            "8.8.8.8:443".parse().expect("ip"),
            "127.0.0.1:443".parse().expect("ip"),
        ];
        assert!(validate_addresses(&mixed, false).is_err());
    }
}
