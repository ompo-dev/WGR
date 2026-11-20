//! Leitura de pixels compartilhada pelas fixtures offline da GPU.

use wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, MapMode, Origin3d,
    PollType, TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo,
};
use wgr_player::gpu::worker::Worker;
pub fn read_pixel(device: &wgpu::Device, queue: &wgpu::Queue, worker: &Worker, id: u32) -> [u8; 4] {
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
            texture: &worker.texture(id).unwrap().texture,
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
    let pixel = data[0..4].try_into().unwrap();
    drop(data);
    readback.unmap();
    pixel
}
