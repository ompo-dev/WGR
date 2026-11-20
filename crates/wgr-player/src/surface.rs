//! Janela winit + superfície wgpu. Cria o device que o `Worker` usa e
//! configura a surface no tamanho físico do buffer da engine.

use anyhow::{Context, Result, ensure};
use std::sync::Arc;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, Color, ColorTargetState, ColorWrites,
    FragmentState, LoadOp, MultisampleState, Operations, PipelineLayoutDescriptor, PrimitiveState,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, StoreOp, Texture, TextureFormat,
    TextureSampleType, TextureViewDescriptor, TextureViewDimension, VertexState,
};
use winit::window::Window;

pub struct SurfaceContext {
    pub instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
}

impl SurfaceContext {
    pub async fn new(window: Arc<Window>, width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window)
            .context("superfície da janela")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| anyhow::anyhow!("adaptador: {error:?}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("wgr-player"),
                ..Default::default()
            })
            .await
            .map_err(|error| anyhow::anyhow!("device: {error:?}"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .context("surface sem formato")?;
        let present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        Ok(Self {
            instance,
            surface,
            adapter,
            device,
            queue,
            config,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if self.config.width == width && self.config.height == height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn acquire(&self) -> Result<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Ok(texture),
            other => anyhow::bail!("quadro da surface indisponível: {other:?}"),
        }
    }
}

/// Compõe o alvo final da engine na superfície da janela.
///
/// O bind group é reutilizado enquanto a mesma textura da engine continuar
/// sendo apresentada. A geração evita reaproveitar uma view quando o id é
/// destruído e recriado pelo ring.
pub struct Presenter {
    surface: SurfaceContext,
    pipeline: RenderPipeline,
    layout: BindGroupLayout,
    source: Option<((u32, u64), BindGroup)>,
}

impl Presenter {
    pub fn new(surface: SurfaceContext) -> Self {
        let layout = surface
            .device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("wgr-player-present-layout"),
                entries: &[BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: false },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
        let module = surface.device.create_shader_module(ShaderModuleDescriptor {
            label: Some("wgr-player-present-shader"),
            source: ShaderSource::Wgsl(include_str!("gpu/present.wgsl").into()),
        });
        let pipeline_layout = surface
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("wgr-player-present-pipeline-layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let pipeline = surface
            .device
            .create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("wgr-player-present-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState {
                        format: surface.config.format,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        Self {
            surface,
            pipeline,
            layout,
            source: None,
        }
    }

    pub fn device(&self) -> wgpu::Device {
        self.surface.device.clone()
    }

    pub fn queue(&self) -> wgpu::Queue {
        self.surface.queue.clone()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.surface.resize(width, height);
    }

    pub fn present(
        &mut self,
        id: u32,
        generation: u64,
        texture: &Texture,
        format: TextureFormat,
        width: u32,
        height: u32,
    ) -> Result<()> {
        ensure!(
            width > 0 && height > 0 && !format.is_depth_stencil_format(),
            "Textura de apresentação inválida"
        );
        let key = (id, generation);
        if self.source.as_ref().map(|(cached, _)| *cached) != Some(key) {
            let view = texture.create_view(&TextureViewDescriptor {
                dimension: Some(TextureViewDimension::D2),
                base_mip_level: 0,
                mip_level_count: Some(1),
                base_array_layer: 0,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let binding = self.surface.device.create_bind_group(&BindGroupDescriptor {
                label: Some("wgr-player-present-bind"),
                layout: &self.layout,
                entries: &[BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&view),
                }],
            });
            self.source = Some((key, binding));
        }
        let frame = self.surface.acquire()?;
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let viewport = crate::input::content_viewport(
            self.surface.config.width,
            self.surface.config.height,
            width,
            height,
        );
        let mut encoder =
            self.surface
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("wgr-player-present"),
                });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("wgr-player-present"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_viewport(viewport.0, viewport.1, viewport.2, viewport.3, 0.0, 1.0);
            pass.set_bind_group(0, &self.source.as_ref().expect("source criada").1, &[]);
            pass.draw(0..3, 0..1);
        }
        self.surface.queue.submit(Some(encoder.finish()));
        self.surface.queue.present(frame);
        Ok(())
    }
}
