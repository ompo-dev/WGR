//! Montagem do descritor de pipeline (layout de vértice, alvos, profundidade e
//! bind group layouts), transcrita de `pipelineDescFor` do `wgpu_worker.js`.
//! A função é pura: o worker só entrega o resultado ao wgpu.

use crate::gpu::assembly::{BufferPlan, LayoutElement, ShaderInput, vertex_buffers};
use crate::gpu::layout::layout_entry;
use crate::gpu::pipeline_state::{blend_for, primitive_state, stencil_faces};
use crate::shader::{Binding, ShaderInfo};
use wgpu::{
    BindGroupLayoutEntry, ColorTargetState, CompareFunction, DepthBiasState, DepthStencilState,
    MultisampleState, PrimitiveState, StencilState, TextureFormat,
};

pub struct PipelineInput<'a> {
    pub vs_label: String,
    pub ps_label: Option<String>,
    pub vs: &'a ShaderInfo,
    pub ps: Option<&'a ShaderInfo>,
    pub inputs: &'a [ShaderInput],
    pub elements: &'a [LayoutElement],
    pub strides: &'a [u32],
    pub topology: u32,
    pub strip_format: Option<wgpu::IndexFormat>,
    pub blend_words: Option<&'a [u32]>,
    pub depth_words: Option<&'a [u32]>,
    pub raster_words: Option<&'a [u32]>,
    pub targets: &'a [Option<TextureFormat>],
    pub samples: u32,
    pub depth_format: Option<TextureFormat>,
}

#[derive(Debug, Clone)]
pub struct PipelinePlan {
    pub label: String,
    pub vertex_buffers: Vec<Option<BufferPlan>>,
    pub primitive: PrimitiveState,
    pub depth_stencil: Option<DepthStencilState>,
    pub targets: Vec<Option<ColorTargetState>>,
    pub multisample: MultisampleState,
    pub group0: Vec<BindGroupLayoutEntry>,
    pub group1: Vec<BindGroupLayoutEntry>,
}

fn entries_for(bindings: &[Binding], group: u32, stage: u8) -> Vec<BindGroupLayoutEntry> {
    bindings
        .iter()
        .filter(|binding| binding.group == group)
        .filter_map(|binding| layout_entry(binding, stage))
        .collect()
}

pub fn plan(input: &PipelineInput<'_>) -> PipelinePlan {
    let (topology, strip) = crate::gpu::pipeline_state::topology(input.topology);
    let mut primitive = primitive_state(input.raster_words);
    primitive.topology = topology;
    if strip {
        primitive.strip_index_format = input.strip_format;
    }
    let depth_stencil = input.depth_format.map(|format| {
        let mut depth = DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(CompareFunction::Always),
            stencil: StencilState::default(),
            bias: DepthBiasState::default(),
        };
        if let Some(words) = input.depth_words {
            let get = |index: usize| words.get(index).copied().unwrap_or(0);
            if get(0) != 0 {
                depth.depth_write_enabled = Some(get(1) == 1);
                depth.depth_compare = Some(
                    crate::gpu::pipeline_state::compare(get(2)).unwrap_or(CompareFunction::Less),
                );
            }
            if format.has_stencil_aspect()
                && let Some((front, back, read_mask, write_mask)) = stencil_faces(words)
            {
                depth.stencil = StencilState {
                    front,
                    back,
                    read_mask,
                    write_mask,
                };
            }
        }
        depth
    });
    let outputs = input
        .ps
        .map(|shader| shader.outputs.as_slice())
        .unwrap_or(&[]);
    let targets = input
        .targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            target.map(|format| {
                if !outputs.contains(&(index as u32)) {
                    return ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::empty(),
                    };
                }
                let (blend, write_mask) = match input.blend_words {
                    Some(words) => blend_for(words, index),
                    None => (None, wgpu::ColorWrites::ALL),
                };
                ColorTargetState {
                    format,
                    blend,
                    write_mask,
                }
            })
        })
        .collect();
    let samples = input.samples.max(1);
    PipelinePlan {
        label: match &input.ps_label {
            Some(ps) => format!("{} + {ps}", input.vs_label),
            None => format!("{} + -", input.vs_label),
        },
        vertex_buffers: vertex_buffers(input.inputs, input.elements, input.strides),
        primitive,
        depth_stencil,
        targets,
        multisample: MultisampleState {
            count: samples,
            mask: !0,
            alpha_to_coverage_enabled: input
                .blend_words
                .is_some_and(|words| words.first().copied().unwrap_or(0) != 0)
                && samples == 1,
        },
        group0: entries_for(&input.vs.bindings, 0, 0),
        group1: input
            .ps
            .map(|shader| entries_for(&shader.bindings, 1, 1))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::assembly::{parse_layout, parse_shader};
    use crate::gpu::formats::map;
    use crate::shader::parse_wgsl;

    fn shader() -> ShaderInfo {
        parse_wgsl(
            r#"
@group(1u) @binding(0u) var<uniform> cb : vec4<f32>;
@group(1u) @binding(16u) var s0 : sampler;
@group(1u) @binding(32u) var t0 : texture_2d<f32>;
struct Out { @location(0u) color : vec4<f32>, }
@fragment fn main() -> Out { return Out(vec4<f32>(1.0)); }
"#,
        )
    }

    #[test]
    fn builds_targets_and_skips_slots_the_pixel_shader_does_not_write() {
        let display = map(28, 0).format;
        let aux = TextureFormat::Rgba16Float;
        let pipeline = plan(&PipelineInput {
            vs_label: "ped::VS".into(),
            ps_label: Some("ped::PS".into()),
            vs: &shader(),
            ps: Some(&shader()),
            inputs: &[],
            elements: &[],
            strides: &[],
            topology: 4,
            strip_format: None,
            blend_words: None,
            depth_words: None,
            raster_words: None,
            targets: &[Some(display), Some(aux)],
            samples: 1,
            depth_format: Some(TextureFormat::Depth32Float),
        });
        assert_eq!(pipeline.label, "ped::VS + ped::PS");
        assert_eq!(pipeline.targets.len(), 2);
        assert_eq!(pipeline.targets[0].as_ref().unwrap().format, display);
        assert_eq!(
            pipeline.targets[0].as_ref().unwrap().write_mask,
            wgpu::ColorWrites::ALL
        );
        assert_eq!(
            pipeline.targets[1].as_ref().unwrap().write_mask,
            wgpu::ColorWrites::empty()
        );
        assert_eq!(
            pipeline.depth_stencil.as_ref().unwrap().format,
            TextureFormat::Depth32Float
        );
        assert_eq!(pipeline.group1.len(), 3);
    }

    #[test]
    fn includes_the_vertex_layout_matched_by_semantics() {
        let (_, shader) =
            parse_shader(&[7, 0, 0x9df6_27f5, 0x0289_2187, 1, 2013849738, 0, 7, 0xf]).unwrap();
        let (_, elements) = parse_layout(&[9, 1, 2013849738, 0, 6, 0, 0, 0, 0]).unwrap();
        let info = shader_info_without_outputs();
        let pipeline = plan(&PipelineInput {
            vs_label: "vs".into(),
            ps_label: None,
            vs: &info,
            ps: None,
            inputs: &shader.inputs,
            elements: &elements,
            strides: &[36],
            topology: 4,
            strip_format: None,
            blend_words: None,
            depth_words: None,
            raster_words: None,
            targets: &[],
            samples: 1,
            depth_format: None,
        });
        let buffer = pipeline.vertex_buffers[0].as_ref().unwrap();
        assert_eq!(buffer.array_stride, 36);
        assert_eq!(buffer.attributes[0].shader_location, 7);
        assert_eq!(buffer.attributes[0].format, wgpu::VertexFormat::Float32x3);
        assert!(pipeline.targets.is_empty());
        assert!(pipeline.depth_stencil.is_none());
    }

    fn shader_info_without_outputs() -> ShaderInfo {
        ShaderInfo {
            bindings: Vec::new(),
            outputs: Vec::new(),
        }
    }
}
