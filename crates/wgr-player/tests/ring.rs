//! Teste headless do ring completo: os opcodes criam textura, enviam pixels,
//! registram shaders/layout/sampler/SRV, ligam o render target, desenham e o
//! pixel lido de volta precisa ser o vermelho enviado.

mod common;

use common::read_pixel;
use wgpu::{DeviceDescriptor, Instance, RequestAdapterOptions};
use wgr_player::gpu::upload::{BufferUpload, parse_texture_upload};
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
  return textureSampleLevel(t, s, vec2<f32>(0.5), 0.0);
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
    for (op, words) in [
        (4, vec![]),
        (5, vec![1]),
        (16, vec![4, 3]),
        (18, vec![0]),
        (21, vec![0, u32::MAX]),
        (24, vec![0, 0, 2, 1]),
        (30, vec![u32::MAX]),
        (32, vec![]),
        (39, vec![1, 1, 1]),
        (48, vec![]),
    ] {
        assert!(worker.exec(op, &words, 0, &memory).is_err(), "opcode {op}");
    }
    worker.exec(21, &[0, 1, 99, 24, 8], 0, &memory).unwrap();
    assert_eq!(worker.state.vertex_buffers[0].stride, 24);
    assert_eq!(worker.state.vertex_buffers[0].offset, 8);
    worker.exec(22, &[99, 57, 4], 0, &memory).unwrap();
    assert_eq!(worker.state.index_buffer.stride, 57);
    assert_eq!(worker.state.index_buffer.offset, 4);
    worker
        .exec(38, &[2, 0, 1, 77, u32::MAX], 0, &memory)
        .unwrap();
    assert_eq!(worker.state.uav[2][0], 77);
    worker.exec(33, &[], 0, &memory).unwrap();
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
    worker.exec(11, &[6, 16, 0x4, 0, 0], 0, &memory).unwrap();
    let outside = BufferUpload {
        id: 6,
        offset: 8,
        size: 12,
        data_address: 1048,
        no_overwrite: false,
    };
    assert!(
        worker.upload_buffer(&outside, &memory).is_err(),
        "não trunca silenciosamente um upload fora do buffer"
    );
    let outside = parse_texture_upload(1000, &[1, 0, 0, 1, 0, 0, 4, 4, 1, 16, 4, 64]).unwrap();
    assert!(
        worker.upload_texture(&outside, &memory).is_err(),
        "limites da textura são validados antes do wgpu"
    );
    worker.exec(3, &texture(2, 0x20), 0, &memory).unwrap();
    worker.exec(3, &texture(5, 0x20), 0, &memory).unwrap();
    worker
        .exec(
            5,
            &[5, 0, 0, 0, 1f32.to_bits(), 0, 1f32.to_bits()],
            0,
            &memory,
        )
        .unwrap();
    worker.submit();
    assert_eq!(
        read_pixel(&device, &queue, &worker, 5),
        [0, 255, 0, 255],
        "clear sem desenho nem alvo ligado"
    );
    // UPLOAD_TEXTURE: 12 palavras; os bytes começam 48 bytes depois.
    worker
        .exec(13, &[1, 0, 0, 0, 0, 0, 4, 4, 1, 16, 4, 64], 1000, &memory)
        .unwrap();
    // CREATE_SRV da fonte e CREATE_STATE de sampler.
    worker
        .exec(17, &[3, 0, 1, 0, 2, 0, 0, 0, 0], 0, &memory)
        .unwrap();
    let mut sampler = vec![4, 3, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 32f32.to_bits()];
    worker.exec(16, &sampler, 0, &memory).unwrap();
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
    worker
        .exec(27, &[0, 0, 0, 0, 0, u32::MAX], 0, &memory)
        .unwrap();
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
    memory.write(1048, &[0, 0, 255, 255].repeat(16)).unwrap();
    worker
        .exec(13, &[1, 0, 0, 0, 0, 0, 4, 4, 1, 16, 4, 64], 1000, &memory)
        .unwrap();
    targets[0] = 5;
    worker.exec(26, &targets, 0, &memory).unwrap();
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    assert_eq!(
        read_pixel(&device, &queue, &worker, 5),
        [0, 0, 255, 255],
        "desenho depois do upload usa os novos pixels"
    );

    assert!(matches!(worker.objects.get(10), Some(Object::Shader(_))));
    assert!(matches!(worker.objects.get(11), Some(Object::Layout(_))));
    assert!(matches!(worker.objects.get(4), Some(Object::Sampler(_))));
    assert!(worker.view(3).is_some());
    assert_eq!(
        read_pixel(&device, &queue, &worker, 2),
        [255, 0, 0, 255],
        "upload posterior não muda desenho já gravado"
    );
    worker
        .exec(
            5,
            &[2, 0, 0, 0, 0, 1f32.to_bits(), 1f32.to_bits()],
            0,
            &memory,
        )
        .unwrap();
    worker.submit();
    assert_eq!(
        read_pixel(&device, &queue, &worker, 2),
        [0, 0, 255, 255],
        "clear no recurso solicitado, não no slot zero"
    );

    let row = [
        [255, 0, 0, 255],
        [255, 0, 0, 255],
        [0, 0, 255, 255],
        [0, 0, 255, 255],
    ]
    .concat();
    memory.write(1048, &row.repeat(4)).unwrap();
    worker
        .exec(13, &[1, 0, 0, 0, 0, 0, 4, 4, 1, 16, 4, 64], 1000, &memory)
        .unwrap();
    targets[0] = 2;
    worker.exec(26, &targets, 0, &memory).unwrap();
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    assert_eq!(
        read_pixel(&device, &queue, &worker, 2),
        [0, 0, 255, 255],
        "filtro point usa o texel da direita"
    );
    sampler[0] = 7;
    sampler[2] = 0x15;
    worker.exec(16, &sampler, 0, &memory).unwrap();
    worker.exec(25, &[1, 0, 1, 7], 0, &memory).unwrap();
    worker.exec(32, &[0, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    let pixel = read_pixel(&device, &queue, &worker, 2);
    assert!(
        (127..=128).contains(&pixel[0]) && (127..=128).contains(&pixel[2]),
        "filtro linear mistura os texels: {pixel:?}"
    );
    assert_eq!((pixel[1], pixel[3]), (0, 255));
    memory.write(1048, &[0, 0, 255, 255].repeat(16)).unwrap();
    worker
        .exec(13, &[1, 0, 0, 0, 0, 0, 4, 4, 1, 16, 4, 64], 1000, &memory)
        .unwrap();

    worker.register_shader_source(
        "0000000000000060",
        r#"
        @vertex fn main(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(pos, 0.0, 1.0);
        }
    "#,
    );
    worker
        .exec(14, &[60, 0, 0x60, 0, 1, 1, 0, 0, 3], 0, &memory)
        .unwrap();
    worker
        .exec(15, &[61, 1, 1, 0, 16, 0, 0, 0, 0], 0, &memory)
        .unwrap();
    worker.exec(11, &[62, 64, 1, 0, 0], 0, &memory).unwrap();
    worker.exec(11, &[63, 8, 2, 0, 0], 0, &memory).unwrap();
    let mut vertices = vec![0; 8];
    for (x, y) in [(-1f32, -1f32), (-1., 3.), (3., -1.)] {
        vertices.extend_from_slice(&x.to_le_bytes());
        vertices.extend_from_slice(&y.to_le_bytes());
        vertices.extend_from_slice(&[0; 8]);
    }
    vertices.resize(64, 0);
    memory.write(2000, &vertices).unwrap();
    memory.write(2100, &[255, 255, 0, 0, 1, 0, 2, 0]).unwrap();
    worker.exec(12, &[62, 0, 64], 1988, &memory).unwrap();
    worker.exec(12, &[63, 0, 8], 2088, &memory).unwrap();
    worker.exec(18, &[0, 60], 0, &memory).unwrap();
    worker.exec(19, &[61], 0, &memory).unwrap();
    worker.exec(21, &[0, 1, 62, 16, 8], 0, &memory).unwrap();
    worker.exec(22, &[63, 57, 2], 0, &memory).unwrap();
    worker
        .exec(
            5,
            &[5, 0, 0, 0, 1f32.to_bits(), 0, 1f32.to_bits()],
            0,
            &memory,
        )
        .unwrap();
    worker
        .exec(5, &[2, 0, 0, 0, 0, 0, 1f32.to_bits()], 0, &memory)
        .unwrap();
    targets[0] = 2;
    worker.exec(26, &targets, 0, &memory).unwrap();
    worker.exec(32, &[1, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    targets[0] = 5;
    worker.exec(26, &targets, 0, &memory).unwrap();
    worker.exec(32, &[1, 3, 1, 0, 0, 0], 0, &memory).unwrap();
    worker.submit();
    for id in [2, 5] {
        assert_eq!(
            read_pixel(&device, &queue, &worker, id),
            [0, 0, 255, 255],
            "desenho indexado, stride e offset no alvo {id}"
        );
    }
}
