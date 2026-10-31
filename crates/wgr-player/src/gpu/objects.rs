//! Leitura dos ops de criação de recursos do ring (`CREATE_TEXTURE`,
//! `CREATE_BUFFER`, `CREATE_SRV`, `CREATE_UAV` e `CREATE_STATE`), com os mesmos
//! campos que o `wgpu_worker.js` consome.

use wgpu::{TextureDimension, TextureViewDimension};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureRequest {
    pub id: u32,
    pub dimension: TextureDimension,
    pub width: u32,
    pub height: u32,
    pub depth_or_array_layers: u32,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub dxgi: u32,
    pub bind: u32,
    pub samples: u32,
}

pub fn parse_texture(words: &[u32]) -> Option<TextureRequest> {
    let get = |index: usize| words.get(index).copied();
    Some(TextureRequest {
        id: get(0)?,
        dimension: match get(1)? {
            1 => TextureDimension::D1,
            3 => TextureDimension::D3,
            _ => TextureDimension::D2,
        },
        width: get(2)?,
        height: get(3)?,
        depth_or_array_layers: get(4)?,
        mip_levels: get(5)?.max(1),
        array_layers: get(6)?.max(1),
        dxgi: get(7)?,
        bind: get(8)?,
        samples: get(9)?.max(1),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferRequest {
    pub id: u32,
    pub size: u64,
    pub bind: u32,
    pub misc: u32,
    pub stride: u32,
}

pub fn parse_buffer(words: &[u32]) -> Option<BufferRequest> {
    let get = |index: usize| words.get(index).copied();
    Some(BufferRequest {
        id: get(0)?,
        size: u64::from(get(1)?),
        bind: get(2)?,
        misc: get(3)?,
        stride: get(4)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewRequest {
    pub id: u32,
    pub resource: u32,
    pub is_buffer: bool,
    pub dxgi: u32,
    pub dimension: TextureViewDimension,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
    pub flags: u32,
}

fn view_dimension(value: u32) -> TextureViewDimension {
    match value {
        1 => TextureViewDimension::D1,
        3 => TextureViewDimension::D3,
        4 => TextureViewDimension::Cube,
        5 => TextureViewDimension::CubeArray,
        6 => TextureViewDimension::D2Array,
        _ => TextureViewDimension::D2,
    }
}

fn parse_view(words: &[u32], with_flags: bool) -> Option<ViewRequest> {
    let get = |index: usize| words.get(index).copied();
    Some(ViewRequest {
        id: get(0)?,
        is_buffer: get(1)? == 1,
        resource: get(2)?,
        dxgi: get(3)?,
        dimension: view_dimension(get(4)?),
        a: get(5)?,
        b: get(6)?,
        c: get(7)?,
        d: if with_flags { 0 } else { get(8)? },
        flags: if with_flags { get(8)? } else { 0 },
    })
}

/// `CREATE_SRV`: id, resKind, resId, dxgi, viewDim, a, b, c, d.
pub fn parse_srv(words: &[u32]) -> Option<ViewRequest> {
    parse_view(words, false)
}

/// `CREATE_UAV`: id, resKind, resId, dxgi, viewDim, a, b, c, flags.
pub fn parse_uav(words: &[u32]) -> Option<ViewRequest> {
    parse_view(words, true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateRequest {
    pub id: u32,
    pub kind: u32,
    pub first_word: u32,
}

pub fn parse_state(words: &[u32]) -> Option<StateRequest> {
    Some(StateRequest {
        id: *words.first()?,
        kind: *words.get(1)?,
        first_word: words.get(2).copied().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_texture_creation_payload() {
        let request = parse_texture(&[7, 2, 1920, 1200, 1, 1, 1, 28, 0x8, 1]).unwrap();
        assert_eq!(request.id, 7);
        assert_eq!(request.dimension, TextureDimension::D2);
        assert_eq!(request.width, 1920);
        assert_eq!(request.height, 1200);
        assert_eq!(request.dxgi, 28);
        assert_eq!(request.bind, 0x8);
        assert_eq!(request.samples, 1);
    }

    #[test]
    fn parses_a_buffer_creation_payload() {
        let request = parse_buffer(&[3, 65536, 0x4, 0, 256]).unwrap();
        assert_eq!(request.id, 3);
        assert_eq!(request.size, 65536);
        assert_eq!(request.bind, 0x4);
        assert_eq!(request.stride, 256);
    }

    #[test]
    fn parses_srv_and_uav_ranges() {
        let srv = parse_srv(&[11, 0, 7, 71, 6, 0, 1, 0, 0]).unwrap();
        assert_eq!(srv.id, 11);
        assert_eq!(srv.resource, 7);
        assert!(!srv.is_buffer);
        assert_eq!(srv.dimension, TextureViewDimension::D2Array);
        let uav = parse_uav(&[12, 1, 3, 28, 2, 0, 0, 0, 1]).unwrap();
        assert!(uav.is_buffer);
        assert_eq!(uav.flags, 1);
    }

    #[test]
    fn rejects_truncated_payloads() {
        assert!(parse_texture(&[1, 2]).is_none());
        assert!(parse_buffer(&[1]).is_none());
        assert!(parse_srv(&[1, 0]).is_none());
    }
}
