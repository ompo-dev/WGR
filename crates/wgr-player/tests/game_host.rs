//! Teste pesado da hospedagem: carrega o game.wasm real do cache local,
//! instancia com o linker dinâmico e chama `__main_argc_argv`. Requer o
//! arquivo em cache, memória64 e vários GiB de RAM; fica fora do CI.

use std::path::PathBuf;
use std::time::Duration;
use wgr_player::engine;
use wgr_player::loader::{Launch, engine_args};

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
    let Some(path) = cached_game() else {
        eprintln!("game.wasm ausente; teste ignorado");
        return;
    };
    let bytes = std::fs::read(&path).expect("ler game.wasm");
    println!("game.wasm: {} bytes ({})", bytes.len(), path.display());
    let mut host = engine::load(&bytes, 4, 6 << 30).expect("hospedar game.wasm");
    println!(
        "imports={} memory={} MiB gpu_ring={}",
        host.imports,
        host.memory.len() >> 20,
        host.host
            .gpu_ring
            .load(std::sync::atomic::Ordering::Acquire)
    );
    let runtime = host.runtime.clone();
    let watchdog = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        runtime.interrupt();
    });
    let args = engine_args(&Launch::new(1920, 1200));
    let result = host.start(&args);
    println!("main: {:?}", result.as_ref().err().map(ToString::to_string));
    println!(
        "gpu_started={} io_started={}",
        host.gpu_started(),
        host.io_started()
    );
    for line in host.messages().into_iter().take(20) {
        println!("[eng] {line}");
    }
    let _ = watchdog.join();
    assert!(
        host.gpu_started(),
        "a engine não chegou ao bootstrap da GPU"
    );
}
