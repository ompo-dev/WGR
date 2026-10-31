//! Espelho do estado D3D11 que acompanha o ring, transcrito dos casos
//! `SET_*` do `wgpu_worker.js`. O worker aplica esse estado ao wgpu; aqui ele é
//! mantido puro e testável, sem device.

pub const STAGES: usize = 3;
pub const MAX_SRV: usize = 128;
pub const MAX_SAMPLER: usize = 16;
pub const MAX_CBUFFER: usize = 16;
pub const MAX_UAV: usize = 16;
pub const MAX_VERTEX_BUFFER: usize = 16;
pub const MAX_RENDER_TARGETS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConstantBuffer {
    pub buffer: u32,
    pub first: u32,
    pub size: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderTarget {
    pub texture: u32,
    pub mip: u32,
    pub slice: u32,
    pub format: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DepthTarget {
    pub texture: u32,
    pub mip: u32,
    pub slice: u32,
    pub format: u32,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scissor {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VertexBuffer {
    pub buffer: u32,
    pub stride: u32,
    pub offset: u32,
}

#[derive(Debug, Clone)]
pub struct State {
    pub cb: [[ConstantBuffer; MAX_CBUFFER]; STAGES],
    pub srv: [[u32; MAX_SRV]; STAGES],
    pub sampler: [[u32; MAX_SAMPLER]; STAGES],
    pub uav: [[u32; MAX_UAV]; STAGES],
    pub render_targets: [RenderTarget; MAX_RENDER_TARGETS],
    pub depth: DepthTarget,
    pub shader: [u32; STAGES],
    pub input_layout: u32,
    pub topology: u32,
    pub strip_format: u32,
    pub vertex_buffers: [VertexBuffer; MAX_VERTEX_BUFFER],
    pub index_buffer: VertexBuffer,
    pub blend: u32,
    pub depth_stencil: u32,
    pub raster: u32,
    pub viewports: Vec<Viewport>,
    pub scissors: Vec<Scissor>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            cb: [[ConstantBuffer::default(); MAX_CBUFFER]; STAGES],
            srv: [[0; MAX_SRV]; STAGES],
            sampler: [[0; MAX_SAMPLER]; STAGES],
            uav: [[0; MAX_UAV]; STAGES],
            render_targets: [RenderTarget::default(); MAX_RENDER_TARGETS],
            depth: DepthTarget::default(),
            shader: [0; STAGES],
            input_layout: 0,
            topology: 0,
            strip_format: 0,
            vertex_buffers: [VertexBuffer::default(); MAX_VERTEX_BUFFER],
            index_buffer: VertexBuffer::default(),
            blend: 0,
            depth_stencil: 0,
            raster: 0,
            viewports: Vec::new(),
            scissors: Vec::new(),
        }
    }
}

fn stage_of(value: u32) -> Option<usize> {
    if (value as usize) < STAGES {
        Some(value as usize)
    } else {
        None
    }
}

impl State {
    pub fn set_shader(&mut self, stage: u32, shader: u32) {
        if let Some(stage) = stage_of(stage) {
            self.shader[stage] = shader;
        }
    }

    /// `SET_CBUFFERS`: stage, início, contagem e (buffer, first, size) por slot.
    pub fn set_cbuffers(&mut self, stage: u32, start: u32, entries: &[(u32, u32, u32)]) {
        let Some(stage) = stage_of(stage) else {
            return;
        };
        for (index, &(buffer, first, size)) in entries.iter().enumerate() {
            let slot = start as usize + index;
            if slot < MAX_CBUFFER {
                self.cb[stage][slot] = ConstantBuffer {
                    buffer,
                    first,
                    size,
                };
            }
        }
    }

    /// `SET_SRVS`: stage, início, contagem e ids.
    pub fn set_srvs(&mut self, stage: u32, start: u32, ids: &[u32]) {
        let Some(stage) = stage_of(stage) else {
            return;
        };
        for (index, &id) in ids.iter().enumerate() {
            let slot = start as usize + index;
            if slot < MAX_SRV {
                self.srv[stage][slot] = id;
            }
        }
    }

    /// `SET_SAMPLERS`: stage, início, contagem e ids.
    pub fn set_samplers(&mut self, stage: u32, start: u32, ids: &[u32]) {
        let Some(stage) = stage_of(stage) else {
            return;
        };
        for (index, &id) in ids.iter().enumerate() {
            let slot = start as usize + index;
            if slot < MAX_SAMPLER {
                self.sampler[stage][slot] = id;
            }
        }
    }

    /// `SET_UAVS`: stage, início, contagem e ids.
    pub fn set_uavs(&mut self, stage: u32, start: u32, ids: &[u32]) {
        let Some(stage) = stage_of(stage) else {
            return;
        };
        for (index, &id) in ids.iter().enumerate() {
            let slot = start as usize + index;
            if slot < MAX_UAV {
                self.uav[stage][slot] = id;
            }
        }
    }

    /// `SET_RENDER_TARGETS`: oito RTs e o par profundidade/estêncil.
    pub fn set_render_targets(&mut self, words: &[u32]) {
        for index in 0..MAX_RENDER_TARGETS {
            let at = index * 4;
            if at + 3 >= words.len() {
                break;
            }
            self.render_targets[index] = RenderTarget {
                texture: words[at],
                mip: words[at + 1],
                slice: words[at + 2],
                format: words[at + 3],
            };
        }
        let at = MAX_RENDER_TARGETS * 4;
        if at + 4 < words.len() {
            self.depth = DepthTarget {
                texture: words[at],
                mip: words[at + 1],
                slice: words[at + 2],
                format: words[at + 3],
                flags: words[at + 4],
            };
        }
    }

    /// `SET_VERTEX_BUFFERS`: buffer, offset e stride por slot.
    pub fn set_vertex_buffers(&mut self, entries: &[(u32, u32, u32)]) {
        self.set_vertex_buffers_at(0, entries);
    }

    pub fn set_vertex_buffers_at(&mut self, start: usize, entries: &[(u32, u32, u32)]) {
        for (index, &(buffer, offset, stride)) in entries.iter().enumerate() {
            let slot = start + index;
            if slot < MAX_VERTEX_BUFFER {
                self.vertex_buffers[slot] = VertexBuffer {
                    buffer,
                    stride,
                    offset,
                };
            }
        }
    }

    pub fn set_index_buffer(&mut self, buffer: u32, offset: u32) {
        self.index_buffer = VertexBuffer {
            buffer,
            stride: 0,
            offset,
        };
    }

    pub fn set_viewports(&mut self, viewports: Vec<Viewport>) {
        self.viewports = viewports;
    }

    pub fn set_scissors(&mut self, scissors: Vec<Scissor>) {
        self.scissors = scissors;
    }

    pub fn set_topology(&mut self, topology: u32, strip_format: u32) {
        self.topology = topology;
        self.strip_format = strip_format;
    }

    /// Limpa o estado por desenho depois de um `CLEAR_STATE`.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_srvs_at_the_requested_offset() {
        let mut state = State::default();
        state.set_srvs(1, 4, &[10, 11, 12]);
        assert_eq!(state.srv[1][4..7], [10, 11, 12]);
        assert_eq!(state.srv[0][4], 0);
    }

    #[test]
    fn ignores_out_of_range_slots_instead_of_panicking() {
        let mut state = State::default();
        state.set_srvs(0, MAX_SRV as u32 - 1, &[1, 2, 3]);
        assert_eq!(state.srv[0][MAX_SRV - 1], 1);
        state.set_srvs(9, 0, &[1]);
        state.set_samplers(0, MAX_SAMPLER as u32, &[1]);
    }

    #[test]
    fn parses_render_targets_and_depth_from_the_same_payload() {
        let mut words = vec![0u32; 37];
        words[0] = 5;
        words[3] = 28;
        words[4] = 6;
        words[32] = 99;
        words[35] = 40;
        words[36] = 1;
        let mut state = State::default();
        state.set_render_targets(&words);
        assert_eq!(state.render_targets[0].texture, 5);
        assert_eq!(state.render_targets[1].texture, 6);
        assert_eq!(state.render_targets[0].format, 28);
        assert_eq!(state.depth.texture, 99);
        assert_eq!(state.depth.format, 40);
        assert_eq!(state.depth.flags, 1);
    }

    #[test]
    fn viewports_keep_the_six_words_per_entry() {
        let mut state = State::default();
        state.set_viewports(vec![Viewport {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1200.0,
            min_depth: 0.0,
            max_depth: 1.0,
        }]);
        assert_eq!(state.viewports[0].width, 1920.0);
        assert_eq!(state.viewports[0].max_depth, 1.0);
    }
}
