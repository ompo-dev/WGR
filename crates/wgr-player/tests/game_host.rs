//! Teste pesado da hospedagem: carrega o game.wasm real do cache local,
//! instancia com o linker dinâmico e chama `__main_argc_argv`. Requer o
//! arquivo em cache, memória64 e vários GiB de RAM; fica fora do CI.

use std::path::PathBuf;
use std::time::Duration;
use wgr_player::engine;
use wgr_player::loader::Launch;

fn cached_game() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("WGR_GAME_WASM") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let root = std::env::var_os("LOCALAPPDATA")?;
    let path = PathBuf::from(root).join(
        "WGR/cache/objects/11/11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0",
    );
    path.is_file().then_some(path)
}

#[test]
#[ignore = "requer game.wasm em cache, memória64 e vários GiB de RAM"]
fn loads_the_cached_game_wasm_and_reaches_the_gpu_bootstrap() {
    let path =
        cached_game().expect("game.wasm ausente; defina WGR_GAME_WASM para executar este teste");
    let bytes = std::fs::read(&path).expect("ler game.wasm");
    assert_eq!(
        wgr_core::assets::sha256(&bytes),
        "11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0",
        "bundle diferente da build esperada"
    );
    println!("game.wasm: {} bytes ({})", bytes.len(), path.display());
    let io_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mut host = engine::load(&bytes, 4, 6 << 30).expect("hospedar game.wasm");
    if let Ok(root) = std::env::var("WGR_PLAYER_DATA") {
        let filesystem = wgr_player::io::HttpFs::local(std::path::Path::new(&root))
            .expect("bundle HTTPFS local");
        host.configure_httpfs(filesystem).unwrap();
    } else if let Ok(base) = std::env::var("WGR_PLAYER_ASSET_BASE") {
        let manifest = std::env::var("WGR_PLAYER_MANIFEST")
            .expect("defina WGR_PLAYER_MANIFEST com o manifesto autorizado");
        let bytes = std::fs::read(manifest).expect("manifesto autorizado");
        let policy =
            wgr_core::security::NetworkPolicy::for_url(&base, false).expect("origem autorizada");
        let filesystem = wgr_player::io::HttpFs::remote(
            bytes,
            &base,
            wgr_core::security::HttpClient::new(policy),
            io_runtime.handle().clone(),
        )
        .expect("HTTPFS remoto");
        host.configure_httpfs(filesystem).unwrap();
    }
    println!(
        "imports={} memory={} MiB gpu_ring={}",
        host.imports,
        host.memory.len() >> 20,
        host.host
            .gpu_ring
            .load(std::sync::atomic::Ordering::Acquire)
    );
    let runtime = host.runtime.clone();
    let (cancel, deadline) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if deadline.recv_timeout(Duration::from_secs(20)).is_err() {
            runtime.interrupt();
        }
    });
    let result = host.start_with_launch(&Launch::new(1920, 1200));
    let _ = cancel.send(());
    host.runtime.interrupt();
    println!("main: {:?}", result.as_ref().err().map(ToString::to_string));
    println!(
        "gpu_started={} io_started={} audio_started={}",
        host.gpu_started(),
        host.io_started(),
        host.audio_started()
    );
    for line in host.messages().into_iter().take(20) {
        println!("[eng] {line}");
    }
    let _ = watchdog.join();
    if let Err(error) = &result {
        let message = error.to_string();
        assert!(!message.contains("__wasm_call_ctors"), "{message}");
        assert!(!message.contains("type mismatch"), "{message}");
    }
    assert!(
        host.gpu_started(),
        "a engine não chegou ao bootstrap da GPU"
    );
}
