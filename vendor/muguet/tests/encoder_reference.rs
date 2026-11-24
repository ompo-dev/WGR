//! Opt-in real GPU fixture matching scripts/Benchmark-Dawn.mjs. No GTA data.
use rayon::prelude::*;
use std::time::Instant;
use wgpu::*;

#[test]
#[ignore = "requires physical GPU"]
fn large_uniform_upload_reference() {
    let mut desc = InstanceDescriptor::new_without_display_handle();
    desc.backends = Backends::VULKAN;
    let instance = Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
    let dst = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 8 << 20,
        usage: BufferUsages::COPY_DST | BufferUsages::COPY_SRC | BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    let rb = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for bytes in [2 << 20, 5 << 20, 7 << 20] {
        let mut data = vec![0x52u8; bytes];
        let mut samples = Vec::new();
        for run in 0..23 {
            data[bytes - 4..].copy_from_slice(&(run as u32).to_le_bytes());
            let t = Instant::now();
            queue.write_buffer(&dst, 0, &data);
            let upload = t.elapsed().as_secs_f64() * 1000.;
            let mut enc = device.create_command_encoder(&CommandEncoderDescriptor::default());
            enc.copy_buffer_to_buffer(&dst, (bytes - 4) as u64, &rb, 0, 4);
            let idx = queue.submit([enc.finish()]);
            rb.slice(..).map_async(MapMode::Read, |r| r.unwrap());
            device
                .poll(PollType::Wait {
                    submission_index: Some(idx),
                    timeout: None,
                })
                .unwrap();
            let view = rb.slice(..).get_mapped_range().unwrap();
            assert_eq!(&view[..4], &(run as u32).to_le_bytes());
            drop(view);
            rb.unmap();
            if run >= 3 {
                samples.push(serde_json::json!({"upload_ms":upload,"total_ms":t.elapsed().as_secs_f64()*1000.}));
            }
        }
        println!(
            "WGR_UPLOAD_BENCH {}",
            serde_json::json!({"bytes":bytes,"samples":samples})
        );
    }
}

#[test]
#[ignore = "requires a physical GPU; run separately from gameplay"]
fn encoder_reference() {
    let mut desc = InstanceDescriptor::new_without_display_handle();
    desc.backends = if std::env::var("WGR_GPU_BACKEND").as_deref() == Ok("dx12") {
        Backends::DX12
    } else {
        Backends::VULKAN
    };
    let instance = Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
    let info = adapter.get_info();
    println!("ADAPTER {} {:?}", info.name, info.backend);
    let stride = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(256);
    let constants = device.create_buffer(&BufferDescriptor {
        label: None,
        size: stride * 256,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut data = vec![0u8; (stride * 256) as usize];
    for i in 0..256usize {
        let color = if i % 2 == 0 {
            [1.0f32, 0.0, 0.0, 1.0]
        } else {
            [0.0f32, 1.0, 0.0, 1.0]
        };
        for (j, c) in color.iter().enumerate() {
            let at = i * stride as usize + j * 4;
            data[at..at + 4].copy_from_slice(&c.to_le_bytes());
        }
    }
    queue.write_buffer(&constants, 0, &data);
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: None,
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: BufferSize::new(16),
            },
            count: None,
        }],
    });
    let bg = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: &constants,
                offset: 0,
                size: BufferSize::new(16),
            }),
        }],
    });
    let pl_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: None,
        source: ShaderSource::Wgsl(
            include_str!("../../../scripts/fixtures/encoder-reference.wgsl").into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: None,
        layout: Some(&pl_layout),
        vertex: VertexState {
            module: &module,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState {
                format: TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let target = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let read = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 256,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for (passes, draws) in [(1, 3200), (320, 10), (800, 4)] {
        for mode in [
            "serial",
            "split_serial",
            "parallel_finish",
            "parallel_record",
        ] {
            if passes == 1 && mode != "serial" {
                continue;
            }
            let mut samples = Vec::new();
            for run in 0..13 {
                let started = Instant::now();
                let record = |start, end| {
                    let mut encoder =
                        device.create_command_encoder(&CommandEncoderDescriptor::default());
                    for p in start..end {
                        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(RenderPassColorAttachment {
                                view: &view,
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
                        for d in 0..draws {
                            pass.set_bind_group(
                                0,
                                &bg,
                                &[(((p * draws + d) % 256) as u64 * stride) as u32],
                            );
                            pass.draw(0..3, 0..1);
                        }
                    }
                    if end == passes {
                        encoder.copy_texture_to_buffer(
                            TexelCopyTextureInfo {
                                texture: &target,
                                mip_level: 0,
                                origin: Origin3d::ZERO,
                                aspect: TextureAspect::All,
                            },
                            TexelCopyBufferInfo {
                                buffer: &read,
                                layout: TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(256),
                                    rows_per_image: Some(1),
                                },
                            },
                            Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                    encoder
                };
                let count = if mode == "serial" { 1 } else { 4 };
                let ranges: Vec<_> = (0..count)
                    .map(|i| (passes * i / count, passes * (i + 1) / count))
                    .collect();
                let encoders: Vec<_> = if mode == "parallel_record" {
                    pool.install(|| ranges.into_par_iter().map(|(a, b)| record(a, b)).collect())
                } else {
                    ranges.into_iter().map(|(a, b)| record(a, b)).collect()
                };
                let encoded = Instant::now();
                let commands: Vec<_> = if mode == "parallel_record" || mode == "parallel_finish" {
                    pool.install(|| {
                        encoders
                            .into_par_iter()
                            .map(CommandEncoder::finish)
                            .collect()
                    })
                } else {
                    encoders.into_iter().map(CommandEncoder::finish).collect()
                };
                let finished = Instant::now();
                let submitted = queue.submit(commands);
                let queued = Instant::now();
                read.slice(..).map_async(MapMode::Read, |r| r.unwrap());
                device
                    .poll(PollType::Wait {
                        submission_index: Some(submitted),
                        timeout: None,
                    })
                    .unwrap();
                let slice = read.slice(..).get_mapped_range().unwrap();
                assert_eq!(&slice[..4], &[0, 255, 0, 255]);
                drop(slice);
                read.unmap();
                let ended = Instant::now();
                if run >= 3 {
                    let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f64() * 1000.0;
                    samples.push(serde_json::json!({"record_ms":ms(started,encoded),"finish_ms":ms(encoded,finished),"submit_ms":ms(finished,queued),"total_ms":ms(started,ended)}));
                }
            }
            println!(
                "WGR_ENCODER_BENCH {}",
                serde_json::json!({"adapter":info.name,"backend":format!("{:?}",info.backend),"mode":mode,"passes":passes,"draws_per_pass":draws,"samples":samples,"pixel":[0,255,0,255]})
            );
        }
    }
}
