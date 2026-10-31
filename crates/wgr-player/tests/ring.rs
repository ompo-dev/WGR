//! Teste headless do ring completo: os opcodes criam textura, enviam pixels,
//! registram shaders/layout/sampler/SRV, ligam o render target, desenham e o
//! pixel lido de volta precisa ser o vermelho enviado.

use wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, DeviceDescriptor, Extent3d, Instance,
    MapMode, Origin3d, PollType, RequestAdapterOptions, TexelCopyBufferInfo, TexelCopyBufferLayout,
    TexelCopyTextureInfo,
};
use wgr_player::gpu::worker::{Object, Worker};
use wgr_runtime::NativeRuntime;
use wgr_runtime::shared::GuestMemory;

const VS_HASH: (u32, u32) = (0xdead_beef, 0);
const PS_HASH: (u32, u32) = (0xcafe_babe, 0);

const VS: &str = r#"
struct VOut { @builtin(position) pos : vec4<f32>, }
@vertex fn main(@builtin(vertex_index) i : u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(-1.0, 3.0), vec2<f32>(3.0, -1.0));
  var o : VOut;
  o.pos = vec4<f32>(p[i], 0.0, 1.0);
  return o;
}
"#;

const PS: &str = r#"
@group(1) @binding(32) var t : texture_2d<f32>;
@group(1) @binding(16) var s : sampler;
@fragment fn main(@builtin(position) p : vec4<f32>) -> @location(0) vec4<f32> {
  return textureLoad(t, vec2<i32>(p.xy), 0);
}
"#;

fn texture(id: u32, bind: u32) -> Vec<u32> {
    vec![id, 2, 4, 4, 1, 1, 1, 28, bind, 1]
}

#[test]
fn executes_the_ring_until_a_drawn_pixel_is_read_back() {
    let instance = Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
    else {
        eprintln!("sem adaptador wgpu; teste do ring ignorado");
        return;
    };
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("wgr-player-ring"),
        ..Default::default()
    })) else {
        eprintln!("sem device wgpu; teste do ring ignorado");
        return;
    };
    let runtime = NativeRuntime::new().unwrap();
    let memory = GuestMemory::new64(runtime.engine(), 1, 1, 1 << 20).unwrap();
    let mut red = Vec::with_capacity(64);
    for _ in 0..16 {
        red.extend_from_slice(&[255, 0, 0, 255]);
    }
    memory.write(1048, &red).unwrap();

    let mut worker = Worker::new(device.clone(), queue.clone());
    worker.register_shader_source("00000000deadbeef", VS);
    worker.register_shader_source("00000000cafebabe", PS);

    // CREATE_SHADER: id, stage, hash baixo, hash alto, entradas.
    worker
        .exec(14, &[10, 0, VS_HASH.0, VS_HASH.1, 0], 0, &memory)
        .unwrap();
    worker
        .exec(14, &[12, 1, PS_HASH.0, PS_HASH.1, 0], 0, &memory)
        .unwrap();
    // CREATE_LAYOUT sem elementos: o vertex shader usa só vertex_index.
    worker.exec(15, &[11, 0], 0, &memory).unwrap();
    // CREATE_TEXTURE fonte e alvo.
    worker.exec(3, &texture(1, 0x8), 0, &memory).unwrap();
    worker.exec(3, &texture(2, 0x20), 0, &memory).unwrap();
    // UPLOAD_TEXTURE: 12 palavras; os bytes começam 48 bytes depois.
    worker
        .exec(13, &[1, 0, 0, 0, 0, 0, 4, 4, 1, 16, 4, 64], 1000, &memory)
        .unwrap();
    // CREATE_SRV da fonte e CREATE_STATE de sampler.
    worker
        .exec(17, &[3, 0, 1, 0, 2, 0, 0, 0, 0], 0, &memory)
        .unwrap();
    worker.exec(16, &[4, 3], 0, &memory).unwrap();
    // SET_SHADER (0 e 1), SET_INPUT_LAYOUT e SET_TOPOLOGY.
    worker.exec(18, &[0, 10], 0, &memory).unwrap();
    worker.exec(18, &[1, 12], 0, &memory).unwrap();
    worker.exec(19, &[11], 0, &memory).unwrap();
    worker.exec(20, &[4], 0, &memory).unwrap();
    // SET_RENDER_TARGETS: 8 RTs de 4 palavras + 5 da profundidade.
    let mut targets = vec![0u32; 37];
    targets[0] = 2;
    targets[3] = 28;
    worker.exec(26, &targets, 0, &memory).unwrap();
    // SET_VIEWPORTS: contagem e seis floats.
    let viewport = [
        1u32,
        0f32.to_bits(),
        0f32.to_bits(),
        4f32.to_bits(),
        4f32.to_bits(),
        0f32.to_bits(),
        1f32.to_bits(),
    ];
    worker.exec(30, &viewport, 0, &memory).unwrap();
    worker.exec(24, &[1, 0, 1, 3], 0, &memory).unwrap();
    worker.exec(25, &[1, 0, 1, 4], 0, &memory).unwrap();
    worker.exec(27, &[0], 0, &memory).unwrap();
    worker
        .exec(
            5,
            &[
                2,
                0,
                0,
                0f32.to_bits(),
                0f32.to_bits(),
                0f32.to_bits(),
                1f32.to_bits(),
            ],
            0,
            &memory,
        )
        .unwrap();
    // DRAW: kind, count, instances, start, baseVertex, startInstance.
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();

    assert!(matches!(worker.objects.get(10), Some(Object::Shader(_))));
    assert!(matches!(worker.objects.get(11), Some(Object::Layout(_))));
    assert!(matches!(worker.objects.get(4), Some(Object::Sampler(_))));
    assert!(worker.view(3).is_some());

    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("ring-readback"),
        size: 256 * 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("ring-readback"),
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &worker.texture(2).unwrap().texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(4),
            },
        },
        Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    let _ = device.poll(PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    receiver.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().unwrap();
    assert_eq!(&data[0..4], &[255, 0, 0, 255], "pixel desenhado no alvo");
    drop(data);
    readback.unmap();
}
