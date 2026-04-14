use bytemuck::{Pod, Zeroable};
use egui::{Color32, Pos2, Rect};
use egui_wgpu::wgpu;

use crate::{
    gate_plans::{Gate, GateStoreLocation, NodeId},
    ui_config::{
        CHILD_ZOOM_PREVIEW, MAX_PULSE_CYCLES_PER_SECOND, MIN_PULSE_CYCLES_PER_SECOND,
        PULSE_CYCLES_PER_TICK,
    },
    visual_ui::{FocusedScene, PlacedGate, ViewportState, VisualWire},
};

const SHAPE_BUFFER_LABEL: &str = "scene-render-shapes";
const WIRE_BUFFER_LABEL: &str = "scene-render-wires";
const SHAPE_KIND_GATE: u32 = 0;
const SHAPE_KIND_CHILD: u32 = 1;
const SHAPE_KIND_INPUT_PORT: u32 = 2;
const SHAPE_KIND_OUTPUT_PORT: u32 = 3;
const SHAPE_KIND_CHILD_INPUT_PORT: u32 = 4;
const SHAPE_KIND_CHILD_OUTPUT_PORT: u32 = 5;
const SHAPE_KIND_ANCESTOR_PORT: u32 = 6;
const SHAPE_KIND_GATE_INPUT_MARKER: u32 = 7;
const SHAPE_KIND_GATE_OUTPUT_MARKER: u32 = 8;
const CHARGE_SOURCE_READ: u32 = 0;
const CHARGE_SOURCE_WRITE: u32 = 1;
const CHARGE_SOURCE_NONE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SceneUniform {
    surface_scene_min: [f32; 4],
    scene_size_screen_min: [f32; 4],
    source_scale_time_pulse: [f32; 4],
    grid_rect: [f32; 4],
    scene_bits: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<SceneUniform>() == 80);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShapeInstance {
    min: [f32; 2],
    max: [f32; 2],
    charge: [u32; 4],
    shape_meta: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WireInstance {
    start: [f32; 2],
    end: [f32; 2],
    path: [f32; 4],
    color: [f32; 4],
    charge: [u32; 4],
}

struct UploadedChild {
    rect: Rect,
    scene: Box<UploadedScene>,
}

struct UploadedScene {
    shape_buffer: wgpu::Buffer,
    wire_buffer: wgpu::Buffer,
    shape_count: u32,
    wire_count: u32,
    words_per_buffer: u32,
    grid_rect: Rect,
    grid_dims: [u32; 2],
    children: Vec<UploadedChild>,
}

#[derive(Clone, Copy)]
struct SceneTransform {
    source_min: Pos2,
    target_min: Pos2,
    scale: f32,
}

impl SceneTransform {
    fn identity() -> Self {
        Self {
            source_min: Pos2::ZERO,
            target_min: Pos2::ZERO,
            scale: 1.0,
        }
    }

    fn fit(source: Rect, target: Rect) -> Self {
        let scale = (target.width() / source.width().max(f32::EPSILON))
            .min(target.height() / source.height().max(f32::EPSILON));
        let fitted_size = source.size() * scale;
        let target_min = target.center() - fitted_size * 0.5;
        Self {
            source_min: source.min,
            target_min,
            scale,
        }
    }

    fn rect(self, rect: Rect) -> Rect {
        Rect::from_min_max(self.pos(rect.min), self.pos(rect.max))
    }

    fn pos(self, pos: Pos2) -> Pos2 {
        self.target_min + (pos - self.source_min) * self.scale
    }
}

pub struct SceneRenderer {
    board_pipeline: wgpu::RenderPipeline,
    shape_pipeline: wgpu::RenderPipeline,
    wire_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    uploaded_scene: Option<UploadedScene>,
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene-render-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene_render.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene-render-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene-render-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene-render-uniforms"),
            size: std::mem::size_of::<SceneUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shape_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ShapeInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2,
                1 => Float32x2,
                2 => Uint32x4,
                3 => Uint32x4
            ],
        };
        let wire_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WireInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2,
                1 => Float32x2,
                2 => Float32x4,
                3 => Float32x4,
                4 => Uint32x4
            ],
        };

        let board_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene-render-board-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_board"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_board"),
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

        let shape_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene-render-shape-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shape"),
                buffers: &[shape_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_shape"),
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

        let wire_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene-render-wire-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_wire"),
                buffers: &[wire_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_wire"),
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

        Self {
            board_pipeline,
            shape_pipeline,
            wire_pipeline,
            bind_group_layout,
            uniform_buffer,
            uploaded_scene: None,
        }
    }

    pub fn upload_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &FocusedScene,
    ) {
        self.uploaded_scene = Some(upload_scene_tree(
            device,
            queue,
            scene,
            false,
            SceneTransform::identity(),
        ));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        surface_size: [u32; 2],
        scene_rect: Option<Rect>,
        pixels_per_point: f32,
        viewport: &ViewportState,
        current_charge: &wgpu::Buffer,
        next_charge: &wgpu::Buffer,
        clear_background: bool,
        time: f32,
        pulse_rate_hz: f32,
    ) {
        let Some(uploaded_scene) = &self.uploaded_scene else {
            return;
        };
        let Some(scene_rect) = scene_rect else {
            return;
        };

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene-render-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: current_charge.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: next_charge.as_entire_binding(),
                },
            ],
        });

        let scene_rect_px = rect_to_pixels(scene_rect, pixels_per_point);
        self.draw_shapes_tree(
            queue,
            encoder,
            output_view,
            &bind_group,
            surface_size,
            scene_rect_px,
            uploaded_scene,
            uploaded_scene.grid_rect,
            viewport,
            pixels_per_point,
            clear_background,
            time,
            pulse_rate_hz,
            false,
        );
        self.draw_wires_tree(
            queue,
            encoder,
            output_view,
            &bind_group,
            surface_size,
            scene_rect_px,
            uploaded_scene,
            uploaded_scene.grid_rect,
            viewport,
            pixels_per_point,
            time,
            pulse_rate_hz,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_shapes_tree(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        surface_size: [u32; 2],
        scene_rect_px: Rect,
        scene: &UploadedScene,
        clip_world_rect: Rect,
        viewport: &ViewportState,
        pixels_per_point: f32,
        clear_background: bool,
        time: f32,
        pulse_rate_hz: f32,
        nested: bool,
    ) {
        let Some((scissor_x, scissor_y, scissor_width, scissor_height)) = self
            .write_scene_uniforms(
                queue,
                surface_size,
                scene_rect_px,
                scene,
                clip_world_rect,
                viewport,
                pixels_per_point,
                time,
                pulse_rate_hz,
                nested,
            )
        else {
            return;
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene-render-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear_background {
                        wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.05,
                            b: 0.07,
                            a: 1.0,
                        })
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..Default::default()
        });
        pass.set_scissor_rect(scissor_x, scissor_y, scissor_width, scissor_height);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_pipeline(&self.board_pipeline);
        pass.draw(0..6, 0..1);

        if scene.shape_count > 0 {
            pass.set_pipeline(&self.shape_pipeline);
            pass.set_vertex_buffer(0, scene.shape_buffer.slice(..));
            pass.draw(0..6, 0..scene.shape_count);
        }
        drop(pass);

        if viewport.zoom >= CHILD_ZOOM_PREVIEW {
            for child in &scene.children {
                self.draw_shapes_tree(
                    queue,
                    encoder,
                    output_view,
                    bind_group,
                    surface_size,
                    scene_rect_px,
                    &child.scene,
                    child.rect,
                    viewport,
                    pixels_per_point,
                    false,
                    time,
                    pulse_rate_hz,
                    true,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_wires_tree(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        surface_size: [u32; 2],
        scene_rect_px: Rect,
        scene: &UploadedScene,
        clip_world_rect: Rect,
        viewport: &ViewportState,
        pixels_per_point: f32,
        time: f32,
        pulse_rate_hz: f32,
        nested: bool,
    ) {
        let Some((scissor_x, scissor_y, scissor_width, scissor_height)) = self
            .write_scene_uniforms(
                queue,
                surface_size,
                scene_rect_px,
                scene,
                clip_world_rect,
                viewport,
                pixels_per_point,
                time,
                pulse_rate_hz,
                nested,
            )
        else {
            return;
        };

        if viewport.zoom >= CHILD_ZOOM_PREVIEW {
            for child in &scene.children {
                self.draw_wires_tree(
                    queue,
                    encoder,
                    output_view,
                    bind_group,
                    surface_size,
                    scene_rect_px,
                    &child.scene,
                    child.rect,
                    viewport,
                    pixels_per_point,
                    time,
                    pulse_rate_hz,
                    true,
                );
            }
        }

        if scene.wire_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene-render-wire-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..Default::default()
        });
        pass.set_scissor_rect(scissor_x, scissor_y, scissor_width, scissor_height);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_pipeline(&self.wire_pipeline);
        pass.set_vertex_buffer(0, scene.wire_buffer.slice(..));
        pass.draw(0..6, 0..scene.wire_count);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_scene_uniforms(
        &self,
        queue: &wgpu::Queue,
        surface_size: [u32; 2],
        scene_rect_px: Rect,
        scene: &UploadedScene,
        clip_world_rect: Rect,
        viewport: &ViewportState,
        pixels_per_point: f32,
        time: f32,
        pulse_rate_hz: f32,
        nested: bool,
    ) -> Option<(u32, u32, u32, u32)> {
        let scissor_rect =
            root_world_rect_to_screen(scene_rect_px, viewport, pixels_per_point, clip_world_rect)
                .intersect(scene_rect_px);
        let scissor = scissor_to_u32(scissor_rect, surface_size)?;

        let scene_origin_px = scene_rect_px.min + viewport.pan * pixels_per_point;
        let uniforms = SceneUniform {
            surface_scene_min: [
                surface_size[0].max(1) as f32,
                surface_size[1].max(1) as f32,
                scene_rect_px.min.x,
                scene_rect_px.min.y,
            ],
            scene_size_screen_min: [
                scene_rect_px.width(),
                scene_rect_px.height(),
                scene_origin_px.x,
                scene_origin_px.y,
            ],
            source_scale_time_pulse: [0.0, 0.0, viewport.zoom * pixels_per_point, time],
            grid_rect: [
                scene.grid_rect.min.x,
                scene.grid_rect.min.y,
                scene.grid_rect.max.x,
                scene.grid_rect.max.y,
            ],
            scene_bits: [
                scene.words_per_buffer,
                nested as u32,
                scene.grid_dims[1].max(1),
                pulse_cycles_per_second(pulse_rate_hz).to_bits(),
            ],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        Some(scissor)
    }
}

fn upload_scene_tree(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &FocusedScene,
    nested: bool,
    transform: SceneTransform,
) -> UploadedScene {
    let cached = build_scene_instances(scene, nested, transform);
    let shape_buffer = upload_buffer(device, queue, SHAPE_BUFFER_LABEL, &cached.shapes);
    let wire_buffer = upload_buffer(device, queue, WIRE_BUFFER_LABEL, &cached.wires);
    let children = scene
        .children
        .iter()
        .map(|child| {
            let child_rect = transform.rect(child.rect);
            let child_transform = SceneTransform::fit(child.scene.grid_rect, child_rect);
            UploadedChild {
                rect: child_rect,
                scene: Box::new(upload_scene_tree(
                    device,
                    queue,
                    &child.scene,
                    true,
                    child_transform,
                )),
            }
        })
        .collect();

    UploadedScene {
        shape_buffer,
        wire_buffer,
        shape_count: cached.shapes.len() as u32,
        wire_count: cached.wires.len() as u32,
        words_per_buffer: scene.words_per_buffer,
        grid_rect: transform.rect(scene.grid_rect),
        grid_dims: scene.grid_dims,
        children,
    }
}

fn upload_buffer<T: Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    values: &[T],
) -> wgpu::Buffer {
    let size = (values.len().max(1) * std::mem::size_of::<T>()) as u64;
    let usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    });
    if !values.is_empty() {
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(values));
    }
    buffer
}

struct CachedInstances {
    shapes: Vec<ShapeInstance>,
    wires: Vec<WireInstance>,
}

fn build_scene_instances(
    scene: &FocusedScene,
    nested: bool,
    transform: SceneTransform,
) -> CachedInstances {
    let mut cached = CachedInstances {
        shapes: Vec::new(),
        wires: Vec::new(),
    };

    for gate in &scene.gates {
        cached.shapes.push(ShapeInstance {
            min: [
                transform.pos(gate.rect.min + egui::vec2(10.0, 10.0)).x,
                transform.pos(gate.rect.min + egui::vec2(10.0, 10.0)).y,
            ],
            max: [
                transform.pos(gate.rect.max - egui::vec2(10.0, 10.0)).x,
                transform.pos(gate.rect.max - egui::vec2(10.0, 10.0)).y,
            ],
            charge: charge_ref_for_gate(
                scene,
                (scene.node, gate.id),
                CHARGE_SOURCE_WRITE,
                gate.gate,
            ),
            shape_meta: [SHAPE_KIND_GATE, gate_tag(gate.gate), 0, 0],
        });
        for (input_index, source_gate) in gate.input_sources.iter().enumerate() {
            let anchor = gate_anchor(gate, Some(input_index));
            cached.shapes.push(circle_instance(
                transform.pos(anchor),
                7.0 * transform.scale,
                source_gate
                    .and_then(|source| scene.gate_store.get(&source).copied())
                    .map(|store| charge_ref(store, CHARGE_SOURCE_READ, 0))
                    .unwrap_or([0, 0, CHARGE_SOURCE_NONE, 0]),
                SHAPE_KIND_GATE_INPUT_MARKER,
            ));
        }
        cached.shapes.push(circle_instance(
            transform.pos(gate_anchor(gate, None)),
            7.0 * transform.scale,
            charge_ref_for_gate(scene, (scene.node, gate.id), CHARGE_SOURCE_READ, gate.gate),
            SHAPE_KIND_GATE_OUTPUT_MARKER,
        ));
    }

    for child in &scene.children {
        cached.shapes.push(ShapeInstance {
            min: [
                transform.pos(child.rect.min).x,
                transform.pos(child.rect.min).y,
            ],
            max: [
                transform.pos(child.rect.max).x,
                transform.pos(child.rect.max).y,
            ],
            charge: [0, 0, CHARGE_SOURCE_NONE, 0],
            shape_meta: [SHAPE_KIND_CHILD, 0, 0, 0],
        });
        for port in &child.inputs {
            cached.shapes.push(circle_instance(
                transform.pos(port.anchor),
                6.5 * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                SHAPE_KIND_CHILD_INPUT_PORT,
            ));
        }
        for port in &child.outputs {
            cached.shapes.push(circle_instance(
                transform.pos(port.anchor),
                6.5 * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                SHAPE_KIND_CHILD_OUTPUT_PORT,
            ));
        }
    }

    if !nested {
        for port in &scene.input_ports {
            cached.shapes.push(circle_instance(
                transform.pos(port.anchor),
                6.5 * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                SHAPE_KIND_INPUT_PORT,
            ));
        }
        for port in &scene.output_ports {
            cached.shapes.push(circle_instance(
                transform.pos(port.anchor),
                6.5 * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                SHAPE_KIND_OUTPUT_PORT,
            ));
        }
        for port in &scene.ancestor_ports {
            cached.shapes.push(circle_instance(
                transform.pos(port.anchor),
                6.5 * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                SHAPE_KIND_ANCESTOR_PORT,
            ));
        }
    }

    for wire in &scene.wires {
        append_wire_instances(&mut cached.wires, scene, wire, transform);
    }

    cached
}

fn append_wire_instances(
    out: &mut Vec<WireInstance>,
    scene: &FocusedScene,
    wire: &VisualWire,
    transform: SceneTransform,
) {
    if wire.points.len() < 2 {
        return;
    }
    let raw_total_len: f32 = wire
        .points
        .windows(2)
        .map(|segment| segment[0].distance(segment[1]))
        .sum();
    if raw_total_len <= f32::EPSILON {
        return;
    }
    let total_len = raw_total_len * transform.scale;
    let charge = wire
        .source_gate
        .and_then(|source| scene.gate_store.get(&source).copied())
        .map(|store| charge_ref(store, CHARGE_SOURCE_READ, 0))
        .unwrap_or([0, 0, CHARGE_SOURCE_NONE, 0]);
    let color = color_to_linear(wire.color);
    let mut path_start = 0.0;
    for segment in wire.points.windows(2) {
        let start = transform.pos(segment[0]);
        let end = transform.pos(segment[1]);
        let len = start.distance(end);
        if len <= f32::EPSILON {
            continue;
        }
        let path_end = path_start + len;
        out.push(WireInstance {
            start: [start.x, start.y],
            end: [end.x, end.y],
            path: [path_start / total_len, path_end / total_len, total_len, 5.0],
            color,
            charge,
        });
        path_start = path_end;
    }
}

fn circle_instance(center: Pos2, radius: f32, charge: [u32; 4], kind: u32) -> ShapeInstance {
    ShapeInstance {
        min: [center.x - radius, center.y - radius],
        max: [center.x + radius, center.y + radius],
        charge,
        shape_meta: [kind, 0, 0, 0],
    }
}

fn charge_ref_for_gate(
    scene: &FocusedScene,
    source_gate: (NodeId, crate::gate_plans::GateId),
    source_mode: u32,
    gate: Gate,
) -> [u32; 4] {
    scene
        .gate_store
        .get(&source_gate)
        .copied()
        .map(|store| charge_ref(store, source_mode, gate_tag(gate)))
        .unwrap_or([0, 0, CHARGE_SOURCE_NONE, gate_tag(gate)])
}

fn charge_ref(store: GateStoreLocation, source_mode: u32, gate_tag: u32) -> [u32; 4] {
    [store.buffer.0, store.bit.0, source_mode, gate_tag]
}

fn gate_tag(gate: Gate) -> u32 {
    match gate {
        Gate::BitNAND { .. } => 1,
        Gate::BitAND { .. } => 2,
        Gate::BitOR { .. } => 3,
        Gate::BitNOR { .. } => 4,
        Gate::BitXOR { .. } => 5,
        Gate::BitXNOR { .. } => 6,
        Gate::BitNot { .. } => 7,
        Gate::BitNop { .. } => 8,
    }
}

fn gate_anchor(gate: &PlacedGate, input: Option<usize>) -> Pos2 {
    let local = match (gate.gate, input) {
        (Gate::BitNot { .. } | Gate::BitNop { .. }, Some(0)) => [0.08, 0.5],
        (
            Gate::BitAND { .. }
            | Gate::BitOR { .. }
            | Gate::BitXOR { .. }
            | Gate::BitNAND { .. }
            | Gate::BitNOR { .. }
            | Gate::BitXNOR { .. },
            Some(0),
        ) => [0.08, 0.3],
        (
            Gate::BitAND { .. }
            | Gate::BitOR { .. }
            | Gate::BitXOR { .. }
            | Gate::BitNAND { .. }
            | Gate::BitNOR { .. }
            | Gate::BitXNOR { .. },
            Some(1),
        ) => [0.08, 0.7],
        _ => [0.92, 0.5],
    };
    Pos2::new(
        gate.rect.left() + gate.rect.width() * local[0],
        gate.rect.top() + gate.rect.height() * local[1],
    )
}

fn pulse_cycles_per_second(pulse_rate_hz: f32) -> f32 {
    (pulse_rate_hz * PULSE_CYCLES_PER_TICK)
        .clamp(MIN_PULSE_CYCLES_PER_SECOND, MAX_PULSE_CYCLES_PER_SECOND)
}

fn rect_to_pixels(rect: Rect, pixels_per_point: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.min.x * pixels_per_point, rect.min.y * pixels_per_point),
        Pos2::new(rect.max.x * pixels_per_point, rect.max.y * pixels_per_point),
    )
}

fn root_world_rect_to_screen(
    scene_rect_px: Rect,
    viewport: &ViewportState,
    pixels_per_point: f32,
    rect: Rect,
) -> Rect {
    let offset = scene_rect_px.min + viewport.pan * pixels_per_point;
    let scale = viewport.zoom * pixels_per_point;
    Rect::from_min_max(
        offset + rect.min.to_vec2() * scale,
        offset + rect.max.to_vec2() * scale,
    )
}

fn scissor_to_u32(rect: Rect, surface_size: [u32; 2]) -> Option<(u32, u32, u32, u32)> {
    if !rect.is_positive() {
        return None;
    }

    let x = rect.min.x.max(0.0).floor() as u32;
    let y = rect.min.y.max(0.0).floor() as u32;
    let max_width = surface_size[0].saturating_sub(x);
    let max_height = surface_size[1].saturating_sub(y);
    if max_width == 0 || max_height == 0 {
        return None;
    }

    let width = rect.width().ceil() as u32;
    let height = rect.height().ceil() as u32;
    Some((
        x,
        y,
        width.min(max_width).max(1),
        height.min(max_height).max(1),
    ))
}

fn color_to_linear(color: Color32) -> [f32; 4] {
    let [r, g, b, a] = color.to_array();
    [
        srgb_to_linear(r),
        srgb_to_linear(g),
        srgb_to_linear(b),
        a as f32 / 255.0,
    ]
}

fn srgb_to_linear(channel: u8) -> f32 {
    let value = channel as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
