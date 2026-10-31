//! Teste headless do worker: cria textura, envia pixels, cria sampler/SRV,
//! monta um pipeline e desenha um triângulo que amostra a textura. Requer um
//! adaptador wgpu; sem adaptador (CI sem GPU) o teste é ignorado com aviso.

use wgpu::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, BufferDescriptor, BufferUsages, Color,
    ColorTargetState, ColorWrites, CommandEncoderDescriptor, DeviceDescriptor, Extent3d,
    FragmentState, Instance, LoadOp, MapMode, MultisampleState, Operations, Origin3d,
    PipelineLayoutDescriptor, PollType, PrimitiveState, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, RequestAdapterOptions, ShaderModuleDescriptor,
    ShaderSource, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo,
    TextureDimension, TextureFormat, TextureViewDescriptor, VertexState,
};
use wgr_player::gpu::objects::{TextureRequest, ViewRequest};
use wgr_player::gpu::upload::TextureUpload;
use wgr_player::gpu::worker::Worker;
use wgr_runtime::NativeRuntime;
use wgr_runtime::shared::GuestMemory;

#[test]
fn draws_a_frame_from_an_uploaded_texture() {
    let instance = Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
    else {
        eprintln!("sem adaptador wgpu; teste headless ignorado");
        return;
    };
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("wgr-player-headless"),
        ..Default::default()
    })) else {
        eprintln!("sem device wgpu; teste headless ignorado");
        return;
    };

    let runtime = NativeRuntime::new().unwrap();
    let memory = GuestMemory::new64(runtime.engine(), 1, 1, 1 << 20).unwrap();
    let mut red = Vec::with_capacity(4 * 4 * 4);
    for _ in 0..16 {
        red.extend_from_slice(&[255, 0, 0, 255]);
    }
    memory.write(0, &red).unwrap();

    let mut worker = Worker::new(device.clone(), queue.clone());
    worker
        .create_texture(&TextureRequest {
            id: 1,
            dimension: TextureDimension::D2,
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
            mip_levels: 1,
            array_layers: 1,
            dxgi: 28,
            bind: 0x8,
            samples: 1,
        })
        .unwrap();
    worker
        .upload_texture(
            &TextureUpload {
                id: 1,
                mip: 0,
                slice: 0,
                x: 0,
                y: 0,
                z: 0,
                width: 4,
                height: 4,
                depth: 1,
                bytes_per_row: 16,
                rows: 4,
                size: 64,
                data_address: 0,
            },
            &memory,
        )
        .unwrap();
    worker
        .create_sampler(2, &wgpu::SamplerDescriptor::default())
        .unwrap();
    worker
        .create_view(&ViewRequest {
            id: 3,
            resource: 1,
            is_buffer: false,
            dxgi: 0,
            dimension: wgpu::TextureViewDimension::D2,
            a: 0,
            b: 0,
            c: 0,
            d: 0,
            flags: 0,
        })
        .unwrap();
    worker
        .create_texture(&TextureRequest {
            id: 4,
            dimension: TextureDimension::D2,
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
            mip_levels: 1,
            array_layers: 1,
            dxgi: 28,
            bind: 0x20,
            samples: 1,
        })
        .unwrap();

    let target = worker
        .texture(4)
        .unwrap()
        .texture
        .create_view(&TextureViewDescriptor::default());
    let source = worker.view(3).unwrap();
    let sampler = match worker.objects.get(2) {
        Some(wgr_player::gpu::worker::Object::Sampler(sampler)) => sampler,
        _ => panic!("sampler ausente"),
    };

    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("headless"),
        source: ShaderSource::Wgsl(
            r#"
@group(0) @binding(0) var t : texture_2d<f32>;
@group(0) @binding(1) var s : sampler;
struct VOut { @builtin(position) pos : vec4<f32>, @location(0) uv : vec2<f32>, }
@vertex fn vs(@builtin(vertex_index) i : u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  var o : VOut;
  o.pos = vec4<f32>(p[i], 0.0, 1.0);
  o.uv = (p[i] + vec2<f32>(1.0, 1.0)) * 0.5;
  return o;
}
@fragment fn fs(v : VOut) -> @location(0) vec4<f32> {
  return textureSample(t, s, v.uv);
}
"#
            .into(),
        ),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("headless"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("headless"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("headless"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState {
                format: TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("headless"),
        layout: &bgl,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(source),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(sampler),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("headless"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("headless"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size: 256 * 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &worker.texture(4).unwrap().texture,
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
    assert_eq!(&data[0..4], &[255, 0, 0, 255]);
    drop(data);
    readback.unmap();
}
