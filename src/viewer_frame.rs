use egui::{ClippedPrimitive, Rect, TexturesDelta};
use egui_wgpu::wgpu;
use std::time::{Duration, Instant};

use crate::{scene_render::SceneRenderer, visual_ui::ViewportState};

#[derive(Debug, Clone, Copy, Default)]
pub struct ViewerFrameStats {
    pub texture_updates: Duration,
    pub acquire_surface: Duration,
    pub update_buffers: Duration,
    pub submit_present: Duration,
}

pub fn render_viewer_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Surface<'_>,
    config: &wgpu::SurfaceConfiguration,
    egui_renderer: &mut egui_wgpu::Renderer,
    scene_renderer: &SceneRenderer,
    scene_rect: Option<Rect>,
    pixels_per_point: f32,
    viewport: &ViewportState,
    current_charge: &wgpu::Buffer,
    next_charge: &wgpu::Buffer,
    time: f32,
    pulse_rate_hz: f32,
    textures_delta: &TexturesDelta,
    paint_jobs: &[ClippedPrimitive],
) -> Result<ViewerFrameStats, wgpu::SurfaceError> {
    let screen_descriptor = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [config.width, config.height],
        pixels_per_point,
    };

    let texture_started_at = Instant::now();
    for (id, image_delta) in &textures_delta.set {
        egui_renderer.update_texture(device, queue, *id, image_delta);
    }
    let texture_updates = texture_started_at.elapsed();

    let acquire_started_at = Instant::now();
    let frame = surface.get_current_texture()?;
    let acquire_surface = acquire_started_at.elapsed();

    let update_buffers_started_at = Instant::now();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    egui_renderer.update_buffers(device, queue, &mut encoder, paint_jobs, &screen_descriptor);
    let update_buffers = update_buffers_started_at.elapsed();

    let output_view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    scene_renderer.draw(
        device,
        queue,
        &mut encoder,
        &output_view,
        [config.width, config.height],
        scene_rect,
        pixels_per_point,
        viewport,
        current_charge,
        next_charge,
        time,
        pulse_rate_hz,
    );

    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewer-egui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            })
            .forget_lifetime();
        egui_renderer.render(&mut pass, paint_jobs, &screen_descriptor);
    }

    for id in &textures_delta.free {
        egui_renderer.free_texture(id);
    }

    let submit_started_at = Instant::now();
    queue.submit(Some(encoder.finish()));
    frame.present();
    let submit_present = submit_started_at.elapsed();

    Ok(ViewerFrameStats {
        texture_updates,
        acquire_surface,
        update_buffers,
        submit_present,
    })
}
