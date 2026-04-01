use bytemuck::{Pod, Zeroable};
use egui::TextureId;
use egui_wgpu::wgpu;
use egui_winit::winit;
use winit::dpi::PhysicalSize;

const TARGET_ASPECT_RATIO: f32 = 16.0 / 9.0;
const MAX_ZOOM: f32 = 8.0;
const MIN_ZOOM_FIT_FACTOR: f32 = 0.9;
const TOOL_PREVIEW_WIDTH: u32 = 50;
const TOOL_PREVIEW_HEIGHT: u32 = 46;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RenderParams {
    view: [f32; 4],
    arena_z: [u32; 4],
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HoverPreviewParams {
    view: [f32; 4],
    board: [u32; 4],
    cell: [u32; 4],
    circuit: [u32; 4],
    charge: [u32; 4],
    overlay: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ToolPreviewParams {
    circuit: [u32; 4],
    charge: [u32; 4],
}

#[derive(Clone, Copy)]
pub struct HoverPreviewState {
    pub cell: [u32; 2],
    pub circuit: [u32; 4],
    pub charge: u8,
    pub overlay: [f32; 4],
}

pub struct HoverPreviewRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
}

fn shader_with_gate_header(source: &str) -> String {
    format!("{}\n{}", include_str!("gates_render_header.wgsl"), source)
}

pub fn create_editor_tool_previews(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    egui_renderer: &mut egui_wgpu::Renderer,
) -> [TextureId; crate::editor::EditorTool::COUNT] {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("editor-tool-preview"),
        source: wgpu::ShaderSource::Wgsl(
            shader_with_gate_header(include_str!("editor_tool_preview.wgsl")).into(),
        ),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("editor-tool-preview-bind-group-layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("editor-tool-preview-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("editor-tool-preview-pipeline"),
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
                format: wgpu::TextureFormat::Rgba8Unorm,
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

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("editor-tool-preview-encoder"),
    });
    let mut texture_ids = Vec::with_capacity(crate::editor::EditorTool::COUNT);

    for tool in crate::editor::EditorTool::ALL {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("editor-tool-preview-texture"),
            size: wgpu::Extent3d {
                width: TOOL_PREVIEW_WIDTH,
                height: TOOL_PREVIEW_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        texture_ids.push(egui_renderer.register_native_texture(
            device,
            &view,
            wgpu::FilterMode::Nearest,
        ));

        let (circuit, charge) = tool_preview_state(tool);
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("editor-tool-preview-params-buffer"),
            size: std::mem::size_of::<ToolPreviewParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &params_buffer,
            0,
            bytemuck::bytes_of(&ToolPreviewParams {
                circuit,
                charge: [charge, 0, 0, 0],
            }),
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("editor-tool-preview-bind-group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("editor-tool-preview-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..Default::default()
        });
        render_pass.set_pipeline(&pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    queue.submit(Some(encoder.finish()));
    texture_ids.try_into().expect("tool preview count mismatch")
}

fn tool_preview_state(tool: crate::editor::EditorTool) -> ([u32; 4], u32) {
    let snapshot = match tool {
        crate::editor::EditorTool::Wire => [255, 0, 0, 0],
        crate::editor::EditorTool::Source => crate::simulation::CellSnapshot::source(0xff).words,
        crate::editor::EditorTool::Noop => crate::simulation::CellSnapshot::noop().words,
        crate::editor::EditorTool::Not => {
            crate::simulation::CellSnapshot::gate(crate::simulation::GateKind::Not).words
        }
        crate::editor::EditorTool::And => {
            crate::simulation::CellSnapshot::gate(crate::simulation::GateKind::And).words
        }
        crate::editor::EditorTool::Or => {
            crate::simulation::CellSnapshot::gate(crate::simulation::GateKind::Or).words
        }
        crate::editor::EditorTool::Xor => {
            crate::simulation::CellSnapshot::gate(crate::simulation::GateKind::Xor).words
        }
        crate::editor::EditorTool::Nand => {
            crate::simulation::CellSnapshot::gate(crate::simulation::GateKind::Nand).words
        }
        crate::editor::EditorTool::Nor => {
            crate::simulation::CellSnapshot::gate(crate::simulation::GateKind::Nor).words
        }
        crate::editor::EditorTool::Xnor => {
            crate::simulation::CellSnapshot::gate(crate::simulation::GateKind::Xnor).words
        }
    };

    (
        snapshot.map(u32::from),
        if matches!(tool, crate::editor::EditorTool::Source) {
            0xff
        } else {
            0
        },
    )
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        surface_size: PhysicalSize<u32>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render"),
            source: wgpu::ShaderSource::Wgsl(
                shader_with_gate_header(include_str!("render.wgsl")).into(),
            ),
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
        self.update_view_arena_z(queue, camera, 0);
    }

    pub fn update_view_arena_z(&self, queue: &wgpu::Queue, camera: CameraState, arena_z: u32) {
        let uv_scale = camera.view_params();
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&RenderParams {
                view: [uv_scale[0], uv_scale[1], camera.offset[0], camera.offset[1]],
                arena_z: [arena_z, 0, 0, 0],
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

impl HoverPreviewRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hover-preview"),
            source: wgpu::ShaderSource::Wgsl(
                shader_with_gate_header(include_str!("hover_preview.wgsl")).into(),
            ),
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hover-preview-params-buffer"),
            size: std::mem::size_of::<HoverPreviewParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hover-preview-bind-group-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hover-preview-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hover-preview-pipeline"),
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
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
        renderer.update(queue, CameraState::new(PhysicalSize::new(1, 1)), None);
        renderer
    }

    pub fn update(
        &self,
        queue: &wgpu::Queue,
        camera: CameraState,
        preview: Option<HoverPreviewState>,
    ) {
        let uv_scale = camera.view_params();
        let params = if let Some(preview) = preview {
            HoverPreviewParams {
                view: [uv_scale[0], uv_scale[1], camera.offset[0], camera.offset[1]],
                board: [
                    crate::simulation::GRID_WIDTH,
                    crate::simulation::GRID_HEIGHT,
                    0,
                    0,
                ],
                cell: [preview.cell[0], preview.cell[1], 0, 1],
                circuit: preview.circuit.map(u32::from),
                charge: [u32::from(preview.charge), 0, 0, 0],
                overlay: preview.overlay,
            }
        } else {
            HoverPreviewParams {
                view: [uv_scale[0], uv_scale[1], camera.offset[0], camera.offset[1]],
                board: [
                    crate::simulation::GRID_WIDTH,
                    crate::simulation::GRID_HEIGHT,
                    0,
                    0,
                ],
                cell: [0, 0, 0, 0],
                circuit: [0, 0, 0, 0],
                charge: [0, 0, 0, 0],
                overlay: [0.0, 0.0, 0.0, 0.0],
            }
        };

        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    pub fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surface_texture: &wgpu::Texture,
    ) {
        let output_view = surface_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hover-preview-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.params_buffer.as_entire_binding(),
            }],
        });

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hover-preview-pass"),
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
