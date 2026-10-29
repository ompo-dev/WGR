//! Implemented Emscripten64 kernel capabilities. This is not a complete RAGE
//! adapter: missing imports remain unresolved, never silently stubbed.
use crate::shared::GuestMemory;
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc::SyncSender},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use wasmtime::{Caller, Linker};

pub struct Kernel {
    pub memory: GuestMemory,
    pub boot: Instant,
    pub cores: u32,
    pub native_window: bool,
    pub properties: BTreeMap<String, i32>,
    pub log: Option<SyncSender<String>>,
}
fn error(e: impl std::fmt::Display) -> wasmtime::Error {
    wasmtime::Error::msg(e.to_string())
}
fn string(memory: &GuestMemory, pointer: i64) -> wasmtime::Result<String> {
    memory.c_string(pointer as u64, 16384).map_err(error)
}
pub fn install(
    linker: &mut Linker<Arc<Kernel>>,
    store: &mut wasmtime::Store<Arc<Kernel>>,
) -> wasmtime::Result<()> {
    let memory = store.data().memory.shared();
    linker.define(&mut *store, "env", "memory", memory)?;
    for name in ["wasm_now_ms", "emscripten_get_now"] {
        linker.func_wrap("env", name, |caller: Caller<'_, Arc<Kernel>>| {
            caller.data().boot.elapsed().as_secs_f64() * 1000.
        })?;
    }
    linker.func_wrap("env", "emscripten_date_now", || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.
    })?;
    linker.func_wrap(
        "env",
        "emscripten_num_logical_cores",
        |caller: Caller<'_, Arc<Kernel>>| caller.data().cores.clamp(1, 64) as i32,
    )?;
    linker.func_wrap(
        "env",
        "wasm_has_page_js",
        |caller: Caller<'_, Arc<Kernel>>| i32::from(caller.data().native_window),
    )?;
    linker.func_wrap(
        "env",
        "wasm_module_int_js",
        |caller: Caller<'_, Arc<Kernel>>, name: i64, default: i32| -> wasmtime::Result<i32> {
            let name = string(&caller.data().memory, name)?;
            Ok(caller
                .data()
                .properties
                .get(&name)
                .copied()
                .unwrap_or(default))
        },
    )?;
    for name in ["wasm_print_line_js", "wasm_hang_line_js"] {
        linker.func_wrap(
            "env",
            name,
            |caller: Caller<'_, Arc<Kernel>>, pointer: i64| -> wasmtime::Result<()> {
                let text = string(&caller.data().memory, pointer)?;
                if let Some(log) = &caller.data().log {
                    let _ = log.try_send(text);
                }
                Ok(())
            },
        )?;
    }
    linker.func_wrap(
        "env",
        "emscripten_get_heap_max",
        |caller: Caller<'_, Arc<Kernel>>| {
            caller
                .data()
                .memory
                .shared()
                .ty()
                .maximum()
                .unwrap_or(0)
                .saturating_mul(65536) as i64
        },
    )?;
    linker.func_wrap(
        "env",
        "emscripten_resize_heap",
        |caller: Caller<'_, Arc<Kernel>>, size: i64| -> i32 {
            if size < 0 {
                return 0;
            }
            let memory = caller.data().memory.shared();
            let current = memory.size();
            let pages = (size as u64).div_ceil(65536);
            if pages <= current {
                return 1;
            }
            i32::from(memory.grow(pages - current).is_ok())
        },
    )?;
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "clock_time_get",
        |caller: Caller<'_, Arc<Kernel>>,
         clock: i32,
         _precision: i64,
         pointer: i64|
         -> wasmtime::Result<i32> {
            let nanos = match clock {
                0 => SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                1 => caller.data().boot.elapsed().as_nanos(),
                _ => return Ok(28),
            };
            caller
                .data()
                .memory
                .write(
                    pointer as u64,
                    &(nanos.min(u64::MAX as u128) as u64).to_le_bytes(),
                )
                .map_err(error)?;
            Ok(0)
        },
    )?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binds_real_clock_and_properties_to_memory64_guest() {
        let runtime = crate::NativeRuntime::new().unwrap();
        let memory = GuestMemory::new64(&runtime.engine, 1, 2, 131072).unwrap();
        let mut properties = BTreeMap::new();
        properties.insert("fps".into(), 90);
        memory.write(100, b"fps\0").unwrap();
        let kernel = Arc::new(Kernel {
            memory,
            boot: Instant::now(),
            cores: 4,
            native_window: true,
            properties,
            log: None,
        });
        let mut store = wasmtime::Store::new(&runtime.engine, kernel);
        store.set_epoch_deadline(1);
        let mut linker = Linker::new(&runtime.engine);
        install(&mut linker, &mut store).unwrap();
        let bytes=wat::parse_str("(module (import \"env\" \"memory\" (memory i64 1 2 shared)) (import \"env\" \"wasm_module_int_js\" (func $option (param i64 i32) (result i32))) (func (export \"value\") (result i32) i64.const 100 i32.const 60 call $option))").unwrap();
        let module = runtime.compile(&bytes).unwrap();
        let instance = linker.instantiate(&mut store, &module.module).unwrap();
        let value = instance
            .get_typed_func::<(), i32>(&mut store, "value")
            .unwrap();
        assert_eq!(value.call(&mut store, ()).unwrap(), 90);
    }
}
