//! Entradas de bind group layout, transcritas de `layoutEntry` do
//! `wgpu_worker.js`. A tabela é pura; o worker só a entrega ao wgpu.

use crate::shader::{Binding, BindingKind};
use wgpu::{
    BindGroupLayoutEntry, BindingType, BufferBindingType, SamplerBindingType, ShaderStages,
    StorageTextureAccess, TextureFormat, TextureSampleType, TextureViewDimension,
};

pub fn shader_stages(stage: u8) -> ShaderStages {
    match stage {
        1 => ShaderStages::FRAGMENT,
        2 => ShaderStages::COMPUTE,
        _ => ShaderStages::VERTEX,
    }
}

pub fn view_dimension(dim: &str) -> TextureViewDimension {
    match dim {
        "1d" => TextureViewDimension::D1,
        "2d-array" => TextureViewDimension::D2Array,
        "3d" => TextureViewDimension::D3,
        "cube" => TextureViewDimension::Cube,
        "cube-array" => TextureViewDimension::CubeArray,
        _ => TextureViewDimension::D2,
    }
}

pub fn sample_type(sample: &str) -> TextureSampleType {
    match sample {
        "uint" => TextureSampleType::Uint,
        "sint" => TextureSampleType::Sint,
        "depth" => TextureSampleType::Depth,
        "unfilterable-float" => TextureSampleType::Float { filterable: false },
        _ => TextureSampleType::Float { filterable: true },
    }
}

pub fn storage_access(access: &str) -> StorageTextureAccess {
    match access {
        "read-only" => StorageTextureAccess::ReadOnly,
        "read-write" => StorageTextureAccess::ReadWrite,
        _ => StorageTextureAccess::WriteOnly,
    }
}

/// Nome WebGPU/WGSL (minúsculo) para o formato de storage texture.
pub fn parse_format(name: &str) -> Option<TextureFormat> {
    use TextureFormat as F;
    Some(match name {
        "r8unorm" => F::R8Unorm,
        "r8snorm" => F::R8Snorm,
        "r8uint" => F::R8Uint,
        "r8sint" => F::R8Sint,
        "r16uint" => F::R16Uint,
        "r16sint" => F::R16Sint,
        "r16float" => F::R16Float,
        "rg8unorm" => F::Rg8Unorm,
        "rg8snorm" => F::Rg8Snorm,
        "rg8uint" => F::Rg8Uint,
        "rg8sint" => F::Rg8Sint,
        "r32uint" => F::R32Uint,
        "r32sint" => F::R32Sint,
        "r32float" => F::R32Float,
        "rg16uint" => F::Rg16Uint,
        "rg16sint" => F::Rg16Sint,
        "rg16float" => F::Rg16Float,
        "rgba8unorm" => F::Rgba8Unorm,
        "rgba8unorm-srgb" => F::Rgba8UnormSrgb,
        "rgba8snorm" => F::Rgba8Snorm,
        "rgba8uint" => F::Rgba8Uint,
        "rgba8sint" => F::Rgba8Sint,
        "bgra8unorm" => F::Bgra8Unorm,
        "bgra8unorm-srgb" => F::Bgra8UnormSrgb,
        "rgb10a2unorm" => F::Rgb10a2Unorm,
        "rgb10a2uint" => F::Rgb10a2Uint,
        "rg11b10ufloat" => F::Rg11b10Ufloat,
        "rg32uint" => F::Rg32Uint,
        "rg32sint" => F::Rg32Sint,
        "rg32float" => F::Rg32Float,
        "rgba16uint" => F::Rgba16Uint,
        "rgba16sint" => F::Rgba16Sint,
        "rgba16float" => F::Rgba16Float,
        "rgba32uint" => F::Rgba32Uint,
        "rgba32sint" => F::Rgba32Sint,
        "rgba32float" => F::Rgba32Float,
        _ => return None,
    })
}

pub fn layout_entry(binding: &Binding, stage: u8) -> Option<BindGroupLayoutEntry> {
    let ty = match binding.kind {
        BindingKind::Uniform => BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: binding.binding != 15,
            min_binding_size: None,
        },
        BindingKind::Storage => BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        BindingKind::ReadOnlyStorage => BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        BindingKind::Sampler => BindingType::Sampler(SamplerBindingType::Filtering),
        BindingKind::SamplerComparison => BindingType::Sampler(SamplerBindingType::Comparison),
        BindingKind::Texture => BindingType::Texture {
            sample_type: sample_type(&binding.sample),
            view_dimension: view_dimension(&binding.dim),
            multisampled: binding.multisampled,
        },
        BindingKind::StorageTexture => BindingType::StorageTexture {
            access: storage_access(binding.access.as_deref().unwrap_or("write-only")),
            format: parse_format(binding.format.as_deref().unwrap_or("rgba8unorm"))?,
            view_dimension: view_dimension(&binding.dim),
        },
    };
    Some(BindGroupLayoutEntry {
        binding: binding.binding,
        visibility: shader_stages(stage),
        ty,
        count: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(kind: BindingKind, binding: u32) -> Binding {
        Binding {
            group: 1,
            binding,
            name: "b".into(),
            kind,
            dim: "2d".into(),
            sample: "float".into(),
            multisampled: false,
            format: None,
            access: None,
        }
    }

    #[test]
    fn packed_uniforms_use_a_dynamic_offset_but_inline_constants_do_not() {
        let entry = layout_entry(&binding(BindingKind::Uniform, 0), 1).unwrap();
        assert!(matches!(
            entry.ty,
            BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: true,
                ..
            }
        ));
        let inline = layout_entry(&binding(BindingKind::Uniform, 15), 1).unwrap();
        assert!(matches!(
            inline.ty,
            BindingType::Buffer {
                has_dynamic_offset: false,
                ..
            }
        ));
        assert_eq!(entry.visibility, ShaderStages::FRAGMENT);
    }

    #[test]
    fn samplers_and_comparison_samplers_keep_their_binding_type() {
        let filtering = layout_entry(&binding(BindingKind::Sampler, 16), 0).unwrap();
        assert!(matches!(
            filtering.ty,
            BindingType::Sampler(SamplerBindingType::Filtering)
        ));
        let comparison = layout_entry(&binding(BindingKind::SamplerComparison, 17), 0).unwrap();
        assert!(matches!(
            comparison.ty,
            BindingType::Sampler(SamplerBindingType::Comparison)
        ));
    }

    #[test]
    fn depth_and_array_textures_keep_their_view_and_sample_types() {
        let mut depth = binding(BindingKind::Texture, 34);
        depth.sample = "depth".into();
        let entry = layout_entry(&depth, 1).unwrap();
        assert!(matches!(
            entry.ty,
            BindingType::Texture {
                sample_type: TextureSampleType::Depth,
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            }
        ));
        let mut array = binding(BindingKind::Texture, 33);
        array.dim = "2d-array".into();
        array.sample = "uint".into();
        let entry = layout_entry(&array, 1).unwrap();
        assert!(matches!(
            entry.ty,
            BindingType::Texture {
                sample_type: TextureSampleType::Uint,
                view_dimension: TextureViewDimension::D2Array,
                ..
            }
        ));
    }

    #[test]
    fn storage_textures_map_the_wgsl_format_and_access() {
        let mut storage = binding(BindingKind::StorageTexture, 36);
        storage.format = Some("rgba8unorm".into());
        storage.access = Some("read-write".into());
        let entry = layout_entry(&storage, 1).unwrap();
        assert!(matches!(
            entry.ty,
            BindingType::StorageTexture {
                access: StorageTextureAccess::ReadWrite,
                format: TextureFormat::Rgba8Unorm,
                view_dimension: TextureViewDimension::D2,
            }
        ));
        let mut unknown = storage.clone();
        unknown.format = Some("nope".into());
        assert!(layout_entry(&unknown, 1).is_none());
    }

    #[test]
    fn unknown_texture_formats_are_rejected() {
        assert_eq!(
            parse_format("rgba32float"),
            Some(TextureFormat::Rgba32Float)
        );
        assert_eq!(parse_format("bc7-rgba-unorm"), None);
    }
}
