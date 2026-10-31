//! Tabela de recursos e usos de textura/buffer, transcrita de `createTexture`,
//! `createBuffer` e do roteamento de SRV/UAV do `wgpu_worker.js`.

use crate::gpu::formats::{BIND_DS, BIND_RT, BIND_SRV, BIND_UAV};
use wgpu::{
    BufferDescriptor, BufferUsages, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureViewDimension,
};

/// Bits de uso de buffer do protocolo D3D11 emulado.
pub const BIND_VERTEX: u32 = 0x1;
pub const BIND_INDEX: u32 = 0x2;
pub const BIND_CONSTANT: u32 = 0x4;
pub const BIND_STORAGE: u32 = 0x8 | 0x80;
pub const MISC_INDIRECT: u32 = 0x10;

/// Usos de uma textura, na mesma ordem do worker web.
pub fn texture_usage(bind: u32, sample_count: u32) -> TextureUsages {
    let mut usage = TextureUsages::COPY_SRC | TextureUsages::COPY_DST;
    if bind & BIND_SRV != 0 {
        usage |= TextureUsages::TEXTURE_BINDING;
    }
    if bind & (BIND_RT | BIND_DS) != 0 {
        usage |= TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
    }
    if bind & BIND_UAV != 0 {
        usage |= TextureUsages::STORAGE_BINDING;
    }
    if bind & (BIND_SRV | BIND_RT | BIND_DS | BIND_UAV) == 0 {
        usage |= TextureUsages::TEXTURE_BINDING;
    }
    if sample_count > 1 {
        usage.remove(
            TextureUsages::COPY_SRC | TextureUsages::COPY_DST | TextureUsages::STORAGE_BINDING,
        );
    }
    usage
}

/// Usos de um buffer, incluindo o espelho de constantes (que não vira buffer
/// de GPU: o host empacota os uniformes por desenho).
pub fn buffer_usage(bind: u32, misc: u32) -> BufferUsages {
    let mut usage = BufferUsages::COPY_DST | BufferUsages::COPY_SRC;
    if bind & BIND_VERTEX != 0 {
        usage |= BufferUsages::VERTEX;
    }
    if bind & BIND_INDEX != 0 {
        usage |= BufferUsages::INDEX;
    }
    if bind & (BIND_STORAGE | 0x80) != 0 {
        usage |= BufferUsages::STORAGE;
    }
    if misc & MISC_INDIRECT != 0 {
        usage |= BufferUsages::INDIRECT;
    }
    usage
}

/// Descreve uma textura do protocolo e cria o objeto wgpu correspondente.
#[allow(clippy::too_many_arguments)]
pub fn create_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    depth_or_array_layers: u32,
    mip_level_count: u32,
    sample_count: u32,
    dimension: TextureDimension,
    format: TextureFormat,
    view_formats: &[TextureFormat],
    bind: u32,
) -> wgpu::Texture {
    let usage = texture_usage(bind, sample_count);
    let descriptor = TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: if dimension == TextureDimension::D1 {
                1
            } else {
                height.max(1)
            },
            depth_or_array_layers: depth_or_array_layers.max(1),
        },
        mip_level_count: mip_level_count.max(1),
        sample_count: sample_count.max(1),
        dimension,
        format,
        usage,
        view_formats,
    };
    device.create_texture(&descriptor)
}

pub fn create_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    bind: u32,
    misc: u32,
) -> wgpu::Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage: buffer_usage(bind, misc),
        mapped_at_creation: false,
    })
}

/// Dimensão da view de um SRV/UAV no espaço do shader.
pub fn view_dimension(dim: u32) -> TextureViewDimension {
    match dim {
        1 => TextureViewDimension::D1,
        3 => TextureViewDimension::D3,
        4 => TextureViewDimension::Cube,
        5 => TextureViewDimension::CubeArray,
        6 => TextureViewDimension::D2Array,
        _ => TextureViewDimension::D2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_a_texture_requires_texture_binding() {
        assert!(texture_usage(BIND_SRV, 1).contains(TextureUsages::TEXTURE_BINDING));
        assert!(!texture_usage(BIND_SRV, 1).contains(TextureUsages::RENDER_ATTACHMENT));
    }

    #[test]
    fn render_targets_can_be_sampled_like_the_web_worker() {
        let usage = texture_usage(BIND_RT, 1);
        assert!(usage.contains(TextureUsages::RENDER_ATTACHMENT));
        assert!(usage.contains(TextureUsages::TEXTURE_BINDING));
    }

    #[test]
    fn multisampled_textures_drop_copy_and_storage_usages() {
        let usage = texture_usage(BIND_RT | BIND_UAV, 4);
        assert!(usage.contains(TextureUsages::RENDER_ATTACHMENT));
        assert!(!usage.contains(TextureUsages::COPY_SRC));
        assert!(!usage.contains(TextureUsages::STORAGE_BINDING));
    }

    #[test]
    fn buffers_route_vertex_index_storage_and_indirect() {
        assert!(buffer_usage(BIND_VERTEX, 0).contains(BufferUsages::VERTEX));
        assert!(buffer_usage(BIND_INDEX, 0).contains(BufferUsages::INDEX));
        assert!(buffer_usage(BIND_STORAGE, 0).contains(BufferUsages::STORAGE));
        assert!(buffer_usage(0, MISC_INDIRECT).contains(BufferUsages::INDIRECT));
    }

    #[test]
    fn constant_buffers_are_not_gpu_buffers() {
        let usage = buffer_usage(BIND_CONSTANT, 0);
        assert!(!usage.contains(BufferUsages::UNIFORM));
        assert!(!usage.contains(BufferUsages::STORAGE));
    }
}
