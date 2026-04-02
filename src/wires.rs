use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use egui_winit::winit;
use winit::dpi::PhysicalSize;

use crate::{
    render::CameraState,
    wire_render::{StoredWireEdge, WireEndpointId},
};

const INITIAL_SEGMENT_CAPACITY: usize = 16;
const WIRE_THICKNESS_PX: f32 = 5.0;
const MIN_POINT_DELTA: f32 = 0.01;
pub const DEFAULT_WIRE_COLOR: [f32; 4] = [0.20, 0.48, 0.82, 1.0];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WireRenderParams {
    view: [f32; 4],
    surface: [f32; 4],
    board: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WireSegmentInstance {
    start: [f32; 2],
    end: [f32; 2],
    source_coord: [u32; 4],
    path: [f32; 4],
    color: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GridCell {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WirePoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct DraftWire {
    pub arena_z: u32,
    pub source: GridCell,
    pub points: Vec<WirePoint>,
}

#[derive(Clone)]
struct AestheticWire {
    arena_z: u32,
    source: GridCell,
    points: Vec<WirePoint>,
    color: [f32; 4],
}

pub struct WireOverlay {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    segment_buffer: wgpu::Buffer,
    segment_capacity: usize,
    segment_count: u32,
    surface_size: PhysicalSize<u32>,
    board_size: [u32; 2],
    wires: Vec<AestheticWire>,
    thickness_px: f32,
    visible_arena_z: u32,
    draft_arena_z: u32,
    draft_source: Option<GridCell>,
    draft_points: Vec<WirePoint>,
    draft_color: [f32; 4],
    hover_point: Option<WirePoint>,
}

impl WireOverlay {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        surface_size: PhysicalSize<u32>,
        board_size: [u32; 2],
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wire-overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("wires.wgsl").into()),
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wire-overlay-params"),
            size: std::mem::size_of::<WireRenderParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let segment_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wire-overlay-segments"),
            size: (INITIAL_SEGMENT_CAPACITY * std::mem::size_of::<WireSegmentInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
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

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wire-overlay-bind-group-layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
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
            label: Some("wire-overlay-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WireSegmentInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2,
                1 => Float32x2,
                2 => Uint32x4,
                3 => Float32x4,
                4 => Float32x4
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire-overlay-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_layout],
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
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let overlay = Self {
            pipeline,
            bind_group_layout,
            params_buffer,
            segment_buffer,
            segment_capacity: INITIAL_SEGMENT_CAPACITY,
            segment_count: 0,
            surface_size,
            board_size,
            wires: Vec::new(),
            thickness_px: effective_wire_thickness_px(
                CameraState::new(surface_size),
                surface_size,
                board_size,
            ),
            visible_arena_z: 0,
            draft_arena_z: 0,
            draft_source: None,
            draft_points: Vec::new(),
            draft_color: DEFAULT_WIRE_COLOR,
            hover_point: None,
        };

        overlay.update_view(
            queue,
            camera_params(CameraState::new(surface_size), surface_size, board_size),
        );
        overlay
    }

    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: CameraState,
        surface_size: PhysicalSize<u32>,
    ) {
        self.surface_size = surface_size;
        let thickness_px = effective_wire_thickness_px(camera, surface_size, self.board_size);
        self.update_view(queue, camera_params(camera, surface_size, self.board_size));
        if (self.thickness_px - thickness_px).abs() > f32::EPSILON {
            self.thickness_px = thickness_px;
            self.sync_segments(device, queue);
        }
    }

    pub fn update_camera(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: CameraState,
    ) {
        let thickness_px = effective_wire_thickness_px(camera, self.surface_size, self.board_size);
        self.update_view(
            queue,
            camera_params(camera, self.surface_size, self.board_size),
        );
        if (self.thickness_px - thickness_px).abs() > f32::EPSILON {
            self.thickness_px = thickness_px;
            self.sync_segments(device, queue);
        }
    }

    pub fn set_draft_color(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, color: [f32; 4]) {
        if self.draft_color == color {
            return;
        }

        self.draft_color = color;
        self.sync_segments(device, queue);
    }

    pub fn set_visible_arena_z(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        arena_z: u32,
    ) {
        if self.visible_arena_z == arena_z {
            return;
        }

        self.visible_arena_z = arena_z;
        self.sync_segments(device, queue);
    }

    pub fn update_hover(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        hover_point: Option<WirePoint>,
    ) {
        self.hover_point = hover_point;
        self.sync_segments(device, queue);
    }

    pub fn has_draft(&self) -> bool {
        !self.draft_points.is_empty()
    }

    pub fn current_draft(&self) -> Option<DraftWire> {
        Some(DraftWire {
            arena_z: self.draft_arena_z,
            source: self.draft_source?,
            points: self.draft_points.clone(),
        })
    }

    pub fn restore_draft(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draft: Option<&DraftWire>,
    ) {
        if let Some(draft) = draft {
            self.draft_arena_z = draft.arena_z;
            self.draft_source = Some(draft.source);
            self.draft_points = draft.points.clone();
        } else {
            self.draft_source = None;
            self.draft_points.clear();
        }

        self.sync_segments(device, queue);
    }

    pub fn add_point(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        arena_z: u32,
        point: WirePoint,
        source: GridCell,
    ) {
        self.draft_arena_z = arena_z;
        if self.draft_points.is_empty() {
            self.draft_source = Some(source);
        }

        if self
            .draft_points
            .last()
            .copied()
            .is_some_and(|last| point_distance(last, point) < MIN_POINT_DELTA)
        {
            return;
        }

        self.draft_points.push(point);
        self.sync_segments(device, queue);
    }

    pub fn pop_point(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.draft_points.pop().is_none() {
            return;
        }

        if self.draft_points.is_empty() {
            self.draft_source = None;
        }

        self.sync_segments(device, queue);
    }

    pub fn cancel_draft(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.draft_points.is_empty() {
            return;
        }

        self.draft_points.clear();
        self.draft_source = None;
        self.sync_segments(device, queue);
    }

    pub fn commit_draft(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: [f32; 4],
    ) -> Option<StoredWireEdge> {
        let Some(source) = self.draft_source else {
            return None;
        };
        if self.draft_points.len() < 2 {
            return None;
        }

        let destination = self.draft_points.last().copied().map(|point| GridCell {
            x: point.x.floor() as u32,
            y: point.y.floor() as u32,
        })?;

        let edge = StoredWireEdge {
            source_id: WireEndpointId::from_grid_cell(source, self.draft_arena_z),
            destination_id: WireEndpointId::from_grid_cell(destination, self.draft_arena_z),
            points: self.draft_points.clone(),
            color,
        };
        self.draft_points.clear();
        self.draft_source = None;
        self.sync_segments(device, queue);
        Some(edge)
    }

    pub fn replace_wires(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        wires: Vec<StoredWireEdge>,
    ) {
        self.wires = wires
            .into_iter()
            .map(|wire| AestheticWire {
                arena_z: wire.source_id.arena_z,
                source: wire.source_id.as_grid_cell(),
                points: wire.points,
                color: wire.color,
            })
            .collect();
        self.sync_segments(device, queue);
    }

    pub fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surface_texture: &wgpu::Texture,
        read_charge_view: &wgpu::TextureView,
        write_charge_view: &wgpu::TextureView,
    ) {
        if self.segment_count == 0 {
            return;
        }

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wire-overlay-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(read_charge_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(write_charge_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });

        let output_view = surface_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("wire-overlay-pass"),
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

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, self.segment_buffer.slice(..));
        pass.draw(0..6, 0..self.segment_count);
    }

    fn update_view(&self, queue: &wgpu::Queue, params: WireRenderParams) {
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    fn sync_segments(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let segments = self.build_segments();
        self.ensure_segment_capacity(device, segments.len());
        self.segment_count = segments.len() as u32;

        if segments.is_empty() {
            return;
        }

        queue.write_buffer(&self.segment_buffer, 0, bytemuck::cast_slice(&segments));
    }

    fn ensure_segment_capacity(&mut self, device: &wgpu::Device, needed: usize) {
        if needed <= self.segment_capacity {
            return;
        }

        let mut next_capacity = self.segment_capacity.max(1);
        while next_capacity < needed {
            next_capacity *= 2;
        }

        self.segment_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wire-overlay-segments"),
            size: (next_capacity * std::mem::size_of::<WireSegmentInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.segment_capacity = next_capacity;
    }

    fn build_segments(&self) -> Vec<WireSegmentInstance> {
        let mut segments = Vec::new();

        for wire in &self.wires {
            if wire.arena_z != self.visible_arena_z {
                continue;
            }

            append_segments(
                &mut segments,
                wire.arena_z,
                wire.source,
                &wire.points,
                wire.color,
                self.thickness_px,
            );
        }

        if self.draft_arena_z == self.visible_arena_z {
            if let Some(source) = self.draft_source {
                if self.draft_points.len() >= 2 {
                    append_segments(
                        &mut segments,
                        self.draft_arena_z,
                        source,
                        &self.draft_points,
                        self.draft_color,
                        self.thickness_px,
                    );
                }

                if let (Some(hover), Some(last)) =
                    (self.hover_point, self.draft_points.last().copied())
                {
                    if point_distance(hover, last) >= MIN_POINT_DELTA {
                        let preview = [last, hover];
                        append_segments(
                            &mut segments,
                            self.draft_arena_z,
                            source,
                            &preview,
                            self.draft_color,
                            self.thickness_px,
                        );
                    }
                }
            }
        }

        segments
    }
}

fn append_segments(
    out: &mut Vec<WireSegmentInstance>,
    arena_z: u32,
    source: GridCell,
    points: &[WirePoint],
    color: [f32; 4],
    thickness_px: f32,
) {
    if points.len() < 2 {
        return;
    }

    let total_length: f32 = points
        .windows(2)
        .map(|pair| point_distance(pair[0], pair[1]))
        .sum();
    if total_length <= f32::EPSILON {
        return;
    }

    let source_coord = [source.x, source.y, arena_z, 0];
    let mut path_start = 0.0;

    for pair in points.windows(2) {
        let start = pair[0];
        let end = pair[1];
        let length = point_distance(start, end);
        if length <= f32::EPSILON {
            continue;
        }

        let path_end = path_start + length;
        out.push(WireSegmentInstance {
            start: [start.x, start.y],
            end: [end.x, end.y],
            source_coord,
            path: [
                path_start / total_length,
                path_end / total_length,
                length,
                thickness_px,
            ],
            color,
        });
        path_start = path_end;
    }
}

fn point_distance(a: WirePoint, b: WirePoint) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

fn camera_params(
    camera: CameraState,
    surface_size: PhysicalSize<u32>,
    board_size: [u32; 2],
) -> WireRenderParams {
    let view = camera.view_params();
    WireRenderParams {
        view: [view[0], view[1], camera.offset[0], camera.offset[1]],
        surface: [
            surface_size.width.max(1) as f32,
            surface_size.height.max(1) as f32,
            0.0,
            0.0,
        ],
        board: [board_size[0] as f32, board_size[1] as f32, 0.0, 0.0],
    }
}

fn effective_wire_thickness_px(
    camera: CameraState,
    surface_size: PhysicalSize<u32>,
    board_size: [u32; 2],
) -> f32 {
    let view = camera.view_params();
    let cell_width_px = surface_size.width.max(1) as f32 / (board_size[0].max(1) as f32 * view[0]);
    let cell_height_px =
        surface_size.height.max(1) as f32 / (board_size[1].max(1) as f32 * view[1]);
    WIRE_THICKNESS_PX.min(cell_width_px.min(cell_height_px))
}

pub fn cursor_to_board_point(
    camera: CameraState,
    cursor: [f32; 2],
    board_size: [u32; 2],
) -> Option<WirePoint> {
    let uv = camera.surface_to_world_uv(cursor)?;
    Some(WirePoint {
        x: uv[0] * board_size[0] as f32,
        y: uv[1] * board_size[1] as f32,
    })
}

pub fn snap_cursor_to_cell(
    camera: CameraState,
    cursor: [f32; 2],
    board_size: [u32; 2],
) -> Option<GridCell> {
    let point = cursor_to_board_point(camera, cursor, board_size)?;

    if point.x < 0.0
        || point.y < 0.0
        || point.x >= board_size[0] as f32
        || point.y >= board_size[1] as f32
    {
        return None;
    }

    Some(GridCell {
        x: point.x.floor() as u32,
        y: point.y.floor() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyline_segments_keep_normalized_path_progress() {
        let mut segments = Vec::new();
        append_segments(
            &mut segments,
            2,
            GridCell { x: 1, y: 1 },
            &[
                WirePoint { x: 1.0, y: 1.0 },
                WirePoint { x: 3.0, y: 1.0 },
                WirePoint { x: 3.0, y: 4.0 },
            ],
            DEFAULT_WIRE_COLOR,
            WIRE_THICKNESS_PX,
        );

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].source_coord[..3], [1, 1, 2]);
        assert!((segments[0].path[0] - 0.0).abs() < 0.0001);
        assert!((segments[0].path[1] - 0.4).abs() < 0.0001);
        assert!((segments[1].path[0] - 0.4).abs() < 0.0001);
        assert!((segments[1].path[1] - 1.0).abs() < 0.0001);
    }

    #[test]
    fn wire_thickness_never_exceeds_cell_size() {
        let surface = PhysicalSize::new(1600, 900);
        let fit_camera = CameraState::new(surface);
        assert_eq!(
            effective_wire_thickness_px(fit_camera, surface, [8, 8]),
            WIRE_THICKNESS_PX
        );

        let mut zoomed_out_camera = CameraState::new(surface);
        zoomed_out_camera.zoom = 0.01;
        assert!(
            effective_wire_thickness_px(zoomed_out_camera, surface, [8, 8]) < WIRE_THICKNESS_PX
        );
    }

    #[test]
    fn hover_updates_do_not_retag_active_draft() {
        let crate::windowing::GpuState { device, queue, .. } =
            pollster::block_on(crate::windowing::prepare_gpu(None)).expect("device");
        let mut overlay = WireOverlay::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            PhysicalSize::new(1600, 900),
            [8, 8],
        );

        overlay.add_point(
            &device,
            &queue,
            2,
            WirePoint { x: 1.0, y: 1.0 },
            GridCell { x: 1, y: 1 },
        );
        overlay.add_point(
            &device,
            &queue,
            2,
            WirePoint { x: 3.0, y: 1.0 },
            GridCell { x: 1, y: 1 },
        );

        overlay.set_visible_arena_z(&device, &queue, 1);
        overlay.update_hover(&device, &queue, Some(WirePoint { x: 4.0, y: 1.0 }));

        assert_eq!(overlay.current_draft().unwrap().arena_z, 2);
    }

    #[test]
    fn build_segments_only_includes_visible_arena_z() {
        let crate::windowing::GpuState { device, queue, .. } =
            pollster::block_on(crate::windowing::prepare_gpu(None)).expect("device");
        let mut overlay = WireOverlay::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            PhysicalSize::new(1600, 900),
            [8, 8],
        );

        overlay.replace_wires(
            &device,
            &queue,
            vec![
                StoredWireEdge {
                    source_id: WireEndpointId {
                        x: 1,
                        y: 1,
                        arena_z: 0,
                    },
                    destination_id: WireEndpointId {
                        x: 2,
                        y: 1,
                        arena_z: 0,
                    },
                    points: vec![WirePoint { x: 1.0, y: 1.0 }, WirePoint { x: 2.0, y: 1.0 }],
                    color: DEFAULT_WIRE_COLOR,
                },
                StoredWireEdge {
                    source_id: WireEndpointId {
                        x: 3,
                        y: 3,
                        arena_z: 1,
                    },
                    destination_id: WireEndpointId {
                        x: 4,
                        y: 3,
                        arena_z: 1,
                    },
                    points: vec![WirePoint { x: 3.0, y: 3.0 }, WirePoint { x: 4.0, y: 3.0 }],
                    color: DEFAULT_WIRE_COLOR,
                },
            ],
        );

        overlay.set_visible_arena_z(&device, &queue, 1);

        let segments = overlay.build_segments();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].source_coord[..3], [3, 3, 1]);
    }
}
