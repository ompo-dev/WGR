//! Hospedagem da engine: memória64 compartilhada, linker dinâmico, pthreads
//! (uma instância Wasmtime por thread sobre a mesma memória) e chamada de
//! `__main_argc_argv`.

use anyhow::{Context, Result, bail, ensure};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Instant;
use wasmtime::{Caller, Engine, ExternType, Linker, Module, Store, Val, ValType};
use wgr_runtime::BinaryContract;
use wgr_runtime::NativeRuntime;
use wgr_runtime::shared::GuestMemory;

pub struct Shared {
    pub module: Module,
    pub linker: Arc<Linker<Arc<Host>>>,
    pub engine: Engine,
}

pub struct Host {
    pub memory: GuestMemory,
    pub boot: Instant,
    pub cores: u32,
    pub gpu_started: AtomicBool,
    pub io_started: AtomicBool,
    pub gpu_ring: AtomicU64,
    pub gpu_ring_size: AtomicU64,
    pub input_block: AtomicU64,
    pub messages: Mutex<Vec<String>>,
    pub shared: OnceLock<Arc<Shared>>,
}

pub struct EngineHost {
    pub runtime: Arc<NativeRuntime>,
    pub shared: Arc<Shared>,
    pub store: Store<Arc<Host>>,
    pub instance: wasmtime::Instance,
    pub memory: GuestMemory,
    pub host: Arc<Host>,
    pub imports: usize,
}

fn default_val(ty: &ValType) -> Val {
    match ty {
        ValType::I32 => Val::I32(0),
        ValType::I64 => Val::I64(0),
        ValType::F32 => Val::F32(0),
        ValType::F64 => Val::F64(0),
        ValType::V128 => Val::V128(0u128.into()),
        ValType::Ref(_) => Val::null_any_ref(),
    }
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

fn argument_i32(params: &[Val], index: usize) -> i32 {
    match params.get(index) {
        Some(Val::I32(value)) => *value,
        Some(Val::I64(value)) => *value as i32,
        _ => 0,
    }
}

fn argument_i64(params: &[Val], index: usize) -> i64 {
    match params.get(index) {
        Some(Val::I64(value)) => *value,
        Some(Val::I32(value)) => i64::from(*value),
        _ => 0,
    }
}

fn export_call(
    store: &mut Store<Arc<Host>>,
    instance: &wasmtime::Instance,
    name: &str,
    params: &[Val],
) -> Result<Vec<Val>> {
    let function = instance
        .get_func(&mut *store, name)
        .with_context(|| format!("export {name}"))?;
    let mut results = vec![Val::I64(0); function.ty(&*store).results().len()];
    function
        .call(&mut *store, params, &mut results)
        .map_err(|error| anyhow::anyhow!("{name}: {error:?}"))?;
    Ok(results)
}

fn call_table(
    store: &mut Store<Arc<Host>>,
    instance: &wasmtime::Instance,
    index: u64,
    argument: u64,
) -> Result<()> {
    let table = instance
        .get_table(&mut *store, "__indirect_function_table")
        .context("tabela de funções")?;
    let function = table
        .get(&mut *store, index)
        .and_then(|value| value.as_func().flatten().cloned())
        .context("índice de função nulo")?;
    function
        .call(&mut *store, &[Val::I64(argument as i64)], &mut [])
        .map_err(|error| anyhow::anyhow!("thread: {error:?}"))?;
    Ok(())
}

fn run_pthread(host: Arc<Host>, shared: Arc<Shared>, ptr: u64, start: u64, arg: u64) -> Result<()> {
    let mut store = Store::new(&shared.engine, host.clone());
    store.set_epoch_deadline(1);
    let instance = shared
        .linker
        .instantiate(&mut store, &shared.module)
        .map_err(|error| anyhow::anyhow!("instância da thread: {error:?}"))?;
    let high = host
        .memory
        .read(ptr + 80, 8)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .unwrap_or(0);
    let size = host
        .memory
        .read(ptr + 88, 8)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .unwrap_or(0);
    let _ = export_call(
        &mut store,
        &instance,
        "emscripten_stack_set_limits",
        &[
            Val::I64(high as i64),
            Val::I64(high.saturating_sub(size) as i64),
        ],
    );
    let _ = export_call(
        &mut store,
        &instance,
        "_emscripten_stack_restore",
        &[Val::I64(high as i64)],
    );
    export_call(
        &mut store,
        &instance,
        "_emscripten_thread_init",
        &[
            Val::I64(ptr as i64),
            Val::I32(0),
            Val::I32(0),
            Val::I32(1),
            Val::I64(0),
            Val::I32(0),
        ],
    )?;
    call_table(&mut store, &instance, start, arg)
}

fn spawn_pthread(host: &Arc<Host>, ptr: u64, start: u64, arg: u64) -> i32 {
    let Some(shared) = host.shared.get().cloned() else {
        return 6;
    };
    let name = host
        .memory
        .c_string(arg.wrapping_add(24), 64)
        .ok()
        .filter(|name| !name.is_empty() && name.len() <= 40)
        .unwrap_or_else(|| format!("pthread-{ptr:x}"));
    let host = host.clone();
    let spawned = std::thread::Builder::new()
        .name(name)
        .stack_size(16 << 20)
        .spawn(move || {
            if let Err(error) = run_pthread(host, shared, ptr, start, arg) {
                eprintln!("[player] pthread 0x{ptr:x}: {error:#}");
            }
        });
    if spawned.is_ok() { 0 } else { 6 }
}

fn invoke(
    caller: &mut Caller<'_, Arc<Host>>,
    params: &[Val],
    results: &mut [Val],
) -> wasmtime::Result<()> {
    let index = argument_i64(params, 0) as u64;
    let table = caller
        .get_export("__indirect_function_table")
        .and_then(|export| export.into_table())
        .ok_or_else(|| wasmtime::Error::msg("sem tabela de funções"))?;
    let function = table
        .get(&mut *caller, index)
        .and_then(|value| value.as_func().flatten().cloned())
        .ok_or_else(|| wasmtime::Error::msg(format!("invoke: índice nulo {index}")))?;
    function.call(&mut *caller, &params[1..], results)
}

fn trace_imports() -> bool {
    static TRACE: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("WGR_TRACE_IMPORTS").as_deref() == Ok("1"));
    *TRACE
}

fn dispatch(
    name: &str,
    result_types: &[ValType],
    caller: &mut Caller<'_, Arc<Host>>,
    params: &[Val],
    results: &mut [Val],
) -> wasmtime::Result<()> {
    for (result, ty) in results.iter_mut().zip(result_types) {
        *result = default_val(ty);
    }
    if trace_imports() {
        eprintln!("[import] {name} params={params:?}");
    }
    if name.starts_with("invoke_") {
        let called = invoke(caller, params, results);
        if trace_imports() {
            eprintln!("[import] {name} -> {results:?}");
        }
        return called;
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
        "emscripten_get_heap_max" => result_i64(results, host.memory.len() as i64),
        "emscripten_resize_heap" => {
            let size = argument_i64(params, 0);
            if size <= 0 {
                result_i32(results, 0);
            } else {
                let memory = host.memory.shared();
                let current = memory.size();
                let pages = (size as u64).div_ceil(65536);
                let grown = pages <= current || memory.grow(pages - current).is_ok();
                result_i32(results, i32::from(grown));
            }
        }
        "fd_write" => {
            let iovs = argument_i64(params, 1) as u64;
            let count = argument_i32(params, 2).max(0) as usize;
            let written = argument_i64(params, 3) as u64;
            let mut total = 0u64;
            for index in 0..count {
                let at = iovs + index as u64 * 16;
                let pointer = host
                    .memory
                    .read(at, 8)
                    .ok()
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u64::from_le_bytes)
                    .unwrap_or(0);
                let length = host
                    .memory
                    .read(at + 8, 8)
                    .ok()
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u64::from_le_bytes)
                    .unwrap_or(0);
                if let Ok(bytes) = host.memory.read(pointer, length as usize) {
                    eprint!("{}", String::from_utf8_lossy(&bytes));
                    total += length;
                }
            }
            let _ = host.memory.write(written, &total.to_le_bytes());
            result_i32(results, 0);
        }
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
            host.gpu_ring_size
                .store(argument_i64(params, 1) as u64, Ordering::Release);
            host.gpu_started.store(true, Ordering::Release);
            result_i32(results, 0);
        }
        "wasm_httpfs_io_start_js" => {
            host.io_started.store(true, Ordering::Release);
            result_i32(results, 0);
        }
        "__pthread_create_js" => {
            let pointer = argument_i64(params, 0) as u64;
            let start = argument_i64(params, 2) as u64;
            let argument = argument_i64(params, 3) as u64;
            result_i32(results, spawn_pthread(&host, pointer, start, argument));
        }
        _ => {
            for (result, ty) in results.iter_mut().zip(result_types) {
                *result = default_val(ty);
            }
        }
    }
    if trace_imports() {
        eprintln!("[import] {name} -> {results:?}");
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
    let engine = runtime.engine().clone();
    let module = Module::new(&engine, bytes)
        .map_err(|error| anyhow::anyhow!("Compilação do game.wasm: {error:?}"))?;
    let min_pages = memory_contract.minimum_bytes.div_ceil(65536);
    let max_pages = memory_contract
        .maximum_bytes
        .unwrap_or(min_pages * 4)
        .div_ceil(65536);
    let memory = GuestMemory::new64(&engine, min_pages, max_pages, memory_budget)
        .context("Memória compartilhada da engine")?;
    let host = Arc::new(Host {
        memory: memory.clone(),
        boot: Instant::now(),
        cores,
        gpu_started: AtomicBool::new(false),
        io_started: AtomicBool::new(false),
        gpu_ring: AtomicU64::new(0),
        gpu_ring_size: AtomicU64::new(0),
        input_block: AtomicU64::new(0),
        messages: Mutex::new(Vec::new()),
        shared: OnceLock::new(),
    });
    let mut store = Store::new(&engine, host.clone());
    store.set_epoch_deadline(1);
    let import_count = module.imports().count();
    let mut linker: Linker<Arc<Host>> = Linker::new(&engine);
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
                if trace_imports() {
                    let params: Vec<_> = ty.params().collect();
                    let results: Vec<_> = ty.results().collect();
                    eprintln!(
                        "[define] {module_name}.{name} params={params:?} results={results:?}"
                    );
                }
                let dispatch_name = name.clone();
                let error_name = name.clone();
                let result_types: Vec<ValType> = ty.results().collect();
                linker
                    .func_new(
                        module_name,
                        &name,
                        ty,
                        move |mut caller, params, results| {
                            dispatch(&dispatch_name, &result_types, &mut caller, params, results)
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
    let linker = Arc::new(linker);
    let shared = Arc::new(Shared {
        module: module.clone(),
        linker: linker.clone(),
        engine: engine.clone(),
    });
    host.shared
        .set(shared.clone())
        .map_err(|_| anyhow::anyhow!("shared já definido"))?;
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|error| anyhow::anyhow!("Instanciação do game.wasm: {error:?}"))?;
    Ok(EngineHost {
        runtime,
        shared,
        store,
        instance,
        memory,
        host,
        imports: import_count,
    })
}

impl EngineHost {
    pub fn start(&mut self, args: &[String]) -> Result<i32> {
        // Emscripten initializes a tabela de funções e os dados no ctors; sem
        // ele, call_indirect/invoke caem em funcref nulo.
        for name in ["__wasm_apply_data_relocs", "__wasm_init_memory"] {
            if let Ok(init) = self
                .instance
                .get_typed_func::<(), ()>(&mut self.store, name)
            {
                init.call(&mut self.store, ())
                    .map_err(|error| anyhow::anyhow!("{name}: {error:?}"))?;
            }
        }
        if let Ok(tls) = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "_emscripten_tls_init")
        {
            tls.call(&mut self.store, ())
                .map_err(|error| anyhow::anyhow!("_emscripten_tls_init: {error:?}"))?;
        }
        if let Ok(ctors) = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "__wasm_call_ctors")
        {
            ctors
                .call(&mut self.store, ())
                .map_err(|error| anyhow::anyhow!("__wasm_call_ctors: {error:?}"))?;
        }
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
    fn result_defaults_follow_the_declared_type() {
        assert!(matches!(default_val(&ValType::I32), Val::I32(0)));
        assert!(matches!(
            default_val(&ValType::F64),
            Val::F64(bits) if bits == 0
        ));
        assert!(matches!(default_val(&ValType::I64), Val::I64(0)));
    }
}
