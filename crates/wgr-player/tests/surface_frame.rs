//! Teste de janela real: cria a janela winit, a superfície wgpu, limpa o
//! quadro e apresenta. Exige um desktop interativo; fica fora do CI.

use std::sync::Arc;
use wgr_player::surface::{Presenter, SurfaceContext};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    window: Option<Arc<Window>>,
    presented: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes())
                .expect("janela"),
        );
        let surface =
            pollster::block_on(SurfaceContext::new(window.clone(), 320, 240)).expect("superfície");
        let source = surface.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface-test-source"),
            size: wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let source_view = source.create_view(&Default::default());
        let mut encoder = surface
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("surface-test"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("surface-test"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &source_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        surface.queue.submit(Some(encoder.finish()));
        let mut presenter = Presenter::new(surface);
        presenter
            .present(1, 1, &source, wgpu::TextureFormat::Rgba8Unorm, 2, 2)
            .expect("apresentar fonte da engine");
        self.presented = true;
        self.window = Some(window);
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

#[test]
#[ignore = "abre uma janela real"]
fn presents_a_clear_frame_through_the_surface() {
    #[cfg(windows)]
    use winit::platform::windows::EventLoopBuilderExtWindows;
    #[cfg(windows)]
    let event_loop = {
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().expect("event loop")
    };
    #[cfg(not(windows))]
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        window: None,
        presented: false,
    };
    event_loop.run_app(&mut app).expect("event loop");
    assert!(app.presented, "nenhum quadro apresentado");
}
