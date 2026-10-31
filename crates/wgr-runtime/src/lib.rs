//! Direct binary execution. This crate has no HTML, JavaScript, WebView or URL
//! navigation dependency. Imports are capabilities, not assumed compatibility.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};
use wasmtime::{
    Config, Engine, Instance, Module, OptLevel, Store, StoreLimits, StoreLimitsBuilder,
};

pub mod emscripten;
pub mod gpu_protocol;
pub mod shared;
pub const MAX_BINARY_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub memory64: bool,
    pub shared: bool,
    pub minimum_bytes: u64,
    pub maximum_bytes: Option<u64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Export {
    pub name: String,
    pub kind: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct BinaryContract {
    pub sha256: String,
    pub bytes: usize,
    pub imports: Vec<Import>,
    pub memories: Vec<Memory>,
    pub exports: Vec<Export>,
    pub instantiated: bool,
}
impl BinaryContract {
    pub fn inspect(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_BINARY_BYTES, "Binário excede 256 MiB");
        ensure!(
            bytes.starts_with(b"\0asm\x01\0\0\0"),
            "Formato não suportado pelo host WASM: forneça um módulo WebAssembly core"
        );
        let mut contract = Self {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            bytes: bytes.len(),
            imports: vec![],
            memories: vec![],
            exports: vec![],
            instantiated: false,
        };
        let mut types = vec![];
        for payload in Parser::new(0).parse_all(bytes) {
            match payload? {
                Payload::TypeSection(section) => {
                    for ty in section.into_iter_err_on_gc_types() {
                        types.push(ty?);
                    }
                }
                Payload::ImportSection(section) => {
                    for group in section {
                        for import in group?.into_iter() {
                            let (_, import) = import?;
                            let (kind, signature) = match import.ty {
                                TypeRef::Func(index) => (
                                    "function",
                                    Some(format!(
                                        "{:?} -> {:?}",
                                        types
                                            .get(index as usize)
                                            .context("Tipo de função ausente")?
                                            .params(),
                                        types[index as usize].results()
                                    )),
                                ),
                                TypeRef::Memory(memory) => {
                                    contract.memories.push(memory_contract(memory)?);
                                    ("memory", None)
                                }
                                TypeRef::Table(_) => ("table", None),
                                TypeRef::Global(_) => ("global", None),
                                TypeRef::Tag(_) => ("tag", None),
                                _ => ("unsupported", None),
                            };
                            contract.imports.push(Import {
                                module: import.module.into(),
                                name: import.name.into(),
                                kind: kind.into(),
                                signature,
                            });
                        }
                    }
                }
                Payload::MemorySection(section) => {
                    for memory in section {
                        contract.memories.push(memory_contract(memory?)?);
                    }
                }
                Payload::ExportSection(section) => {
                    for export in section {
                        let export = export?;
                        contract.exports.push(Export {
                            name: export.name.into(),
                            kind: match export.kind {
                                ExternalKind::Func => "function",
                                ExternalKind::Memory => "memory",
                                ExternalKind::Table => "table",
                                ExternalKind::Global => "global",
                                ExternalKind::Tag => "tag",
                                _ => "unsupported",
                            }
                            .into(),
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(contract)
    }
    pub fn read(path: &Path) -> Result<(Self, Vec<u8>)> {
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_BINARY_BYTES as u64,
            "Binário excede 256 MiB"
        );
        let mut bytes = vec![];
        file.take(MAX_BINARY_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok((Self::inspect(&bytes)?, bytes))
    }
}
fn memory_contract(memory: wasmparser::MemoryType) -> Result<Memory> {
    let page = 1u64
        .checked_shl(memory.page_size_log2.unwrap_or(16))
        .context("Tamanho de página inválido")?;
    Ok(Memory {
        memory64: memory.memory64,
        shared: memory.shared,
        minimum_bytes: memory
            .initial
            .checked_mul(page)
            .context("Memória mínima inválida")?,
        maximum_bytes: memory
            .maximum
            .map(|n| n.checked_mul(page).context("Memória máxima inválida"))
            .transpose()?,
    })
}

pub struct NativeRuntime {
    engine: Engine,
    compiler: rayon::ThreadPool,
}
impl NativeRuntime {
    pub fn new() -> Result<Arc<Self>> {
        let mut config = Config::new();
        config
            .wasm_memory64(true)
            .wasm_threads(true)
            .shared_memory(true)
            .epoch_interruption(true)
            .cranelift_opt_level(OptLevel::Speed)
            .parallel_compilation(true)
            .max_wasm_stack(2 * 1024 * 1024);
        let engine =
            Engine::new(&config).map_err(|e| anyhow::anyhow!("Configuração Wasmtime: {e:#}"))?;
        let compiler = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|id| format!("wgr-compiler-{id}"))
            .build()?;
        Ok(Arc::new(Self { engine, compiler }))
    }
    pub fn interrupt(&self) {
        self.engine.increment_epoch();
    }
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
    pub fn compile(&self, bytes: &[u8]) -> Result<NativeModule> {
        let contract = BinaryContract::inspect(bytes)?;
        let module = self
            .compiler
            .install(|| Module::new(&self.engine, bytes))
            .map_err(|e| anyhow::anyhow!("Compilação nativa: {e:#}"))?;
        Ok(NativeModule { contract, module })
    }
}
pub struct NativeModule {
    pub contract: BinaryContract,
    module: Module,
}
pub struct CoreInstance {
    store: Store<StoreLimits>,
    instance: Instance,
}
impl NativeModule {
    /// Core modules need no host ABI. Emscripten/WASI modules must be linked by
    /// their adapter; unknown imports never get zero/no-op replacements.
    pub fn instantiate_core(&self, memory_limit: usize) -> Result<CoreInstance> {
        if !self.contract.imports.is_empty() {
            bail!(
                "O port exige adaptador de host; {} importações ainda não vinculadas: {}",
                self.contract.imports.len(),
                self.contract
                    .imports
                    .iter()
                    .map(|i| format!("{}.{}", i.module, i.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for memory in &self.contract.memories {
            ensure!(
                memory.minimum_bytes <= memory_limit as u64,
                "Memória mínima do port excede o limite da sessão"
            );
        }
        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_limit)
            .table_elements(1_000_000)
            .instances(1)
            .memories(1)
            .tables(4)
            .build();
        let mut store = Store::new(self.module.engine(), limits);
        store.limiter(|limits| limits);
        store.set_epoch_deadline(1);
        let instance = Instance::new(&mut store, &self.module, &[])
            .map_err(|e| anyhow::anyhow!("Instanciação: {e:#}"))?;
        Ok(CoreInstance { store, instance })
    }
}
impl CoreInstance {
    pub fn step(&mut self, value: i32, delta: i32) -> Result<i32> {
        let function = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "step")
            .map_err(|e| anyhow::anyhow!("Contrato step(i32,i32)->i32: {e:#}"))?;
        function
            .call(&mut self.store, (value, delta))
            .map_err(|e| anyhow::anyhow!("Execução nativa: {e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn executes_original_orbit_binary_without_browser() {
        let runtime = NativeRuntime::new().unwrap();
        let module = runtime
            .compile(include_bytes!("../../../fixtures/orbit/math.wasm"))
            .unwrap();
        assert!(module.contract.imports.is_empty());
        let mut instance = module.instantiate_core(16 * 1024 * 1024).unwrap();
        assert_eq!(instance.step(480, 4).unwrap(), 484);
        assert_eq!(instance.step(484, -7).unwrap(), 477);
    }
    #[test]
    fn missing_host_abi_is_explicit() {
        let bytes = wat::parse_str("(module (import \"gpu\" \"present\" (func)))").unwrap();
        let module = NativeRuntime::new().unwrap().compile(&bytes).unwrap();
        assert!(
            module
                .instantiate_core(1048576)
                .err()
                .unwrap()
                .to_string()
                .contains("gpu.present")
        );
    }
    #[test]
    fn inspects_shared_memory64_without_allocating_it() {
        let bytes =
            wat::parse_str("(module (import \"env\" \"memory\" (memory i64 49152 262144 shared)))")
                .unwrap();
        let report = BinaryContract::inspect(&bytes).unwrap();
        assert_eq!(report.memories[0].minimum_bytes, 3221225472);
        assert!(report.memories[0].shared && report.memories[0].memory64);
        assert!(!report.instantiated);
    }
}
