//! Tabela DXGI → WebGPU, transcrita de `mapFormat` do `wgpu_worker.js`.
//!
//! O web worker declara, para cada formato, o formato principal e os formatos
//! alternativos de view (reinterpretação sRGB). O host nativo precisa dos
//! mesmos `view_formats`; sem eles, uma textura tipeless não pode ser amostrada
//! no espaço de cor que o shader espera.

use wgpu::TextureFormat as T;

/// Bits de uso de textura do protocolo D3D11 emulado.
pub const BIND_SRV: u32 = 0x8;
pub const BIND_RT: u32 = 0x20;
pub const BIND_DS: u32 = 0x40;
pub const BIND_UAV: u32 = 0x80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    pub format: T,
    pub views: Vec<T>,
    pub a8: bool,
}

/// Traduz um `DXGI_FORMAT` para o formato wgpu e as views alternativas.
pub fn map(dxgi: u32, bind: u32) -> Format {
    let ds = bind & BIND_DS != 0;
    let (format, views) = match dxgi {
        2 => (T::Rgba32Float, &[][..]),
        9..=11 => (T::Rgba16Float, &[][..]),
        16 => (T::Rg32Float, &[][..]),
        23 | 24 => (T::Rgb10a2Unorm, &[][..]),
        26 => (T::Rg11b10Ufloat, &[][..]),
        27 | 28 => (T::Rgba8Unorm, &[T::Rgba8UnormSrgb][..]),
        29 => (T::Rgba8UnormSrgb, &[T::Rgba8Unorm][..]),
        90 | 87 | 88 => (T::Bgra8Unorm, &[T::Bgra8UnormSrgb][..]),
        91 => (T::Bgra8UnormSrgb, &[T::Bgra8Unorm][..]),
        33..=35 => (T::Rg16Float, &[][..]),
        39 => (if ds { T::Depth32Float } else { T::R32Float }, &[][..]),
        40 => (T::Depth32Float, &[][..]),
        41 => (T::R32Float, &[][..]),
        19..=20 => (T::Depth32FloatStencil8, &[][..]),
        44..=45 => (T::Depth24PlusStencil8, &[][..]),
        48..=49 => (T::Rg8Unorm, &[][..]),
        53 => (if ds { T::Depth16Unorm } else { T::R16Float }, &[][..]),
        55 => (T::Depth16Unorm, &[][..]),
        54 => (T::R16Float, &[][..]),
        60..=61 => (T::R8Unorm, &[][..]),
        63 => (T::R8Snorm, &[][..]),
        51 => (T::Rg8Snorm, &[][..]),
        31 => (T::Rgba8Snorm, &[][..]),
        67 => (T::Rgb9e5Ufloat, &[][..]),
        65 => (T::Rgba8Unorm, &[][..]),
        56 => (T::Depth16Unorm, &[][..]),
        21 | 22 | 46 | 47 => (T::Depth24PlusStencil8, &[][..]),
        70..=71 => (T::Bc1RgbaUnorm, &[T::Bc1RgbaUnormSrgb][..]),
        72 => (T::Bc1RgbaUnormSrgb, &[T::Bc1RgbaUnorm][..]),
        73..=74 => (T::Bc2RgbaUnorm, &[T::Bc2RgbaUnormSrgb][..]),
        75 => (T::Bc2RgbaUnormSrgb, &[T::Bc2RgbaUnorm][..]),
        76..=77 => (T::Bc3RgbaUnorm, &[T::Bc3RgbaUnormSrgb][..]),
        78 => (T::Bc3RgbaUnormSrgb, &[T::Bc3RgbaUnorm][..]),
        79..=80 => (T::Bc4RUnorm, &[][..]),
        82..=83 => (T::Bc5RgUnorm, &[][..]),
        94..=95 => (T::Bc6hRgbUfloat, &[][..]),
        97..=98 => (T::Bc7RgbaUnorm, &[T::Bc7RgbaUnormSrgb][..]),
        99 => (T::Bc7RgbaUnormSrgb, &[T::Bc7RgbaUnorm][..]),
        other => {
            tracing::warn!(dxgi = other, "Formato DXGI desconhecido; usando rgba8unorm");
            (T::Rgba8Unorm, &[][..])
        }
    };
    Format {
        format,
        views: views.to_vec(),
        a8: dxgi == 65,
    }
}

pub fn is_depth(format: T) -> bool {
    format.is_depth_stencil_format()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typeless_rgba_has_the_srgb_view_the_shader_expects() {
        let mapped = map(27, BIND_SRV);
        assert_eq!(mapped.format, T::Rgba8Unorm);
        assert_eq!(mapped.views, vec![T::Rgba8UnormSrgb]);
        let srgb = map(29, BIND_SRV);
        assert_eq!(srgb.format, T::Rgba8UnormSrgb);
        assert_eq!(srgb.views, vec![T::Rgba8Unorm]);
    }

    #[test]
    fn compressed_formats_keep_their_srgb_view() {
        let bc1 = map(71, BIND_SRV);
        assert_eq!(bc1.format, T::Bc1RgbaUnorm);
        assert_eq!(bc1.views, vec![T::Bc1RgbaUnormSrgb]);
        assert_eq!(map(78, BIND_SRV).format, T::Bc3RgbaUnormSrgb);
        assert_eq!(map(83, BIND_SRV).format, T::Bc5RgUnorm);
        assert_eq!(map(99, BIND_SRV).format, T::Bc7RgbaUnormSrgb);
    }

    #[test]
    fn depth_uses_the_bind_flag_to_disambiguate_typeless() {
        assert_eq!(map(39, BIND_DS).format, T::Depth32Float);
        assert_eq!(map(39, BIND_SRV).format, T::R32Float);
        assert_eq!(map(53, BIND_DS).format, T::Depth16Unorm);
    }

    #[test]
    fn unknown_formats_fall_back_without_panicking() {
        assert_eq!(map(999, 0).format, T::Rgba8Unorm);
    }
}
