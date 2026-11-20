use std::collections::HashMap;

use wgpu::{BindGroupLayout, RenderPipeline};

pub const PIPELINE_CACHE_CAPACITY: usize = 128;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PipelineKey {
    pub vertex_generation: u64,
    pub fragment_generation: Option<u64>,
    pub layout: Vec<u32>,
    pub strides: [u32; 16],
    pub topology: u32,
    pub strip_format: Option<u8>,
    pub blend: Vec<u32>,
    pub depth: Vec<u32>,
    pub raster: Vec<u32>,
    pub targets: Vec<Option<wgpu::TextureFormat>>,
    pub samples: u32,
    pub depth_format: Option<wgpu::TextureFormat>,
}

#[derive(Clone)]
pub struct CachedPipeline {
    pub pipeline: RenderPipeline,
    pub group0: BindGroupLayout,
    pub group1: BindGroupLayout,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PipelineCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub live: usize,
}

struct Entry {
    pipeline: CachedPipeline,
    last_used: u64,
}

pub struct PipelineCache {
    entries: HashMap<PipelineKey, Entry>,
    clock: u64,
    hits: u64,
    misses: u64,
}

impl PipelineCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn get(&mut self, key: &PipelineKey) -> Option<CachedPipeline> {
        self.clock = self.clock.saturating_add(1);
        let Some(entry) = self.entries.get_mut(key) else {
            self.misses = self.misses.saturating_add(1);
            return None;
        };
        entry.last_used = self.clock;
        self.hits = self.hits.saturating_add(1);
        Some(entry.pipeline.clone())
    }

    pub fn insert(&mut self, key: PipelineKey, pipeline: CachedPipeline) {
        self.clock = self.clock.saturating_add(1);
        if self.entries.len() >= PIPELINE_CACHE_CAPACITY
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            Entry {
                pipeline,
                last_used: self.clock,
            },
        );
    }

    pub fn stats(&self) -> PipelineCacheStats {
        PipelineCacheStats {
            hits: self.hits,
            misses: self.misses,
            live: self.entries.len(),
        }
    }
}

impl Default for PipelineCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_keeps_pipeline_state_but_not_resource_bindings() {
        let key = PipelineKey {
            vertex_generation: 1,
            fragment_generation: Some(2),
            layout: vec![1, 2],
            strides: [0; 16],
            topology: 4,
            strip_format: None,
            blend: vec![3],
            depth: vec![4],
            raster: vec![5],
            targets: vec![Some(wgpu::TextureFormat::Rgba8Unorm)],
            samples: 1,
            depth_format: None,
        };
        let mut changed = key.clone();
        changed.blend[0] = 6;
        assert_ne!(key, changed);
        let mut changed_target = key.clone();
        changed_target.targets[0] = Some(wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_ne!(key, changed_target);
        let mut changed_layout = key.clone();
        changed_layout.layout.push(3);
        assert_ne!(key, changed_layout);
        let mut changed_stride = key.clone();
        changed_stride.strides[0] = 16;
        assert_ne!(key, changed_stride);
        let mut changed_shader = key.clone();
        changed_shader.vertex_generation = 2;
        assert_ne!(key, changed_shader);
    }
}
