//! Lista as importações e memórias do game.wasm para orientar o host.
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("uso: dump_imports <game.wasm>"))?;
    let bytes = std::fs::read(&path)?;
    let runtime = wgr_runtime::NativeRuntime::new()?;
    let module = runtime.compile(&bytes)?;
    println!(
        "bytes={} imports={} memories={}",
        bytes.len(),
        module.contract.imports.len(),
        module.contract.memories.len()
    );
    for memory in &module.contract.memories {
        println!(
            "memory memory64={} shared={} min={} max={:?}",
            memory.memory64, memory.shared, memory.minimum_bytes, memory.maximum_bytes
        );
    }
    for import in &module.contract.imports {
        println!(
            "import {}.{} kind={} sig={:?}",
            import.module, import.name, import.kind, import.signature
        );
    }
    Ok(())
}
