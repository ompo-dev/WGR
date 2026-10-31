//! Worker do ring: recursos, estado, passes com MRT, desenho e submissão.
//! Traduz os opcodes do `wgpu_worker.js` para o wgpu; a hospedagem da engine
//! entrega os comandos e a memória compartilhada.

use crate::gpu::assembly::{LayoutElement, ShaderInput, parse_layout, parse_shader};
use crate::gpu::formats::map;
use crate::gpu::objects::{
    BufferRequest, TextureRequest, ViewRequest, parse_buffer, parse_srv, parse_state,
    parse_texture, parse_uav,
};
use crate::gpu::pipeline::{self, PipelineInput};
use crate::gpu::resources::{create_buffer, create_texture};
use crate::gpu::table::ObjectTable;
use crate::gpu::upload::{
    BufferUpload, TextureUpload, parse_buffer_upload, parse_texture_upload, valid_texture_upload,
};
use crate::shader::ShaderInfo;
use crate::shader::{Pack, pack_uniforms, parse_wgsl};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use wgpu::{
    BindGroup, BindGroupLayout, BindGroupLayoutDescriptor, Buffer, Color, CommandEncoder,
    CommandEncoderDescriptor, Device, Extent3d, FragmentState, LoadOp, Operations, Origin3d,
    PipelineLayoutDescriptor, Queue, RenderPass, RenderPassColorAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, Sampler, SamplerDescriptor, ShaderModule, ShaderModuleDescriptor,
    ShaderSource, StoreOp, Texture, TextureDimension, TextureFormat, TextureView,
    TextureViewDescriptor, TextureViewDimension, VertexState,
};
use wgr_runtime::shared::GuestMemory;

pub struct TextureObject {
    pub texture: Texture,
    pub format: TextureFormat,
    pub view_formats: Vec<TextureFormat>,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mips: u32,
    pub samples: u32,
    pub dimension: TextureDimension,
}

pub struct BufferObject {
    pub buffer: Option<Buffer>,
    pub shadow: Option<Vec<u8>>,
    pub size: u64,
    pub stride: u32,
}

pub struct ViewObject {
    pub resource: u32,
    pub is_buffer: bool,
    pub dxgi: u32,
    pub dimension: TextureViewDimension,
    pub mip: u32,
    pub mip_count: u32,
    pub layer: u32,
    pub layer_count: u32,
    pub view: Option<TextureView>,
}

#[derive(Clone)]
pub struct Program {
    pub stage: u32,
    pub hash: String,
    pub inputs: Vec<ShaderInput>,
    pub module: ShaderModule,
    pub info: ShaderInfo,
    pub pack: Option<Pack>,
    pub label: String,
}

pub enum Object {
    Texture(TextureObject),
    Buffer(BufferObject),
    Sampler(Sampler),
    View(ViewObject),
    Shader(Program),
    Layout(Vec<LayoutElement>),
    State { kind: u32, words: Vec<u32> },
}

pub struct Worker {
    pub device: Device,
    pub queue: Queue,
    pub objects: ObjectTable<Object>,
    pub state: crate::gpu::state::State,
    shader_sources: HashMap<String, String>,
    encoder: Option<CommandEncoder>,
    pass: Option<RenderPass<'static>>,
    pass_views: Vec<Option<TextureView>>,
    pending_clears: [Option<Color>; 8],
}

impl Worker {
    pub fn new(device: Device, queue: Queue) -> Self {
        Self {
            device,
            queue,
            objects: ObjectTable::new(),
            state: Default::default(),
            shader_sources: HashMap::new(),
            encoder: None,
            pass: None,
            pass_views: Vec::new(),
            pending_clears: [None; 8],
        }
    }

    pub fn register_shader_source(&mut self, hash: &str, source: &str) {
        self.shader_sources
            .insert(hash.to_string(), source.to_string());
    }

    pub fn texture(&self, id: u32) -> Option<&TextureObject> {
        match self.objects.get(id) {
            Some(Object::Texture(texture)) => Some(texture),
            _ => None,
        }
    }

    pub fn view(&self, id: u32) -> Option<&TextureView> {
        match self.objects.get(id) {
            Some(Object::View(view)) => view.view.as_ref(),
            _ => None,
        }
    }

    fn shader(&self, stage: u32) -> Option<&Program> {
        let id = *self.state.shader.get(stage as usize)?;
        match self.objects.get(id) {
            Some(Object::Shader(program)) => Some(program),
            _ => None,
        }
    }

    fn layout(&self) -> Option<&Vec<LayoutElement>> {
        match self.objects.get(self.state.input_layout) {
            Some(Object::Layout(elements)) => Some(elements),
            _ => None,
        }
    }

    fn state_words(&self, id: u32) -> Option<&Vec<u32>> {
        match self.objects.get(id) {
            Some(Object::State { words, .. }) => Some(words),
            _ => None,
        }
    }

    pub fn create_texture(&mut self, request: &TextureRequest) -> Result<()> {
        let mapped = map(request.dxgi, request.bind);
        let texture = create_texture(
            &self.device,
            "texture",
            request.width,
            request.height,
            request.depth_or_array_layers,
            request.mip_levels,
            request.samples,
            request.dimension,
            mapped.format,
            &mapped.views,
            request.bind,
        );
        self.objects.insert(
            request.id,
            Object::Texture(TextureObject {
                texture,
                format: mapped.format,
                view_formats: mapped.views,
                width: request.width,
                height: request.height,
                depth: request.depth_or_array_layers.max(1),
                mips: request.mip_levels,
                samples: request.samples,
                dimension: request.dimension,
            }),
        );
        Ok(())
    }

    pub fn upload_texture(&mut self, upload: &TextureUpload, guest: &GuestMemory) -> Result<()> {
        let texture = match self.objects.get(upload.id) {
            Some(Object::Texture(texture)) => texture,
            _ => bail!("Upload para textura desconhecida {}", upload.id),
        };
        if !valid_texture_upload(upload, texture.format) {
            bail!("Upload inválido para a textura {}", upload.id);
        }
        if upload.mip >= texture.mips {
            bail!("Mip {} fora da textura {}", upload.mip, upload.id);
        }
        let bytes = guest
            .read(upload.data_address, upload.size as usize)
            .context("Leitura dos bytes da textura")?;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.texture,
                mip_level: upload.mip,
                origin: Origin3d {
                    x: upload.x,
                    y: upload.y,
                    z: if texture.dimension == TextureDimension::D3 {
                        upload.z
                    } else {
                        upload.slice
                    },
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(upload.bytes_per_row),
                rows_per_image: Some(upload.rows),
            },
            Extent3d {
                width: upload.width,
                height: upload.height,
                depth_or_array_layers: upload.depth,
            },
        );
        Ok(())
    }

    pub fn create_buffer(&mut self, request: &BufferRequest) -> Result<()> {
        let gpu = if request.bind & (0x1 | 0x2 | 0x8 | 0x80 | 0x10) != 0 {
            Some(create_buffer(
                &self.device,
                "buffer",
                request.size,
                request.bind,
                request.misc,
            ))
        } else {
            None
        };
        let shadow = (request.bind & 0x4 != 0).then(|| vec![0u8; request.size as usize]);
        self.objects.insert(
            request.id,
            Object::Buffer(BufferObject {
                buffer: gpu,
                shadow,
                size: request.size,
                stride: request.stride,
            }),
        );
        Ok(())
    }

    pub fn upload_buffer(&mut self, upload: &BufferUpload, guest: &GuestMemory) -> Result<()> {
        let Some(Object::Buffer(buffer)) = self.objects.get(upload.id) else {
            bail!("Upload para buffer desconhecido {}", upload.id);
        };
        let bytes = guest
            .read(upload.data_address, upload.size as usize)
            .context("Leitura dos bytes do buffer")?;
        if let Some(gpu) = &buffer.buffer {
            self.queue.write_buffer(gpu, upload.offset, &bytes);
        }
        if let Some(shadow) = self
            .objects
            .get_mut(upload.id)
            .and_then(|object| match object {
                Object::Buffer(buffer) => buffer.shadow.as_mut(),
                _ => None,
            })
        {
            let start = upload.offset as usize;
            let end = start.saturating_add(bytes.len()).min(shadow.len());
            if start < end {
                shadow[start..end].copy_from_slice(&bytes[..end - start]);
            }
        }
        Ok(())
    }

    pub fn create_sampler(&mut self, id: u32, descriptor: &SamplerDescriptor<'_>) -> Result<()> {
        let sampler = self.device.create_sampler(descriptor);
        self.objects.insert(id, Object::Sampler(sampler));
        Ok(())
    }

    pub fn create_view(&mut self, request: &ViewRequest) -> Result<()> {
        if request.is_buffer {
            self.objects.insert(
                request.id,
                Object::View(ViewObject {
                    resource: request.resource,
                    is_buffer: true,
                    dxgi: request.dxgi,
                    dimension: request.dimension,
                    mip: 0,
                    mip_count: 1,
                    layer: 0,
                    layer_count: 1,
                    view: None,
                }),
            );
            return Ok(());
        }
        let Some(Object::Texture(texture)) = self.objects.get(request.resource) else {
            bail!("SRV/UAV para textura desconhecida {}", request.resource);
        };
        let mip = request.a.min(texture.mips.saturating_sub(1));
        let available_mips = texture.mips.saturating_sub(mip);
        let mip_count = if request.b == 0 || request.b == u32::MAX {
            available_mips.max(1)
        } else {
            request.b.min(available_mips).max(1)
        };
        let layer = request.c.min(texture.depth.saturating_sub(1));
        let available_layers = texture.depth.saturating_sub(layer);
        let layer_count = if request.d == 0 {
            available_layers.max(1)
        } else {
            request.d.min(available_layers).max(1)
        };
        let format = (request.dxgi != 0).then(|| map(request.dxgi, 0).format);
        let view = texture.texture.create_view(&TextureViewDescriptor {
            format,
            dimension: Some(request.dimension),
            base_mip_level: mip,
            mip_level_count: Some(mip_count),
            base_array_layer: layer,
            array_layer_count: Some(layer_count),
            ..Default::default()
        });
        self.objects.insert(
            request.id,
            Object::View(ViewObject {
                resource: request.resource,
                is_buffer: false,
                dxgi: request.dxgi,
                dimension: request.dimension,
                mip,
                mip_count,
                layer,
                layer_count,
                view: Some(view),
            }),
        );
        Ok(())
    }

    pub fn destroy(&mut self, id: u32) -> Option<Object> {
        self.objects.remove(id)
    }

    fn create_shader(&mut self, words: &[u32]) -> Result<()> {
        let (id, shader) = parse_shader(words).context("CREATE_SHADER inválido")?;
        let source = self
            .shader_sources
            .get(&shader.hash)
            .with_context(|| format!("WGSL ausente para {}", shader.hash))?
            .clone();
        let (text, pack) = pack_uniforms(&source);
        let label = format!("{}:{}", shader.hash, shader.stage);
        let module = self.device.create_shader_module(ShaderModuleDescriptor {
            label: Some(&label),
            source: ShaderSource::Wgsl(text.as_str().into()),
        });
        let info = parse_wgsl(&text);
        self.objects.insert(
            id,
            Object::Shader(Program {
                stage: shader.stage,
                hash: shader.hash,
                inputs: shader.inputs,
                module,
                info,
                pack,
                label,
            }),
        );
        Ok(())
    }

    fn create_layout(&mut self, words: &[u32]) -> Result<()> {
        let (id, elements) = parse_layout(words).context("CREATE_LAYOUT inválido")?;
        self.objects.insert(id, Object::Layout(elements));
        Ok(())
    }

    fn create_state(&mut self, words: &[u32]) -> Result<()> {
        let request = parse_state(words).context("CREATE_STATE inválido")?;
        let body: Vec<u32> = words.get(2..).unwrap_or_default().to_vec();
        let object = if request.kind == 3 {
            let sampler = self.device.create_sampler(&SamplerDescriptor::default());
            Object::Sampler(sampler)
        } else {
            Object::State {
                kind: request.kind,
                words: body,
            }
        };
        self.objects.insert(request.id, object);
        Ok(())
    }

    fn exec_state(&mut self, op: u32, words: &[u32]) -> Result<()> {
        match op {
            18 => self.state.set_shader(words[0], words[1]),
            19 => self.state.input_layout = words[0],
            20 => self.state.set_topology(words[0], 0),
            21 => {
                let start = words[0] as usize;
                let count = words[1] as usize;
                let mut entries = Vec::with_capacity(count);
                for i in 0..count {
                    let at = 2 + i * 3;
                    if let (Some(buffer), Some(stride), Some(offset)) =
                        (words.get(at), words.get(at + 1), words.get(at + 2))
                    {
                        entries.push((*buffer, *stride, *offset));
                    }
                }
                self.state.set_vertex_buffers_at(start, &entries);
            }
            22 => self.state.set_index_buffer(words[0], words[1]),
            23 => {
                let stage = words[0];
                let start = words[1] as usize;
                let count = words[2] as usize;
                let mut entries = Vec::with_capacity(count);
                for i in 0..count {
                    let at = 3 + i * 3;
                    if let (Some(buffer), Some(first), Some(size)) =
                        (words.get(at), words.get(at + 1), words.get(at + 2))
                    {
                        entries.push((*buffer, *first, *size));
                    }
                }
                self.state.set_cbuffers(stage, start as u32, &entries);
            }
            24 => {
                let stage = words[0];
                let start = words[1];
                self.state
                    .set_srvs(stage, start, &words[3..3 + words[2] as usize]);
            }
            25 => {
                let stage = words[0];
                let start = words[1];
                self.state
                    .set_samplers(stage, start, &words[3..3 + words[2] as usize]);
            }
            38 => {
                let stage = words[0];
                let start = words[1];
                let count = words[2] as usize;
                self.state.set_uavs(stage, start, &words[3..3 + count]);
            }
            26 => self.state.set_render_targets(words),
            27 => self.state.blend = words[0],
            28 => self.state.depth_stencil = words[0],
            29 => self.state.raster = words[0],
            30 => {
                let count = words[0] as usize;
                let mut viewports = Vec::with_capacity(count);
                for i in 0..count {
                    let at = 1 + i * 6;
                    if at + 5 < words.len() {
                        viewports.push(crate::gpu::state::Viewport {
                            x: f32::from_bits(words[at]),
                            y: f32::from_bits(words[at + 1]),
                            width: f32::from_bits(words[at + 2]),
                            height: f32::from_bits(words[at + 3]),
                            min_depth: f32::from_bits(words[at + 4]),
                            max_depth: f32::from_bits(words[at + 5]),
                        });
                    }
                }
                self.state.set_viewports(viewports);
            }
            31 => {
                let count = words[0] as usize;
                let mut scissors = Vec::with_capacity(count);
                for i in 0..count {
                    let at = 1 + i * 4;
                    if at + 3 < words.len() {
                        scissors.push(crate::gpu::state::Scissor {
                            left: words[at] as i32,
                            top: words[at + 1] as i32,
                            right: words[at + 2] as i32,
                            bottom: words[at + 3] as i32,
                        });
                    }
                }
                self.state.set_scissors(scissors);
            }
            _ => {}
        }
        Ok(())
    }

    /// Executa um comando do ring. `payload_address` é o endereço absoluto dos
    /// bytes do comando na memória da engine (para uploads).
    pub fn exec(
        &mut self,
        op: u32,
        words: &[u32],
        payload_address: u64,
        guest: &GuestMemory,
    ) -> Result<()> {
        match op {
            3 => {
                let request = parse_texture(words).context("CREATE_TEXTURE inválido")?;
                self.create_texture(&request)
            }
            4 => {
                self.destroy(words[0]);
                Ok(())
            }
            5 => {
                let color = Color {
                    r: f64::from(f32::from_bits(words[3])),
                    g: f64::from(f32::from_bits(words[4])),
                    b: f64::from(f32::from_bits(words[5])),
                    a: f64::from(f32::from_bits(words[6])),
                };
                self.pending_clear(words[0], color);
                Ok(())
            }
            11 => {
                let request = parse_buffer(words).context("CREATE_BUFFER inválido")?;
                self.create_buffer(&request)
            }
            12 | 47 => {
                let upload = parse_buffer_upload(payload_address, words, op == 47)
                    .context("UPLOAD_BUFFER")?;
                self.upload_buffer(&upload, guest)
            }
            13 => {
                let upload =
                    parse_texture_upload(payload_address, words).context("UPLOAD_TEXTURE")?;
                self.upload_texture(&upload, guest)
            }
            14 => self.create_shader(words),
            15 => self.create_layout(words),
            16 => self.create_state(words),
            17 => {
                let request = parse_srv(words).context("CREATE_SRV inválido")?;
                self.create_view(&request)
            }
            37 => {
                let request = parse_uav(words).context("CREATE_UAV inválido")?;
                self.create_view(&request)
            }
            18..=31 => self.exec_state(op, words),
            32 => self.draw(words),
            33 => {
                self.state.clear();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn pending_clear(&mut self, id: u32, color: Color) {
        for (index, target) in self.state.render_targets.iter().enumerate() {
            if target.texture == id {
                self.pending_clears[index] = Some(color);
                return;
            }
        }
        self.pending_clears[0] = Some(color);
    }

    fn ensure_pass(&mut self) -> Result<()> {
        if self.pass.is_some() {
            return Ok(());
        }
        let mut views: Vec<Option<TextureView>> = Vec::new();
        let mut formats: Vec<TextureFormat> = Vec::new();
        for target in &self.state.render_targets {
            if target.texture == 0 {
                views.push(None);
                formats.push(TextureFormat::Rgba8Unorm);
                continue;
            }
            let Some(texture) = self.texture(target.texture) else {
                bail!("Alvo {} sem textura", target.texture);
            };
            let format = if target.format != 0 {
                map(target.format, 0).format
            } else {
                texture.format
            };
            let view = texture.texture.create_view(&TextureViewDescriptor {
                base_mip_level: target.mip,
                mip_level_count: Some(1),
                base_array_layer: target.slice,
                array_layer_count: Some(1),
                ..Default::default()
            });
            views.push(Some(view));
            formats.push(format);
        }
        if views.iter().all(Option::is_none) {
            bail!("Nenhum render target ligado");
        }
        let clear = self.pending_clears;
        let color_attachments: Vec<Option<RenderPassColorAttachment<'_>>> = views
            .iter()
            .enumerate()
            .map(|(index, view)| {
                view.as_ref().map(|view| RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: clear[index].map_or(LoadOp::Load, LoadOp::Clear),
                        store: StoreOp::Store,
                    },
                })
            })
            .collect();
        let encoder = self.encoder.get_or_insert_with(|| {
            self.device
                .create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("wgr-player"),
                })
        });
        let pass = encoder
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("wgr-player"),
                color_attachments: &color_attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        self.pass_views = views;
        self.pass = Some(pass);
        self.pending_clears = [None; 8];
        Ok(())
    }

    fn bind_group(
        &self,
        stage: u32,
        program: &Program,
        group: u32,
        layout: &BindGroupLayout,
    ) -> Result<Option<BindGroup>> {
        let bindings: Vec<_> = program
            .info
            .bindings
            .iter()
            .filter(|binding| binding.group == group)
            .collect();
        if bindings.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for binding in &bindings {
            let resource = match binding.kind {
                crate::shader::BindingKind::Sampler
                | crate::shader::BindingKind::SamplerComparison => {
                    let id = *self
                        .state
                        .sampler
                        .get(stage as usize)
                        .and_then(|slot| slot.get(binding.binding.wrapping_sub(16) as usize))
                        .unwrap_or(&0);
                    let Some(Object::Sampler(sampler)) = self.objects.get(id) else {
                        bail!("Sampler ausente no binding {}", binding.binding);
                    };
                    wgpu::BindingResource::Sampler(sampler)
                }
                crate::shader::BindingKind::Texture => {
                    let id = *self
                        .state
                        .srv
                        .get(stage as usize)
                        .and_then(|slot| slot.get(binding.binding.wrapping_sub(32) as usize))
                        .unwrap_or(&0);
                    let Some(Object::View(view)) = self.objects.get(id) else {
                        bail!("SRV ausente no binding {}", binding.binding);
                    };
                    let Some(view) = &view.view else {
                        bail!("SRV de buffer não suportado no binding {}", binding.binding);
                    };
                    wgpu::BindingResource::TextureView(view)
                }
                _ => continue,
            };
            entries.push(wgpu::BindGroupEntry {
                binding: binding.binding,
                resource,
            });
        }
        if entries.is_empty() {
            return Ok(None);
        }
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wgr-player"),
            layout,
            entries: &entries,
        });
        Ok(Some(bind_group))
    }

    fn draw(&mut self, words: &[u32]) -> Result<()> {
        let kind = words[0];
        let count = *words.get(1).unwrap_or(&0);
        let instances = *words.get(2).unwrap_or(&1);
        let start = *words.get(3).unwrap_or(&0);
        let base_vertex = *words.get(4).unwrap_or(&0) as i32;
        let start_instance = *words.get(5).unwrap_or(&0);
        let vs = self.shader(0).context("Vertex shader ausente")?;
        let vs_inputs = vs.inputs.clone();
        let vs_info = vs.info.clone();
        let vs_module = vs.module.clone();
        let vs_label = vs.label.clone();
        let ps = self.shader(1);
        let (ps_info, ps_module, ps_label) = match ps {
            Some(program) => (
                Some(program.info.clone()),
                Some(program.module.clone()),
                Some(program.label.clone()),
            ),
            None => (None, None, None),
        };
        let elements = self.layout().cloned().unwrap_or_default();

        self.ensure_pass()?;
        let target_formats: Vec<Option<TextureFormat>> = self
            .state
            .render_targets
            .iter()
            .map(|target| {
                if target.texture == 0 {
                    None
                } else if target.format != 0 {
                    Some(map(target.format, 0).format)
                } else {
                    self.texture(target.texture).map(|texture| texture.format)
                }
            })
            .collect();
        let blend_words = self.state_words(self.state.blend).cloned();
        let depth_words = self.state_words(self.state.depth_stencil).cloned();
        let raster_words = self.state_words(self.state.raster).cloned();
        let plan = pipeline::plan(&PipelineInput {
            vs_label,
            ps_label,
            vs: &vs_info,
            ps: ps_info.as_ref(),
            inputs: &vs_inputs,
            elements: &elements,
            strides: &[],
            topology: self.state.topology,
            strip_format: None,
            blend_words: blend_words.as_deref(),
            depth_words: depth_words.as_deref(),
            raster_words: raster_words.as_deref(),
            targets: &target_formats,
            samples: 1,
            depth_format: None,
        });

        let bgl0 = self
            .device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("wgr-player"),
                entries: &plan.group0,
            });
        let bgl1 = self
            .device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("wgr-player"),
                entries: &plan.group1,
            });
        let layout = self
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("wgr-player"),
                bind_group_layouts: &[Some(&bgl0), Some(&bgl1)],
                immediate_size: 0,
            });
        let buffer_plans: Vec<_> = plan
            .vertex_buffers
            .iter()
            .map(|plan| plan.as_ref().map(|plan| (plan, plan.attributes())))
            .collect();
        let mut vertex_layouts = Vec::new();
        for entry in &buffer_plans {
            match entry {
                Some((plan, attrs)) => vertex_layouts.push(Some(plan.layout(attrs))),
                None => vertex_layouts.push(None),
            }
        }
        let pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDescriptor {
                label: Some(&plan.label),
                layout: Some(&layout),
                vertex: VertexState {
                    module: &vs_module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    buffers: &vertex_layouts,
                },
                fragment: ps_module.as_ref().map(|module| FragmentState {
                    module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    targets: &plan.targets,
                }),
                primitive: plan.primitive,
                depth_stencil: None,
                multisample: plan.multisample,
                multiview_mask: None,
                cache: None,
            });

        let bind0 = match self.shader(0).cloned() {
            Some(program) => self.bind_group(0, &program, 0, &bgl0)?,
            None => None,
        };
        let bind1 = match self.shader(1).cloned() {
            Some(program) => self.bind_group(1, &program, 1, &bgl1)?,
            None => None,
        };

        if let Some(pass) = self.pass.as_mut() {
            pass.set_pipeline(&pipeline);
            if let Some(bind) = &bind0 {
                pass.set_bind_group(0, bind, &[]);
            }
            if let Some(bind) = &bind1 {
                pass.set_bind_group(1, bind, &[]);
            }
            for (slot, buffer) in self.state.vertex_buffers.iter().enumerate() {
                if buffer.buffer != 0
                    && let Some(Object::Buffer(object)) = self.objects.get(buffer.buffer)
                    && let Some(gpu) = &object.buffer
                {
                    pass.set_vertex_buffer(slot as u32, gpu.slice(buffer.offset as u64..));
                }
            }
            if let Some(viewport) = self.state.viewports.first() {
                pass.set_viewport(
                    viewport.x,
                    viewport.y,
                    viewport.width.max(1.0),
                    viewport.height.max(1.0),
                    viewport.min_depth,
                    viewport.max_depth,
                );
            }
            let indexed = kind == 1 || kind == 3 || kind == 4;
            if indexed {
                let Some(Object::Buffer(index)) = self.objects.get(self.state.index_buffer.buffer)
                else {
                    bail!("Index buffer ausente");
                };
                let Some(gpu) = &index.buffer else {
                    bail!("Index buffer sem GPU");
                };
                let format = if self.state.index_buffer.stride == 57 {
                    wgpu::IndexFormat::Uint16
                } else {
                    wgpu::IndexFormat::Uint32
                };
                pass.set_index_buffer(gpu.slice(self.state.index_buffer.offset as u64..), format);
                pass.draw_indexed(
                    start..start + count,
                    base_vertex,
                    start_instance..start_instance + instances,
                );
            } else {
                pass.draw(
                    start..start + count,
                    start_instance..start_instance + instances,
                );
            }
        }
        Ok(())
    }

    pub fn submit(&mut self) {
        self.pass = None;
        if let Some(encoder) = self.encoder.take() {
            self.queue.submit(Some(encoder.finish()));
        }
    }
}
