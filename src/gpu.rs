use egui_wgpu::wgpu;
use std::sync::Arc;
use winit::event::WindowEvent;
use winit::window::Window;

pub struct SharedGpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl SharedGpu {
    pub fn new_for_surface(
        instance: wgpu::Instance,
        compatible_surface: &wgpu::Surface<'static>,
    ) -> Result<Self, String> {
        pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(compatible_surface),
                    force_fallback_adapter: false,
                })
                .await
                .ok_or_else(|| "No compatible GPU adapter found.".to_string())?;

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .map_err(|e| format!("Failed to initialize GPU device: {e}"))?;

            Ok(Self {
                instance,
                adapter,
                device,
                queue,
            })
        })
    }
}

pub struct EguiWindowSurface {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub egui_ctx: egui::Context,
    pub egui_state: egui_winit::State,
    pub renderer: egui_wgpu::Renderer,
}

impl EguiWindowSurface {
    pub fn new(
        window: Arc<Window>,
        shared_gpu: &mut Option<Arc<SharedGpu>>,
    ) -> Result<(Self, Arc<SharedGpu>), String> {
        let (gpu, surface) = if let Some(existing) = shared_gpu.as_ref() {
            let surface = existing
                .instance
                .create_surface(window.clone())
                .map_err(|e| format!("Failed to create window surface: {e}"))?;
            (existing.clone(), surface)
        } else {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let surface = instance
                .create_surface(window.clone())
                .map_err(|e| format!("Failed to create initial surface: {e}"))?;
            let created = Arc::new(SharedGpu::new_for_surface(instance, &surface)?);
            *shared_gpu = Some(created.clone());
            (created, surface)
        };

        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let caps = surface.get_capabilities(&gpu.adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);

        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            caps.present_modes
                .first()
                .copied()
                .unwrap_or(wgpu::PresentMode::AutoVsync)
        };

        let alpha_mode = caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width,
            height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&gpu.device, &surface_config);

        let egui_ctx = egui::Context::default();
        configure_visuals(&egui_ctx);

        let max_tex_side = gpu.device.limits().max_texture_dimension_2d as usize;
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(max_tex_side),
        );

        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            surface_format,
            None,
            1,
            false,
        );

        Ok((
            Self {
                window,
                surface,
                surface_config,
                egui_ctx,
                egui_state,
                renderer,
            },
            gpu,
        ))
    }

    pub fn handle_event(&mut self, event: &WindowEvent) -> egui_winit::EventResponse {
        let resp = self.egui_state.on_window_event(&self.window, event);
        if resp.repaint {
            self.window.request_redraw();
        }
        resp
    }

    pub fn resize(&mut self, gpu: &SharedGpu, new_width: u32, new_height: u32) {
        if new_width > 0 && new_height > 0 {
            self.surface_config.width = new_width;
            self.surface_config.height = new_height;
            self.surface.configure(&gpu.device, &self.surface_config);
        }
    }

    pub fn render<F>(&mut self, gpu: &SharedGpu, run_ui: F)
    where
        F: FnMut(&egui::Context),
    {
        let inner = self.window.inner_size();
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if inner.width != self.surface_config.width || inner.height != self.surface_config.height {
            self.resize(gpu, inner.width, inner.height);
        }

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&gpu.device, &self.surface_config);
                return;
            }
            Err(_) => return,
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let full_output = self.egui_ctx.run(raw_input, run_ui);

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);

        let pixels_per_point = full_output.pixels_per_point;
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };

        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(&gpu.device, &gpu.queue, *id, image_delta);
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("skrinshotter_egui_encoder"),
            });

        self.renderer.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("skrinshotter_egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.08,
                            g: 0.09,
                            b: 0.11,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut static_pass = render_pass.forget_lifetime();
            self.renderer
                .render(&mut static_pass, &paint_jobs, &screen_descriptor);
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }

        if let Some(vp_out) = full_output.viewport_output.get(&egui::ViewportId::ROOT) {
            if vp_out.repaint_delay.is_zero() {
                self.window.request_redraw();
            }
        }
    }
}

fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(24, 26, 32);
    visuals.panel_fill = egui::Color32::from_rgb(24, 26, 32);
    ctx.set_visuals(visuals);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wgpu_backends_enabled() {
        let enabled = wgpu::Instance::enabled_backend_features();
        assert!(
            !enabled.is_empty(),
            "At least one native wgpu graphics backend (Metal/DX12/Vulkan/GL) must be enabled"
        );
        #[cfg(target_os = "macos")]
        assert!(
            enabled.contains(wgpu::Backends::METAL),
            "Metal backend must be enabled on macOS"
        );
        #[cfg(target_os = "windows")]
        assert!(
            enabled.contains(wgpu::Backends::DX12) || enabled.contains(wgpu::Backends::VULKAN),
            "DX12 or Vulkan backend must be enabled on Windows"
        );
    }
}
