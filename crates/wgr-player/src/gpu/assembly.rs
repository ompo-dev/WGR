//! Montagem do layout de vértice a partir dos ops `CREATE_SHADER` e
//! `CREATE_LAYOUT` do ring, transcrita de `createShader`, `createLayout`,
//! `layoutSlots` e `vertexBuffersFor` do `wgpu_worker.js`.

use crate::gpu::pipeline_state::vertex_format;
use wgpu::{VertexAttribute, VertexBufferLayout, VertexFormat, VertexStepMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderInput {
    pub semantic: u32,
    pub index: u32,
    pub register: u32,
    pub mask: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shader {
    pub stage: u32,
    pub hash: String,
    pub inputs: Vec<ShaderInput>,
}

/// `CREATE_SHADER`: id, estágio, hash baixo, hash alto, contagem e
/// (semântica, índice, registrador, máscara) por entrada.
pub fn parse_shader(words: &[u32]) -> Option<(u32, Shader)> {
    let id = *words.first()?;
    let stage = *words.get(1)?;
    let low = *words.get(2)?;
    let high = *words.get(3)?;
    let count = *words.get(4)? as usize;
    if count > 32 {
        return None;
    }
    let mut inputs = Vec::with_capacity(count);
    for i in 0..count {
        let at = 5 + i * 4;
        let semantic = *words.get(at)?;
        let index = *words.get(at + 1)?;
        let register = *words.get(at + 2)?;
        let mask = *words.get(at + 3)?;
        inputs.push(ShaderInput {
            semantic,
            index,
            register,
            mask,
        });
    }
    Some((
        id,
        Shader {
            stage,
            hash: format!("{high:08x}{low:08x}"),
            inputs,
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutElement {
    pub semantic: u32,
    pub index: u32,
    pub format: u32,
    pub slot: u32,
    pub offset: u32,
    pub class: u32,
    pub step_rate: u32,
}

/// `CREATE_LAYOUT`: id, contagem e sete palavras por elemento.
pub fn parse_layout(words: &[u32]) -> Option<(u32, Vec<LayoutElement>)> {
    let id = *words.first()?;
    let count = *words.get(1)? as usize;
    if count > 64 {
        return None;
    }
    let mut elements = Vec::with_capacity(count);
    for i in 0..count {
        let at = 2 + i * 7;
        elements.push(LayoutElement {
            semantic: *words.get(at)?,
            index: *words.get(at + 1)?,
            format: *words.get(at + 2)?,
            slot: *words.get(at + 3)?,
            offset: *words.get(at + 4)?,
            class: *words.get(at + 5)?,
            step_rate: *words.get(at + 6)?,
        });
    }
    Some((id, elements))
}

/// Cada slot de vertex buffer uma vez, com a classe do primeiro elemento.
pub fn layout_slots(elements: &[LayoutElement]) -> Vec<(u32, u32)> {
    let mut seen = 0u32;
    let mut slots = Vec::new();
    for element in elements {
        if element.slot < 32 && (seen >> element.slot) & 1 == 0 {
            seen |= 1 << element.slot;
            slots.push((element.slot, element.class));
        }
    }
    slots
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributePlan {
    pub shader_location: u32,
    pub offset: u64,
    pub format: VertexFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferPlan {
    pub array_stride: u64,
    pub step_mode: VertexStepMode,
    pub attributes: Vec<AttributePlan>,
}

impl BufferPlan {
    pub fn attributes(&self) -> Vec<VertexAttribute> {
        self.attributes
            .iter()
            .map(|attribute| VertexAttribute {
                format: attribute.format,
                offset: attribute.offset,
                shader_location: attribute.shader_location,
            })
            .collect()
    }

    pub fn layout<'a>(&self, attributes: &'a [VertexAttribute]) -> VertexBufferLayout<'a> {
        VertexBufferLayout {
            array_stride: self.array_stride,
            step_mode: self.step_mode,
            attributes,
        }
    }
}

/// Junta a assinatura do vertex shader com o layout de entrada por semântica,
/// como `vertexBuffersFor`. Elementos que o shader não lê são ignorados.
pub fn vertex_buffers(
    inputs: &[ShaderInput],
    elements: &[LayoutElement],
    strides: &[u32],
) -> Vec<Option<BufferPlan>> {
    let mut by_slot: Vec<Option<BufferPlan>> = Vec::new();
    for element in elements {
        let Some(input) = inputs
            .iter()
            .find(|input| input.semantic == element.semantic && input.index == element.index)
        else {
            continue;
        };
        let Some(format) = vertex_format(element.format) else {
            continue;
        };
        let slot = element.slot as usize;
        if slot >= 32 {
            continue;
        }
        if by_slot.len() <= slot {
            by_slot.resize_with(slot + 1, || None);
        }
        let entry = by_slot[slot].get_or_insert_with(|| BufferPlan {
            array_stride: u64::from(strides.get(slot).copied().unwrap_or(0)),
            step_mode: if element.class == 1 {
                VertexStepMode::Instance
            } else {
                VertexStepMode::Vertex
            },
            attributes: Vec::new(),
        });
        entry.attributes.push(AttributePlan {
            shader_location: input.register,
            offset: u64::from(element.offset),
            format,
        });
    }
    by_slot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader_payload() -> Vec<u32> {
        vec![
            7,
            0,
            0x9df6_27f5,
            0x0289_2187,
            2,
            2013849738,
            0,
            7,
            0xf,
            1009949074,
            0,
            3,
            0xf,
        ]
    }

    fn layout_payload() -> Vec<u32> {
        vec![
            9, 2, 2013849738, 0, 6, 0, 0, 0, 0, 1009949074, 0, 28, 1, 12, 0, 0,
        ]
    }

    #[test]
    fn parses_the_shader_signature_and_hash_like_the_worker() {
        let (id, shader) = parse_shader(&shader_payload()).unwrap();
        assert_eq!(id, 7);
        assert_eq!(shader.stage, 0);
        assert_eq!(shader.hash, "028921879df627f5");
        assert_eq!(shader.inputs.len(), 2);
        assert_eq!(shader.inputs[0].register, 7);
        assert_eq!(shader.inputs[1].semantic, 1009949074);
    }

    #[test]
    fn parses_each_input_layout_element() {
        let (id, elements) = parse_layout(&layout_payload()).unwrap();
        assert_eq!(id, 9);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].format, 6);
        assert_eq!(elements[1].slot, 1);
        assert_eq!(elements[1].offset, 12);
        assert_eq!(layout_slots(&elements), vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn pairs_semantics_with_the_shader_registers() {
        let (_, shader) = parse_shader(&shader_payload()).unwrap();
        let (_, elements) = parse_layout(&layout_payload()).unwrap();
        let strides = [36u32, 16];
        let buffers = vertex_buffers(&shader.inputs, &elements, &strides);
        assert_eq!(buffers.len(), 2);
        let slot0 = buffers[0].as_ref().unwrap();
        assert_eq!(slot0.array_stride, 36);
        assert_eq!(slot0.attributes.len(), 1);
        assert_eq!(slot0.attributes[0].shader_location, 7);
        assert_eq!(slot0.attributes[0].format, VertexFormat::Float32x3);
        let slot1 = buffers[1].as_ref().unwrap();
        assert_eq!(slot1.attributes[0].format, VertexFormat::Unorm8x4);
        assert_eq!(slot1.attributes[0].offset, 12);
    }

    #[test]
    fn ignores_elements_the_shader_does_not_read() {
        let (_, shader) = parse_shader(&shader_payload()).unwrap();
        let mut elements = parse_layout(&layout_payload()).unwrap().1;
        elements.push(LayoutElement {
            semantic: 42,
            index: 0,
            format: 2,
            slot: 2,
            offset: 0,
            class: 0,
            step_rate: 0,
        });
        let buffers = vertex_buffers(&shader.inputs, &elements, &[36, 16, 16]);
        assert_eq!(buffers.len(), 2);
    }
}
