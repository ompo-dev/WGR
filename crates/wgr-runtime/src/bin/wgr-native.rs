use anyhow::{Context, Result, ensure};
use std::path::Path;
use wgr_runtime::{BinaryContract, NativeRuntime};
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(
        args.len() >= 2,
        "Uso: wgr-native inspect <port.wasm> | step <port.wasm> <valor> <delta>"
    );
    let (contract, bytes) = BinaryContract::read(Path::new(&args[1]))?;
    match args[0].as_str() {
        "inspect" => println!("{}", serde_json::to_string_pretty(&contract)?),
        "start-contract" => {
            use wasmparser::{KnownCustom, Name, Parser, Payload};
            let mut selected = std::collections::BTreeMap::new();
            for payload in Parser::new(0).parse_all(&bytes) {
                if let Payload::CustomSection(section) = payload?
                    && let KnownCustom::Name(names) = section.as_known()
                {
                    for name in names {
                        if let Name::Function(map) = name? {
                            for name in map {
                                let name = name?;
                                if name.name.contains("wasm_start_mode") {
                                    selected.insert(name.index, name.name.to_string());
                                }
                            }
                        }
                    }
                }
            }
            let mut index = contract
                .imports
                .iter()
                .filter(|import| import.kind == "function")
                .count() as u32;
            for payload in Parser::new(0).parse_all(&bytes) {
                if let Payload::CodeSectionEntry(body) = payload? {
                    if let Some(name) = selected.get(&index) {
                        println!("{index}: {name}");
                        for operator in body.get_operators_reader()?.into_iter().take(140) {
                            println!("  {:?}", operator?);
                        }
                    }
                    index += 1;
                }
            }
        }
        "step" => {
            ensure!(args.len() == 4, "Informe valor e delta");
            let runtime = NativeRuntime::new()?;
            let module = runtime.compile(&bytes)?;
            let mut instance = module.instantiate_core(64 * 1024 * 1024)?;
            println!(
                "{}",
                instance.step(
                    args[2].parse().context("Valor inválido")?,
                    args[3].parse().context("Delta inválido")?
                )?
            );
        }
        _ => anyhow::bail!("Comando desconhecido"),
    }
    Ok(())
}
