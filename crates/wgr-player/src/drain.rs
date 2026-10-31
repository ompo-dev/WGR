//! Drenagem contínua do ring da GPU para o `Worker`. O host da engine só
//! precisa entregar memória, base e tamanho; cada comando é decodificado,
//! executado e confirmado no ring.

use crate::gpu::worker::Worker;
use anyhow::Result;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use wgr_runtime::gpu_protocol::Ring;
use wgr_runtime::shared::GuestMemory;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub commands: u64,
    pub frames: u64,
}

/// Processa até `budget` comandos sem bloquear; devolve o que passou.
pub fn pump(
    ring: &mut Ring,
    worker: &mut Worker,
    memory: &GuestMemory,
    budget: usize,
) -> Result<Stats> {
    let mut stats = Stats::default();
    for _ in 0..budget {
        let Some(command) = ring.next_command()? else {
            break;
        };
        worker.exec(command.opcode, &command.words, command.address, memory)?;
        if command.opcode == 7 {
            worker.submit();
            stats.frames += 1;
        }
        ring.acknowledge(command)?;
        stats.commands += 1;
    }
    Ok(stats)
}

/// Thread contínua: inicializa o ring, drena enquanto `stop` for falso e
/// submete o que restar ao sair.
pub fn spawn(
    memory: GuestMemory,
    base: u64,
    size: u32,
    device: wgpu::Device,
    queue: wgpu::Queue,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Result<Stats>> {
    std::thread::Builder::new()
        .name("wgr-gpu-drain".into())
        .spawn(move || {
            let mut ring = Ring::new(memory.clone(), base, size)?;
            ring.ready()?;
            let mut worker = Worker::new(device, queue);
            let mut total = Stats::default();
            while !stop.load(Ordering::Acquire) {
                let stats = pump(&mut ring, &mut worker, &memory, 4096)?;
                total.commands += stats.commands;
                total.frames += stats.frames;
                if stats.commands == 0 {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            worker.submit();
            Ok(total)
        })
        .expect("thread de drenagem")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgr_runtime::NativeRuntime;

    #[test]
    fn decodes_acknowledges_and_applies_a_ring_command() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("sem adaptador wgpu; teste de drenagem ignorado");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            eprintln!("sem device wgpu; teste de drenagem ignorado");
            return;
        };
        let runtime = NativeRuntime::new().unwrap();
        let memory = GuestMemory::new64(runtime.engine(), 1, 1, 1 << 20).unwrap();
        let mut ring = wgr_runtime::gpu_protocol::Ring::new(memory.clone(), 0, 4096).unwrap();
        ring.ready().unwrap();
        let command = [3u32, 10, 1, 2, 1, 1, 1, 1, 1, 28, 0x8, 1];
        memory.store_u32(64u64, command[0]).unwrap();
        memory.store_u32(68u64, command[1]).unwrap();
        for (index, word) in command[2..].iter().enumerate() {
            memory.store_u32(72u64 + index as u64 * 4, *word).unwrap();
        }
        memory.store_u32(0, 64 + 8 + command[1] * 4).unwrap();
        let mut worker = Worker::new(device, queue);
        let stats = pump(&mut ring, &mut worker, &memory, 8).unwrap();
        assert_eq!(stats.commands, 1);
        assert_eq!(stats.frames, 0);
        assert!(worker.texture(1).is_some());
        assert_eq!(memory.load_u32(4).unwrap(), 112);
    }
}
