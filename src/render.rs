use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use egui_winit::winit;
use winit::dpi::PhysicalSize;

const TARGET_ASPECT_RATIO: f32 = 16.0 / 9.0;
const MAX_ZOOM: f32 = 8.0;
const MIN_ZOOM_FIT_FACTOR: f32 = 0.9;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RenderParams {
    view: [f32; 4],
    layer: [u32; 4],
}

#[derive(Clone, Copy)]
pub struct CameraState {
    surface_size: PhysicalSize<u32>,
    pub zoom: f32,
    pub offset: [f32; 2],
}

impl CameraState {
    pub fn new(surface_size: PhysicalSize<u32>) -> Self {
        Self {
            surface_size,
            zoom: fit_zoom(surface_size),
            offset: [0.0, 0.0],
        }
    }

    pub fn resize(&mut self, surface_size: PhysicalSize<u32>) {
        self.surface_size = surface_size;
        self.zoom = self.zoom.clamp(min_zoom(self.surface_size), MAX_ZOOM);
        self.clamp_offset();
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(min_zoom(self.surface_size), MAX_ZOOM);
        self.clamp_offset();
    }

    pub fn reset_to_fit(&mut self) {
        self.zoom = fit_zoom(self.surface_size);
        self.offset = [0.0, 0.0];
    }

    pub fn pan_by(&mut self, delta: [f32; 2]) {
        self.offset[0] += delta[0];
        self.offset[1] += delta[1];
        self.clamp_offset();
    }

    pub fn view_params(&self) -> [f32; 2] {
        view_uv_scale(self.surface_size, self.zoom)
    }

    pub fn surface_to_world_uv(&self, position: [f32; 2]) -> Option<[f32; 2]> {
        let width = self.surface_size.width.max(1) as f32;
        let height = self.surface_size.height.max(1) as f32;
        let uv = [position[0] / width, position[1] / height];
        let view = self.view_params();
        let world = [
            (uv[0] - 0.5) * view[0] + 0.5 + self.offset[0],
            (uv[1] - 0.5) * view[1] + 0.5 + self.offset[1],
        ];

        if !(0.0..1.0).contains(&world[0]) || !(0.0..1.0).contains(&world[1]) {
            return None;
        }

        Some(world)
    }

    fn clamp_offset(&mut self) {
        let scaled = view_uv_scale(self.surface_size, self.zoom);
        let max_offset_x = (1.0 - scaled[0]).abs() * 0.5;
        let max_offset_y = (1.0 - scaled[1]).abs() * 0.5;
        self.offset[0] = self.offset[0].clamp(-max_offset_x, max_offset_x);
        self.offset[1] = self.offset[1].clamp(-max_offset_y, max_offset_y);
    }
}

fn aspect_crop(surface_size: PhysicalSize<u32>) -> [f32; 2] {
    let width = surface_size.width.max(1) as f32;
    let height = surface_size.height.max(1) as f32;
    let surface_aspect = width / height;

    if surface_aspect > TARGET_ASPECT_RATIO {
        [1.0, TARGET_ASPECT_RATIO / surface_aspect]
    } else {
        [surface_aspect / TARGET_ASPECT_RATIO, 1.0]
    }
}

fn min_zoom(surface_size: PhysicalSize<u32>) -> f32 {
    fit_zoom(surface_size) * MIN_ZOOM_FIT_FACTOR
}

fn fit_zoom(surface_size: PhysicalSize<u32>) -> f32 {
    let cropped = aspect_crop(surface_size);
    cropped[0].min(cropped[1])
}

fn view_uv_scale(surface_size: PhysicalSize<u32>, zoom: f32) -> [f32; 2] {
    let cropped = aspect_crop(surface_size);

    [cropped[0] / zoom, cropped[1] / zoom]
}

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        surface_size: PhysicalSize<u32>,
    ) -> Self {
        let shader_source = [
            include_str!("gates_render_header.wgsl"),
            include_str!("render.wgsl"),
        ]
        .join("\n");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D3,
                sample_type: wgpu::TextureSampleType::Uint,
            },
            count: None,
        };

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("render-params-buffer"),
            size: std::mem::size_of::<RenderParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render-bind-group-layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("render-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let renderer = Self {
            pipeline,
            bind_group_layout,
            params_buffer,
        };

        renderer.update_view(queue, CameraState::new(surface_size));
        renderer
    }

    pub fn update_view(&self, queue: &wgpu::Queue, camera: CameraState) {
        self.update_view_layer(queue, camera, 0);
    }

    pub fn update_view_layer(&self, queue: &wgpu::Queue, camera: CameraState, layer: u32) {
        let uv_scale = camera.view_params();
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&RenderParams {
                view: [uv_scale[0], uv_scale[1], camera.offset[0], camera.offset[1]],
                layer: [layer, 0, 0, 0],
            }),
        );
    }

    pub fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surface_texture: &wgpu::Texture,
        charge_view: &wgpu::TextureView,
        circuit_view: &wgpu::TextureView,
    ) {
        let output_view = surface_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("render-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(charge_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(circuit_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("render-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..Default::default()
        });

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;

        adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()
    }

    #[test]
    fn renderer_initializes_with_headless_device() {
        let Some((device, queue)) = pollster::block_on(create_headless_device()) else {
            return;
        };

        let _renderer = Renderer::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            PhysicalSize::new(128, 128),
        );
    }
}
