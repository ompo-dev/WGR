mod common;

use anyhow::Result;
use common::read_pixel;
use wgpu::{DeviceDescriptor, Instance, RequestAdapterOptions};
use wgr_player::gpu::worker::Worker;
use wgr_runtime::NativeRuntime;
use wgr_runtime::shared::GuestMemory;

const VS: &str = r#"
struct VOut { @builtin(position) pos : vec4<f32>, }
@vertex fn main(@builtin(vertex_index) i : u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(-1.0, 3.0), vec2<f32>(3.0, -1.0));
  var o : VOut; o.pos = vec4<f32>(p[i], 0.0, 1.0); return o;
}
"#;
const PS: &str = r#"
struct Color { values : array<vec4<f32>, 1>, }
@group(1) @binding(0) var<uniform> color : Color;
@fragment fn main(@builtin(position) p : vec4<f32>) -> @location(0) vec4<f32> {
  return color.values[0];
}
"#;

fn rgba_bytes(color: [f32; 4]) -> Vec<u8> {
    color.into_iter().flat_map(f32::to_le_bytes).collect()
}

fn setup() -> Result<(Worker, wgpu::Device, wgpu::Queue, GuestMemory)> {
    let instance = Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
        .expect("adaptador wgpu obrigatório para pipeline_cache");
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("wgr-player-pipeline-cache"),
        ..Default::default()
    }))
    .expect("device wgpu obrigatório para pipeline_cache");
    let runtime = NativeRuntime::new()?;
    let memory = GuestMemory::new64(runtime.engine(), 1, 1, 1 << 20)?;
    memory.write(1060, &rgba_bytes([1.0, 0.0, 0.0, 1.0]))?;
    let mut worker = Worker::new(device.clone(), queue.clone());
    worker.register_shader_source("00000000deadbeef", VS);
    worker.register_shader_source("00000000cafebabe", PS);
    worker.exec(14, &[10, 0, 0xdead_beef, 0, 0], 0, &memory)?;
    worker.exec(14, &[12, 1, 0xcafe_babe, 0, 0], 0, &memory)?;
    worker.exec(15, &[11, 0], 0, &memory)?;
    worker.exec(3, &[2, 2, 4, 4, 1, 1, 1, 28, 0x20, 1], 0, &memory)?;
    worker.exec(11, &[6, 16, 4, 0, 0], 0, &memory)?;
    worker.exec(12, &[6, 0, 16], 1048, &memory)?;
    worker.exec(18, &[0, 10], 0, &memory)?;
    worker.exec(18, &[1, 12], 0, &memory)?;
    worker.exec(19, &[11], 0, &memory)?;
    worker.exec(20, &[4], 0, &memory)?;
    worker.exec(23, &[1, 0, 1, 6, 0, 16], 0, &memory)?;
    let mut targets = vec![0u32; 37];
    targets[0] = 2;
    targets[3] = 28;
    worker.exec(26, &targets, 0, &memory)?;
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory)?;
    Ok((worker, device, queue, memory))
}

#[test]
fn pipeline_cache_hits_when_uniform_bytes_change() {
    let (mut worker, device, queue, memory) = setup().expect("fixture pipeline_cache");
    worker.submit();
    assert_eq!(read_pixel(&device, &queue, &worker, 2), [255, 0, 0, 255]);
    let first = worker.pipeline_cache_stats();
    assert_eq!((first.hits, first.misses, first.live), (0, 1, 1));

    memory
        .write(1060, &rgba_bytes([0.0, 0.0, 1.0, 1.0]))
        .unwrap();
    worker.exec(12, &[6, 0, 16], 1048, &memory).unwrap();
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    assert_eq!(read_pixel(&device, &queue, &worker, 2), [0, 0, 255, 255]);
    let second = worker.pipeline_cache_stats();
    assert_eq!(
        second.hits, 1,
        "upload/uniform state não deve invalidar pipeline"
    );
    assert_eq!(second.misses, 1);
}

#[test]
fn pipeline_cache_invalidates_shader_generation_and_evicts() {
    let (mut worker, device, queue, memory) = setup().expect("fixture pipeline_cache");
    worker.submit();
    for _ in 0..129 {
        worker
            .exec(14, &[10, 0, 0xdead_beef, 0, 0], 0, &memory)
            .unwrap();
        worker.exec(18, &[0, 10], 0, &memory).unwrap();
        worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
        worker.submit();
    }
    let stats = worker.pipeline_cache_stats();
    assert_eq!(stats.live, 128);
    assert_eq!(
        stats.hits, 0,
        "shader recriado com mesmo ID/hash precisa perder o cache"
    );
    assert_eq!(stats.misses, 130);
    // A entrada quente não pode ser descartada pela eviction das antigas.
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    let warm = worker.pipeline_cache_stats();
    assert_eq!((warm.hits, warm.misses, warm.live), (1, 130, 128));
    let _ = (device, queue);
}

#[test]
fn pipeline_cache_invalidates_blend_and_reused_target_format() {
    let (mut worker, device, queue, memory) = setup().expect("fixture pipeline_cache");
    worker.submit();
    assert_eq!(read_pixel(&device, &queue, &worker, 2), [255, 0, 0, 255]);

    // Blend substitui o alvo: src=One(2), dst=Zero(1), Add(0), máscara RGBA.
    worker
        .exec(16, &[7, 1, 0, 0, 1, 2, 1, 0, 2, 1, 0, 15], 0, &memory)
        .unwrap();
    worker.exec(27, &[7, 0, 0, 0, 0, 0], 0, &memory).unwrap();
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    assert_eq!(read_pixel(&device, &queue, &worker, 2), [255, 0, 0, 255]);

    worker.exec(4, &[2], 0, &memory).unwrap();
    worker
        .exec(3, &[2, 2, 4, 4, 1, 1, 1, 29, 0x20, 1], 0, &memory)
        .unwrap();
    let mut targets = vec![0u32; 37];
    targets[0] = 2;
    targets[3] = 29;
    worker.exec(26, &targets, 0, &memory).unwrap();
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    assert_eq!(read_pixel(&device, &queue, &worker, 2), [255, 0, 0, 255]);

    let stats = worker.pipeline_cache_stats();
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 3);
    assert_eq!(stats.live, 3);
}
