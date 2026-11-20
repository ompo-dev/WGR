//! Montagem limitada de uniformes: snapshots CPU antes de cada desenho.

use super::state::{ConstantBuffer, MAX_CBUFFER};
use crate::shader::Pack;
use anyhow::{Context, Result, ensure};

pub const MAX_UNIFORM_BYTES: u64 = 64 * 1024;

pub fn validate_pack(pack: &Pack, limit: u64) -> Result<()> {
    ensure!(
        pack.size > 0 && pack.size <= limit.min(MAX_UNIFORM_BYTES),
        "Pack de uniformes excede o limite ou está vazio"
    );
    ensure!(!pack.members.is_empty(), "Pack sem membros");
    let mut end = 0;
    let mut previous = None;
    for member in &pack.members {
        ensure!(
            (member.binding as usize) < MAX_CBUFFER
                && member.binding != 15
                && previous.is_none_or(|binding| binding < member.binding)
                && member.offset == end
                && member.size > 0
                && member.size.is_multiple_of(16),
            "Layout de uniformes inválido"
        );
        end = end
            .checked_add(member.size)
            .context("Intervalo de uniformes excedido")?;
        ensure!(end <= pack.size, "Membro fora do pack de uniformes");
        previous = Some(member.binding);
    }
    ensure!(end == pack.size, "Tamanho de pack inconsistente");
    Ok(())
}

/// `first` conta registros de 16 bytes. Como no JS, `size` do slot não limita
/// a cópia: vale o tamanho do membro e o que restar no shadow. Ausência vira zero.
pub fn snapshot<'a>(
    pack: &Pack,
    slots: &[ConstantBuffer],
    mut shadow: impl FnMut(u32) -> Option<&'a [u8]>,
    limit: u64,
) -> Result<Vec<u8>> {
    validate_pack(pack, limit)?;
    let mut bytes = vec![0; pack.size as usize];
    for member in &pack.members {
        let slot = slots
            .get(member.binding as usize)
            .copied()
            .unwrap_or_default();
        if slot.buffer == 0 {
            continue;
        }
        let Some(source) = shadow(slot.buffer) else {
            continue;
        };
        let first = u64::from(slot.first) * 16;
        if first >= source.len() as u64 {
            continue;
        }
        let count = member.size.min(source.len() as u64 - first) as usize;
        let start = member.offset as usize;
        bytes[start..start + count]
            .copy_from_slice(&source[first as usize..first as usize + count]);
    }
    Ok(bytes)
}

/// Tabela JSON numérica do pack original: ToUint32, um escalar por vec4.
pub fn inline_constants(json: &str, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        json.len() <= MAX_UNIFORM_BYTES as usize * 4,
        "JSON de constantes excessivo"
    );
    let values: Vec<f64> = serde_json::from_str(json).context("Tabela de constantes inválida")?;
    let size = (values.len().max(1) as u64)
        .checked_mul(16)
        .context("Tabela de constantes excessiva")?;
    ensure!(
        size <= limit.min(MAX_UNIFORM_BYTES),
        "Tabela de constantes excede o limite"
    );
    let mut bytes = vec![0; size as usize];
    for (value, slot) in values.into_iter().zip(bytes.as_chunks_mut::<16>().0) {
        let word = value.trunc().rem_euclid(4294967296.0) as u32;
        slot[..4].copy_from_slice(&word.to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::PackMember;

    fn pack() -> Pack {
        Pack {
            size: 48,
            members: vec![
                PackMember {
                    binding: 2,
                    offset: 0,
                    size: 32,
                },
                PackMember {
                    binding: 5,
                    offset: 32,
                    size: 16,
                },
            ],
        }
    }

    #[test]
    fn copies_first_in_registers_and_zeroes_missing_or_short_shadows() {
        let mut slots = [ConstantBuffer::default(); MAX_CBUFFER];
        slots[2] = ConstantBuffer {
            buffer: 7,
            first: 1,
            size: 0,
        };
        let source = [9u8; 20];
        let bytes = snapshot(&pack(), &slots, |_| Some(&source), 48).unwrap();
        assert_eq!(&bytes[..4], &[9; 4]);
        assert!(bytes[4..].iter().all(|v| *v == 0));
        slots[2].first = u32::MAX;
        assert_eq!(
            snapshot(&pack(), &slots, |_| Some(&source), 48).unwrap(),
            [0; 48]
        );
        slots[2].first = 0;
        assert_eq!(snapshot(&pack(), &slots, |_| None, 48).unwrap(), [0; 48]);
    }

    #[test]
    fn rejects_bad_layouts_before_reading_or_allocating_a_snapshot() {
        let mut bad = Vec::new();
        for size in [0, 47, 49, MAX_UNIFORM_BYTES + 16, u64::MAX] {
            let mut p = pack();
            p.size = size;
            bad.push(p);
        }
        for (binding, offset, size) in [
            (16, 32, 16),
            (15, 32, 16),
            (2, 32, 16),
            (5, 31, 16),
            (5, 32, 0),
            (5, 32, u64::MAX),
        ] {
            let mut p = pack();
            p.members[1] = PackMember {
                binding,
                offset,
                size,
            };
            bad.push(p);
        }
        let mut empty = pack();
        empty.members.clear();
        bad.push(empty);
        for p in bad {
            assert!(
                snapshot(
                    &p,
                    &[],
                    |_| panic!("layout inválido chegou ao shadow"),
                    u64::MAX
                )
                .is_err()
            );
        }
        assert!(validate_pack(&pack(), 32).is_err());
        let full = Pack {
            size: MAX_UNIFORM_BYTES,
            members: vec![PackMember {
                binding: 0,
                offset: 0,
                size: MAX_UNIFORM_BYTES,
            }],
        };
        assert!(validate_pack(&full, MAX_UNIFORM_BYTES).is_ok());
    }

    #[test]
    fn inline_words_follow_to_uint32_and_keep_vec4_padding() {
        let bytes = inline_constants("[-1,4294967297,3.75,-3.75,1065353216]", 80).unwrap();
        for (slot, expected) in
            bytes
                .as_chunks::<16>()
                .0
                .iter()
                .zip([u32::MAX, 1, 3, 4294967293, 1065353216])
        {
            assert_eq!(&slot[..4], &expected.to_le_bytes());
            assert_eq!(&slot[4..], &[0; 12]);
        }
        assert_eq!(inline_constants("[]", 16).unwrap(), [0; 16]);
        for json in ["null", "{}", "[true]", "[\"x\"]", "[1e999]", "[1,2]"] {
            assert!(inline_constants(json, 16).is_err(), "{json}");
        }
        assert!(inline_constants("[]", 15).is_err());
        assert!(inline_constants(&format!("[{}]", vec!["0"; 4097].join(",")), u64::MAX).is_err());
    }
}
