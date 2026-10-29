use anyhow::Result;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use std::{
    io::{Cursor, Read, Write},
    sync::Arc,
};
use wgr_core::{
    assets::DiskCache,
    gta::{RemoteArchive, ZipEntry, ZipIndex},
    security::NetworkPolicy,
};
use wgr_http::{AssetServer, ServerConfig, parse_range};

async fn local() -> Result<(tempfile::TempDir, AssetServer, reqwest::Client)> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("game");
    std::fs::create_dir_all(root.join("data"))?;
    for (name, bytes) in [
        ("index.html", b"<html>offline</html>".as_slice()),
        ("math.wasm", b"\0asm\x01\0\0\0".as_slice()),
        ("data/test.bin", b"0123456789abcdef".as_slice()),
        ("empty.bin", b"".as_slice()),
    ] {
        std::fs::write(root.join(name), bytes)?;
    }
    let cache = Arc::new(DiskCache::open(temp.path().join("cache"), 1024 * 1024)?);
    let server = AssetServer::start(ServerConfig {
        root,
        archive: None,
        cache,
    })
    .await?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    Ok((temp, server, client))
}

#[tokio::test]
async fn ranges_head_isolation_and_cache_validation() -> Result<()> {
    let (_temp, server, client) = local().await?;
    for (range, status, expected) in [
        (None, 200, "0123456789abcdef"),
        (Some("bytes=0-9"), 206, "0123456789"),
        (Some("bytes=-4"), 206, "cdef"),
        (Some("bytes=12-"), 206, "cdef"),
        (Some("bytes=14-999"), 206, "ef"),
        (Some("bytes=99-"), 416, ""),
        (Some("bytes=-0"), 416, ""),
        (Some("bytes=1-2,4-5"), 416, ""),
        (Some("bytes=18446744073709551616-"), 416, ""),
    ] {
        let mut request = client.get(format!("{}/data/test.bin", server.origin));
        if let Some(r) = range {
            request = request.header("range", r);
        }
        let response = request.send().await?;
        assert_eq!(response.status().as_u16(), status, "{range:?}");
        for (name, value) in [
            ("cross-origin-opener-policy", "same-origin"),
            ("cross-origin-embedder-policy", "require-corp"),
            ("cross-origin-resource-policy", "same-origin"),
        ] {
            assert_eq!(response.headers()[name], value);
        }
        let length: usize = response.headers()["content-length"].to_str()?.parse()?;
        assert_eq!(length, expected.len());
        assert_eq!(response.text().await?, expected);
    }
    let response = client
        .head(format!("{}/math.wasm", server.origin))
        .send()
        .await?;
    assert_eq!(response.headers()["content-type"], "application/wasm");
    assert_eq!(response.headers()["content-length"], "8");
    let etag = response.headers()["etag"].clone();
    assert!(response.bytes().await?.is_empty());
    let cached = client
        .get(format!("{}/math.wasm", server.origin))
        .header("if-none-match", etag)
        .send()
        .await?;
    assert_eq!(cached.status(), 304);
    let empty = client
        .get(format!("{}/empty.bin", server.origin))
        .send()
        .await?;
    assert_eq!(empty.status(), 200);
    assert!(empty.bytes().await?.is_empty());
    let ranged_empty = client
        .get(format!("{}/empty.bin", server.origin))
        .header("range", "bytes=0-1")
        .send()
        .await?;
    assert_eq!(ranged_empty.status(), 416);
    server.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn batch_is_ordered_gzip_and_raw_sha1_cache() -> Result<()> {
    let (_temp, server, client) = local().await?;
    let body = r#"[["test.bin",0,2],["test.bin",12,100]]"#;
    let response = client
        .post(format!("{}/data/batch?gz=1&c=1&v=test-v1", server.origin))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-run-lengths"], "3,4");
    assert_eq!(response.headers()["content-encoding"], "gzip");
    let compressed = response.bytes().await?;
    let mut decoded = String::new();
    flate2::read::GzDecoder::new(compressed.as_ref()).read_to_string(&mut decoded)?;
    assert_eq!(decoded, "012cdef");
    let hash = RemoteArchive::batch_key("test-v1", body.as_bytes());
    let cached = client
        .get(format!("{}/data/batchc/{hash}.bin", server.origin))
        .send()
        .await?;
    assert_eq!(cached.status(), 200);
    assert!(cached.headers().get("content-encoding").is_none());
    assert_eq!(cached.bytes().await?, compressed);
    // The original GTA worker sends JSON as a string without application/json.
    for content_type in [None, Some("text/plain;charset=UTF-8")] {
        let mut request = client
            .post(format!("{}/data/batch", server.origin))
            .body(body);
        if let Some(value) = content_type {
            request = request.header("content-type", value);
        }
        let response = request.send().await?;
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await?, "012cdef");
    }
    for invalid in [
        r#"[["../outside",0,1]]"#,
        r#"[["test.bin",4,1]]"#,
        r#"[["test.bin",-1,4]]"#,
        r#"[["test.bin",0,9999999999999999999999]]"#,
        r#"{}"#,
    ] {
        let response = client
            .post(format!("{}/data/batch", server.origin))
            .header("content-type", "application/json")
            .body(invalid)
            .send()
            .await?;
        assert_eq!(response.status(), 400, "{invalid}");
    }
    server.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn rejects_host_origin_traversal_and_oversized_requests() -> Result<()> {
    let (_temp, server, client) = local().await?;
    for (name, value) in [
        ("host", "evil.example"),
        ("origin", "https://evil.example"),
        ("sec-fetch-site", "cross-site"),
        ("referer", "https://evil.example/"),
    ] {
        let response = client
            .get(format!("{}/index.html", server.origin))
            .header(name, value)
            .send()
            .await?;
        assert_eq!(response.status(), 400);
    }
    for path in [
        "/%2e%2e%2fsecret",
        "/C%3A/Windows",
        "/foo%5Cbar",
        "/data/%00",
    ] {
        let response = client
            .get(format!("{}{path}", server.origin))
            .send()
            .await?;
        assert!(response.status().is_client_error(), "{path}");
    }
    let response = client
        .post(format!("{}/data/batch", server.origin))
        .header("content-type", "application/json")
        .body(vec![b' '; 1024 * 1024 + 1])
        .send()
        .await?;
    assert!(response.status().is_client_error());
    let response = client
        .put(format!("{}/index.html", server.origin))
        .send()
        .await?;
    assert_eq!(response.status(), 405);
    server.shutdown().await;
    Ok(())
}

async fn upstream(State(bytes): State<Arc<Vec<u8>>>, headers: HeaderMap) -> Response {
    let size = bytes.len() as u64;
    let Some(range) = headers.get("range").and_then(|h| h.to_str().ok()) else {
        return (StatusCode::OK, bytes.as_ref().clone()).into_response();
    };
    let Ok((start, end)) = parse_range(range, size) else {
        return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    };
    Response::builder()
        .status(206)
        .header("content-range", format!("bytes {start}-{end}/{size}"))
        .header("content-length", end - start + 1)
        .body(Body::from(bytes[start as usize..=end as usize].to_vec()))
        .expect("response")
}
#[tokio::test]
async fn remote_zip_stored_deflate_offsets_and_concurrent_dedup() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let prefix = "fixture/mirror/playgta5.com/";
    for (path, method, bytes) in [
        (
            "index.html",
            zip::CompressionMethod::Deflated,
            b"<html>fixture</html>".as_slice(),
        ),
        (
            "data/stored.bin",
            zip::CompressionMethod::Stored,
            b"0123456789abcdef".as_slice(),
        ),
        (
            "b/build-test/loader.js",
            zip::CompressionMethod::Deflated,
            b"console.log('legal fixture');".as_slice(),
        ),
    ] {
        writer.start_file(
            format!("{prefix}{path}"),
            zip::write::SimpleFileOptions::default().compression_method(method),
        )?;
        writer.write_all(bytes)?;
    }
    let mut state = 0x12345678u32;
    let large: Vec<u8> = (0..10 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    writer.start_file(
        format!("{prefix}data/large.rpf"),
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )?;
    writer.write_all(&large)?;
    for number in 0..20 {
        writer.start_file(
            format!("{prefix}data/other{number}.bin"),
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )?;
        writer.write_all(&[3u8; 64])?;
    }
    let bytes = writer.finish()?.into_inner();
    let mut zip = zip::ZipArchive::new(Cursor::new(&bytes))?;
    let mut entries = std::collections::BTreeMap::new();
    for i in 0..zip.len() {
        let file = zip.by_index(i)?;
        entries.insert(
            file.name().into(),
            ZipEntry {
                csize: file.compressed_size(),
                usize: file.size(),
                method: if file.compression() == zip::CompressionMethod::Stored {
                    0
                } else {
                    8
                },
                lho: file.header_start(),
                crc: Some(file.crc32()),
            },
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let size = bytes.len() as u64;
    let router = Router::new()
        .route("/fixture.zip", get(upstream))
        .with_state(Arc::new(bytes));
    let upstream =
        tokio::spawn(async move { axum::serve(listener, router).await.expect("upstream") });
    let index = ZipIndex {
        url: format!("{origin}/fixture.zip"),
        size,
        entries,
    };
    let cache = Arc::new(DiskCache::open(
        temp.path().join("cache"),
        16 * 1024 * 1024,
    )?);
    let archive = Arc::new(RemoteArchive::new(
        index,
        NetworkPolicy::for_url(&origin, true)?,
        cache.clone(),
    )?);
    assert_eq!(archive.read("data/stored.bin", 4, 7).await?, b"4567");
    let hits_before = archive.hits.load(std::sync::atomic::Ordering::Relaxed);
    let mut tasks = vec![];
    for _ in 0..8 {
        let archive = archive.clone();
        tasks.push(tokio::spawn(async move {
            archive.read("b/build-test/loader.js", 0, 6).await
        }));
    }
    for task in tasks {
        assert_eq!(task.await??, b"console");
    }
    assert_eq!(archive.misses.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(
        archive.hits.load(std::sync::atomic::Ordering::Relaxed) - hits_before,
        7
    );
    let header_before = archive
        .network_bytes
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(archive.read("data/large.rpf", 0, 31).await?, large[..32]);
    assert!(
        archive
            .network_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
            - header_before
            < 16 * 1024,
        "cabeçalho pequeno não exige baixar/descomprimir 1 MiB"
    );
    let before = archive
        .network_bytes
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        archive.read("data/large.rpf", 800_000, 800_127).await?,
        large[800_000..800_128]
    );
    let after = archive
        .network_bytes
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        after - before < 3 * 1024 * 1024,
        "cabeçalho parcial não baixa o RPF completo"
    );
    assert_eq!(
        archive.read("data/large.rpf", 1000, 1007).await?,
        large[1000..1008]
    );
    assert_eq!(
        archive
            .network_bytes
            .load(std::sync::atomic::Ordering::Relaxed),
        after
    );
    assert_eq!(
        archive.read("data/large.rpf", 1_048_576, 1_048_703).await?,
        large[1_048_576..1_048_704]
    );
    assert_eq!(
        archive
            .read(
                "data/large.rpf",
                10 * 1024 * 1024 - 200,
                10 * 1024 * 1024 - 1
            )
            .await?,
        large[large.len() - 200..]
    );
    for number in 0..20 {
        assert_eq!(
            archive
                .read(&format!("data/other{number}.bin"), 0, 31)
                .await?,
            [3u8; 32]
        );
    }
    let checkpoint_before = archive
        .network_bytes
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        archive.read("data/large.rpf", 7_500_000, 7_500_063).await?,
        large[7_500_000..7_500_064]
    );
    assert!(
        archive
            .network_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
            - checkpoint_before
            < 2 * 1024 * 1024,
        "leitura retroativa deve retomar checkpoint sem baixar o prefixo todo"
    );
    assert!(
        archive
            .checkpoint_hits
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
    assert!(
        cache.stats()?.bytes < 1024 * 1024,
        "RPF permanece somente em memória"
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("cache/staging"))?.count(),
        0,
        "nenhum temporário de RPF é gravado"
    );
    let root = temp.path().join("public");
    std::fs::create_dir(&root)?;
    let server = AssetServer::start(ServerConfig {
        root,
        archive: Some(archive.clone()),
        cache: cache.clone(),
    })
    .await?;
    let response = reqwest::get(format!("{}/b/build-test/loader.js", server.origin)).await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await?, "console.log('legal fixture');");
    let before = cache.stats()?.bytes;
    let body = r#"[["stored.bin",4,7]]"#;
    let response = reqwest::Client::new()
        .post(format!("{}/data/batch?gz=1&c=1&v=memory", server.origin))
        .body(body)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let compressed = response.bytes().await?;
    let hash = RemoteArchive::batch_key("memory", body.as_bytes());
    let response = reqwest::get(format!("{}/data/batchc/{hash}.bin", server.origin)).await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.bytes().await?, compressed);
    assert_eq!(cache.stats()?.bytes, before, "batch remoto não ocupa disco");
    server.shutdown().await;
    upstream.abort();
    Ok(())
}

#[tokio::test]
#[ignore = "Benchmark de rede explícito; baixa somente cabeçalhos do ZIP configurado"]
async fn live_zip_small_reads() -> Result<()> {
    use futures_util::{StreamExt, TryStreamExt, stream};
    use std::sync::atomic::Ordering;
    let index = ZipIndex::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/gta/zip_index.json"),
    )?;
    let prefix = index.prefix()?;
    let paths: Vec<String> = index
        .entries
        .iter()
        .filter(|(name, entry)| name.ends_with(".rpf") && entry.usize >= 32)
        .take(32)
        .map(|(name, _)| name.strip_prefix(&prefix).expect("prefix").to_string())
        .collect();
    let temp = tempfile::tempdir()?;
    let cache = Arc::new(DiskCache::open(temp.path().join("cache"), 1024 * 1024)?);
    let policy = NetworkPolicy {
        allowed_origins: vec![
            "https://archive.org".into(),
            "https://ia800507.us.archive.org".into(),
            "https://ia600507.us.archive.org".into(),
        ],
        development_loopback: false,
    };
    let archive = Arc::new(RemoteArchive::new(index, policy, cache.clone())?);
    let started = std::time::Instant::now();
    let count: Vec<usize> = stream::iter(paths.iter().map(|path| {
        let archive = archive.clone();
        async move { Ok::<_, anyhow::Error>(archive.read(path, 0, 31).await?.len()) }
    }))
    .buffer_unordered(12)
    .try_collect()
    .await?;
    assert!(count.iter().all(|n| *n == 32));
    let report = serde_json::json!({"files": count.len(), "elapsed_ms": started.elapsed().as_millis(), "requested_bytes": archive.requested_bytes.load(Ordering::Relaxed), "archive_bytes": archive.network_bytes.load(Ordering::Relaxed), "inflated_bytes": archive.inflated_bytes.load(Ordering::Relaxed), "disk_cache_bytes": cache.stats()?.bytes});
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/live-zip-headers.json");
    std::fs::write(target, serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    Ok(())
}

#[tokio::test]
async fn loopback_server_releases_listener_on_shutdown() -> Result<()> {
    let (_temp, server, client) = local().await?;
    let url = format!("{}/index.html", server.origin);
    assert_eq!(client.get(&url).send().await?.status(), 200);
    server.shutdown().await;
    assert!(client.get(&url).send().await.is_err());
    Ok(())
}

#[tokio::test]
async fn legacy_contract_matches_original_node_server() -> Result<()> {
    use std::io::BufRead;
    let (temp, rust, client) = local().await?;
    let root = temp.path().join("game");
    let cache = temp.path().join("node-cache");
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/reference/node-baseline.cjs");
    let mut command = std::process::Command::new("node");
    command
        .arg(script)
        .arg(root)
        .arg(cache)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    struct NodeGuard(std::process::Child);
    impl Drop for NodeGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Node sem stdout"))?;
    let mut guard = NodeGuard(child);
    let mut line = String::new();
    std::io::BufReader::new(stdout).read_line(&mut line)?;
    let data: serde_json::Value = serde_json::from_str(&line)?;
    let origin = data["origin"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Node sem origem"))?;
    let mut comparisons = vec![];
    for (path, range, method) in [
        ("/", None, "GET"),
        ("/math.wasm", None, "HEAD"),
        ("/data/test.bin", Some("bytes=0-9"), "GET"),
        ("/data/test.bin", Some("bytes=-4"), "GET"),
        ("/data/test.bin", Some("bytes=12-"), "GET"),
        ("/data/test.bin", Some("bytes=99-"), "GET"),
    ] {
        let mut requests = vec![];
        for base in [&rust.origin, origin] {
            let mut request = client.request(
                reqwest::Method::from_bytes(method.as_bytes())?,
                format!("{base}{path}"),
            );
            if let Some(range) = range {
                request = request.header("range", range);
            }
            requests.push(request.send().await?);
        }
        let node = requests.pop().expect("node");
        let implementation = requests.pop().expect("rust");
        assert_eq!(implementation.status(), node.status(), "{path} {range:?}");
        for header in [
            "content-length",
            "content-range",
            "content-type",
            "accept-ranges",
            "cross-origin-opener-policy",
            "cross-origin-embedder-policy",
            "cross-origin-resource-policy",
        ]
        .into_iter()
        .filter(|h| {
            implementation.status() != 416 || ["content-length", "content-range"].contains(h)
        }) {
            assert_eq!(
                implementation.headers().get(header),
                node.headers().get(header),
                "{header} {path} {range:?}"
            );
        }
        assert_eq!(implementation.bytes().await?, node.bytes().await?);
        comparisons.push(format!("{method} {path} {range:?}"));
    }
    let body = r#"[["test.bin",0,2],["test.bin",12,100]]"#;
    for query in ["", "?gz=1&c=1&v=contract-v1"] {
        let a = client
            .post(format!("{}/data/batch{query}", rust.origin))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        let b = client
            .post(format!("{origin}/data/batch{query}"))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        assert_eq!(a.status(), b.status());
        for h in ["x-run-lengths", "content-encoding", "content-type"] {
            assert_eq!(a.headers().get(h), b.headers().get(h));
        }
        assert_eq!(a.bytes().await?, b.bytes().await?, "batch {query}");
        comparisons.push(format!("POST batch{query}"));
    }
    let hash = RemoteArchive::batch_key("contract-v1", body.as_bytes());
    let path = format!("/data/batchc/{hash}.bin");
    let a = client.get(format!("{}{path}", rust.origin)).send().await?;
    let b = client.get(format!("{origin}{path}")).send().await?;
    assert_eq!(a.status(), b.status());
    assert_eq!(
        a.headers().get("content-encoding"),
        b.headers().get("content-encoding")
    );
    assert_eq!(
        a.headers().get("cache-control"),
        b.headers().get("cache-control")
    );
    assert_eq!(a.bytes().await?, b.bytes().await?);
    comparisons.push("GET batchc SHA-1".into());
    let evidence =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/node-contract.json");
    std::fs::create_dir_all(evidence.parent().expect("artifacts"))?;
    std::fs::write(
        evidence,
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"comparisons":comparisons,"notes":"Cache-Control e ETag de arquivos são políticas próprias do WGR; cabeçalhos de isolamento, Range, tamanho, MIME e bytes comparados. gzip byte-exato nesta fixture."}),
        )?,
    )?;
    let _ = guard.0.kill();
    rust.shutdown().await;
    Ok(())
}
