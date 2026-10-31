//! Worker mínimo do ring: recursos, uploads, views e destruição ligados ao
//! wgpu. Os passes e o desenho usam esses objetos; esta camada é a base do
//! backend que substitui o `wgpu_worker.js`.

use crate::gpu::formats::map;
use crate::gpu::objects::{BufferRequest, TextureRequest, ViewRequest};
use crate::gpu::resources::{create_buffer, create_texture};
use crate::gpu::table::ObjectTable;
use crate::gpu::upload::{BufferUpload, TextureUpload, valid_texture_upload};
use anyhow::{Context, Result, bail};
use wgpu::{
    Buffer, Device, Extent3d, Origin3d, Queue, Sampler, SamplerDescriptor, TexelCopyBufferLayout,
    TexelCopyTextureInfo, Texture, TextureDimension, TextureFormat, TextureView,
    TextureViewDescriptor, TextureViewDimension,
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

pub enum Object {
    Texture(TextureObject),
    Buffer(BufferObject),
    Sampler(Sampler),
    View(ViewObject),
}

pub struct Worker {
    pub device: Device,
    pub queue: Queue,
    pub objects: ObjectTable<Object>,
}

impl Worker {
    pub fn new(device: Device, queue: Queue) -> Self {
        Self {
            device,
            queue,
            objects: ObjectTable::new(),
        }
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
            TexelCopyTextureInfo {
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
            TexelCopyBufferLayout {
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
}
