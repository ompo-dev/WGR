//! Diagnóstico nativo de GPU: comando renderizado e lido de volta sem navegador.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::sync::mpsc;
use wgpu::{
    BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, Extent3d, LoadOp, MapMode,
    Operations, Origin3d, PollType, RenderPassColorAttachment, RenderPassDescriptor, StoreOp,
    TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

pub async fn diagnose() -> Result<Value> {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        })
        .await
        .context("GPU nativa: nenhum adaptador wgpu encontrado")?;
    let info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("WGR native GPU diagnostics"),
            ..Default::default()
        })
        .await
        .context("GPU nativa: não foi possível criar o dispositivo")?;

    let texture = device.create_texture(&TextureDescriptor {
        label: Some("WGR native probe texture"),
        size: Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("WGR native probe readback"),
        size: 256,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("WGR native probe encoder"),
    });
    {
        let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("WGR native probe pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &buffer,
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
    queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(8)),
        })
        .context("GPU nativa: timeout esperando readback")?;
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .context("GPU nativa: readback não respondeu")??;
    let data = slice.get_mapped_range()?;
    let pixel = data[..4].to_vec();
    drop(data);
    buffer.unmap();
    ensure!(
        pixel == [255, 0, 0, 255],
        "GPU nativa: pixel incorreto: {pixel:?}"
    );

    Ok(json!({
        "backend": "rust-wgpu",
        "browser": false,
        "adapter_name": info.name,
        "adapter_backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type),
        "vendor_id": info.vendor,
        "device_id": info.device,
        "max_texture_dimension_2d": device.limits().max_texture_dimension_2d,
        "pixel_readback": pixel,
        "rendered_and_read_back": true,
        "wasm_runtime": "Wasmtime 50",
        "note": "GPU validada com uma textura de 1x1; não comprova jogo GTA."
    }))
}
