//! Preflight da GPU física e de compatibilidade, sem usar navegador/Servo.
use anyhow::Result;
use serde_json::{Value, json};

pub async fn run() -> Result<Value> {
    let started = std::time::Instant::now();
    let gpu = wgr_player::gpu::probe::diagnose().await?;
    Ok(json!({
        "kind": "wgr-native-check-v1",
        "architecture": "Rust / Wasmtime / wgpu / GPUI",
        "uses_browser": false,
        "gpu": gpu,
        "gpu_probe_ms": started.elapsed().as_millis(),
        "game_frame_verified": false,
        "note": "Readback prova a GPU; cada jogo ainda precisa de ABI/imports, IO, áudio e entrada nativos."
    }))
}
