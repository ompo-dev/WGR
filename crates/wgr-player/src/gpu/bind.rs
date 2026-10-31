//! Chave de bind group por ids, transcrita de `idSources`/`sameIds` do
//! `wgpu_worker.js`. Revalidar alguns ids é mais barato que remontar a chave
//! inteira a cada desenho.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Uniform,
    Sampler,
    SamplerComparison,
    Texture,
    ReadOnlyStorage,
    Storage,
    StorageTexture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub binding: u32,
    pub kind: Kind,
}

/// Par (tipo de recurso, índice no estado da engine): 0 = constant buffer,
/// 1 = primeiro registro do pack, 2 = sampler, 3 = SRV, 4 = UAV.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdSource {
    pub kind: u8,
    pub index: u32,
}

pub fn id_sources(bindings: &[Binding]) -> Vec<IdSource> {
    let mut sources = Vec::with_capacity(bindings.len() * 2);
    for binding in bindings {
        match binding.kind {
            Kind::Uniform => {
                if binding.binding == 15 {
                    continue;
                }
                sources.push(IdSource {
                    kind: 0,
                    index: binding.binding,
                });
                sources.push(IdSource {
                    kind: 1,
                    index: binding.binding,
                });
            }
            Kind::Sampler | Kind::SamplerComparison => sources.push(IdSource {
                kind: 2,
                index: binding.binding.wrapping_sub(16),
            }),
            Kind::Texture => sources.push(IdSource {
                kind: 3,
                index: binding.binding.wrapping_sub(32),
            }),
            Kind::ReadOnlyStorage | Kind::Storage => {
                if binding.binding >= 160 {
                    let base = if binding.binding >= 176 { 176 } else { 160 };
                    sources.push(IdSource {
                        kind: 4,
                        index: binding.binding - base,
                    });
                } else {
                    sources.push(IdSource {
                        kind: 3,
                        index: binding.binding.wrapping_sub(32),
                    });
                }
            }
            Kind::StorageTexture => sources.push(IdSource {
                kind: 4,
                index: binding.binding.wrapping_sub(160),
            }),
        }
    }
    sources
}

/// Estado de bindings da engine, consultado pela chave rápida.
pub trait BoundIds {
    fn bound(&self, kind: u8, index: u32) -> u32;
}

pub fn same_ids(state: &impl BoundIds, sources: &[IdSource], cached: &[u32]) -> bool {
    if sources.len() != cached.len() {
        return false;
    }
    sources
        .iter()
        .zip(cached)
        .all(|(source, id)| state.bound(source.kind, source.index) == *id)
}

pub fn read_ids(state: &impl BoundIds, sources: &[IdSource]) -> Vec<u32> {
    sources
        .iter()
        .map(|source| state.bound(source.kind, source.index))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct State {
        cb: Vec<(u32, u32)>,
        smp: Vec<u32>,
        srv: Vec<u32>,
        uav: Vec<u32>,
    }

    impl BoundIds for State {
        fn bound(&self, kind: u8, index: u32) -> u32 {
            let at = index as usize;
            match kind {
                0 => self.cb.get(at).map_or(0, |entry| entry.0),
                1 => self.cb.get(at).map_or(0, |entry| entry.1),
                2 => self.smp.get(at).copied().unwrap_or(0),
                3 => self.srv.get(at).copied().unwrap_or(0),
                4 => self.uav.get(at).copied().unwrap_or(0),
                _ => 0,
            }
        }
    }

    #[test]
    fn converts_bindings_to_the_same_sources_as_the_web_worker() {
        let bindings = [
            Binding {
                binding: 2,
                kind: Kind::Uniform,
            },
            Binding {
                binding: 16,
                kind: Kind::Sampler,
            },
            Binding {
                binding: 32,
                kind: Kind::Texture,
            },
            Binding {
                binding: 160,
                kind: Kind::StorageTexture,
            },
        ];
        assert_eq!(
            id_sources(&bindings),
            vec![
                IdSource { kind: 0, index: 2 },
                IdSource { kind: 1, index: 2 },
                IdSource { kind: 2, index: 0 },
                IdSource { kind: 3, index: 0 },
                IdSource { kind: 4, index: 0 },
            ]
        );
    }

    #[test]
    fn inline_constants_at_binding_fifteen_are_not_sources() {
        let bindings = [Binding {
            binding: 15,
            kind: Kind::Uniform,
        }];
        assert!(id_sources(&bindings).is_empty());
    }

    #[test]
    fn compares_cached_ids_against_the_current_state() {
        let state = State {
            cb: vec![(7, 3)],
            smp: vec![9],
            srv: vec![11],
            uav: vec![],
        };
        let bindings = [
            Binding {
                binding: 0,
                kind: Kind::Uniform,
            },
            Binding {
                binding: 16,
                kind: Kind::Sampler,
            },
            Binding {
                binding: 32,
                kind: Kind::Texture,
            },
        ];
        let sources = id_sources(&bindings);
        let ids = read_ids(&state, &sources);
        assert_eq!(ids, [7, 3, 9, 11]);
        assert!(same_ids(&state, &sources, &ids));
        assert!(!same_ids(&state, &sources, &[7, 3, 9, 12]));
    }

    #[test]
    fn storage_buffers_select_the_uav_range() {
        let bindings = [Binding {
            binding: 176,
            kind: Kind::Storage,
        }];
        assert_eq!(id_sources(&bindings), vec![IdSource { kind: 4, index: 0 }]);
    }
}
