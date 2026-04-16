use bytemuck::{Pod, Zeroable};
use egui::{Color32, Pos2, Rect};
use egui_wgpu::wgpu;

use crate::{
    gate_plans::{Gate, GateStoreLocation, NodeId},
    ui_config::{MAX_PULSE_CYCLES_PER_SECOND, MIN_PULSE_CYCLES_PER_SECOND, PULSE_CYCLES_PER_TICK},
    visual_ui::{FocusedScene, PlacedGate, ViewportState, VisualWire},
};

// Edit and run rendering must stay visually aligned.
// Keep shared sizes, offsets, and instance-packing rules centralized here.
// Do not introduce inline visual constants in only one render path unless the
// divergence is intentional and documented at the call site.
const GRID_BUFFER_LABEL: &str = "scene-render-grids";
const GATE_BUFFER_LABEL: &str = "scene-render-gates";
const PORT_BUFFER_LABEL: &str = "scene-render-ports";
const CHILD_FRAME_BUFFER_LABEL: &str = "scene-render-child-frames";
const WIRE_BUFFER_LABEL: &str = "scene-render-wires";
const SCENE_RENDER_SHADER_LABEL: &str = "scene-render-shader";
const SCENE_RENDER_EDIT_SHADER_LABEL: &str = "scene-render-edit-shader";
const SCENE_RENDER_BIND_GROUP_LABEL: &str = "scene-render-bind-group";
const SCENE_RENDER_EDIT_BIND_GROUP_LABEL: &str = "scene-render-edit-bind-group";
const PORT_KIND_INPUT: u32 = 0;
const PORT_KIND_OUTPUT: u32 = 1;
const PORT_KIND_ANCESTOR: u32 = 2;
const PORT_KIND_CHILD_INPUT: u32 = 3;
const PORT_KIND_CHILD_OUTPUT: u32 = 4;
const PORT_KIND_GATE_INPUT_MARKER: u32 = 5;
const PORT_KIND_GATE_OUTPUT_MARKER: u32 = 6;
const CHARGE_SOURCE_READ: u32 = 0;
const CHARGE_SOURCE_WRITE: u32 = 1;
const CHARGE_SOURCE_NONE: u32 = 2;
const ZERO_CHARGE_REF: [u32; 4] = [0, 0, CHARGE_SOURCE_NONE, 0];
const GATE_BODY_INSET: f32 = 10.0;
const GATE_MARKER_RADIUS: f32 = 7.0;
const PORT_RADIUS: f32 = 6.5;
const GRID_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Uint32x4];
const GATE_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint32x4, 3 => Uint32x4];
const PORT_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint32x4, 3 => Uint32x4];
const CHILD_FRAME_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
const WIRE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Uint32x4];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SceneUniform {
    surface_size: [f32; 4],
    scene_rect: [f32; 4],
    view_scale_time: [f32; 4],
    scene_bits: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<SceneUniform>() == 64);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GridInstance {
    min: [f32; 2],
    max: [f32; 2],
    grid_min: [f32; 2],
    grid_max: [f32; 2],
    grid_dims: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GateInstance {
    min: [f32; 2],
    max: [f32; 2],
    charge: [u32; 4],
    gate_meta: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PortInstance {
    min: [f32; 2],
    max: [f32; 2],
    charge: [u32; 4],
    port_meta: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ChildFrameInstance {
    min: [f32; 2],
    max: [f32; 2],
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

struct UploadedBuffers {
    grid_buffer: wgpu::Buffer,
    gate_buffer: wgpu::Buffer,
    port_buffer: wgpu::Buffer,
    child_frame_buffer: wgpu::Buffer,
    wire_buffer: wgpu::Buffer,
}

struct UploadedScene {
    buffers: UploadedBuffers,
    grid_count: u32,
    gate_count: u32,
    port_count: u32,
    child_frame_count: u32,
    wire_count: u32,
    words_per_buffer: u32,
    children: Vec<UploadedChild>,
}

#[derive(Clone, PartialEq)]
struct UploadedChildMeta {
    id: crate::gate_plans::ChildId,
    rect: Rect,
    inputs: Vec<ScenePortKey>,
    outputs: Vec<ScenePortKey>,
}

#[derive(Clone, PartialEq)]
struct EditSceneLevelKey {
    node: NodeId,
    bounds: Rect,
    grid_rect: Rect,
    grid_dims: [u32; 2],
    words_per_buffer: u32,
    drill_child: Option<crate::gate_plans::ChildId>,
    input_ports: Vec<[u32; 2]>,
    output_ports: Vec<[u32; 2]>,
    ancestor_ports: Vec<[u32; 2]>,
    gates: Vec<SceneGateKey>,
    children: Vec<UploadedChildMeta>,
    wires: Vec<SceneWireKey>,
    nested: bool,
    transform: SceneTransformKey,
    clip_rect: Rect,
}

#[derive(Clone, PartialEq)]
struct SceneGateKey {
    id: crate::gate_plans::GateId,
    gate: Gate,
    rect: Rect,
    input_sources: [Option<(NodeId, crate::gate_plans::GateId)>; 2],
}

#[derive(Clone, PartialEq)]
struct SceneWireKey {
    source_gate: Option<(NodeId, crate::gate_plans::GateId)>,
    color: [u8; 4],
    points: Vec<[u32; 2]>,
}

#[derive(Clone, Copy, PartialEq)]
struct ScenePortKey {
    source_gate: (NodeId, crate::gate_plans::GateId),
    anchor: [u32; 2],
}

#[derive(Clone, Copy, PartialEq)]
struct SceneTransformKey {
    source_min: [u32; 2],
    target_min: [u32; 2],
    scale: u32,
}

struct UploadedEditSceneLevel {
    key: EditSceneLevelKey,
    clip_rect: Rect,
    buffers: UploadedBuffers,
    grid_count: u32,
    gate_count: u32,
    port_count: u32,
    child_frame_count: u32,
    wire_count: u32,
    words_per_buffer: u32,
}

#[derive(Clone, Copy)]
struct SceneTransform {
    source_min: Pos2,
    target_min: Pos2,
    scale: f32,
}

const BASE_WIRE_WIDTH: f32 = 5.0;
const DRILL_IN_VISIBLE_COVERAGE_THRESHOLD: f32 = 0.20;
const DRILL_IN_FOCUS_REGION: f32 = 0.55;

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
    runtime_pipelines: ScenePipelines,
    edit_pipelines: ScenePipelines,
    uniform_buffer: wgpu::Buffer,
    uploaded_scene: Option<UploadedScene>,
    uploaded_edit_stack: Vec<UploadedEditSceneLevel>,
}

struct ScenePipelines {
    grid_pipeline: wgpu::RenderPipeline,
    gate_pipeline: wgpu::RenderPipeline,
    port_pipeline: wgpu::RenderPipeline,
    child_frame_pipeline: wgpu::RenderPipeline,
    wire_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let runtime_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(SCENE_RENDER_SHADER_LABEL),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("gate_shared.wgsl"),
                    "\n",
                    include_str!("scene_render.wgsl"),
                    "\n",
                    include_str!("scene_render_runtime.wgsl")
                )
                .into(),
            ),
        });

        let edit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(SCENE_RENDER_EDIT_SHADER_LABEL),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("gate_shared.wgsl"),
                    "\n",
                    include_str!("scene_render.wgsl"),
                    "\n",
                    include_str!("scene_render_edit.wgsl")
                )
                .into(),
            ),
        });

        let runtime_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        let edit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("scene-render-edit-bind-group-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let runtime_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("scene-render-pipeline-layout"),
                bind_group_layouts: &[&runtime_bind_group_layout],
                push_constant_ranges: &[],
            });

        let edit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene-render-edit-pipeline-layout"),
            bind_group_layouts: &[&edit_bind_group_layout],
            push_constant_ranges: &[],
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene-render-uniforms"),
            size: std::mem::size_of::<SceneUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let runtime_pipelines = create_scene_pipelines(
            device,
            surface_format,
            &runtime_shader,
            &runtime_pipeline_layout,
            runtime_bind_group_layout,
            "scene-render",
        );
        let edit_pipelines = create_scene_pipelines(
            device,
            surface_format,
            &edit_shader,
            &edit_pipeline_layout,
            edit_bind_group_layout,
            "scene-render-edit",
        );

        Self {
            runtime_pipelines,
            edit_pipelines,
            uniform_buffer,
            uploaded_scene: None,
            uploaded_edit_stack: Vec::new(),
        }
    }

    pub fn upload_runtime_scene(
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
        self.uploaded_edit_stack.clear();
    }

    pub fn upload_edit_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &FocusedScene,
    ) {
        self.uploaded_scene = None;
        let mut levels = Vec::new();
        collect_edit_scene_levels(
            scene,
            false,
            SceneTransform::identity(),
            scene.bounds,
            &mut levels,
        );

        let shared_prefix = self
            .uploaded_edit_stack
            .iter()
            .zip(levels.iter())
            .take_while(|(uploaded, next)| uploaded.key == next.key)
            .count();

        let mut reusable_suffix = self
            .uploaded_edit_stack
            .split_off(shared_prefix)
            .into_iter();
        for level in levels.into_iter().skip(shared_prefix) {
            self.uploaded_edit_stack.push(upload_edit_scene_level(
                device,
                queue,
                reusable_suffix.next(),
                level,
            ));
        }
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
        hover_world: Option<Pos2>,
        pixels_per_point: f32,
        viewport: &ViewportState,
        current_charge: &wgpu::Buffer,
        next_charge: &wgpu::Buffer,
        time: f32,
        pulse_rate_hz: f32,
    ) {
        let Some(scene_rect) = scene_rect else {
            return;
        };

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(SCENE_RENDER_BIND_GROUP_LABEL),
            layout: &self.runtime_pipelines.bind_group_layout,
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

        self.draw_uploaded_scene(
            queue,
            encoder,
            output_view,
            &bind_group,
            &self.runtime_pipelines,
            surface_size,
            scene_rect,
            hover_world,
            pixels_per_point,
            viewport,
            time,
            pulse_rate_hz,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_edit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        surface_size: [u32; 2],
        scene_rect: Option<Rect>,
        _hover_world: Option<Pos2>,
        pixels_per_point: f32,
        viewport: &ViewportState,
        time: f32,
        pulse_rate_hz: f32,
    ) {
        let Some(scene_rect) = scene_rect else {
            return;
        };
        if self.uploaded_edit_stack.is_empty() {
            return;
        }

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(SCENE_RENDER_EDIT_BIND_GROUP_LABEL),
            layout: &self.edit_pipelines.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_entire_binding(),
            }],
        });

        self.draw_uploaded_edit_stack(
            queue,
            encoder,
            output_view,
            &bind_group,
            &self.edit_pipelines,
            surface_size,
            scene_rect,
            pixels_per_point,
            viewport,
            time,
            pulse_rate_hz,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_uploaded_scene(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        pipelines: &ScenePipelines,
        surface_size: [u32; 2],
        scene_rect: Rect,
        hover_world: Option<Pos2>,
        pixels_per_point: f32,
        viewport: &ViewportState,
        time: f32,
        pulse_rate_hz: f32,
    ) {
        let Some(uploaded_scene) = &self.uploaded_scene else {
            return;
        };
        let scene_rect_px = rect_to_pixels(scene_rect, pixels_per_point);
        self.draw_scene_tree_solids(
            queue,
            encoder,
            output_view,
            &bind_group,
            pipelines,
            surface_size,
            scene_rect_px,
            uploaded_scene,
            scene_rect_px,
            hover_world,
            viewport,
            pixels_per_point,
            time,
            pulse_rate_hz,
            false,
        );
        self.draw_scene_tree_wires(
            queue,
            encoder,
            output_view,
            &bind_group,
            pipelines,
            surface_size,
            scene_rect_px,
            uploaded_scene,
            scene_rect_px,
            hover_world,
            viewport,
            pixels_per_point,
            time,
            pulse_rate_hz,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_uploaded_edit_stack(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        pipelines: &ScenePipelines,
        surface_size: [u32; 2],
        scene_rect: Rect,
        pixels_per_point: f32,
        viewport: &ViewportState,
        time: f32,
        pulse_rate_hz: f32,
    ) {
        let scene_rect_px = rect_to_pixels(scene_rect, pixels_per_point);
        for (level_index, level) in self.uploaded_edit_stack.iter().enumerate() {
            let clip_world_rect = if level_index == 0 {
                scene_rect_px
            } else {
                level.clip_rect
            };
            let Some((scissor_x, scissor_y, scissor_width, scissor_height)) = self
                .write_scene_uniforms(
                    queue,
                    surface_size,
                    scene_rect_px,
                    clip_world_rect,
                    viewport,
                    pixels_per_point,
                    time,
                    pulse_rate_hz,
                    level.words_per_buffer,
                    level_index > 0,
                )
            else {
                continue;
            };

            self.draw_instance_pass(
                encoder,
                output_view,
                bind_group,
                &pipelines.grid_pipeline,
                level.buffers.grid_buffer.slice(..),
                level.grid_count,
                scissor_x,
                scissor_y,
                scissor_width,
                scissor_height,
                "scene-render-edit-grid-pass",
            );
            self.draw_instance_pass(
                encoder,
                output_view,
                bind_group,
                &pipelines.child_frame_pipeline,
                level.buffers.child_frame_buffer.slice(..),
                level.child_frame_count,
                scissor_x,
                scissor_y,
                scissor_width,
                scissor_height,
                "scene-render-edit-child-frame-pass",
            );
            self.draw_instance_pass(
                encoder,
                output_view,
                bind_group,
                &pipelines.gate_pipeline,
                level.buffers.gate_buffer.slice(..),
                level.gate_count,
                scissor_x,
                scissor_y,
                scissor_width,
                scissor_height,
                "scene-render-edit-gate-pass",
            );
            self.draw_instance_pass(
                encoder,
                output_view,
                bind_group,
                &pipelines.port_pipeline,
                level.buffers.port_buffer.slice(..),
                level.port_count,
                scissor_x,
                scissor_y,
                scissor_width,
                scissor_height,
                "scene-render-edit-port-pass",
            );
        }
        for (level_index, level) in self.uploaded_edit_stack.iter().enumerate() {
            let clip_world_rect = if level_index == 0 {
                scene_rect_px
            } else {
                level.clip_rect
            };
            let Some((scissor_x, scissor_y, scissor_width, scissor_height)) = self
                .write_scene_uniforms(
                    queue,
                    surface_size,
                    scene_rect_px,
                    clip_world_rect,
                    viewport,
                    pixels_per_point,
                    time,
                    pulse_rate_hz,
                    level.words_per_buffer,
                    level_index > 0,
                )
            else {
                continue;
            };
            self.draw_instance_pass(
                encoder,
                output_view,
                bind_group,
                &pipelines.wire_pipeline,
                level.buffers.wire_buffer.slice(..),
                level.wire_count,
                scissor_x,
                scissor_y,
                scissor_width,
                scissor_height,
                "scene-render-edit-wire-pass",
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_scene_tree_solids(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        pipelines: &ScenePipelines,
        surface_size: [u32; 2],
        scene_rect_px: Rect,
        scene: &UploadedScene,
        clip_world_rect: Rect,
        hover_world: Option<Pos2>,
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
                clip_world_rect,
                viewport,
                pixels_per_point,
                time,
                pulse_rate_hz,
                scene.words_per_buffer,
                nested,
            )
        else {
            return;
        };

        self.draw_instance_pass(
            encoder,
            output_view,
            bind_group,
            &pipelines.grid_pipeline,
            scene.buffers.grid_buffer.slice(..),
            scene.grid_count,
            scissor_x,
            scissor_y,
            scissor_width,
            scissor_height,
            "scene-render-grid-pass",
        );
        self.draw_instance_pass(
            encoder,
            output_view,
            bind_group,
            &pipelines.child_frame_pipeline,
            scene.buffers.child_frame_buffer.slice(..),
            scene.child_frame_count,
            scissor_x,
            scissor_y,
            scissor_width,
            scissor_height,
            "scene-render-child-frame-pass",
        );
        self.draw_instance_pass(
            encoder,
            output_view,
            bind_group,
            &pipelines.gate_pipeline,
            scene.buffers.gate_buffer.slice(..),
            scene.gate_count,
            scissor_x,
            scissor_y,
            scissor_width,
            scissor_height,
            "scene-render-gate-pass",
        );
        self.draw_instance_pass(
            encoder,
            output_view,
            bind_group,
            &pipelines.port_pipeline,
            scene.buffers.port_buffer.slice(..),
            scene.port_count,
            scissor_x,
            scissor_y,
            scissor_width,
            scissor_height,
            "scene-render-port-pass",
        );

        if let Some(child) = select_drill_child(
            scene,
            viewport_visible_world_rect(scene_rect_px, viewport, pixels_per_point),
            hover_world,
        ) {
            self.draw_scene_tree_solids(
                queue,
                encoder,
                output_view,
                bind_group,
                pipelines,
                surface_size,
                scene_rect_px,
                &child.scene,
                child.rect,
                hover_world,
                viewport,
                pixels_per_point,
                time,
                pulse_rate_hz,
                true,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_scene_tree_wires(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        pipelines: &ScenePipelines,
        surface_size: [u32; 2],
        scene_rect_px: Rect,
        scene: &UploadedScene,
        clip_world_rect: Rect,
        hover_world: Option<Pos2>,
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
                clip_world_rect,
                viewport,
                pixels_per_point,
                time,
                pulse_rate_hz,
                scene.words_per_buffer,
                nested,
            )
        else {
            return;
        };

        self.draw_instance_pass(
            encoder,
            output_view,
            bind_group,
            &pipelines.wire_pipeline,
            scene.buffers.wire_buffer.slice(..),
            scene.wire_count,
            scissor_x,
            scissor_y,
            scissor_width,
            scissor_height,
            "scene-render-wire-pass",
        );

        if let Some(child) = select_drill_child(
            scene,
            viewport_visible_world_rect(scene_rect_px, viewport, pixels_per_point),
            hover_world,
        ) {
            self.draw_scene_tree_wires(
                queue,
                encoder,
                output_view,
                bind_group,
                pipelines,
                surface_size,
                scene_rect_px,
                &child.scene,
                child.rect,
                hover_world,
                viewport,
                pixels_per_point,
                time,
                pulse_rate_hz,
                true,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_instance_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        pipeline: &wgpu::RenderPipeline,
        buffer_slice: wgpu::BufferSlice<'_>,
        instance_count: u32,
        scissor_x: u32,
        scissor_y: u32,
        scissor_width: u32,
        scissor_height: u32,
        label: &str,
    ) {
        if instance_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
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
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, buffer_slice);
        pass.draw(0..6, 0..instance_count);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_scene_uniforms(
        &self,
        queue: &wgpu::Queue,
        surface_size: [u32; 2],
        scene_rect_px: Rect,
        clip_world_rect: Rect,
        viewport: &ViewportState,
        pixels_per_point: f32,
        time: f32,
        pulse_rate_hz: f32,
        words_per_buffer: u32,
        nested: bool,
    ) -> Option<(u32, u32, u32, u32)> {
        let scissor_rect = if nested {
            root_world_rect_to_screen(scene_rect_px, viewport, pixels_per_point, clip_world_rect)
                .intersect(scene_rect_px)
        } else {
            scene_rect_px
        };
        let scissor = scissor_to_u32(scissor_rect, surface_size)?;
        let scene_origin_px = scene_rect_px.min + viewport.pan * pixels_per_point;
        let uniforms = SceneUniform {
            surface_size: [
                surface_size[0].max(1) as f32,
                surface_size[1].max(1) as f32,
                0.0,
                0.0,
            ],
            scene_rect: [
                scene_rect_px.min.x,
                scene_rect_px.min.y,
                scene_origin_px.x,
                scene_origin_px.y,
            ],
            view_scale_time: [0.0, 0.0, viewport.zoom * pixels_per_point, time],
            scene_bits: [
                words_per_buffer,
                nested as u32,
                pulse_cycles_per_second(pulse_rate_hz).to_bits(),
                0,
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
    let grid_buffer = upload_buffer(device, queue, GRID_BUFFER_LABEL, &cached.grids);
    let gate_buffer = upload_buffer(device, queue, GATE_BUFFER_LABEL, &cached.gates);
    let port_buffer = upload_buffer(device, queue, PORT_BUFFER_LABEL, &cached.ports);
    let child_frame_buffer = upload_buffer(
        device,
        queue,
        CHILD_FRAME_BUFFER_LABEL,
        &cached.child_frames,
    );
    let wire_buffer = upload_buffer(device, queue, WIRE_BUFFER_LABEL, &cached.wires);
    let children = scene
        .children
        .iter()
        .map(|child| {
            let child_rect = transform.rect(child.rect);
            UploadedChild {
                rect: child_rect,
                scene: Box::new(upload_scene_tree(
                    device,
                    queue,
                    &child.scene,
                    true,
                    SceneTransform::fit(child.scene.grid_rect, child_rect),
                )),
            }
        })
        .collect();

    UploadedScene {
        buffers: UploadedBuffers {
            grid_buffer,
            gate_buffer,
            port_buffer,
            child_frame_buffer,
            wire_buffer,
        },
        grid_count: cached.grids.len() as u32,
        gate_count: cached.gates.len() as u32,
        port_count: cached.ports.len() as u32,
        child_frame_count: cached.child_frames.len() as u32,
        wire_count: cached.wires.len() as u32,
        words_per_buffer: scene.words_per_buffer,
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

fn collect_edit_scene_levels<'a>(
    scene: &'a FocusedScene,
    nested: bool,
    transform: SceneTransform,
    clip_rect: Rect,
    out: &mut Vec<EditSceneLevelUpload<'a>>,
) {
    out.push(EditSceneLevelUpload {
        key: edit_scene_level_key(scene, nested, transform, clip_rect),
        scene,
        nested,
        transform,
        clip_rect,
    });

    let Some(drill_child) = scene.drill_child else {
        return;
    };
    let Some(child) = scene.children.iter().find(|child| child.id == drill_child) else {
        return;
    };
    let child_rect = transform.rect(child.rect);
    collect_edit_scene_levels(
        &child.scene,
        true,
        SceneTransform::fit(child.scene.grid_rect, child_rect),
        child_rect,
        out,
    );
}

struct EditSceneLevelUpload<'a> {
    key: EditSceneLevelKey,
    scene: &'a FocusedScene,
    nested: bool,
    transform: SceneTransform,
    clip_rect: Rect,
}

fn upload_edit_scene_level(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    previous: Option<UploadedEditSceneLevel>,
    level: EditSceneLevelUpload<'_>,
) -> UploadedEditSceneLevel {
    let cached = build_scene_instances(level.scene, level.nested, level.transform);
    let mut buffers = previous
        .map(|level| level.buffers)
        .unwrap_or_else(|| UploadedBuffers {
            grid_buffer: upload_buffer(device, queue, GRID_BUFFER_LABEL, &[] as &[GridInstance]),
            gate_buffer: upload_buffer(device, queue, GATE_BUFFER_LABEL, &[] as &[GateInstance]),
            port_buffer: upload_buffer(device, queue, PORT_BUFFER_LABEL, &[] as &[PortInstance]),
            child_frame_buffer: upload_buffer(
                device,
                queue,
                CHILD_FRAME_BUFFER_LABEL,
                &[] as &[ChildFrameInstance],
            ),
            wire_buffer: upload_buffer(device, queue, WIRE_BUFFER_LABEL, &[] as &[WireInstance]),
        });
    write_buffer_reuse(
        device,
        queue,
        &mut buffers.grid_buffer,
        GRID_BUFFER_LABEL,
        &cached.grids,
    );
    write_buffer_reuse(
        device,
        queue,
        &mut buffers.gate_buffer,
        GATE_BUFFER_LABEL,
        &cached.gates,
    );
    write_buffer_reuse(
        device,
        queue,
        &mut buffers.port_buffer,
        PORT_BUFFER_LABEL,
        &cached.ports,
    );
    write_buffer_reuse(
        device,
        queue,
        &mut buffers.child_frame_buffer,
        CHILD_FRAME_BUFFER_LABEL,
        &cached.child_frames,
    );
    write_buffer_reuse(
        device,
        queue,
        &mut buffers.wire_buffer,
        WIRE_BUFFER_LABEL,
        &cached.wires,
    );

    UploadedEditSceneLevel {
        key: level.key,
        clip_rect: level.clip_rect,
        buffers,
        grid_count: cached.grids.len() as u32,
        gate_count: cached.gates.len() as u32,
        port_count: cached.ports.len() as u32,
        child_frame_count: cached.child_frames.len() as u32,
        wire_count: cached.wires.len() as u32,
        words_per_buffer: level.scene.words_per_buffer,
    }
}

fn write_buffer_reuse<T: Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    label: &str,
    values: &[T],
) {
    let required_size = (values.len().max(1) * std::mem::size_of::<T>()) as u64;
    if buffer.size() < required_size {
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: required_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
    if !values.is_empty() {
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(values));
    }
}

fn edit_scene_level_key(
    scene: &FocusedScene,
    nested: bool,
    transform: SceneTransform,
    clip_rect: Rect,
) -> EditSceneLevelKey {
    EditSceneLevelKey {
        node: scene.node,
        bounds: scene.bounds,
        grid_rect: scene.grid_rect,
        grid_dims: scene.grid_dims,
        words_per_buffer: scene.words_per_buffer,
        drill_child: scene.drill_child,
        input_ports: scene
            .input_ports
            .iter()
            .map(|port| pos_key(port.anchor))
            .collect(),
        output_ports: scene
            .output_ports
            .iter()
            .map(|port| pos_key(port.anchor))
            .collect(),
        ancestor_ports: scene
            .ancestor_ports
            .iter()
            .map(|port| pos_key(port.anchor))
            .collect(),
        gates: scene
            .gates
            .iter()
            .map(|gate| SceneGateKey {
                id: gate.id,
                gate: gate.gate,
                rect: gate.rect,
                input_sources: gate.input_sources,
            })
            .collect(),
        children: scene
            .children
            .iter()
            .map(|child| UploadedChildMeta {
                id: child.id,
                rect: child.rect,
                inputs: child
                    .inputs
                    .iter()
                    .map(|port| ScenePortKey {
                        source_gate: port.source_gate,
                        anchor: pos_key(port.anchor),
                    })
                    .collect(),
                outputs: child
                    .outputs
                    .iter()
                    .map(|port| ScenePortKey {
                        source_gate: port.source_gate,
                        anchor: pos_key(port.anchor),
                    })
                    .collect(),
            })
            .collect(),
        wires: scene
            .wires
            .iter()
            .map(|wire| SceneWireKey {
                source_gate: wire.source_gate,
                color: wire.color.to_array(),
                points: wire.points.iter().map(|point| pos_key(*point)).collect(),
            })
            .collect(),
        nested,
        transform: SceneTransformKey {
            source_min: pos_key(transform.source_min),
            target_min: pos_key(transform.target_min),
            scale: transform.scale.to_bits(),
        },
        clip_rect,
    }
}

fn pos_key(pos: Pos2) -> [u32; 2] {
    [pos.x.to_bits(), pos.y.to_bits()]
}

struct CachedInstances {
    grids: Vec<GridInstance>,
    gates: Vec<GateInstance>,
    ports: Vec<PortInstance>,
    child_frames: Vec<ChildFrameInstance>,
    wires: Vec<WireInstance>,
}

fn build_scene_instances(
    scene: &FocusedScene,
    nested: bool,
    transform: SceneTransform,
) -> CachedInstances {
    let mut cached = CachedInstances {
        grids: vec![GridInstance {
            min: [
                transform.pos(scene.bounds.min).x,
                transform.pos(scene.bounds.min).y,
            ],
            max: [
                transform.pos(scene.bounds.max).x,
                transform.pos(scene.bounds.max).y,
            ],
            grid_min: [
                transform.pos(scene.grid_rect.min).x,
                transform.pos(scene.grid_rect.min).y,
            ],
            grid_max: [
                transform.pos(scene.grid_rect.max).x,
                transform.pos(scene.grid_rect.max).y,
            ],
            grid_dims: [scene.grid_dims[0], scene.grid_dims[1], nested as u32, 0],
        }],
        gates: Vec::new(),
        ports: Vec::new(),
        child_frames: Vec::new(),
        wires: Vec::new(),
    };

    for gate in &scene.gates {
        let body_rect = inset_rect(gate.rect, GATE_BODY_INSET);
        cached.gates.push(GateInstance {
            min: [
                transform.pos(body_rect.min).x,
                transform.pos(body_rect.min).y,
            ],
            max: [
                transform.pos(body_rect.max).x,
                transform.pos(body_rect.max).y,
            ],
            charge: charge_ref_for_gate(
                scene,
                (scene.node, gate.id),
                CHARGE_SOURCE_WRITE,
                gate.gate,
            ),
            gate_meta: [gate_tag(gate.gate), 0, 0, 0],
        });
        for (input_index, source_gate) in gate.input_sources.iter().enumerate() {
            let anchor = gate_anchor(gate, Some(input_index));
            cached.ports.push(circle_instance(
                transform.pos(anchor),
                GATE_MARKER_RADIUS * transform.scale,
                source_gate
                    .and_then(|source| scene.gate_store.get(&source).copied())
                    .map(|store| charge_ref(store, CHARGE_SOURCE_READ, 0))
                    .unwrap_or(ZERO_CHARGE_REF),
                PORT_KIND_GATE_INPUT_MARKER,
            ));
        }
        cached.ports.push(circle_instance(
            transform.pos(gate_anchor(gate, None)),
            GATE_MARKER_RADIUS * transform.scale,
            charge_ref_for_gate(scene, (scene.node, gate.id), CHARGE_SOURCE_READ, gate.gate),
            PORT_KIND_GATE_OUTPUT_MARKER,
        ));
    }

    for child in &scene.children {
        cached.child_frames.push(ChildFrameInstance {
            min: [
                transform.pos(child.rect.min).x,
                transform.pos(child.rect.min).y,
            ],
            max: [
                transform.pos(child.rect.max).x,
                transform.pos(child.rect.max).y,
            ],
        });
        for port in &child.inputs {
            cached.ports.push(circle_instance(
                transform.pos(port.anchor),
                PORT_RADIUS * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                PORT_KIND_CHILD_INPUT,
            ));
        }
        for port in &child.outputs {
            cached.ports.push(circle_instance(
                transform.pos(port.anchor),
                PORT_RADIUS * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                PORT_KIND_CHILD_OUTPUT,
            ));
        }
    }

    if !nested {
        for port in &scene.input_ports {
            cached.ports.push(circle_instance(
                transform.pos(port.anchor),
                PORT_RADIUS * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                PORT_KIND_INPUT,
            ));
        }
        for port in &scene.output_ports {
            cached.ports.push(circle_instance(
                transform.pos(port.anchor),
                PORT_RADIUS * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                PORT_KIND_OUTPUT,
            ));
        }
        for port in &scene.ancestor_ports {
            cached.ports.push(circle_instance(
                transform.pos(port.anchor),
                PORT_RADIUS * transform.scale,
                charge_ref_for_gate(
                    scene,
                    port.source_gate,
                    CHARGE_SOURCE_READ,
                    Gate::BitNop {
                        src: crate::gate_plans::SignalRef::ThisGate(port.source_gate.1),
                    },
                ),
                PORT_KIND_ANCESTOR,
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
        .unwrap_or(ZERO_CHARGE_REF);
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
            path: [
                path_start / total_len,
                path_end / total_len,
                total_len,
                BASE_WIRE_WIDTH * transform.scale,
            ],
            color,
            charge,
        });
        path_start = path_end;
    }
}

fn circle_instance(center: Pos2, radius: f32, charge: [u32; 4], port_kind: u32) -> PortInstance {
    PortInstance {
        min: [center.x - radius, center.y - radius],
        max: [center.x + radius, center.y + radius],
        charge,
        port_meta: [port_kind, 0, 0, 0],
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

fn viewport_visible_world_rect(
    scene_rect_px: Rect,
    viewport: &ViewportState,
    pixels_per_point: f32,
) -> Rect {
    let scale = (viewport.zoom * pixels_per_point).max(f32::EPSILON);
    let offset = scene_rect_px.min + viewport.pan * pixels_per_point;
    Rect::from_min_max(
        Pos2::new(
            (scene_rect_px.min.x - offset.x) / scale,
            (scene_rect_px.min.y - offset.y) / scale,
        ),
        Pos2::new(
            (scene_rect_px.max.x - offset.x) / scale,
            (scene_rect_px.max.y - offset.y) / scale,
        ),
    )
}

fn select_drill_child(
    scene: &UploadedScene,
    visible_world_rect: Rect,
    hover_world: Option<Pos2>,
) -> Option<&UploadedChild> {
    select_drill_child_index(
        scene.children.len(),
        |index| scene.children[index].rect,
        visible_world_rect,
        hover_world,
    )
    .and_then(|index| scene.children.get(index))
}

fn select_drill_child_index(
    child_count: usize,
    child_rect: impl Fn(usize) -> Rect,
    visible_world_rect: Rect,
    hover_world: Option<Pos2>,
) -> Option<usize> {
    let viewport_area = rect_area(visible_world_rect).max(f32::EPSILON);
    let viewport_center = visible_world_rect.center();
    if let Some(hover_world) = hover_world {
        for index in 0..child_count {
            if child_rect(index).contains(hover_world) {
                return Some(index);
            }
        }
    }
    let mut candidates = (0..child_count)
        .filter(|&index| child_focus_rect(child_rect(index)).contains(viewport_center))
        .filter_map(|index| {
            let rect = child_rect(index);
            let overlap = rect.intersect(visible_world_rect);
            let visible_coverage = rect_area(overlap) / viewport_area;
            (visible_coverage >= DRILL_IN_VISIBLE_COVERAGE_THRESHOLD)
                .then_some((visible_coverage, index))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(a, _), (b, _)| b.total_cmp(a));
    match candidates.as_slice() {
        [] => None,
        [(_, index)] => Some(*index),
        [(best, index), (next, _), ..] if (best - next).abs() > 0.001 => Some(*index),
        _ => None,
    }
}

fn child_focus_rect(rect: Rect) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * DRILL_IN_FOCUS_REGION)
}

fn rect_area(rect: Rect) -> f32 {
    rect.width().max(0.0) * rect.height().max(0.0)
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

fn create_scene_pipelines(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    pipeline_layout: &wgpu::PipelineLayout,
    bind_group_layout: wgpu::BindGroupLayout,
    label_prefix: &str,
) -> ScenePipelines {
    let make_pipeline =
        |suffix: &str, vertex: &str, fragment: &str, buffers: &[wgpu::VertexBufferLayout<'_>]| {
            let label = format!("{label_prefix}-{suffix}-pipeline");
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&label),
                layout: Some(pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vertex),
                    buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fragment),
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
            })
        };

    ScenePipelines {
        grid_pipeline: make_pipeline("grid", "vs_grid", "fs_grid", &[grid_layout()]),
        gate_pipeline: make_pipeline("gate", "vs_gate", "fs_gate", &[gate_layout()]),
        port_pipeline: make_pipeline("port", "vs_port", "fs_port", &[port_layout()]),
        child_frame_pipeline: make_pipeline(
            "child-frame",
            "vs_child_frame",
            "fs_child_frame",
            &[child_frame_layout()],
        ),
        wire_pipeline: make_pipeline("wire", "vs_wire", "fs_wire", &[wire_layout()]),
        bind_group_layout,
    }
}

fn grid_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GridInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &GRID_ATTRIBUTES,
    }
}

fn gate_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GateInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &GATE_ATTRIBUTES,
    }
}

fn port_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<PortInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &PORT_ATTRIBUTES,
    }
}

fn child_frame_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ChildFrameInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &CHILD_FRAME_ATTRIBUTES,
    }
}

fn wire_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<WireInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &WIRE_ATTRIBUTES,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        gate_plans::{ChildId, GateId, PortId, PortLocation},
        visual_ui::{FocusedScene, PlacedPort},
    };
    use foldhash::HashMap;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LayerKind {
        Solids,
        Wires,
    }

    #[test]
    fn hover_selection_picks_hovered_child() {
        let children = [
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            Rect::from_min_max(Pos2::new(60.0, 0.0), Pos2::new(100.0, 40.0)),
        ];

        let selected = select_drill_child_index(
            children.len(),
            |index| children[index],
            Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)),
            Some(Pos2::new(75.0, 20.0)),
        );

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn ambiguous_auto_selection_returns_none() {
        let children = [
            Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(60.0, 60.0)),
            Rect::from_min_max(Pos2::new(40.0, 10.0), Pos2::new(90.0, 60.0)),
        ];

        let selected = select_drill_child_index(
            children.len(),
            |index| children[index],
            Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)),
            None,
        );

        assert_eq!(selected, None);
    }

    #[test]
    fn dominant_auto_selection_picks_larger_child() {
        let children = [
            Rect::from_min_max(Pos2::new(5.0, 5.0), Pos2::new(85.0, 85.0)),
            Rect::from_min_max(Pos2::new(70.0, 70.0), Pos2::new(95.0, 95.0)),
        ];

        let selected = select_drill_child_index(
            children.len(),
            |index| children[index],
            Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)),
            None,
        );

        assert_eq!(selected, Some(0));
    }

    #[test]
    fn focused_scene_layer_order_draws_all_solids_before_any_wires() {
        let scene = test_scene(
            0,
            Some(ChildId(0)),
            vec![test_child(
                0,
                test_scene(
                    1,
                    Some(ChildId(0)),
                    vec![test_child(0, test_scene(2, None, vec![]))],
                ),
            )],
        );

        assert_eq!(
            focused_scene_layer_order(&scene),
            vec![
                (NodeId(0), LayerKind::Solids),
                (NodeId(1), LayerKind::Solids),
                (NodeId(2), LayerKind::Solids),
                (NodeId(0), LayerKind::Wires),
                (NodeId(1), LayerKind::Wires),
                (NodeId(2), LayerKind::Wires),
            ]
        );
    }

    #[test]
    fn edit_scene_level_key_changes_when_child_ports_change() {
        let mut scene = test_scene(0, None, vec![test_child(0, test_scene(1, None, vec![]))]);
        scene.children[0]
            .inputs
            .push(test_port(0, Pos2::new(24.0, 52.0)));

        let before = edit_scene_level_key(&scene, false, SceneTransform::identity(), scene.bounds);

        scene.children[0].inputs[0].anchor = Pos2::new(28.0, 52.0);

        let after = edit_scene_level_key(&scene, false, SceneTransform::identity(), scene.bounds);

        assert!(before != after);
    }

    fn focused_scene_layer_order(scene: &FocusedScene) -> Vec<(NodeId, LayerKind)> {
        let mut order = Vec::new();
        collect_focused_scene_solids(scene, &mut order);
        collect_focused_scene_wires(scene, &mut order);
        order
    }

    fn collect_focused_scene_solids(scene: &FocusedScene, out: &mut Vec<(NodeId, LayerKind)>) {
        out.push((scene.node, LayerKind::Solids));
        if let Some(child) = selected_focused_child(scene) {
            collect_focused_scene_solids(&child.scene, out);
        }
    }

    fn collect_focused_scene_wires(scene: &FocusedScene, out: &mut Vec<(NodeId, LayerKind)>) {
        out.push((scene.node, LayerKind::Wires));
        if let Some(child) = selected_focused_child(scene) {
            collect_focused_scene_wires(&child.scene, out);
        }
    }

    fn selected_focused_child(scene: &FocusedScene) -> Option<&crate::visual_ui::PlacedChild> {
        let drill_child = scene.drill_child?;
        scene.children.iter().find(|child| child.id == drill_child)
    }

    fn test_scene(
        node: u32,
        drill_child: Option<ChildId>,
        children: Vec<crate::visual_ui::PlacedChild>,
    ) -> FocusedScene {
        let grid_rect = Rect::from_min_max(Pos2::new(8.0, 32.0), Pos2::new(88.0, 112.0));
        FocusedScene {
            node: NodeId(node),
            title: format!("Scene {node}"),
            bounds: Rect::from_min_max(Pos2::ZERO, Pos2::new(96.0, 120.0)),
            words_per_buffer: 0,
            gate_store: Arc::new(HashMap::default()),
            grid_rect,
            grid_dims: [2, 2],
            input_ports: Vec::new(),
            output_ports: Vec::new(),
            gates: Vec::new(),
            children,
            drill_child,
            ancestor_ports: Vec::new(),
            wires: Vec::new(),
        }
    }

    fn test_child(id: u32, scene: FocusedScene) -> crate::visual_ui::PlacedChild {
        crate::visual_ui::PlacedChild {
            id: ChildId(id),
            node: scene.node,
            rect: Rect::from_min_max(Pos2::new(20.0, 40.0), Pos2::new(76.0, 96.0)),
            inputs: Vec::new(),
            outputs: Vec::new(),
            scene: Box::new(scene),
        }
    }

    fn test_port(id: u32, anchor: Pos2) -> PlacedPort {
        PlacedPort {
            id: PortId(id),
            source_gate: (NodeId(7), GateId(9)),
            anchor,
            location: PortLocation { x: 0, y: 0 },
            label: String::new(),
        }
    }
}

fn inset_rect(rect: Rect, inset: f32) -> Rect {
    Rect::from_min_max(
        rect.min + egui::vec2(inset, inset),
        rect.max - egui::vec2(inset, inset),
    )
}
