//! Hospedagem da engine: memória64 compartilhada, linker dinâmico dos imports
//! Emscripten/WASI/pthreads e chamada de `__main_argc_argv`.
//!
//! O host liga os pontos de entrada que o `wgpu_worker.js` recebe do
//! `browser_main`: `wgpu_start_worker` (ring da GPU), `wasm_input_publish_js`
//! (bloco de input), `wasm_httpfs_io_start_js` (IO) e `wasm_audio_publish_js`
//! (áudio). Nesta etapa os pontos são registrados e sinalizados; a drenagem do
//! ring usa o `Worker` do crate.

use anyhow::{Context, Result, bail, ensure};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Instant;
use wasmtime::{Caller, ExternType, Linker, Module, Store, Val};
use wgr_runtime::BinaryContract;
use wgr_runtime::NativeRuntime;
use wgr_runtime::shared::GuestMemory;

pub struct Host {
    pub memory: GuestMemory,
    pub boot: Instant,
    pub cores: u32,
    pub gpu_started: AtomicBool,
    pub io_started: AtomicBool,
    pub input_block: AtomicU64,
    pub gpu_ring: AtomicU64,
    pub messages: Mutex<Vec<String>>,
}

pub struct EngineHost {
    pub runtime: Arc<NativeRuntime>,
    pub store: Store<Arc<Host>>,
    pub instance: wasmtime::Instance,
    pub memory: GuestMemory,
    pub host: Arc<Host>,
    pub imports: usize,
}

fn zero(value: &mut Val) {
    let replacement = match &*value {
        Val::I32(_) => Val::I32(0),
        Val::I64(_) => Val::I64(0),
        Val::F32(_) => Val::F32(0),
        Val::F64(_) => Val::F64(0),
        Val::V128(_) => Val::V128(0u128.into()),
        other => *other,
    };
    *value = replacement;
}

fn result_i32(results: &mut [Val], value: i32) {
    if let Some(slot) = results.first_mut() {
        *slot = Val::I32(value);
    }
}
fn result_i64(results: &mut [Val], value: i64) {
    if let Some(slot) = results.first_mut() {
        *slot = Val::I64(value);
    }
}
fn result_f64(results: &mut [Val], value: f64) {
    if let Some(slot) = results.first_mut() {
        *slot = Val::F64(value.to_bits());
    }
}

fn argument_i64(params: &[Val], index: usize) -> i64 {
    match params.get(index) {
        Some(Val::I64(value)) => *value,
        Some(Val::I32(value)) => i64::from(*value),
        _ => 0,
    }
}

fn dispatch(
    name: &str,
    caller: &mut Caller<'_, Arc<Host>>,
    params: &[Val],
    results: &mut [Val],
) -> wasmtime::Result<()> {
    for result in results.iter_mut() {
        zero(result);
    }
    let host = caller.data().clone();
    match name {
        "wasm_now_ms" | "emscripten_get_now" => {
            result_f64(results, host.boot.elapsed().as_secs_f64() * 1000.0)
        }
        "emscripten_date_now" => {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                * 1000.0;
            result_f64(results, millis);
        }
        "emscripten_num_logical_cores" => result_i32(results, host.cores.clamp(1, 64) as i32),
        "emscripten_get_heap_max" => {
            let max = host.memory.len() as i64;
            result_i64(results, max);
        }
        "emscripten_resize_heap" => result_i32(results, 1),
        "wasm_print_line_js" | "wasm_hang_line_js" => {
            let pointer = argument_i64(params, 0);
            if let Ok(text) = host.memory.c_string(pointer as u64, 16384)
                && let Ok(mut messages) = host.messages.lock()
                && messages.len() < 512
            {
                messages.push(text);
            }
            result_i32(results, 0);
        }
        "wasm_input_publish_js" => {
            host.input_block
                .store(argument_i64(params, 0) as u64, Ordering::Release);
            result_i32(results, 0);
        }
        "wgpu_start_worker" => {
            host.gpu_ring
                .store(argument_i64(params, 0) as u64, Ordering::Release);
            host.gpu_started.store(true, Ordering::Release);
            result_i32(results, 0);
        }
        "wasm_httpfs_io_start_js" => {
            host.io_started.store(true, Ordering::Release);
            result_i32(results, 0);
        }
        "wasm_audio_publish_js" => result_i32(results, 0),
        _ => {
            for result in results.iter_mut() {
                zero(result);
            }
        }
    }
    Ok(())
}

pub fn load(bytes: &[u8], cores: u32, memory_budget: u64) -> Result<EngineHost> {
    let contract = BinaryContract::inspect(bytes).context("Contrato do game.wasm")?;
    let memory_contract = contract
        .memories
        .first()
        .context("game.wasm sem memória importada")?;
    ensure!(memory_contract.memory64, "game.wasm não usa memória64");
    ensure!(
        memory_contract.shared,
        "game.wasm não usa memória compartilhada"
    );
    let runtime = NativeRuntime::new()?;
    let module = Module::new(runtime.engine(), bytes)
        .map_err(|error| anyhow::anyhow!("Compilação do game.wasm: {error:?}"))?;
    let min_pages = memory_contract.minimum_bytes.div_ceil(65536);
    let max_pages = memory_contract
        .maximum_bytes
        .unwrap_or(min_pages * 4)
        .div_ceil(65536);
    let memory = GuestMemory::new64(runtime.engine(), min_pages, max_pages, memory_budget)
        .context("Memória compartilhada da engine")?;
    let host = Arc::new(Host {
        memory: memory.clone(),
        boot: Instant::now(),
        cores,
        gpu_started: AtomicBool::new(false),
        io_started: AtomicBool::new(false),
        input_block: AtomicU64::new(0),
        gpu_ring: AtomicU64::new(0),
        messages: Mutex::new(Vec::new()),
    });
    let mut store = Store::new(runtime.engine(), host.clone());
    store.set_epoch_deadline(1);
    let import_count = module.imports().count();
    let mut linker = Linker::new(runtime.engine());
    linker
        .define(&mut store, "env", "memory", memory.shared())
        .map_err(|error| anyhow::anyhow!("Import env.memory: {error:?}"))?;
    for import in module.imports() {
        let module_name = import.module();
        if matches!(import.ty(), ExternType::Memory(_)) {
            continue;
        }
        if module_name != "env" && module_name != "wasi_snapshot_preview1" {
            bail!(
                "Import de módulo não suportado: {module_name}.{}",
                import.name()
            );
        }
        match import.ty() {
            ExternType::Func(ty) => {
                let name = import.name().to_string();
                let dispatch_name = name.clone();
                let error_name = name.clone();
                linker
                    .func_new(
                        module_name,
                        &name,
                        ty,
                        move |mut caller, params, results| {
                            dispatch(&dispatch_name, &mut caller, params, results)
                        },
                    )
                    .map_err(|error| {
                        anyhow::anyhow!("Import {module_name}.{error_name}: {error:?}")
                    })?;
            }
            other => bail!(
                "Import {module_name}.{} não é função: {other:?}",
                import.name()
            ),
        }
    }
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|error| anyhow::anyhow!("Instanciação do game.wasm: {error:?}"))?;
    Ok(EngineHost {
        runtime,
        store,
        instance,
        memory,
        host,
        imports: import_count,
    })
}

impl EngineHost {
    /// Monta `argv` na pilha linear e chama o ponto de entrada do port.
    pub fn start(&mut self, args: &[String]) -> Result<i32> {
        let alloc = self
            .instance
            .get_typed_func::<i64, i64>(&mut self.store, "_emscripten_stack_alloc")
            .map_err(|error| anyhow::anyhow!("game.wasm sem _emscripten_stack_alloc: {error:?}"))?;
        let mut pointers = Vec::with_capacity(args.len());
        for argument in args {
            let mut bytes = argument.as_bytes().to_vec();
            bytes.push(0);
            let pointer = alloc
                .call(&mut self.store, bytes.len() as i64)
                .map_err(|error| anyhow::anyhow!("stack alloc: {error:?}"))?
                as u64;
            self.memory.write(pointer, &bytes)?;
            pointers.push(pointer);
        }
        let argv = alloc
            .call(&mut self.store, ((pointers.len() + 1) * 8) as i64)
            .map_err(|error| anyhow::anyhow!("stack alloc argv: {error:?}"))?
            as u64;
        for (index, pointer) in pointers.iter().chain(std::iter::once(&0u64)).enumerate() {
            self.memory
                .write(argv + index as u64 * 8, &pointer.to_le_bytes())?;
        }
        let main = self
            .instance
            .get_func(&mut self.store, "__main_argc_argv")
            .context("game.wasm sem __main_argc_argv")?;
        let mut output = [Val::I32(0)];
        main.call(
            &mut self.store,
            &[Val::I32(args.len() as i32), Val::I64(argv as i64)],
            &mut output,
        )
        .map_err(|error| anyhow::anyhow!("main da engine: {error:?}"))?;
        Ok(match output[0] {
            Val::I32(value) => value,
            _ => 0,
        })
    }

    pub fn messages(&self) -> Vec<String> {
        self.host
            .messages
            .lock()
            .map(|messages| messages.clone())
            .unwrap_or_default()
    }

    pub fn gpu_started(&self) -> bool {
        self.host.gpu_started.load(Ordering::Acquire)
    }

    pub fn io_started(&self) -> bool {
        self.host.io_started.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_non_wasm_file() {
        assert!(load(b"not wasm", 2, 1 << 20).is_err());
    }

    #[test]
    fn val_types_are_zeroed_for_unknown_imports() {
        let mut value = Val::F64(7f64.to_bits());
        zero(&mut value);
        assert!(matches!(value, Val::F64(bits) if bits == 0));
        let mut value = Val::I32(9);
        zero(&mut value);
        assert!(matches!(value, Val::I32(0)));
    }
}
