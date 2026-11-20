//! Descritor D3D11 de sampler, conforme `makeSampler` do worker original.

use super::pipeline_state::compare;
use anyhow::{Result, ensure};
use wgpu::{AddressMode, CompareFunction, FilterMode, MipmapFilterMode, SamplerDescriptor};

const ADDRESS: [AddressMode; 6] = [
    AddressMode::Repeat,
    AddressMode::Repeat,
    AddressMode::MirrorRepeat,
    AddressMode::ClampToEdge,
    AddressMode::ClampToEdge,
    AddressMode::MirrorRepeat,
];

pub fn descriptor(words: &[u32]) -> Result<SamplerDescriptor<'static>> {
    ensure!(words.len() >= 13, "Descritor de sampler truncado");
    let min_lod = f32::from_bits(words[11]);
    let max_lod = f32::from_bits(words[12]);
    ensure!(
        min_lod.is_finite() && max_lod.is_finite(),
        "LOD de sampler não finito"
    );
    let filter = words[0];
    let address = |mode: u32| {
        ADDRESS
            .get(mode as usize)
            .copied()
            .unwrap_or(AddressMode::Repeat)
    };
    let anisotropic = filter & 0x7f == 0x55;
    let lod_min_clamp = min_lod.max(0.0);
    let linear = |bit| {
        if anisotropic || filter & bit != 0 {
            FilterMode::Linear
        } else {
            FilterMode::Nearest
        }
    };
    // shortcut: border/MirrorOnce e MipLODBias seguem as aproximações do JS; rever ao ampliar a paridade D3D11.
    Ok(SamplerDescriptor {
        label: Some("wgr-player-sampler"),
        address_mode_u: address(words[1]),
        address_mode_v: address(words[2]),
        address_mode_w: address(words[3]),
        mag_filter: linear(0x4),
        min_filter: linear(0x10),
        mipmap_filter: if anisotropic || filter & 1 != 0 {
            MipmapFilterMode::Linear
        } else {
            MipmapFilterMode::Nearest
        },
        lod_min_clamp,
        lod_max_clamp: max_lod.clamp(0.0, 32.0).max(lod_min_clamp),
        compare: (filter & 0x80 != 0).then(|| compare(words[6]).unwrap_or(CompareFunction::Always)),
        anisotropy_clamp: if anisotropic {
            words[5].clamp(1, 16) as u16
        } else {
            1
        },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(filter: u32) -> [u32; 13] {
        let mut words = [0; 13];
        words[0] = filter;
        words[1..4].fill(1);
        words[12] = f32::MAX.to_bits();
        words
    }

    #[test]
    fn uses_the_d3d_descriptor_instead_of_wgpu_defaults() {
        let sampler = descriptor(&words(0)).unwrap();
        assert_eq!(sampler.address_mode_u, AddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, AddressMode::Repeat);
        assert_eq!(sampler.address_mode_w, AddressMode::Repeat);
        assert_eq!(sampler.mag_filter, FilterMode::Nearest);
        assert_eq!(sampler.min_filter, FilterMode::Nearest);
        assert_eq!(sampler.mipmap_filter, MipmapFilterMode::Nearest);
        assert_eq!(sampler.lod_min_clamp, 0.0);
        assert_eq!(sampler.lod_max_clamp, 32.0);
        assert_eq!(sampler.compare, None);
        assert_eq!(sampler.anisotropy_clamp, 1);
    }

    #[test]
    fn decodes_each_filter_bit_independently() {
        for (filter, min, mag, mip) in [
            (0x10, true, false, false),
            (0x4, false, true, false),
            (1, false, false, true),
            (0x15, true, true, true),
        ] {
            let sampler = descriptor(&words(filter)).unwrap();
            assert_eq!(sampler.min_filter == FilterMode::Linear, min);
            assert_eq!(sampler.mag_filter == FilterMode::Linear, mag);
            assert_eq!(sampler.mipmap_filter == MipmapFilterMode::Linear, mip);
        }
    }

    #[test]
    fn keeps_the_web_ports_addressing_approximations() {
        for (mode, expected) in [
            (0, AddressMode::Repeat),
            (1, AddressMode::Repeat),
            (2, AddressMode::MirrorRepeat),
            (3, AddressMode::ClampToEdge),
            (4, AddressMode::ClampToEdge),
            (5, AddressMode::MirrorRepeat),
            (99, AddressMode::Repeat),
        ] {
            let mut words = words(0);
            words[1..4].fill(mode);
            let sampler = descriptor(&words).unwrap();
            assert_eq!(sampler.address_mode_u, expected);
            assert_eq!(sampler.address_mode_v, expected);
            assert_eq!(sampler.address_mode_w, expected);
        }
    }

    #[test]
    fn handles_comparison_and_anisotropy() {
        for (function, expected) in [
            (0, CompareFunction::Always),
            (2, CompareFunction::Less),
            (4, CompareFunction::LessEqual),
            (99, CompareFunction::Always),
        ] {
            let mut words = words(0x80);
            words[6] = function;
            assert_eq!(descriptor(&words).unwrap().compare, Some(expected));
        }
        for (requested, expected) in [(0, 1), (8, 8), (u32::MAX, 16)] {
            let mut words = words(0xd5);
            words[5] = requested;
            let sampler = descriptor(&words).unwrap();
            assert_eq!(sampler.anisotropy_clamp, expected);
            assert_eq!(sampler.min_filter, FilterMode::Linear);
            assert_eq!(sampler.mag_filter, FilterMode::Linear);
            assert_eq!(sampler.mipmap_filter, MipmapFilterMode::Linear);
            assert_eq!(sampler.compare, Some(CompareFunction::Always));
        }
    }

    #[test]
    fn clamps_lod_and_rejects_truncation_or_non_finite_values() {
        for (min, max, expected) in [
            (-3.0f32, 50.0f32, (0.0, 32.0)),
            (5.0, 2.0, (5.0, 5.0)),
            (40.0, 60.0, (40.0, 40.0)),
        ] {
            let mut words = words(0);
            words[11] = min.to_bits();
            words[12] = max.to_bits();
            let sampler = descriptor(&words).unwrap();
            assert_eq!((sampler.lod_min_clamp, sampler.lod_max_clamp), expected);
        }
        for length in 0..13 {
            assert!(descriptor(&[0; 13][..length]).is_err());
        }
        for index in [11, 12] {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut words = words(0);
                words[index] = value.to_bits();
                assert!(descriptor(&words).is_err());
            }
        }
    }
}
