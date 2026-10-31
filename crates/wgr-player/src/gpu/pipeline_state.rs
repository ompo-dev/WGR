//! Conversão das palavras de estado D3D11 para os tipos de pipeline do wgpu,
//! transcrita de `pipelineDescFor` e dos helpers do `wgpu_worker.js`.

use wgpu::{
    BlendComponent, BlendFactor, BlendOperation, BlendState, ColorWrites, CompareFunction, Face,
    FrontFace, PrimitiveState, PrimitiveTopology, StencilFaceState, StencilOperation, VertexFormat,
};

pub fn compare(value: u32) -> Option<CompareFunction> {
    use CompareFunction as C;
    Some(match value {
        1 => C::Never,
        2 => C::Less,
        3 => C::Equal,
        4 => C::LessEqual,
        5 => C::Greater,
        6 => C::NotEqual,
        7 => C::GreaterEqual,
        8 => C::Always,
        _ => return None,
    })
}

pub fn stencil_operation(value: u32) -> StencilOperation {
    use StencilOperation as S;
    match value {
        2 => S::Zero,
        3 => S::Replace,
        4 => S::IncrementClamp,
        5 => S::DecrementClamp,
        6 => S::Invert,
        7 => S::IncrementWrap,
        8 => S::DecrementWrap,
        _ => S::Keep,
    }
}

pub fn blend_factor(value: u32, default: BlendFactor) -> BlendFactor {
    use BlendFactor as B;
    match value {
        1 => B::Zero,
        2 => B::One,
        3 => B::Src,
        4 => B::OneMinusSrc,
        5 => B::SrcAlpha,
        6 => B::OneMinusSrcAlpha,
        7 => B::DstAlpha,
        8 => B::OneMinusDstAlpha,
        9 => B::Dst,
        10 => B::OneMinusDst,
        11 => B::SrcAlphaSaturated,
        14 => B::Constant,
        15 => B::OneMinusConstant,
        16 => B::Src1,
        17 => B::OneMinusSrc1,
        18 => B::Src1Alpha,
        19 => B::OneMinusSrc1Alpha,
        _ => default,
    }
}

pub fn blend_operation(value: u32) -> BlendOperation {
    use BlendOperation as O;
    match value {
        2 => O::Subtract,
        3 => O::ReverseSubtract,
        4 => O::Min,
        5 => O::Max,
        _ => O::Add,
    }
}

/// Topologia D3D11 e se o formato do índice de strip vale para ela.
pub fn topology(value: u32) -> (PrimitiveTopology, bool) {
    use PrimitiveTopology as P;
    match value {
        1 => (P::PointList, false),
        2 => (P::LineList, false),
        3 => (P::LineStrip, true),
        5 => (P::TriangleStrip, true),
        _ => (P::TriangleList, false),
    }
}

pub fn vertex_format(dxgi: u32) -> Option<VertexFormat> {
    use VertexFormat as V;
    Some(match dxgi {
        2 => V::Float32x4,
        6 => V::Float32x3,
        16 => V::Float32x2,
        41 => V::Float32,
        3 => V::Uint32x4,
        7 => V::Uint32x3,
        17 => V::Uint32x2,
        42 => V::Uint32,
        4 => V::Sint32x4,
        8 => V::Sint32x3,
        18 => V::Sint32x2,
        43 => V::Sint32,
        10 => V::Float16x4,
        34 => V::Float16x2,
        11 => V::Unorm16x4,
        35 => V::Unorm16x2,
        12 => V::Uint16x4,
        36 => V::Uint16x2,
        13 => V::Snorm16x4,
        37 => V::Snorm16x2,
        14 => V::Sint16x4,
        38 => V::Sint16x2,
        28 => V::Unorm8x4,
        30 => V::Uint8x4,
        31 => V::Snorm8x4,
        32 => V::Sint8x4,
        49 => V::Unorm8x2,
        50 => V::Uint8x2,
        51 => V::Snorm8x2,
        52 => V::Sint8x2,
        87 => V::Unorm8x4Bgra,
        24 => V::Unorm10_10_10_2,
        _ => return None,
    })
}

/// Estado de blend do alvo `rt`, com a máscara de escrita de cor.
pub fn blend_for(words: &[u32], rt: usize) -> (Option<BlendState>, ColorWrites) {
    let get = |index: usize| words.get(index).copied().unwrap_or(0);
    let target = if get(1) != 0 { rt } else { 0 };
    let at = 2 + target * 8;
    let write = ColorWrites::from_bits_truncate(get(at + 7) & 0xf);
    if get(at) == 0 {
        return (None, write);
    }
    (
        Some(BlendState {
            color: BlendComponent {
                src_factor: blend_factor(get(at + 1), BlendFactor::One),
                dst_factor: blend_factor(get(at + 2), BlendFactor::Zero),
                operation: blend_operation(get(at + 3)),
            },
            alpha: BlendComponent {
                src_factor: blend_factor(get(at + 4), BlendFactor::One),
                dst_factor: blend_factor(get(at + 5), BlendFactor::Zero),
                operation: blend_operation(get(at + 6)),
            },
        }),
        write,
    )
}

/// Estado de raster: culling e ordem dos vértices.
pub fn primitive_state(words: Option<&[u32]>) -> PrimitiveState {
    let mut primitive = PrimitiveState::default();
    match words {
        Some(words) => {
            let get = |index: usize| words.get(index).copied().unwrap_or(0);
            primitive.cull_mode = match get(1) {
                2 => Some(Face::Front),
                3 => Some(Face::Back),
                _ => None,
            };
            primitive.front_face = if get(2) != 0 {
                FrontFace::Ccw
            } else {
                FrontFace::Cw
            };
        }
        None => {
            primitive.cull_mode = Some(Face::Back);
            primitive.front_face = FrontFace::Cw;
        }
    }
    primitive
}

/// Frente/verso do estêncil, apenas quando o alvo tem estêncil.
pub fn stencil_faces(words: &[u32]) -> Option<(StencilFaceState, StencilFaceState, u32, u32)> {
    let get = |index: usize| words.get(index).copied().unwrap_or(0);
    if get(3) == 0 {
        return None;
    }
    let front = StencilFaceState {
        fail_op: stencil_operation(get(5)),
        depth_fail_op: stencil_operation(get(6)),
        pass_op: stencil_operation(get(7)),
        compare: compare(get(8)).unwrap_or(CompareFunction::Always),
    };
    let back = StencilFaceState {
        fail_op: stencil_operation(get(9)),
        depth_fail_op: stencil_operation(get(10)),
        pass_op: stencil_operation(get(11)),
        compare: compare(get(12)).unwrap_or(CompareFunction::Always),
    };
    Some((front, back, get(4) & 0xff, (get(4) >> 8) & 0xff))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_d3d_comparison_and_topology_tables() {
        assert_eq!(compare(2), Some(CompareFunction::Less));
        assert_eq!(compare(8), Some(CompareFunction::Always));
        assert_eq!(compare(0), None);
        assert_eq!(topology(5).0, PrimitiveTopology::TriangleStrip);
        assert!(topology(5).1);
        assert_eq!(topology(4).0, PrimitiveTopology::TriangleList);
    }

    #[test]
    fn maps_vertex_formats_used_by_ped_vertices() {
        assert_eq!(vertex_format(28), Some(VertexFormat::Unorm8x4));
        assert_eq!(vertex_format(87), Some(VertexFormat::Unorm8x4Bgra));
        assert_eq!(vertex_format(24), Some(VertexFormat::Unorm10_10_10_2));
        assert_eq!(vertex_format(6), Some(VertexFormat::Float32x3));
        assert_eq!(vertex_format(999), None);
    }

    #[test]
    fn blend_target_zero_uses_components_and_write_mask() {
        let mut words = vec![0u32; 10];
        words[2] = 1;
        words[3] = 2;
        words[4] = 1;
        words[5] = 0;
        words[6] = 2;
        words[7] = 1;
        words[8] = 0;
        words[9] = 0xf;
        let (blend, write) = blend_for(&words, 0);
        let blend = blend.expect("blend enabled");
        assert_eq!(blend.color.src_factor, BlendFactor::One);
        assert_eq!(blend.color.dst_factor, BlendFactor::Zero);
        assert_eq!(blend.alpha.src_factor, BlendFactor::One);
        assert_eq!(write, ColorWrites::ALL);
    }

    #[test]
    fn independent_targets_select_their_own_words() {
        let mut words = vec![0u32; 26];
        words[1] = 1;
        words[2 + 8] = 1;
        words[2 + 8 + 1] = 5;
        words[2 + 8 + 2] = 6;
        words[2 + 8 + 7] = 0x3;
        let (blend, write) = blend_for(&words, 1);
        let blend = blend.expect("target one blend");
        assert_eq!(blend.color.src_factor, BlendFactor::SrcAlpha);
        assert_eq!(blend.color.dst_factor, BlendFactor::OneMinusSrcAlpha);
        assert_eq!(write, ColorWrites::RED | ColorWrites::GREEN);
        assert!(blend_for(&words, 0).0.is_none());
    }

    #[test]
    fn stencil_faces_are_only_present_when_enabled() {
        let mut words = vec![0u32; 13];
        assert!(stencil_faces(&words).is_none());
        words[3] = 1;
        words[4] = 0xff | (0x0f << 8);
        words[5] = 3;
        words[8] = 8;
        let (front, back, read, write) = stencil_faces(&words).unwrap();
        assert_eq!(front.fail_op, StencilOperation::Replace);
        assert_eq!(front.compare, CompareFunction::Always);
        assert_eq!(back.compare, CompareFunction::Always);
        assert_eq!(read, 0xff);
        assert_eq!(write, 0x0f);
    }
}
