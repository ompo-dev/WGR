//! Contratos HTTPFS memory64 com arquivos originais offline.

use std::time::{Duration, Instant};
use wgr_player::engine;
use wgr_player::io::HttpFs;

const FIXTURE: &str = r#"
(module
  (import "env" "memory" (memory i64 2 2 shared))
  (import "env" "wasm_httpfs_manifest_js" (func $manifest (param i64 i32) (result i32)))
  (import "env" "wasm_httpfs_io_start_js" (func $io (param i64)))
  (func (export "manifest") (param i64 i32) (result i32)
    local.get 0 local.get 1 call $manifest)
  (func (export "io") (param i64) local.get 0 call $io)
)
"#;

#[test]
fn missing_manifest_is_an_error_not_an_empty_success() {
    let mut host = engine::load(&wat::parse_str(FIXTURE).unwrap(), 2, 1 << 20).unwrap();
    let manifest = host
        .instance
        .get_typed_func::<(i64, i32), i32>(&mut host.store, "manifest")
        .unwrap();
    assert_eq!(manifest.call(&mut host.store, (0, 0)).unwrap(), -1);
    assert!(manifest.call(&mut host.store, (1024, -1)).is_err());
    let io = host
        .instance
        .get_typed_func::<i64, ()>(&mut host.store, "io")
        .unwrap();
    assert!(io.call(&mut host.store, 64).is_err());
    assert!(!host.io_started());
}

#[test]
fn engine_reads_a_local_file_and_stops_its_io_worker() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = br#"{"version":1,"files":[["original.bin",8]]}"#;
    std::fs::write(directory.path().join("manifest.json"), bytes).unwrap();
    std::fs::write(directory.path().join("original.bin"), b"original").unwrap();
    let filesystem = HttpFs::local(directory.path()).unwrap();
    let mut host = engine::load(&wat::parse_str(FIXTURE).unwrap(), 2, 1 << 20).unwrap();
    host.configure_httpfs(filesystem.clone()).unwrap();
    assert!(host.configure_httpfs(filesystem).is_err());
    let manifest = host
        .instance
        .get_typed_func::<(i64, i32), i32>(&mut host.store, "manifest")
        .unwrap();
    assert_eq!(
        manifest.call(&mut host.store, (1024, 1024)).unwrap(),
        bytes.len() as i32
    );
    assert_eq!(host.memory.read(1024, bytes.len()).unwrap(), bytes);
    for (at, value) in [(68, 1), (132, 0), (136, 2), (144, 2048), (152, 3), (128, 1)] {
        host.memory.store_u32(at, value).unwrap();
    }
    let io = host
        .instance
        .get_typed_func::<i64, ()>(&mut host.store, "io")
        .unwrap();
    io.call(&mut host.store, 64).unwrap();
    io.call(&mut host.store, 64).unwrap();
    assert!(io.call(&mut host.store, 256).is_err());
    assert!(host.io_started());
    let deadline = Instant::now() + Duration::from_secs(3);
    while host.memory.load_u32(128).unwrap() != 2 {
        assert!(
            Instant::now() < deadline,
            "worker HTTPFS não concluiu a leitura"
        );
        host.memory
            .wait32(128, 1, Duration::from_millis(10))
            .unwrap();
    }
    assert_eq!(host.memory.load_u32(156).unwrap(), 3);
    assert_eq!(host.memory.read(2048, 3).unwrap(), b"igi");
    drop(host);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_ranges_use_the_authorized_asset_server() -> anyhow::Result<()> {
    use std::sync::Arc;
    use wgr_core::{
        assets::DiskCache,
        security::{HttpClient, NetworkPolicy},
    };
    use wgr_http::{AssetServer, ServerConfig};
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("data");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("original #%.bin"), b"original")?;
    let server = AssetServer::start(ServerConfig {
        root,
        archive: None,
        cache: Arc::new(DiskCache::open(directory.path().join("cache"), 1024)?),
    })
    .await?;
    let client = HttpClient::new(NetworkPolicy::for_url(&server.origin, true)?);
    let filesystem = HttpFs::remote(
        br#"{"files":[["original #%.bin",8]]}"#.to_vec(),
        &format!("{}/", server.origin),
        client,
        tokio::runtime::Handle::current(),
    )?;
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let runtime = wgr_runtime::NativeRuntime::new()?;
        let memory = wgr_runtime::shared::GuestMemory::new64(runtime.engine(), 1, 1, 65536)?;
        assert_eq!(filesystem.read(&memory, 0, 1, 128, 4)?, 4);
        memory.read(128, 4)
    })
    .await??;
    assert_eq!(bytes, b"rigi");
    server.shutdown().await;
    Ok(())
}
